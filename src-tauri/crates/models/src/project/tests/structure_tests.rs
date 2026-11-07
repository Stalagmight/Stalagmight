//! Section connectivity, station keys, serialization, the cascade BFS and station-ref traversals.
//!
//! Lines 23-483 of the former single-file `project/tests.rs` (#576 AFK-D3), moved verbatim; shared
//! helpers live in the module root (`tests.rs`).

use super::*;

#[test]
fn get_connected_section_ids_traverses_merges() {
    let mut project = Project::new("Merged".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let c = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(b, 0).into(),
        station_b: StationRef::new(c, 0).into(),
        merged_at: Utc::now(),
    });

    let connected = project.get_connected_section_ids(a);
    assert!(connected.contains(&a));
    assert!(connected.contains(&b));
    assert!(connected.contains(&c));
    assert_eq!(connected.len(), 3);
}

#[test]
fn station_key_round_trips_and_tracks_station_across_insert() {
    // A StationKey (stored, position-independent) round-trips with the positional
    // StationRef, and crucially tracks the SAME physical station when an unrelated
    // shot insert renumbers indices - the property that lets the stable-identity
    // refactor delete shift_station_refs.
    let mut project = Project::new("Keys".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0), shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids(); // guarantee anchors are available

    // Station 2 = the boundary after shot index 1.
    let sr = StationRef::new(a, 2);
    let key = project.key_for_ref(sr).expect("key for in-range station");
    assert!(matches!(key.anchor, StationAnchor::AfterShot(_)));
    assert_eq!(
        project.resolve_key(&key),
        Some(sr),
        "key_for_ref/resolve_key round-trip"
    );

    // SectionStart round-trips too.
    let start = StationRef::new(a, 0);
    let start_key = project.key_for_ref(start).unwrap();
    assert_eq!(start_key.anchor, StationAnchor::SectionStart);
    assert_eq!(project.resolve_key(&start_key), Some(start));

    // Insert a shot at the front: the physical station that was index 2 is now
    // index 3, and the key (anchored to the original shot's UUID) resolves there.
    let first_id = project.get_section(a).unwrap().shots[0].id;
    project.insert_shot_before(a, first_id, shot(0.0, 0.0, 5.0, 0.0, 0.0), None, None);
    assert_eq!(
        project.resolve_key(&key),
        Some(StationRef::new(a, 3)),
        "the key must track the same physical station after the insert shifts indices"
    );
    // A key whose anchoring shot no longer exists resolves to None (dangling).
    let bogus = StationKey::new(a, StationAnchor::AfterShot("not-a-real-uuid".to_string()));
    assert_eq!(project.resolve_key(&bogus), None);
}

