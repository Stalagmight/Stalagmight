//! Split Section Here (#335): cut a section in two at a mid-section station
//! without deleting any shot.
//!
//! The split station is duplicated: the first half (which keeps the section's
//! id) ends on it and the second half starts on a co-located copy. The halves
//! are NOT merged to each other; each can move on its own afterwards. What sat
//! on the split station follows the #335 rules:
//! - merges and loop closures there are copied to both copies, so whatever the
//!   station was tied to stays tied to both halves;
//! - its name and manual position offset go to both copies (one physical
//!   station);
//! - its comment goes to the half the caller picks, or both;
//! - its zero datum goes to the half the caller picks.

use chrono::Utc;

use crate::{
    Project, SectionData, SectionId, Shot, ShotData, SplitSectionOptions, SplitSide, StationKey,
    StationMerge, StationRef,
};

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

        let surveyor = section.surveyor.clone();
        let date = section.date;
        let direction = section.direction;
        let quality = section.quality;
        let azimuth_reference = section.azimuth_reference;
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

        // Resolve every stored cross-reference to its position BEFORE the
        // section changes; a key cannot follow a shot into another section.
        let old_refs: Vec<Option<StationRef>> = {
            let mut out = Vec::new();
            self.for_each_station_ref(|_label, key| out.push(self.resolve_key(&key)));
            out
        };

        let first = self
            .get_section_mut(section_id)
            .ok_or_else(|| format!("Section {section_id} not found"))?;
        first.name = Some(options.name_a.clone());
        first.color = options.color_a.clone();
        first.shots.truncate(k);

        let second_id = self
            .add_section(SectionData {
                name: Some(options.name_b.clone()),
                surveyor,
                date,
                color: options.color_b.clone(),
                direction: Some(direction),
                quality,
                shots: tail,
            })
            .id;
        if let Some(second) = self.get_section_mut(second_id) {
            second.azimuth_reference = azimuth_reference;
        }
        self.set_section_offset(second_id, (ox + dx, oy + dy));
        self.reindex_section_shot_uuids(section_id);
        self.reindex_section_shot_uuids(second_id);

        // Stations past the split move to the second half; the split station
        // itself stays the first half's end. The rebuilt keys are consumed in
        // the order `for_each_station_ref` produced them.
        let new_keys: Vec<Option<StationKey>> = old_refs
            .iter()
            .map(|old| {
                let sr = (*old)?;
                let moved = sr.section_id == section_id && sr.station_index > station_index;
                let new_ref = if moved {
                    StationRef::new(second_id, sr.station_index - station_index)
                } else {
                    sr
                };
                self.key_for_ref(new_ref)
            })
            .collect();
        let mut next = 0usize;
        self.map_station_refs(|old_key| {
            let key = new_keys.get(next).cloned().flatten();
            next += 1;
            key.unwrap_or(old_key)
        });
        debug_assert_eq!(
            next,
            new_keys.len(),
            "split-section remap visited a different number of refs than it captured"
        );

        self.duplicate_split_station(
            StationRef::new(section_id, station_index),
            StationRef::new(second_id, 0),
            options,
        );

        for id in [section_id, second_id] {
            if let Some(section) = self.get_section_mut(id) {
                section.fold_credit();
            }
        }
        Ok(second_id)
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
            self.station_merges.push(merge);
        }

        let closure_copies: Vec<_> = self
            .loop_closures
            .iter()
            .filter_map(|c| {
                let (a, b) = (is_end(self, &c.station_a), is_end(self, &c.station_b));
                let mut copied = c.clone();
                match (a, b) {
                    (true, false) => copied.station_a = copy_key.clone(),
                    (false, true) => copied.station_b = copy_key.clone(),
                    _ => return None,
                }
                Some(copied)
            })
            .collect();
        for mut closure in closure_copies {
            closure.id = self.next_closure_id;
            self.next_closure_id += 1;
            self.loop_closures.push(closure);
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

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::float_cmp,
    reason = "test module: panics on a failed assumption are the point"
)]
mod tests {
    use chrono::NaiveDate;

    use crate::{
        GpsCoordinates, Project, SectionData, SectionId, ShotCredit, ShotData, SplitSectionOptions,
        SplitSide, StationRef,
    };

    fn options(comment_side: SplitSide, datum_side: SplitSide) -> SplitSectionOptions {
        SplitSectionOptions {
            name_a: "Main-A".to_string(),
            name_b: "Main-B".to_string(),
            color_a: "#111111".to_string(),
            color_b: "#222222".to_string(),
            comment_side,
            datum_side,
        }
    }

    fn shot(azimuth: f64, length: f64) -> ShotData {
        ShotData {
            azimuth_in: azimuth,
            azimuth_out: azimuth,
            length,
            ..Default::default()
        }
    }

    /// "Main": four shots bending east then north, plus a side section whose
    /// start is merged to Main's station 2.
    fn project() -> (Project, SectionId, SectionId) {
        let mut project = Project::new("Split".to_string(), None);
        let main = project
            .add_section(SectionData {
                name: Some("Main".to_string()),
                surveyor: Some("Bob".to_string()),
                shots: vec![
                    shot(90.0, 10.0),
                    shot(90.0, 5.0),
                    shot(0.0, 7.0),
                    shot(45.0, 3.0),
                ],
                ..Default::default()
            })
            .id;
        let side = project
            .add_section(SectionData {
                name: Some("Side".to_string()),
                shots: vec![shot(180.0, 4.0)],
                ..Default::default()
            })
            .id;
        project
            .merge_stations(StationRef::new(main, 2), StationRef::new(side, 0))
            .unwrap();
        (project, main, side)
    }

