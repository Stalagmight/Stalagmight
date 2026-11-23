//! Shot operations (plan 5.4): create (append, insert before or after,
//! prepend), update, set kind, delete, split at distance (Insert Station),
//! and the delete-shot preview.
//!
//! - **Create** mints one station for the new chain position, named by
//!   today's creation rules (End, Mid, Prepend: `station_naming`). Insert
//!   after shot `k`: the new shot runs from `k`'s end to the new station and
//!   the shot after `k` now starts there. Prepend: the new station is the
//!   line's new start. A new shot with no credit of its own takes its
//!   neighbour's (the shot before the insert point, or the first shot when
//!   prepending); the first shot of a shot-less line takes the line's legacy
//!   surveyor and date, which the line then lets go (as today's executor).
//! - **Delete** removes one station from the line exactly as today: station 0
//!   for the first shot, else the shot's end station (the following shot
//!   re-attaches to the shot's start). A junction stays for its other lines
//!   (the line simply leaves it, as today; its new end is not merged to the
//!   junction); a station on no other line is deleted with its comment,
//!   datums and closures. At a junction the closures attached via the line
//!   follow today's member rules (`edit/junction_leave.rs`, S13). Then the
//!   orphan-closure sweep runs and closures newly across one shot are
//!   removed (#737). A mid-line delete is refused when it would close a loop
//!   or both sides are held by a placement datum (today's `shift_blocked`).
//! - **Split at distance** turns one shot into two collinear ones: a new head
//!   (fresh id and uuid) and the old shot as the tail (its id, uuid, comment
//!   and marker), with a new station between them (Mid rule), using today's
//!   split math (`Project::compute_split`, `MIN_SPLIT_CHILD_LENGTH_M`).
//!
//! Geometry edits declare `EditContext::geometry(line)`: a datum-held piece
//! stays on its datum, a free one keeps the line's first station still
//! (plan 4.1).

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::station_naming::{name_after_end, name_before_start, name_with_free_letter};
use crate::{
    LineId, ShotData, ShotId, ShotKind, ShotUpdates, StationId, StationRef, SurveyCredit,
    FEATURE_ID_STRIDE,
};

use super::super::{EditContext, Line, LoopClosure, Project, Shot, ZeroDatum};
use super::{EditError, Edited, Epilogue, Promotion};

/// Where a new shot goes on its line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShotPlace {
    /// After the line's last station (End rule).
    End,
    /// After shot `ShotId`: from its end station to a new station, the next
    /// shot now starting there (Mid rule; End rule after the last shot).
    After(ShotId),
    /// Before shot `ShotId`: the same insertion as after the shot before it;
    /// before the first shot it is a prepend.
    Before(ShotId),
    /// Before the line's first station: the new station is the new start
    /// (Prepend rule). Works on a shot-less line too.
    Start,
}

/// The shot an operation created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedShot {
    /// Its session id.
    pub shot: ShotId,
    pub uuid: String,
    /// Its index on the line.
    pub index: usize,
    /// The station it created: its end, or the new start for a prepend.
    pub station: StationId,
}

/// What a delete took away.
#[derive(Debug, Clone, PartialEq)]
pub struct DeletedShot {
    /// The shot, as it was.
    pub shot: Shot,
    /// Its index on the line before the delete.
    pub index: usize,
    /// The station the delete took off the line.
    pub station: StationId,
    /// Whether that station left the project (it was on no other line).
    pub station_deleted: bool,
    /// The datums deleted with it.
    pub datums_removed: Vec<ZeroDatum>,
    /// The closures deleted with it, as they were, then the `Tie`s the
    /// line's slot carried at a junction it left (S13).
    pub closures_removed: Vec<LoopClosure>,
    /// Its comment, deleted with it.
    pub comment_removed: Option<String>,
}

