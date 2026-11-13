//! Combine Sections, the read-only half: can two sections be joined into one,
//! and where. The context menu, the Sections panel and the dialog ask this, and
//! `combine_sections` asks it again before it changes anything.
//!
//! The rule: the two chosen sections meet end to end at exactly one point;
//! other sections there stay attached as branches. An end station of each is
//! the same point when:
//! - **merged**: the two end stations are in one merge group;
//! - **co-located**: unmerged, their chain positions (section offset plus the
//!   shot walk, without the stations' manual offsets) agree within
//!   [`COMBINE_TOLERANCE_M`] horizontally.
//!
//! Every other section with an END merged at the point stays merged to the
//! joined station, which becomes a mid-section station of the combined
//! section (a branch); one with an end there that is not merged is left as it
//! is (`CombineCandidate::loose_ends`). A station in the MIDDLE of a section
//! merged at the point blocks (that section would cross the combined one). At
//! a junction the station query lists every partner, and the user picks one.
//! Depth is not part of the match: the
//!   two stations read the depth of different shots (the state a fresh split
//!   leaves), and the joined station takes their average. A gap above
//!   [`COMBINE_DEPTH_NOTICE_M`] is reported for the dialog; one above
//!   [`COMBINE_DEPTH_LIMIT_M`] makes the ends two points, and is refused.
//!
//! Blocked, with the reason the user sees, when combining would change what
//! the survey means or break an invariant the app keeps elsewhere (see
//! [`Project::combine_blocked_reason`]).

use std::collections::{HashMap, HashSet};

use crate::{
    CombineCandidate, Section, SectionEnd, SectionId, StationKey, StationRef, FEATURE_ID_STRIDE,
};

use super::station_rekey::value_naming;
use super::Project;

/// How far apart (metres) the two join stations may be and still count as the
/// same point; also the bound on how far the other section moves.
pub const COMBINE_TOLERANCE_M: f64 = 0.001;

/// Above this depth gap (metres) between two co-located, unmerged join
/// stations, the candidate reports it and the dialog shows it: the joined
/// station takes their average, so each end moves by half the gap. Half the
/// 0.1 m resolution depth gauges and dive computers record; a smaller gap is
/// below what the survey can tell apart.
pub const COMBINE_DEPTH_NOTICE_M: f64 = 0.05;

/// Above this depth gap (metres) the two ends are refused: at one horizontal
/// point but this far apart vertically they are two points (a shaft, stacked
/// passages), not one station read twice. Merging them first, which averages
/// their depth as a deliberate choice, makes the combine possible.
pub const COMBINE_DEPTH_LIMIT_M: f64 = 1.0;

/// One way two sections can meet: an end of each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Pairing {
    pub a: SectionId,
    pub end_a: SectionEnd,
    pub b: SectionId,
    pub end_b: SectionEnd,
}

/// What a touching pairing looks like.
#[derive(Debug, Clone, Default)]
struct Touch {
    merged: bool,
    blocked: Option<String>,
    /// Other sections merged at the join point: they stay attached to the
    /// joined station, a branch of the combined section.
    branches: Vec<SectionId>,
    /// Other sections with an end at the join point that is NOT merged to
    /// it: left untouched.
    loose_ends: Vec<SectionId>,
}

/// The most other section ends the station query lists at one point. Past it
/// the point is a pile (unplaced sections all start at the origin), not a
/// junction to choose from in a menu, and every partner costs a pair query;
/// the Sections panel still combines any two of them.
pub const MAX_COMBINE_PARTNERS: usize = 16;

fn end_index(section: &Section, end: SectionEnd) -> u32 {
    match end {
        SectionEnd::Start => 0,
        SectionEnd::End => section.shots.len() as u32,
    }
}

fn label(section: &Section) -> String {
    section
        .name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| format!("Section {}", section.id))
}

fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

