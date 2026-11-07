//! One fuzz run: build a fresh world on the instance, drive the app and the
//! second client step by step with faults armed on the proxy, check every
//! invariant after every step, trace it, and shrink a failure.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::app::{App, Claim, Outcome, Proj};
use super::model::{next_step, Act, Fault, FaultSpec, Hint, Rng, Step};
use super::oracle::{check, snapshot, Ghost, Snap, Violation};
use super::proxy::{FaultKind, FaultProxy, FaultRule};
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
    /// The first step that broke an invariant, and what broke.
    pub(super) failure: Option<(usize, Vec<Violation>)>,
    pub(super) setup_error: Option<String>,
    pub(super) elapsed: Duration,
}

/// The fixed part of every run: accounts, folders.
pub(super) struct Harness {
    pub(super) target: FuzzTarget,
    pub(super) server: Server,
    pub(super) cache_root: PathBuf,
    pub(super) config_dir: PathBuf,
    pub(super) copies: PathBuf,
}

/// The run's projects: an owner project the collaborator may write, a project
/// the collaborator owns and shares (the one whose access the second client can
/// take away), and an Ariane-only project to adopt.
struct World {
    projects: Vec<Proj>,
    /// Who administers each project (for the second client's admin steps).
    admin_of: HashMap<String, Who>,
}

fn seed_sections() -> Vec<(String, usize)> {
    vec![
        ("A".to_owned(), 3),
        ("B".to_owned(), 2),
        ("C".to_owned(), 2),
    ]
}

impl Harness {
    fn world(&self, tag: &str) -> Result<World, String> {
        let s = &self.server;
        let own = s.create_stalx_project(Who::Owner, &format!("{tag}-own"), &seed_sections())?;
        s.observe(Who::Owner, &own)?;
        s.grant(Who::Owner, &own, &s.collab.email, Permission::ReadAndWrite)?;
        let shared =
            s.create_stalx_project(Who::Collab, &format!("{tag}-shared"), &seed_sections())?;
        s.grant(
            Who::Collab,
            &shared,
            &s.owner.email,
            Permission::ReadAndWrite,
        )?;
        s.observe(Who::Collab, &shared)?;
        let ariane =
            s.create_ariane_project(Who::Owner, &format!("{tag}-ariane"), &seed_sections())?;
        s.grant(
            Who::Owner,
            &ariane,
            &s.collab.email,
            Permission::ReadAndWrite,
        )?;
        s.observe(Who::Owner, &ariane)?;
        let projects = vec![
            Proj {
                id: own.clone(),
                name: format!("{tag}-own"),
            },
            Proj {
                id: shared.clone(),
                name: format!("{tag}-shared"),
            },
            Proj {
                id: ariane.clone(),
                name: format!("{tag}-ariane"),
            },
        ];
        let admin_of = [
            (own, Who::Owner),
            (shared, Who::Collab),
            (ariane, Who::Owner),
        ]
        .into_iter()
        .collect();
        Ok(World { projects, admin_of })
    }

    /// Wipe the app's folders between runs (the app state is gone by then).
    fn wipe_local(&self) {
        for dir in [&self.cache_root, &self.config_dir, &self.copies] {
            let _ = std::fs::remove_dir_all(dir);
            let _ = std::fs::create_dir_all(dir);
        }
    }

