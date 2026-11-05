//! The oracle: what is true after a step (server, files, the app), and the
//! invariants the fuzzer holds the app to.
//!
//! ## The invariants (each violation carries its id)
//!
//! - **I1 no silent loss.** An edit token the app accepted is, after every step,
//!   in at least one of: a server commit of any project of the run (git history
//!   is never rewritten), the open project in memory, or a `.stalx` on disk the
//!   user can reach (the SpeleoDB cache or a saved local copy). It may leave all
//!   three only in a step whose result told the user so: a confirmed discard
//!   (take remote, discard changes, a discarding open / restore / unwind) or a
//!   crash (edits that were only in memory).
//! - **I2 lock handed back.** After a deliberate release (Done -> `Released`,
//!   Discard without keeping the lock, release current / all, take remote,
//!   unlink) the server shows no lock held by the app's account on that project.
//!   **I2b** the same after a silent best-effort release (logout, close, quit)
//!   over a CLEAN copy: the documented veto keeps a lock only over un-pushed
//!   edits, so a lock left with nothing to protect was stranded without a word.
//! - **I3 BASE is real.** A linked copy's `base_commit_sha` names a commit in the
//!   server's history of that project.
//! - **I4 the un-pushed mark matches the real difference.** A linked copy marked
//!   clean holds exactly the content of the commit its BASE names, on disk and
//!   (when it is the open project) in memory. A clean mark over content the
//!   server does not have is how a later pull overwrites edits without asking.
//! - **I5 no orphan cache files.** The SpeleoDB cache holds only
//!   `<id>/project.stalx` (and SQLite's own sidecars while that file is open).
//! - **I6 a confirmed push agrees with the server.** After Push -> `Pushed`,
//!   Done -> `Released` / `LockHeldElsewhere`, take remote, a pushed adoption and
//!   a publish: BASE is the server tip, the copy is marked clean, and its content
//!   equals the tip's. **I6b** after Update -> up to date / fast-forwarded: BASE
//!   is the tip.
//! - **I7 every refusal is a coded error the UI can show.** No panic; no
//!   `INTERNAL_ERROR`; no empty message.
//! - **I8 the app only ever talks to the fuzz instance.** The session endpoint is
//!   the fault proxy (or the in-process other-server stub). A breach aborts.
//! - **I9 a reported delete happened.** After delete -> Ok, the project is gone
//!   on the server (no account of the run can see it).
//! - **I10 a linked copy keeps its identity.** A cache copy linked to a project
//!   stays linked to that project (same endpoint and id) through every step that
//!   does not unlink it on purpose.
//! - **I11 editing means holding the lock.** While the app's edit gate is open
//!   (not force-editing) on a linked project nobody else touched the lock of, the
//!   server shows the app's account holding it.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use super::app::{App, Claim, Outcome};
use super::world::{fingerprint, fingerprint_file, link_of_file, Fp, Server, ServerView};

/// The content of a server commit, by project and sha (the oracle's only
/// server read during a check; a map in the offline tests).
pub(super) trait CommitFps {
    fn commit_fp(&mut self, id: &str, sha: &str) -> Result<Fp, String>;
}

impl CommitFps for Server {
    fn commit_fp(&mut self, id: &str, sha: &str) -> Result<Fp, String> {
        self.fingerprint_at(id, sha)
    }
}
use crate::models::SpeleoLink;

/// One broken invariant.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub(super) struct Violation {
    pub(super) id: String,
    pub(super) severity: String,
    pub(super) detail: String,
}

fn violation(id: &str, detail: String) -> Violation {
    let severity = match id {
        "I8" => "critical",
        "I1" | "I4" | "I6" | "I9" | "I10" | "I7-panic" => "high",
        "I2" | "I3" | "I11" | "I6b" => "medium",
        _ => "low",
    };
    Violation {
        id: id.to_owned(),
        severity: severity.to_owned(),
        detail,
    }
}

/// One local file as the oracle read it.
#[derive(Clone, Debug)]
pub(super) struct FileSnap {
    pub(super) fp: Result<Fp, String>,
    pub(super) link: Result<Option<SpeleoLink>, String>,
}

/// Everything true after a step.
#[derive(Clone, Debug, Default)]
pub(super) struct Snap {
    pub(super) server: BTreeMap<String, ServerView>,
    /// Every edit token in any commit of any project of the run.
    pub(super) server_tokens: BTreeSet<String>,
    pub(super) mem: Option<(Option<PathBuf>, Fp)>,
    pub(super) files: BTreeMap<PathBuf, FileSnap>,
    pub(super) cache_files: Vec<PathBuf>,
    pub(super) edit_enabled: bool,
    pub(super) force_editing: bool,
    pub(super) endpoint: Option<String>,
}

