//! The execute pipeline of the station-graph model (plan 9.1; C0).
//!
//! One pipeline runs every recorded edit: [`AppState::execute`] for a
//! command or a batch, and every bypass recipe (split, combine, delete-split,
//! imports, repair, settings, elevation) through [`AppState::run_recorded`]
//! with its own record:
//!
//! 1. `command_lock` (the caller holds it), the read-only gates (SpeleoDB
//!    edit session; a file made with a newer Stalagmight);
//! 2. the record (kind, label worded with the names in force now, stable
//!    refs, recorded preview rows);
//! 3. `before = project.clone()`;
//! 4. the mutation through the survey operations, which resolve positional
//!    slots to `StationId`s (`resolve_slot`, refusing an out-of-range slot),
//!    settle anchors and run the orphan-closure sweep themselves for the
//!    topology edits; a refusal restores `before`;
//! 5. `validate()`: a violation restores `before` and refuses;
//! 6. `capture(&before, &project)`: an empty changeset is a provable no-op
//!    (no entry, the redo branch kept, nothing dirtied);
//! 7. `modified_at` stamped (the model never does), the revision bumped,
//!    the entry pushed (redo cleared), the file marked dirty, the SpeleoDB
//!    dirty bit and Unwind position updated.

use super::AppState;
use crate::core::{Command, CommandResult, ReversedLine, ValidationWarning};
use crate::models::survey::changeset::{capture, HistoryRefs, RecordedPreview};
use crate::models::survey::{HistoryEntry, Project};

/// What a recorded edit says about itself (plan 8.3's entry minus the time
/// and the changeset).
#[derive(Debug, Clone, Default)]
pub(crate) struct Record {
    pub(crate) kind: String,
    pub(crate) label: String,
    pub(crate) refs: HistoryRefs,
    pub(crate) preview: RecordedPreview,
}

impl Record {
    /// The record of `cmd` on `project` before it runs.
    pub(crate) fn of(project: &Project, cmd: &Command) -> Self {
        Self {
            kind: crate::core::command_kind(cmd).to_string(),
            label: super::labels::entry_label(project, cmd),
            refs: super::labels::command_refs(project, cmd),
            preview: super::labels::command_preview(project, cmd),
        }
    }
}

/// What a command's mutation reports besides the new state.
#[derive(Debug, Default)]
pub(crate) struct Applied {
    pub(crate) warnings: Vec<ValidationWarning>,
    pub(crate) line_id: Option<u32>,
    pub(crate) line_ids: Vec<u32>,
    pub(crate) shot_id: Option<u32>,
    pub(crate) reversed: Vec<ReversedLine>,
    /// The Section a `CreateCaveSection` created (#327).
    pub(crate) cave_section_id: Option<u32>,
    /// What the command did to identities on its own (RS 7 "Notices"):
    /// rule-L letters, restored archived identities, dropped closure
    /// archives.
    pub(crate) identities: crate::models::IdentityNotice,
}

impl Applied {
    /// A batch sub-command's report folded into the batch's, as 0.6.0's
    /// `execute_batch` folded it: every sub-command's `line_id` and
    /// `line_ids` go to `line_ids` (the batch itself names no `line_id`),
    /// the warnings in order; the reverses are parity-reduced at the end
    /// ([`Self::into_result`]).
    fn merge(&mut self, other: Applied) {
        self.warnings.extend(other.warnings);
        self.line_ids.extend(other.line_id);
        self.line_ids.extend(other.line_ids);
        if other.shot_id.is_some() {
            self.shot_id = other.shot_id;
        }
        self.reversed.extend(other.reversed);
        self.identities.extend(other.identities);
    }

    fn into_result(self) -> CommandResult {
        let mut result = CommandResult::success().with_warnings(self.warnings);
        result.line_id = self.line_id;
        result.line_ids = self.line_ids;
        result.shot_id = self.shot_id;
        result.cave_section_id = self.cave_section_id;
        if !self.identities.is_empty() {
            result.identities = Some(self.identities);
        }
        let reversed = net_reversed(self.reversed);
        if !reversed.is_empty() {
            result = result.with_reversed_lines(reversed);
        }
        result
    }
}