/// What Delete Shot would do, for the dialog. Computed by running the delete
/// on a copy (the facts are what the delete does), plus today's
/// shift-blocked verdict, which refuses the delete.
///
/// Replaces today's `DeleteShotImpact`: with shared stations a junction
/// simply stays for its other lines, so there are no replacement merges and
/// no closure re-anchoring to report; a closure on a junction stays on it.
#[derive(Debug, Clone, PartialEq)]
pub struct DeleteShotPreview {
    pub line: LineId,
    /// The shot's index on the line.
    pub index: usize,
    /// The slot the delete removes, before the delete: station 0 for the
    /// first shot, else the shot's end.
    pub removed_slot: StationRef,
    /// The station at that slot.
    pub removed_station: StationId,
    /// The other lines through that station: non-empty for a junction, which
    /// stays for them (only this line leaves it).
    pub station_stays_on: Vec<LineId>,
    /// The datums deleted with the station (none for a junction).
    pub removed_datums: Vec<ZeroDatum>,
    /// The comment deleted with the station.
    pub removed_comment: Option<String>,
    /// Closures deleted with the station, as they were; at a junction, the
    /// `Tie`s (demoted 0.6.0 merges) on this line's slot, which leave with
    /// it as today's merge did unless today re-stitched them (S13).
    pub removed_closures: Vec<LoopClosure>,
    /// Closures the sweep then promotes to a merge (the loop they closed is
    /// broken), as they were, with the piece that moves (a closure on this
    /// line's slot at a junction moves the junction's piece, as today).
    pub promoted_closures: Vec<Promotion>,
    /// Closures the sweep removes because their two ends became one station.
    pub swept_closures: Vec<LoopClosure>,
    /// Closures removed because the delete left them across one shot (#737).
    pub removed_across_one_shot: Vec<LoopClosure>,
    /// A mid-line delete closes the gap; it is refused when the two sides
    /// are already connected (a loop) or both are held by a placement datum.
    pub shift_blocked: bool,
    /// Today's reasons, in today's words.
    pub shift_blocked_reasons: Vec<String>,
    /// What choosing Split instead would weld (interior shots only).
    pub split_weld: Option<super::SplitWeld>,
}

/// What a split at distance made.
#[derive(Debug, Clone)]
pub struct SplitShot {
    /// The new head shot's session id and uuid (inserted before the tail).
    pub head: ShotId,
    pub head_uuid: String,
    /// The new station between the two pieces.
    pub station: StationId,
    /// The head's data and the updates applied to the tail (the command layer
    /// words its undo label from them).
    pub head_data: ShotData,
    pub tail_updates: ShotUpdates,
}

/// The most shots one line may hold (its shot ids must stay in its window).
pub(super) fn max_shots_per_line() -> usize {
    FEATURE_ID_STRIDE.saturating_sub(1) as usize
}

/// How many session shot ids `line` has left in its id window (today's
/// `Line::shot_ids_left`).
pub(super) fn shot_ids_left(line: &Line) -> u64 {
    let window_end = (u64::from(line.id) + 1) * u64::from(FEATURE_ID_STRIDE);
    window_end.saturating_sub(u64::from(line.next_shot_id))
}

/// The credit a shot stores for `credit` (empty -> none), normalised as every
/// credit is.
pub(super) fn stored_credit(credit: SurveyCredit) -> Option<Arc<SurveyCredit>> {
    credit.into_storable().stored().map(Arc::new)
}

/// Apply today's `ShotUpdates` to a shot, through the one implementation
/// (`crate::Shot::apply_updates`), so every field's rule (an empty comment
/// clears it, `null` pitch clears it) is today's exactly.
pub(super) fn apply_shot_updates(shot: &mut Shot, updates: &ShotUpdates) {
    let mut legacy = crate::Shot::with_uuid(shot.id, shot.to_shot_data(), None);
    legacy.apply_updates(updates);
    shot.azimuth_in = legacy.azimuth_in;
    shot.azimuth_out = legacy.azimuth_out;
    shot.length = legacy.length;
    shot.depth_in = legacy.depth_in;
    shot.depth_out = legacy.depth_out;
    shot.pitch_in = legacy.pitch_in;
    shot.pitch_out = legacy.pitch_out;
    shot.left = legacy.left;
    shot.right = legacy.right;
    shot.up = legacy.up;
    shot.down = legacy.down;
    shot.comment = legacy.comment;
    shot.kind = legacy.kind;
    shot.marker_index = legacy.marker_index;
    shot.temperature = legacy.temperature;
}

impl Project {
    /// The index of shot `shot` on line `line`, or today's refusal.
    pub(super) fn shot_index(&self, line: LineId, shot: ShotId) -> Result<usize, EditError> {
        self.line_or_refuse(line)?
            .shots
            .iter()
            .position(|s| s.id == shot)
            .ok_or_else(EditError::shot_gone)
    }