/// The fuzzer's memory across steps.
#[derive(Default)]
pub(super) struct Ghost {
    /// Edit tokens accepted by the app and not (yet) seen in any server commit.
    pub(super) pending: BTreeSet<String>,
    /// Projects whose lock / access the second client touched since the app last
    /// took the lock (I11 does not apply to them).
    pub(super) interference: HashSet<String>,
}

/// Every file under `dir`, recursively.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

fn tokens(fp: &Fp) -> impl Iterator<Item = &String> {
    fp.keys().filter(|k| k.starts_with('E'))
}

/// Read the world. `ids` are the run's projects; `roots` the folders holding
/// the user's files (the cache root and the local-copies folder).
pub(super) fn snapshot(
    app: &App,
    server: &mut Server,
    ids: &[String],
    cache_root: &Path,
    copies: &Path,
) -> Snap {
    let mut snap = Snap {
        edit_enabled: app.state.edit_enabled(),
        force_editing: app.state.force_editing(),
        endpoint: app.session_endpoint(),
        ..Snap::default()
    };
    for (id, view) in server.views(ids) {
        for sha in &view.commits {
            if let Ok(fp) = server.fingerprint_at(&id, sha) {
                snap.server_tokens.extend(tokens(&fp).cloned());
            }
        }
        snap.server.insert(id, view);
    }
    snap.mem = app
        .state
        .project
        .lock()
        .as_ref()
        .map(|p| (p.file_path.clone(), fingerprint(p)));
    walk(cache_root, &mut snap.cache_files);
    let mut stalx: Vec<PathBuf> = snap
        .cache_files
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "stalx"))
        .cloned()
        .collect();
    walk(copies, &mut stalx);
    for path in stalx {
        if path.extension().is_some_and(|e| e == "stalx") {
            let file = FileSnap {
                fp: fingerprint_file(&path),
                link: link_of_file(&path),
            };
            snap.files.insert(path, file);
        }
    }
    snap
}

/// What the step that produced `after` is excused from.
struct Excuses {
    disclosed: bool,
    crashed: bool,
    link_dropped: bool,
}

/// Check every invariant across one step. `owner` is the app's account email,
/// `allowed` the endpoints the app may hold.
#[expect(
    clippy::too_many_arguments,
    reason = "the oracle's whole context, passed once"
)]
pub(super) fn check(
    ghost: &mut Ghost,
    out: &Outcome,
    is_actor: bool,
    before: &Snap,
    after: &Snap,
    server: &mut dyn CommitFps,
    owner: &str,
    allowed: &[&str],
    cache_root: &Path,
) -> Vec<Violation> {
    let mut v = Vec::new();
    let excuses = Excuses {
        disclosed: out.claims.contains(&Claim::DisclosedLoss),
        crashed: out.claims.contains(&Claim::Crashed),
        link_dropped: out.claims.contains(&Claim::LinkDropped)
            || out.claims.iter().any(|c| matches!(c, Claim::Published(_))),
    };

    // I8: the endpoint allowlist (a breach ends the run).
    if let Some(endpoint) = &after.endpoint {
        if !super::target::endpoint_allowed(endpoint, allowed) {
            v.push(violation("I8", format!("the app session holds {endpoint}")));
        }
    }

    // I7: panics and uncoded refusals.
    if let Some(panic) = &out.panicked {
        v.push(violation("I7-panic", format!("a verb panicked: {panic}")));
    }
    for r in &out.refusals {
        if r.code == "INTERNAL_ERROR" || r.message.trim().is_empty() {
            v.push(violation(
                "I7",
                format!("{} refused with {} {:?}", r.verb, r.code, r.message),
            ));
        }
    }

    check_loss(ghost, out, before, after, &excuses, &mut v);
    check_files(ghost, before, after, server, &excuses, cache_root, &mut v);
    check_claims(out, after, server, owner, cache_root, &mut v);

    // I11: an open edit gate means the lock is held (unless someone else touched it).
    let _ = is_actor;
    if after.edit_enabled && !after.force_editing {
        if let Some(link) = open_link(after) {
            let pid = &link.project_id;
            if let Some(view) = after.server.get(pid) {
                let holder = view.lock_holder.as_deref();
                if view.exists && !ghost.interference.contains(pid) && holder != Some(owner) {
                    v.push(violation(
                        "I11",
                        format!(
                            "the app is editing {pid} but the server lock holder is {holder:?}"
                        ),
                    ));
                }
            }
        }
    }
    v
}

