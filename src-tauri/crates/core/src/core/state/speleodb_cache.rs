//! Replacing the local SpeleoDB cache `.stalx` with a freshly-downloaded copy.
//!
//! Three flows do this - the pull (`speleodb_open_project`), the fast-forward
//! (`speleodb_start_editing` / `speleodb_update`), and the revision swap
//! (`speleodb_restore_revision`) - and all three must handle the same hazards
//! around clobbering a file the app may still hold open:
//!
//! 1. the per-machine `display_settings` in the OLD file have to survive the
//!    replace (the server copy has them stripped, #197),
//! 2. any live SQLite handle to the destination has to be closed BEFORE the
//!    replace, and the replace itself has to be atomic, and
//! 3. the OLD file's SQLite sidecars have to be out of the way before the new
//!    file takes its name (`speleodb_sidecars`).
//!
//! Every flow stages through `stage_download`; the pull then renames through
//! `replace_cache_for_session`, the fast-forward and the revision swap through
//! the prepared reload (`install_prepared_database`). Each of those renames
//! handles all three, so no call site can forget one. Both run under the
//! session's locks and refuse once a newer open or login has moved it, so a
//! late download can never replace a copy the user has since reopened.
//!
//! It also carries the DATA-SAFETY half: the copy being replaced may hold edits
//! the server has never seen, and those exist in no git anywhere - overwriting
//! them is unrecoverable. So a [`LocalEditsPolicy::Protect`] replace REFUSES
//! ([`SpeleoDbError::WouldDiscardLocalEdits`], which crosses IPC as a typed
//! routing signal for the cancel / take-remote choice), and a caller that
//! genuinely means to discard has to say so by name with
//! [`LocalEditsPolicy::Discard`], so the waiver shows up in a diff as that word.
//! Every caller entitled to it arrives from a frontend that has already had
//! the loss confirmed by name: `speleodb_take_remote` (the divergence prompt's
//! destroy-confirmed "Take remote"), `speleodb_open_project_discarding_local` (the
//! same confirmed choice on a refused open), and `speleodb_restore_revision` under
//! its `discard_local` waiver (a Restore only, never a View - a restore stamps the
//! copy un-pushed itself, so without the waiver Restore was single-use). Anything
//! else passes [`LocalEditsPolicy::Protect`] and refuses instead.

use std::collections::HashMap;
use std::path::Path;

use super::AppState;
use crate::speleodb::SpeleoDbError;

/// Snapshot the local per-machine `display_settings` from an existing cache file
/// BEFORE a download-replace overwrites it. View toggles are stripped from the
/// server copy on push (#197), so without re-applying them a fast-forward / re-open
/// - which replaces the whole local `.stalx` with the server's - would reset the
/// user's view to defaults. Empty when the file is absent or unreadable (a first
/// open → defaults, which is correct). Pairs with [`restore_display_settings`].
///
/// Opens READ-ONLY via `open_for_metadata`: this is a handful of `SELECT`s, so it
/// needs neither the integrity check / migration chain nor `open_existing`'s
/// read-write connection with its 256 MB mmap - and it must not add another
/// mmap'd handle to a file that is about to be replaced.
pub(super) fn preserved_display_settings(dest: &Path) -> HashMap<String, String> {
    if !dest.exists() {
        return HashMap::new();
    }
    crate::persistence::Database::open_for_metadata(dest)
        .and_then(|db| db.load_display_settings())
        .unwrap_or_default()
}

/// [#303] The `speleo_link.undo_mode` already recorded in the `.stalx` at `dest`.
///
/// Every stamp site rewrites the WHOLE link row, so without this a pull / fast-forward /
/// revision swap would reset the project's explicit undo policy, silently discarding a
/// `Disabled` that the project owner set precisely because the
/// project is too big to ship undo snapshots. The value rides in the pushed snapshot
/// (it is project state, not machine state), so the value in the downloaded file
/// IS the authoritative answer and the stamp has to carry it forward.
///
/// `None` when the file carries no link or a legacy NULL. The caller materializes the
/// application default before stamping the shared row. Takes the handle each stamp site
/// has ALREADY opened on the file it is about to stamp, rather than a path: a second
/// connection to a file mid-replace is exactly what the rest of this module avoids.
pub(super) fn preserved_undo_mode(
    db: &crate::persistence::Database,
) -> Option<crate::models::UndoMode> {
    db.load_speleo_link()
        .ok()
        .flatten()
        .and_then(|l| l.undo_mode)
}

