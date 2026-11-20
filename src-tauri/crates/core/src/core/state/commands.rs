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
use crate::core::stub::NotPorted;
use crate::core::{Command, CommandResult, IntegrityCheckResult, ReversedLine, ValidationWarning};
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
            label: super::labels::command_label(project, cmd),
            refs: super::labels::command_refs(project, cmd),
            preview: RecordedPreview::default(),
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
}

impl Applied {
    fn merge(&mut self, other: Applied) {
        self.warnings.extend(other.warnings);
        if other.line_id.is_some() {
            self.line_id = other.line_id;
        }
        self.line_ids.extend(other.line_ids);
        if other.shot_id.is_some() {
            self.shot_id = other.shot_id;
        }
        self.reversed.extend(other.reversed);
    }

    fn into_result(self) -> CommandResult {
        let mut result = CommandResult::success().with_warnings(self.warnings);
        result.line_id = self.line_id;
        result.line_ids = self.line_ids;
        result.shot_id = self.shot_id;
        if !self.reversed.is_empty() {
            result = result.with_reversed_lines(self.reversed);
        }
        result
    }
}

/// The failure text of a command family C0 has not ported.
fn not_ported(what: &'static str) -> String {
    tracing::warn!(
        what,
        "reached a command the station-graph switch has not ported yet"
    );
    NotPorted(what).to_string()
}

/// The message a frozen shot's geometry edit is refused with.
const FROZEN_GEOMETRY: &str =
    "Cannot edit geometry of a frozen shot. This shot is part of an adjusted survey network.";

/// Apply one command's mutation to `project` (no record, no validation: the
/// pipeline does both). Every family not ported in C0 refuses with a typed
/// "not yet ported" failure.
pub(crate) fn apply_command(project: &mut Project, cmd: &Command) -> Result<Applied, String> {
    match cmd {
        Command::UpdateShot {
            line_id,
            shot_id,
            updates,
        } => {
            if updates.touches_geometry() {
                let index = project
                    .line(*line_id)
                    .and_then(|l| l.shots.iter().position(|s| s.id == *shot_id));
                if index.is_some_and(|i| project.is_shot_frozen(*line_id, i)) {
                    return Err(FROZEN_GEOMETRY.to_string());
                }
            }
            project
                .update_shot(*line_id, *shot_id, updates)
                .map_err(|e| e.to_string())?;
            Ok(Applied::default())
        }
        Command::SetShotKind {
            line_id,
            shot_id,
            kind,
        } => {
            project
                .set_shot_kind(*line_id, *shot_id, *kind)
                .map_err(|e| e.to_string())?;
            Ok(Applied::default())
        }
        Command::ReverseLine {
            line_id,
            swap_left_right,
        } => {
            let shot_count = project
                .line(*line_id)
                .map(|l| u32::try_from(l.shots.len()).unwrap_or(u32::MAX))
                .ok_or_else(|| format!("Line {line_id} not found"))?;
            project
                .reverse_line(*line_id, *swap_left_right)
                .map_err(|e| e.to_string())?;
            Ok(Applied {
                reversed: vec![ReversedLine {
                    line_id: *line_id,
                    shot_count,
                }],
                ..Applied::default()
            })
        }
        Command::DeleteLine { line_id } => {
            project.delete_line(*line_id).map_err(|e| e.to_string())?;
            Ok(Applied::default())
        }
        Command::MergeStations {
            station_a,
            station_b,
        } => {
            let (a, b) = project
                .resolve_slot_pair(*station_a, *station_b)
                .map_err(|e| e.to_string())?;
            project
                .merge_stations(a.station, b.station)
                .map_err(|e| e.to_string())?;
            Ok(Applied::default())
        }
        Command::Batch { commands, .. } => {
            let mut applied = Applied::default();
            for sub in commands {
                if !AppState::is_batch_allowed(sub) {
                    return Err(format!(
                        "{} cannot be part of a batch",
                        crate::core::command_kind(sub)
                    ));
                }
                applied.merge(apply_command(project, sub)?);
            }
            Ok(applied)
        }
        Command::CreateLine { .. } => Err(not_ported("execute::create_line")),
        Command::UpdateLineMetadata { .. } => Err(not_ported("execute::update_line_metadata")),
        Command::CreateShot { .. } => Err(not_ported("execute::create_shot")),
        Command::DeleteShot { .. } => Err(not_ported("execute::delete_shot")),
        Command::EditShotCredit { .. } => Err(not_ported("execute::edit_shot_credit")),
        Command::UnmergeStations { .. } => Err(not_ported("execute::unmerge_stations")),
        Command::UnmergeStationPair { .. } => Err(not_ported("execute::unmerge_station_pair")),
        Command::CreateLoopClosure { .. } => Err(not_ported("execute::create_loop_closure")),
        Command::RemoveLoopClosure { .. } => Err(not_ported("execute::remove_loop_closure")),
        Command::SetZeroDatum { .. } => Err(not_ported("execute::set_zero_datum")),
        Command::RemoveZeroDatum { .. } => Err(not_ported("execute::remove_zero_datum")),
        Command::SetStationComment { .. } => Err(not_ported("execute::set_station_comment")),
        Command::SetStationName { .. } => Err(not_ported("execute::set_station_name")),
        Command::MoveLine { .. } => Err(not_ported("execute::move_line")),
        Command::MoveSegment { .. } => Err(not_ported("execute::move_segment")),
        Command::MoveStation { .. } => Err(not_ported("execute::move_station")),
        Command::MoveComment { .. } => Err(not_ported("execute::move_comment")),
        Command::UntrackImport { .. } => Err(not_ported("execute::untrack_import")),
        Command::CreateCaveSection { .. }
        | Command::UpdateCaveSection { .. }
        | Command::DeleteCaveSection { .. }
        | Command::MoveLinesToCaveSection { .. }
        | Command::ApplyCaveSectionColor { .. } => Err(not_ported("execute::cave_sections")),
        Command::AddProjectFile { .. } => Err(not_ported("execute::add_project_file")),
        Command::CreateProject { .. }
        | Command::OpenProject { .. }
        | Command::SaveProject { .. }
        | Command::CloseProject
        | Command::MergeProjects { .. }
        | Command::RepairIntegrity
        | Command::UpdateProjectSettings
        | Command::SetSurfaceElevation => Err(format!(
            "{} is not an edit of the open project",
            crate::core::command_kind(cmd)
        )),
    }
}

