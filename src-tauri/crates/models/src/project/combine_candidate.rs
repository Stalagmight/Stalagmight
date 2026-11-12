//! Combine Sections, the read-only half: can two sections be joined into one,
//! and where. The context menu, the Sections panel and the dialog ask this, and
//! `combine_sections` asks it again before it changes anything.
//!
//! Two sections are a candidate when an end station of each is the same point:
//! - **merged**: the two end stations are station-merged, and the merge group
//!   is EXACTLY those two stations (a third station there is a branch);
//! - **co-located**: unmerged, both stations' merge groups are empty, their
//!   chain positions (section offset plus the shot walk, without the stations'
//!   manual offsets) agree within [`COMBINE_TOLERANCE_M`] horizontally, and no
//!   third section's end sits there too. Depth is never compared: the two
//!   stations read the depth of different shots (the state a fresh split
//!   leaves).
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

/// One way two sections can meet: an end of each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Pairing {
    pub a: SectionId,
    pub end_a: SectionEnd,
    pub b: SectionId,
    pub end_b: SectionEnd,
}

/// What a touching pairing looks like.
#[derive(Debug, Clone)]
struct Touch {
    merged: bool,
    blocked: Option<String>,
}

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
pub(crate) struct EndTable {
    ends: Vec<(SectionId, SectionEnd, (f64, f64))>,
    by_end: HashMap<(SectionId, SectionEnd), (f64, f64)>,
}

