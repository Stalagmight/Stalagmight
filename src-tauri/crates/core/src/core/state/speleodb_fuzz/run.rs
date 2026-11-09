//! One fuzz run: build a fresh world on the instance, drive the app and the
//! second client step by step with faults armed on the proxy, check every
//! invariant after every step, trace it, and shrink a failure.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::app::{Accounts, App, Claim, Outcome, Proj};
use super::gen::{next_step, Hint};
use super::model::{Acct, Act, Fault, FaultSpec, Rng, Step};
use super::oracle::{check, violation, Emails, Ghost, StepCtx, Violation};
use super::proxy::{FaultKind, FaultProxy, FaultRule};
use super::snap::{snapshot, Snap};
use super::stub::OtherServer;
use super::target::FuzzTarget;
use super::world::{append_line, Server, Who};
use crate::speleodb::Permission;

/// Where a run's steps come from.
pub(super) enum Source {
    /// Generated from a seed, `fault_pct` % of app steps with a fault.
    Seed { rng: Rng, fault_pct: u64 },
    /// A recorded list (a replay or a shrink candidate).
    Fixed(Vec<Step>),
}

/// What a run did.
pub(super) struct RunResult {
    pub(super) steps: Vec<Step>,
    /// The first step that broke an unmuted invariant, and what broke.
    pub(super) failure: Option<(usize, Vec<Violation>)>,
    /// Muted violations: `(step, id)`, first occurrence of each id.
    pub(super) muted: Vec<(usize, String)>,
    pub(super) setup_error: Option<String>,
    pub(super) elapsed: Duration,
}

/// The fixed part of every run: accounts, folders, knobs.
pub(super) struct Harness {
    pub(super) target: FuzzTarget,
    pub(super) server: Server,
    pub(super) cache_root: PathBuf,
    pub(super) copies: PathBuf,
    /// Invariant ids (or id prefixes) whose violations do not fail a seed.
    pub(super) mute: Vec<String>,
    /// Act names the generator never draws.
    pub(super) skip_acts: Vec<String>,
}

/// The run's projects and who administers each.
struct World {
    projects: Vec<Proj>,
    admin_of: HashMap<String, Who>,
    legacy: HashSet<String>,
}

fn sections(spec: &[(&str, usize)]) -> Vec<(String, usize)> {
    spec.iter().map(|(n, s)| ((*n).to_owned(), *s)).collect()
}

pub(super) fn who(acct: Acct) -> Who {
    match acct {
        Acct::Owner => Who::Owner,
        Acct::Collab => Who::Collab,
        Acct::Ro => Who::Ro,
        Acct::Admin => Who::Admin,
    }
}

pub(super) fn is_muted(mute: &[String], id: &str) -> bool {
    mute.iter()
        .any(|m| !m.is_empty() && (id == m || id.starts_with(m.as_str())))
}

