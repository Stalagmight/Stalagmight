//! Shot UUIDs unique across the whole project (`project/uuid_unique.rs`).
//!
//! Every project here is synthetic. Two sections carrying the same ids is the
//! shape a file saved before project-wide uniqueness can hold: an Ariane file
//! imported twice, or a merge of two copies of one project.

use chrono::Utc;

use super::*;

const IDS: [&str; 3] = [
    "59508181-468f-429e-9dac-7c8e82fe53a1",
    "473a69ed-22ac-449f-8fa1-11308801104a",
    "45fcbcd1-6d49-45fb-8504-7c5734fa7c9d",
];

fn shot_with(uuid: &str, azimuth: f64) -> ShotData {
    ShotData {
        uuid: Some(uuid.to_string()),
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length: 10.0,
        ..Default::default()
    }
}

fn copy_of_ids() -> SectionData {
    SectionData {
        shots: IDS
            .iter()
            .enumerate()
            .map(|(i, id)| shot_with(id, i as f64 * 30.0))
            .collect(),
        ..Default::default()
    }
}

/// Sections 0 and 1 hold the same three ids. Section 1 carries a comment on its
/// station 2, a merge from its start onto section 0's station 2, and a gauge pair;
/// section 0 carries a comment on its own station 2.
fn two_copies() -> Project {
    let mut project = Project::new("Copies".to_string(), None);
    project.add_section(copy_of_ids());
    project.add_section(copy_of_ids());
    let on_first = project.key_for_ref(StationRef::new(0, 2)).unwrap();
    let on_second = project.key_for_ref(StationRef::new(1, 2)).unwrap();
    let second_start = project.key_for_ref(StationRef::new(1, 0)).unwrap();
    project
        .station_comments
        .insert(on_first.clone(), "first".to_string());
    project
        .station_comments
        .insert(on_second, "second".to_string());
    project.record_station_merge(StationMerge {
        station_a: second_start,
        station_b: on_first,
        merged_at: Utc::now(),
    });
    let second = project.get_section_mut(1).unwrap();
    second.gauge_shot_uuid = Some(IDS[0].to_string());
    second.gauge_neighbour_uuid = Some(IDS[1].to_string());
    project
}

fn ids_of(project: &Project, section: SectionId) -> Vec<String> {
    project
        .get_section(section)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.uuid.clone().unwrap())
        .collect()
}

fn comment_at(project: &Project, section: SectionId, index: u32) -> Option<String> {
    let key = project.key_for_ref(StationRef::new(section, index))?;
    project.station_comments.get(&key).cloned()
}

#[test]
fn the_derived_id_is_pinned_v4_shaped_and_keyed_by_section_and_old_id() {
    // Computed independently (Python `uuid.uuid5` over the same name, re-stamped
    // as a v4); a change here re-heals every file differently.
    assert_eq!(
        reminted_shot_uuid(1, IDS[0]),
        "2333152b-70f0-41a9-bc11-52639e8794ac"
    );
    assert_eq!(
        reminted_shot_uuid(7, "abc"),
        "0f70476a-37ad-4224-a528-4ddc4e23d8e1"
    );
    let parsed = uuid::Uuid::parse_str(&reminted_shot_uuid(7, "abc")).unwrap();
    assert_eq!(parsed.get_version_num(), 4);
    assert_eq!(parsed.get_variant(), uuid::Variant::RFC4122);
    assert_ne!(reminted_shot_uuid(1, "abc"), reminted_shot_uuid(2, "abc"));
}

#[test]
fn stepping_skips_an_id_already_in_use() {
    let first = reminted_shot_uuid(7, "abc");
    let stepped = reminted_shot_uuid_avoiding(7, "abc", |c| c == first);
    assert_eq!(stepped, "24ae3b14-e2ae-45ef-bf30-6b9f650db8c6");
}