/// The lines reversed an ODD number of times, in first-occurrence order
/// (0.6.0's `Command::net_reversed`): a line a batch reverses twice is not
/// renumbered at all, so it is not reported.
fn net_reversed(reversed: Vec<ReversedLine>) -> Vec<ReversedLine> {
    let mut counts: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
    for r in &reversed {
        *counts.entry(r.line_id).or_insert(0) += 1;
    }
    let mut seen = std::collections::HashSet::new();
    reversed
        .into_iter()
        .filter(|r| counts.get(&r.line_id).is_some_and(|n| n % 2 == 1) && seen.insert(r.line_id))
        .collect()
}

/// The message a frozen shot's geometry edit is refused with (S12: the
/// model's operations do not check it; the command layer does).
pub(crate) const FROZEN_GEOMETRY: &str =
    "Cannot edit geometry of a frozen shot. This shot is part of an adjusted survey network.";

/// Apply one command's mutation to `project` (no snapshot, no validation:
/// the pipeline does both). Each family's `apply` lives in its own file.
/// `record` is the entry being recorded, for the preview rows or label
/// refinements only the result can say.
pub(crate) fn apply_command(
    project: &mut Project,
    record: &mut Record,
    cmd: &Command,
) -> Result<Applied, String> {
    use super::{
        command_cave_section, command_comment, command_credit, command_line, command_move,
        command_shot, command_station, command_station_name, import_tracking,
    };
    match cmd {
        Command::CreateLine { .. }
        | Command::DeleteLine { .. }
        | Command::UpdateLineMetadata { .. }
        | Command::ReverseLine { .. } => command_line::apply(project, record, cmd),
        Command::CreateShot { .. }
        | Command::UpdateShot { .. }
        | Command::DeleteShot { .. }
        | Command::SetShotKind { .. } => command_shot::apply(project, record, cmd),
        Command::EditShotCredit { .. } => command_credit::apply(project, record, cmd),
        Command::MergeStations { .. }
        | Command::UnmergeStations { .. }
        | Command::UnmergeStationPair { .. }
        | Command::CreateLoopClosure { .. }
        | Command::RemoveLoopClosure { .. }
        | Command::SetZeroDatum { .. }
        | Command::RemoveZeroDatum { .. } => command_station::apply(project, record, cmd),
        Command::SetStationName { .. } => command_station_name::apply(project, record, cmd),
        Command::SetStationComment { .. } | Command::MoveComment { .. } => {
            command_comment::apply(project, record, cmd)
        }
        Command::MoveLine { .. } | Command::MoveSegment { .. } | Command::MoveStation { .. } => {
            command_move::apply(project, record, cmd)
        }
        Command::CreateCaveSection { .. }
        | Command::UpdateCaveSection { .. }
        | Command::DeleteCaveSection { .. }
        | Command::MoveLinesToCaveSection { .. }
        | Command::ApplyCaveSectionColor { .. } => {
            command_cave_section::apply(project, record, cmd)
        }
        Command::UntrackImport { .. } => import_tracking::apply(project, record, cmd),
        Command::Batch { commands, .. } => {
            let mut applied = Applied::default();
            // Several slots of one station move it once (plan 9.1).
            let mut station_moves = command_move::StationMoves::default();
            let mut rest = commands.as_slice();
            while let Some((sub, tail)) = rest.split_first() {
                if !AppState::is_batch_allowed(sub) {
                    // 0.6.0's words, naming the command as the history would.
                    return Err(format!(
                        "Command '{}' is not allowed in batch context",
                        super::labels::command_label(project, sub)
                    ));
                }
                if station_moves.repeats(project, sub) {
                    rest = tail;
                    continue;
                }
                // A run of shot updates (a data-table paste) is one survey
                // pass: the same updates one after another, to the bit
                // (`ShotEditRun`). An update reports nothing to fold.
                let updates = rest
                    .iter()
                    .take_while(|c| matches!(c, Command::UpdateShot { .. }))
                    .count();
                if updates > 0 {
                    command_shot::apply_updates(project, &rest[..updates])?;
                    rest = &rest[updates..];
                    continue;
                }
                applied.merge(apply_command(project, record, sub)?);
                rest = tail;
            }
            // A reverse is reported for the lines the batch leaves (0.6.0's
            // `settle_reversed_lines`): a line it reversed and then deleted
            // has nothing for the frontend to mirror.
            applied
                .reversed
                .retain(|r| project.line(r.line_id).is_some_and(|l| !l.shots.is_empty()));
            Ok(applied)
        }
        Command::CreateProject { .. }
        | Command::OpenProject { .. }
        | Command::SaveProject { .. }
        | Command::CloseProject
        | Command::MergeProjects { .. }
        | Command::AddProjectFile { .. }
        | Command::RepairIntegrity
        | Command::UpdateProjectSettings
        | Command::SetSurfaceElevation => Err(format!(
            "{} is not an edit of the open project",
            crate::core::command_kind(cmd)
        )),
    }
}