/// Every section end's chain position (section offset plus the shot walk,
/// without manual station offsets), computed ONCE per query: a candidate
/// query looks for co-located ends across the whole project, and walking every
/// section per pairing made a right-click O(sections x shots).
///
/// The ends are also bucketed on a grid one tolerance wide, so the ends near a
/// point are in its cell or the eight around it and a lookup reads only those,
/// not every end in the project: a pile of sections at one point (unplaced
/// sections all start at the origin) made each partner's third-end check a
/// full scan, O(sections^2) per right-click.
pub(crate) struct EndTable {
    ends: Vec<(SectionId, SectionEnd, (f64, f64))>,
    by_end: HashMap<(SectionId, SectionEnd), (f64, f64)>,
    cells: HashMap<(i64, i64), Vec<usize>>,
}

#[cfg(test)]
thread_local! {
    /// How many [`EndTable`]s this thread built: a query must build one.
    pub(crate) static END_TABLE_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many ends [`EndTable::near`] has compared against a point.
    pub(crate) static END_TABLE_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The grid cell `point` falls in. `as` saturates (and reads NaN as 0); a
/// non-finite position then fails the exact distance test in `near`, so the
/// cell it lands in does not matter.
fn cell_of(point: (f64, f64)) -> (i64, i64) {
    (
        (point.0 / COMBINE_TOLERANCE_M).floor() as i64,
        (point.1 / COMBINE_TOLERANCE_M).floor() as i64,
    )
}

impl EndTable {
    fn new(ends: Vec<(SectionId, SectionEnd, (f64, f64))>) -> Self {
        let by_end = ends.iter().map(|(id, end, p)| ((*id, *end), *p)).collect();
        let mut cells: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, (_, _, p)) in ends.iter().enumerate() {
            cells.entry(cell_of(*p)).or_default().push(i);
        }
        Self {
            ends,
            by_end,
            cells,
        }
    }

    fn at(&self, id: SectionId, end: SectionEnd) -> Option<(f64, f64)> {
        self.by_end.get(&(id, end)).copied()
    }

    /// Every section end within the tolerance of `point`, lazily: a caller
    /// that stops at the first match reads only the ends before it.
    fn near(&self, point: (f64, f64)) -> impl Iterator<Item = (SectionId, SectionEnd)> + '_ {
        let (cx, cy) = cell_of(point);
        (-1..=1)
            .flat_map(move |dx| {
                (-1..=1).map(move |dy| (cx.saturating_add(dx), cy.saturating_add(dy)))
            })
            .filter_map(|cell| self.cells.get(&cell))
            .flatten()
            .map(|&i| &self.ends[i])
            .filter(move |(_, _, p)| {
                #[cfg(test)]
                END_TABLE_PROBES.with(|probes| probes.set(probes.get() + 1));
                distance(*p, point) <= COMBINE_TOLERANCE_M
            })
            .map(|(id, end, _)| (*id, *end))
    }
}

impl Project {
    /// See [`EndTable`]. The walk is the coordinate solver's
    /// (`compute_station_raw_xy`) without the station offsets.
    pub(crate) fn end_table(&self) -> EndTable {
        #[cfg(test)]
        END_TABLE_BUILDS.with(|builds| builds.set(builds.get() + 1));
        let mut ends = Vec::with_capacity(self.sections.len() * 2);
        for section in &self.sections {
            let start = self.section_offset(section.id);
            let reference = self.azimuth_reference_for(section);
            let (mut x, mut y) = start;
            for shot in &section.shots {
                let azimuth = shot.average_azimuth_true_rad(reference);
                let horizontal = shot.horizontal_length();
                x += horizontal * azimuth.sin();
                y += horizontal * azimuth.cos();
            }
            ends.push((section.id, SectionEnd::Start, start));
            ends.push((section.id, SectionEnd::End, (x, y)));
        }
        EndTable::new(ends)
    }

    /// Does `station` carry a resolvable zero datum?
    fn has_datum_at(&self, station: StationRef) -> bool {
        self.zero_datums
            .iter()
            .any(|datum| self.resolve_key(&datum.station_ref) == Some(station))
    }

