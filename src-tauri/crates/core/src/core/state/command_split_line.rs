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
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        // Every refusal is decided first, read-only: the ignore pin below
        // writes to the database, so a refused split must not reach it.
        let cut = match self.split_refusal(line_id, at, &options) {
            Ok(cut) => cut,
            Err(reason) => return CommandResult::failure(reason),
        };
        tracing::info!(line_id, ?at, "Executing split_line as single undo unit");

        // The shots past the cut move to a new line: position-keyed
        // ignores on them would lapse (see the pin's doc).
        self.pin_position_keyed_ignores_before_move(&[line_id]);

        let before_snapshot = match self.capture_project_snapshot() {
            Ok(s) => s,
            Err(result) => return result,
        };
        // This path bypasses `execute()`, so it carries its own frame check
        // (#469): moving a datum to the second half can re-define the frame.
        let datums_before = self.datum_layout();
        let undo_name_b = options.name_b.trim().to_string();

        let second_id = {
            let Ok(mut proj_guard) = self.project.lock() else {
                return CommandResult::failure("Failed to lock project".to_string());
            };
            let Some(project) = proj_guard.as_mut() else {
                return CommandResult::failure("No project open".to_string());
            };
            // `validate_cut` ran every check the model refuses on, but the
            // shot split shortens the tail before it inserts the head, so any
            // failure rolls back to the snapshot.
            match Self::apply_cut(project, line_id, cut, &options) {
                Ok(outcome) => {
                    // Logged here, not in the sweep: the preview runs the same
                    // cut on a copy, and its removals never happen.
                    outcome.sweep.log_removed("split line");
                    project.touch();
                    outcome.second
                }
                Err(e) => {
                    let _ = Self::restore_snapshot_into(&mut proj_guard, &before_snapshot);
                    return CommandResult::failure(e);
                }
            }
        };

        self.settle_datum_layout(&datums_before);
        self.reconcile_geometry();

        let after_snapshot = match self.capture_after_snapshot_or_rollback(&before_snapshot) {
            Ok(s) => s,
            Err(result) => return result,
        };
        // The label only names the operation in history. It must not contain a
        // ReverseLine: undo/redo settle reversed lines off this command.
        let label = Command::Batch {
            commands: vec![Command::CreateLine {
                name: Some(undo_name_b),
                surveyor: None,
                date: None,
                color: String::new(),
                direction: None,
                shots: Vec::new(),
            }],
        };
        self.push_snapshot_undo(label, before_snapshot, after_snapshot);
        // Dirty-bit flip (#191, D-B16): set-only; a no-op for a local project.
        self.mark_remote_forward_edit();

        tracing::info!(line_id, second_id, "Split line completed");
        CommandResult::success()
            .with_line_id(second_id)
            .with_line_ids(vec![line_id, second_id])
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
        let cut = self.split_refusal(line_id, at, options)?;
        self.with_project(|project| {
            let mut copy = project.clone();
            let last = copy
                .get_line(line_id)
                .map_or(0, |line| line.shots.len() as u32);
            let start = StationRef::new(line_id, 0);
            let (start_before, end_before) = (
                copy.compute_station_raw_coords(start),
                copy.compute_station_raw_coords(StationRef::new(line_id, last)),
            );
            let CutOutcome {
                second,
                sweep,
                removed,
            } = Self::apply_cut(&mut copy, line_id, cut, options)?;
            let second_last = copy
                .get_line(second)
                .map_or(0, |line| line.shots.len() as u32);
            let moved = |before: Option<Coordinates>, after: Option<Coordinates>| {
                before.zip(after).map_or(0.0, |(b, a)| {
                    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2) + (a.z - b.z).powi(2)).sqrt()
                })
            };
            let moved_a = moved(start_before, copy.compute_station_raw_coords(start));
            let moved_b = moved(
                end_before,
                copy.compute_station_raw_coords(StationRef::new(second, second_last)),
            );
            let (moved_side, moved_m) = if moved_a.max(moved_b) <= SPLIT_PREVIEW_STILL_M {
                (None, 0.0)
            } else if moved_b >= moved_a {
                (Some(SplitSide::B), moved_b)
            } else {
                (Some(SplitSide::A), moved_a)
            };
            Ok(SplitPreview {
                closures_to_merges: u32::try_from(sweep.converted).unwrap_or(u32::MAX),
                moved_side,
                moved_m,
                removed_closures: removed
                    .iter()
                    .filter(|r| !r.already_one_point)
                    .map(|r| r.pair)
                    .collect(),
                removed_already_one_point: removed
                    .iter()
                    .filter(|r| r.already_one_point)
                    .map(|r| r.pair)
                    .collect(),
            })
        })
    }

    /// Every refusal of Split Line Here, read-only and in the order the
    /// command decides them: the read-only edit gate (#191, D-B6), the
    /// options (a half with no name, a zero datum asked to stay on both
    /// halves: [`SplitLineOptions::refusal`]), then the cut itself
    /// ([`Self::validate_cut`]). The command and
    /// [`Self::split_line_preview`] both run it, so a preview refuses
    /// exactly what the split would, with the same reason. The edit gate reads
    /// only the SpeleoDB session, before the project lock is taken.
    fn split_refusal(
        &self,
        line_id: LineId,
        at: SplitAt,
        options: &SplitLineOptions,
    ) -> Result<Cut, String> {
        if let Err(refused) = self.guard_mutation("Split line") {
            return Err(refused.error.unwrap_or_default());
        }
        if let Some(refusal) = options.refusal() {
            return Err(refusal);
        }
        self.with_project(|project| Self::validate_cut(project, line_id, at))
    }

    /// Resolve `at` to where the split cuts, or why it is refused. Read-only:
    /// every refusal of the split is decided here, before anything changes.
    fn validate_cut(project: &Project, line_id: LineId, at: SplitAt) -> Result<Cut, String> {
        let Some(line) = project.get_line(line_id) else {
            return Err(format!("Line {line_id} not found"));
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
                    return Err(format!("Shot {shot_id} not found in line {line_id}"));
                };
                let length = line.shots[pos].length;
                if !distance.is_finite() || distance < 0.0 || distance > length {
                    return Err(format!(
                        "Distance {distance} is outside shot {shot_id} (0 to {length} m)"
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
