//! Combine Sections (#707): join two sections whose ends meet into one, the
//! inverse of Split Section Here.
//!
//! Two ends meet when they are station-merged (directly or through other
//! merges) or lie within [`JOIN_TOLERANCE_M`] of each other horizontally in
//! measured positions, which is what a fresh split leaves. Exactly one pair of
//! ends may meet: two would make the combined section a ring. The kept section
//! keeps its id and direction; the other is reversed first (left and right
//! swapped) when its matching end faces the wrong way, then its shots are
//! appended or prepended with fresh ids and their own uuids, and the other
//! section is removed.
//!
//! The two ends become one station:
//! - its comment, name and manual offset come from the end the caller picks
//!   when both carry a different one, else from whichever carries one;
//! - a zero datum on either end survives, the kept end's when both have one;
//! - merges re-point to it and are de-duplicated, and the merge that joined
//!   the two ends is dropped, as is a loop closure between them.
//!
//! Credit: every shot of the other section whose effective surveyor or date
//! differs from the kept section's gets that value as an override, so no metre
//! changes hands. Nothing is folded afterwards.
//!
//! Positions: every station keeps its measured xy, with two exceptions. The
//! other section's stations move by the (at most 1 mm) gap between the ends
//! when they did not coincide exactly, and by the difference between the two
//! ends' manual offsets when those differed. Depth is the inverse of a split's:
//! the joined station now averages the `depth_out` of the shot arriving and the
//! `depth_in` of the shot leaving, where each unmerged end read its own.
//!
//! Gauge: the kept section's least-squares gauge pair is captured before its
//! shots move, so a prepend does not walk the gauge onto the new station 0.
//! A component's gauge sits on its lowest section id, so when the other
//! section has the lower id its gauge station replaces the kept section's.

use std::collections::{HashMap, HashSet, VecDeque};

use chrono::NaiveDate;

use crate::{
    shot_run_displacement, CombineCandidate, CombineOptions, CombinePairing, CreditRun, Section,
    SectionId, ShotCredit, ShotData, SplitSide, StationKey, StationRef,
};

use super::union_find::UnionFind;
use super::Project;

/// How far apart two section ends may lie, horizontally, and still meet.
const JOIN_TOLERANCE_M: f64 = 1e-3;

type Adjacency = HashMap<StationRef, Vec<StationRef>>;

/// Every station reachable from `from` over `adjacency`, `from` included.
fn reach(adjacency: &Adjacency, from: StationRef) -> HashSet<StationRef> {
    let mut seen = HashSet::from([from]);
    let mut queue = VecDeque::from([from]);
    while let Some(current) = queue.pop_front() {
        for &next in adjacency.get(&current).into_iter().flatten() {
            if seen.insert(next) {
                queue.push_back(next);
            }
        }
    }
    seen
}

/// Remove and return the value `map` holds for station `at`, under whichever
/// stored key names it.
fn take_at<V>(project: &Project, map: &mut HashMap<StationKey, V>, at: StationRef) -> Option<V> {
    let keys: Vec<StationKey> = map
        .keys()
        .filter(|key| project.resolve_key(key) == Some(at))
        .cloned()
        .collect();
    let values: Vec<V> = keys.iter().filter_map(|key| map.remove(key)).collect();
    values.into_iter().next()
}

/// The value that survives on the joined station: the picked side's when both
/// ends carry a different one, else whichever end carries one.
fn choose<V: PartialEq>(kept: Option<V>, other: Option<V>, side: SplitSide) -> Option<V> {
    match (kept, other) {
        (Some(kept), Some(other)) if side == SplitSide::B && kept != other => Some(other),
        (kept, other) => kept.or(other),
    }
}

/// The value `map` holds for station `at`, under whichever stored key names it.
fn value_at<'a, V>(
    project: &Project,
    map: &'a HashMap<StationKey, V>,
    at: StationRef,
) -> Option<&'a V> {
    map.iter()
        .find(|(key, _)| project.resolve_key(key) == Some(at))
        .map(|(_, value)| value)
}