#[cfg(test)]
thread_local! {
    /// How many [`EndTable`]s this thread built: a query must build one.
    pub(crate) static END_TABLE_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl EndTable {
    fn at(&self, id: SectionId, end: SectionEnd) -> Option<(f64, f64)> {
        self.by_end.get(&(id, end)).copied()
    }

    /// Every section end within the tolerance of `point`.
    fn near(&self, point: (f64, f64)) -> impl Iterator<Item = (SectionId, SectionEnd)> + '_ {
        self.ends
            .iter()
            .filter(move |(_, _, p)| distance(*p, point) <= COMBINE_TOLERANCE_M)
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
        let by_end = ends.iter().map(|(id, end, p)| ((*id, *end), *p)).collect();
        EndTable { ends, by_end }
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
        let blocked = if merged {
            (group_a.len() > 2).then(|| {
                format!(
                    "{} stations are merged at this point, so it is a branch; only two \
                     sections that meet end to end can be combined",
                    group_a.len()
                )
            })
        } else if group_a.len() > 1 || self.get_merge_group(at_b).len() > 1 {
            Some(
                "Another station is merged at this point, so it is a branch; only two \
                 sections that meet end to end can be combined"
                    .to_string(),
            )
        } else {
            let third = ends
                .near(pa)
                .map(|(id, _)| id)
                .find(|id| *id != pairing.a && *id != pairing.b);
            third.map(|third| {
                format!(
                    "{} also ends at this point, so it is a branch; only two sections that \
                     meet end to end can be combined",
                    self.get_section(third)
                        .map_or_else(|| format!("Section {third}"), label)
                )
            })
        }
        .or_else(|| self.combine_blocked_reason(pairing, at_a, at_b, merged))
        .or_else(|| {
            // A merge the offset solver could not align: combining would move
            // the other section by the gap.
            (apart > COMBINE_TOLERANCE_M).then(|| {
                format!(
                    "The merged join stations are {:.1} mm apart; realign the merge first",
                    apart * 1000.0
                )
            })
        });
        Some(Touch { merged, blocked })
    }

    /// Why combining at a touching `pairing` would be wrong, or `None`.
    fn combine_blocked_reason(
        &self,
        pairing: Pairing,
        at_a: StationRef,
        at_b: StationRef,
        merged: bool,
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
                "{name_a} and {name_b} share a shot identity (one shot uuid in both), so the \
                 combined section would hold the same shot twice"
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
                "{name_a} and {name_b} have different quality factors, which weight the \
                 loop-closure adjustment differently"
            ));
        }
        if self.rigidly_connected_beyond_join(at_a, at_b) {
            return Some(format!(
                "{name_a} and {name_b} are also connected elsewhere (directly or through \
                 other sections), so the combined section would close a loop onto itself"
            ));
        }
        if self.has_datum_at(at_a) && self.has_datum_at(at_b) {
            return Some("Both join stations carry a zero datum".to_string());
        }
        if !merged
            && self.section_has_zero_datum(pairing.a)
            && self.section_has_zero_datum(pairing.b)
        {
            return Some(format!(
                "{name_a} and {name_b} are each anchored by a zero datum; joining them \
                 would over-constrain one rigid group"
            ));
        }
        let offset = |at| self.station_offset_for_ref(at).unwrap_or_default();
        let (oa, ob) = (offset(at_a), offset(at_b));
        let moved = ((oa.0 - ob.0).powi(2) + (oa.1 - ob.1).powi(2) + (oa.2 - ob.2).powi(2)).sqrt();
        if moved > COMBINE_TOLERANCE_M {
            return Some(
                "The two join stations were moved by different amounts; move one onto the \
                 other first"
                    .to_string(),
            );
        }
        let budget = FEATURE_ID_STRIDE.saturating_sub(1) as usize;
        if a.shots.len() + b.shots.len() > budget {
            return Some(format!(
                "The combined section would have {} shots; a section holds at most {budget}",
                a.shots.len() + b.shots.len()
            ));
        }
        None
    }

    /// Why `kept` cannot take `other`'s shots: they get ids from
    /// `kept.next_shot_id`, which only ever grows within a session, and must
    /// stay inside `kept`'s id window (`kept.id * FEATURE_ID_STRIDE ..`) or they
    /// collide with the next section's ids. Not renumbered in place: position-
    /// keyed ignored warnings and the frontend's selection, hover and table
    /// state address shots by id. A reload renumbers every section compactly.
    pub(crate) fn shot_id_room_refusal(kept: &Section, other: &Section) -> Option<String> {
        let window_end = u64::from(kept.id + 1) * u64::from(FEATURE_ID_STRIDE);
        let last = u64::from(kept.next_shot_id) + other.shots.len() as u64;
        (last > window_end).then(|| {
            format!(
                "{} has used up its shot numbers for this session, so {}'s shots would take \
                 another section's numbers. Keep {} instead, or save and reopen the project \
                 (which renumbers every shot) and combine again",
                label(kept),
                label(other),
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
                "{name_a} and {name_b} use different north references, so one section's \
                 bearings would change meaning in the other"
            )),
            (None, Some(_)) => Some(format!(
                "{name_a} follows the project's default north reference and {name_b} declares \
                 its own; set the same reference on both first"
            )),
            (Some(_), None) => Some(format!(
                "{name_b} follows the project's default north reference and {name_a} declares \
                 its own; set the same reference on both first"
            )),
        }
    }

    /// Are the join stations' sections still rigidly connected (merges and
    /// aligned closures) once every tie BETWEEN the two join stations is
    /// removed? Then the combined section would be tied to itself: a rigid
    /// loop inside one section, or through a third section (which
    /// `merge_stations` refuses for the same reason).
    fn rigidly_connected_beyond_join(&self, at_a: StationRef, at_b: StationRef) -> bool {
        let is_join = |x: Option<StationRef>, y: Option<StationRef>| {
            (x == Some(at_a) && y == Some(at_b)) || (x == Some(at_b) && y == Some(at_a))
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
                    "{} and {} meet at both ends; right-click the end station where they \
                     should be combined",
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
                    merged: false,
                    blocked: Some(format!(
                        "{} and {} do not meet at an end station",
                        label(a),
                        label(b)
                    )),
                };
                self.candidate_for(pairing, touch)
            }
        }
    }

    /// Can the section that `station` ends combine with another section ending
    /// at the same point? `None` when `station` is not an end of its section,
    /// or no other section touches it (nothing to offer). A branch, or a
    /// partner that is blocked, is `Some` with the reason.
    pub fn combine_candidate(&self, station: StationRef) -> Option<CombineCandidate> {
        let section = self.get_section(station.section_id)?;
        let end = if station.station_index == 0 {
            SectionEnd::Start
        } else if station.station_index as usize == section.shots.len() {
            SectionEnd::End
        } else {
            return None;
        };
        // Every other section with an end at this point: merged to it, or
        // co-located with it.
        let table = self.end_table();
        let at = table.at(section.id, end)?;
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
        let partners: Vec<(Pairing, Touch)> = other_ends
            .into_iter()
            .filter_map(|(id, other_end)| {
                let pairing = Pairing {
                    a: section.id,
                    end_a: end,
                    b: id,
                    end_b: other_end,
                };
                self.pairing_touch(pairing, &table)
                    .map(|touch| (pairing, touch))
            })
            .collect();
        match partners.as_slice() {
            [] if group.len() <= 1 => None,
            [] => {
                // Merged to a station that is not a section end: a branch.
                let other = group.iter().find(|s| **s != station)?;
                let pairing = Pairing {
                    a: section.id,
                    end_a: end,
                    b: other.section_id,
                    end_b: SectionEnd::Start,
                };
                let touch = Touch {
                    merged: true,
                    blocked: Some(
                        "This station is merged to the middle of another section, so it is \
                         a branch; only two sections that meet end to end can be combined"
                            .to_string(),
                    ),
                };
                self.candidate_for(pairing, touch)
            }
            [(pairing, touch)] => {
                let mut touch = touch.clone();
                if touch.blocked.is_none() {
                    // The pair's own answer decides, so the station menu, the
                    // Sections panel and the command agree: two sections that
                    // meet at both ends are ambiguous from either. When it is
                    // eligible it is this pairing, the only eligible one.
                    touch.blocked = self
                        .candidate_for_sections_with(pairing.a, pairing.b, &table)?
                        .blocked_reason;
                }
                self.candidate_for(*pairing, touch)
            }
            [(pairing, touch), ..] => {
                let mut touch = touch.clone();
                touch.blocked = Some(format!(
                    "{} section ends meet at this point, so it is a branch; only two \
                     sections that meet end to end can be combined",
                    partners.len() + 1
                ));
                self.candidate_for(*pairing, touch)
            }
        }
    }
}
