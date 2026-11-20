//! Split Line Here (#335, plan 5.4): cut a line in two without deleting a
//! shot. The first half keeps the line's id and its shots before the cut;
//! a new line takes the shots after it and starts at **the same station**,
//! which becomes a junction of the two halves.
//!
//! Declared change from today (plan 12.2): the split station is no longer
//! duplicated into two co-located unmerged stations. Its name, id, comment and
//! datums stay on the one station, so there is no comment or datum side to
//! choose; its hand move is copied to the new line (`hand(L2, k) = hand(L1,
//! k)`), so the second half does not shift. Consequences: an edit to one half
//! now moves the other, Move Segment on the second half is refused when the
//! first holds a datum, and merging the halves' far ends routes to the closure
//! dialog.
//!
//! The station graph does not change, so nothing moves and no closure becomes
//! an orphan: the sweep the command ends in finds nothing in a valid model.
//! A cut inside a shot first inserts a station there (Insert Station's math);
//! within `MIN_SPLIT_CHILD_LENGTH_M` of either end it snaps to that end's
//! station instead.

use std::collections::BTreeSet;

use crate::project::MIN_SPLIT_CHILD_LENGTH_M;
use crate::{LineId, SplitAt, StationId};

use super::super::{EditContext, Line, LoopClosure, Project};
use super::{EditError, Edited, Epilogue, Promotion};

/// The choices of the Split Line Here dialog: today's `SplitLineOptions`
/// without the comment and datum sides (the halves share the split station).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitOptions {
    pub name_a: String,
    pub name_b: String,
    /// `#rrggbb`.
    pub color_a: String,
    /// `#rrggbb`.
    pub color_b: String,
}

/// What Split Line Here would do, for the dialog (replaces today's
/// `SplitPreview`, whose closure promotions and moved half cannot happen any
/// more: the halves stay connected at their shared station).
#[derive(Debug, Clone, PartialEq)]
pub struct SplitLinePreview {
    /// The station the halves will share: `None` when the cut inserts a new
    /// station inside a shot.
    pub station: Option<StationId>,
    /// The cut's chain index on the line (after the inserted station).
    pub station_index: usize,
    /// The id the second half will get.
    pub new_line: LineId,
    /// How many shots each half keeps.
    pub shots_a: usize,
    pub shots_b: usize,
    /// The split station carries a hand move on the line, copied to the
    /// second half.
    pub hand_move_copied: bool,
    /// What the orphan sweep the split ends in would do (nothing in a valid
    /// model).
    pub promoted_closures: Vec<Promotion>,
    pub swept_closures: Vec<LoopClosure>,
}

/// Where a validated split cuts.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Cut {
    Station(usize),
    InShot { index: usize, distance: f64 },
}

impl Project {
    /// Every refusal of Split Line Here, in today's order: names missing,
    /// the line gone, no line id left, the cut outside the shot or at an
    /// end station, no room for the inserted station, the halves' names
    /// (empty, another line's, or equal).
    fn split_cut(
        &self,
        line: LineId,
        at: SplitAt,
        options: &SplitOptions,
    ) -> Result<(Cut, String, String), EditError> {
        if options.name_a.trim().is_empty() || options.name_b.trim().is_empty() {
            return Err(EditError::refused("Each half of a split needs a name"));
        }
        let l = self.line_or_refuse(line)?;
        if let Some(refusal) = self.split_line_budget_refusal(line) {
            return Err(EditError::Refused(refusal));
        }
        let cut = match at {
            SplitAt::Station { station_index } => Cut::Station(station_index as usize),
            SplitAt::Shot { shot_id, distance } => {
                let index = self.shot_index(line, shot_id)?;
                let length = l.shots[index].length;
                if !distance.is_finite() || distance < 0.0 || distance > length {
                    return Err(EditError::refused(format!(
                        "Distance {distance} is outside the shot (0 to {length} m)"
                    )));
                }
                if distance < MIN_SPLIT_CHILD_LENGTH_M {
                    Cut::Station(index)
                } else if length - distance < MIN_SPLIT_CHILD_LENGTH_M {
                    Cut::Station(index + 1)
                } else {
                    if let Some(refusal) = self.shot_insert_refusal(line) {
                        return Err(EditError::Refused(refusal));
                    }
                    Cut::InShot { index, distance }
                }
            }
        };
        if let Cut::Station(k) = cut {
            if k == 0 || k >= l.shots.len() {
                let name = l
                    .station(k)
                    .and_then(|s| self.stations.get(&s))
                    .map_or_else(|| k.to_string(), |s| s.name.clone());
                return Err(EditError::refused(format!(
                    "Station {name} is the first or last station of {}; split at a station in \
                     the middle",
                    self.line_label_of(line)
                )));
            }
        }
        let name_a = self.validate_line_name(&options.name_a, &[line])?;
        let name_b = self.validate_line_name(&options.name_b, &[line])?;
        if name_a == name_b {
            return Err(EditError::refused(format!(
                "A line named \"{name_b}\" already exists"
            )));
        }
        Ok((cut, name_a, name_b))
    }