/// Both values are present and differ.
fn differ<V: PartialEq>(a: Option<&V>, b: Option<&V>) -> bool {
    matches!((a, b), (Some(a), Some(b)) if a != b)
}

/// A gauge pair naming `section`'s station `station`; see
/// [`Section::gauge_shot_uuid`].
fn gauge_pair_at(section: &Section, station: usize) -> (Option<String>, Option<String>) {
    let uuid = |index: usize| section.shots.get(index).and_then(|s| s.uuid.clone());
    if station + 1 < section.shots.len() {
        (uuid(station), uuid(station + 1))
    } else if station >= 2 {
        (uuid(station - 1), uuid(station - 2))
    } else if section.reverse_parity {
        (station.checked_sub(1).and_then(uuid), None)
    } else {
        (uuid(station), None)
    }
}

/// Each shot's effective `(surveyor, date, length)`, in section order. An
/// empty surveyor is unknown, the same as none.
fn shot_credits(section: &Section) -> Vec<(Option<String>, Option<NaiveDate>, f64)> {
    section
        .shots
        .iter()
        .map(|shot| {
            let credit = shot.credit();
            (
                credit
                    .surveyor_or(section.surveyor.as_deref())
                    .filter(|surveyor| !surveyor.is_empty())
                    .map(str::to_owned),
                credit.date_or(section.date),
                shot.length,
            )
        })
        .collect()
}

impl Project {
    /// Whether sections `a` and `b` can be combined, keeping the lower id.
    /// `None` when either is missing or has no shots, they are the same
    /// section, or no end of one meets an end of the other.
    pub fn combine_candidate(&self, a: SectionId, b: SectionId) -> Option<CombineCandidate> {
        self.combine_candidate_keeping(a.min(b), a.max(b))
    }

    /// The combine offered at `station`: it has to be the first or last
    /// station of its section, and meet an end of exactly one other section.
    pub fn combine_candidate_at_station(&self, station: StationRef) -> Option<CombineCandidate> {
        let shots = self.get_section(station.section_id)?.shots.len() as u32;
        if shots == 0 || (station.station_index != 0 && station.station_index != shots) {
            return None;
        }
        let merged = reach(&self.build_station_merge_adjacency(), station);
        let here = self.compute_station_raw_xy(station)?;
        let mut partners = self
            .sections
            .iter()
            .filter(|s| s.id != station.section_id && !s.shots.is_empty())
            .filter(|s| {
                [0, s.shots.len() as u32].into_iter().any(|index| {
                    let end = StationRef::new(s.id, index);
                    merged.contains(&end) || self.within_join_tolerance(here, end)
                })
            })
            .map(|s| s.id);
        let other = partners.next()?;
        if partners.next().is_some() {
            return None;
        }
        self.combine_candidate(station.section_id, other)
    }

