//! Delete a contiguous shot range (plan 5.4), in today's four forms:
//!
//! - **gap-close** (interior range `start..=end`, "shift"): the shots and the
//!   stations `start+1 ..= end+1` leave the line, and the shot after the
//!   range re-attaches to station `start`;
//! - **split** (interior range): the shots and the stations strictly inside
//!   (`start+1 ..= end`) leave; the line keeps its shots before the range (its
//!   id), and a new line takes the shots after it, starting at station
//!   `end+1`;
//! - **prefix** (`start == 0`, shift): stations `0 ..= end` leave; station
//!   `end+1` is the new start;
//! - **suffix** (`end` is the last shot, shift): stations `start+1 ..= n`
//!   leave.
//!
//! The whole line is refused (delete the line instead). Each removed station
//! follows the delete rule: a junction stays for its other lines (the closures
//! attached there via the line follow today's member rules,
//! `edit/junction_leave.rs`); any other is deleted with its comment, datums
//! and closures. Then the orphan-closure sweep runs; the shift forms also
//! remove closures they newly leave across one shot (#737).
//!
//! Today's refusals of the shift: an interior range whose two sides are still
//! connected without it (over shots and junctions: a loop), whose two sides
//! are both held by a placement datum (reached over shots, junctions and
//! closures, never through a station the gap-close deletes), or that would
//! delete a zero datum ("Use split instead.").

use std::collections::BTreeSet;

use crate::{LineId, RangeDeleteKind, ShotId, StationId, StationRef};

use super::super::{EditContext, Line, LoopClosure, Project, ZeroDatum};
use super::{EditError, Edited, Epilogue, Promotion};

/// How a range delete closes the range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RangeDeleteMode {
    /// Gap-close (interior), or drop a prefix or a suffix.
    Shift,
    /// Interior only: split the line in two around the range; the halves get
    /// these names (validated as a rename is).
    Split { name_a: String, name_b: String },
}

/// What a range delete did.
#[derive(Debug, Clone, PartialEq)]
pub struct DeletedRange {
    pub line: LineId,
    pub kind: RangeDeleteKind,
    pub start: usize,
    pub end: usize,
    /// The new second line of a split.
    pub new_line: Option<LineId>,
    /// The stations that left the line, and those of them that left the
    /// project.
    pub stations_off_line: Vec<StationId>,
    pub stations_deleted: Vec<StationId>,
    pub datums_removed: Vec<ZeroDatum>,
    pub closures_removed: Vec<LoopClosure>,
    pub comments_removed: Vec<(StationId, String)>,
}

/// What deleting a shot range would do, for the bulk-delete dialog
/// (today's `DeleteShotRangeImpact`, without the replacement-merge rows that
/// cannot happen with shared stations). The action facts come from running
/// the offered action on a copy: the gap-close (shift) when it is offered,
/// else the split.
#[derive(Debug, Clone, PartialEq)]
pub struct DeleteRangePreview {
    pub line: LineId,
    pub start: usize,
    pub end: usize,
    pub kind: RangeDeleteKind,
    /// The slots the offered action takes off the line, before it, with the
    /// station at each (the gap-close superset for an interior range unless
    /// only the split is offered).
    pub removed_slots: Vec<(StationRef, StationId)>,
    /// Of those, the junctions: they stay for their other lines.
    pub junctions_kept: Vec<StationId>,
    /// Zero datums the SPLIT would delete (on a strictly interior station on
    /// no other line); the shift refuses instead.
    pub removed_datums: Vec<ZeroDatum>,
    /// Comments on the stations the gap-close deletes (the dialog frames them
    /// per action; a split keeps station `end+1`).
    pub removed_comments: Vec<(StationId, String)>,
    /// Closures the offered action deletes with its stations, as they were.
    pub removed_closures: Vec<LoopClosure>,
    /// Closures the sweep then promotes or removes.
    pub promoted_closures: Vec<Promotion>,
    pub swept_closures: Vec<LoopClosure>,
    /// Today's shift verdict.
    pub shift_blocked: bool,
    pub shift_blocked_reasons: Vec<String>,
    /// Closures the gap-close would remove because it leaves them across one
    /// shot (#737); `None` when no gap-close is offered.
    pub closures_removed_across_one_shot: Option<Vec<LoopClosure>>,
}