#[test]
fn station_key_deserializes_legacy_positional_shape() {
    // Persisted undo snapshots (and offset/comment keys) written by pre-anchor
    // builds embed station refs in the old positional `{section_id, station_index}`
    // shape. The current StationKey must still deserialize them - mapping the index
    // to a transitional anchor - so opening an old project after upgrading does not
    // silently drop its undo history (the old blob would fail to parse otherwise).
    // Legacy index 0 -> SectionStart.
    let start: StationKey = serde_json::from_str(r#"{"section_id":3,"station_index":0}"#).unwrap();
    assert_eq!(start, StationKey::new(3, StationAnchor::SectionStart));

    // Legacy positive index -> transitional Index anchor (the load re-anchor pass
    // later canonicalizes it; resolve_key tolerates it in the meantime).
    let mid: StationKey = serde_json::from_str(r#"{"section_id":3,"station_index":2}"#).unwrap();
    assert_eq!(mid, StationKey::new(3, StationAnchor::Index(2)));

    // The current `{section_id, anchor}` shape still round-trips.
    let modern = StationKey::new(7, StationAnchor::AfterShot("uuid-1".to_string()));
    let json = serde_json::to_string(&modern).unwrap();
    assert_eq!(serde_json::from_str::<StationKey>(&json).unwrap(), modern);

    // A payload with neither field is still rejected.
    assert!(serde_json::from_str::<StationKey>(r#"{"section_id":3}"#).is_err());
}

#[test]
fn project_serialization_is_key_ordered_and_state_determined() {
    // The three HashMap-backed collections used to serialize in iteration order,
    // which is a function of the map's RandomState and its insertion history - NOT
    // of the project. So the same state produced different bytes: a whole-project
    // undo snapshot (`serde_json::to_string(project)`, regenerated on every
    // undo/redo) differed from the one taken at the state it had returned to, and a
    // re-save of an untouched project rewrote permuted keys and re-rolled the
    // save/load fixpoint. Ten sections, three maps, keys inserted in DESCENDING
    // order so "ascending output" cannot be insertion order leaking through.
    let mut project = Project::new("Deterministic".to_string(), None);
    let mut ids = Vec::new();
    for _ in 0..10 {
        ids.push(
            project
                .add_section(SectionData {
                    shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
                    ..Default::default()
                })
                .id,
        );
    }
    project.ensure_shot_uuids();
    for (i, &id) in ids.iter().enumerate().rev() {
        project.set_section_offset(id, (i as f64 + 1.0, -(i as f64) - 1.0));
        let start = project.key_for_ref(StationRef::new(id, 0)).unwrap();
        let after = project.key_for_ref(StationRef::new(id, 1)).unwrap();
        project.station_offsets.insert(after, (0.4, 0.5, 0.6));
        project
            .station_offsets
            .insert(start.clone(), (0.1, 0.2, 0.3));
        project
            .station_comments
            .insert(start, format!("comment {i}"));
    }
    assert_eq!(project.section_offsets.len(), 10);
    assert_eq!(project.station_offsets.len(), 20);

    // Every round trip rebuilds all three maps from scratch - a fresh RandomState
    // and a fresh insertion history, the two things iteration order is a function
    // of - so byte-identical output across several of them is the property.
    let mut previous = serde_json::to_string(&project).unwrap();
    for round in 0..8 {
        let copy: Project = serde_json::from_str(&previous).unwrap();
        let again = serde_json::to_string(&copy).unwrap();
        assert_eq!(
            again, previous,
            "round {round}: the same project must serialize to the same bytes"
        );
        previous = again;
    }

    // Stable AND ascending: `section_offsets` keeps its object shape, so check the
    // key order in the raw text (its values are arrays, so the object ends at the
    // first closing brace).
    let head = previous
        .find("\"section_offsets\":{")
        .expect("section_offsets in the payload");
    let object = &previous[head..head + previous[head..].find('}').expect("object end")];
    let mut last = 0;
    for &id in &ids {
        let at = object
            .find(&format!("\"{id}\":"))
            .unwrap_or_else(|| panic!("section {id} missing from {object}"));
        assert!(
            at > last,
            "section_offsets keys must ascend; {id} came out at {at} after {last} in {object}"
        );
        last = at;
    }

    // The two StationKey-keyed maps are emitted as arrays of pairs (serde_json
    // cannot key an object by a struct), ordered by section then anchor - and
    // `SectionStart` sorts before `AfterShot`, so each section's station 0 leads.
    let value: serde_json::Value = serde_json::from_str(&previous).unwrap();
    for field in ["station_offsets", "station_comments"] {
        let entries = value[field].as_array().expect("array of pairs");
        let order: Vec<(u64, u8)> = entries
            .iter()
            .map(|entry| {
                let key = &entry[0];
                let section = key["section_id"].as_u64().expect("section id");
                // `SectionStart` is a bare string, `AfterShot(uuid)` an object.
                let rank = u8::from(!key["anchor"].is_string());
                (section, rank)
            })
            .collect();
        let mut ascending = order.clone();
        ascending.sort_unstable();
        assert_eq!(order, ascending, "{field} must be emitted in key order");
    }
}

#[test]
fn cascade_bfs_does_not_propagate_through_a_non_finite_section() {
    // A section with non-finite (NaN/inf) geometry must be treated like a dangling
    // edge: cascade_bfs must NOT mark it - nor anything reachable ONLY through it -
    // authoritative, or it would seed propagation from an unpositioned section and
    // mis-locate that section's other merge partners. (Regression guard: the bug
    // anchored + enqueued the neighbour unconditionally, skipping only the offset
    // adjustment.)
    use std::collections::{HashMap, HashSet};

    let mut project = Project::new("NaN".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let n = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let x = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Per-station local coords. Section N is NaN-poisoned at every station.
    let local: HashMap<SectionId, Vec<(f64, f64)>> = HashMap::from([
        (a, vec![(0.0, 0.0), (1.0, 0.0)]),
        (n, vec![(f64::NAN, 0.0), (f64::NAN, 0.0)]),
        (x, vec![(5.0, 0.0), (6.0, 0.0)]),
    ]);
    let station_offsets: HashMap<StationRef, (f64, f64, f64)> = HashMap::new();
    // A:0 merges to N:0 (the only path to N), and N:1 merges to X:0 (the only path
    // to X), so X is reachable solely through the NaN section N.
    let adjacency: HashMap<SectionId, Vec<(StationRef, StationRef)>> = HashMap::from([
        (a, vec![(StationRef::new(a, 0), StationRef::new(n, 0))]),
        (n, vec![(StationRef::new(n, 1), StationRef::new(x, 0))]),
    ]);

    let mut authoritative: HashSet<SectionId> = HashSet::from([a]);
    project.cascade_bfs(
        &adjacency,
        &local,
        &station_offsets,
        &mut authoritative,
        &[a],
    );

    // Only the seed A stays authoritative: N is reached via a non-finite delta (not
    // anchored), and X is reachable only through N (never reached).
    assert!(
        !authoritative.contains(&n),
        "a NaN-poisoned section must not be anchored"
    );
    assert!(
        !authoritative.contains(&x),
        "a section reachable only through a NaN section must not be anchored"
    );
    assert_eq!(
        authoritative.len(),
        1,
        "only the seed should remain authoritative"
    );
}

#[test]
fn station_ref_traversals_cover_every_bearing_collection() {
    // for_each_station_ref / map_station_refs are the single enumeration of every
    // StationRef-bearing collection. If a new such collection is added without
    // wiring it in here, this test (and the integrity check that consumes the
    // traversal) won't see it - the failure that motivates centralizing them.
    let mut project = Project::new("Refs".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // One ref in each of the five collections, all anchored at station a:1.
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 1).into(),
        path_length: 1.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 1).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    project
        .station_offsets
        .insert(StationRef::new(a, 1).into(), (1.0, 2.0, 3.0));
    project
        .station_comments
        .insert(StationRef::new(a, 1).into(), "note".to_string());

    // READ traversal: every collection's label is visited.
    let mut labels: HashSet<String> = HashSet::new();
    let mut a1_hits = 0;
    project.for_each_station_ref(|label, sr| {
        labels.insert(label.to_string());
        if sr == StationRef::new(a, 1).into() {
            a1_hits += 1;
        }
    });
    for expected in [
        "merge",
        "loop closure",
        "zero datum",
        "station offset",
        "station comment",
    ] {
        assert!(
            labels.contains(expected),
            "for_each_station_ref must visit {expected}"
        );
    }
    assert_eq!(
        a1_hits, 5,
        "a:1 is referenced once in each of the five collections"
    );

    // REWRITE traversal: shifting a:1 -> a:2 must update every collection, incl.
    // the two HashMap keys.
    project.map_station_refs(|key| {
        if key == StationRef::new(a, 1).into() {
            StationRef::new(a, 2).into()
        } else {
            key
        }
    });
    // Pure traversal mechanics: keys were inserted as transitional Index anchors
    // and never reanchored, so compare the rewritten keys directly (a:2 is
    // intentionally past the end - this checks the rewrite, not resolvability).
    assert_eq!(
        project.station_merges[0].station_a,
        StationRef::new(a, 2).into()
    );
    assert_eq!(
        project.loop_closures[0].station_a,
        StationRef::new(a, 2).into()
    );
    assert_eq!(
        project.zero_datums[0].station_ref,
        StationRef::new(a, 2).into()
    );
    assert!(project
        .station_offsets
        .contains_key(&StationRef::new(a, 2).into()));
    assert!(!project
        .station_offsets
        .contains_key(&StationRef::new(a, 1).into()));
    assert!(project
        .station_comments
        .contains_key(&StationRef::new(a, 2).into()));
    assert!(!project
        .station_comments
        .contains_key(&StationRef::new(a, 1).into()));
}

#[test]
fn section_has_zero_datum_with_connected_merges() {
    let mut project = Project::new("Zero".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let c = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let d = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(b, 0).into(),
        station_b: StationRef::new(c, 0).into(),
        merged_at: Utc::now(),
    });

    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(c, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 5.0).unwrap(),
    });

    assert!(project.section_has_zero_datum(a));
    assert!(!project.section_has_zero_datum(d));
}

#[test]
fn compute_station_raw_coords_applies_offsets_and_bounds() {
    let mut project = Project::new("Coords".to_string(), None);
    let shot_a = shot(0.0, 0.0, 10.0, 0.0, 0.0);
    let shot_b = shot(90.0, 90.0, 5.0, 0.0, 2.0);
    let section_id = project
        .add_section(SectionData {
            shots: vec![shot_a.clone(), shot_b.clone()],
            ..Default::default()
        })
        .id;

    project.section_offsets.insert(section_id, (2.0, -3.0));

    let coords = project
        .compute_station_raw_coords(StationRef::new(section_id, 2))
        .expect("coords");
    let expected_x = 2.0 + shot_b.horizontal_length();
    let expected_y = -3.0 + shot_a.horizontal_length();
    assert!((coords.x - expected_x).abs() < 1e-6);
    assert!((coords.y - expected_y).abs() < 1e-6);
    assert!((coords.z - 2.0).abs() < 1e-6);

    let invalid = project.compute_station_raw_coords(StationRef::new(section_id, 5));
    assert!(invalid.is_none());
}