impl AppState {
    fn is_lifecycle_passthrough(cmd: &Command) -> bool {
        use Command::*;
        match cmd {
            CreateProject { .. }
            | OpenProject { .. }
            | SaveProject { .. }
            | CloseProject
            | MergeProjects { .. } => true,
            CreateLine { .. }
            | DeleteLine { .. }
            | UpdateLineMetadata { .. }
            | ReverseLine { .. }
            | CreateShot { .. }
            | UpdateShot { .. }
            | DeleteShot { .. }
            | SetShotKind { .. }
            | EditShotCredit { .. }
            | MergeStations { .. }
            | UnmergeStations { .. }
            | UnmergeStationPair { .. }
            | CreateLoopClosure { .. }
            | RemoveLoopClosure { .. }
            | SetZeroDatum { .. }
            | RemoveZeroDatum { .. }
            | SetStationComment { .. }
            | SetStationName { .. }
            | MoveLine { .. }
            | MoveSegment { .. }
            | MoveStation { .. }
            | MoveComment { .. }
            | Batch { .. }
            | UntrackImport { .. }
            | CreateCaveSection { .. }
            | UpdateCaveSection { .. }
            | DeleteCaveSection { .. }
            | MoveLinesToCaveSection { .. }
            | ApplyCaveSectionColor { .. }
            // Adding a file INTO the open project is an edit of it (#674),
            // unlike MergeProjects, which writes a new file.
            | AddProjectFile { .. }
            | RepairIntegrity
            | UpdateProjectSettings
            | SetSurfaceElevation => false,
        }
    }


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
    fn guard_file_writable(&self) -> Result<(), CommandResult> {
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
    /// changes it (any partial change is rolled back on `Err`). `Ok(None)`
    /// never happens; `Ok(Some(value))` whether or not anything changed.
    pub(crate) fn run_recorded<T>(
        &self,
        record: impl FnOnce(&Project) -> Record,
        mutate: impl FnOnce(&mut Project) -> Result<T, String>,
    ) -> Result<T, CommandResult> {
        self.guard_file_writable()?;
        let mut guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(project) = guard.as_mut() else {
            return Err(CommandResult::failure("No project open".to_string()));
        };
        let record = record(project);
        let before = project.clone();
        let value = match mutate(project) {
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
            Command::MergeProjects { .. } => {
                return crate::core::stub::not_ported("execute::merge_projects");
            }
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
        tracing::info!(
            command = crate::core::command_kind(&cmd),
            purpose = "apply user action",
            "Executing command to update project state"
        );
        match self.run_recorded(|p| Record::of(p, &cmd), |p| apply_command(p, &cmd)) {
            Ok(applied) => applied.into_result(),
            Err(failure) => failure,
        }
    }

    /// The model's invariant violations (plan section 2) of the open project,
    /// as messages. A loaded project is valid by construction (load refuses
    /// an invalid file; salvage opens read-only), so this is empty unless a
    /// bug let one through.
    pub fn integrity_violation_messages(&self) -> Vec<String> {
        let guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.as_ref().map_or_else(Vec::new, |p| {
            p.validate().iter().map(ToString::to_string).collect()
        })
    }

    pub fn integrity_check(&self) -> IntegrityCheckResult {
        let messages = self.integrity_violation_messages();
        IntegrityCheckResult {
            save_blocked: !messages.is_empty(),
            messages,
        }
    }

    /// Repair is the salvage route now (plan 6.6), ported by the recovery
    /// work package. Reports what is left, changes nothing.
    pub fn repair_open_project_integrity(&self) -> crate::core::IntegrityRepairResult {
        let _: Option<()> =
            crate::core::stub::not_ported("commands::repair_open_project_integrity");
        let remaining =
            u32::try_from(self.integrity_violation_messages().len()).unwrap_or(u32::MAX);
        crate::core::IntegrityRepairResult {
            fixed: 0,
            remaining,
            changed: false,
        }
    }
}
