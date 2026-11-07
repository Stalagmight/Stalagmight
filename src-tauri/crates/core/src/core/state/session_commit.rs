//! [#576 A5] The project-slot commit: the ONE place that writes every cell
//! describing the open project.
//!
//! `AppState` keeps the open project's state in independently locked cells (the
//! slot, the database handle, the undo stack and its bounds, the edit gates, the
//! history/newer-file/unread-settings verdicts, the workspace mode, the display
//! cache, the deferred-save flag). A project-slot commit - both loads, both
//! creates, and close - has to write all of them, because a cell a commit leaves
//! alone keeps the PREVIOUS project's value. The four commits used to be written
//! out by hand, and they drifted: File > New kept a Full-mode project's 32 MiB
//! byte budget (#576 A18(f)), close and both creates kept its inherited-history
//! watermark (NF-AN3-6), the in-memory create kept its database handle, and that
//! create wrote three of the gates after dropping the `project` guard.
//!
//! Every commit now builds a [`SessionCommit`] - a value naming every cell - and
//! hands it to [`AppState::commit_open_session`], which writes them all under ONE
//! `project` acquisition. A cell is therefore written by every commit or by none,
//! and adding a cell means adding a field the compiler then demands at every
//! construction site. Pinned behaviourally by `session_commit_tests.rs`.
//!
//! [#576 A1] Every commit also holds `command_lock`, proven by a [`CommandsHeld`]
//! argument. `execute`/undo/redo hold that lock for a whole command but take and
//! release `project` several times inside it (undo data, mutation, epilogue, undo
//! push); a commit that took only `project` could land in between and leave the
//! OLD project's undo entry on the NEW project's freshly reset stack (the #413
//! class). The caller takes the lock AFTER its slow file read, so a load still
//! never blocks edits for the length of the read. Lock order, outermost first:
//! `speleodb_lock_ops` -> `command_lock` -> `project` -> `database` ->
//! `undo_stack` (docs/undo-redo.md, Lock Ordering). Pinned by
//! `lifecycle_lock_tests.rs`.

use super::project_slot::ProjectSlot;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::MutexGuard;

use chrono::{DateTime, Utc};

use super::AppState;
use crate::models::{Project, UndoPolicy};
use crate::persistence::Database;

/// Proof that the holder owns `command_lock`, the lock that serializes every
/// command, undo and redo. A project-slot commit takes one as an argument, so a
/// commit cannot be written without it.
pub(super) struct CommandsHeld<'a> {
    _guard: MutexGuard<'a, ()>,
}

/// One persisted history row as the load reads it: command, undo data, timestamp.
pub(super) type HistoryRow = (String, String, Option<String>);

/// The history a commit installs, and the bounds it lives under.
pub(super) struct SessionHistory {
    /// Decides the depth bound: the byte budget for a Full-mode linked project,
    /// the configured entry window for everything else. Armed BEFORE the entries
    /// go in, because the restore's own `enforce_depth_limit` trims to whichever
    /// bound is in force at that instant (#303).
    pub(super) policy: UndoPolicy,
    /// The inherited-history watermark (#303) that floors every new undo stamp.
    /// Always written, `None` included, so a value cannot outlive its project.
    pub(super) inherited_through: Option<DateTime<Utc>>,
    pub(super) undo: Vec<HistoryRow>,
    pub(super) redo: Vec<HistoryRow>,
}

impl SessionHistory {
    /// No history: the local entry window and no watermark. What a create and a
    /// close install.
    pub(super) fn none() -> Self {
        Self {
            policy: UndoPolicy::local(),
            inherited_through: None,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }
}

/// Every per-project cell a project-slot commit writes. No `Default`: each
/// construction site names every field, so a new cell cannot be forgotten.
pub(super) struct SessionCommit {
    /// The project to install; `None` closes.
    pub(super) project: Option<Project>,
    pub(super) database: Option<Database>,
    /// The project has no `.stalx` yet (#92): its first import must create one.
    pub(super) initial_save_deferred: bool,
    /// The read-only gate (#191): `false` only for a remote project opened
    /// without the write lock. Force-edit (#213) is always cleared.
    pub(super) edit_enabled: bool,
    /// [#473] The project's persisted history could not be read.
    pub(super) history_discarded: bool,
    /// [#576 B6-D1] The schema version of a file a newer build saved.
    pub(super) newer_file_schema: Option<i32>,
    /// [#576 B15] Stored settings the load could not read.
    pub(super) unreadable_project_settings: Vec<String>,
    /// What the in-memory display-settings cache holds after the commit: the
    /// defaults for a project with no database to read them from, else nothing.
    pub(super) display_settings: Vec<(String, String)>,
    pub(super) history: SessionHistory,
    /// The file to put at the top of Recent Projects (one that exists on disk).
    pub(super) recent: Option<PathBuf>,
}

impl SessionCommit {
    /// A brand-new local project: editable, with nothing discarded or unread and
    /// no history. The caller sets the deferred flag, display seed and recents.
    pub(super) fn new_local(project: Project, database: Option<Database>) -> Self {
        Self {
            project: Some(project),
            database,
            initial_save_deferred: false,
            edit_enabled: true,
            history_discarded: false,
            newer_file_schema: None,
            unreadable_project_settings: Vec::new(),
            display_settings: Vec::new(),
            history: SessionHistory::none(),
            recent: None,
        }
    }

