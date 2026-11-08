//! [#576 FZ-SDB-5] Unlink: move the open project's copy OUT of the SpeleoDB cache.
//!
//! Unlink used to drop the copy's `speleo_link` row in place, leaving a plain
//! `.stalx` at `<cache>/speleodb/<id>/project.stalx`. That path is the managed
//! project's download slot, so the next Open of the same project from SpeleoDB
//! replaced the file - and every local edit made before or after the Unlink with
//! it (the un-pushed-edits guard reads the link row, and there was none).
//!
//! Now Unlink is a MOVE to a path the user picked (decisions FZ-SDB-5-D1/D2): the
//! frontend asks with a Save As dialog, and this verb writes the copy there, closes
//! the session on the cache file, removes the cache file and hands the server lock
//! back. The order is the data-safety argument:
//!
//! 1. Refuse first, before anything is written: no open project, an open project
//!    that is not in the cache (nothing to move), a destination inside the cache,
//!    and (#576 RVF-c-C1) an Unlink made for another project than the open one -
//!    checked in the save's own `command_lock` span.
//! 2. Write the copy through the ordinary save pipeline, so it is the same file a
//!    Save As would write (survey, settings, undo history; staged to a temp file and
//!    renamed into place). A linked project's explicit-destination save is the
//!    detached-copy save, which leaves the link row out; a link-less one (an Unlink
//!    from before this change, see `guard_local_edits`) is an ordinary Save As.
//! 3. Read the copy back: present, readable, and NOT linked. Until this passes,
//!    the cache file, its link and the lock are as they were. A read-back that
//!    refuses leaves the copy it read at the destination, and its error says so: the
//!    user confirmed replacing that file in the Save As, so it is not removed.
//! 4. Only then close the session and remove the cache file, in the SAME
//!    `command_lock` span as the save, so no edit can land in the cache file after
//!    the copy was taken. A removal that fails leaves a duplicate behind, never a
//!    loss, and is reported as a warning.
//! 5. Hand the lock back (best-effort, the abandoning release: the link is gone, so
//!    a lock kept back could never be released from this project again), outside
//!    the `command_lock` span because it is a network call, and inside the lock-ops
//!    span the whole verb holds, so no Start Editing can take the mutex in between.
//!
//! The frontend then loads the copy, so the open session follows the new path.

use std::path::{Path, PathBuf};

use super::speleodb::local_dto;
use super::speleodb_local_state::{cache_root, is_the_cached_copy};
use super::AppState;
use crate::core::{SaveOutcome, ValidationWarning, WarningSeverity};
use crate::persistence::Database;
use crate::speleodb::{SpeleoDbError, SpeleoErrorDto};

/// Whether `path` lies inside `root`, literally or once both are resolved: the
/// destination check, where a literal match is reason enough to refuse. A
/// destination need not exist yet, so its folder is resolved and its name joined
/// back on.
fn inside(root: &Path, path: &Path) -> bool {
    path.starts_with(root) || resolved_inside(root, path)
}

/// Whether `path` lies inside `root` once both are resolved, and only then: the
/// check that decides whether the source is DELETED (#576 W87b). A literal
/// `starts_with` is component-wise and does not resolve `..`, so
/// `<cache>/../elsewhere.stalx` passed it, and the move then deleted a file outside
/// the cache.
fn resolved_inside(root: &Path, path: &Path) -> bool {
    let Ok(root) = std::fs::canonicalize(root) else {
        // No cache folder on disk: nothing can be inside it.
        return false;
    };
    let resolved = std::fs::canonicalize(path).ok().or_else(|| {
        let name = path.file_name()?;
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty())?;
        std::fs::canonicalize(parent).ok().map(|p| p.join(name))
    });
    resolved.is_some_and(|p| p.starts_with(&root))
}

#[cfg(test)]
thread_local! {
    /// Test seam (#576 W87b): run on the destination right after the unlink's save
    /// wrote it and before the read-back, on the thread that set it, so a test can
    /// make that read-back refuse.
    pub(super) static AFTER_COPY_WRITTEN: std::cell::Cell<Option<fn(&Path)>> =
        const { std::cell::Cell::new(None) };
}

/// Remove the cache copy at `path` and what SQLite may keep beside it, then its
/// per-project folder when that is empty. Returns why the copy itself could not be
/// removed; the sidecars and the folder are best-effort.
fn remove_cache_copy(path: &Path) -> Result<(), String> {
    std::fs::remove_file(path).map_err(|e| e.to_string())?;
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = path.as_os_str().to_owned();
        sidecar.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(sidecar));
    }
    if let Some(dir) = path.parent() {
        // Only an empty folder goes: anything else in it is not ours to delete.
        let _ = std::fs::remove_dir(dir);
    }
    Ok(())
}