/// [#303] Empty the undo/redo tables in a freshly-downloaded cache file.
///
/// The file arrives carrying whatever the last pusher's session held (a Full-mode push
/// ships its undo history on purpose), and inheriting that implicitly is the #194 b3
/// replay hazard. Clearing it HERE - in the bytes on disk, before anything opens them -
/// rather than in memory after the load is deliberate: the load path reads these tables
/// into the live stack, and a clear that only touched memory would leave the history in
/// the file for the next open to pick up again.
///
/// Fatal while the file is still staged. A downloaded database is never installed until
/// all inherited history metadata has been removed successfully.
pub(super) fn drop_inherited_undo(dest: &Path) -> Result<(), SpeleoDbError> {
    crate::persistence::Database::clear_undo_history_at(dest).map_err(|e| {
        SpeleoDbError::Io(format!(
            "could not clear inherited history from {}: {e}",
            dest.display()
        ))
    })
}

/// Re-apply preserved local `display_settings` into a freshly-downloaded cache file
/// (companion to [`preserved_display_settings`]). Best-effort per key - a failed
/// restore only means that one view toggle falls back to its default.
pub(super) fn restore_display_settings(
    db: &crate::persistence::Database,
    preserved: &HashMap<String, String>,
) {
    for (key, value) in preserved {
        if let Err(e) = db.save_display_setting(key, value) {
            tracing::warn!(error = %e, key = %key, "failed to restore local display setting after download-replace");
        }
    }
}

/// Whether a re-pull may destroy un-pushed local edits at its destination.
///
/// For the orchestration paths that serve BOTH kinds of caller with the same
/// mechanics - `speleodb_fast_forward`, the shared body behind
/// `speleodb_open_project` / `speleodb_open_project_discarding_local`, and
/// `speleodb_restore_revision`'s View-vs-Restore split - so the intent is named at
/// every call site instead of riding on a bare `bool`, and so a diff that adds a
/// waiver says `Discard` in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LocalEditsPolicy {
    /// Refuse the replace when the destination holds un-pushed edits. The default
    /// for every path the user did not explicitly point at their own work.
    Protect,
    /// Discard them: the user confirmed it (take-remote's destroy-confirmation, or
    /// the "take remote and discard local" branch of the open-project prompt).
    Discard,
}

/// [#303] What to do with the undo history riding IN the downloaded `.stalx`.
///
/// Now that a Full-mode push ships its undo stack, every server copy arrives carrying
/// the last pusher's edit history. Inheriting that IMPLICITLY is the #194 b3 hazard:
/// press Ctrl-Z on a freshly-pulled project and you replay a stranger's command, with
/// its prior-state payload, against a tree you have not looked at yet.
///
/// So the default is [`Self::Drop`] on every path that replaces the cache file, and the
/// history is only ever handed back through the ONE explicit, confirmed, lock-held route
/// that exists to ask for it: the Unwind. `undo_history_keep_is_unwind_only` pins that
/// there is exactly one `Keep` site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InheritedUndo {
    /// Empty the downloaded file's undo/redo tables. Every implicit adopt path.
    Drop,
    /// Keep them: the caller is the Unwind, and that history is what it came for.
    Keep,
}

impl AppState {
    /// Refuse when the copy of `project_id` at `dest` still holds edits the server
    /// has never seen. The gate in front of every download-replace, and the reason
    /// re-opening a project from the SpeleoDB list can no longer silently destroy a
    /// week of unpushed survey.
    ///
    /// Reads through [`AppState::local_copy_state`], so the OPEN project's live
    /// in-memory link counts - the case a naive on-disk-only check gets wrong, and
    /// the one where the user is most likely to be actively working.
    ///
    /// Shared with the pull's early check, so known local edits can refuse the
    /// download before any network work starts.
    pub(super) fn guard_local_edits(
        &self,
        project_id: &str,
        dest: &Path,
    ) -> Result<(), SpeleoDbError> {
        // `None` = the file is there but unreadable, so we CANNOT rule out un-pushed
        // edits. A guard that stands in front of an overwrite has to fail closed on
        // that: the likely causes are a hot journal after a kill or power loss (which
        // a read-only connection cannot roll back - `SQLITE_READONLY_ROLLBACK`) and a
        // transient `SQLITE_BUSY`, and in both cases a read-write open would have
        // recovered work that reading the silence as "clean" would have overwritten.
        // Refusing is recoverable in a way overwriting is not: the frontend's prompt
        // offers cancel (nothing is touched) or an explicit take-remote.
        let Some(local) = self.local_copy_state(project_id, dest) else {
            tracing::warn!(
                project_id = %project_id,
                path = %dest.display(),
                purpose = "fail closed when the local copy cannot be read",
                "speleodb: refusing to replace a local copy whose state could not be determined"
            );
            return Err(SpeleoDbError::WouldDiscardLocalEdits {
                project_id: project_id.to_owned(),
                local_path: dest.to_string_lossy().into_owned(),
                last_edit_at: None,
            });
        };
        if !local.has_unpushed_edits {
            return Ok(());
        }
        tracing::info!(
            project_id = %project_id,
            path = %dest.display(),
            is_open = local.is_open,
            purpose = "avoid silently destroying un-pushed local work on a re-pull",
            "speleodb: refusing to replace a local copy that has un-pushed edits"
        );
        Err(SpeleoDbError::WouldDiscardLocalEdits {
            project_id: project_id.to_owned(),
            local_path: dest.to_string_lossy().into_owned(),
            last_edit_at: local.last_edit_at,
        })
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "unit test fixtures fail loudly on setup errors"
)]
mod tests {
    use super::*;
    use crate::core::state::tests_support::speleodb_prod_matches;
    use crate::models::{Project, SpeleoLink};
    use crate::persistence::Database;

