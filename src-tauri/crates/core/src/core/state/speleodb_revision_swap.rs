//! Installing a downloaded revision as the open project (View, Restore, Unwind),
//! bound to the project it was started from.
//!
//! A revision swap downloads first and commits last, and nothing bounds the
//! download: View sits in the persistent workspace with no blocking overlay, so
//! the user may open, and edit, a different project while it runs - that is the
//! point of not blocking. The swap used to check the open project only on entry
//! and then close whatever was open when its reply landed, unsaved edits and
//! all, and reload its own copy in place.
//!
//! So the swap binds itself to the world it started in ([`SwapBinding`]) and
//! installs through the prepared reload the fast-forward uses
//! (`open_project_download`): the file replacement and the project commit are
//! ONE acquisition of the `project` guard, and the session generation, the open
//! link's identity and the exact in-memory project are re-checked inside it. If
//! any moved, the swap is refused with [`SpeleoDbError::SessionMoved`] before a
//! byte is replaced, and the project open at that moment is left exactly as it
//! is. There is no window in which the old project is closed and the new one not
//! yet loaded, so there is nothing for a second open to slip into either.

use std::path::Path;

use super::speleodb::local_dto;
use super::speleodb_cache::{
    preserved_display_settings, restore_display_settings, InheritedUndo, LocalEditsPolicy,
};
use super::speleodb_cache_install::{stage_download, sync_staged_file};
use super::AppState;
use crate::models::SpeleoLink;
use crate::persistence::Database;
use crate::speleodb::{SpeleoDbError, SpeleoErrorDto};

/// The open project a revision swap was asked for, captured under ONE `project`
/// acquisition so the three facts describe one world.
///
/// Deliberately a snapshot, not a download reservation: reserving bumps the
/// session generation, which would abort a Start Editing or an upload already in
/// flight on this very project merely because View was clicked - even when the
/// un-pushed-edits guard is about to refuse that View. Any boundary that does
/// matter (a close, an open, a logout, another install) bumps the generation on
/// its own, and the swap's commit consumes the generation it was bound to.
pub(super) struct SwapBinding {
    generation: u64,
    /// The open project's persisted link when the swap started.
    pub(super) link: SpeleoLink,
    /// The open project exactly as it was, so an edit made while the download
    /// ran refuses the swap instead of being discarded by it.
    snapshot: Vec<u8>,
}

impl AppState {
    /// Bind a revision swap to the project open now (see [`SwapBinding`]).
    /// `NoRemoteProject` when that is not a linked project.
    pub(super) fn speleodb_swap_binding(&self) -> Result<SwapBinding, SpeleoDbError> {
        let project = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let link = self.speleodb_open_link()?;
        let snapshot =
            serde_json::to_vec(&*project).map_err(|e| SpeleoDbError::Io(e.to_string()))?;
        Ok(SwapBinding {
            generation: self.speleodb_session_generation(),
            link,
            snapshot,
        })
    }

    /// Validate, stamp and stage the downloaded revision at `native`, then install
    /// it at `dest` and open it in ONE commit - or refuse with `SessionMoved`,
    /// leaving both the cache file and the open project untouched, when the
    /// project the swap was bound to is no longer the one open.
    ///
    /// `editing` is the edit gate the reopened project gets, set inside that same
    /// commit: a gate opened afterwards could land on a project the user switched
    /// to in between.
    #[expect(
        clippy::result_large_err,
        reason = "preserve the shared structured IPC recovery payload"
    )]
    pub(super) fn speleodb_swap_in_revision(
        &self,
        native: &Path,
        dest: &Path,
        link: SpeleoLink,
        (policy, undo): (LocalEditsPolicy, InheritedUndo),
        binding: SwapBinding,
        editing: bool,
    ) -> Result<(), SpeleoErrorDto> {
        if policy == LocalEditsPolicy::Protect {
            self.guard_local_edits(&link.project_id, dest)
                .map_err(local_dto)?;
        }
        self.stamp_downloaded_link(native, link)
            .map_err(local_dto)?;
        let staged = stage_download(native, dest, undo, sync_staged_file).map_err(local_dto)?;
        {
            let db = Database::open_existing(&staged)
                .map_err(|e| local_dto(SpeleoDbError::Io(e.to_string())))?;
            restore_display_settings(&db, &preserved_display_settings(dest));
        }
        sync_staged_file(&staged).map_err(local_dto)?;
        let result = self.open_project_download(
            staged.to_path_buf(),
            dest.to_path_buf(),
            (binding.generation, binding.snapshot),
            editing,
        )?;
        if result.success {
            Ok(())
        } else {
            Err(local_dto(SpeleoDbError::Io(
                result
                    .error
                    .unwrap_or_else(|| "Could not reload project".into()),
            )))
        }
    }

    /// Run `then` only while the copy a swap installed at `dest` is still the
    /// open project for `project_id`, under the `project` guard so no switch can
    /// land between the check and the writes. The follow-up writes a swap makes
    /// after its commit (lock provenance, the Unwind chain pointer, the
    /// inherited-history watermark) address whatever is open, so a switch in
    /// that gap would otherwise stamp them onto someone else's project.
    pub(super) fn while_swapped_in_project_is_open(
        &self,
        dest: &Path,
        project_id: &str,
        then: impl FnOnce(),
    ) {
        // Held across `then`; it only takes locks that nest inside `project`.
        let project = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let at_dest = project
            .as_ref()
            .and_then(|open| open.file_path.as_deref())
            .is_some_and(|open| super::project_io::same_file(open, dest));
        if at_dest
            && self
                .speleodb_open_link()
                .is_ok_and(|link| link.project_id == project_id)
        {
            then();
        } else {
            tracing::info!(
                project_id = %project_id,
                purpose = "never stamp a revision swap's follow-up onto another project",
                "speleodb: the open project changed right after a revision swap; skipping its follow-up"
            );
        }
    }
}