    /// Whether `pairing`'s two end stations are the same point (`None` when
    /// they are not), and if so why combining there is blocked, if it is.
    fn pairing_touch(&self, pairing: Pairing, ends: &EndTable) -> Option<Touch> {
        let section_a = self.get_section(pairing.a)?;
        let section_b = self.get_section(pairing.b)?;
        let at_a = StationRef::new(pairing.a, end_index(section_a, pairing.end_a));
        let at_b = StationRef::new(pairing.b, end_index(section_b, pairing.end_b));
        let (pa, pb) = (
            ends.at(pairing.a, pairing.end_a)?,
            ends.at(pairing.b, pairing.end_b)?,
        );
        let apart = distance(pa, pb);
        let group_a = self.get_merge_group(at_a);
        let merged = group_a.contains(&at_b);
        if !merged && apart > COMBINE_TOLERANCE_M {
            return None;
        }
        // Every station at the join point: both join stations' merge groups.
        let mut join_group = group_a;
        if !merged {
            join_group.extend(self.get_merge_group(at_b));
        }
        let mut branches: Vec<SectionId> = join_group
            .iter()
            .map(|at| at.section_id)
            .filter(|id| *id != pairing.a && *id != pairing.b)
            .collect();
        branches.sort_unstable();
        branches.dedup();
        let mut loose_ends: Vec<SectionId> = ends
            .near(pa)
            .map(|(id, _)| id)
            .filter(|id| *id != pairing.a && *id != pairing.b && !branches.contains(id))
            .collect();
        loose_ends.sort_unstable();
        loose_ends.dedup();
        let blocked = self
            .mid_section_at_join(&join_group)
            .or_else(|| self.combine_blocked_reason(pairing, at_a, at_b, merged, &join_group))
            .or_else(|| {
                // A merge the offset solver could not align: combining would move
                // the other section by the gap.
                (apart > COMBINE_TOLERANCE_M).then(|| {
                    format!(
                        "The merged join stations are {:.1} mm apart",
                        apart * 1000.0
                    )
                })
            });
        Some(Touch {
            merged,
            blocked,
            branches,
            loose_ends,
        })
    }

    /// Why a station merged at the join point stops the combine, if one
    /// does: one in the MIDDLE of its section. Other sections' ends there stay
    /// attached to the joined station as branches, but a section passing
    /// through the point would cross the combined one there.
    fn mid_section_at_join(&self, join_group: &HashSet<StationRef>) -> Option<String> {
        let mut through: Vec<&StationRef> = join_group
            .iter()
            .filter(|at| {
                self.get_section(at.section_id).is_some_and(|section| {
                    at.station_index > 0 && (at.station_index as usize) < section.shots.len()
                })
            })
            .collect();
        through.sort_by_key(|at| (at.section_id, at.station_index));
        through.first().map(|at| {
            format!(
                "{} is merged here at a station in its middle",
                self.get_section(at.section_id)
                    .map_or_else(|| format!("Section {}", at.section_id), label)
            )
        })
    }

