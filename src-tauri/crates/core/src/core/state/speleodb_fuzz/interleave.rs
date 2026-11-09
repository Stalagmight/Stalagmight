//! Two app verbs at once. The first verb's bare backend call runs on a worker
//! thread while one of its requests (or replies) is parked at the proxy; the
//! second verb's whole flow runs on another thread; then the parked request is
//! let go (or, when the second verb ends the process, dropped as the dying
//! process would drop it).
//!
//! - **ST-LATE** after a session move (sign-out, sign-in, open, close, a new local
//!   project), the first verb's late reply changed the app: its session, open
//!   project, edit gate, link (BASE, dirty), memory or a cache file.
//!   **R7-late-revision-install** when the late reply was a revision View or
//!   Revert (or an edit made while it was in flight was then lost).
//! - **ST-CRED** a sign-in whose reply landed after a sign-out signed the app back
//!   in, or saved its token.
//! - **R4-main-thread-block** Close (`close_project`) or a config update, both
//!   synchronous main-thread commands, did not return within
//!   [`MAIN_THREAD_BUDGET`] while the first verb's network span was held.
//!   **P2-project-mutex-block** the same for a read or an edit of the open
//!   project (`get_project_state`, an edit command).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::app::{App, Outcome, Proj};
use super::flows;
use super::model::{Act, HoldSpec};
use super::oracle::{violation, Violation};
use super::proxy::{FaultKind, FaultProxy, FaultRule};
use super::snap::{app_view, hash_bytes, walk, AppView};

/// How long a synchronous main-thread command may wait before the app counts
/// as frozen.
pub(super) const MAIN_THREAD_BUDGET: Duration = Duration::from_secs(2);
/// How long the first verb gets to reach its held request.
const REACH_HOLD: Duration = Duration::from_secs(20);
/// How long any verb may take before the step counts as hung.
pub(super) const VERB_BUDGET: Duration = Duration::from_secs(240);

/// The app's local side, for comparing before and after a late reply.
#[derive(Clone, Debug)]
struct LocalView {
    app: AppView,
    hashes: BTreeMap<PathBuf, u64>,
}

fn local_view(app: &App) -> LocalView {
    let mut files = Vec::new();
    walk(&app.cache_root, &mut files);
    let hashes = files
        .into_iter()
        .filter(|p| p.file_name().is_some_and(|n| n == "project.stalx"))
        .map(|p| {
            let h = std::fs::read(&p).map_or(0, |b| hash_bytes(&b));
            (p, h)
        })
        .collect();
    LocalView {
        app: app_view(app),
        hashes,
    }
}

/// What changed between two local views, for a violation's detail.
fn changes(mid: &LocalView, after: &LocalView) -> Vec<String> {
    let (a, b) = (&mid.app, &after.app);
    let mut c = Vec::new();
    if (a.logged_in, &a.email, &a.endpoint) != (b.logged_in, &b.email, &b.endpoint) {
        c.push(format!(
            "session {:?} -> {:?}",
            (a.logged_in, &a.email),
            (b.logged_in, &b.email)
        ));
    }
    if a.open_path != b.open_path {
        c.push(format!(
            "open project {:?} -> {:?}",
            a.open_path.as_deref().map(super::app::short),
            b.open_path.as_deref().map(super::app::short)
        ));
    }
    if a.gate != b.gate {
        c.push(format!("edit gate {} -> {}", a.gate, b.gate));
    }
    let key = |v: &AppView| {
        v.link.as_ref().map(|l| {
            (
                l.project_id.clone(),
                l.base_commit_sha.clone(),
                l.has_unpushed_edits,
            )
        })
    };
    if key(a) != key(b) {
        c.push(format!("link {:?} -> {:?}", key(a), key(b)));
    }
    if a.mem != b.mem {
        c.push("the open project's content".to_owned());
    }
    if a.saved_token != b.saved_token {
        c.push(format!(
            "saved token {} -> {}",
            a.saved_token, b.saved_token
        ));
    }
    for (path, h) in &after.hashes {
        if mid.hashes.get(path).is_some_and(|m| m != h) {
            c.push(format!("cache file {} rewritten", short_dir(path)));
        }
    }
    c
}

