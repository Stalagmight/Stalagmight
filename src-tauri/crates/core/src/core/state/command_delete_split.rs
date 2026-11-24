//! The delete recipes beyond one shot (plan 5.4, 9.1): delete a shot and
//! split the line, delete a shot range by splitting or by closing the gap,
//! and delete a line's last shot with the line. Each is one recorded edit
//! on `run_recorded` (one undo entry), on the station-graph operations
//! `survey::Project::delete_shot_range` and `delete_line`, which leave a
//! junction on its other lines, settle anchors, sweep orphan closures and
//! (the gap-close) remove the closures newly left across one shot (#737).
//!
//! What stays here: the refusals that are the command's (frozen shots, S12;
//! the edge-shot and shot-count wording of 0.6.0), the history record as
//! 0.6.0 wrote it (kind `batch`, the `delete_shots` label worded before the
//! edit, the placeholder sub-command rows worded after it), and the
//! warnings naming the closures the edit removed.

use super::command_shot::{any_shot_frozen, closure_warnings, ClosureNames};
use super::commands::Record;
use super::labels::{delete_shots_label, Label, ShotsDeleted};
use super::AppState;

/// A recipe's closure warnings and identity notice (RS 7 "Notices").
type Notes = (
    Vec<crate::core::ValidationWarning>,
    crate::models::IdentityNotice,
);
use crate::core::{Command, CommandResult};
use crate::models::survey::changeset::PreviewRow;
use crate::models::survey::{Project as Survey, RangeDeleteMode};
use crate::models::{LineId, LineUpdates, ShotId};

/// The history kind 0.6.0 recorded every composite edit under (the icon the
/// frontend maps): its undo entry was a labelled `Command::Batch`.
const RECIPE_KIND: &str = "batch";

/// A recipe's record before its edit: kind `batch`, `label` (with its
/// parts), and `line` in the refs when it exists.
pub(super) fn recipe_record(project: &Survey, label: Label, line: LineId) -> Record {
    let mut record = Record {
        kind: RECIPE_KIND.to_string(),
        ..Record::default()
    };
    record.set_label(label);
    if project.line(line).is_some() {
        record.refs.lines.push(line);
    }
    record
}

/// One preview row of a placeholder sub-command, as 0.6.0's batch preview
/// listed it: the command as the history names it on `project`, and a
/// detail.
fn row(project: &Survey, cmd: &Command, detail: String) -> PreviewRow {
    PreviewRow {
        field: super::labels::command_label(project, cmd),
        old: String::new(),
        new: detail,
    }
}

/// The placeholder `CreateLine` of a split's second half, named `name`.
fn create_line_named(name: &str) -> Command {
    Command::CreateLine {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: String::new(),
        direction: None,
        shots: Vec::new(),
        cave_section_id: None,
    }
}

/// The two halves' names of a split of `line`, trimmed, or the refusal:
/// each validated as a rename is (empty, a forbidden character, another
/// line's name), and the two must differ.
fn split_names(
    project: &Survey,
    line: LineId,
    a: &str,
    b: &str,
) -> Result<(String, String), String> {
    let a = project
        .validate_line_name(a, &[line])
        .map_err(|e| e.to_string())?;
    let b = project
        .validate_line_name(b, &[line])
        .map_err(|e| e.to_string())?;
    if a == b {
        return Err(format!("A line named \"{b}\" already exists"));
    }
    Ok((a, b))
}

/// The positions of `first` and `last` on `line` (either order, ascending),
/// or today's line-gone / shot-gone refusal.
fn range_positions(
    project: &Survey,
    line: LineId,
    first: ShotId,
    last: ShotId,
) -> Result<(usize, usize, usize), String> {
    let l = project
        .line(line)
        .ok_or_else(|| crate::models::LINE_GONE.to_string())?;
    let position = |id: ShotId| {
        l.shots
            .iter()
            .position(|s| s.id == id)
            .ok_or_else(|| crate::models::SHOT_GONE.to_string())
    };
    let (a, b) = (position(first)?, position(last)?);
    Ok((a.min(b), a.max(b), l.shots.len()))
}