    const ID: &str = "11111111-1111-1111-1111-111111111111";

    /// [#303] Spec item 15's structural half, and the reason `InheritedUndo` is a named
    /// enum rather than a bool: KEEPING a downloaded copy's undo history is a waiver of
    /// the #194 b3 guard, so there must be exactly ONE place in the whole orchestration
    /// that asks for it - the Unwind, which exists to fetch that history back under an
    /// explicit, confirmed, lock-held action.
    ///
    /// A diff that adds a second `Keep` site fails here and has to justify itself.
    #[test]
    fn undo_history_keep_is_unwind_only() {
        let keeps = speleodb_prod_matches("InheritedUndo::Keep", &["speleodb_cache.rs"]);
        assert_eq!(
            keeps, 1,
            "exactly one production site may keep a downloaded copy's undo history (the \
             Unwind); found {keeps}. Every other adopt path inherits a stranger's \
             history one Ctrl-Z away from the user (#194 b3)."
        );
    }

    /// A `.stalx` at `path` linked to [`ID`], carrying the given un-pushed-edits bit.
    fn seed_linked_stalx(path: &Path, dirty: bool) {
        let mut db = Database::open(path).expect("open db");
        db.save_project(&Project::new("Local".to_owned(), None))
            .expect("save project");
        db.save_speleo_link(Some(&SpeleoLink {
            endpoint: "http://x/".to_owned(),
            project_id: ID.to_owned(),
            project_name: "Local".to_owned(),
            base_commit_sha: Some("abc123".to_owned()),
            has_unpushed_edits: dirty,
            last_edit_at: dirty.then(|| "2026-07-27T10:00:00Z".to_owned()),
            last_verify_at: None,
            undo_mode: None,
        }))
        .expect("save link");
    }

    /// A native SQLite file standing in for a downloaded native file.
    fn server_stalx() -> tempfile::NamedTempFile {
        let temp = tempfile::TempDir::new().expect("server project temp");
        let project = temp.path().join("server.stalx");
        {
            let mut db = Database::open(&project).expect("open server project");
            db.save_project(&Project::new("Server Copy".to_owned(), None))
                .expect("save server project");
        }
        let native = tempfile::NamedTempFile::new().expect("native file");
        std::fs::copy(&project, native.path()).expect("copy native file");
        native
    }
    /// Install `source` over `dest` through the session-bound replace, under the
    /// generation that is current right now (nothing moves it in these tests).
    fn install(
        state: &AppState,
        source: &Path,
        dest: &Path,
        policy: LocalEditsPolicy,
        undo: InheritedUndo,
    ) -> Result<HashMap<String, String>, SpeleoDbError> {
        state.replace_cache_for_session(
            source,
            dest,
            ID,
            policy,
            undo,
            state.speleodb_session_generation(),
        )
    }
    fn replaced_project_name(path: &Path) -> String {
        Database::open_existing(path)
            .expect("open replaced project")
            .load_project()
            .expect("load replaced project")
            .name
    }