fn short_dir(path: &Path) -> String {
    path.parent()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The first verb's bare backend call (no frontend continuation: a newer
/// transition drops it).
fn bare(app: &App, act: &Act, projects: &[Proj], out: &mut Outcome) {
    let pick = |p: u8| {
        projects
            .get(usize::from(p) % projects.len().max(1))
            .cloned()
    };
    let st = app.st();
    match act {
        Act::SaveChanges { .. } => {
            if flows::save(app, None, out) {
                match st.speleodb_push_only_acknowledged(Some("fuzz held upload".to_owned()), false)
                {
                    Ok(r) => out.note(format!("push(held) -> Ok({r:?})")),
                    Err(e) => out.refused("push(held)", &e),
                }
            }
        }
        Act::StartEditing => match st.speleodb_start_editing() {
            Ok(r) => out.note(format!("start_editing(held) -> Ok({r:?})")),
            Err(e) => out.refused("start_editing(held)", &e),
        },
        Act::Open { p, .. } => {
            if let Some(proj) = pick(*p) {
                match st.speleodb_open_project(proj.id, proj.name, None) {
                    Ok(r) => out.note(format!("open_project(held) -> Ok(offline={:?})", r.offline)),
                    Err(e) => out.refused("open_project(held)", &e),
                }
            }
        }
        Act::View { back } | Act::Revert { back } => {
            let restore = matches!(act, Act::Revert { .. });
            let Some(link) = app.open_link() else { return };
            let revisions = match st.speleodb_get_revisions(link.project_id.clone()) {
                Ok(r) => r,
                Err(e) => return out.refused("get_revisions", &e),
            };
            let Some(rev) = revisions.get(usize::from(*back) % revisions.len().max(1)) else {
                return;
            };
            match st.speleodb_restore_revision(
                link.project_id,
                link.project_name,
                rev.id.clone(),
                restore,
                false,
                false,
                false,
                false,
            ) {
                Ok(_) => out.note(format!("restore_revision(held, restore={restore}) -> Ok")),
                Err(e) => out.refused("restore_revision(held)", &e),
            }
        }
        Act::Reload => {
            let discard =
                app.front().unsaved || app.open_link().is_some_and(|l| l.has_unpushed_edits);
            let ctx = flows::release_context(app, out);
            match st.speleodb_reload(discard, ctx) {
                Ok(r) => out.note(format!("update(held) -> Ok({r:?})")),
                Err(e) => out.refused("update(held)", &e),
            }
        }
        Act::Login { acct } => {
            let account = match acct {
                super::model::Acct::Collab => app.accounts.collab.clone(),
                _ => app.accounts.owner.clone(),
            };
            match st.speleodb_login(
                Some(app.proxy_url.clone()),
                Some(account.email),
                Some(account.password),
                None,
            ) {
                Ok(r) => out.note(format!("login(held) {} -> Ok", r.user_email)),
                Err(e) => out.refused("login(held)", &e),
            }
        }
        Act::ReadOnly => flows::read_only(app, out),
        Act::Publish => {
            let n = app.front().publishes + 1;
            match st.speleodb_publish(flows::publish_params(app, n)) {
                Ok(r) => {
                    app.front().publishes = n;
                    out.note("publish(held) -> Ok");
                    out.claims.push(super::app::Claim::Published(Proj {
                        id: r.project_id,
                        name: flows::publish_params(app, n).name,
                    }));
                }
                Err(e) => out.refused("publish(held)", &e),
            }
        }
        Act::Refresh => flows::link_status(app, out),
        Act::Close => flows::close_project(app, out),
        other => out.note(format!("{} has no bare verb", other.name())),
    }
}

/// Run `f` on a thread of its own (adopting the run's config), answering
/// through a channel.
fn spawn(
    app: &Arc<App>,
    f: impl FnOnce(&App) -> Outcome + Send + 'static,
) -> mpsc::Receiver<Outcome> {
    let (tx, rx) = mpsc::channel();
    let app = Arc::clone(app);
    std::thread::spawn(move || {
        let _config = app.config.adopt();
        let out = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&app))) {
            Ok(out) => out,
            Err(p) => Outcome {
                panicked: Some(super::app::panic_text(p.as_ref())),
                ..Outcome::default()
            },
        };
        let _ = tx.send(out);
    });
    rx
}

fn invalidating(first: &Act, second: &Act) -> bool {
    matches!(
        second,
        Act::Logout
            | Act::Login { .. }
            | Act::LoginOther
            | Act::Open { .. }
            | Act::Close
            | Act::NewLocal
    ) || (matches!(first, Act::View { .. } | Act::Revert { .. } | Act::Reload)
        && matches!(second, Act::Edit))
}

/// What an interleaving step did.
pub(super) struct Interleaved {
    pub(super) out: Outcome,
    pub(super) violations: Vec<Violation>,
    /// A verb never returned: the run cannot go on.
    pub(super) hung: bool,
}

