//! Data invariants: BASE, the dirty bit, edits, uploads, overwrites,
//! newer-schema files and the cache.
//!
//! - **DB-LOSS** an edit token the app accepted is, after the step, in no server
//!   commit of any run project, not in memory, and in no `.stalx` the user can
//!   reach, and the step disclosed no matching loss (a confirmed discard of
//!   unsaved edits for an edit that was only in memory; of local edits for one
//!   that was on disk; a crash for one only in memory).
//! - **DB-BASE** a linked copy's BASE is not a commit of its (existing) project.
//! - **DB-CLEAN** a linked copy marked clean differs from its BASE commit, on disk
//!   or (the open copy) in memory. **R6-crash-residue-install** when a crash left a
//!   hot journal beside that copy.
//! - **DB-INTEGRITY** a cache copy written in this step fails `integrity_check`
//!   as the app's next open will read it. **R6-crash-residue-install** with residue.
//! - **DB-UPLOAD** after a confirmed upload BASE is not the server tip, the copy is
//!   still marked dirty, or its content is not the tip's.
//! - **DB-RELOAD** after Reload said up to date or fast-forwarded, BASE is not the
//!   tip.
//! - **R3-same-account-overwrite** an upload the app made in this step dropped
//!   sections its parent commit had, without a deliberate revert: another
//!   session's revision was superseded without a prompt.
//! - **DB-NEWER-INSTALL** a cache copy holds a file a newer schema wrote.
//! - **DB-NEWER-OVERWRITE** the app uploaded on top of a newer-schema revision.
//! - **CA-FAIL** a failed open / reload / take remote / view changed the cache copy
//!   (bytes or link) of the project it was about.
//! - **CA-ORPHAN** the cache holds a file other than `<id>/project.stalx`, its
//!   SQLite sidecars and the upload snapshot.

use std::path::Path;

use super::app::Claim;
use super::model::Act;
use super::oracle::{diff, short, violation, CommitFps, Ghost, StepCtx, Violation};
use super::snap::Snap;

fn in_mem(snap: &Snap, t: &str) -> bool {
    snap.app.mem.as_ref().is_some_and(|fp| fp.contains_key(t))
}

fn on_disk(snap: &Snap, t: &str) -> bool {
    snap.files
        .values()
        .any(|f| f.fp.as_ref().is_ok_and(|fp| fp.contains_key(t)))
}

fn on_server(snap: &Snap, t: &str) -> bool {
    snap.server_tokens.values().any(|s| s.contains(t))
}