    #[test]
    fn only_a_named_discard_gets_a_replace_past_un_pushed_edits() {
        // The behaviour the policy exists for, pinned from both sides: `Protect`
        // refuses over un-pushed work and leaves the file alone, and `Discard` is the
        // ONLY thing that gets through. A caller that wants the destruction has to
        // name it.
        let temp = tempfile::TempDir::new().expect("temp");
        let dest = temp.path().join("project.stalx");
        seed_linked_stalx(&dest, true);
        let before = std::fs::read(&dest).expect("read dest");
        let state = AppState::new().expect("state");
        let native = server_stalx();

        let err = install(
            &state,
            native.path(),
            &dest,
            LocalEditsPolicy::Protect,
            InheritedUndo::Drop,
        )
        .expect_err("un-pushed edits must refuse the protected replace");
        assert!(
            matches!(err, SpeleoDbError::WouldDiscardLocalEdits { .. }),
            "and refuse it with the typed routing signal, got {err:?}"
        );
        assert_eq!(
            std::fs::read(&dest).expect("read dest"),
            before,
            "a refusal must leave the local copy byte-identical"
        );

        install(
            &state,
            native.path(),
            &dest,
            LocalEditsPolicy::Discard,
            InheritedUndo::Drop,
        )
        .expect("the confirmed-discard waiver replaces the copy");
        assert_eq!(
            replaced_project_name(&dest),
            "Server Copy",
            "and only that waiver actually overwrites it"
        );
    }