/// The open project's link, read from its file.
fn open_link(snap: &Snap) -> Option<SpeleoLink> {
    let (Some(path), _) = snap.mem.as_ref()? else {
        return None;
    };
    snap.files.get(path)?.link.clone().ok().flatten()
}

fn check_loss(
    ghost: &mut Ghost,
    out: &Outcome,
    before: &Snap,
    after: &Snap,
    ex: &Excuses,
    v: &mut Vec<Violation>,
) {
    for c in &out.claims {
        if let Claim::Edited(t) = c {
            ghost.pending.insert(t.clone());
        }
    }
    ghost.pending.retain(|t| !after.server_tokens.contains(t));
    let in_mem =
        |snap: &Snap, t: &String| snap.mem.as_ref().is_some_and(|(_, fp)| fp.contains_key(t));
    let on_disk = |snap: &Snap, t: &String| {
        snap.files
            .values()
            .any(|f| f.fp.as_ref().is_ok_and(|fp| fp.contains_key(t)))
    };
    let mut gone = Vec::new();
    for t in &ghost.pending {
        if in_mem(after, t) || on_disk(after, t) {
            continue;
        }
        let excused = ex.disclosed || (ex.crashed && !on_disk(before, t));
        if !excused {
            let was = if on_disk(before, t) {
                "on disk"
            } else if in_mem(before, t) {
                "in memory only"
            } else {
                "nowhere (already)"
            };
            v.push(violation(
                "I1",
                format!("edit {t} is gone (it was {was} before this step; never on the server) and the step said nothing"),
            ));
        }
        gone.push(t.clone());
    }
    for t in gone {
        ghost.pending.remove(&t);
    }
}

fn check_files(
    ghost: &Ghost,
    before: &Snap,
    after: &Snap,
    server: &mut dyn CommitFps,
    ex: &Excuses,
    cache_root: &Path,
    v: &mut Vec<Violation>,
) {
    let _ = ghost;
    let open_path = after.mem.as_ref().and_then(|(p, _)| p.clone());
    for (path, file) in &after.files {
        let Ok(Some(link)) = &file.link else {
            continue;
        };
        let Some(view) = after.server.get(&link.project_id) else {
            continue;
        };
        if !view.exists {
            continue;
        }
        // I3: BASE names a real commit.
        if let Some(base) = &link.base_commit_sha {
            if !view.commits.iter().any(|c| c == base) {
                v.push(violation(
                    "I3",
                    format!(
                        "{} BASE {base} is not a commit of {}",
                        short(path),
                        link.project_id
                    ),
                ));
                continue;
            }
            // I4: a clean mark means the content IS the BASE commit.
            if !link.has_unpushed_edits {
                if let Ok(base_fp) = server.commit_fp(&link.project_id, base) {
                    if let Ok(disk_fp) = &file.fp {
                        if *disk_fp != base_fp {
                            v.push(violation(
                                "I4",
                                format!(
                                    "{} is marked clean but differs from its BASE {base}: {}",
                                    short(path),
                                    diff(disk_fp, &base_fp)
                                ),
                            ));
                        }
                    }
                    if open_path.as_deref() == Some(path.as_path()) {
                        if let Some((_, mem_fp)) = &after.mem {
                            if *mem_fp != base_fp {
                                v.push(violation(
                                    "I4",
                                    format!(
                                        "the open copy is marked clean but its memory differs from BASE {base}: {}",
                                        diff(mem_fp, &base_fp)
                                    ),
                                ));
                            }
                        }
                    }
                }
            }
        }
    }

    // I10: a linked copy keeps its identity unless the step unlinked on purpose.
    if !ex.link_dropped {
        for (path, was) in &before.files {
            let Ok(Some(was)) = &was.link else { continue };
            let Some(now) = after.files.get(path) else {
                continue;
            };
            match &now.link {
                Ok(Some(l)) if l.project_id == was.project_id && l.endpoint == was.endpoint => {}
                Ok(other) => v.push(violation(
                    "I10",
                    format!(
                        "{} was linked to {} and is now {:?}",
                        short(path),
                        was.project_id,
                        other.as_ref().map(|l| (&l.project_id, &l.endpoint))
                    ),
                )),
                Err(_) => {}
            }
        }
    }

    // I5: only `<id>/project.stalx` (+ SQLite sidecars of the open file).
    for f in &after.cache_files {
        let rel = f.strip_prefix(cache_root).unwrap_or(f);
        let parts: Vec<_> = rel.components().collect();
        let name = f
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let canonical = parts.len() == 2 && name == "project.stalx";
        let sidecar = parts.len() == 2
            && [
                "project.stalx-wal",
                "project.stalx-shm",
                "project.stalx-journal",
            ]
            .contains(&name.as_str())
            && open_path
                .as_deref()
                .is_some_and(|o| o.parent() == f.parent());
        if !canonical && !sidecar {
            v.push(violation(
                "I5",
                format!("orphan cache file {}", rel.display()),
            ));
        }
    }
}