pub(super) fn run(
    app: &Arc<App>,
    proxy: &FaultProxy,
    first: &Act,
    hold: &HoldSpec,
    second: &Act,
    projects: &[Proj],
) -> Interleaved {
    let mut out = Outcome::default();
    let mut violations = Vec::new();
    let method = ["GET", "POST", "PUT", "PATCH", "DELETE"]
        .into_iter()
        .find(|m| m.eq_ignore_ascii_case(&hold.method));
    proxy.arm(FaultRule {
        method,
        path_contains: hold.path.clone(),
        kind: if hold.reply {
            FaultKind::HoldReply
        } else {
            FaultKind::HoldBefore
        },
        times: 1,
    });
    let first_rx = {
        let (first, projects) = (first.clone(), projects.to_vec());
        spawn(app, move |app| {
            let mut out = Outcome::default();
            bare(app, &first, &projects, &mut out);
            out
        })
    };
    let engaged = proxy.wait_held(REACH_HOLD);
    out.note(format!(
        "hold {} {} engaged: {engaged}",
        hold.method, hold.path
    ));
    let ends_process = matches!(second, Act::Quit | Act::Crash { .. } | Act::RestartUpdate);
    let mut mid = None;
    let mut second_rx = None;
    let mut second_out = None;
    let mut blocked = None;
    if engaged {
        let started = Instant::now();
        let rx = {
            let (second, projects) = (second.clone(), projects.to_vec());
            spawn(app, move |app| app.run(&second, &projects))
        };
        if let Ok(o) = rx.recv_timeout(MAIN_THREAD_BUDGET) {
            mid = Some(local_view(app));
            second_out = Some(o);
        } else {
            blocked = Some(started);
            second_rx = Some(rx);
        }
    }
    if ends_process {
        proxy.abort_holds();
    } else {
        proxy.release_holds();
    }
    let mut hung = false;
    if let Ok(o) = first_rx.recv_timeout(VERB_BUDGET) {
        out.merge(o);
    } else {
        hung = true;
        violations.push(violation(
            "HANG",
            format!("{} never returned", first.name()),
        ));
    }
    if let Some(rx) = second_rx {
        if let Ok(o) = rx.recv_timeout(VERB_BUDGET) {
            second_out = Some(o);
        } else {
            hung = true;
            violations.push(violation(
                "HANG",
                format!("{} never returned", second.name()),
            ));
        }
    }
    if let (Some(started), false) = (blocked, hung) {
        let waited = started.elapsed().as_millis();
        let id = match second {
            Act::Close | Act::ConfigTouch => Some("R4-main-thread-block"),
            Act::StateRead | Act::Edit => Some("P2-project-mutex-block"),
            _ => None,
        };
        out.note(format!(
            "{} waited {waited} ms behind the held {}",
            second.name(),
            first.name()
        ));
        if let Some(id) = id {
            violations.push(violation(
                id,
                format!("{} (a synchronous main-thread command) waited {waited} ms behind {}'s held {} {}", second.name(), first.name(), hold.method, hold.path),
            ));
        }
    }
    if !engaged && !hung {
        // The first verb never reached its held request: run the second plainly.
        let rx = {
            let (second, projects) = (second.clone(), projects.to_vec());
            spawn(app, move |app| app.run(&second, &projects))
        };
        if let Ok(o) = rx.recv_timeout(VERB_BUDGET) {
            second_out = Some(o);
        } else {
            hung = true;
            violations.push(violation(
                "HANG",
                format!("{} never returned", second.name()),
            ));
        }
    }
    let second_ok = second_out
        .as_ref()
        .is_some_and(|o| o.refusals.is_empty() && o.panicked.is_none());
    if let Some(o) = second_out {
        out.merge(o);
    }
    if hung {
        return Interleaved {
            out,
            violations,
            hung,
        };
    }
    if let (Some(mid), false) = (mid, ends_process) {
        let after = local_view(app);
        if matches!(second, Act::Logout) && (after.app.logged_in || after.app.saved_token) {
            violations.push(violation(
                "ST-CRED",
                format!("a {} reply that landed after the sign-out left the app signed in ({}) / a token saved ({})", first.name(), after.app.logged_in, after.app.saved_token),
            ));
        }
        if invalidating(first, second) && second_ok {
            let c = changes(&mid, &after);
            if !c.is_empty() {
                let id = if matches!(first, Act::View { .. } | Act::Revert { .. }) {
                    "R7-late-revision-install"
                } else {
                    "ST-LATE"
                };
                violations.push(violation(
                    id,
                    format!(
                        "{}'s late reply after {} changed: {}",
                        first.name(),
                        second.name(),
                        c.join("; ")
                    ),
                ));
            }
        }
    }
    Interleaved {
        out,
        violations,
        hung,
    }
}