    /// Why combining at a touching `pairing` would be wrong, or `None`.
    fn combine_blocked_reason(
        &self,
        pairing: Pairing,
        at_a: StationRef,
        at_b: StationRef,
        merged: bool,
        join_group: &HashSet<StationRef>,
    ) -> Option<String> {
        let a = self.get_section(pairing.a)?;
        let b = self.get_section(pairing.b)?;
        let (name_a, name_b) = (label(a), label(b));
        if a.shots.is_empty() || b.shots.is_empty() {
            return Some("A section with no shots cannot be combined".to_string());
        }
        // One uuid in both would put the same shot in the section twice, and
        // every `AfterShot` anchor names the first of them (from #713).
        let uuids_a: HashSet<&str> = a.shots.iter().filter_map(|s| s.uuid.as_deref()).collect();
        if b.shots
            .iter()
            .filter_map(|s| s.uuid.as_deref())
            .any(|uuid| uuids_a.contains(uuid))
        {
            return Some(format!(
                "{name_a} and {name_b} share a shot identity (the same shot uuid)"
            ));
        }
        // The RAW references: the combined section keeps only the kept one's
        // `azimuth_reference`, so a section that declares its own reference and
        // one that follows the project default must not merge even while they
        // resolve alike. A later change of the default would silently rotate
        // the shots that brought their own.
        if let Some(reason) = Self::north_reference_mismatch(a, b, &name_a, &name_b) {
            return Some(reason);
        }
        if (a.quality - b.quality).abs() > 1e-12 {
            return Some(format!(
                "{name_a} and {name_b} have different quality factors"
            ));
        }
        if self.rigidly_connected_beyond_join(at_a, at_b, join_group) {
            return Some(format!(
                "{name_a} and {name_b} are also connected elsewhere (a loop)"
            ));
        }
        if self.has_datum_at(at_a) && self.has_datum_at(at_b) {
            return Some("Both join stations carry a zero datum".to_string());
        }
        if !merged
            && self.section_has_zero_datum(pairing.a)
            && self.section_has_zero_datum(pairing.b)
        {
            return Some(format!("{name_a} and {name_b} each have a zero datum"));
        }
        if let Some(gap) = self.join_depth_gap(at_a, at_b, merged) {
            if gap > COMBINE_DEPTH_LIMIT_M {
                return Some(format!(
                    "The two ends are {gap:.2} m apart in depth (more than \
                     {COMBINE_DEPTH_LIMIT_M:.0} m), so they are not one station; merge them \
                     first if they are"
                ));
            }
        }
        let offset = |at| self.station_offset_for_ref(at).unwrap_or_default();
        let (oa, ob) = (offset(at_a), offset(at_b));
        let moved = ((oa.0 - ob.0).powi(2) + (oa.1 - ob.1).powi(2) + (oa.2 - ob.2).powi(2)).sqrt();
        if moved > COMBINE_TOLERANCE_M {
            return Some("The join stations were moved by different amounts".to_string());
        }
        let budget = FEATURE_ID_STRIDE.saturating_sub(1) as usize;
        if a.shots.len() + b.shots.len() > budget {
            return Some(format!(
                "The combined section would have {} shots (at most {budget})",
                a.shots.len() + b.shots.len()
            ));
        }
        None
    }

    /// Why `kept` cannot take `other`'s shots: they get new ids from `kept`,
    /// which has only [`Section::shot_ids_left`] of them. Not renumbered in
    /// place: position-
    /// keyed ignored warnings and the frontend's selection, hover and table
    /// state address shots by id. A reload renumbers every section compactly.
    pub(crate) fn shot_id_room_refusal(kept: &Section, other: &Section) -> Option<String> {
        (other.shots.len() as u64 > kept.shot_ids_left()).then(|| {
            format!(
                "{} has used up its shot numbers for this session: keep {}, or save and \
                 reopen",
                label(kept),
                label(other)
            )
        })
    }

    /// Why combining `other` into `kept` is refused, if it is: the pair's
    /// reason, else the one that depends on which section is kept. The one
    /// check `combine_sections` and the command both run before changing
    /// anything.
    pub fn combine_refusal(&self, kept: SectionId, other: SectionId) -> Option<String> {
        let Some(candidate) = self.combine_candidate_for_sections(kept, other) else {
            return Some(format!("Sections {kept} and {other} cannot be combined"));
        };
        candidate.blocked_reason.or(if candidate.section_a == kept {
            candidate.blocked_keeping_a
        } else {
            candidate.blocked_keeping_b
        })
    }