#[test]
fn the_lowest_section_keeps_its_ids_and_the_other_is_renamed_with_its_refs() {
    let mut project = two_copies();
    let stamp = project.modified_at;

    let remap = project.make_shot_uuids_project_unique();

    assert_eq!(ids_of(&project, 0), IDS);
    let expected: Vec<String> = IDS.iter().map(|id| reminted_shot_uuid(1, id)).collect();
    assert_eq!(ids_of(&project, 1), expected);
    assert_eq!(remap.len(), 3);
    assert_eq!(remap.get(1, IDS[2]), Some(expected[2].as_str()));
    assert_eq!(remap.get(0, IDS[2]), None);

    // Each comment is still on its own physical station.
    assert_eq!(comment_at(&project, 0, 2).as_deref(), Some("first"));
    assert_eq!(comment_at(&project, 1, 2).as_deref(), Some("second"));
    // The merge still joins section 1's start to section 0's station 2.
    let merge = project.station_merges.iter().next().unwrap();
    assert_eq!(
        project.resolve_key(&merge.station_a),
        Some(StationRef::new(1, 0))
    );
    assert_eq!(
        project.resolve_key(&merge.station_b),
        Some(StationRef::new(0, 2))
    );
    // The gauge pair follows its shots.
    let second = project.get_section(1).unwrap();
    assert_eq!(
        second.gauge_shot_uuid.as_deref(),
        Some(expected[0].as_str())
    );
    assert_eq!(
        second.gauge_neighbour_uuid.as_deref(),
        Some(expected[1].as_str())
    );

    assert!(project.check_structural_integrity().is_empty());
    assert_eq!(
        project.modified_at, stamp,
        "a heal must not touch the project"
    );
}

#[test]
fn healing_twice_changes_nothing_the_second_time() {
    let mut project = two_copies();
    project.make_shot_uuids_project_unique();
    let once = serde_json::to_string(&project).unwrap();
    assert!(project.make_shot_uuids_project_unique().is_empty());
    assert_eq!(serde_json::to_string(&project).unwrap(), once);
}

#[test]
fn a_door_renames_only_its_newcomers_even_when_they_hold_the_lower_id() {
    let mut project = Project::new("Door".to_string(), None);
    project.add_section(copy_of_ids());
    project.add_section(copy_of_ids());
    project.add_section(copy_of_ids());

    // Section 0 is the newcomer; 1 and 2 were already in the project, so 1 keeps
    // the ids (lowest existing holder), 0 yields, and 2 is left to the load heal.
    let remap = project.reassign_repeated_shot_uuids(&[0]);
    assert_eq!(remap.sections(), [0].into_iter().collect());
    let expected: Vec<String> = IDS.iter().map(|id| reminted_shot_uuid(0, id)).collect();
    assert_eq!(ids_of(&project, 0), expected);
    assert_eq!(ids_of(&project, 1), IDS);
    assert_eq!(ids_of(&project, 2), IDS);
}

#[test]
fn two_newcomers_sharing_ids_keep_them_in_the_lower_one() {
    let mut project = Project::new("Door".to_string(), None);
    project.add_section(copy_of_ids());
    project.add_section(copy_of_ids());

    let remap = project.reassign_repeated_shot_uuids(&[1, 0]);
    assert_eq!(ids_of(&project, 0), IDS);
    assert_eq!(remap.sections(), [1].into_iter().collect());
}

#[test]
fn the_json_rewrite_matches_the_typed_one() {
    let original = two_copies();
    let mut typed = original.clone();
    let remap = typed.make_shot_uuids_project_unique();

    let mut value = serde_json::to_value(&original).unwrap();
    let changed = remap_project_json(&mut value, &|section, old| {
        remap.get(section, old).map(str::to_string)
    });
    assert!(changed);

    // Compared through a reparse: the snapshot serializer sorts keyed maps by
    // key, and a rename can change that order.
    let rewritten: Project = serde_json::from_value(value).unwrap();
    assert_eq!(
        serde_json::to_string(&rewritten).unwrap(),
        serde_json::to_string(&typed).unwrap()
    );
}

#[test]
fn the_json_rewrite_adds_no_key_and_leaves_positional_refs_alone() {
    let mut value = serde_json::json!({
        "sections": [{ "id": 1, "shots": [{ "uuid": IDS[0] }] }],
        "station_comments": [
            [{ "section_id": 1, "anchor": { "Index": 1 } }, "positional"],
            [{ "section_id": 1, "anchor": { "AfterShot": IDS[0] } }, "anchored"],
            [{ "section_id": 0, "anchor": { "AfterShot": IDS[0] } }, "other section"]
        ]
    });
    let new = reminted_shot_uuid(1, IDS[0]);
    let changed = remap_project_json(&mut value, &|section, old| {
        (section == 1 && old == IDS[0]).then(|| new.clone())
    });
    assert!(changed);
    assert_eq!(value["sections"][0]["shots"][0]["uuid"], new.as_str());
    assert_eq!(value["station_comments"][0][0]["anchor"]["Index"], 1);
    assert_eq!(
        value["station_comments"][1][0]["anchor"]["AfterShot"],
        new.as_str()
    );
    assert_eq!(
        value["station_comments"][2][0]["anchor"]["AfterShot"],
        IDS[0]
    );
    let text = value.to_string();
    assert!(!text.contains("\"validation\":"));
    assert!(!text.contains("\"surface_elevation\":"));
}