impl Harness {
    /// Three STALAGMIGHT projects: one the owner administers (the collaborator is
    /// an admin too, so it can force a release), one the collaborator owns and
    /// shares (it can take the owner's access away), a second owner project with
    /// other content (switches); plus, when the server accepts it, a project
    /// published the way v0.5.x builds did (generic upload).
    fn world(&self, tag: &str, log: &mut Vec<String>) -> Result<World, String> {
        let s = &self.server;
        let base = sections(&[("A", 3), ("B", 2), ("C", 2)]);
        let own = s.create_project(Who::Owner, &format!("{tag}-own"), &base)?;
        s.observe(Who::Owner, &own)?;
        s.grant(Who::Owner, &own, s.email(Who::Collab), Permission::Admin)?;
        let shared = s.create_project(Who::Collab, &format!("{tag}-shared"), &base)?;
        s.grant(
            Who::Collab,
            &shared,
            s.email(Who::Owner),
            Permission::ReadAndWrite,
        )?;
        s.observe(Who::Collab, &shared)?;
        let own2 = s.create_project(
            Who::Owner,
            &format!("{tag}-own2"),
            &sections(&[("A", 3), ("B", 2), ("C", 2), ("D", 4)]),
        )?;
        s.observe(Who::Owner, &own2)?;
        s.grant(
            Who::Owner,
            &own2,
            s.email(Who::Collab),
            Permission::ReadAndWrite,
        )?;
        if s.has(Who::Ro) {
            for (admin, id) in [
                (Who::Owner, &own),
                (Who::Collab, &shared),
                (Who::Owner, &own2),
            ] {
                s.grant(admin, id, s.email(Who::Ro), Permission::ReadOnly)?;
            }
        }
        let mut projects = vec![
            Proj {
                id: own.clone(),
                name: format!("{tag}-own"),
            },
            Proj {
                id: shared.clone(),
                name: format!("{tag}-shared"),
            },
            Proj {
                id: own2.clone(),
                name: format!("{tag}-own2"),
            },
        ];
        let mut admin_of: HashMap<String, Who> =
            [(own, Who::Owner), (shared, Who::Collab), (own2, Who::Owner)]
                .into_iter()
                .collect();
        let mut legacy = HashSet::new();
        match s.create_legacy_project(Who::Owner, &format!("{tag}-legacy"), &base) {
            Ok(id) => {
                let _ = s.observe(Who::Owner, &id);
                let _ = s.grant(
                    Who::Owner,
                    &id,
                    s.email(Who::Collab),
                    Permission::ReadAndWrite,
                );
                admin_of.insert(id.clone(), Who::Owner);
                legacy.insert(id.clone());
                projects.push(Proj {
                    id,
                    name: format!("{tag}-legacy"),
                });
            }
            Err(e) => log.push(format!(
                "legacy layout unavailable: {}",
                super::app::clip(&e)
            )),
        }
        Ok(World {
            projects,
            admin_of,
            legacy,
        })
    }

    /// Wipe the app's folders between runs.
    fn wipe_local(&self) {
        for dir in [&self.cache_root, &self.copies] {
            let _ = std::fs::remove_dir_all(dir);
            let _ = std::fs::create_dir_all(dir);
        }
    }

    fn emails(&self) -> Emails {
        let low = |w: Who| self.server.email(w).to_ascii_lowercase();
        Emails {
            owner: low(Who::Owner),
            collab: low(Who::Collab),
            ro: self.server.has(Who::Ro).then(|| low(Who::Ro)),
        }
    }

    fn snap(&mut self, app: &App, projects: &[Proj]) -> Snap {
        let ids: Vec<String> = projects.iter().map(|p| p.id.clone()).collect();
        snapshot(app, &mut self.server, &ids, &self.cache_root, &self.copies)
    }