fn check_loss(ghost: &mut Ghost, cx: &StepCtx<'_>, v: &mut Vec<Violation>) {
    for c in cx.claims() {
        if let Claim::Edited(t) = c {
            ghost.pending.insert(t.clone());
        }
    }
    let unsaved_ok = cx.has(|c| matches!(c, Claim::DiscardedUnsaved | Claim::Crashed));
    let local_ok = cx.has(|c| matches!(c, Claim::DiscardedLocal));
    let mut gone = Vec::new();
    for t in &ghost.pending {
        if on_server(cx.after, t) {
            gone.push(t.clone());
            continue;
        }
        if in_mem(cx.after, t) || on_disk(cx.after, t) {
            continue;
        }
        let was_on_disk = on_disk(cx.before, t);
        let excused = local_ok || (unsaved_ok && !was_on_disk);
        if !excused {
            let was = if was_on_disk {
                "on disk"
            } else if in_mem(cx.before, t) {
                "in memory only"
            } else {
                "nowhere (already)"
            };
            v.push(violation(
                "DB-LOSS",
                format!("edit {t} is gone (it was {was} before this step, never on the server) and the step said nothing"),
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
    cx: &StepCtx<'_>,
    server: &mut dyn CommitFps,
    v: &mut Vec<Violation>,
) {
    let open = cx.after.app.open_path.clone();
    for (path, file) in &cx.after.files {
        let changed = cx
            .before
            .files
            .get(path)
            .is_none_or(|b| b.hash != file.hash || b.residue != file.residue);
        let link = file.link.as_ref().ok().cloned().flatten();
        let residue_id = |pid: &str| {
            if ghost.residue.contains(pid) || file.residue {
                "R6-crash-residue-install"
            } else {
                "DB-CLEAN"
            }
        };
        if file.newer && path.starts_with(cx.cache_root) {
            v.push(violation(
                "DB-NEWER-INSTALL",
                format!("{} holds a file a newer schema wrote", short(path)),
            ));
        }
        if let (Err(why), true) = (&file.integrity, changed || file.residue) {
            // The cache folder names the project even when the copy is too
            // damaged to read its link.
            let folder = path
                .parent()
                .and_then(Path::file_name)
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let id = if file.residue
                || ghost.residue.contains(&folder)
                || link
                    .as_ref()
                    .is_some_and(|l| ghost.residue.contains(&l.project_id))
            {
                "R6-crash-residue-install"
            } else {
                "DB-INTEGRITY"
            };
            v.push(violation(
                id,
                format!(
                    "{} fails integrity_check as the next open reads it: {}",
                    short(path),
                    super::app::clip(why)
                ),
            ));
        }
        let Some(link) = link else { continue };
        let Some(view) = cx.after.server.get(&link.project_id) else {
            continue;
        };
        if !view.exists || view.commits.is_empty() {
            continue;
        }
        let Some(base) = &link.base_commit_sha else {
            continue;
        };
        if !view.commits.iter().any(|c| c == base) {
            v.push(violation(
                "DB-BASE",
                format!(
                    "{} BASE {base} is not a commit of {}",
                    short(path),
                    link.project_id
                ),
            ));
            continue;
        }
        if link.has_unpushed_edits || server.is_newer(&link.project_id, base) {
            continue;
        }
        let Ok(base_fp) = server.commit_fp(&link.project_id, base) else {
            continue;
        };
        if let Ok(disk) = &file.fp {
            if *disk != base_fp {
                v.push(violation(
                    residue_id(&link.project_id),
                    format!(
                        "{} is marked clean but differs from its BASE {base}: {}",
                        short(path),
                        diff(disk, &base_fp)
                    ),
                ));
                continue;
            }
        }
        let is_open = open.as_deref() == Some(path.as_path());
        if let (true, Some(mem), false) = (is_open, &cx.after.app.mem, cx.after.app.unsaved) {
            if *mem != base_fp {
                v.push(violation(
                    residue_id(&link.project_id),
                    format!(
                        "the open copy is marked clean but its memory differs from BASE {base}: {}",
                        diff(mem, &base_fp)
                    ),
                ));
            }
        }
    }
}

fn check_claims(cx: &StepCtx<'_>, server: &mut dyn CommitFps, v: &mut Vec<Violation>) {
    let cache_file = |pid: &str| cx.cache_root.join(pid).join("project.stalx");
    for c in cx.claims() {
        match c {
            Claim::Uploaded { project_id } => {
                let Some(view) = cx.after.server.get(project_id) else {
                    continue;
                };
                let Some(file) = cx.after.files.get(&cache_file(project_id)) else {
                    continue;
                };
                let link = file.link.clone().ok().flatten();
                let base = link.as_ref().and_then(|l| l.base_commit_sha.clone());
                let dirty = link.as_ref().is_some_and(|l| l.has_unpushed_edits);
                if base != view.tip || dirty {
                    v.push(violation(
                        "DB-UPLOAD",
                        format!("{project_id}: after a confirmed upload BASE={base:?} dirty={dirty}, server tip={:?}", view.tip),
                    ));
                    continue;
                }
                if let (Some(tip), Ok(disk)) = (&view.tip, &file.fp) {
                    if let Ok(tip_fp) = server.commit_fp(project_id, tip) {
                        if *disk != tip_fp {
                            v.push(violation(
                                "DB-UPLOAD",
                                format!(
                                    "{project_id}: the copy differs from the tip it uploaded: {}",
                                    diff(disk, &tip_fp)
                                ),
                            ));
                        }
                    }
                }
            }
            Claim::BaseIsTip { project_id } => {
                let Some(view) = cx.after.server.get(project_id) else {
                    continue;
                };
                let base = cx
                    .after
                    .files
                    .get(&cache_file(project_id))
                    .and_then(|f| f.link.clone().ok().flatten())
                    .and_then(|l| l.base_commit_sha);
                if base != view.tip {
                    v.push(violation(
                        "DB-RELOAD",
                        format!(
                            "{project_id}: Reload said current but BASE={base:?}, tip={:?}",
                            view.tip
                        ),
                    ));
                }
            }
            _ => {}
        }
    }
}

/// Commits that appeared in an app step are the app's own uploads.
fn check_overwrites(cx: &StepCtx<'_>, server: &mut dyn CommitFps, v: &mut Vec<Violation>) {
    for (pid, view) in &cx.after.server {
        let Some(was) = cx.before.server.get(pid) else {
            continue;
        };
        let reverted = cx.has(|c| matches!(c, Claim::Reverted { project_id } if project_id == pid));
        for sha in view.commits.iter().filter(|c| !was.commits.contains(c)) {
            let Some(parent) = view.parents.get(sha).and_then(|p| p.first()) else {
                continue;
            };
            if server.is_newer(pid, parent) {
                v.push(violation(
                    "DB-NEWER-OVERWRITE",
                    format!("{pid}: the app uploaded {sha} on top of {parent}, a revision a newer schema wrote"),
                ));
                continue;
            }
            if reverted {
                continue;
            }
            let (Ok(mine), Ok(theirs)) =
                (server.commit_fp(pid, sha), server.commit_fp(pid, parent))
            else {
                continue;
            };
            let dropped: Vec<&String> = theirs.keys().filter(|k| !mine.contains_key(*k)).collect();
            if !dropped.is_empty() {
                v.push(violation(
                    "R3-same-account-overwrite",
                    format!("{pid}: the app's upload {sha} dropped sections {dropped:?} its parent {parent} had (a revision superseded without a prompt)"),
                ));
            }
        }
    }
}

/// A failed transfer leaves the existing cache copy of its project untouched.
fn check_failed_transfers(cx: &StepCtx<'_>, v: &mut Vec<Violation>) {
    let target = match cx.act {
        Act::Open { .. } | Act::Reload | Act::TakeRemote | Act::View { .. } => {
            cx.before.app.link.as_ref().map(|l| l.project_id.clone())
        }
        _ => None,
    };
    let failed = cx.out.refusals.iter().any(|r| {
        [
            "open_project",
            "update",
            "take_remote",
            "restore_revision(view)",
        ]
        .iter()
        .any(|verb| r.verb.starts_with(verb))
    });
    let open_target = match cx.act {
        Act::Open { .. } => cx
            .events
            .iter()
            .find(|e| e.path.contains("/download/stalagmight/"))
            .and_then(|e| super::oracle::project_of(&e.path).map(str::to_owned)),
        _ => None,
    };
    let Some(pid) = open_target.or(target) else {
        return;
    };
    // A refused open answered with "Take the server copy" replaces the copy on
    // purpose; only a step that ends refused is a failed transfer.
    let retaken =
        matches!(cx.act, Act::Open { .. }) && cx.has(|c| matches!(c, Claim::DiscardedLocal));
    if !failed || retaken {
        return;
    }
    let path = cx.cache_root.join(&pid).join("project.stalx");
    let (Some(b), Some(a)) = (cx.before.files.get(&path), cx.after.files.get(&path)) else {
        return;
    };
    let link_same = match (&b.link, &a.link) {
        (Ok(x), Ok(y)) => x == y,
        _ => true,
    };
    if b.hash != a.hash || !link_same {
        v.push(violation(
            "CA-FAIL",
            format!(
                "{pid}: a failed {} changed the cache copy (bytes changed: {}, link changed: {})",
                cx.act.name(),
                b.hash != a.hash,
                !link_same
            ),
        ));
    }
}

fn check_orphans(cx: &StepCtx<'_>, v: &mut Vec<Violation>) {
    for f in &cx.after.cache_files {
        let rel = f.strip_prefix(cx.cache_root).unwrap_or(f);
        let depth = rel.components().count();
        let name = f
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let known = [
            "project.stalx",
            "project.stalx-journal",
            "project.stalx-wal",
            "project.stalx-shm",
            "snapshot.stalx",
            "snapshot.stalx-journal",
        ];
        if depth == 2 && known.contains(&name.as_str()) {
            continue;
        }
        if cx.before.cache_files.contains(f) || depth != 2 {
            v.push(violation(
                "CA-ORPHAN",
                format!("orphan cache file {}", rel.display()),
            ));
        }
    }
}

pub(super) fn check(
    ghost: &mut Ghost,
    cx: &StepCtx<'_>,
    server: &mut dyn CommitFps,
    v: &mut Vec<Violation>,
) {
    check_loss(ghost, cx, v);
    check_files(ghost, cx, server, v);
    // A crash's residue is settled once its copy reads back whole without it.
    ghost.residue.retain(|pid| {
        cx.after
            .files
            .get(&cx.cache_root.join(pid).join("project.stalx"))
            .is_some_and(|f| f.residue || f.integrity.is_err())
    });
    check_claims(cx, server, v);
    if !cx.act.is_actor() {
        check_overwrites(cx, server, v);
        check_failed_transfers(cx, v);
    }
    check_orphans(cx, v);
}