fn check_claims(
    out: &Outcome,
    after: &Snap,
    server: &mut dyn CommitFps,
    owner: &str,
    cache_root: &Path,
    v: &mut Vec<Violation>,
) {
    let cache_file = |pid: &str| cache_root.join(pid).join("project.stalx");
    let holder_is_owner = |pid: &str| {
        after
            .server
            .get(pid)
            .is_some_and(|s| s.exists && s.lock_holder.as_deref() == Some(owner))
    };
    for c in &out.claims {
        match c {
            Claim::InSync { project_id } => {
                let Some(view) = after.server.get(project_id) else {
                    continue;
                };
                let path = cache_file(project_id);
                let Some(file) = after.files.get(&path) else {
                    v.push(violation(
                        "I6",
                        format!("{project_id}: no cache copy after a confirmed push"),
                    ));
                    continue;
                };
                let link = file.link.clone().ok().flatten();
                let base = link.as_ref().and_then(|l| l.base_commit_sha.clone());
                let dirty = link.as_ref().is_some_and(|l| l.has_unpushed_edits);
                if base != view.tip || dirty {
                    v.push(violation(
                        "I6",
                        format!(
                            "{project_id}: after a confirmed push BASE={base:?} dirty={dirty}, server tip={:?}",
                            view.tip
                        ),
                    ));
                    continue;
                }
                if let (Some(tip), Ok(disk)) = (&view.tip, &file.fp) {
                    if let Ok(tip_fp) = server.commit_fp(project_id, tip) {
                        if *disk != tip_fp {
                            v.push(violation(
                                "I6",
                                format!("{project_id}: the copy differs from the tip it says it pushed: {}", diff(disk, &tip_fp)),
                            ));
                        }
                    }
                }
            }
            Claim::BaseIsTip { project_id } => {
                let Some(view) = after.server.get(project_id) else {
                    continue;
                };
                let base = after
                    .files
                    .get(&cache_file(project_id))
                    .and_then(|f| f.link.clone().ok().flatten())
                    .and_then(|l| l.base_commit_sha);
                if base != view.tip {
                    v.push(violation(
                        "I6b",
                        format!(
                            "{project_id}: Update said current but BASE={base:?}, tip={:?}",
                            view.tip
                        ),
                    ));
                }
            }
            Claim::LockReleased { project_id } if holder_is_owner(project_id) => {
                v.push(violation(
                    "I2",
                    format!("{project_id}: the app said it released the lock, the server still shows {owner}"),
                ));
            }
            Claim::AllLocksReleased => {
                for (pid, view) in &after.server {
                    if view.exists && view.lock_holder.as_deref() == Some(owner) {
                        v.push(violation(
                            "I2",
                            format!("{pid}: release-all said Ok, lock still held"),
                        ));
                    }
                }
            }
            Claim::LockReleaseBestEffort { project_id } if holder_is_owner(project_id) => {
                v.push(violation(
                    "I2b",
                    format!("{project_id}: a silent release over a clean copy left the lock held, without a word"),
                ));
            }
            Claim::Deleted { project_id }
                if after.server.get(project_id).is_some_and(|s| s.exists) =>
            {
                v.push(violation(
                    "I9",
                    format!("{project_id}: the app said the project was deleted; it still exists on the server"),
                ));
            }
            _ => {}
        }
    }
}

fn diff(a: &Fp, b: &Fp) -> String {
    let only_a: Vec<_> = a.iter().filter(|(k, n)| b.get(*k) != Some(*n)).collect();
    let only_b: Vec<_> = b.iter().filter(|(k, n)| a.get(*k) != Some(*n)).collect();
    format!("local-only {only_a:?} / server-only {only_b:?}")
}

fn short(path: &Path) -> String {
    let parent = path
        .parent()
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!(
        "{parent}/{}",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    )
}