    /// Why one more shot cannot go into `line`, if it cannot: the line is
    /// missing, holds the most shots a line may, or has no session shot id
    /// left (today's `shot_insert_refusal`, same words).
    pub fn shot_insert_refusal(&self, line: LineId) -> Option<String> {
        let Some(l) = self.line(line) else {
            return Some(crate::LINE_GONE.to_string());
        };
        let max = max_shots_per_line();
        if l.shots.len() >= max {
            return Some(format!(
                "Line \"{}\" has reached the maximum of {max} shots",
                self.line_label_of(line)
            ));
        }
        (shot_ids_left(l) == 0).then(|| {
            format!(
                "Line \"{}\" has used up its shot numbers for this session: save and reopen to \
                 add a station to it",
                self.line_label_of(line)
            )
        })
    }

    /// Create a shot on `line` at `place` (module docs). A uuid in `data` is
    /// kept when no shot has it (redo, import), refused when one does; with
    /// none, a fresh one is minted.
    pub fn create_shot(
        &mut self,
        line: LineId,
        data: ShotData,
        place: ShotPlace,
    ) -> Result<Edited<CreatedShot>, EditError> {
        self.apply_edit(|p| {
            let created = p.create_shot_in_place(line, data, place)?;
            Ok((created, Epilogue::settle(EditContext::geometry(line))))
        })
    }

    /// [`Self::create_shot`] at the end of the line.
    pub fn append_shot(
        &mut self,
        line: LineId,
        data: ShotData,
    ) -> Result<Edited<CreatedShot>, EditError> {
        self.create_shot(line, data, ShotPlace::End)
    }

    /// [`Self::create_shot`] before the line's first station.
    pub fn prepend_shot(
        &mut self,
        line: LineId,
        data: ShotData,
    ) -> Result<Edited<CreatedShot>, EditError> {
        self.create_shot(line, data, ShotPlace::Start)
    }

    /// The mutation of [`Self::create_shot`], without the epilogue.
    fn create_shot_in_place(
        &mut self,
        line: LineId,
        mut data: ShotData,
        place: ShotPlace,
    ) -> Result<CreatedShot, EditError> {
        if !data.all_finite() {
            return Err(EditError::refused("Shot has a non-finite (NaN/inf) value"));
        }
        if let Some(refusal) = data.measurement_refusal() {
            return Err(EditError::Refused(refusal));
        }
        if let Some(refusal) = self.shot_insert_refusal(line) {
            return Err(EditError::Refused(refusal));
        }
        let l = self.line_or_refuse(line)?;
        // The index the new shot gets.
        let index = match place {
            ShotPlace::End => l.shots.len(),
            ShotPlace::Start => 0,
            ShotPlace::After(after) => self.shot_index(line, after)? + 1,
            ShotPlace::Before(before) => self.shot_index(line, before)?,
        };
        let new_shot = format!("The new shot in line \"{}\"", self.line_label_of(line));
        if data.length == 0.0 {
            return Err(EditError::refused(format!("{new_shot} has zero length")));
        }
        if data.length < 0.0 {
            return Err(EditError::refused(format!(
                "{new_shot} has negative length: {:.1}m",
                data.length
            )));
        }
        if let Some(uuid) = data.uuid.as_deref() {
            if self.is_shot_uuid_taken(uuid) {
                return Err(super::super::BuildError::ShotUuidTaken(uuid.to_string()).into());
            }
        }
        // A new shot was surveyed with the one it continues: with no credit of
        // its own it takes its neighbour's; the first shot of a shot-less
        // line takes the line's legacy surveyor and date.
        if data.credit().is_empty() {
            let l = self.line_or_refuse(line)?;
            data.credit = if l.shots.is_empty() {
                stored_credit(SurveyCredit::from_legacy(
                    l.legacy_surveyor.as_deref(),
                    l.legacy_date,
                ))
            } else {
                let neighbour = match index.checked_sub(1) {
                    Some(previous) => l.shots.get(previous),
                    None => l.shots.first(),
                };
                neighbour.and_then(|shot| shot.credit.clone())
            };
        }
        let uuid = match data.uuid.take() {
            Some(uuid) => uuid,
            None => self.mint_shot_uuid(),
        };
        data.uuid = Some(uuid.clone());

        let station = self.mint_station(String::new())?;
        let prepend = index == 0 && matches!(place, ShotPlace::Start | ShotPlace::Before(_));
        let Some(l) = self.line_mut(line) else {
            return Err(EditError::line_gone());
        };
        let id = l.mint_shot_id();
        if prepend {
            let old_start = l.start;
            l.start = station;
            l.shots.insert(0, Shot::from_data(id, data, old_start));
        } else {
            l.shots.insert(index, Shot::from_data(id, data, station));
        }
        l.legacy_surveyor = None;
        l.legacy_date = None;
        let station_index = if prepend { 0 } else { index + 1 };
        self.name_created_station(line, station_index);
        Ok(CreatedShot {
            shot: id,
            uuid,
            index,
            station,
        })
    }