    /// Why two sections' stored north references keep them apart, if they do.
    fn north_reference_mismatch(
        a: &Section,
        b: &Section,
        name_a: &str,
        name_b: &str,
    ) -> Option<String> {
        match (a.azimuth_reference, b.azimuth_reference) {
            (None, None) => None,
            (Some(x), Some(y))
                if x.north == y.north && (x.correction - y.correction).abs() <= 1e-9 =>
            {
                None
            }
            (Some(_), Some(_)) => Some(format!(
                "{name_a} and {name_b} use different north references"
            )),
            (None, Some(_)) => Some(format!(
                "{name_a} follows the project's default north reference, {name_b} declares \
                 its own"
            )),
            (Some(_), None) => Some(format!(
                "{name_b} follows the project's default north reference, {name_a} declares \
                 its own"
            )),
        }
    }

    /// Are the join stations' sections still rigidly connected (merges and
    /// aligned closures) once every tie AT the join point is removed (between
    /// two of its stations, `join_group`, a third section's end included)?
    /// Then the combined section would be tied to itself: a rigid loop inside
    /// one section, or through a third section (which `merge_stations`
    /// refuses for the same reason). A third section merged only at the join
    /// point is a branch, not a loop.
    fn rigidly_connected_beyond_join(
        &self,
        at_a: StationRef,
        at_b: StationRef,
        join_group: &HashSet<StationRef>,
    ) -> bool {
        let at_join = |x: Option<StationRef>| x.is_some_and(|x| join_group.contains(&x));
        let is_join = |x: Option<StationRef>, y: Option<StationRef>| {
            (at_join(x) && at_join(y))
                || (x == Some(at_a) && y == Some(at_b))
                || (x == Some(at_b) && y == Some(at_a))
        };
        let mut adjacency: HashMap<SectionId, Vec<SectionId>> = HashMap::new();
        let mut edge = |x: &StationKey, y: &StationKey| {
            adjacency
                .entry(x.section_id)
                .or_default()
                .push(y.section_id);
            adjacency
                .entry(y.section_id)
                .or_default()
                .push(x.section_id);
        };
        for merge in &self.station_merges {
            if !is_join(
                self.resolve_key(&merge.station_a),
                self.resolve_key(&merge.station_b),
            ) {
                edge(&merge.station_a, &merge.station_b);
            }
        }
        for closure in self.loop_closures.iter().filter(|c| c.is_aligned) {
            if !is_join(
                self.resolve_key(&closure.station_a),
                self.resolve_key(&closure.station_b),
            ) {
                edge(&closure.station_a, &closure.station_b);
            }
        }
        let mut seen: HashSet<SectionId> = HashSet::new();
        let mut queue = vec![at_a.section_id];
        while let Some(current) = queue.pop() {
            if current == at_b.section_id {
                return true;
            }
            if seen.insert(current) {
                queue.extend(adjacency.get(&current).into_iter().flatten().copied());
            }
        }
        false
    }

    /// The depth gap between two co-located join stations that are NOT
    /// merged, each read on its own (`compute_station_depth`); `None` for a
    /// merged join, whose stations are already one depth.
    fn join_depth_gap(&self, at_a: StationRef, at_b: StationRef, merged: bool) -> Option<f64> {
        (!merged).then(|| {
            (self.compute_station_depth(&[at_a]) - self.compute_station_depth(&[at_b])).abs()
        })
    }

