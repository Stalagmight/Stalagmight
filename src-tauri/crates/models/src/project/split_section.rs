//! Split Section Here (#335): cut a section in two at a mid-section station
//! without deleting any shot.
//!
//! The split station is duplicated: the first half (which keeps the section's
//! id) ends on it and the second half starts on a co-located copy. The halves
//! are NOT merged to each other; each can move on its own afterwards. What sat
//! on the split station follows the #335 rules:
//! - merges there are copied to both copies, so whatever the station was
//!   merged to stays merged to both halves;
//! - loop closures there stay on the first half's end and are NOT copied: a
//!   copy double-counts the same physical loop in the least-squares
//!   adjustment, and when the copy is the second half's only tie,
//!   `convert_orphan_closures_to_merges` promotes it to a rigid merge and
//!   moves the second half by the loop's misclosure;
//! - its name and manual position offset go to both copies (one physical
//!   station);
//! - its comment goes to the half the caller picks, or both;
//! - its zero datum goes to the half the caller picks.
//!
//! Depth: the copies are independent stations, so the first half's end reads
//! `depth_out` of the shot before the cut and the second half's start reads
//! `depth_in` of the shot after it, where the unsplit station averaged the
//! two. Every other station keeps its depth, and every station keeps its xy.
//!
//! Gauge: the second half inherits the section's `reverse_parity`, and the
//! least-squares gauge stays on the same physical station: the half that holds
//! it gets a pair naming it (`keep_gauge_on_station`), re-captured when the
//! cut took the old pair's neighbour shot. A split ON the gauge station pins
//! both copies.
//!
//! Credit: survey credit (surveyors, explorers, date) lives on every shot, so
//! each shot keeps its own exactly; the second half is built with no legacy
//! section template, which would otherwise be applied to its uncredited shots.

use std::collections::HashMap;

use chrono::Utc;

use crate::{
    Project, SectionData, SectionId, Shot, ShotData, SplitSectionOptions, SplitSide, StationKey,
    StationMerge, StationRef,
};

use super::gauge::{captured_gauge, GaugePair};

impl Project {
    /// Split `section_id` at station `station_index`, which must be a
    /// mid-section station (not the first or last). Returns the new second
    /// half's section id. Every station keeps its measured position.
    ///
    /// Validated before anything mutates, so an `Err` leaves the project as it
    /// was.
    pub fn split_section_at_station(
        &mut self,
        section_id: SectionId,
        station_index: u32,
        options: &SplitSectionOptions,
    ) -> Result<SectionId, String> {
        if options.datum_side == SplitSide::Both {
            return Err("A zero datum can stay on only one half of a split".to_string());
        }
        let section = self
            .get_section(section_id)
            .ok_or_else(|| format!("Section {section_id} not found"))?;
        let shot_count = section.shots.len();
        let k = station_index as usize;
        if k == 0 || k >= shot_count {
            return Err(format!(
                "Station {station_index} is the first or last station of section \
                 {section_id}; split at a station in the middle"
            ));
        }

        let direction = section.direction;
        let quality = section.quality;
        let azimuth_reference = section.azimuth_reference;
        let reverse_parity = section.reverse_parity;
        let gauge_station = Self::gauge_index(section);
        let gauge_capture = captured_gauge(section);
        let tail: Vec<ShotData> = section.shots[k..].iter().map(Shot::to_shot_data).collect();

        // The second half starts where the first ends: walk the first k shots
        // from the section's start, exactly as the coordinate solver does.
        let resolved_reference = self.azimuth_reference_for_id(section_id);
        let (mut dx, mut dy) = (0.0, 0.0);
        for shot in &section.shots[..k] {
            let azimuth = shot.average_azimuth_true_rad(resolved_reference);
            let horizontal = shot.horizontal_length();
            dx += horizontal * azimuth.sin();
            dy += horizontal * azimuth.cos();
        }
        let (ox, oy) = self.section_offset(section_id);

        // Resolve every stored cross-reference into this section BEFORE it
        // changes: a key cannot follow a shot into another section.
        let stored = self.resolve_stored_keys_in(&[section_id]);

        let first = self
            .get_section_mut(section_id)
            .ok_or_else(|| format!("Section {section_id} not found"))?;
        first.name = Some(options.name_a.clone());
        first.color.clone_from(&options.color_a);
        first.shots.truncate(k);

        let second_id = self
            .add_section(SectionData {
                name: Some(options.name_b.clone()),
                // No legacy template: `Section::new` would give it to every
                // tail shot whose credit is empty, crediting shots nobody
                // credited.
                surveyor: None,
                date: None,
                color: options.color_b.clone(),
                direction: Some(direction),
                quality,
                shots: tail,
            })
            .id;
        if let Some(second) = self.get_section_mut(second_id) {
            second.azimuth_reference = azimuth_reference;
            second.reverse_parity = reverse_parity;
        }
        self.set_section_offset(second_id, (ox + dx, oy + dy));
        self.reindex_section_shot_uuids(section_id);
        self.reindex_section_shot_uuids(second_id);

        // Stations past the split move to the second half; the split station
        // itself stays the first half's end.
        let homes: HashMap<StationKey, StationRef> = stored
            .into_iter()
            .filter(|(_, at)| at.station_index > station_index)
            .map(|(key, at)| {
                let home = StationRef::new(second_id, at.station_index - station_index);
                (key, home)
            })
            .collect();
        self.rekey_station_refs(&homes, "split section");

        self.duplicate_split_station(
            StationRef::new(section_id, station_index),
            StationRef::new(second_id, 0),
            options,
        );
        self.keep_split_gauge(
            section_id,
            second_id,
            k,
            gauge_station,
            gauge_capture.as_ref(),
        );
        Ok(second_id)
    }

