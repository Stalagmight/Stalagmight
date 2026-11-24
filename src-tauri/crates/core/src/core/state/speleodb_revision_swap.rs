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

use std::path::{Path, PathBuf};

use super::speleodb::local_dto;
use super::speleodb_cache::{InheritedUndo, LocalEditsPolicy, OpenProjectToken};
use super::AppState;
use crate::models::SpeleoLink;
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
    /// The open project exactly as it was (session generation and project
    /// revision), so an edit made while the download ran refuses the swap
    /// instead of being discarded by it.
    token: OpenProjectToken,
    /// The open project's persisted link when the swap started.
    pub(super) link: SpeleoLink,
}

impl AppState {
    /// Bind a revision swap to the project open now (see [`SwapBinding`]).
    /// `NoRemoteProject` when that is not a linked project.
    pub(super) fn speleodb_swap_binding(&self) -> Result<SwapBinding, SpeleoDbError> {
        let _project = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let link = self.speleodb_open_link()?;
        Ok(SwapBinding {
            token: self.open_project_token_locked(),
            link,
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
    ///
    /// `unwound_to` is the Unwind chain position the swapped-in copy records
    /// (`Some(hexsha)` for a history-keeping step, whose loaded history is then
    /// stamped inherited) or clears (`None`). It is written by the follow-up in
    /// ONE durable save.
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
        unwound_to: Option<&str>,
    ) -> Result<(), SpeleoErrorDto> {
        if policy == LocalEditsPolicy::Protect {
            self.guard_local_edits(&link.project_id, dest)
                .map_err(local_dto)?;
        }
        let project_id = link.project_id.clone();
        // The download is read and checked in memory (a legacy SQLite revision
        // migrated), then installed and opened in one commit, which also carries
        // the outgoing copy's view toggles across (`outgoing_display`).
        let prepared = self
            .prepare_download(
                native,
                link,
                undo,
                &super::speleodb_pull::download_decode_options(),
            )
            .map_err(local_dto)?;
        // Review G-speleodb-8: a Restore stamps the copy un-pushed on top of the tip,
        // but a revision a newer Stalagmight saved opens read-only here, so its save,
        // and with it every push, is refused: a dirty copy holding the lock that only
        // Discard gets out of. Refused before anything is replaced; a View (clean)
        // still shows it, and restoring the tip itself stamps nothing.
        if editing && prepared.link.has_unpushed_edits && prepared.extras.read_only.is_some() {
            return Err(local_dto(SpeleoDbError::Io(
                "this revision was saved by a newer version of Stalagmight, so this version \
                 can view it but not restore it; update Stalagmight to restore it"
                    .to_owned(),
            )));
        }
        self.open_project_download(prepared, dest.to_path_buf(), binding.token, editing)
            .map_err(local_dto)?;
        self.while_swapped_in_project_is_open(dest, &project_id, |path| {
            self.speleodb_after_swap(path, unwound_to);
        });
        Ok(())
    }

    /// What a swap records on the copy it just opened, made durable in one
    /// local-only save. The caller holds the `project` guard (`path` is the
    /// open project's `file_path`, read through it): [#303] the Unwind chain
    /// pointer (`unwound_to`), and the inherited-history watermark: for a
    /// history-keeping step the later of now and the newest loaded entry
    /// (`speleodb_loaded_history_watermark`), else cleared. The display
    /// settings were already carried by the install.
    pub(super) fn speleodb_after_swap(&self, path: Option<PathBuf>, unwound_to: Option<&str>) {
        let watermark = unwound_to.map(|_| self.speleodb_loaded_history_watermark());
        self.with_file(|file| {
            let sync = &mut file.extras.local.speleodb;
            sync.unwound_from = unwound_to.map(str::to_owned);
            sync.inherited_through = watermark;
        });
        self.undo_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .set_inherited_through(watermark);
        if let Err(e) = self.persist_local_state_at(path) {
            tracing::warn!(
                error = %e,
                purpose = "record a revision swap's chain position and watermark (#303)",
                "could not persist the swapped-in copy's local state; it is set in memory"
            );
        }
    }

    /// Run `then` only while the copy a swap installed at `dest` is still the
    /// open project for `project_id`, under the `project` guard so no switch can
    /// land between the check and the writes. The follow-up writes a swap makes
    /// after its commit (lock provenance, the Unwind chain pointer, the
    /// inherited-history watermark) address whatever is open, so a switch in
    /// that gap would otherwise stamp them onto someone else's project.
    ///
    /// `then` gets the open project's `file_path`, read through that guard: the
    /// guard is not reentrant, so a durable save inside `then` must go through
    /// `persist_local_state_at`, never `persist_local_state`.
    pub(super) fn while_swapped_in_project_is_open(
        &self,
        dest: &Path,
        project_id: &str,
        then: impl FnOnce(Option<PathBuf>),
    ) {
        // Held across `then`; it only takes locks that nest inside `project`.
        let project = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let path = project.as_ref().and_then(|open| open.file_path.clone());
        let at_dest = path
            .as_deref()
            .is_some_and(|open| super::project_io::same_file(open, dest));
        if at_dest
            && self
                .speleodb_open_link()
                .is_ok_and(|link| link.project_id == project_id)
        {
            then(path);
        } else {
            tracing::info!(
                project_id = %project_id,
                purpose = "never stamp a revision swap's follow-up onto another project",
                "speleodb: the open project changed right after a revision swap; skipping its follow-up"
            );
        }
    }
}