/// A normalized range.
#[derive(Debug, Clone, Copy)]
struct Range {
    start: usize,
    end: usize,
    n: usize,
    kind: RangeDeleteKind,
}

impl Range {
    /// The chain indices of the stations the shift (gap-close, prefix,
    /// suffix) takes off the line.
    fn shift_indices(self) -> std::ops::RangeInclusive<usize> {
        match self.kind {
            RangeDeleteKind::Prefix => 0..=self.end,
            RangeDeleteKind::WholeLine => 0..=self.n,
            RangeDeleteKind::Suffix | RangeDeleteKind::Interior => self.start + 1..=self.end + 1,
        }
    }

    /// The chain indices the split takes off (strictly interior).
    fn split_indices(self) -> std::ops::RangeInclusive<usize> {
        self.start + 1..=self.end
    }
}

/// What Delete Shot + Split of one interior shot would weld (review finding
/// UX-5): the closures the orphan sweep promotes once the split leaves them
/// the only connection between the halves' pieces, and the largest raw move
/// of a station the split keeps. 0.6.0's Delete Shot + Split promoted them
/// the same way (`sweep_orphan_closures`, issue #8; the parity test is
/// `persistence::migrate::migrate_tests::delete_shot_and_split_welds_a_stranded_loop_closure_as_0_6_0_did`),
/// so the dialog says so instead of the edit changing.
#[derive(Debug, Clone, PartialEq)]
pub struct SplitWeld {
    /// The closures promoted to a junction, as they were.
    pub promoted: Vec<Promotion>,
    /// The largest raw (measured) horizontal move of any station, metres.
    pub max_move_m: f64,
}

impl Project {
    /// What Delete Shot + Split of shot `index` of `line` would weld
    /// ([`SplitWeld`]), from the split run on a copy; `None` for an edge shot
    /// or a split the edit refuses.
    pub(super) fn split_delete_weld(&self, line: LineId, index: usize) -> Option<SplitWeld> {
        let shots = &self.line(line)?.shots;
        if index == 0 || index + 1 >= shots.len() {
            return None;
        }
        let shot = shots[index].id;
        let range = self.range_of(line, shot, shot).ok()?;
        let before = self.raw_positions();
        let mut copy = self.clone();
        let edited = copy
            .run_edit(self, |p| p.split_out_range(line, range, None))
            .ok()?;
        let after = copy.raw_positions();
        let max_move_m = before
            .iter()
            .filter_map(|(s, [x, y, _])| {
                let [ax, ay] = after.xy(s)?;
                Some((ax - x).hypot(ay - y))
            })
            .fold(0.0, f64::max);
        Some(SplitWeld {
            promoted: edited.report.sweep.promoted,
            max_move_m,
        })
    }

    /// Normalize a range given by two shot ids (either order).
    fn range_of(&self, line: LineId, first: ShotId, last: ShotId) -> Result<Range, EditError> {
        let n = self.line_or_refuse(line)?.shots.len();
        let (a, b) = (self.shot_index(line, first)?, self.shot_index(line, last)?);
        let (start, end) = (a.min(b), a.max(b));
        let kind = match (start == 0, end + 1 == n) {
            (true, true) => RangeDeleteKind::WholeLine,
            (true, false) => RangeDeleteKind::Prefix,
            (false, true) => RangeDeleteKind::Suffix,
            (false, false) => RangeDeleteKind::Interior,
        };
        Ok(Range {
            start,
            end,
            n,
            kind,
        })
    }

    /// The stations at chain `indices` of `line` that no other line passes.
    fn stations_only_on(&self, line: LineId, indices: &[usize]) -> Vec<StationId> {
        let topology = self.topology();
        let chain = topology.chain(line).unwrap_or(&[]);
        indices
            .iter()
            .filter_map(|&i| chain.get(i).copied())
            .filter(|&s| topology.lines_of(s).iter().all(|l| *l == line))
            .collect()
    }