    /// Name the station an operation created at chain index `index` of
    /// `line` by today's creation rules (`name_new_station`): Prepend at the
    /// start, End at the end, Mid inside, each free among the line's names.
    pub(super) fn name_created_station(&mut self, line: LineId, index: usize) {
        let Some(chain) = self.line_stations(line).map(<[StationId]>::to_vec) else {
            return;
        };
        let Some(&station) = chain.get(index) else {
            return;
        };
        // The names the line shows (a junction by the name it knows it by).
        let name_of = |p: &Project, i: usize| -> String {
            chain
                .get(i)
                .and_then(|&s| p.shown_name(s))
                .map_or_else(|| i.to_string(), str::to_string)
        };
        let taken: BTreeSet<String> = chain
            .iter()
            .filter(|s| **s != station)
            .filter_map(|&s| self.shown_name(s).map(str::to_string))
            .collect();
        let names = || taken.iter().map(String::as_str);
        let last = chain.len() - 1;
        let name = if index == 0 {
            if last == 0 {
                "0".to_string()
            } else {
                name_before_start(&name_of(self, 1), names())
            }
        } else if index == last {
            name_after_end(&name_of(self, index - 1), names())
        } else {
            name_with_free_letter(&name_of(self, index - 1), &|n| taken.contains(n))
        };
        if let Some(s) = self.stations.get_mut(&station) {
            s.name = name;
        }
    }

    /// Update one shot. A geometry edit (azimuth, length, depth, pitch)
    /// settles as one (the datum or the line's first station stays); any
    /// other edit moves nothing. Refused: a non-finite value, a zero or
    /// negative length set by the update (today's words).
    pub fn update_shot(
        &mut self,
        line: LineId,
        shot: ShotId,
        updates: &ShotUpdates,
    ) -> Result<Edited<()>, EditError> {
        if !updates.all_finite() {
            return Err(EditError::refused(
                "Shot update has a non-finite (NaN/inf) value",
            ));
        }
        if let Some(refusal) = updates.measurement_refusal() {
            return Err(EditError::Refused(refusal));
        }
        let index = self.shot_index(line, shot)?;
        if let Some(length) = updates.length {
            let label = self.shot_label_at(line, index);
            if length == 0.0 {
                return Err(EditError::refused(format!("Shot {label} has zero length")));
            }
            if length < 0.0 {
                return Err(EditError::refused(format!(
                    "Shot {label} has negative length: {length:.1}m"
                )));
            }
        }
        if !updates.touches_geometry() && updates.kind.is_none() {
            // An annotation (walls, comment, marker, temperature) moves
            // nothing and breaks no invariant but finiteness, checked above:
            // no working copy, no anchor pass, no validation (as E4's
            // accuracy-only datum edit). A paste of 1,000 wall cells is 1,000
            // of these, and a full pass each made it O(cells x survey).
            let Some(l) = self.line_mut(line) else {
                return Err(EditError::line_gone());
            };
            apply_shot_updates(&mut l.shots[index], updates);
            return Ok(Edited {
                value: (),
                report: super::EditReport::default(),
            });
        }
        let ctx = if updates.touches_geometry() {
            EditContext::geometry(line)
        } else {
            EditContext::none()
        };
        self.apply_edit(|p| {
            let Some(l) = p.line_mut(line) else {
                return Err(EditError::line_gone());
            };
            apply_shot_updates(&mut l.shots[index], updates);
            Ok(((), Epilogue::settle(ctx)))
        })
    }