    /// Join `other` onto `kept`, which keeps its id and direction. Refused, with
    /// nothing changed, when the two do not meet end to end or the combine is
    /// blocked; see [`Self::combine_candidate`].
    pub fn combine_sections(
        &mut self,
        kept: SectionId,
        other: SectionId,
        options: &CombineOptions,
    ) -> Result<(), String> {
        if options.name_side == SplitSide::Both || options.offset_side == SplitSide::Both {
            return Err("A joined station keeps only one name and one manual offset".to_string());
        }
        let candidate = self
            .combine_candidate_keeping(kept, other)
            .ok_or_else(|| format!("Sections {kept} and {other} do not meet end to end"))?;
        if let Some(reason) = candidate.blocked {
            return Err(reason);
        }
        let pairing = candidate.pairing;
        let prepend = pairing.prepends();
        let kept_shots = self
            .get_section(kept)
            .map_or(0, |section| section.shots.len() as u32);
        let other_shots = self
            .get_section(other)
            .map_or(0, |section| section.shots.len() as u32);

        self.ensure_section_shot_uuids(kept);
        self.ensure_section_shot_uuids(other);
        if let Some(section) = self.get_section_mut(other) {
            Self::capture_missing_gauge_anchor(section);
        }
        if pairing.reverses_other() {
            self.reverse_section(other, true);
        }
        if let Some(section) = self.get_section_mut(kept) {
            Self::capture_missing_gauge_anchor(section);
        }
        // The lower id carries the component's gauge, so keeping the higher
        // one has to keep the other section's gauge station.
        let other_gauge = (other < kept)
            .then(|| self.get_section(other))
            .flatten()
            .map(|section| {
                (
                    section.gauge_shot_uuid.clone(),
                    section.gauge_neighbour_uuid.clone(),
                    Self::gauge_index(section) as u32,
                )
            });

        // After the reverse the other section always joins at its start
        // (append) or its end (prepend).
        let (kept_end, other_end) = if prepend {
            (
                StationRef::new(kept, 0),
                StationRef::new(other, other_shots),
            )
        } else {
            (StationRef::new(kept, kept_shots), StationRef::new(other, 0))
        };
        // The two ends become one key below; settle what both hold first, or
        // the HashMap rebuild in `map_station_refs` silently keeps one.
        self.settle_joined_station(kept_end, other_end, options);

        let old: HashMap<StationKey, StationRef> = {
            let mut out = HashMap::new();
            self.for_each_station_ref(|_label, key| {
                if (key.section_id == kept || key.section_id == other) && !out.contains_key(&key) {
                    if let Some(sr) = self.resolve_key(&key) {
                        out.insert(key, sr);
                    }
                }
            });
            out
        };

        let (Some(kept_section), Some(other_section)) =
            (self.get_section(kept), self.get_section(other))
        else {
            return Err(format!("Sections {kept} and {other} not found"));
        };
        let (kept_surveyor, kept_date) = (kept_section.surveyor.clone(), kept_section.date);
        let data: Vec<ShotData> = other_section
            .shots
            .iter()
            .map(|shot| {
                let credit = shot.credit();
                let surveyor = credit.surveyor_or(other_section.surveyor.as_deref());
                let date = credit.date_or(other_section.date);
                ShotData {
                    credit: ShotCredit {
                        surveyor: (surveyor != kept_surveyor.as_deref())
                            .then(|| surveyor.map(str::to_owned)),
                        date: (date != kept_date).then_some(date),
                    }
                    .into_stored(),
                    ..shot.to_shot_data()
                }
            })
            .collect();
        let (dx, dy) = shot_run_displacement(&data, self.azimuth_reference_for_id(kept));
        let (ox, oy) = self.section_offset(kept);

        let Some(section) = self.get_section_mut(kept) else {
            return Err(format!("Section {kept} not found"));
        };
        section.name = (!options.name.is_empty()).then(|| options.name.clone());
        section.color.clone_from(&options.color);
        for shot in data {
            section.add_shot(shot);
        }
        if prepend {
            section.shots.rotate_right(other_shots as usize);
            if dx.is_finite() && dy.is_finite() {
                self.set_section_offset(kept, (ox - dx, oy - dy));
            }
        }
        self.remove_section_offset(other);
        self.remove_section(other);
        self.reindex_section_shot_uuids(kept);
        self.reindex_section_shot_uuids(other);

        let moved = |sr: StationRef| -> StationRef {
            match (sr.section_id == other, prepend) {
                (true, true) => StationRef::new(kept, sr.station_index),
                (true, false) => StationRef::new(kept, kept_shots + sr.station_index),
                (false, true) => StationRef::new(kept, other_shots + sr.station_index),
                (false, false) => sr,
            }
        };
        let rebuilt: HashMap<StationKey, StationKey> = old
            .into_iter()
            .filter_map(|(key, sr)| Some((key, self.key_for_ref(moved(sr))?)))
            .collect();
        self.map_station_refs(|key| rebuilt.get(&key).cloned().unwrap_or(key));

        if let Some((shot, neighbour, index)) = other_gauge {
            let station = moved(StationRef::new(other, index)).station_index as usize;
            if let Some(section) = self.get_section_mut(kept) {
                // Without a neighbour the side comes from the parity, which is
                // kept's now; name the station afresh instead.
                (section.gauge_shot_uuid, section.gauge_neighbour_uuid) = if neighbour.is_some() {
                    (shot, neighbour)
                } else {
                    gauge_pair_at(section, station)
                };
            }
        }

        self.tidy_joined_relationships(kept);
        self.touch();
        Ok(())
    }