    /// Today's shift verdict for `range` (module docs).
    fn shift_blocked_reasons(&self, line: LineId, range: Range) -> Vec<String> {
        let mut reasons = Vec::new();
        let shift: Vec<usize> = range.shift_indices().collect();
        let deleted = self.stations_only_on(line, &shift);
        if range.kind == RangeDeleteKind::Interior {
            let chain = self.topology().chain(line).unwrap_or(&[]).to_vec();
            if let (Some(&head), Some(&tail)) = (chain.get(range.start), chain.get(range.end + 2)) {
                // Every shot of the line that touches a removed station.
                let (from, to) = (range.start, range.end + 1);
                let excluded = |l: LineId, i: usize| l == line && (from..=to).contains(&i);
                let without = self.reach_without_shots(&[head], &excluded, false);
                if without[0].contains(&tail) {
                    reasons.push(
                        "This range is part of a loop (the two sides are already connected)."
                            .to_string(),
                    );
                }
                let held: Vec<StationId> = self
                    .placement_datum_stations()
                    .into_iter()
                    .filter(|s| !deleted.contains(s))
                    .collect();
                // Today's walk never enters a station the gap-close removes,
                // so a closure ending on one is no path to a datum (a junction
                // stays for its other lines and still carries one).
                let sides = self.reach_avoiding(&[head, tail], &excluded, true, &deleted);
                let reaches = |side: &BTreeSet<StationId>| held.iter().any(|s| side.contains(s));
                if reaches(&sides[0]) && reaches(&sides[1]) {
                    reasons.push("Both sides are anchored to a zero datum.".to_string());
                }
            }
        }
        if self.datums.iter().any(|d| deleted.contains(&d.station)) {
            reasons.push("Deleting this range would remove a zero datum.".to_string());
        }
        reasons
    }

    /// Delete shots `first ..= last` (either order) of `line` by `mode`
    /// (module docs).
    pub fn delete_shot_range(
        &mut self,
        line: LineId,
        first: ShotId,
        last: ShotId,
        mode: &RangeDeleteMode,
    ) -> Result<Edited<DeletedRange>, EditError> {
        let range = self.range_of(line, first, last)?;
        match mode {
            RangeDeleteMode::Shift => {
                if range.kind == RangeDeleteKind::WholeLine {
                    return Err(EditError::refused(
                        "Cannot shift-delete an entire line; delete the line instead.",
                    ));
                }
                let reasons = self.shift_blocked_reasons(line, range);
                if !reasons.is_empty() {
                    return Err(EditError::refused(format!(
                        "{} Use split instead.",
                        reasons.join(" ")
                    )));
                }
                self.apply_edit(|p| p.gap_close(line, range))
            }
            RangeDeleteMode::Split { name_a, name_b } => {
                if range.kind != RangeDeleteKind::Interior {
                    return Err(EditError::refused(
                        "Range is not interior to the line (split needs shots before and \
                         after). Use shift or delete the line instead.",
                    ));
                }
                let name_a = self.validate_line_name(name_a, &[line])?;
                let name_b = self.validate_line_name(name_b, &[line])?;
                if name_a == name_b {
                    return Err(EditError::refused(format!(
                        "A line named \"{name_b}\" already exists"
                    )));
                }
                if let Some(refusal) = self.split_line_budget_refusal(line) {
                    return Err(EditError::Refused(refusal));
                }
                self.apply_edit(|p| p.split_out_range(line, range, Some((name_a, name_b))))
            }
        }
    }

    /// Today's refusal when a split has no line id left to mint.
    pub(super) fn split_line_budget_refusal(&self, line: LineId) -> Option<String> {
        (self.next_line_id() > crate::MAX_LINE_ID).then(|| {
            format!(
                "Cannot split line \"{}\": to split it into two lines needs a new line id, but \
                 the project has reached the maximum number of lines (limit = {}); shot ids \
                 encode as line_id * {} + position in 32 bits, so line ids past {} cannot hold \
                 shots",
                self.line_label_of(line),
                crate::MAX_LINES_IN_ID_SPACE,
                crate::FEATURE_ID_STRIDE,
                crate::MAX_LINE_ID
            )
        })
    }