impl AppState {
    pub(super) fn is_batch_allowed(cmd: &Command) -> bool {
        // The allowlist also prevents a path-bearing lifecycle command from being
        // nested inside an otherwise undoable Batch and uploaded in Full mode.
        matches!(
            cmd,
            Command::CreateLine { .. }
                | Command::CreateLoopClosure { .. }
                | Command::MergeStations { .. }
                | Command::MoveLine { .. }
                | Command::MoveSegment { .. }
                | Command::MoveStation { .. }
                | Command::SetZeroDatum { .. }
                | Command::UpdateShot { .. }
                // Reversing a multi-selection is one user action: batching gives
                // it all-or-nothing rollback and a single undo entry, instead of
                // N commands that can strand the project half-reversed.
                | Command::ReverseLine { .. }
                // Withdrawing an abandoned import is one user action too (#394) -
                // the deletes and the dedup bookkeeping that describes them have
                // to come back together, so both are batchable.
                | Command::DeleteLine { .. }
                | Command::UntrackImport { .. }
                // Metadata only, and already one step for many shots; a batch
                // may still group it with other edits of the same action.
                | Command::EditShotCredit { .. }
                // A data-table paste of station names is one user action.
                | Command::SetStationName { .. }
                // Edit Line's Save renames / recolours a line AND edits its
                // shots' credit in one user action, so both go in one batch and
                // come back with one undo.
                | Command::UpdateLineMetadata { .. }
                // Sections (#327) are metadata like the line edits above, so
                // an action that also moves a line into one stays one step.
                | Command::CreateCaveSection { .. }
                | Command::UpdateCaveSection { .. }
                | Command::DeleteCaveSection { .. }
                | Command::MoveLinesToCaveSection { .. }
                | Command::ApplyCaveSectionColor { .. }
        )
    }

    /// The read-only refusal of a file this build cannot rewrite (plan 6.4).
    pub(super) fn guard_file_writable(&self) -> Result<(), CommandResult> {
        let read_only = self.with_file(|file| file.extras.read_only.is_some());
        if read_only {
            return Err(CommandResult::failure(format!(
                "{}: this project was made with a newer Stalagmight and is open read-only",
                super::open_file::READ_ONLY_FILE_CODE
            )));
        }
        Ok(())
    }

    /// The pipeline's steps 2-7 (module docs) for one recorded edit. The
    /// caller holds `command_lock` and has passed the edit-session gate.
    /// `record` words the entry on the project before the edit; `mutate`
    /// changes it (any partial change is rolled back on `Err`) and may add
    /// to the record (preview rows, a refined label) from what it did.
    pub(crate) fn run_recorded<T>(
        &self,
        record: impl FnOnce(&Project) -> Record,
        mutate: impl FnOnce(&mut Project, &mut Record) -> Result<T, String>,
    ) -> Result<T, CommandResult> {
        self.run_recorded_from(record, |project, record, _before| mutate(project, record))
    }