    /// The empty slot a close leaves: every cell at its no-project value, which is
    /// also what the next project to open starts from until its own commit.
    pub(super) fn closed() -> Self {
        Self {
            project: None,
            database: None,
            initial_save_deferred: false,
            edit_enabled: true,
            history_discarded: false,
            newer_file_schema: None,
            unreadable_project_settings: Vec::new(),
            display_settings: Vec::new(),
            history: SessionHistory::none(),
            recent: None,
        }
    }
}

impl AppState {
    /// Take `command_lock` for a project-slot commit (or for anything else that must
    /// not interleave with a command). Waits for an in-flight command to finish.
    ///
    /// RECOVERS a poisoned lock rather than refusing, like every lock (#576 A11):
    /// refusing would make every later load, create and close impossible for the
    /// rest of the session.
    ///
    /// Never call it while holding `project`, `database` or `undo_stack` (they sit
    /// below it), and never take `speleodb_lock_ops` while holding it (that one sits
    /// above it).
    pub(super) fn hold_commands(&self) -> CommandsHeld<'_> {
        CommandsHeld {
            _guard: self.command_lock.lock(),
        }
    }

    /// Install `commit` as the open session, under one `project` acquisition taken
    /// inside the caller's `command_lock`.
    pub(super) fn commit_open_session(&self, commands: &CommandsHeld<'_>, commit: SessionCommit) {
        let mut slot = self.project.lock();
        self.commit_open_session_into(commands, &mut slot, commit);
    }

    /// [`Self::commit_open_session`] with the `project` guard supplied by a caller
    /// that must DECIDE under the same acquisition (the delete's check-and-teardown).
    ///
    /// Everything is written while that guard is held: the delete reasons "strictly
    /// before the commit, or strictly after it", and a slot committed in its own span
    /// with the handle, history and recents in later ones let a delete land in the
    /// middle and tear down a half-built session (round 5). Lock order is the
    /// documented one - `project` outermost of {project, database, undo_stack}, the
    /// rest are leaves. Every lock is RECOVERED from poisoning: the values written
    /// here replace whatever a panicking holder left, and skipping a write is what
    /// leaves a previous project's cell behind.
    pub(super) fn commit_open_session_into(
        &self,
        _commands: &CommandsHeld<'_>,
        slot: &mut ProjectSlot,
        commit: SessionCommit,
    ) {
        let SessionCommit {
            project,
            database,
            initial_save_deferred,
            edit_enabled,
            history_discarded,
            newer_file_schema,
            unreadable_project_settings,
            display_settings,
            history,
            recent,
        } = commit;

        // [#576 NF-W8FE-3] The file this open is committed from, beside the slot:
        // what a display-setting write made for it names (`save_display_setting_for`).
        // [#576 RV10b-4] A new session: the files an earlier one was saved through go.
        *self.opened_from.lock() = vec![project.as_ref().and_then(|p| p.file_path.clone())];
        slot.set(project);
        // A session boundary, bumped under the guard per the `speleodb_session` rule.
        self.bump_speleodb_session();
        // The verdicts describe exactly the project in the slot, so they are written
        // under the acquisition that commits it, unconditionally: a clean commit
        // clears the previous project's verdict by assignment.
        self.set_history_discarded(history_discarded);
        self.set_newer_file_schema(newer_file_schema);
        self.set_unreadable_project_settings(unreadable_project_settings);
        // [#576 C5] A project opens in Survey, here as in the frontend.
        self.reset_workspace_mode();
        self.set_edit_enabled(edit_enabled);
        // #213: force-edit is per-session; no commit starts force-editing.
        self.set_force_editing(false);

        *self.database.lock() = database;
        self.initial_save_deferred
            .store(initial_save_deferred, Ordering::Relaxed);

        {
            let mut cache = self.display_settings_cache.lock();
            cache.clear();
            cache.extend(display_settings);
        }
        self.invalidate_adjusted_cache();

        {
            let SessionHistory {
                policy,
                inherited_through,
                undo,
                redo,
            } = history;
            let mut stack = self.undo_stack.lock();
            stack.clear();
            // [#303] The bound first, then the watermark, then the entries: see the
            // fields' docs for why each must precede the restore.
            super::undo_policy::arm_undo_depth_limit(&mut stack, policy);
            stack.set_inherited_through(inherited_through);
            if !(undo.is_empty() && redo.is_empty()) {
                stack.load_from_persisted_with_timestamps(undo, redo);
                // ...and against the project they will be replayed onto: a restored
                // entry that cannot find its shot fails and is pushed straight back,
                // sealing off everything under it (#413).
                if let Some(project) = slot.as_ref() {
                    stack.drop_entries_addressing_a_missing_shot(project);
                }
                tracing::info!(
                    undo = stack.len(),
                    purpose = "restore edit history",
                    "Restored undo/redo stacks from database"
                );
            }
        }

        // Last, and still under the guard: the delete prunes recents only after its
        // own teardown, so the row added here must not land between the two.
        if let Some(path) = recent {
            let mut recents = self.recent_projects.lock();
            recents.retain(|p| p != &path);
            recents.insert(0, path);
            // Under the config write lock (#576 RV4-R-M2).
            if let Err(e) = self.persist_recent_projects(&mut recents, true) {
                tracing::warn!(
                    error = %e,
                    purpose = "persist recent projects list",
                    "Failed to save config with recent projects"
                );
            }
        }
    }
}