    /// The gap-close / prefix / suffix mutation.
    fn gap_close(
        &mut self,
        line: LineId,
        range: Range,
    ) -> Result<(DeletedRange, Epilogue), EditError> {
        let chain: Vec<StationId> = self.line_or_refuse(line)?.stations().collect();
        let off: Vec<StationId> = range
            .shift_indices()
            .filter_map(|i| chain.get(i).copied())
            .collect();
        let layout = self.member_tie_layout(line, &off);
        let Some(l) = self.line_mut(line) else {
            return Err(EditError::line_gone());
        };
        if range.kind == RangeDeleteKind::Prefix {
            l.start = l.shots[range.end].to;
        }
        l.shots.drain(range.start..=range.end);
        let mut gone = self.delete_stations_off_lines(&off);
        let left = self.leave_junctions(line, &off, layout.as_ref());
        gone.closures.extend(left.removed.iter().copied());
        let deleted = DeletedRange {
            line,
            kind: range.kind,
            start: range.start,
            end: range.end,
            new_line: None,
            stations_off_line: off,
            stations_deleted: gone.stations,
            datums_removed: gone.datums,
            closures_removed: gone.closures,
            comments_removed: gone.comments,
        };
        Ok((
            deleted,
            Epilogue::delete(EditContext::geometry(line), left.moving_ends),
        ))
    }

    /// The split mutation: the line keeps its shots before the range, a new
    /// line takes those after it (from station `end+1`). `names` renames the
    /// two halves (a preview passes none).
    fn split_out_range(
        &mut self,
        line: LineId,
        range: Range,
        names: Option<(String, String)>,
    ) -> Result<(DeletedRange, Epilogue), EditError> {
        let new_id = self.next_line_id();
        let chain: Vec<StationId> = self.line_or_refuse(line)?.stations().collect();
        let off: Vec<StationId> = range
            .split_indices()
            .filter_map(|i| chain.get(i).copied())
            .collect();
        let layout = self.member_tie_layout(line, &off);
        let Some(l) = self.line_mut(line) else {
            return Err(EditError::line_gone());
        };
        let tail_shots = l.shots.split_off(range.end + 1);
        l.shots.truncate(range.start);
        let tail_start = chain[range.end + 1];
        let mut second = Line {
            id: new_id,
            name: None,
            start: tail_start,
            shots: tail_shots,
            hand_moves: std::collections::BTreeMap::new(),
            next_shot_id: 0,
            ..l.clone()
        };
        second.legacy_surveyor = None;
        second.legacy_date = None;
        let tail_stations: BTreeSet<StationId> = second.stations().collect();
        let moved: Vec<StationId> = l
            .hand_moves
            .keys()
            .copied()
            .filter(|s| tail_stations.contains(s))
            .collect();
        for s in moved {
            if let Some(hand) = l.hand_moves.remove(&s) {
                second.hand_moves.insert(s, hand);
            }
        }
        if let Some((name_a, name_b)) = names {
            l.name = Some(name_a);
            second.name = Some(name_b);
        }
        second.assign_session_shot_ids();
        self.lines_mut().push(second);
        self.repoint_vias(line, new_id, &tail_stations);

        let mut gone = self.delete_stations_off_lines(&off);
        let left = self.leave_junctions(line, &off, layout.as_ref());
        gone.closures.extend(left.removed.iter().copied());
        let deleted = DeletedRange {
            line,
            kind: range.kind,
            start: range.start,
            end: range.end,
            new_line: Some(new_id),
            stations_off_line: off,
            stations_deleted: gone.stations,
            datums_removed: gone.datums,
            closures_removed: gone.closures,
            comments_removed: gone.comments,
        };
        let epilogue = Epilogue {
            ctx: EditContext::line(line),
            sweep: true,
            remove_across_one_shot: false,
            moving_ends: left.moving_ends,
        };
        Ok((deleted, epilogue))
    }