    /// [#303] Spec item 15's behavioural half: a replace that carries
    /// `InheritedUndo::Drop` leaves every downloaded history table EMPTY, and one that
    /// carries `Keep` leaves them intact.
    ///
    /// Asserted on the FILE rather than on the in-memory stack, because that is where it
    /// has to be true: the load path reads these tables into the live stack, so a clear
    /// that only touched memory would leave the history on disk for the next open.
    #[test]
    fn a_dropped_replace_empties_the_downloaded_undo_tables() {
        /// A real `.stalx` dump entry carrying two undo rows and one redo row, standing
        /// in for a Full-mode push's snapshot.
        fn stalx_with_history() -> tempfile::NamedTempFile {
            let temp = tempfile::TempDir::new().expect("temp");
            let seeded = temp.path().join("seeded.stalx");
            {
                let mut db = Database::open(&seeded).expect("open seed");
                db.save_project(&Project::new("Pushed".to_owned(), None))
                    .expect("save project");
                db.save_undo_stack(vec![
                    (r#"{"c":1}"#.to_owned(), r#"{"u":1}"#.to_owned()),
                    (r#"{"c":2}"#.to_owned(), r#"{"u":2}"#.to_owned()),
                ])
                .expect("seed undo");
                db.save_redo_stack_with_timestamps(vec![(
                    r#"{"c":3}"#.to_owned(),
                    r#"{"u":3}"#.to_owned(),
                    None,
                )])
                .expect("seed redo");
                db.save_speleo_undo_chain(Some("older-revision"))
                    .expect("seed chain pointer");
                db.save_speleo_undo_watermark(Some("2026-08-01T00:00:00+00:00"))
                    .expect("seed watermark");
            }
            let native = tempfile::NamedTempFile::new().expect("native file");
            std::fs::copy(&seeded, native.path()).expect("copy native file");
            native
        }
        let history = |path: &Path| -> (usize, usize, Option<String>, Option<String>) {
            let db = Database::open_existing(path).expect("open replaced");
            (
                db.load_undo_stack_with_timestamps()
                    .expect("load undo")
                    .len(),
                db.load_redo_stack_with_timestamps()
                    .expect("load redo")
                    .len(),
                db.load_speleo_undo_chain().expect("load chain pointer"),
                db.load_speleo_undo_watermark().expect("load watermark"),
            )
        };

        let state = AppState::new().expect("state");
        let native = stalx_with_history();

        // Sanity: the fixture really does carry history, so the assertions below are not
        // passing vacuously.
        let temp = tempfile::TempDir::new().expect("temp");
        let kept = temp.path().join("kept.stalx");
        install(
            &state,
            native.path(),
            &kept,
            LocalEditsPolicy::Discard,
            InheritedUndo::Keep,
        )
        .expect("replace with Keep");
        assert_eq!(
            history(&kept),
            (
                2,
                1,
                Some("older-revision".to_owned()),
                Some("2026-08-01T00:00:00+00:00".to_owned()),
            ),
            "Keep leaves the downloaded history in place - what the Unwind needs"
        );

        let dropped = temp.path().join("dropped.stalx");
        install(
            &state,
            native.path(),
            &dropped,
            LocalEditsPolicy::Discard,
            InheritedUndo::Drop,
        )
        .expect("replace with Drop");
        assert_eq!(
            history(&dropped),
            (0, 0, None, None),
            "Drop empties all history metadata, so no adopt path can inherit it (#194 b3)"
        );
    }

    #[test]
    fn failed_history_cleanup_leaves_the_existing_cache_untouched() {
        let seed_dir = tempfile::TempDir::new().expect("seed temp");
        let seeded = seed_dir.path().join("seeded.stalx");
        {
            let mut db = Database::open(&seeded).expect("open seed");
            db.save_project(&Project::new("Pushed".to_owned(), None))
                .expect("save project");
            db.save_undo_stack(vec![(r#"{"c":1}"#.to_owned(), r#"{"u":1}"#.to_owned())])
                .expect("seed undo");
        }
        rusqlite::Connection::open(&seeded)
            .expect("reopen seed")
            .execute_batch(
                "CREATE TRIGGER block_undo_delete BEFORE DELETE ON undo_stack \
                 BEGIN SELECT RAISE(ABORT, 'blocked'); END;",
            )
            .expect("install blocking trigger");
        let temp = tempfile::TempDir::new().expect("temp");
        let dest = temp.path().join("project.stalx");
        std::fs::write(&dest, b"original cache").expect("seed destination");
        let before = std::fs::read(&dest).expect("read destination");
        let state = AppState::new().expect("state");

        let err = install(
            &state,
            &seeded,
            &dest,
            LocalEditsPolicy::Discard,
            InheritedUndo::Drop,
        )
        .expect_err("a downloaded file carrying a trigger must not be adopted");
        // Refused by the schema check before the cleanup could fire the trigger.
        assert!(
            err.to_string()
                .contains("the downloaded project file was refused: the file defines a trigger"),
            "{err}"
        );
        assert_eq!(std::fs::read(&dest).expect("read destination"), before);
        let remaining: Vec<_> = std::fs::read_dir(temp.path())
            .expect("directory")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(
            remaining,
            vec![std::ffi::OsString::from("project.stalx")],
            "schema rejection must remove every actual staging file"
        );
    }

    #[test]
    fn a_clean_copy_is_replaced_without_any_waiver() {
        // The other half: `Protect` is not "always refuse". With nothing un-pushed
        // there is no work to lose, so an ordinary re-pull goes straight through.
        let temp = tempfile::TempDir::new().expect("temp");
        let dest = temp.path().join("project.stalx");
        seed_linked_stalx(&dest, false);
        let state = AppState::new().expect("state");
        let native = server_stalx();

        install(
            &state,
            native.path(),
            &dest,
            LocalEditsPolicy::Protect,
            InheritedUndo::Drop,
        )
        .expect("a clean local copy needs no waiver");
        assert_eq!(
            replaced_project_name(&dest),
            "Server Copy",
            "the server copy landed"
        );
    }

    /// [#649] A server `.stalx` a newer Stalagmight saved never replaces the local
    /// copy: this build could not open it, and a save from it would push a file
    /// with the newer build's additions dropped. Refused before the history
    /// cleanup writes into the staged copy and before the rename.
    #[test]
    fn a_download_from_a_newer_schema_never_replaces_the_cache() {
        let temp = tempfile::TempDir::new().expect("temp");
        let source = temp.path().join("download.stalx");
        Database::open(&source)
            .expect("seed")
            .save_project(&Project::new("From the future".into(), None))
            .expect("save");
        rusqlite::Connection::open(&source)
            .expect("connection")
            .pragma_update(None, "user_version", crate::persistence::SCHEMA_VERSION + 1)
            .expect("stamp newer schema");
        let dest = temp.path().join("project.stalx");
        seed_linked_stalx(&dest, false);
        let before = std::fs::read(&dest).expect("destination");
        let state = AppState::new().expect("state");
        let err = install(
            &state,
            &source,
            &dest,
            LocalEditsPolicy::Protect,
            InheritedUndo::Drop,
        )
        .expect_err("newer schema refused");
        let SpeleoDbError::NewerSchema(newer) = &err else {
            panic!("wrong refusal: {err:?}");
        };
        assert_eq!(
            *newer,
            crate::persistence::NewerSchema {
                file: "project.stalx".into(),
                found: crate::persistence::SCHEMA_VERSION + 1,
                supported: crate::persistence::SCHEMA_VERSION,
            }
        );
        assert_eq!(err.to_string(), newer.to_string());
        assert_eq!(std::fs::read(&dest).expect("destination"), before);
        let mut names = std::fs::read_dir(temp.path())
            .expect("directory")
            .map(|entry| entry.expect("entry").file_name())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(
            names,
            vec![
                std::ffi::OsString::from("download.stalx"),
                std::ffi::OsString::from("project.stalx")
            ]
        );
    }
}
