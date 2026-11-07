//! [#576 NF-W13BE-2] Two stored keys that land on one key when the station refs
//! are rewritten.
//!
//! A legacy `Index` anchor beside the canonical one names the same station; the
//! re-anchor of a restored project (an undo snapshot from an older build)
//! rewrites both onto the canonical key. `map_station_refs` rebuilt the comment
//! and offset maps by collecting, so one of the two entries was silently
//! dropped - and which one depended on `HashMap` order, so the same project
//! could keep either. Every map here is built fresh, with its own random seed,
//! so an outcome that depends on iteration order shows up across the runs.

use std::collections::{BTreeSet, HashMap};

use super::Project;
use crate::test_fixtures::straight_shot;
use crate::{SectionData, SectionId, StationAnchor, StationKey, StationRef};

/// Runs per test: each builds fresh maps, so an order-dependent outcome shows up
/// as more than one distinct result.
const RUNS: usize = 64;

fn project_with_three_shots() -> (Project, SectionId) {
    let mut project = Project::new("CaveA".to_string(), None);
    let section = project.add_section(SectionData {
        name: None,
        surveyor: None,
        date: None,
        color: "#ffffff".to_string(),
        direction: None,
        quality: 1.0,
        shots: vec![
            straight_shot(0.0, 10.0),
            straight_shot(90.0, 10.0),
            straight_shot(180.0, 10.0),
        ],
    });
    let id = section.id;
    project.ensure_shot_uuids();
    (project, id)
}

fn legacy(section_id: SectionId, station_index: u32) -> StationKey {
    StationKey {
        section_id,
        anchor: StationAnchor::Index(station_index),
    }
}

/// The re-anchor keeps BOTH comments (the canonical key's first, then the legacy
/// key's, on its own line) and the canonical key's offset, whatever the order
/// the maps hand their entries out in.
#[test]
fn the_reanchor_keeps_both_comments_and_the_canonical_offset_whatever_the_map_order() {
    let mut outcomes: BTreeSet<String> = BTreeSet::new();
    for _ in 0..RUNS {
        let (mut project, id) = project_with_three_shots();
        let canonical = project
            .key_for_ref(StationRef::new(id, 2))
            .expect("station 2 has a canonical key");
        let old = legacy(id, 2);
        assert_ne!(canonical, old, "precondition: two distinct keys");
        assert_eq!(project.resolve_key(&canonical), project.resolve_key(&old));
        project.station_comments = HashMap::from([
            (canonical.clone(), "canonical note".to_string()),
            (old.clone(), "legacy note".to_string()),
        ]);
        project.station_offsets =
            HashMap::from([(canonical.clone(), (1.0, 2.0, 3.0)), (old, (7.0, 8.0, 9.0))]);

        project.reanchor_station_refs();

        outcomes.insert(format!(
            "comments={} {:?} offsets={} {:?}",
            project.station_comments.len(),
            project.station_comments.get(&canonical),
            project.station_offsets.len(),
            project.station_offsets.get(&canonical),
        ));
    }
    for outcome in &outcomes {
        println!("NF-W13BE-2 reanchor outcome: {outcome}");
    }
    let expected = format!(
        "comments=1 {:?} offsets=1 {:?}",
        Some("canonical note\nlegacy note"),
        Some((1.0, 2.0, 3.0))
    );
    assert_eq!(
        outcomes,
        BTreeSet::from([expected]),
        "one outcome over {RUNS} fresh maps, both comments kept, the canonical offset kept"
    );
}

/// Two rewritten keys landing on a third (neither was already there): the
/// order is the old keys' own (`SectionStart` before `Index`), never the map's.
/// A comment equal to the one it joins is not repeated.
#[test]
fn a_collision_between_two_rewritten_keys_is_resolved_by_key_order_not_map_order() {
    let mut outcomes: BTreeSet<String> = BTreeSet::new();
    for _ in 0..RUNS {
        let (mut project, id) = project_with_three_shots();
        let start = project
            .key_for_ref(StationRef::new(id, 0))
            .expect("station 0 has a canonical key");
        let target = project
            .key_for_ref(StationRef::new(id, 3))
            .expect("station 3 has a canonical key");
        let old = legacy(id, 1);
        let same = legacy(id, 2);
        project.station_comments = HashMap::from([
            (start.clone(), "start note".to_string()),
            (old.clone(), "index note".to_string()),
            (same.clone(), "start note".to_string()),
        ]);
        project.station_offsets = HashMap::from([
            (start.clone(), (1.0, 1.0, 1.0)),
            (old.clone(), (2.0, 2.0, 2.0)),
        ]);

        project.map_station_refs(|key| {
            if key == start || key == old || key == same {
                target.clone()
            } else {
                key
            }
        });

        outcomes.insert(format!(
            "comments={} {:?} offsets={} {:?}",
            project.station_comments.len(),
            project.station_comments.get(&target),
            project.station_offsets.len(),
            project.station_offsets.get(&target),
        ));
    }
    for outcome in &outcomes {
        println!("NF-W13BE-2 rewrite outcome: {outcome}");
    }
    let expected = format!(
        "comments=1 {:?} offsets=1 {:?}",
        Some("start note\nindex note"),
        Some((1.0, 1.0, 1.0))
    );
    assert_eq!(outcomes, BTreeSet::from([expected]));
}