    /// The answer for one touching pairing, `a` the lower section id.
    fn candidate_for(&self, pairing: Pairing, touch: Touch) -> Option<CombineCandidate> {
        let pairing = if pairing.a <= pairing.b {
            pairing
        } else {
            Pairing {
                a: pairing.b,
                end_a: pairing.end_b,
                b: pairing.a,
                end_b: pairing.end_a,
            }
        };
        let a = self.get_section(pairing.a)?;
        let b = self.get_section(pairing.b)?;
        let station_a = StationRef::new(a.id, end_index(a, pairing.end_a));
        let station_b = StationRef::new(b.id, end_index(b, pairing.end_b));
        let differing = |map: &HashMap<StationKey, String>| match (
            value_naming(self, map, station_a),
            value_naming(self, map, station_b),
        ) {
            (Some(x), Some(y)) if x != y => (Some(x.clone()), Some(y.clone())),
            _ => (None, None),
        };
        let (comment_a, comment_b) = differing(&self.station_comments);
        let (station_name_a, station_name_b) = differing(&self.station_names);
        Some(CombineCandidate {
            section_a: a.id,
            section_b: b.id,
            end_a: pairing.end_a,
            end_b: pairing.end_b,
            station_a,
            station_b,
            merged: touch.merged,
            reverses: pairing.end_a == pairing.end_b,
            blocked_reason: touch.blocked,
            blocked_keeping_a: Self::shot_id_room_refusal(a, b),
            blocked_keeping_b: Self::shot_id_room_refusal(b, a),
            shot_count_a: a.shots.len() as u32,
            shot_count_b: b.shots.len() as u32,
            comment_a,
            comment_b,
            station_name_a,
            station_name_b,
            depth_difference_m: self
                .join_depth_gap(station_a, station_b, touch.merged)
                .filter(|gap| *gap > COMBINE_DEPTH_NOTICE_M),
            branches: touch.branches,
            loose_ends: touch.loose_ends,
        })
    }

    /// Every pairing of an end of `s1` with an end of `s2` that touches.
    fn touching_pairings(
        &self,
        s1: SectionId,
        s2: SectionId,
        table: &EndTable,
    ) -> Vec<(Pairing, Touch)> {
        let ends = [SectionEnd::Start, SectionEnd::End];
        ends.iter()
            .flat_map(|&end_a| ends.iter().map(move |&end_b| (end_a, end_b)))
            .filter_map(|(end_a, end_b)| {
                let pairing = Pairing {
                    a: s1,
                    end_a,
                    b: s2,
                    end_b,
                };
                self.pairing_touch(pairing, table)
                    .map(|touch| (pairing, touch))
            })
            .collect()
    }

    /// Can sections `s1` and `s2` be combined? Tries all four end pairings and
    /// requires exactly one to be eligible: two sections that meet at both ends
    /// are ambiguous (and usually a loop). `None` when either section is
    /// missing or they are the same section.
    pub fn combine_candidate_for_sections(
        &self,
        s1: SectionId,
        s2: SectionId,
    ) -> Option<CombineCandidate> {
        self.candidate_for_sections_with(s1, s2, &self.end_table())
    }

    /// [`Self::combine_candidate_for_sections`] against an end table the
    /// caller already built (the station query builds one for its own search).
    fn candidate_for_sections_with(
        &self,
        s1: SectionId,
        s2: SectionId,
        table: &EndTable,
    ) -> Option<CombineCandidate> {
        if s1 == s2 {
            return None;
        }
        // Lower id first, so the answer (and every reason's wording) is the
        // same whichever section the caller names first or keeps.
        let (s1, s2) = (s1.min(s2), s1.max(s2));
        let (a, b) = (self.get_section(s1)?, self.get_section(s2)?);
        let touching = self.touching_pairings(s1, s2, table);
        let eligible: Vec<&(Pairing, Touch)> = touching
            .iter()
            .filter(|(_, t)| t.blocked.is_none())
            .collect();
        match (eligible.as_slice(), touching.first()) {
            ([(pairing, touch)], _) => self.candidate_for(*pairing, touch.clone()),
            ([], Some((pairing, touch))) => self.candidate_for(*pairing, touch.clone()),
            ([(pairing, touch), ..], _) => {
                let mut touch = touch.clone();
                touch.blocked = Some(format!(
                    "{} and {} meet at both ends (a loop)",
                    label(a),
                    label(b)
                ));
                self.candidate_for(*pairing, touch)
            }
            ([], None) => {
                let pairing = Pairing {
                    a: s1,
                    end_a: SectionEnd::End,
                    b: s2,
                    end_b: SectionEnd::Start,
                };
                let touch = Touch {
                    blocked: Some(format!(
                        "{} and {} do not meet at an end station",
                        label(a),
                        label(b)
                    )),
                    ..Touch::default()
                };
                self.candidate_for(pairing, touch)
            }
        }
    }