    /// Run `max_steps` steps; stop at the first broken (unmuted) invariant.
    pub(super) fn run(
        &mut self,
        label: &str,
        mut source: Source,
        max_steps: usize,
        trace: Option<&Path>,
    ) -> RunResult {
        let started = Instant::now();
        let _config = crate::test_helpers::isolated_config();
        self.wipe_local();
        let tag = format!(
            "fz-{label}-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        );
        let mut result = RunResult {
            steps: Vec::new(),
            failure: None,
            muted: Vec::new(),
            setup_error: None,
            elapsed: Duration::ZERO,
        };
        let mut setup_log = Vec::new();
        let world = match self.world(&tag, &mut setup_log) {
            Ok(w) => w,
            Err(e) => {
                result.setup_error = Some(e);
                return result;
            }
        };
        let proxy = match FaultProxy::start(self.target.upstream) {
            Ok(p) => p,
            Err(e) => {
                result.setup_error = Some(format!("proxy: {e}"));
                return result;
            }
        };
        let other = match OtherServer::start(self.server.email(Who::Owner)) {
            Ok(o) => o,
            Err(e) => {
                result.setup_error = Some(format!("other server: {e}"));
                return result;
            }
        };
        let allowed = vec![proxy.base_url(), other.base_url()];
        let app = Arc::new(App::new(
            proxy.base_url(),
            other.base_url(),
            Accounts {
                owner: self.target.owner.clone(),
                collab: self.target.collab.clone(),
                ro: self.target.ro.clone(),
            },
            self.copies.clone(),
            self.cache_root.clone(),
            tag.clone(),
        ));
        let emails = self.emails();
        let mut projects = world.projects.clone();
        let mut admin_of = world.admin_of.clone();
        if let Some(path) = trace {
            append_line(
                path,
                &serde_json::json!({
                    "run": label, "tag": tag, "projects": projects,
                    "legacy": world.legacy, "setup": setup_log,
                }),
            );
        }
        let mut ghost = Ghost::default();
        let mut actor_shas: HashSet<String> = HashSet::new();
        let mut before = self.snap(&app, &projects);
        let mut muted_ids: BTreeSet<String> = BTreeSet::new();
        for i in 0..max_steps {
            let step = match &mut source {
                Source::Seed { rng, fault_pct } => match i {
                    0 => Step::plain(Act::Login { acct: Acct::Owner }),
                    1 => Step::plain(Act::Open {
                        p: 0,
                        take_server: false,
                    }),
                    _ => {
                        let hint = self.hint(&app, &projects);
                        next_step(rng, hint, *fault_pct, &self.skip_acts)
                    }
                },
                Source::Fixed(list) => match list.get(i) {
                    Some(s) => s.clone(),
                    None => break,
                },
            };
            let t0 = Instant::now();
            let mut extra: Vec<Violation> = Vec::new();
            let mut hung = false;
            let (out, events) = match &step.act {
                act if act.is_actor() => {
                    let out = self.actor(
                        act,
                        &projects,
                        &admin_of,
                        &app,
                        &mut ghost,
                        &mut actor_shas,
                        i,
                        &mut extra,
                    );
                    (out, Vec::new())
                }
                Act::Interleave {
                    first,
                    hold,
                    second,
                } => {
                    proxy.clear();
                    let server = &self.server;
                    let holder_of = |pid: &str| {
                        server
                            .views(&[pid.to_owned()])
                            .remove(pid)
                            .and_then(|v| v.lock_holder)
                    };
                    let r = super::interleave::run(
                        &app, &proxy, first, hold, second, &projects, &holder_of,
                    );
                    hung = r.hung;
                    extra.extend(r.violations);
                    proxy.clear();
                    settle(&proxy);
                    (r.out, proxy.take_log())
                }
                act => {
                    proxy.clear();
                    for f in step.fault.iter().chain(&step.then) {
                        proxy.arm(rule(f));
                    }
                    let (out, h) = run_app(&app, act, &projects);
                    hung = h;
                    if hung {
                        extra.push(violation("HANG", format!("{} never returned", act.name())));
                    }
                    proxy.clear();
                    settle(&proxy);
                    (out, proxy.take_log())
                }
            };
            for c in &out.claims {
                if let Claim::Published(p) = c {
                    let _ = self.server.observe(Who::Owner, &p.id);
                    admin_of.insert(p.id.clone(), Who::Owner);
                    if !projects.iter().any(|q| q.id == p.id) {
                        projects.push(p.clone());
                    }
                }
            }
            // R2: a project published the way v0.5.x builds did must open.
            if let Act::Open { p, .. } = &step.act {
                let proj = projects.get(usize::from(*p) % projects.len().max(1));
                if proj.is_some_and(|q| world.legacy.contains(&q.id))
                    && out.refusals.iter().any(|r| r.status == Some(422))
                {
                    extra.push(violation(
                        "R2-legacy-native-422",
                        "a project published through the generic upload (v0.5.x) fails its native download with 422".to_owned(),
                    ));
                }
            }
            let after = self.snap(&app, &projects);
            let newer_shas: BTreeSet<(String, String)> = after
                .server
                .iter()
                .flat_map(|(id, v)| v.commits.iter().map(move |c| (id.clone(), c.clone())))
                .filter(|(id, c)| self.server.is_newer(id, c))
                .collect();
            let cx = StepCtx {
                act: &step.act,
                out: &out,
                events: &events,
                before: &before,
                after: &after,
                emails: &emails,
                allowed: &allowed,
                cache_root: &self.cache_root,
                newer_shas: &newer_shas,
            };
            let mut violations = check(&mut ghost, &cx, &mut self.server);
            violations.extend(extra);
            let violations = self.confirm_orphans(violations, &app, &projects);
            let (muted, failing): (Vec<Violation>, Vec<Violation>) = violations
                .into_iter()
                .partition(|v| is_muted(&self.mute, &v.id) && v.id != "ENDPOINT");
            for m in &muted {
                if muted_ids.insert(m.id.clone()) {
                    result.muted.push((i, m.id.clone()));
                }
            }
            if let Some(path) = trace {
                append_line(
                    path,
                    &serde_json::json!({
                        "i": i, "step": step, "ms": t0.elapsed().as_millis(),
                        "log": out.log, "refusals": out.refusals, "claims": out.claims,
                        "panic": out.panicked,
                        "proxy": events.iter().map(|e| format!(
                            "{} {} -> {:?}{}{}", e.method, e.path, e.upstream_status,
                            e.fault.as_ref().map(|f| format!(" [{f}]")).unwrap_or_default(),
                            if e.delivered { "" } else { " (undelivered)" }
                        )).collect::<Vec<_>>(),
                        "pending": ghost.pending,
                        "violations": failing, "muted": muted,
                    }),
                );
            }
            result.steps.push(step);
            if !failing.is_empty() {
                result.failure = Some((i, failing));
                break;
            }
            if hung {
                break;
            }
            before = after;
        }
        app.kill();
        proxy.clear();
        drop(app);
        for p in &projects {
            let admin = admin_of.get(&p.id).copied().unwrap_or(Who::Owner);
            for w in [Who::Owner, Who::Collab] {
                let _ = self.server.release(w, &p.id);
            }
            let _ = self.server.delete(admin, &p.id);
        }
        drop(proxy);
        drop(other);
        result.elapsed = started.elapsed();
        result
    }

    fn hint(&self, app: &App, projects: &[Proj]) -> Hint {
        let link = app.open_link();
        Hint {
            logged_in: app.session().logged_in,
            open: app.open_path().is_some(),
            linked: link.is_some(),
            editing: app.editing(),
            dirty: link.as_ref().is_some_and(|l| l.has_unpushed_edits),
            unsaved: app.front().unsaved,
            projects: u8::try_from(projects.len()).unwrap_or(u8::MAX),
            has_ro: self.server.has(Who::Ro),
        }
    }

    /// An orphan file may be a background write still settling: count it only if
    /// it is still there a moment later.
    fn confirm_orphans(
        &mut self,
        violations: Vec<Violation>,
        app: &App,
        projects: &[Proj],
    ) -> Vec<Violation> {
        if !violations.iter().any(|v| v.id == "CA-ORPHAN") {
            return violations;
        }
        std::thread::sleep(Duration::from_millis(1500));
        let again = self.snap(app, projects);
        violations
            .into_iter()
            .filter(|v| {
                v.id != "CA-ORPHAN"
                    || again.cache_files.iter().any(|f| {
                        v.detail.ends_with(
                            &f.strip_prefix(&self.cache_root)
                                .unwrap_or(f)
                                .display()
                                .to_string(),
                        )
                    })
            })
            .collect()
    }

    /// The second client's step (direct to the server).
    #[expect(
        clippy::too_many_arguments,
        reason = "the run's whole context, passed once"
    )]
    fn actor(
        &self,
        act: &Act,
        projects: &[Proj],
        admin_of: &HashMap<String, Who>,
        app: &App,
        ghost: &mut Ghost,
        actor_shas: &mut HashSet<String>,
        n: usize,
        extra: &mut Vec<Violation>,
    ) -> Outcome {
        let mut out = Outcome::default();
        let pick = |p: u8| {
            projects
                .get(usize::from(p) % projects.len().max(1))
                .cloned()
        };
        let s = &self.server;
        let app_who = app.account().and_then(|e| {
            [Who::Owner, Who::Collab, Who::Ro]
                .into_iter()
                .find(|w| s.email(*w).eq_ignore_ascii_case(&e))
        });
        let answer: Result<String, String> = match act {
            Act::ActorLock { who: w, p } => pick(*p).map_or(Ok(String::new()), |proj| {
                ghost.unverified.insert(proj.id.clone());
                s.acquire(who(*w), &proj.id)
                    .map(|()| format!("{w:?} locked {}", proj.name))
            }),
            Act::ActorRelease { who: w, p } => pick(*p).map_or(Ok(String::new()), |proj| {
                ghost.unverified.insert(proj.id.clone());
                s.release(who(*w), &proj.id)
                    .map(|()| format!("{w:?} released {}", proj.name))
            }),
            Act::ActorPush { who: w, p, .. } | Act::ActorPushNewer { who: w, p }
                if !matches!(w, Acct::Admin) =>
            {
                let keep = matches!(
                    act,
                    Act::ActorPush {
                        keep_lock: true,
                        ..
                    }
                );
                let newer = matches!(act, Act::ActorPushNewer { .. });
                pick(*p).map_or(Ok(String::new()), |proj| {
                    ghost.unverified.insert(proj.id.clone());
                    let pushed = s.tip_project(who(*w), &proj.id).and_then(|mut tip| {
                        tip.add_section(super::data::section(&format!("X{n}"), 2));
                        s.push(who(*w), &proj.id, &tip, newer, keep, "second client")
                    });
                    match pushed {
                        Ok(sha) => {
                            actor_shas.insert(sha.clone());
                            Ok(format!("{w:?} pushed X{n} (newer={newer}, keep_lock={keep}) to {} as {sha}", proj.name))
                        }
                        Err(e) => {
                            if ghost.long_message.contains(&proj.id) && e.contains("500") {
                                extra.push(violation(
                                    "P2-long-message-poisons",
                                    format!("{}: an upload failed with 500 after an over-long commit message landed: {}", proj.id, super::app::clip(&e)),
                                ));
                            }
                            Err(e)
                        }
                    }
                })
            }
            Act::ActorDelete { p } => pick(*p).map_or(Ok(String::new()), |proj| {
                ghost.unverified.insert(proj.id.clone());
                ghost.access_changed.insert(proj.id.clone());
                let admin = admin_of.get(&proj.id).copied().unwrap_or(Who::Owner);
                s.delete(admin, &proj.id)
                    .map(|()| format!("{admin:?} deleted {}", proj.name))
            }),
            Act::ActorRename { p } => pick(*p).map_or(Ok(String::new()), |proj| {
                let admin = admin_of.get(&proj.id).copied().unwrap_or(Who::Owner);
                s.rename(admin, &proj.id, &format!("{}-r{n}", proj.name))
                    .map(|()| format!("renamed {}", proj.name))
            }),
            Act::ActorSetAccess { p, level } => pick(*p).map_or(Ok(String::new()), |proj| {
                let admin = admin_of.get(&proj.id).copied().unwrap_or(Who::Owner);
                let Some(target) = app_who.filter(|w| *w != admin) else {
                    return Ok(
                        "the app's account administers it (or none is signed in); unchanged"
                            .to_owned(),
                    );
                };
                ghost.unverified.insert(proj.id.clone());
                ghost.access_changed.insert(proj.id.clone());
                let email = s.email(target).to_owned();
                match level {
                    0 => s.revoke(admin, &proj.id, &email),
                    1 => s.grant(admin, &proj.id, &email, Permission::ReadOnly),
                    _ => s.grant(admin, &proj.id, &email, Permission::ReadAndWrite),
                }
                .map(|()| format!("{target:?} access on {} -> level {level}", proj.name))
            }),
            _ => Ok(String::new()),
        };
        match answer {
            Ok(line) => out.note(format!("actor: {line}")),
            Err(e) => out.note(format!("actor failed: {e}")),
        }
        out
    }
}

