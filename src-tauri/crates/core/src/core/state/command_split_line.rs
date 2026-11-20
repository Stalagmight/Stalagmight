//! Split Line Here (#335) as one atomic undo/redo unit, on the station
//! graph (plan 5.4): the first half keeps the line's id and the shots before
//! the cut, a new line takes the rest and starts on THE SAME station, so the
//! halves stay joined at a junction. A cut inside a shot inserts that station
//! first, in the same undo step.
//!
//! Declared change (plan 12.2, S9): the split station is no longer duplicated
//! into two co-located unmerged stations, so it has one name, one comment and
//! one set of datums, and the dialog's comment and datum sides choose nothing
//! (`SplitLineOptions::comment_side` / `datum_side` are ignored; a datum "on
//! both halves" is no longer refused, it IS on both). Nothing moves and no
//! loop closure becomes a merge, so the preview always reports none.
//!
//! The model ([`crate::models::survey::Project::split_line`]) decides every
//! refusal in 0.6.0's order and wording except one it leaves to the command
//! layer (S12): a cut inside a frozen shot, checked here at the point 0.6.0
//! checked it.

use super::commands::Record;
use super::AppState;
use crate::core::CommandResult;
use crate::models::survey::changeset::{HistoryRefs, PreviewRow, RecordedPreview};
use crate::models::survey::{Project, SplitOptions};
use crate::models::{
    LineId, SplitAt, SplitLineOptions, SplitPreview, MAX_LINE_ID, MIN_SPLIT_CHILD_LENGTH_M,
};

/// 0.6.0's refusal of a cut inside a frozen shot (Insert Station's guard: a
/// new station inside a shot of an adjusted network changes that network).
const FROZEN_SPLIT: &str =
    "Cannot split a frozen shot. This shot is part of an adjusted survey network.";

/// The model's options from the dialog's (S9: the comment and datum sides
/// have nothing left to choose).
fn split_options(options: &SplitLineOptions) -> SplitOptions {
    SplitOptions {
        name_a: options.name_a.clone(),
        name_b: options.name_b.clone(),
        color_a: options.color_a.clone(),
        color_b: options.color_b.clone(),
    }
}

/// The frozen-shot refusal of a cut inside a shot, where 0.6.0 decided it:
/// after the halves' names are present, the line exists with a line id left
/// for the second half, the shot is on it and the distance is on the shot
/// and not snapped to one of its ends; before the line's room for the
/// inserted station, the end-station rule and the names' uniqueness. Every
/// earlier refusal is the model's, so this answers `None` for those and the
/// model words them.
fn frozen_cut_refusal(
    project: &Project,
    line_id: LineId,
    at: SplitAt,
    options: &SplitOptions,
) -> Option<String> {
    let SplitAt::Shot { shot_id, distance } = at else {
        return None;
    };
    if options.name_a.trim().is_empty()
        || options.name_b.trim().is_empty()
        || project.next_line_id() > MAX_LINE_ID
    {
        return None;
    }
    let line = project.line(line_id)?;
    let index = line.shots.iter().position(|s| s.id == shot_id)?;
    let length = line.shots.get(index)?.length;
    let inside = distance.is_finite()
        && distance >= MIN_SPLIT_CHILD_LENGTH_M
        && length - distance >= MIN_SPLIT_CHILD_LENGTH_M;
    (inside && project.is_shot_frozen(line_id, index)).then(|| FROZEN_SPLIT.to_string())
}

/// What the history calls the split, read once it is made: `Split line
/// "Main" at SP`, the line by the name it had before the cut (`line_name`;
/// the halves are renamed) and the station it was cut at by its name now (a
/// cut inside a shot names its new station only from here on), else its
/// position on the first half.
fn split_label(project: &Project, line_name: &str, first: LineId, second: LineId) -> String {
    let at = project
        .line(second)
        .and_then(|l| project.stations.get(&l.start))
        .map(|s| s.name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| project.line(first).map_or(0, |l| l.shots.len()).to_string());
    super::labels::split_line_label(line_name, &at)
}