    /// Split `line` at `at` (module docs). Returns the second half's id.
    pub fn split_line(
        &mut self,
        line: LineId,
        at: SplitAt,
        options: &SplitOptions,
    ) -> Result<Edited<LineId>, EditError> {
        let (cut, name_a, name_b) = self.split_cut(line, at, options)?;
        self.apply_edit(|p| {
            let second = p.split_line_in_place(line, cut, (name_a, name_b), options)?;
            Ok((second, split_epilogue(line)))
        })
    }

    /// What [`Self::split_line`] would do (see [`SplitLinePreview`]): refused
    /// exactly as the split is, else the split run on a copy.
    pub fn preview_split_line(
        &self,
        line: LineId,
        at: SplitAt,
        options: &SplitOptions,
    ) -> Result<SplitLinePreview, EditError> {
        let (cut, name_a, name_b) = self.split_cut(line, at, options)?;
        let l = self.line_or_refuse(line)?;
        let (station, station_index) = match cut {
            Cut::Station(k) => (l.station(k), k),
            Cut::InShot { index, .. } => (None, index + 1),
        };
        let hand_move_copied = station.is_some_and(|s| l.hand_moves.contains_key(&s));
        let total = l.shots.len() + usize::from(matches!(cut, Cut::InShot { .. }));
        let mut copy = self.clone();
        let edited = copy.run_edit(|p| {
            let second = p.split_line_in_place(line, cut, (name_a, name_b), options)?;
            Ok((second, split_epilogue(line)))
        })?;
        Ok(SplitLinePreview {
            station,
            station_index,
            new_line: edited.value,
            shots_a: station_index,
            shots_b: total - station_index,
            hand_move_copied,
            promoted_closures: edited.report.sweep.promoted,
            swept_closures: edited.report.sweep.removed,
        })
    }

    /// The mutation of a split (also the delete-and-split shapes reuse its
    /// rules): the line keeps `shots[..k]`, a new line takes `shots[k..]` from
    /// the shared station `k`.
    fn split_line_in_place(
        &mut self,
        line: LineId,
        cut: Cut,
        (name_a, name_b): (String, String),
        options: &SplitOptions,
    ) -> Result<LineId, EditError> {
        let k = match cut {
            Cut::Station(k) => k,
            Cut::InShot { index, distance } => {
                self.split_shot_in_place(line, index, distance, None)?;
                index + 1
            }
        };
        let new_id = self.next_line_id();
        let Some(l) = self.line_mut(line) else {
            return Err(EditError::line_gone());
        };
        let chain: Vec<StationId> = l.stations().collect();
        let shared = chain[k];
        let tail = l.shots.split_off(k);
        let mut second = Line {
            id: new_id,
            name: Some(name_b),
            color: options.color_b.clone(),
            direction: l.direction,
            quality: l.quality,
            azimuth_reference: l.azimuth_reference,
            section_id: l.section_id,
            legacy_surveyor: None,
            legacy_date: None,
            start: shared,
            shots: tail,
            hand_moves: std::collections::BTreeMap::new(),
            next_shot_id: 0,
        };
        let second_stations: BTreeSet<StationId> = chain[k..].iter().copied().collect();
        for &s in &chain[k + 1..] {
            if let Some(hand) = l.hand_moves.remove(&s) {
                second.hand_moves.insert(s, hand);
            }
        }
        if let Some(&hand) = l.hand_moves.get(&shared) {
            second.hand_moves.insert(shared, hand);
        }
        l.name = Some(name_a);
        l.color.clone_from(&options.color_a);
        second.assign_session_shot_ids();
        self.lines_mut().push(second);
        self.repoint_vias(line, new_id, &second_stations);
        Ok(new_id)
    }
}

/// A split settles with the line as the edited one and runs the sweep (it
/// finds nothing in a valid model, as the station graph does not change).
fn split_epilogue(line: LineId) -> Epilogue {
    Epilogue {
        ctx: EditContext::line(line),
        sweep: true,
        remove_across_one_shot: false,
    }
}