/// Run one app step on a thread of its own, so a verb that never returns ends
/// the run instead of the process.
fn run_app(app: &Arc<App>, act: &Act, projects: &[Proj]) -> (Outcome, bool) {
    let (tx, rx) = std::sync::mpsc::channel();
    let (worker, act, projects) = (Arc::clone(app), act.clone(), projects.to_vec());
    std::thread::spawn(move || {
        let _config = worker.config.adopt();
        let _ = tx.send(worker.run(&act, &projects));
    });
    match rx.recv_timeout(super::interleave::VERB_BUDGET) {
        Ok(out) => (out, false),
        Err(_) => (Outcome::default(), true),
    }
}

/// Wait for the proxy to go quiet, including a detached release the step
/// started on its way out.
fn settle(proxy: &FaultProxy) {
    for _ in 0..3 {
        std::thread::sleep(Duration::from_millis(250));
        if proxy.quiesce(Duration::from_secs(60)) {
            break;
        }
    }
}

fn method(m: &str) -> Option<&'static str> {
    ["GET", "POST", "PUT", "PATCH", "DELETE"]
        .into_iter()
        .find(|k| k.eq_ignore_ascii_case(m))
}

/// The proxy rule for a recorded fault.
pub(super) fn rule(f: &FaultSpec) -> FaultRule {
    let kind = match &f.fault {
        Fault::Status(s) => FaultKind::Status(*s),
        Fault::StatusAfter(s) => FaultKind::StatusAfter(*s),
        Fault::LatencyMs(ms) => FaultKind::Latency(Duration::from_millis(*ms)),
        Fault::DropBefore => FaultKind::DropBeforeForward,
        Fault::DropAfter => FaultKind::DropAfterForward,
        Fault::Truncate => FaultKind::TruncateBody,
        Fault::Corrupt => FaultKind::CorruptBody,
        Fault::Scramble => FaultKind::ScrambleBody,
        Fault::HangMs(ms) => FaultKind::HangThenForward(Duration::from_millis(*ms)),
        Fault::Refuse => FaultKind::Refuse,
    };
    FaultRule {
        method: f.method.as_deref().and_then(method),
        path_contains: f.path.clone(),
        kind,
        times: f.times,
    }
}