    /// Run `max_steps` steps; stop at the first broken invariant.
    pub(super) fn run(
        &mut self,
        label: &str,
        mut source: Source,
        max_steps: usize,
        trace: Option<&Path>,
    ) -> RunResult {
        let started = Instant::now();
        self.wipe_local();
        let tag = format!(
            "fz-{label}-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        );
        let mut result = RunResult {
            steps: Vec::new(),
            failure: None,
            setup_error: None,
            elapsed: Duration::ZERO,
        };
        let world = match self.world(&tag) {
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
        let other = match OtherServer::start(&self.target.owner.email) {
            Ok(o) => o,
            Err(e) => {
                result.setup_error = Some(format!("other server: {e}"));
                return result;
            }
        };
        let proxy_url = proxy.base_url();
        let other_url = other.base_url();
        let allowed = [proxy_url.as_str(), other_url.as_str()];
        let mut app = App::new(
            proxy_url.clone(),
            other_url.clone(),
            self.target.owner.clone(),
            self.copies.clone(),
            tag.clone(),
        );
        let mut projects = world.projects.clone();
        let mut admin_of = world.admin_of.clone();
        if let Some(path) = trace {
            append_line(
                path,
                &serde_json::json!({
                    "run": label, "tag": tag, "projects": projects,
                    "owner": self.server.owner.email, "collab": self.server.collab.email,
                }),
            );
        }
        let mut ghost = Ghost::default();
        let mut before = self.snap(&app, &projects);
        let mut actor_edits = 0usize;
        for i in 0..max_steps {
            let step = match &mut source {
                Source::Seed { rng, fault_pct } => {
                    let hint = hint(&app, &projects, i);
                    if i == 0 {
                        Step {
                            act: Act::Login,
                            fault: None,
                            then: Vec::new(),
                        }
                    } else if i == 1 {
                        Step {
                            act: Act::Open {
                                p: 0,
                                on_unpushed: super::model::OnUnpushed::KeepLocal,
                            },
                            fault: None,
                            then: Vec::new(),
                        }
                    } else {
                        next_step(rng, hint, *fault_pct)
                    }
                }
                Source::Fixed(list) => match list.get(i) {
                    Some(s) => s.clone(),
                    None => break,
                },
            };
            let t0 = Instant::now();
            let (out, events) = if step.act.is_actor() {
                actor_edits += 1;
                let out = self.actor(&step.act, &projects, &admin_of, &mut ghost, actor_edits);
                (out, Vec::new())
            } else {
                proxy.clear();
                for f in step.fault.iter().chain(&step.then) {
                    proxy.arm(rule(f));
                }
                let out = app.run(&step.act, &projects);
                proxy.clear();
                let _ = proxy.quiesce(Duration::from_mins(1));
                (out, proxy.take_log())
            };
            for c in &out.claims {
                if let Claim::Published(p) = c {
                    // The owner published it: let the oracle read it too.
                    let _ = self.server.observe(Who::Owner, &p.id);
                    admin_of.insert(p.id.clone(), Who::Owner);
                    projects.push(p.clone());
                }
            }
            // The app took the lock afresh: I11 applies again.
            if matches!(step.act, Act::StartEditing | Act::ResumeHeldLock)
                && out
                    .log
                    .iter()
                    .any(|l| l.contains("Ok(Editing)") || l.contains("FastForwarded"))
            {
                ghost.interference.clear();
                ghost.access_interference.clear();
            }
            let after = self.snap(&app, &projects);
            let violations = check(
                &mut ghost,
                &out,
                step.act.is_actor(),
                &before,
                &after,
                &mut self.server,
                &self.target.owner.email,
                &allowed,
                &self.cache_root,
            );
            let violations = self.confirm_orphans(violations, &app, &projects);
            if let Some(path) = trace {
                append_line(
                    path,
                    &serde_json::json!({
                        "i": i, "step": step, "ms": t0.elapsed().as_millis(),
                        "log": out.log, "refusals": out.refusals, "claims": out.claims,
                        "panic": out.panicked,
                        "proxy": events.iter().map(|e| format!(
                            "{} {} -> {:?}{}", e.method, e.path, e.upstream_status,
                            e.fault.as_ref().map(|f| format!(" [{f}]")).unwrap_or_default()
                        )).collect::<Vec<_>>(),
                        "pending": ghost.pending,
                        "violations": violations,
                    }),
                );
            }
            result.steps.push(step);
            if violations.iter().any(|v| v.id == "I8") {
                result.failure = Some((i, violations));
                break;
            }
            if !violations.is_empty() {
                result.failure = Some((i, violations));
                break;
            }
            before = after;
        }
        app.restart();
        drop(app);
        for p in &projects {
            let admin = admin_of.get(&p.id).copied().unwrap_or(Who::Owner);
            let _ = self.server.release(admin, &p.id);
            let _ = self.server.delete(admin, &p.id);
        }
        drop(proxy);
        drop(other);
        result.elapsed = started.elapsed();
        result
    }

    fn snap(&mut self, app: &App, projects: &[Proj]) -> Snap {
        let ids: Vec<String> = projects.iter().map(|p| p.id.clone()).collect();
        snapshot(app, &mut self.server, &ids, &self.cache_root, &self.copies)
    }

    /// An orphan file may be a background write still settling: count it only if
    /// it is still there a moment later.
    fn confirm_orphans(
        &mut self,
        violations: Vec<Violation>,
        app: &App,
        projects: &[Proj],
    ) -> Vec<Violation> {
        if !violations.iter().any(|v| v.id == "I5") {
            return violations;
        }
        std::thread::sleep(Duration::from_millis(1500));
        let again = self.snap(app, projects);
        violations
            .into_iter()
            .filter(|v| {
                v.id != "I5"
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
    fn actor(
        &self,
        act: &Act,
        projects: &[Proj],
        admin_of: &HashMap<String, Who>,
        ghost: &mut Ghost,
        n: usize,
    ) -> Outcome {
        let mut out = Outcome::default();
        let pick = |p: u8| {
            projects
                .get(usize::from(p) % projects.len().max(1))
                .cloned()
        };
        let s = &self.server;
        let answer: Result<String, String> = match act {
            Act::ActorLock { who, p } => pick(*p).map_or(Ok(String::new()), |proj| {
                if *who == Who::Owner {
                    ghost.interference.insert(proj.id.clone());
                }
                s.acquire(*who, &proj.id)
                    .map(|()| format!("{who:?} locked {}", proj.name))
            }),
            Act::ActorRelease { who, p } => pick(*p).map_or(Ok(String::new()), |proj| {
                let r = s.release(*who, &proj.id);
                if r.is_ok() {
                    ghost.interference.insert(proj.id.clone());
                }
                r.map(|()| format!("{who:?} released {}", proj.name))
            }),
            Act::ActorPush { who, p } | Act::ActorPushNewer { who, p } => {
                pick(*p).map_or(Ok(String::new()), |proj| {
                    let newer = matches!(act, Act::ActorPushNewer { .. });
                    if *who == Who::Owner {
                        ghost.interference.insert(proj.id.clone());
                    }
                    s.tip_project(*who, &proj.id).and_then(|mut tip| {
                        tip.add_section(super::world::section(&format!("X{n}"), 2));
                        s.push_project(*who, &proj.id, &tip, newer, "second client")
                            .map(|()| {
                                format!("{who:?} pushed X{n} (newer={newer}) to {}", proj.name)
                            })
                    })
                })
            }
            Act::ActorDelete { p } => pick(*p).map_or(Ok(String::new()), |proj| {
                ghost.interference.insert(proj.id.clone());
                ghost.access_interference.insert(proj.id.clone());
                let admin = admin_of.get(&proj.id).copied().unwrap_or(Who::Owner);
                s.delete(admin, &proj.id)
                    .map(|()| format!("{admin:?} deleted {}", proj.name))
            }),
            Act::ActorRename { p } => pick(*p).map_or(Ok(String::new()), |proj| {
                let admin = admin_of.get(&proj.id).copied().unwrap_or(Who::Owner);
                s.rename(admin, &proj.id, &format!("{}-r{n}", proj.name))
                    .map(|()| format!("renamed {}", proj.name))
            }),
            Act::ActorSetOwnerAccess { p, level } => pick(*p).map_or(Ok(String::new()), |proj| {
                ghost.interference.insert(proj.id.clone());
                ghost.access_interference.insert(proj.id.clone());
                let admin = admin_of.get(&proj.id).copied().unwrap_or(Who::Owner);
                if admin == Who::Owner {
                    return Ok("owner administers it; access unchanged".to_owned());
                }
                let owner = s.owner.email.clone();
                match level {
                    0 => s.revoke(admin, &proj.id, &owner),
                    1 => s.grant(admin, &proj.id, &owner, Permission::ReadOnly),
                    _ => s.grant(admin, &proj.id, &owner, Permission::ReadAndWrite),
                }
                .map(|()| format!("owner access on {} -> level {level}", proj.name))
            }),
            _ => Ok(String::new()),
        };
        match answer {
            Ok(line) => out.log.push(format!("actor: {line}")),
            Err(e) => out.log.push(format!("actor failed: {e}")),
        }
        out
    }
}

fn hint(app: &App, projects: &[Proj], i: usize) -> Hint {
    let (open, linked, dirty, force_editing) = app.hint_bits();
    let _ = i;
    Hint {
        logged_in: app.logged_in(),
        open,
        linked,
        editing: app.editing(),
        dirty,
        force_editing,
        projects: u8::try_from(projects.len()).unwrap_or(u8::MAX),
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
        Fault::HangMs(ms) => FaultKind::HangThenForward(Duration::from_millis(*ms)),
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
            if lo >= cur.len() {
                break;
            }
            let hi = (lo + chunk).min(cur.len());
            let cand: Vec<Step> = cur
                .iter()
                .enumerate()
                .filter(|(j, _)| *j < lo || *j >= hi)
                .map(|(_, s)| s.clone())
                .collect();
            if tries >= budget {
                break;
            }
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