/// The uuids of the shots at `first` and `last` of `line` (once when they
/// are one shot), for the record's refs.
fn shot_uuids(project: &Survey, line: LineId, first: usize, last: usize) -> Vec<String> {
    let Some(l) = project.line(line) else {
        return Vec::new();
    };
    let mut uuids: Vec<String> = [first, last]
        .iter()
        .filter_map(|&i| l.shots.get(i).map(|s| s.uuid.clone()))
        .collect();
    uuids.dedup();
    uuids
}

/// A split's mutation shared by both delete-and-split recipes: the shots
/// `start..=end` (positions) go, the line keeps the shots before them
/// renamed `name_a`, a new line `name_b` takes the shots after them, in
/// `color_b` when given (the colour a new line gets, as Split Line Here
/// gives it; demo finding I-9), else in the line's colour (0.6.0's split,
/// which copied the line's metadata). Returns the new line's id, the
/// closure warnings and the identity notice (RS 7).
fn split_out(
    project: &mut Survey,
    record: &mut Record,
    line: LineId,
    (start, end): (usize, usize),
    (name_a, name_b, color_b): (&str, &str, Option<&str>),
    context: &str,
) -> Result<(LineId, Notes), String> {
    let (first, last) = {
        let shots = &project
            .line(line)
            .ok_or_else(|| crate::models::LINE_GONE.to_string())?
            .shots;
        let id = |i: usize| {
            shots
                .get(i)
                .map(|s| s.id)
                .ok_or_else(|| crate::models::SHOT_GONE.to_string())
        };
        (id(start)?, id(end)?)
    };
    record.refs.shots = shot_uuids(project, line, start, end);
    let names = ClosureNames::of(project);
    let edited = project
        .delete_shot_range(
            line,
            first,
            last,
            &RangeDeleteMode::Split {
                name_a: name_a.to_string(),
                name_b: name_b.to_string(),
            },
        )
        .map_err(|e| e.to_string())?;
    let new_line = edited
        .value
        .new_line
        .ok_or_else(|| "Failed to split the line".to_string())?;
    if let Some(color) = color_b {
        let updates = LineUpdates {
            color: Some(color.to_string()),
            ..LineUpdates::default()
        };
        project
            .update_line_meta(new_line, &updates)
            .map_err(|e| e.to_string())?;
    }
    record.refs.lines.push(new_line);
    record.preview.rows = vec![
        row(
            project,
            &Command::DeleteShot {
                line_id: line,
                shot_id: first,
            },
            String::new(),
        ),
        row(
            project,
            &create_line_named(name_b),
            format!("Name: {name_b}"),
        ),
    ];
    Ok((
        new_line,
        (
            closure_warnings(&names, project, &edited.report, context),
            project.identity_notice(&edited.report.identities),
        ),
    ))
}

impl AppState {
    /// The two halves' names of a delete-and-split of `line_id`, trimmed, or
    /// the refusal ([`split_names`]), checked before anything changes. A
    /// missing line or project is left for the command to report.
    fn validate_split_names(
        &self,
        line_id: LineId,
        name_a: &str,
        name_b: &str,
    ) -> Result<(String, String), String> {
        let guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(project) = guard.as_ref() else {
            return Ok((name_a.trim().to_string(), name_b.trim().to_string()));
        };
        split_names(project, line_id, name_a, name_b)
    }

    /// Delete a mid-line shot and split the line in two, as one undo/redo
    /// unit: the line keeps the shots before it (renamed `name_a`), a new
    /// line `name_b` takes the shots after it, starting at the deleted shot's
    /// end station. Nothing else leaves the survey: the deleted shot's two
    /// stations stay, one on each half. Returns the new line as `line_id`.
    /// The new line keeps the line's colour; the app passes the colour a new
    /// line gets through [`Self::execute_delete_shot_and_split_with_color`].
    pub fn execute_delete_shot_and_split(
        &self,
        line_id: LineId,
        shot_id: ShotId,
        name_a: String,
        name_b: String,
    ) -> CommandResult {
        self.execute_delete_shot_and_split_with_color(line_id, shot_id, name_a, name_b, None)
    }