    /// The candidate with `kept` kept; see [`Self::combine_candidate`].
    fn combine_candidate_keeping(
        &self,
        kept: SectionId,
        other: SectionId,
    ) -> Option<CombineCandidate> {
        if kept == other {
            return None;
        }
        let kept_shots = self.get_section(kept)?.shots.len() as u32;
        let other_shots = self.get_section(other)?.shots.len() as u32;
        if kept_shots == 0 || other_shots == 0 {
            return None;
        }
        let merges = self.build_station_merge_adjacency();
        let meeting: Vec<CombinePairing> = CombinePairing::ALL
            .into_iter()
            .filter(|pairing| {
                let (k, o) = pairing.ends(kept_shots, other_shots);
                self.ends_meet(&merges, StationRef::new(kept, k), StationRef::new(other, o))
            })
            .collect();
        let pairing = *meeting.first()?;
        let (k, o) = pairing.ends(kept_shots, other_shots);
        let (kept_end, other_end) = (StationRef::new(kept, k), StationRef::new(other, o));
        let blocked = if meeting.len() > 1 {
            Some("The sections meet at both ends, so one section would be a ring".to_string())
        } else {
            self.combine_blocked(kept, other, kept_end, other_end, &merges)
        };
        Some(CombineCandidate {
            kept,
            other,
            pairing,
            reverse_other: pairing.reverses_other(),
            blocked,
            comment_conflict: differ(
                value_at(self, &self.station_comments, kept_end),
                value_at(self, &self.station_comments, other_end),
            ),
            name_conflict: differ(
                value_at(self, &self.station_names, kept_end),
                value_at(self, &self.station_names, other_end),
            ),
            offset_conflict: differ(
                value_at(self, &self.station_offsets, kept_end),
                value_at(self, &self.station_offsets, other_end),
            ),
            credit: self.combined_credit(kept, other, pairing),
        })
    }

    fn within_join_tolerance(&self, at: (f64, f64), station: StationRef) -> bool {
        self.compute_station_raw_xy(station)
            .is_some_and(|(x, y)| (x - at.0).hypot(y - at.1) <= JOIN_TOLERANCE_M)
    }

    fn ends_meet(&self, merges: &Adjacency, a: StationRef, b: StationRef) -> bool {
        reach(merges, a).contains(&b)
            || self
                .compute_station_raw_xy(a)
                .is_some_and(|at| self.within_join_tolerance(at, b))
    }

