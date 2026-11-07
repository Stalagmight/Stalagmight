//! [#576 W30-NF3] Two comments on one station are exported in the save's order,
//! the same on every export.
//!
//! Two stored keys can name one station: a legacy positional `Index` key
//! beside the station's canonical key (see `Project::one_row_per_station`).
//! The `.tml` export sorted the comments by station only, so the two kept the
//! order the project's `HashMap` happened to iterate them in - which differs
//! between two maps holding the same entries, so between runs, and could differ
//! from the order the save joins them in.
//!
//! All data is synthetic, built in code.

#![expect(clippy::unwrap_used, clippy::print_stdout)]

use crate::models::project::join_station_comment;
use crate::models::test_fixtures::straight_shot;
use crate::models::{Project, SectionData, StationKey, StationRef};

use super::tml::export_tml;

const CANONICAL: &str = "comment under the canonical key";
const LEGACY: &str = "comment under a legacy positional key";

/// One section of two shots. Its end station (index 2) holds two comments:
/// one under its canonical key, one under the legacy positional key that names
/// the same station. Every uuid is spelled out so two builds export alike.
fn project_with_two_keys_on_one_station() -> (Project, StationRef) {
    let mut project = Project::new("CaveA".to_string(), None);
    project.add_section(SectionData {
        name: Some("SectionA".to_string()),
        color: "#336699".to_string(),
        shots: vec![
            straight_shot(10.0, 5.0).with_uuid("00000000-0000-4000-8000-000000000001"),
            straight_shot(95.0, 4.0).with_uuid("00000000-0000-4000-8000-000000000002"),
        ],
        ..Default::default()
    });
    let sid = project.sections.first().unwrap().id;
    let end = StationRef::new(sid, 2);
    let canonical = project.key_for_ref(end).unwrap();
    let legacy = StationKey::positional(sid, 2);
    assert_ne!(canonical, legacy, "fixture: two distinct keys");
    project
        .station_comments
        .insert(canonical, CANONICAL.to_string());
    project.station_comments.insert(legacy, LEGACY.to_string());
    (project, end)
}

/// The order the project's comment map iterates its entries in.
fn map_order(project: &Project) -> Vec<String> {
    project.station_comments.values().cloned().collect()
}

/// The `.tmlu` the export writes for `project`, as bytes (plain XML, so no
/// archive timestamp can differ between two exports).
fn export_bytes(project: &Project) -> Vec<u8> {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("CaveA.tmlu");
    export_tml(project, &path).unwrap();
    std::fs::read(&path).unwrap()
}

/// Two projects built alike, whose comment maps iterate the two comments in
/// opposite orders, export the same bytes, with the comments in the order the
/// save joins them: the canonical key's first.
#[test]
fn two_comments_on_one_station_export_in_the_save_order_every_time() {
    let (first, end) = project_with_two_keys_on_one_station();
    let first_order = map_order(&first);
    // A fresh map draws a fresh hash seed, so a few builds give the other order.
    let second = (0..256)
        .map(|_| project_with_two_keys_on_one_station().0)
        .find(|project| map_order(project) != first_order)
        .unwrap();

    let entries: Vec<(&StationKey, StationRef, String)> = first
        .station_comments
        .iter()
        .map(|(key, text)| (key, first.resolve_key(key).unwrap(), text.clone()))
        .collect();
    let saved = first.one_row_per_station(entries, join_station_comment);

    let a = String::from_utf8(export_bytes(&first)).unwrap();
    let b = String::from_utf8(export_bytes(&second)).unwrap();
    let order = |xml: &str| (xml.find(CANONICAL), xml.find(LEGACY));
    println!("map orders: {first_order:?} / {:?}", map_order(&second));
    println!("the save writes for {end:?}: {saved:?}");
    println!(
        "export 1: (canonical, legacy) at {:?}; export 2: at {:?}; same bytes: {}",
        order(&a),
        order(&b),
        a == b
    );

    assert_eq!(
        saved,
        vec![(end, format!("{CANONICAL}\n{LEGACY}"))],
        "the save joins them, the canonical key's first"
    );
    let (canonical_at, legacy_at) = order(&a);
    assert!(
        canonical_at.unwrap() < legacy_at.unwrap(),
        "the export joins them in the save's order"
    );
    assert_eq!(a, b, "two exports of the same survey are the same bytes");
}