impl AppState {
    /// Move the open project's copy out of the SpeleoDB cache to `destination`,
    /// unlinked from SpeleoDB (see the module doc for the order and why). Answers
    /// where the copy landed (`saved_path`, which the frontend then loads) and what
    /// the save could not keep. On `Err` the cache copy, its link and the lock are as
    /// they were; a refused read-back has written a copy to `destination` first, and
    /// its error names it.
    pub fn speleodb_unlink(&self, destination: PathBuf) -> Result<SaveOutcome, SpeleoErrorDto> {
        // The whole verb is one lock operation: the release at the end must not
        // meet a Start Editing that took the mutex after the session closed.
        let _lock_ops = self.speleodb_lock_ops_guard();
        let refuse = |why: String| Err(local_dto(SpeleoDbError::refused(why)));
        // [#576 RVF-c-C1] Made for another project than the open one: say so, rather
        // than refuse over the open project's state below. The check that binds is
        // the one in the save's span.
        {
            let _commands = self.hold_commands();
            self.speleodb_refuse_other_project("Unlink")?;
        }

        let Some(source) = self.open_project_path() else {
            return refuse("no project is open".to_owned());
        };
        let link = match self.speleodb_open_link() {
            Ok(link) => Some(link),
            Err(SpeleoDbError::NoRemoteProject) => None,
            Err(e) => return Err(local_dto(e)),
        };
        let Some(root) = cache_root() else {
            return refuse("the SpeleoDB cache folder is not available".to_owned());
        };
        // Linked: its own cache slot is the only place a managed copy lives. Link-less
        // (an Unlink made before this change): anywhere in the cache, once resolved,
        // because this decides whether the file is deleted after the move.
        let in_cache = match &link {
            Some(link) => is_the_cached_copy(&root, &link.project_id, &source),
            None => resolved_inside(&root, &source),
        };
        if !in_cache {
            return refuse(format!(
                "{} is not in the SpeleoDB cache, so there is nothing to move",
                source.display()
            ));
        }
        let dest = crate::persistence::with_stalx_extension(destination);
        if inside(&root, &dest) {
            return refuse(
                "choose a folder outside the SpeleoDB cache: a copy left there is replaced \
                 by the next download"
                    .to_owned(),
            );
        }
        if super::project_io::same_file(&source, &dest) {
            return refuse("choose a different file than the SpeleoDB copy itself".to_owned());
        }
        // Read before the close: the release needs the link and the edit gate.
        let release = link
            .as_ref()
            .and_then(|_| self.speleodb_abandon_lock_release_params());

        let mut warnings = {
            let commands = self.hold_commands();
            // [#576 RVF-c-C1] An Unlink made for another project than the open one is
            // refused before the Save As: it would write THAT project to the path
            // chosen for this one, then close it and remove its cache copy.
            self.speleodb_refuse_other_project("Unlink")?;
            let saved = self.save_project(Some(dest.clone()));
            if !saved.success {
                let message = saved
                    .error
                    .unwrap_or_else(|| format!("could not save {}", dest.display()));
                return Err(local_dto(match saved.code {
                    Some(code) => SpeleoDbError::refused_with(code, message),
                    None => SpeleoDbError::refused(message),
                }));
            }
            #[cfg(test)]
            if let Some(hook) = AFTER_COPY_WRITTEN.with(std::cell::Cell::get) {
                hook(&dest);
            }
            // Read the copy back before giving anything up: there, readable, unlinked.
            // A refusal here comes AFTER the write: the copy stays where the user
            // confirmed it may go, and the error says it was written.
            match Database::open_for_metadata(&dest).and_then(|db| db.load_speleo_link()) {
                Ok(None) => {}
                Ok(Some(_)) => {
                    return refuse(format!(
                        "a copy was written to {}, but it is still linked to SpeleoDB, so \
                         the move could not be confirmed. That copy was left there; the \
                         SpeleoDB copy was kept and nothing was unlinked",
                        dest.display()
                    ))
                }
                Err(e) => {
                    return refuse(format!(
                        "a copy was written to {}, but it could not be read back ({e}), so \
                         the move could not be confirmed. That copy was left there; the \
                         SpeleoDB copy was kept and nothing was unlinked",
                        dest.display()
                    ))
                }
            }
            let mut warnings = saved.warnings;
            // The copy is safe. Close the session on the cache file (for a link-less
            // project the Save As already moved it to `dest`, and this closes that),
            // and remove the cache file in the same span.
            let _closed = self.close_project_held(&commands);
            if let Err(e) = remove_cache_copy(&source) {
                tracing::warn!(
                    path = %source.display(),
                    error = %e,
                    purpose = "move the copy out of the SpeleoDB cache (#576 FZ-SDB-5)",
                    "speleodb unlink: the copy was saved elsewhere, but the cache copy \
                     could not be removed"
                );
                warnings.push(ValidationWarning {
                    code: "speleodb_unlink_cache_left".to_owned(),
                    message: format!(
                        "Saved to {}, but the SpeleoDB copy at {} could not be removed: {e}",
                        dest.display(),
                        source.display()
                    ),
                    severity: WarningSeverity::Warning,
                });
            }
            warnings
        };
        // The cache entry leaves the recent list; the copy took its place there.
        let forgotten = self.remove_from_recent_projects(source.clone());
        if !forgotten.success {
            warnings.push(ValidationWarning {
                code: "speleodb_unlink_recent_left".to_owned(),
                message: forgotten
                    .error
                    .unwrap_or_else(|| "the recent-projects list could not be saved".to_owned()),
                severity: WarningSeverity::Warning,
            });
        }
        if let Some(p) = release {
            self.speleodb_release_best_effort(
                &p.endpoint,
                &p.token,
                p.insecure_http,
                &p.project_id,
            );
        }
        tracing::info!(
            from = %source.display(),
            to = %dest.display(),
            purpose = "move the copy out of the SpeleoDB cache (#576 FZ-SDB-5)",
            "speleodb unlink: moved the project out of the SpeleoDB cache"
        );
        Ok(SaveOutcome {
            saved_path: Some(dest.display().to_string()),
            unlinked_copy_path: None,
            warnings,
        })
    }
}