    /// [`Self::execute_delete_shot_and_split`] with the new line in
    /// `color_b` when given, as Split Line Here colours its second half
    /// (demo finding I-9: both halves of a Delete & Split were the line's
    /// colour).
    pub fn execute_delete_shot_and_split_with_color(
        &self,
        line_id: LineId,
        shot_id: ShotId,
        name_a: String,
        name_b: String,
        color_b: Option<String>,
    ) -> CommandResult {
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        // Read-only edit gate (#191, D-B6): gated on a read-only remote project.
        if let Err(f) = self.guard_mutation("Delete shot and split line") {
            return f;
        }
        // The halves' names replace the line's: refused like a rename when
        // empty or another line's, before anything changes.
        let (name_a, name_b) = match self.validate_split_names(line_id, &name_a, &name_b) {
            Ok(names) => names,
            Err(refusal) => return CommandResult::failure(refusal),
        };
        tracing::info!(
            line_id,
            shot_id,
            purpose = "split line after deleting shot",
            "Executing delete_shot_and_split as single undo unit"
        );
        let result = self.run_recorded(
            |project| {
                let label = super::command_shot::shot_index(project, line_id, shot_id)
                    .map(|i| delete_shots_label(project, line_id, i, i, ShotsDeleted::Split))
                    .unwrap_or_default();
                recipe_record(project, label, line_id)
            },
            |project, record| {
                let (pos, _, n) = range_positions(project, line_id, shot_id, shot_id)?;
                if pos == 0 || pos + 1 == n {
                    return Err(
                        "Cannot split at edge shot (first or last). Use simple delete instead."
                            .to_string(),
                    );
                }
                // This path deletes `shot_id`: refuse a frozen one (#241).
                if any_shot_frozen(project, line_id, [pos]) {
                    return Err(super::command_shot::FROZEN_DELETE.to_string());
                }
                split_out(
                    project,
                    record,
                    line_id,
                    (pos, pos),
                    (&name_a, &name_b, color_b.as_deref()),
                    "delete shot and split",
                )
            },
        );
        match result {
            Ok((new_line_id, (warnings, identities))) => {
                tracing::info!(
                    line_id,
                    shot_id,
                    new_line_id,
                    purpose = "split line after deleting shot",
                    "Delete shot and split line completed"
                );
                CommandResult::success()
                    .with_line_id(new_line_id)
                    .with_warnings(warnings)
                    .with_identities(identities)
            }
            Err(failure) => failure,
        }
    }

    /// Delete a contiguous *interior* range of shots and split the line in
    /// two: the line keeps the shots before the range (renamed `name_a`), a
    /// new line `name_b` takes the shots after it. The stations strictly
    /// inside the range leave the line (a junction stays for its other
    /// lines). One undo entry. Prefix, suffix and whole ranges use the shift
    /// or the line delete. Returns the new line as `line_id`, in the line's
    /// colour ([`Self::execute_delete_shot_range_split_with_color`] for
    /// another).
    pub fn execute_delete_shot_range_split(
        &self,
        line_id: LineId,
        start_shot_id: ShotId,
        end_shot_id: ShotId,
        name_a: String,
        name_b: String,
    ) -> CommandResult {
        self.execute_delete_shot_range_split_with_color(
            line_id,
            start_shot_id,
            end_shot_id,
            (name_a, name_b),
            None,
        )
    }

