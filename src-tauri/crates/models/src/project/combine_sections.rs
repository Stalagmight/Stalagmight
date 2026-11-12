//! Combine Sections: join two sections that meet end to end into one, the
//! inverse of Split Section Here.
//!
//! The KEPT section keeps its id, its shot ids, its direction, its station 0
//! orientation and its exact position. The OTHER section is reversed first when
//! the two meet end-to-end or start-to-start (`reverse_section`, which
//! reciprocates bearings, swaps depths, negates pitch and optionally swaps left
//! and right), then its shots are appended to the kept section (kept end meets
//! other start) or prepended (kept start meets other end). Every per-shot field
//! and uuid travels; the other section's shots get new ids from the kept
//! section's counter.
//!
//! The two join stations become ONE station:
//! - the join merge, and a zero-length closure between the two ends, go;
//! - a comment or name both carry is kept once if identical, otherwise per
//!   the dialog's choice (kept, other, or both joined);
//! - the kept station's manual offset (or none) wins, the other's is dropped
//!   (the candidate check bounds the difference by the tolerance);
//! - a zero datum on either (never both) and closures to other stations move
//!   onto the joined station.
//!
//! Every other stored station reference moves with its station, the gauge
//! stays on the same physical station (the lower section id's gauge, which is
//! the one the component used), and the other section's id is retired with its
//! parity relation for dive-plan pins (`RetiredSection`).
//!
//! Credit: survey credit (surveyors, explorers, date) lives on every shot and
//! travels with it, through the reverse too, so every shot keeps its own
//! exactly and the combined section's credit is simply the union.

use std::collections::{HashMap, HashSet};

use crate::{
    CombinePick, CombineSectionsOptions, RetiredSection, SectionEnd, SectionId, Shot, ShotData,
    StationKey, StationRef,
};

use super::gauge::captured_gauge;
use super::reverse::shot_run_displacement;
use super::Project;

/// The joined station's text, from the kept and the other join station's.
fn pick_text(
    kept: Option<String>,
    other: Option<String>,
    pick: CombinePick,
    separator: &str,
) -> Option<String> {
    match (kept, other) {
        (Some(kept), Some(other)) if kept == other => Some(kept),
        (Some(kept), Some(other)) => Some(match pick {
            CombinePick::Kept => kept,
            CombinePick::Other => other,
            CombinePick::Both => format!("{kept}{separator}{other}"),
        }),
        (kept, other) => kept.or(other),
    }
}

impl Project {
    /// Where a section id's shots live now, following the sections Combine
    /// Sections retired: `(id, parity_flip)`, `id` unchanged and `false` for a
    /// section that was never retired. Bounded, so a corrupt map with a cycle
    /// cannot loop.
    pub fn resolve_retired_section(&self, id: SectionId) -> (SectionId, bool) {
        let (mut id, mut flip) = (id, false);
        for _ in 0..=self.retired_sections.len() {
            let Some(retired) = self.retired_sections.get(&id) else {
                break;
            };
            flip ^= retired.parity_flip;
            id = retired.into;
        }
        (id, flip)
    }