impl AppState {
    /// Split a line in two at a mid-line station, or at a point along a
    /// shot (inserting a station there first), deleting nothing. One undo
    /// entry covers the optional station insert and the split. Returns the
    /// new second half's id as `line_id`, and both halves as `line_ids`
    /// (first half first).
    pub fn execute_split_line(
        &self,
        line_id: LineId,
        at: SplitAt,
        options: SplitLineOptions,
    ) -> CommandResult {
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        if let Err(refused) = self.guard_mutation("Split line") {
            return refused;
        }
        tracing::info!(line_id, ?at, "Executing split_line as single undo unit");
        let options = split_options(&options);
        let result = self.run_recorded(
            |_| Record {
                kind: "batch".to_string(),
                ..Record::default()
            },
            |project, record| {
                if let Some(refusal) = frozen_cut_refusal(project, line_id, at, &options) {
                    return Err(refusal);
                }
                let line_name = project.line_label_of(line_id);
                let edited = project
                    .split_line(line_id, at, &options)
                    .map_err(|e| e.to_string())?;
                let second = edited.value;
                // Nothing to sweep in a valid model (the station graph does
                // not change); logged here, not in the preview, if it ever
                // does something.
                let sweep = &edited.report.sweep;
                if !sweep.is_empty() {
                    tracing::warn!(
                        removed = sweep.removed.len(),
                        promoted = sweep.promoted.len(),
                        purpose = "split line",
                        "The orphan-closure sweep after a split changed closures"
                    );
                }
                record.label = split_label(project, &line_name, line_id, second);
                let mut lines = vec![line_id, second];
                lines.sort_unstable();
                record.refs = HistoryRefs {
                    lines,
                    stations: project.line(second).map(|l| l.start).into_iter().collect(),
                    ..HistoryRefs::default()
                };
                let name_b = project
                    .line(second)
                    .and_then(|l| l.name.clone())
                    .unwrap_or_else(|| options.name_b.trim().to_string());
                record.preview = RecordedPreview {
                    rows: vec![PreviewRow {
                        field: format!("Create line \"{name_b}\""),
                        old: String::new(),
                        new: format!("Name: {name_b}"),
                    }],
                    more: 0,
                };
                Ok(second)
            },
        );
        match result {
            Ok(second) => {
                tracing::info!(line_id, second_id = second, "Split line completed");
                CommandResult::success()
                    .with_line_id(second)
                    .with_line_ids(vec![line_id, second])
            }
            Err(failure) => failure,
        }
    }

    /// What Split Line Here would do beyond the cut, for the dialog, without
    /// doing it (the split runs on a copy). On the station graph the halves
    /// share the split station, so the answer is always "nothing else": no
    /// closure becomes a merge, no half moves, no closure is removed
    /// (`split_preview_wire`). A refused split returns its reason, exactly as
    /// [`Self::execute_split_line`] words it.
    pub fn split_line_preview(
        &self,
        line_id: LineId,
        at: SplitAt,
        options: &SplitLineOptions,
    ) -> Result<SplitPreview, String> {
        self.split_refusal()?;
        let options = split_options(options);
        self.with_project(|project| {
            if let Some(refusal) = frozen_cut_refusal(project, line_id, at, &options) {
                return Err(refusal);
            }
            let preview = project
                .preview_split_line(line_id, at, &options)
                .map_err(|e| e.to_string())?;
            Ok(project.split_preview_wire(&preview))
        })
    }

    /// The refusals of Split Line Here that come before the project is read,
    /// in the order the command decides them: the SpeleoDB edit gate (#191,
    /// D-B6), then a file made with a newer Stalagmight (open read-only, plan
    /// 6.4). The preview runs it so it refuses exactly what the split does,
    /// with the same reason; the split itself meets the same two gates in
    /// `guard_mutation` and `run_recorded`.
    fn split_refusal(&self) -> Result<(), String> {
        if let Err(refused) = self.guard_mutation("Split line") {
            return Err(refused.error.unwrap_or_default());
        }
        self.guard_file_writable()
            .map_err(|refused| refused.error.unwrap_or_default())
    }
}