    /// The first of [`Self::combine_candidates`]: what a station with one
    /// partner offers.
    pub fn combine_candidate(&self, station: StationRef) -> Option<CombineCandidate> {
        self.combine_candidates(station).into_iter().next()
    }

    /// What the section that `station` ends can be combined with: one
    /// candidate per other section with an end at the same point (merged to
    /// it, or co-located), in section order, each with its own reason when it
    /// is blocked. At a junction the user picks one; the others stay attached
    /// to the joined station as branches (or, unmerged, untouched). Empty when
    /// `station` is not an end of its section or nothing else ends there.
    ///
    /// A station merged only to the middle of other sections gets one blocked
    /// candidate saying so; a pile of more than [`MAX_COMBINE_PARTNERS`] other
    /// ends gets one blocked candidate pointing at the Sections panel.
    pub fn combine_candidates(&self, station: StationRef) -> Vec<CombineCandidate> {
        let Some(section) = self.get_section(station.section_id) else {
            return Vec::new();
        };
        let end = if station.station_index == 0 {
            SectionEnd::Start
        } else if station.station_index as usize == section.shots.len() {
            SectionEnd::End
        } else {
            return Vec::new();
        };
        // Every other section with an end at this point: merged to it, or
        // co-located with it.
        let table = self.end_table();
        let Some(at) = table.at(section.id, end) else {
            return Vec::new();
        };
        let group = self.get_merge_group(station);
        let mut other_ends: Vec<(SectionId, SectionEnd)> = table.near(at).collect();
        for member in &group {
            if let Some(other) = self.get_section(member.section_id) {
                if member.station_index == 0 {
                    other_ends.push((other.id, SectionEnd::Start));
                }
                if member.station_index as usize == other.shots.len() {
                    other_ends.push((other.id, SectionEnd::End));
                }
            }
        }
        other_ends.retain(|(id, _)| *id != section.id);
        other_ends.sort_by_key(|(id, end)| (*id, *end == SectionEnd::End));
        other_ends.dedup();
        let pairing_with = |(id, other_end): (SectionId, SectionEnd)| Pairing {
            a: section.id,
            end_a: end,
            b: id,
            end_b: other_end,
        };
        if other_ends.len() > MAX_COMBINE_PARTNERS {
            let count = other_ends.len() + 1;
            let Some((pairing, mut touch)) = other_ends.into_iter().find_map(|other| {
                let pairing = pairing_with(other);
                self.pairing_touch(pairing, &table)
                    .map(|touch| (pairing, touch))
            }) else {
                return Vec::new();
            };
            touch.blocked = Some(format!(
                "{count} section ends meet here: select the two to combine in the Sections \
                 panel"
            ));
            return self.candidate_for(pairing, touch).into_iter().collect();
        }
        if other_ends.is_empty() {
            if group.len() <= 1 {
                return Vec::new();
            }
            // Merged only to stations that are not section ends: a section
            // passes through here, and there is no other end to join.
            let Some(other) = group.iter().find(|s| **s != station) else {
                return Vec::new();
            };
            let pairing = Pairing {
                a: section.id,
                end_a: end,
                b: other.section_id,
                end_b: SectionEnd::Start,
            };
            let touch = Touch {
                merged: true,
                blocked: Some("Merged to the middle of another section".to_string()),
                ..Touch::default()
            };
            return self.candidate_for(pairing, touch).into_iter().collect();
        }
        other_ends
            .into_iter()
            .filter_map(|other| {
                let pairing = pairing_with(other);
                let mut touch = self.pairing_touch(pairing, &table)?;
                if touch.blocked.is_none() {
                    // The pair's own answer decides, so the station menu, the
                    // Sections panel and the command agree: two sections that
                    // meet at both ends are ambiguous from either. When it is
                    // eligible it is this pairing, the only eligible one.
                    touch.blocked = self
                        .candidate_for_sections_with(pairing.a, pairing.b, &table)?
                        .blocked_reason;
                }
                self.candidate_for(pairing, touch)
            })
            .collect()
    }
}