    /// Set one shot's kind (no geometry change).
    pub fn set_shot_kind(
        &mut self,
        line: LineId,
        shot: ShotId,
        kind: ShotKind,
    ) -> Result<Edited<()>, EditError> {
        let index = self.shot_index(line, shot)?;
        self.apply_edit(|p| {
            let Some(l) = p.line_mut(line) else {
                return Err(EditError::line_gone());
            };
            l.shots[index].kind = kind;
            Ok(((), Epilogue::settle(EditContext::none())))
        })
    }

    /// A shot named the way the user reads it: `Line: from -> to`.
    pub(super) fn shot_label_at(&self, line: LineId, index: usize) -> String {
        let name = |i: usize| {
            self.line(line)
                .and_then(|l| l.station(i))
                .and_then(|s| self.shown_name(s))
                .map_or_else(|| i.to_string(), str::to_string)
        };
        format!(
            "{}: {} \u{2192} {}",
            self.line_label_of(line),
            name(index),
            name(index + 1)
        )
    }

    // ----- delete -----------------------------------------------------------

    /// Today's shift-blocked verdict for deleting shot `index` of `line`: only
    /// a mid-line shot closes a gap; it is blocked when its two ends are still
    /// connected without it (over shots, junctions and closures: a loop), or
    /// both sides reach a placement datum without it.
    ///
    /// The reasons name what blocks it. "Both sides are anchored" is listed
    /// only when the two sides reach different datums: in a loop both sides
    /// reach the same one, which the loop reason already covers (0.6.0 listed
    /// both; demo finding I-8). The verdict is 0.6.0's either way.
    pub(super) fn delete_shift_blocked_reasons(&self, line: LineId, index: usize) -> Vec<String> {
        let Some(l) = self.line(line) else {
            return Vec::new();
        };
        let n = l.shots.len();
        if index == 0 || index + 1 >= n {
            return Vec::new();
        }
        let (Some(before), Some(after)) = (l.station(index), l.station(index + 1)) else {
            return Vec::new();
        };
        let excluded = |l2: LineId, i: usize| l2 == line && i == index;
        let mut reasons = Vec::new();
        let sides = self.reach_without_shots(&[before, after], &excluded, true);
        let looped = sides[0].contains(&after);
        if looped {
            reasons.push(
                "This shot is part of a loop (the two sides are already connected).".to_string(),
            );
        }
        let placement_stations = self.placement_datum_stations();
        if let Some(reason) = anchored_sides_reason(&placement_stations, &sides, looped) {
            reasons.push(reason.to_string());
        }
        reasons
    }

    /// The stations of every placement datum (the first datum of each
    /// component).
    pub(super) fn placement_datum_stations(&self) -> Vec<StationId> {
        self.placement_datums()
            .into_iter()
            .flatten()
            .filter_map(|d| self.datums.get(d).map(|datum| datum.station))
            .collect()
    }

    /// For each of `starts`, every station reachable from it over shots
    /// (except those `excluded` names, by line and index), junctions (one
    /// station) and, with `closures`, loop closures.
    pub(super) fn reach_without_shots(
        &self,
        starts: &[StationId],
        excluded: &dyn Fn(LineId, usize) -> bool,
        closures: bool,
    ) -> Vec<BTreeSet<StationId>> {
        self.reach_avoiding(starts, excluded, closures, &[])
    }

    /// [`Self::reach_without_shots`] that never enters a station of `avoid`
    /// (stations the edit deletes): neither a shot nor a closure end there
    /// carries a path, as today's range walk skips a removed station.
    pub(super) fn reach_avoiding(
        &self,
        starts: &[StationId],
        excluded: &dyn Fn(LineId, usize) -> bool,
        closures: bool,
        avoid: &[StationId],
    ) -> Vec<BTreeSet<StationId>> {
        use std::collections::BTreeMap;
        let mut adjacency: BTreeMap<StationId, Vec<StationId>> = BTreeMap::new();
        let mut link = |x: StationId, y: StationId| {
            if !avoid.contains(&x) && !avoid.contains(&y) {
                adjacency.entry(x).or_default().push(y);
                adjacency.entry(y).or_default().push(x);
            }
        };
        for line in self.lines() {
            let mut from = line.start;
            for (i, shot) in line.shots.iter().enumerate() {
                if !excluded(line.id, i) {
                    link(from, shot.to);
                }
                from = shot.to;
            }
        }
        if closures {
            for c in &self.closures {
                link(c.a, c.b);
            }
        }
        starts
            .iter()
            .map(|&start| {
                let mut seen = BTreeSet::from([start]);
                let mut queue = vec![start];
                while let Some(at) = queue.pop() {
                    for &next in adjacency.get(&at).into_iter().flatten() {
                        if seen.insert(next) {
                            queue.push(next);
                        }
                    }
                }
                seen
            })
            .collect()
    }