    /// Why combining at `kept_end`/`other_end` is refused, if it is.
    pub(super) fn combine_blocked(
        &self,
        kept: SectionId,
        other: SectionId,
        kept_end: StationRef,
        other_end: StationRef,
        merges: &Adjacency,
    ) -> Option<String> {
        let (Some(kept_section), Some(other_section)) =
            (self.get_section(kept), self.get_section(other))
        else {
            return Some(format!("Section {kept} or {other} no longer exists"));
        };
        let kept_uuids: HashSet<&str> = kept_section
            .shots
            .iter()
            .filter_map(|shot| shot.uuid.as_deref())
            .collect();
        if other_section
            .shots
            .iter()
            .filter_map(|shot| shot.uuid.as_deref())
            .any(|uuid| kept_uuids.contains(uuid))
        {
            return Some("The two sections share a shot identity".to_string());
        }
        if kept_section.azimuth_reference != other_section.azimuth_reference {
            return Some("The sections use different azimuth references".to_string());
        }
        if kept_section.quality.to_bits() != other_section.quality.to_bits() {
            return Some("The sections have different quality factors".to_string());
        }
        let total = kept_section.shots.len() + other_section.shots.len();
        let limit = Self::max_shots_per_section();
        if total > limit {
            return Some(format!(
                "The combined section would hold {total} shots; a section holds at most {limit}"
            ));
        }
        let joined = reach(merges, kept_end);
        if self.connected_elsewhere(kept, other, &joined, other_end) {
            return Some(
                "The sections already connect at another station, so the combined section \
                 would loop back on itself"
                    .to_string(),
            );
        }
        if !joined.contains(&other_end)
            && self.section_has_zero_datum(kept)
            && self.section_has_zero_datum(other)
        {
            return Some("Both sections already have a zero datum".to_string());
        }
        None
    }

    /// Whether the two sections are rigidly connected (by merges or aligned
    /// closures) other than through the station they are joined at.
    ///
    /// Ends that only lie together become rigidly linked by the combine, so any
    /// rigid path between the sections closes a loop. Ends that are already
    /// merged (directly or through a third section's station) are linked
    /// already; then every rigid edge not inside the joined station's merge
    /// group counts, so an aligned closure from the joined station into either
    /// section is a loop too.
    fn connected_elsewhere(
        &self,
        kept: SectionId,
        other: SectionId,
        joined: &HashSet<StationRef>,
        other_end: StationRef,
    ) -> bool {
        if !joined.contains(&other_end) {
            return self.get_rigid_connected_section_ids(kept).contains(&other);
        }
        let edges = self.rigid_edges();
        let slot: HashMap<SectionId, usize> = self
            .sections
            .iter()
            .enumerate()
            .map(|(index, section)| (section.id, index))
            .collect();
        let mut sets = UnionFind::new(self.sections.len());
        for (a, b) in edges {
            if joined.contains(&a) && joined.contains(&b) {
                continue;
            }
            if let (Some(&i), Some(&j)) = (slot.get(&a.section_id), slot.get(&b.section_id)) {
                sets.union(i, j);
            }
        }
        match (slot.get(&kept), slot.get(&other)) {
            (Some(&i), Some(&j)) => sets.find(i) == sets.find(j),
            _ => false,
        }
    }

    /// Every merge and aligned loop closure, both ends resolved.
    fn rigid_edges(&self) -> Vec<(StationRef, StationRef)> {
        let merges = self
            .station_merges
            .iter()
            .map(|m| (&m.station_a, &m.station_b));
        let closures = self
            .loop_closures
            .iter()
            .filter(|c| c.is_aligned)
            .map(|c| (&c.station_a, &c.station_b));
        merges
            .chain(closures)
            .filter_map(|(a, b)| Some((self.resolve_key(a)?, self.resolve_key(b)?)))
            .collect()
    }

    /// The combined section's credit summed by surveyor and date, in the order
    /// each pair first appears along it.
    fn combined_credit(
        &self,
        kept: SectionId,
        other: SectionId,
        pairing: CombinePairing,
    ) -> Vec<CreditRun> {
        let (Some(kept_section), Some(other_section)) =
            (self.get_section(kept), self.get_section(other))
        else {
            return Vec::new();
        };
        let kept_shots = shot_credits(kept_section);
        let mut other_shots = shot_credits(other_section);
        if pairing.reverses_other() {
            other_shots.reverse();
        }
        let ordered = if pairing.prepends() {
            other_shots.into_iter().chain(kept_shots)
        } else {
            kept_shots.into_iter().chain(other_shots)
        };
        let mut runs: Vec<(Option<String>, Option<NaiveDate>, f64)> = Vec::new();
        for (surveyor, date, length) in ordered {
            match runs
                .iter_mut()
                .find(|run| run.0 == surveyor && run.1 == date)
            {
                Some(run) => run.2 += length,
                None => runs.push((surveyor, date, length)),
            }
        }
        runs.into_iter()
            .map(|(surveyor, date, length_m)| CreditRun {
                surveyor,
                date: date.map(|d| d.format("%Y-%m-%d").to_string()),
                length_m,
            })
            .collect()
    }