    /// Datum and closure-end `via` hints that name `from` on a station now
    /// on line `to` (and no longer on `from`) name `to`: they follow their
    /// station to the line it moved to.
    pub(super) fn repoint_vias(
        &mut self,
        from: LineId,
        to: LineId,
        stations: &BTreeSet<StationId>,
    ) {
        let topology = self.topology_arc();
        let moved = |s: StationId| stations.contains(&s) && topology.slot_on(s, from).is_none();
        for datum in &mut self.datums {
            if datum.via == Some(from) && moved(datum.station) {
                datum.via = Some(to);
            }
        }
        for closure in &mut self.closures {
            if closure.a_via == Some(from) && moved(closure.a) {
                closure.a_via = Some(to);
            }
            if closure.b_via == Some(from) && moved(closure.b) {
                closure.b_via = Some(to);
            }
        }
    }

    /// What deleting shots `first ..= last` of `line` would do (see
    /// [`DeleteRangePreview`]).
    pub fn preview_delete_shot_range(
        &self,
        line: LineId,
        first: ShotId,
        last: ShotId,
    ) -> Result<DeleteRangePreview, EditError> {
        let range = self.range_of(line, first, last)?;
        let reasons = self.shift_blocked_reasons(line, range);
        let shift_blocked = !reasons.is_empty();
        let split_only = shift_blocked && range.kind == RangeDeleteKind::Interior;
        let indices: Vec<usize> = if split_only {
            range.split_indices().collect()
        } else {
            range.shift_indices().collect()
        };
        let topology = self.topology();
        let chain = topology.chain(line).unwrap_or(&[]);
        let removed_slots: Vec<(StationRef, StationId)> = indices
            .iter()
            .filter_map(|&i| chain.get(i).map(|&s| (StationRef::new(line, i as u32), s)))
            .collect();
        let junctions_kept: Vec<StationId> = removed_slots
            .iter()
            .map(|(_, s)| *s)
            .filter(|s| topology.lines_of(*s).len() > 1)
            .collect();
        let split_deleted: Vec<StationId> = if range.kind == RangeDeleteKind::Interior {
            self.stations_only_on(line, &range.split_indices().collect::<Vec<_>>())
        } else {
            self.stations_only_on(line, &range.shift_indices().collect::<Vec<_>>())
        };
        let removed_datums: Vec<ZeroDatum> = self
            .datums
            .iter()
            .filter(|d| split_deleted.contains(&d.station))
            .copied()
            .collect();
        let shift_deleted = self.stations_only_on(line, &range.shift_indices().collect::<Vec<_>>());
        let removed_comments: Vec<(StationId, String)> = shift_deleted
            .iter()
            .filter_map(|s| {
                let comment = self.stations.get(s)?.comment.clone()?;
                (!comment.is_empty()).then_some((*s, comment))
            })
            .collect();

        let gap_close_offered = !shift_blocked && range.kind != RangeDeleteKind::WholeLine;
        let mut copy = self.clone();
        let edited = if range.kind == RangeDeleteKind::WholeLine {
            None
        } else if split_only {
            Some(copy.run_edit(self, |p| p.split_out_range(line, range, None))?)
        } else {
            Some(copy.run_edit(self, |p| p.gap_close(line, range))?)
        };
        let (removed_closures, promoted_closures, swept_closures, across) = match edited {
            Some(edited) => (
                edited.value.closures_removed,
                edited.report.sweep.promoted,
                edited.report.sweep.removed,
                edited.report.removed_across_one_shot,
            ),
            None => (
                self.closures
                    .iter()
                    .filter(|c| shift_deleted.contains(&c.a) || shift_deleted.contains(&c.b))
                    .copied()
                    .collect(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ),
        };
        Ok(DeleteRangePreview {
            line,
            start: range.start,
            end: range.end,
            kind: range.kind,
            removed_slots,
            junctions_kept,
            removed_datums,
            removed_comments,
            removed_closures,
            promoted_closures,
            swept_closures,
            shift_blocked,
            shift_blocked_reasons: reasons,
            closures_removed_across_one_shot: gap_close_offered.then_some(across),
        })
    }
}