#[test]
fn a_stored_id_matches_the_shot_it_was_renamed_to() {
    let renamed = reminted_shot_uuid(3, IDS[1]);
    assert!(stored_shot_uuid_matches(3, IDS[1], IDS[1]));
    assert!(stored_shot_uuid_matches(3, IDS[1], &renamed));
    assert!(!stored_shot_uuid_matches(4, IDS[1], &renamed));
    assert!(!stored_shot_uuid_matches(3, IDS[2], &renamed));
}

#[test]
fn integrity_reports_an_id_in_two_sections_and_repair_heals_it() {
    let mut project = two_copies();
    let violations = project.check_structural_integrity();
    let across: Vec<_> = violations
        .iter()
        .filter(|v| v.category == "duplicate_shot_uuid_across_sections")
        .collect();
    assert_eq!(across.len(), 3, "one violation per repeated id");
    assert!(across
        .iter()
        .all(|v| v.severity == ViolationSeverity::Critical));

    project.repair_structural_integrity();
    assert!(project.check_structural_integrity().is_empty());
    assert_eq!(comment_at(&project, 1, 2).as_deref(), Some("second"));
}

#[test]
fn a_plan_anchor_saved_against_a_renamed_id_still_finds_its_shot() {
    let mut project = two_copies();
    project.make_shot_uuids_project_unique();
    let second = project.get_section(1).unwrap();
    let found = crate::PlanAnchor::position_of(&project, second, &Some(IDS[2].to_string()));
    assert_eq!(
        found,
        Some(2),
        "the old id resolves through the derived one"
    );
    let first = project.get_section(0).unwrap();
    let kept = crate::PlanAnchor::position_of(&project, first, &Some(IDS[2].to_string()));
    assert_eq!(
        kept,
        Some(2),
        "the section that kept the id still finds it directly"
    );
}

/// Every shot's record parity (`Project::shot_record_parity`), section by
/// section, in shot order.
fn record_parities(project: &Project) -> Vec<Vec<bool>> {
    project
        .sections
        .iter()
        .map(|section| {
            section
                .shots
                .iter()
                .map(|shot| project.shot_record_parity(section, shot.uuid.as_deref()))
                .collect()
        })
        .collect()
}

/// The load heal renames one project's own repeats. The counter-parity set is
/// keyed by id, so before the heal both copies of `IDS[1]` read its entry;
/// after it both still do, the kept copy through the old id and the renamed
/// one through the new.
#[test]
fn the_load_heal_keeps_every_shots_record_parity() {
    let mut project = two_copies();
    project.counter_parity_shots.insert(IDS[1].to_string());
    let before = record_parities(&project);
    assert_eq!(before, vec![vec![false, true, false]; 2], "setup");

    project.make_shot_uuids_project_unique();

    assert_eq!(record_parities(&project), before);
    assert!(project
        .counter_parity_shots
        .contains(&reminted_shot_uuid(1, IDS[1])));
}

/// A newcomer that yields an id to the project does NOT take the project's
/// counter-parity entry for it: that entry belongs to the project's shot, and
/// the newcomer's parity comes from its own source (Add .stalx carries it).
#[test]
fn a_newcomer_renamed_off_a_shared_id_does_not_take_its_counter_parity() {
    let mut project = Project::new("Door".to_string(), None);
    project.add_section(copy_of_ids());
    project.counter_parity_shots.insert(IDS[1].to_string());
    project.add_section(copy_of_ids());

    project.reassign_repeated_shot_uuids(&[1]);

    assert_eq!(
        record_parities(&project),
        vec![vec![false, true, false], vec![false, false, false]]
    );
    assert_eq!(
        project.counter_parity_shots,
        [IDS[1].to_string()].into_iter().collect()
    );
}
