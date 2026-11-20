//! Split Line Here (#335) as one atomic undo/redo unit.

use super::command_delete_split::line_budget_refusal;
use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::{
    Coordinates, LineId, OrphanClosureSweep, Project, RemovedClosure, ShotId, SplitAt,
    SplitLineOptions, SplitPreview, SplitSide, StationRef, MIN_SPLIT_CHILD_LENGTH_M,
};

/// Below this a half "did not move" in [`AppState::split_line_preview`]:
/// float noise from the walk, far under anything a survey resolves.
const SPLIT_PREVIEW_STILL_M: f64 = 1e-6;

impl AppState {
    /// Split a line in two at a mid-line station, or at a point along a
    /// shot (inserting a station there first), deleting nothing. One undo
    /// entry (`ProjectSnapshotPair`) covers the optional station insert and
    /// the split. Returns the new second half's id as `line_id`, and both
    /// halves as `line_ids` (first half first).
    pub fn execute_split_line(
        &self,
        line_id: LineId,
        at: SplitAt,
        options: SplitLineOptions,
    ) -> CommandResult {
        let _ = (&line_id, &at, &options);
        crate::core::stub::not_ported("command_split_line::execute_split_line")
    }

    /// Make the validated `cut` in `project`: split the shot first when the
    /// cut lands inside one, split the line, and turn every loop closure
    /// the cut left as a sole tie into a merge (issue #8, as the
    /// delete-and-split path does: a closure must close a loop), removing any
    /// closure those merges leave between merged stations (#738). Returns the
    /// second half's id, what that sweep did, and the closures it removed,
    /// named by their stations before the cut and told apart by whether the cut
    /// made them redundant or they already were (the whole-project sweep also
    /// takes a closure between merged stations that an undo restored from a
    /// history recorded before #738). The split and its preview both run this,
    /// so the dialog names exactly what the split changes.
    fn apply_cut(
        project: &mut Project,
        line_id: LineId,
        cut: Cut,
        options: &SplitLineOptions,
    ) -> Result<CutOutcome, String> {
        // Before anything changes. The cut adds and drops no closure, so the
        // sweep's positions match these entries one to one.
        let before = project.closures_before_sweep();
        let station_index = match cut {
            Cut::Station(station_index) => station_index,
            Cut::InShot {
                shot_id,
                distance,
                station_index,
            } => {
                project.split_shot_at_distance(line_id, shot_id, distance)?;
                station_index
            }
        };
        let second = project.split_line_at_station(line_id, station_index, options)?;
        let sweep = project.sweep_orphan_closures();
        let removed = project.removed_closures_named(&before, &sweep);
        Ok(CutOutcome {
            second,
            sweep,
            removed,
        })
    }

    /// What Split Line Here would do beyond the cut, without doing it: the
    /// split runs on a copy of the project. A loop closure whose ends only the
    /// cut line joined (a closure from before the cut to past it, with
    /// nothing else tying the halves) becomes the halves' only tie, and the
    /// split turns it into a merge, moving one half by the loop's misclosure.
    /// Refusing that split would only make the user delete the closure, split
    /// and merge by hand, with the same move, and keeping the closure is not an
    /// option: every load and every later edit converts a sole-tie closure
    /// (docs/closures.md). So the dialog shows the move before the user
    /// confirms, and names any closure the split removes because that merge
    /// makes its two stations one point (#738). A refused split returns its
    /// reason.
    pub fn split_line_preview(
        &self,
        line_id: LineId,
        at: SplitAt,
        options: &SplitLineOptions,
    ) -> Result<SplitPreview, String> {
        let _ = (&line_id, &at, &options);
        crate::core::stub::not_ported("command_split_line::split_line_preview")
    }

    /// Every refusal of Split Line Here, read-only and in the order the
    /// command decides them: the read-only edit gate (#191, D-B6), the
    /// options (a half with no name, a zero datum asked to stay on both
    /// halves: [`SplitLineOptions::refusal`]), the cut itself
    /// ([`Self::validate_cut`]), then the halves' names (empty, or another
    /// line's: `Project::validate_replacement_line_names`). The command and
    /// [`Self::split_line_preview`] both run it, so a preview refuses
    /// exactly what the split would, with the same reason. The edit gate reads
    /// only the SpeleoDB session, before the project lock is taken.
    fn split_refusal(
        &self,
        line_id: LineId,
        at: SplitAt,
        options: &SplitLineOptions,
    ) -> Result<Cut, String> {
        let _ = (&line_id, &at, &options);
        crate::core::stub::not_ported("command_split_line::split_refusal")
    }

    /// Resolve `at` to where the split cuts, or why it is refused. Read-only:
    /// every refusal of the split is decided here, before anything changes.
    fn validate_cut(project: &Project, line_id: LineId, at: SplitAt) -> Result<Cut, String> {
        let Some(line) = project.get_line(line_id) else {
            return Err(crate::models::LINE_GONE.to_string());
        };
        // The split mints the second half's id (#555); refuse rather than
        // truncate the first half with nowhere to put the rest.
        if let Some(refusal) = line_budget_refusal(project, line_id) {
            return Err(refusal.error.unwrap_or_default());
        }
        let cut = match at {
            SplitAt::Station { station_index } => Cut::Station(station_index),
            SplitAt::Shot { shot_id, distance } => {
                let Some(pos) = line.shots.iter().position(|s| s.id == shot_id) else {
                    return Err(crate::models::SHOT_GONE.to_string());
                };
                let length = line.shots[pos].length;
                if !distance.is_finite() || distance < 0.0 || distance > length {
                    return Err(format!(
                        "Distance {distance} is outside the shot (0 to {length} m)"
                    ));
                }
                if distance < MIN_SPLIT_CHILD_LENGTH_M {
                    Cut::Station(pos as u32)
                } else if length - distance < MIN_SPLIT_CHILD_LENGTH_M {
                    Cut::Station(pos as u32 + 1)
                } else {
                    // Same guard as Insert Station: a new station inside a
                    // shot of an adjusted network changes that network.
                    if project.is_shot_frozen_by_id(line_id, shot_id) {
                        return Err("Cannot split a frozen shot. This shot is part of an \
                                    adjusted survey network."
                            .to_string());
                    }
                    // The station goes in as a new shot of this line,
                    // so it needs room under the shot cap and an id left
                    // in the line's window; checked here, before the
                    // shot split changes anything.
                    if let Some(refusal) = project.shot_insert_refusal(line_id) {
                        return Err(refusal);
                    }
                    // The new station is always a mid-line one, so the
                    // split cannot refuse it for being an end.
                    return Ok(Cut::InShot {
                        shot_id,
                        distance,
                        station_index: pos as u32 + 1,
                    });
                }
            }
        };
        if let Cut::Station(station_index) = cut {
            if let Some(refusal) = project.split_station_refusal(line_id, station_index) {
                return Err(refusal);
            }
        }
        Ok(cut)
    }
}

/// What [`AppState::apply_cut`] did.
struct CutOutcome {
    /// The second half's id.
    second: LineId,
    /// The sweep that followed the cut.
    sweep: OrphanClosureSweep,
    /// The closures it removed, named for the user.
    removed: Vec<RemovedClosure>,
}

/// Where a validated split cuts: at an existing station, or inside a shot,
/// which first gets a station inserted `distance` along it.
#[derive(Debug, Clone, Copy)]
enum Cut {
    Station(u32),
    InShot {
        shot_id: ShotId,
        distance: f64,
        station_index: u32,
    },
}