    /// [`Self::execute_delete_shot_range_split`] with the new line in
    /// `color_b` when given (demo finding I-9). `names` are the two halves'.
    pub fn execute_delete_shot_range_split_with_color(
        &self,
        line_id: LineId,
        start_shot_id: ShotId,
        end_shot_id: ShotId,
        (name_a, name_b): (String, String),
        color_b: Option<String>,
    ) -> CommandResult {
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        // Read-only edit gate (#191, D-B6): gated on a read-only remote project.
        if let Err(f) = self.guard_mutation("Delete shot range and split line") {
            return f;
        }
        let (name_a, name_b) = match self.validate_split_names(line_id, &name_a, &name_b) {
            Ok(names) => names,
            Err(refusal) => return CommandResult::failure(refusal),
        };
        tracing::info!(
            line_id,
            start_shot_id,
            end_shot_id,
            purpose = "split line after deleting shot range",
            "Executing delete_shot_range_split as single undo unit"
        );
        let result = self.run_recorded(
            |project| {
                let label = range_positions(project, line_id, start_shot_id, end_shot_id)
                    .map(|(a, b, _)| {
                        delete_shots_label(project, line_id, a, b, ShotsDeleted::Split)
                    })
                    .unwrap_or_default();
                recipe_record(project, label, line_id)
            },
            |project, record| {
                let (start, end, n) =
                    range_positions(project, line_id, start_shot_id, end_shot_id)?;
                // Split requires shots both before and after the range.
                if start == 0 || end + 1 >= n {
                    return Err(
                        "Range is not interior to the line (split needs shots before and \
                         after). Use shift or delete the line instead."
                            .to_string(),
                    );
                }
                if any_shot_frozen(project, line_id, start..=end) {
                    return Err(super::command_shot::FROZEN_RANGE_DELETE.to_string());
                }
                split_out(
                    project,
                    record,
                    line_id,
                    (start, end),
                    (&name_a, &name_b, color_b.as_deref()),
                    "delete shot range and split",
                )
            },
        );
        match result {
            Ok((new_line_id, (warnings, identities))) => {
                tracing::info!(
                    line_id,
                    new_line_id,
                    purpose = "split line after deleting shot range",
                    "Delete shot range and split line completed"
                );
                CommandResult::success()
                    .with_line_id(new_line_id)
                    .with_warnings(warnings)
                    .with_identities(identities)
            }
            Err(failure) => failure,
        }
    }

    /// Delete a contiguous range of shots by gap-closing (shifting): the
    /// survivors rejoin and the line stays one line (an interior range), or
    /// the line loses a prefix or a suffix. One undo entry; the anchors are
    /// settled once, after the whole range is gone, so a junction partner
    /// stays where the surveyor put it (the 0.6.0 atomic behaviour, #413
    /// finding 22). Refused when the range is shift-blocked ("Use split
    /// instead.") or is the whole line ("delete the line instead"). The
    /// closures the gap-close leaves across one shot are removed and
    /// reported (#737).
    pub fn execute_delete_shot_range_shift(
        &self,
        line_id: LineId,
        start_shot_id: ShotId,
        end_shot_id: ShotId,
    ) -> CommandResult {
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        // Read-only edit gate (#191, D-B6): gated on a read-only remote project.
        if let Err(f) = self.guard_mutation("Delete shot range and shift") {
            return f;
        }
        tracing::info!(
            line_id,
            start_shot_id,
            end_shot_id,
            purpose = "gap-close a deleted shot range",
            "Executing delete_shot_range_shift as single undo unit"
        );
        let result = self.run_recorded(
            |project| {
                let label = range_positions(project, line_id, start_shot_id, end_shot_id)
                    .map(|(a, b, _)| {
                        delete_shots_label(project, line_id, a, b, ShotsDeleted::CloseUp)
                    })
                    .unwrap_or_default();
                recipe_record(project, label, line_id)
            },
            |project, record| {
                let (start, end, n) =
                    range_positions(project, line_id, start_shot_id, end_shot_id)?;
                if start == 0 && end + 1 == n {
                    return Err(
                        "Cannot shift-delete an entire line; delete the line instead.".to_string(),
                    );
                }
                if any_shot_frozen(project, line_id, start..=end) {
                    return Err(super::command_shot::FROZEN_RANGE_DELETE.to_string());
                }
                record.refs.shots = shot_uuids(project, line_id, start, end);
                let names = ClosureNames::of(project);
                let edited = project
                    .delete_shot_range(line_id, start_shot_id, end_shot_id, &RangeDeleteMode::Shift)
                    .map_err(|e| e.to_string())?;
                record.preview.rows = [start_shot_id, end_shot_id]
                    .iter()
                    .map(|&shot_id| {
                        row(
                            project,
                            &Command::DeleteShot { line_id, shot_id },
                            String::new(),
                        )
                    })
                    .collect();
                Ok((
                    closure_warnings(
                        &names,
                        project,
                        &edited.report,
                        "delete shot range and shift",
                    ),
                    project.identity_notice(&edited.report.identities),
                ))
            },
        );
        match result {
            Ok((warnings, identities)) => {
                tracing::info!(
                    line_id,
                    purpose = "gap-close a deleted shot range",
                    "Delete shot range (shift) completed"
                );
                CommandResult::success()
                    .with_line_id(line_id)
                    .with_warnings(warnings)
                    .with_identities(identities)
            }
            Err(failure) => failure,
        }
    }