    /// Delete one shot (module docs). Refused when the shot or line is gone,
    /// or a mid-line delete is shift-blocked (today's words, with "Use split
    /// instead.").
    pub fn delete_shot(
        &mut self,
        line: LineId,
        shot: ShotId,
    ) -> Result<Edited<DeletedShot>, EditError> {
        let index = self.shot_index(line, shot)?;
        let reasons = self.delete_shift_blocked_reasons(line, index);
        if !reasons.is_empty() {
            return Err(EditError::refused(format!(
                "{} Use split instead.",
                reasons.join(" ")
            )));
        }
        self.apply_edit(|p| p.remove_shot_station(line, index))
    }

    /// The mutation of a delete: take shot `index` and its station off
    /// `line`, and delete that station if no other line has it; at a
    /// junction, what the line kept there follows today's member rules
    /// (`edit/junction_leave.rs`).
    fn remove_shot_station(
        &mut self,
        line: LineId,
        index: usize,
    ) -> Result<(DeletedShot, Epilogue), EditError> {
        let l = self.line_or_refuse(line)?;
        let removed = l.shots.get(index).ok_or_else(EditError::shot_gone)?;
        let station = if index == 0 { l.start } else { removed.to };
        let layout = self.member_tie_layout(line, &[station]);
        let Some(l) = self.line_mut(line) else {
            return Err(EditError::line_gone());
        };
        let shot = l.shots.remove(index);
        if index == 0 {
            l.start = shot.to;
        }
        let mut gone = self.delete_stations_off_lines(&[station]);
        let left = self.leave_junctions(line, &[station], layout.as_ref());
        gone.closures.extend(left.removed.iter().copied());
        let deleted = DeletedShot {
            shot,
            index,
            station,
            station_deleted: gone.stations.contains(&station),
            datums_removed: gone.datums,
            closures_removed: gone.closures,
            comment_removed: gone.comments.into_iter().next().map(|(_, text)| text),
        };
        Ok((
            deleted,
            Epilogue::delete(EditContext::geometry(line), left.moving_ends),
        ))
    }

    /// What Delete Shot would do (see [`DeleteShotPreview`]): the delete run
    /// on a copy, without its shift-blocked refusal, plus that verdict.
    pub fn preview_delete_shot(
        &self,
        line: LineId,
        shot: ShotId,
    ) -> Result<DeleteShotPreview, EditError> {
        let index = self.shot_index(line, shot)?;
        let l = self.line_or_refuse(line)?;
        let slot_index = if index == 0 { 0 } else { index + 1 };
        let removed_slot = StationRef::new(line, slot_index as u32);
        let removed_station = l.station(slot_index).ok_or_else(EditError::shot_gone)?;
        let station_stays_on: Vec<LineId> = self
            .topology()
            .lines_of(removed_station)
            .into_iter()
            .filter(|other| *other != line)
            .collect();
        let reasons = self.delete_shift_blocked_reasons(line, index);
        let mut copy = self.clone();
        let edited = copy.run_edit(self, |p| p.remove_shot_station(line, index))?;
        Ok(DeleteShotPreview {
            line,
            index,
            removed_slot,
            removed_station,
            station_stays_on,
            removed_datums: edited.value.datums_removed,
            removed_comment: edited.value.comment_removed,
            removed_closures: edited.value.closures_removed,
            // Named in the state before the delete (a station the delete
            // gave back its alias identity, I-1, by its old id).
            promoted_closures: edited.report.pre_edit_promotions(),
            swept_closures: edited.report.pre_edit_swept(),
            removed_across_one_shot: edited.report.pre_edit_across_one_shot(),
            shift_blocked: !reasons.is_empty(),
            shift_blocked_reasons: reasons,
            split_weld: self.split_delete_weld(line, index),
        })
    }