    fn xy(project: &Project, section: SectionId, index: u32) -> (f64, f64) {
        let c = project
            .compute_station_raw_coords(StationRef::new(section, index))
            .unwrap();
        ((c.x * 1e6).round() / 1e6, (c.y * 1e6).round() / 1e6)
    }

    #[test]
    fn every_station_keeps_its_position_across_the_split() {
        let (mut project, main, _) = project();
        let before: Vec<_> = (0..=4).map(|i| xy(&project, main, i)).collect();

        let second = project
            .split_section_at_station(main, 2, &options(SplitSide::A, SplitSide::A))
            .unwrap();

        assert_eq!(project.get_section(main).unwrap().shots.len(), 2);
        assert_eq!(project.get_section(second).unwrap().shots.len(), 2);
        let after: Vec<_> = (0..=2)
            .map(|i| xy(&project, main, i))
            .chain((1..=2).map(|i| xy(&project, second, i)))
            .collect();
        assert_eq!(after, before);
        assert_eq!(xy(&project, second, 0), before[2], "the copy is co-located");
    }

    #[test]
    fn the_first_and_last_station_cannot_be_split_and_nothing_changes() {
        let (mut project, main, _) = project();
        let sections = project.sections.len();
        for index in [0, 4] {
            assert!(project
                .split_section_at_station(main, index, &options(SplitSide::A, SplitSide::A))
                .is_err());
        }
        assert!(project
            .split_section_at_station(main, 2, &options(SplitSide::A, SplitSide::Both))
            .is_err());
        assert_eq!(project.sections.len(), sections);
        assert_eq!(project.get_section(main).unwrap().shots.len(), 4);
    }

    #[test]
    fn a_reference_past_the_split_follows_its_station_into_the_second_half() {
        let (mut project, main, _) = project();
        let key = project.key_for_ref(StationRef::new(main, 3)).unwrap();
        project
            .station_comments
            .insert(key, "past the split".to_string());

        let second = project
            .split_section_at_station(main, 2, &options(SplitSide::A, SplitSide::A))
            .unwrap();

        let key = project.key_for_ref(StationRef::new(second, 1)).unwrap();
        assert_eq!(
            project.station_comments.get(&key).map(String::as_str),
            Some("past the split")
        );
    }

    #[test]
    fn a_merge_on_the_split_station_ties_both_halves() {
        let (mut project, main, side) = project();
        let second = project
            .split_section_at_station(main, 2, &options(SplitSide::A, SplitSide::A))
            .unwrap();

        let tied: Vec<(StationRef, StationRef)> = project
            .station_merges
            .iter()
            .map(|m| {
                (
                    project.resolve_key(&m.station_a).unwrap(),
                    project.resolve_key(&m.station_b).unwrap(),
                )
            })
            .collect();
        let side_start = StationRef::new(side, 0);
        assert!(tied.contains(&(StationRef::new(main, 2), side_start)));
        assert!(tied.contains(&(StationRef::new(second, 0), side_start)));
    }

    #[test]
    fn comment_and_datum_go_to_the_chosen_half() {
        for (comment_side, on_a, on_b) in [
            (SplitSide::A, true, false),
            (SplitSide::B, false, true),
            (SplitSide::Both, true, true),
        ] {
            let (mut project, main, _) = project();
            let key = project.key_for_ref(StationRef::new(main, 2)).unwrap();
            project
                .station_comments
                .insert(key, "split here".to_string());
            project
                .set_zero_datum(
                    StationRef::new(main, 2),
                    GpsCoordinates {
                        latitude: 20.0,
                        longitude: -87.0,
                        depth_reference: 0.0,
                    },
                )
                .unwrap();

            let second = project
                .split_section_at_station(main, 2, &options(comment_side, SplitSide::B))
                .unwrap();

            let comment_on = |section: SectionId, index: u32| {
                let key = project
                    .key_for_ref(StationRef::new(section, index))
                    .unwrap();
                project.station_comments.contains_key(&key)
            };
            assert_eq!(comment_on(main, 2), on_a, "{comment_side:?}");
            assert_eq!(comment_on(second, 0), on_b, "{comment_side:?}");
            assert_eq!(project.zero_datums.len(), 1);
            assert_eq!(
                project.resolve_key(&project.zero_datums[0].station_ref),
                Some(StationRef::new(second, 0))
            );
        }
    }

    #[test]
    fn each_half_folds_uniform_credit_back_into_its_section() {
        let mut project = Project::new("Credit".to_string(), None);
        let ann = ShotCredit {
            surveyor: Some(Some("Ann".to_string())),
            date: Some(NaiveDate::from_ymd_opt(2026, 4, 11)),
        };
        let main = project
            .add_section(SectionData {
                surveyor: Some("Bob".to_string()),
                date: NaiveDate::from_ymd_opt(2026, 3, 2),
                shots: vec![
                    shot(90.0, 5.0),
                    shot(90.0, 5.0),
                    ShotData {
                        credit: Some(ann.clone()),
                        ..shot(90.0, 5.0)
                    },
                    ShotData {
                        credit: Some(ann),
                        ..shot(90.0, 5.0)
                    },
                ],
                ..Default::default()
            })
            .id;

        let second = project
            .split_section_at_station(main, 2, &options(SplitSide::A, SplitSide::A))
            .unwrap();

        let first = project.get_section(main).unwrap();
        assert_eq!(first.surveyor.as_deref(), Some("Bob"));
        let second = project.get_section(second).unwrap();
        assert_eq!(second.surveyor.as_deref(), Some("Ann"));
        assert_eq!(second.date, NaiveDate::from_ymd_opt(2026, 4, 11));
        assert!(
            second.shots.iter().all(|s| s.credit.is_none()),
            "Ann's half needs no overrides once the section credits her"
        );
    }
}