    /// Keep the gauge on the physical station `gauge` (an index into the
    /// unsplit section, whose captured pair was `capture`) after a split at
    /// station `k`.
    fn keep_split_gauge(
        &mut self,
        first: SectionId,
        second: SectionId,
        k: usize,
        gauge: usize,
        capture: Option<&GaugePair>,
    ) {
        if gauge <= k {
            self.keep_gauge_on_station(first, gauge, capture);
        } else {
            self.clear_unresolvable_gauge(first);
        }
        if gauge >= k {
            self.keep_gauge_on_station(second, gauge - k, capture);
        }
    }

    /// Give the second half's start (`copy`) what the split station (`end`)
    /// holds, per the module rules. Compared through `resolve_key`, so a stored
    /// key in any anchor form for the same station matches.
    fn duplicate_split_station(
        &mut self,
        end: StationRef,
        copy: StationRef,
        options: &SplitSectionOptions,
    ) {
        let (Some(end_key), Some(copy_key)) = (self.key_for_ref(end), self.key_for_ref(copy))
        else {
            return;
        };
        let is_end = |project: &Project, key: &StationKey| project.resolve_key(key) == Some(end);

        let merge_copies: Vec<StationMerge> = self
            .station_merges
            .iter()
            .filter_map(|m| {
                let (a, b) = (is_end(self, &m.station_a), is_end(self, &m.station_b));
                match (a, b) {
                    (true, false) => Some((copy_key.clone(), m.station_b.clone())),
                    (false, true) => Some((m.station_a.clone(), copy_key.clone())),
                    _ => None,
                }
            })
            .map(|(station_a, station_b)| StationMerge {
                station_a,
                station_b,
                merged_at: Utc::now(),
            })
            .collect();
        for merge in merge_copies {
            self.record_station_merge(merge);
        }

        if let Some(name) = self.station_names.get(&end_key).cloned() {
            self.station_names.insert(copy_key.clone(), name);
        }
        if let Some(offset) = self.station_offsets.get(&end_key).copied() {
            self.station_offsets.insert(copy_key.clone(), offset);
        }
        match (
            options.comment_side,
            self.station_comments.get(&end_key).cloned(),
        ) {
            (SplitSide::A, _) | (_, None) => {}
            (SplitSide::B, Some(comment)) => {
                self.station_comments.remove(&end_key);
                self.station_comments.insert(copy_key.clone(), comment);
            }
            (SplitSide::Both, Some(comment)) => {
                self.station_comments.insert(copy_key.clone(), comment);
            }
        }
        if options.datum_side == SplitSide::B {
            for datum in &mut self.zero_datums {
                if datum.station_ref == end_key {
                    datum.station_ref = copy_key.clone();
                }
            }
        }
    }
}