    // ----- split at distance --------------------------------------------------

    /// Insert a station `distance` metres along shot `shot` (Insert Station,
    /// #59): today's split math and refusals. `head_uuid` names the new head
    /// shot (a redo replays it); `None` mints one.
    pub fn split_shot_at_distance(
        &mut self,
        line: LineId,
        shot: ShotId,
        distance: f64,
        head_uuid: Option<String>,
    ) -> Result<Edited<SplitShot>, EditError> {
        let index = self.shot_index(line, shot)?;
        self.apply_edit(|p| {
            let made = p.split_shot_in_place(line, index, distance, head_uuid)?;
            Ok((made, Epilogue::settle(EditContext::geometry(line))))
        })
    }

    /// The mutation of [`Self::split_shot_at_distance`] on shot `index`
    /// (also Split Line Here's cut inside a shot), with its refusals.
    pub(super) fn split_shot_in_place(
        &mut self,
        line: LineId,
        index: usize,
        distance: f64,
        head_uuid: Option<String>,
    ) -> Result<SplitShot, EditError> {
        let l = self.line_or_refuse(line)?;
        let max = max_shots_per_line();
        if l.shots.len() >= max {
            return Err(EditError::refused(format!(
                "Line \"{}\" has reached the maximum of {max} shots",
                self.line_label_of(line)
            )));
        }
        let parent = l.shots.get(index).ok_or_else(EditError::shot_gone)?;
        let legacy = crate::Shot::with_uuid(parent.id, parent.to_shot_data(), None);
        let (mut head_data, tail_updates) =
            legacy.compute_split(distance).map_err(EditError::Refused)?;
        if let Some(refusal) = self.shot_insert_refusal(line) {
            return Err(EditError::Refused(refusal));
        }
        if let Some(uuid) = head_uuid.as_deref() {
            if self.is_shot_uuid_taken(uuid) {
                return Err(super::super::BuildError::ShotUuidTaken(uuid.to_string()).into());
            }
        }
        let uuid = head_uuid.unwrap_or_else(|| self.mint_shot_uuid());
        head_data.uuid = Some(uuid.clone());
        let station = self.mint_station(String::new())?;
        let Some(l) = self.line_mut(line) else {
            return Err(EditError::line_gone());
        };
        apply_shot_updates(&mut l.shots[index], &tail_updates);
        let id = l.mint_shot_id();
        l.shots
            .insert(index, Shot::from_data(id, head_data.clone(), station));
        self.name_created_station(line, index + 1);
        Ok(SplitShot {
            head: id,
            head_uuid: uuid,
            station,
            head_data,
            tail_updates,
        })
    }
}

/// The shift-blocked reason for two sides that each reach a placement datum.
pub(super) const ANCHORED_SIDES: &str = "Both sides are anchored to a zero datum.";

/// The shift-blocked reason for two sides a loop closure ties to the same
/// placement datum (a range delete, whose loop walk leaves closures out).
pub(super) const TIED_TO_ONE_DATUM: &str =
    "Both sides are tied to the same zero datum through a loop closure.";

/// The datum reason of a gap-close's shift verdict, for `sides` (the
/// stations each side of the gap reaches) and the placement datum stations
/// `held`: `None` unless each side reaches one. Sides that reach different
/// datums are [`ANCHORED_SIDES`]. Sides that reach the same ones are
/// connected: when the loop reason is listed (`looped`) it already says so
/// and nothing is added, else the connection is a loop closure the loop walk
/// left out, [`TIED_TO_ONE_DATUM`]. 0.6.0 said "Both sides are anchored" in
/// all three cases, which read as two datums where there was one (demo
/// finding I-8); whether the shift is blocked is unchanged.
pub(super) fn anchored_sides_reason(
    held: &[StationId],
    sides: &[BTreeSet<StationId>],
    looped: bool,
) -> Option<&'static str> {
    let reached = |side: &BTreeSet<StationId>| -> BTreeSet<StationId> {
        held.iter().copied().filter(|s| side.contains(s)).collect()
    };
    let (first, second) = (reached(sides.first()?), reached(sides.get(1)?));
    if first.is_empty() || second.is_empty() {
        None
    } else if first != second {
        Some(ANCHORED_SIDES)
    } else if looped {
        None
    } else {
        Some(TIED_TO_ONE_DATUM)
    }
}