    /// [`Self::run_recorded`] whose mutation also reads the project as it
    /// was before the edit (the pipeline's own snapshot), for a recipe that
    /// words its result from both states and would otherwise clone the
    /// survey a second time (Combine Lines, C4).
    pub(crate) fn run_recorded_from<T>(
        &self,
        record: impl FnOnce(&Project) -> Record,
        mutate: impl FnOnce(&mut Project, &mut Record, &Project) -> Result<T, String>,
    ) -> Result<T, CommandResult> {
        self.guard_file_writable()?;
        let mut guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(project) = guard.as_mut() else {
            return Err(CommandResult::failure("No project open".to_string()));
        };
        let mut record = record(project);
        let before = project.clone();
        let value = match mutate(project, &mut record, &before) {
            Ok(value) => value,
            Err(message) => {
                *project = before;
                return Err(CommandResult::failure(message));
            }
        };
        let violations = project.validate();
        if let Some(first) = violations.first() {
            tracing::error!(
                count = violations.len(),
                first = %first,
                kind = %record.kind,
                "an edit would have left the survey invalid; rolled back"
            );
            let message = format!("The edit would leave the survey invalid: {first}");
            *project = before;
            return Err(CommandResult::failure(message));
        }
        let changes = capture(&before, project);
        if changes.is_empty() {
            return Ok(value);
        }
        project.touch();
        self.bump_revision();
        let entry = HistoryEntry {
            timestamp: chrono::Utc::now(),
            kind: record.kind,
            label: record.label,
            refs: record.refs,
            preview: record.preview,
            changes,
        };
        self.with_file(|file| file.dirty = true);
        let pushed = self
            .undo_stack
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .record(entry);
        if let Err(e) = pushed {
            // The edit stands; only its history entry could not be encoded
            // (a model bug: validate refuses non-finite numbers). Undo must
            // not step over a missing entry, so the stacks are cleared.
            tracing::error!(error = ?e, "the undo entry of an edit could not be encoded; history cleared");
            self.undo_stack
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clear();
        }
        drop(guard);
        self.mark_remote_forward_edit();
        Ok(value)
    }

    /// Execute a command and return the result.
    pub fn execute(&self, cmd: Command) -> CommandResult {
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        match &cmd {
            Command::CreateProject { name, location } => {
                return self.create_project(name.clone(), location.clone());
            }
            Command::OpenProject { path } => return self.open_project(path.clone()),
            Command::SaveProject { path } => return self.save_project(path.clone()),
            Command::CloseProject => return self.close_project(),
            Command::MergeProjects { .. } => return self.execute_merge_projects(&cmd),
            Command::RepairIntegrity
            | Command::UpdateProjectSettings
            | Command::SetSurfaceElevation => {
                return CommandResult::failure(format!(
                    "{} has its own entry point",
                    crate::core::command_kind(&cmd)
                ));
            }
            _ => {}
        }
        // A background lock re-verify (#202) that found the write lock gone
        // shuts the gate and refuses this edit with the re-verify code.
        if let Err(f) = self.speleodb_collect_reverify() {
            return f;
        }
        if let Err(f) = self.guard_mutation_for(&cmd) {
            return f;
        }
        self.speleodb_verify_lock_if_expired();
        // Reads its source file before it takes the project lock, so it is a
        // recipe (`project_add.rs`) rather than a family `apply`.
        if matches!(cmd, Command::AddProjectFile { .. }) {
            return self.execute_add_project_file(&cmd);
        }
        tracing::info!(
            command = crate::core::command_kind(&cmd),
            purpose = "apply user action",
            "Executing command to update project state"
        );
        match self.run_recorded(|p| Record::of(p, &cmd), |p, r| apply_command(p, r, &cmd)) {
            Ok(applied) => applied.into_result(),
            Err(failure) => failure,
        }
    }
}

#[cfg(test)]
impl AppState {
    /// The model's invariant violations (plan section 2) of the open project,
    /// as messages: the tests' integrity oracle. A loaded project is valid by
    /// construction (load refuses an invalid file; salvage opens read-only)
    /// and every edit and file-decoded history step validates before it
    /// commits, so this is empty unless a bug let one through. (The app's
    /// integrity banner and its `check_project_integrity` command read the
    /// same and could never show anything; both are gone.)
    pub(crate) fn integrity_violation_messages(&self) -> Vec<String> {
        let guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.as_ref().map_or_else(Vec::new, |p| {
            p.validate().iter().map(ToString::to_string).collect()
        })
    }
}