    /// Combine `other` into `kept`. Re-runs the candidate check first, so an
    /// `Err` (the blocked reason) leaves the project exactly as it was.
    pub fn combine_sections(
        &mut self,
        kept: SectionId,
        other: SectionId,
        options: &CombineSectionsOptions,
    ) -> Result<(), String> {
        let candidate = self
            .combine_candidate_for_sections(kept, other)
            .ok_or_else(|| format!("Sections {kept} and {other} cannot be combined"))?;
        if let Some(reason) = candidate.blocked_reason {
            return Err(reason);
        }
        let (kept_end, other_end) = if candidate.section_a == kept {
            (candidate.end_a, candidate.end_b)
        } else {
            (candidate.end_b, candidate.end_a)
        };
        if kept_end == other_end && !self.reverse_section(other, options.swap_lr) {
            return Err(format!("Section {other} not found"));
        }
        // After the reverse the other section meets the kept one start to end.
        let append = kept_end == SectionEnd::End;

        let (Some(k), Some(o)) = (self.get_section(kept), self.get_section(other)) else {
            return Err(format!("Sections {kept} and {other} not found"));
        };
        let (nk, no) = (k.shots.len() as u32, o.shots.len() as u32);
        let join_k = StationRef::new(kept, if append { nk } else { 0 });
        let join_o = StationRef::new(other, if append { 0 } else { no });
        let other_shots: Vec<ShotData> = o.shots.iter().map(Shot::to_shot_data).collect();
        let parity_flip = k.reverse_parity != o.reverse_parity;
        let reference = self.azimuth_reference_for(k);
        // The gauge the component used is the lower section id's (the rule in
        // `default_anchor_for_component`); keep that physical station.
        let source = if kept < other { k } else { o };
        let gauge = Self::gauge_index(source) as u32;
        let gauge = match (source.id == kept, append) {
            (true, true) | (false, false) => gauge,
            (true, false) => gauge + no,
            (false, true) => nk + gauge,
        };
        let gauge_capture = captured_gauge(source);

        // Settle the join first (it touches keyed entries only, never the
        // structure), then resolve every stored key against the structure as
        // it still is, so the kept join station's freshly written key is
        // re-keyed with the rest.
        self.resolve_combine_join(join_k, join_o, options);
        let stored = self.resolve_stored_keys_in(&[kept, other]);

        // ---- The shots ------------------------------------------------------
        self.remove_section(other);
        self.remove_section_offset(other);
        self.rebuild_section_index();
        let section = self
            .get_section_mut(kept)
            .ok_or_else(|| format!("Section {kept} not found"))?;
        for data in other_shots.iter().cloned() {
            section.add_shot(data);
        }
        if !append {
            section.shots.rotate_right(no as usize);
        }
        // An empty name leaves the section unnamed, as it may have been.
        section.name = Some(options.name.clone()).filter(|name| !name.trim().is_empty());
        section.color.clone_from(&options.color);
        self.reindex_section_shot_uuids(kept);
        self.reindex_section_shot_uuids(other);
        if !append {
            // Station 0 is now the other section's start: move the anchor back
            // by the other section's span so the kept stations stay put.
            let (dx, dy) = shot_run_displacement(&other_shots, reference);
            if dx.is_finite() && dy.is_finite() {
                self.adjust_section_offset(kept, -dx, -dy);
            }
        }

        // ---- Every stored reference follows its station --------------------
        let homes: HashMap<StationKey, StationRef> = stored
            .into_iter()
            .filter_map(|(key, at)| {
                let index = if at.section_id == other {
                    if append {
                        nk + at.station_index
                    } else {
                        at.station_index
                    }
                } else if append {
                    return None;
                } else {
                    at.station_index + no
                };
                Some((key, StationRef::new(kept, index)))
            })
            .collect();
        let unmoved = self.rekey_station_refs(&homes, "combine sections");
        self.dedupe_closures_in(kept);
        self.keep_gauge_on_station(kept, gauge as usize, gauge_capture.as_ref());
        self.retired_sections.insert(
            other,
            RetiredSection {
                into: kept,
                parity_flip,
            },
        );
        if cfg!(debug_assertions) {
            self.for_each_station_ref(|label, key| {
                debug_assert!(
                    key.section_id != other || unmoved.contains(&key),
                    "combine sections: a {label} still names the retired section {other}"
                );
            });
        }
        self.touch();
        Ok(())
    }