    /// Delete a line's only shot and the line with it, as one undo/redo
    /// unit (`survey::Project::delete_line`): the line's stations that no
    /// other line passes leave with their comments, datums and closures; a
    /// junction stays for its other lines; then the orphan-closure sweep.
    pub fn execute_delete_last_shot_and_line(
        &self,
        line_id: LineId,
        shot_id: ShotId,
    ) -> CommandResult {
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        // Read-only edit gate (#191, D-B6): gated on a read-only remote project.
        if let Err(f) = self.guard_mutation("Delete last shot and line") {
            return f;
        }
        tracing::info!(
            line_id,
            shot_id,
            purpose = "remove single-shot line",
            "Executing delete_last_shot_and_line as single undo unit"
        );
        let result = self.run_recorded(
            |project| {
                let label = delete_shots_label(project, line_id, 0, 0, ShotsDeleted::LastOfLine);
                recipe_record(project, label, line_id)
            },
            |project, record| {
                let line = project
                    .line(line_id)
                    .ok_or_else(|| crate::models::LINE_GONE.to_string())?;
                if line.shots.len() != 1 {
                    return Err(format!(
                        "That line has {} shots, not exactly 1.",
                        line.shots.len()
                    ));
                }
                if line.shots[0].id != shot_id {
                    return Err(crate::models::SHOT_GONE.to_string());
                }
                // The name the line had, for the preview row that names it
                // once it is gone (0.6.0 read it off the before-snapshot).
                let name = line.name.clone().filter(|n| !n.trim().is_empty());
                record.refs.shots = shot_uuids(project, line_id, 0, 0);
                let names = ClosureNames::of(project);
                let edited = project.delete_line(line_id).map_err(|e| e.to_string())?;
                let delete_line = Command::DeleteLine { line_id };
                record.preview.rows = vec![
                    row(
                        project,
                        &Command::DeleteShot { line_id, shot_id },
                        String::new(),
                    ),
                    PreviewRow {
                        field: name.map_or_else(
                            || super::labels::command_label(project, &delete_line),
                            |name| format!("Delete line \"{name}\""),
                        ),
                        old: String::new(),
                        new: String::new(),
                    },
                ];
                Ok((
                    closure_warnings(&names, project, &edited.report, "delete last shot and line"),
                    project.identity_notice(&edited.report.identities),
                ))
            },
        );
        match result {
            Ok((warnings, identities)) => {
                tracing::info!(
                    line_id,
                    shot_id,
                    purpose = "remove single-shot line",
                    "Deleted last shot and removed line"
                );
                CommandResult::success()
                    .with_warnings(warnings)
                    .with_identities(identities)
            }
            Err(failure) => failure,
        }
    }
}