    /// Leave one value of each kind on `kept_end` and none on `other_end`, per
    /// the module rules, so the re-key that follows has nothing to collide.
    fn settle_joined_station(
        &mut self,
        kept_end: StationRef,
        other_end: StationRef,
        options: &CombineOptions,
    ) {
        let Some(kept_key) = self.key_for_ref(kept_end) else {
            return;
        };

        let mut comments = std::mem::take(&mut self.station_comments);
        let kept_comment = take_at(self, &mut comments, kept_end);
        let other_comment = take_at(self, &mut comments, other_end);
        let comment = match (kept_comment, other_comment, options.comment_side) {
            (Some(kept), Some(other), SplitSide::Both) if kept != other => {
                Some(format!("{kept}\n{other}"))
            }
            (kept, other, side) => choose(kept, other, side),
        };
        if let Some(comment) = comment {
            comments.insert(kept_key.clone(), comment);
        }
        self.station_comments = comments;

        let mut names = std::mem::take(&mut self.station_names);
        let kept_name = take_at(self, &mut names, kept_end);
        let other_name = take_at(self, &mut names, other_end);
        if let Some(name) = choose(kept_name, other_name, options.name_side) {
            names.insert(kept_key.clone(), name);
        }
        self.station_names = names;

        let mut offsets = std::mem::take(&mut self.station_offsets);
        let kept_offset = take_at(self, &mut offsets, kept_end);
        let other_offset = take_at(self, &mut offsets, other_end);
        if let Some(offset) = choose(kept_offset, other_offset, options.offset_side) {
            offsets.insert(kept_key, offset);
        }
        self.station_offsets = offsets;

        let on = |project: &Project, key: &StationKey, at: StationRef| {
            project.resolve_key(key) == Some(at)
        };
        if self
            .zero_datums
            .iter()
            .any(|datum| on(self, &datum.station_ref, kept_end))
        {
            let keep: Vec<bool> = self
                .zero_datums
                .iter()
                .map(|datum| !on(self, &datum.station_ref, other_end))
                .collect();
            let mut keep = keep.into_iter();
            self.zero_datums.retain(|_| keep.next().unwrap_or(true));
        }
    }

    /// After the re-key: drop merges and loop closures touching `section` whose
    /// two ends are now one station (the join), and merges that repeat another.
    fn tidy_joined_relationships(&mut self, section: SectionId) {
        let mut seen: HashSet<(StationRef, StationRef)> = HashSet::new();
        let keep: Vec<bool> = self
            .station_merges
            .iter()
            .map(|m| {
                match (
                    self.resolve_key(&m.station_a),
                    self.resolve_key(&m.station_b),
                ) {
                    (Some(a), Some(b)) if a.section_id == section || b.section_id == section => {
                        a != b && seen.insert((a.min(b), a.max(b)))
                    }
                    _ => true,
                }
            })
            .collect();
        let mut keep = keep.into_iter();
        self.station_merges.retain(|_| keep.next().unwrap_or(true));

        let keep: Vec<bool> = self
            .loop_closures
            .iter()
            .map(|c| {
                match (
                    self.resolve_key(&c.station_a),
                    self.resolve_key(&c.station_b),
                ) {
                    (Some(a), Some(b)) => a != b || a.section_id != section,
                    _ => true,
                }
            })
            .collect();
        let mut keep = keep.into_iter();
        self.loop_closures.retain(|_| keep.next().unwrap_or(true));
    }
}