    /// Make the two join stations one, while the structure is still the old
    /// one: drop the ties between them and settle the keyed texts and offsets
    /// onto the kept join station's key (the re-key afterwards moves that key
    /// when the kept station's anchor changes).
    fn resolve_combine_join(
        &mut self,
        join_k: StationRef,
        join_o: StationRef,
        options: &CombineSectionsOptions,
    ) {
        let between = |project: &Project, x: &StationKey, y: &StationKey| {
            let (x, y) = (project.resolve_key(x), project.resolve_key(y));
            (x == Some(join_k) && y == Some(join_o)) || (x == Some(join_o) && y == Some(join_k))
        };
        let merges: HashSet<usize> = self
            .station_merges
            .iter()
            .enumerate()
            .filter(|(_, m)| between(self, &m.station_a, &m.station_b))
            .map(|(i, _)| i)
            .collect();
        let mut index = 0;
        self.station_merges.retain(|_| {
            index += 1;
            !merges.contains(&(index - 1))
        });
        let closures: HashSet<u64> = self
            .loop_closures
            .iter()
            .filter(|c| between(self, &c.station_a, &c.station_b))
            .map(|c| c.id)
            .collect();
        self.loop_closures.retain(|c| !closures.contains(&c.id));

        let Some(key) = self.key_for_ref(join_k) else {
            return;
        };
        let comment_k = take_at(self, Field::Comments, join_k);
        let comment_o = take_at(self, Field::Comments, join_o);
        if let Some(text) = pick_text(comment_k, comment_o, options.comment_pick, "\n") {
            self.station_comments.insert(key.clone(), text);
        }
        let name_k = take_at(self, Field::Names, join_k);
        let name_o = take_at(self, Field::Names, join_o);
        if let Some(text) = pick_text(name_k, name_o, options.name_pick, " / ") {
            self.station_names.insert(key.clone(), text);
        }
        // The kept station's own offset (or none) wins, so the kept section
        // stays exactly in place; the candidate check bounds the difference.
        let offset_k = take_offset_at(self, join_k);
        take_offset_at(self, join_o);
        if let Some(offset) = offset_k {
            self.station_offsets.insert(key, offset);
        }
    }

    /// Drop loop closures inside `section_id` that the combine made identical
    /// (same two stations), keeping the first: a duplicate would count the
    /// same loop twice in the adjustment.
    fn dedupe_closures_in(&mut self, section_id: SectionId) {
        let mut seen: HashSet<(StationRef, StationRef)> = HashSet::new();
        let duplicates: HashSet<u64> = self
            .loop_closures
            .iter()
            .filter(|c| {
                c.station_a.section_id == section_id || c.station_b.section_id == section_id
            })
            .filter_map(|c| {
                let (a, b) = (
                    self.resolve_key(&c.station_a)?,
                    self.resolve_key(&c.station_b)?,
                );
                let pair = if a <= b { (a, b) } else { (b, a) };
                (!seen.insert(pair)).then_some(c.id)
            })
            .collect();
        if !duplicates.is_empty() {
            self.loop_closures.retain(|c| !duplicates.contains(&c.id));
        }
    }
}

#[derive(Clone, Copy)]
enum Field {
    Comments,
    Names,
}

/// Remove every entry of a keyed text collection that names `station`,
/// returning the first one's text.
fn take_at(project: &mut Project, field: Field, station: StationRef) -> Option<String> {
    let map = match field {
        Field::Comments => &project.station_comments,
        Field::Names => &project.station_names,
    };
    let keys: Vec<StationKey> = map
        .keys()
        .filter(|key| project.resolve_key(key) == Some(station))
        .cloned()
        .collect();
    let map = match field {
        Field::Comments => &mut project.station_comments,
        Field::Names => &mut project.station_names,
    };
    let texts: Vec<String> = keys.iter().filter_map(|key| map.remove(key)).collect();
    texts.into_iter().next()
}

/// Remove every station offset that names `station`, returning the first.
fn take_offset_at(project: &mut Project, station: StationRef) -> Option<(f64, f64, f64)> {
    let keys: Vec<StationKey> = project
        .station_offsets
        .keys()
        .filter(|key| project.resolve_key(key) == Some(station))
        .cloned()
        .collect();
    let offsets: Vec<(f64, f64, f64)> = keys
        .iter()
        .filter_map(|key| project.station_offsets.remove(key))
        .collect();
    offsets.into_iter().next()
}