/// Delta-debug `steps` down to a shorter list that still breaks `id` (within
/// `budget` replays). Faults are then dropped one by one where they are not needed.
pub(super) fn shrink(
    harness: &mut Harness,
    steps: Vec<Step>,
    id: &str,
    budget: usize,
    log: &mut dyn FnMut(&str),
) -> Vec<Step> {
    let mut tries = 0usize;
    let fails = |h: &mut Harness, cand: &[Step], tries: &mut usize| -> bool {
        *tries += 1;
        let r = h.run("shrink", Source::Fixed(cand.to_vec()), cand.len(), None);
        r.failure.is_some_and(|(_, v)| v.iter().any(|x| x.id == id))
    };
    let mut cur = steps;
    let mut n = 2usize;
    while cur.len() >= 2 && tries < budget {
        let chunk = cur.len().div_ceil(n);
        let mut reduced = false;
        let parts = n;
        for k in 0..parts {
            let lo = k * chunk;
            if lo >= cur.len() || tries >= budget {
                break;
            }
            let hi = (lo + chunk).min(cur.len());
            let cand: Vec<Step> = cur
                .iter()
                .enumerate()
                .filter(|(j, _)| *j < lo || *j >= hi)
                .map(|(_, s)| s.clone())
                .collect();
            if fails(harness, &cand, &mut tries) {
                log(&format!(
                    "shrink: {} -> {} steps ({tries} replays)",
                    cur.len(),
                    cand.len()
                ));
                cur = cand;
                n = n.saturating_sub(1).max(2);
                reduced = true;
                break;
            }
        }
        if !reduced {
            if n >= cur.len() {
                break;
            }
            n = (n * 2).min(cur.len());
        }
    }
    for k in 0..cur.len() {
        if tries >= budget {
            break;
        }
        if cur.get(k).is_some_and(|s| s.fault.is_some()) {
            let mut cand = cur.clone();
            if let Some(s) = cand.get_mut(k) {
                s.fault = None;
            }
            if fails(harness, &cand, &mut tries) {
                log(&format!("shrink: fault on step {k} not needed"));
                cur = cand;
            }
        }
    }
    cur
}

#[test]
fn mute_matches_ids_and_prefixes() {
    let mute = vec!["R4".to_owned(), "LK-EXIT-quit".to_owned(), String::new()];
    assert!(is_muted(&mute, "R4-main-thread-block"));
    assert!(is_muted(&mute, "LK-EXIT-quit"));
    assert!(!is_muted(&mute, "LK-EXIT-close"));
    assert!(!is_muted(&[], "R4-main-thread-block"));
}

#[test]
fn every_fault_maps_to_a_proxy_rule() {
    for fault in [
        Fault::Status(500),
        Fault::StatusAfter(502),
        Fault::LatencyMs(5),
        Fault::DropBefore,
        Fault::DropAfter,
        Fault::Truncate,
        Fault::Corrupt,
        Fault::Scramble,
        Fault::HangMs(6_000),
        Fault::Refuse,
    ] {
        let r = rule(&FaultSpec {
            fault,
            method: Some("put".to_owned()),
            path: "/upload/stalagmight/".to_owned(),
            times: 2,
        });
        assert_eq!(r.method, Some("PUT"));
        assert_eq!(r.times, 2);
    }
}
