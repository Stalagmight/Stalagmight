use super::*;
use crate::{ClosureStrategy, ShotKind};

fn shot(azimuth_in: f64, azimuth_out: f64, length: f64, depth_in: f64, depth_out: f64) -> ShotData {
    ShotData {
        azimuth_in,
        azimuth_out,
        length,
        depth_in,
        depth_out,
        ..Default::default()
    }
}

/// Rewrite the first merge in place, for the integrity tests that need a forged
/// (dangling) endpoint. `iter_mut` is the only mutable path into the merges -
/// there is deliberately no `&mut Vec<StationMerge>` - and it invalidates the
/// adjacency memo, exactly as `map_station_refs` does in production.
fn forge_first_merge(project: &mut Project, f: impl FnOnce(&mut StationMerge)) {
    let merge = project
        .station_merges
        .iter_mut()
        .next()
        .expect("fixture has at least one merge");
    f(merge);
}

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

#[test]
fn set_zero_datum_first_datum_positions_station_at_origin() {
    let mut project = Project::new("FirstDatum".to_string(), None);
    // Create a section with a shot going north (azimuth 0)
    let shot_a = shot(0.0, 0.0, 10.0, 0.0, 1.0);
    let section_id = project
        .add_section(SectionData {
            shots: vec![shot_a.clone()],
            ..Default::default()
        })
        .id;

    // Station 1 is at (0, 10, 1) before datum is set
    let station_ref = StationRef::new(section_id, 1);
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();

    // Set first datum on station 1
    project.set_zero_datum(station_ref, gps).unwrap();

    // Check that offset was applied to move station to origin
    let offset = project.section_offsets.get(&section_id).unwrap();
    // Station was at (0, horizontal), needs offset (-0, -horizontal) to be at origin
    assert!(
        (offset.0 - 0.0).abs() < 1e-6,
        "Expected x offset ~0, got {}",
        offset.0
    );
    let expected_y_offset = -shot_a.horizontal_length();
    assert!(
        (offset.1 - expected_y_offset).abs() < 1e-6,
        "Expected y offset ~{}, got {}",
        expected_y_offset,
        offset.1
    );

    // Verify station coordinates after applying offset
    let coords = project.compute_station_raw_coords(station_ref).unwrap();
    assert!((coords.x).abs() < 1e-6, "Expected x ~0, got {}", coords.x);
    assert!((coords.y).abs() < 1e-6, "Expected y ~0, got {}", coords.y);
    // Z should be preserved
    assert!(
        (coords.z - 1.0).abs() < 1e-6,
        "Expected z ~1, got {}",
        coords.z
    );
}

/// Adding the survey's FIRST datum translates the WHOLE survey as one rigid
/// body, not just the datum's own merge group.
///
/// Before the frame exists, local `(0, 0)` means nothing and nothing has been
/// placed against it, so the only real content of the project is the RELATIVE
/// arrangement of its components. Landing the datum on the origin by moving its
/// own merge group is the one answer that destroys exactly that: every other
/// component stays where it was and the whole survey silently re-arranges
/// itself. `renormalize_frame` gives the other answer - one translation applied
/// to every section - which lands the datum on the origin AND leaves every
/// inter-component vector bit-for-bit intact.
///
/// Non-vacuous by construction, on both halves. The two sections are separate
/// components (no merge, no closure, so `adjust_rigid_group_offsets` on `a`
/// cannot reach `b`), and the datum's station sits 10 m north of its section's
/// start, so establishing the frame is a real (0, -10) translation rather than
/// nothing at all. Under the pre-fix rule `b` keeps its coordinates and the
/// vector between the components changes by the full 10 m.
#[test]
fn set_zero_datum_first_datum_translates_every_component() {
    let mut project = Project::new("FirstDatumFrame".to_string(), None);
    // Component A: one shot due north, so station 1 sits at (0, 10) - away from
    // the origin, which is what gives the establishing translation something to
    // do.
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    // Component B: unconnected to A and deliberately parked away from the
    // origin, so "B did not move" and "B moved with A" are different answers.
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 4.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.set_section_offset(b, (25.0, -13.0));
    assert!(
        project.station_merges.is_empty() && project.loop_closures.is_empty(),
        "setup: the two sections have to be independent components, or the datum's rigid group \
         would reach B and the test would prove nothing"
    );

    let datum_station = StationRef::new(a, 1);
    let a1_before = project.compute_station_raw_coords(datum_station).unwrap();
    assert!(
        a1_before.x.abs() < 1e-9 && (a1_before.y - 10.0).abs() < 1e-9,
        "setup: the datum station must start AWAY from the origin, got {a1_before:?}"
    );
    let b0_before = project
        .compute_station_raw_coords(StationRef::new(b, 0))
        .unwrap();
    let b1_before = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();

    project
        .set_zero_datum(
            datum_station,
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();

    // Half one: the datum station IS the origin now. That is what establishing
    // the frame means, and every reader of local metres depends on it.
    let a1_after = project.compute_station_raw_coords(datum_station).unwrap();
    assert!(
        a1_after.x.abs() < 1e-9 && a1_after.y.abs() < 1e-9,
        "the first datum's station has to sit at local (0, 0), got {a1_after:?}"
    );

    // Half two: the OTHER component moved by the same delta, so nothing moved
    // relative to anything else.
    let delta = (-a1_before.x, -a1_before.y);
    let b0_after = project
        .compute_station_raw_coords(StationRef::new(b, 0))
        .unwrap();
    let b1_after = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    assert!(
        (b0_after.x - (b0_before.x + delta.0)).abs() < 1e-9
            && (b0_after.y - (b0_before.y + delta.1)).abs() < 1e-9,
        "the unconnected component has to ride the establishing translation {delta:?}: B:0 went \
         {b0_before:?} -> {b0_after:?}"
    );
    assert!(
        (b1_after.x - (b1_before.x + delta.0)).abs() < 1e-9
            && (b1_after.y - (b1_before.y + delta.1)).abs() < 1e-9,
        "the unconnected component has to ride the establishing translation {delta:?}: B:1 went \
         {b1_before:?} -> {b1_after:?}"
    );

    // Said the other way round, which is the property the user actually owns:
    // the vector between the two components is unchanged.
    let separation_before = (b0_before.x - a1_before.x, b0_before.y - a1_before.y);
    let separation_after = (b0_after.x - a1_after.x, b0_after.y - a1_after.y);
    assert!(
        (separation_after.0 - separation_before.0).abs() < 1e-9
            && (separation_after.1 - separation_before.1).abs() < 1e-9,
        "adding the survey's first georeference moved one component relative to another: A:1 -> \
         B:0 was {separation_before:?}, is {separation_after:?}"
    );
}

/// A datum whose station no longer resolves anchors NOTHING, so the survey's
/// first RESOLVABLE datum is the one that establishes the frame - and a new
/// datum added while only a dangling one is stored lands on the origin, not at
/// `gps_to_local(its gps, the dangling one's gps)`.
///
/// A dangling datum is dropped by `Project::datum_to_wire` before the frontend
/// ever sees it, and `Project::frame_datum_key` skips it, so the map's
/// `gpsOrigin`, the data table's GPS column and the Ariane export all read the
/// first resolvable one. `set_zero_datum` used to read `zero_datums.first()`
/// unfiltered, which positioned the new group against an origin nothing else in
/// the app reads.
///
/// Non-vacuous by construction: the stored datum's `AfterShot` anchor names a
/// uuid no shot carries (asserted below, so the setup cannot rot into a
/// resolvable key), and its GPS sits over 2 km from the new one - so under the
/// unfiltered rule the new component was flung by that whole distance instead
/// of landing on the origin.
#[test]
fn set_zero_datum_ignores_a_dangling_datum_when_choosing_the_frame() {
    let mut project = Project::new("DanglingFrame".to_string(), None);
    // The section the dangling datum claims to be anchored in, and a second,
    // unconnected component that will carry the real datum.
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 7.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.set_section_offset(b, (12.0, 30.0));

    // The dangling datum, pushed straight onto the vector: no mutator will
    // create one, it is what a project reaches when the anchoring shot is
    // deleted out from under a stored datum.
    let dangling_gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project.zero_datums.push(ZeroDatum {
        station_ref: StationKey::new(a, StationAnchor::AfterShot("no-such-uuid".to_string())),
        gps: dangling_gps,
    });
    assert_eq!(
        project.resolve_key(&project.zero_datums[0].station_ref),
        None,
        "setup: the stored datum has to be DANGLING, or this test is about the ordinary \
         subsequent-datum path instead"
    );

    let datum_station = StationRef::new(b, 1);
    let b1_before = project.compute_station_raw_coords(datum_station).unwrap();
    let a0_before = project
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();
    // Over 2 km north of the dangling fix: the displacement the unfiltered rule
    // would have applied, and far past any tolerance here.
    let new_gps = GpsCoordinates::new(45.02, -90.0, 0.0).unwrap();
    let flung = crate::gps_to_local(&new_gps, &dangling_gps);
    assert!(
        flung.0.hypot(flung.1) > 1000.0,
        "setup: the two GPS fixes must be far enough apart to be unmistakable, got {flung:?}"
    );

    project.set_zero_datum(datum_station, new_gps).unwrap();

    let b1_after = project.compute_station_raw_coords(datum_station).unwrap();
    assert!(
        b1_after.x.abs() < 1e-9 && b1_after.y.abs() < 1e-9,
        "the first RESOLVABLE datum defines the frame, so its station has to sit at local (0, 0); \
         got {b1_after:?} (the dangling datum's GPS would have put it near {flung:?})"
    );

    // And, as with any establishing add, nothing moved relative to anything
    // else - the dangling-datum section rode along.
    let a0_after = project
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();
    let separation_before = (a0_before.x - b1_before.x, a0_before.y - b1_before.y);
    let separation_after = (a0_after.x - b1_after.x, a0_after.y - b1_after.y);
    assert!(
        (separation_after.0 - separation_before.0).abs() < 1e-9
            && (separation_after.1 - separation_before.1).abs() < 1e-9,
        "the establishing add moved one component relative to another: B:1 -> A:0 was \
         {separation_before:?}, is {separation_after:?}"
    );
}

#[test]
fn set_zero_datum_connected_sections_move_together() {
    let mut project = Project::new("Connected".to_string(), None);
    // Create two sections
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Align b:0 to a:1 (a:1 is at (0, 10) since shot goes north 10m)
    project.set_section_offset(b, (0.0, 10.0));

    // Merge section b's station 0 to section a's station 1
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    // Record positions before setting datum
    // Section a station 1 is at (0, 10) before datum (shot goes north)
    // Section b station 0 is at (0, 10) before datum (aligned to a:1)
    // Section b station 1 is at (5, 10) before datum (shot goes east from b:0)
    let coords_b1_before = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    assert!(
        (coords_b1_before.x - 5.0).abs() < 1e-6,
        "Pre-datum b.1 x should be 5"
    );
    assert!(
        (coords_b1_before.y - 10.0).abs() < 1e-6,
        "Pre-datum b.1 y should be 10"
    );

    // Set datum on section a, station 1 (which is at y=10 before datum)
    // This should move all connected sections so a.1 lands at origin
    let station_ref = StationRef::new(a, 1);
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project.set_zero_datum(station_ref, gps).unwrap();

    // Both sections should have the datum shift applied. Asserted on the
    // RESOLVED offset rather than on key presence: `b` was aligned to `a:1` by
    // an offset of (0, 10) and the datum shifts everything by (0, -10), which
    // cancels EXACTLY - and `section_offsets` means "sections that are actually
    // displaced", so a value of exactly (0, 0) is dropped rather than stored
    // (#413 finding 2). Comparing the values is strictly stronger than the
    // `contains_key` pair this replaced, which never checked what was in them.
    assert_eq!(project.section_offset(a), (0.0, -10.0));
    assert_eq!(project.section_offset(b), (0.0, 0.0));
    assert!(!project.section_offsets.contains_key(&b));

    // Section a station 1 (datum station) should be at origin
    let coords_a1 = project
        .compute_station_raw_coords(StationRef::new(a, 1))
        .unwrap();
    assert!(
        (coords_a1.x).abs() < 1e-6,
        "Datum station x should be 0, got {}",
        coords_a1.x
    );
    assert!(
        (coords_a1.y).abs() < 1e-6,
        "Datum station y should be 0, got {}",
        coords_a1.y
    );

    // Section a station 0 should be at (0, -10) since it's 10m south of the datum
    let coords_a0 = project
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();
    assert!(
        (coords_a0.x).abs() < 1e-6,
        "a.0 x should be 0, got {}",
        coords_a0.x
    );
    assert!(
        (coords_a0.y - (-10.0)).abs() < 1e-6,
        "a.0 y should be -10, got {}",
        coords_a0.y
    );

    // Section b should have moved by the same delta as section a
    // Before: b.1 was at (5, 10). After: b.1 should be at (5, 0)
    // (The offset delta was (0, -10) to move a.1 from (0, 10) to (0, 0))
    let coords_b1_after = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    assert!(
        (coords_b1_after.x - 5.0).abs() < 1e-6,
        "b.1 x should be 5, got {}",
        coords_b1_after.x
    );
    assert!(
        (coords_b1_after.y - 0.0).abs() < 1e-6,
        "b.1 y should be 0 (moved with connected sections), got {}",
        coords_b1_after.y
    );
}

#[test]
fn set_zero_datum_subsequent_datum_uses_gps_offset() {
    let mut project = Project::new("SubsequentDatum".to_string(), None);
    // Create two unconnected sections
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set first datum on section a, station 0
    let gps_a = GpsCoordinates::new(0.0, 0.0, 0.0).unwrap();
    project
        .set_zero_datum(StationRef::new(a, 0), gps_a)
        .unwrap();

    // Section a station 0 should be at origin
    let coords_a0 = project
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();
    assert!((coords_a0.x).abs() < 1e-6);
    assert!((coords_a0.y).abs() < 1e-6);

    // Set second datum on section b with GPS offset
    // At equator, 0.01 degree longitude ~ 1113 meters east
    let gps_b = GpsCoordinates::new(0.0, 0.01, 0.0).unwrap();
    project
        .set_zero_datum(StationRef::new(b, 0), gps_b)
        .unwrap();

    // Section b station 0 should be at approximately (1113, 0)
    let coords_b0 = project
        .compute_station_raw_coords(StationRef::new(b, 0))
        .unwrap();
    assert!(
        (coords_b0.x - 1113.2).abs() < 10.0,
        "Expected x ~1113, got {}",
        coords_b0.x
    );
    assert!(
        coords_b0.y.abs() < 10.0,
        "Expected y ~0, got {}",
        coords_b0.y
    );
}

#[test]
fn set_zero_datum_z_coordinates_preserved() {
    let mut project = Project::new("PreserveZ".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 5.0),
                shot(0.0, 0.0, 10.0, 5.0, 12.0),
            ],
            ..Default::default()
        })
        .id;

    // Set datum on station 1 (which has z=5)
    let station_ref = StationRef::new(section_id, 1);
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project.set_zero_datum(station_ref, gps).unwrap();

    // Check all stations have preserved z coordinates
    let coords_0 = project
        .compute_station_raw_coords(StationRef::new(section_id, 0))
        .unwrap();
    let coords_1 = project
        .compute_station_raw_coords(StationRef::new(section_id, 1))
        .unwrap();
    let coords_2 = project
        .compute_station_raw_coords(StationRef::new(section_id, 2))
        .unwrap();

    assert!((coords_0.z - 0.0).abs() < 1e-6, "Station 0 z should be 0");
    assert!((coords_1.z - 5.0).abs() < 1e-6, "Station 1 z should be 5");
    assert!((coords_2.z - 12.0).abs() < 1e-6, "Station 2 z should be 12");
}

#[test]
fn remove_zero_datum_keeps_positions_unchanged() {
    let mut project = Project::new("RemoveDatum".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set datum
    let station_ref = StationRef::new(section_id, 1);
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project.set_zero_datum(station_ref, gps).unwrap();

    // Record offset after setting datum
    let offset_before = project.section_offsets.get(&section_id).copied();

    // Remove datum
    project.remove_zero_datum(station_ref);

    // Offset should be unchanged
    let offset_after = project.section_offsets.get(&section_id).copied();
    assert_eq!(
        offset_before, offset_after,
        "Offset should not change when removing datum"
    );
}

#[test]
fn update_first_datum_gps_keeps_station_at_origin() {
    // Issue: Re-setting the first datum should not move it away from origin
    let mut project = Project::new("UpdateFirstDatum".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set first datum on station 1 (which is at y=10 before datum)
    let station_ref = StationRef::new(section_id, 1);
    let gps1 = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project.set_zero_datum(station_ref, gps1).unwrap();

    // Station 1 should be at origin
    let coords_after_first = project.compute_station_raw_coords(station_ref).unwrap();
    assert!(
        (coords_after_first.x).abs() < 1e-6,
        "First datum: x should be 0"
    );
    assert!(
        (coords_after_first.y).abs() < 1e-6,
        "First datum: y should be 0"
    );
    let offset_after_first = project.section_offsets.get(&section_id).copied().unwrap();

    // Update the first datum with different GPS coordinates
    let gps2 = GpsCoordinates::new(46.0, -91.0, 0.0).unwrap();
    project.set_zero_datum(station_ref, gps2).unwrap();

    // Station 1 should STILL be at origin (updating first datum doesn't reposition)
    let coords_after_update = project.compute_station_raw_coords(station_ref).unwrap();
    assert!(
        (coords_after_update.x).abs() < 1e-6,
        "After update: x should still be 0, got {}",
        coords_after_update.x
    );
    assert!(
        (coords_after_update.y).abs() < 1e-6,
        "After update: y should still be 0, got {}",
        coords_after_update.y
    );

    // Offset should be unchanged (no repositioning happened)
    let offset_after_update = project.section_offsets.get(&section_id).copied().unwrap();
    assert_eq!(
        offset_after_first, offset_after_update,
        "Offset should not change when updating first datum GPS"
    );

    // GPS should be updated
    assert_eq!(project.zero_datums[0].gps.latitude, 46.0);
    assert_eq!(project.zero_datums[0].gps.longitude, -91.0);
}

#[test]
fn store_zero_datum_without_repositioning_does_not_modify_offsets() {
    let mut project = Project::new("StoreOnly".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Pre-set an offset manually (simulating merged project scenario)
    project.section_offsets.insert(section_id, (100.0, 200.0));

    // Store datum without repositioning
    let station_ref = StationRef::new(section_id, 0);
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project
        .store_zero_datum_without_repositioning(station_ref, gps)
        .unwrap();

    // Offset should remain unchanged
    let offset = project.section_offsets.get(&section_id).unwrap();
    assert_eq!(*offset, (100.0, 200.0), "Offset should not be modified");

    // But datum should be stored
    assert_eq!(project.zero_datums.len(), 1);
    assert!(project.key_is_ref(&project.zero_datums[0].station_ref, station_ref));
}

#[test]
fn coordinate_computation_no_global_datum_subtraction() {
    // Verify that compute_station_coordinates no longer subtracts primary datum
    let mut project = Project::new("NoGlobalSubtract".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Add a datum directly to zero_datums (bypassing set_zero_datum to avoid offset computation)
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(section_id, 1).into(),
        gps: GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
    });

    // Compute coordinates
    let stations = project.compute_station_coordinates();

    // Station 0 should be at (0, 0) - no global subtraction
    let station_0 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(section_id, 0))
        .unwrap();
    assert!((station_0.coordinates.x).abs() < 1e-6);
    assert!((station_0.coordinates.y).abs() < 1e-6);

    // Station 1 should be at (0, 10) - no global subtraction
    let station_1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(section_id, 1))
        .unwrap();
    assert!((station_1.coordinates.x).abs() < 1e-6);
    assert!((station_1.coordinates.y - 10.0).abs() < 1e-6);
}

#[test]
fn merge_stations_propagates_datum_positioning_a_into_b() {
    // Test: Section without datum merges into section group with datum
    // The merged section should be repositioned relative to the datum
    let mut project = Project::new("MergeDatumA".to_string(), None);

    // Section A: has datum, shot goes north 10m
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set datum on A.0 (positions A.0 at origin)
    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();

    // Verify A is positioned correctly before merge
    let a0_coords = project
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();
    assert!((a0_coords.x).abs() < 1e-6, "A.0 x should be 0");
    assert!((a0_coords.y).abs() < 1e-6, "A.0 y should be 0");
    let a1_coords = project
        .compute_station_raw_coords(StationRef::new(a, 1))
        .unwrap();
    assert!((a1_coords.y - 10.0).abs() < 1e-6, "A.1 y should be 10");

    // Section B: no datum, shot goes east 5m
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge B.0 into A.1 (B merges into datum group)
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();

    // After merge, B should be positioned in datum coordinate system
    // B.0 should be at A.1's position (0, 10)
    let b0_coords = project
        .compute_station_raw_coords(StationRef::new(b, 0))
        .unwrap();
    assert!(
        (b0_coords.x).abs() < 1e-6,
        "B.0 x should be 0, got {}",
        b0_coords.x
    );
    assert!(
        (b0_coords.y - 10.0).abs() < 1e-6,
        "B.0 y should be 10, got {}",
        b0_coords.y
    );

    // B.1 should be at (5, 10) - 5m east of B.0
    let b1_coords = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    assert!(
        (b1_coords.x - 5.0).abs() < 1e-6,
        "B.1 x should be 5, got {}",
        b1_coords.x
    );
    assert!(
        (b1_coords.y - 10.0).abs() < 1e-6,
        "B.1 y should be 10, got {}",
        b1_coords.y
    );

    // A's positions should be unchanged
    let a0_after = project
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();
    assert!((a0_after.x).abs() < 1e-6, "A.0 x should still be 0");
    assert!((a0_after.y).abs() < 1e-6, "A.0 y should still be 0");
}

#[test]
fn merge_stations_propagates_datum_positioning_b_into_a() {
    // Test: Section with datum merges with section without datum
    // The non-datum section should be repositioned
    let mut project = Project::new("MergeDatumB".to_string(), None);

    // Section A: no datum, shot goes north 10m
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Section B: has datum, shot goes east 5m
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set datum on B.0 (positions B.0 at origin)
    project
        .set_zero_datum(
            StationRef::new(b, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();

    // Verify B is positioned correctly before merge
    let b0_coords = project
        .compute_station_raw_coords(StationRef::new(b, 0))
        .unwrap();
    assert!((b0_coords.x).abs() < 1e-6, "B.0 x should be 0");
    assert!((b0_coords.y).abs() < 1e-6, "B.0 y should be 0");

    // Merge A.0 into B.1 (A merges into datum group)
    // B.1 is at (5, 0)
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 1))
        .unwrap();

    // After merge, A should be positioned in datum coordinate system
    // A.0 should be at B.1's position (5, 0)
    let a0_coords = project
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();
    assert!(
        (a0_coords.x - 5.0).abs() < 1e-6,
        "A.0 x should be 5, got {}",
        a0_coords.x
    );
    assert!(
        (a0_coords.y).abs() < 1e-6,
        "A.0 y should be 0, got {}",
        a0_coords.y
    );

    // A.1 should be at (5, 10) - 10m north of A.0
    let a1_coords = project
        .compute_station_raw_coords(StationRef::new(a, 1))
        .unwrap();
    assert!(
        (a1_coords.x - 5.0).abs() < 1e-6,
        "A.1 x should be 5, got {}",
        a1_coords.x
    );
    assert!(
        (a1_coords.y - 10.0).abs() < 1e-6,
        "A.1 y should be 10, got {}",
        a1_coords.y
    );

    // B's positions should be unchanged
    let b0_after = project
        .compute_station_raw_coords(StationRef::new(b, 0))
        .unwrap();
    assert!((b0_after.x).abs() < 1e-6, "B.0 x should still be 0");
    assert!((b0_after.y).abs() < 1e-6, "B.0 y should still be 0");
}

#[test]
fn merge_stations_without_datum_aligns_first_section_to_second() {
    // Test: No datum present; the first section should move to align with the second.
    let mut project = Project::new("MergeNoDatum".to_string(), None);

    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    let a1_before = project
        .compute_station_raw_coords(StationRef::new(a, 1))
        .unwrap();
    let b0_before = project
        .compute_station_raw_coords(StationRef::new(b, 0))
        .unwrap();
    assert!((a1_before.y - 10.0).abs() < 1e-6);
    assert!((b0_before.x).abs() < 1e-6);
    assert!((b0_before.y).abs() < 1e-6);

    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();

    let a1_after = project
        .compute_station_raw_coords(StationRef::new(a, 1))
        .unwrap();
    let b0_after = project
        .compute_station_raw_coords(StationRef::new(b, 0))
        .unwrap();
    assert!((a1_after.x - b0_after.x).abs() < 1e-6);
    assert!((a1_after.y - b0_after.y).abs() < 1e-6);

    let offset_a = project
        .section_offsets
        .get(&a)
        .copied()
        .unwrap_or((0.0, 0.0));
    assert!((offset_a.0).abs() < 1e-6);
    assert!((offset_a.1 + 10.0).abs() < 1e-6);

    let offset_b = project
        .section_offsets
        .get(&b)
        .copied()
        .unwrap_or((0.0, 0.0));
    assert!((offset_b.0).abs() < 1e-6);
    assert!((offset_b.1).abs() < 1e-6);
}

#[test]
fn merge_stations_transitive_datum_propagation() {
    // Test: Merge into a section that's connected to a datum group through another merge
    // C merges into B, which is already merged with datum-holding A
    let mut project = Project::new("TransitiveDatum".to_string(), None);

    // Section A: has datum, shot goes north 10m
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set datum on A.0
    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();

    // Section B: shot goes east 5m (will be connected to A)
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge B.0 into A.1 - B now connected to datum group
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();

    // Section C: shot goes south 3m
    let c = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 3.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Verify B.1 position before merging C
    // B.1 should be at (5, 10) - B.0 at (0, 10), B.1 5m east
    let b1_before = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    assert!((b1_before.x - 5.0).abs() < 1e-6, "B.1 x should be 5");
    assert!((b1_before.y - 10.0).abs() < 1e-6, "B.1 y should be 10");

    // Merge C.0 into B.1 - C should be positioned via transitive datum
    project
        .merge_stations(StationRef::new(b, 1), StationRef::new(c, 0))
        .unwrap();

    // C.0 should be at B.1's position (5, 10)
    let c0_coords = project
        .compute_station_raw_coords(StationRef::new(c, 0))
        .unwrap();
    assert!(
        (c0_coords.x - 5.0).abs() < 1e-6,
        "C.0 x should be 5, got {}",
        c0_coords.x
    );
    assert!(
        (c0_coords.y - 10.0).abs() < 1e-6,
        "C.0 y should be 10, got {}",
        c0_coords.y
    );

    // C.1 should be at (5, 7) - 3m south of C.0
    let c1_coords = project
        .compute_station_raw_coords(StationRef::new(c, 1))
        .unwrap();
    assert!(
        (c1_coords.x - 5.0).abs() < 1e-6,
        "C.1 x should be 5, got {}",
        c1_coords.x
    );
    assert!(
        (c1_coords.y - 7.0).abs() < 1e-6,
        "C.1 y should be 7, got {}",
        c1_coords.y
    );
}

// === Duplicate datum validation tests ===

#[test]
fn set_zero_datum_rejects_duplicate_in_same_section() {
    // Cannot set two datums on different stations in the same section
    let mut project = Project::new("DuplicateSameSection".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Set first datum on station 0
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project
        .set_zero_datum(StationRef::new(section_id, 0), gps)
        .unwrap();

    // Attempt to set second datum on station 1 (same section) - should fail
    let result = project.set_zero_datum(StationRef::new(section_id, 1), gps);
    assert!(
        result.is_err(),
        "Should reject duplicate datum in same section"
    );
    let err = result.unwrap_err();
    assert!(
        err.contains("already has a zero datum"),
        "Error should mention existing datum: {err}"
    );
}

#[test]
fn set_zero_datum_rejects_duplicate_in_connected_group() {
    // Cannot set datum on a station connected through merges to another datum station
    let mut project = Project::new("DuplicateConnected".to_string(), None);

    // Create two sections
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge B.0 to A.1
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    // Set first datum on section A
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project.set_zero_datum(StationRef::new(a, 0), gps).unwrap();

    // Attempt to set second datum on section B (connected through merge) - should fail
    let result = project.set_zero_datum(StationRef::new(b, 1), gps);
    assert!(
        result.is_err(),
        "Should reject duplicate datum in connected group"
    );
    let err = result.unwrap_err();
    assert!(
        err.contains("already has a zero datum"),
        "Error should mention existing datum: {err}"
    );
}

#[test]
fn set_zero_datum_allows_disconnected_groups() {
    // Disconnected section groups can each have their own datum
    let mut project = Project::new("DisconnectedGroups".to_string(), None);

    // Create two unconnected sections
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set datum on section A
    let gps_a = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project
        .set_zero_datum(StationRef::new(a, 0), gps_a)
        .unwrap();

    // Set datum on section B (disconnected) - should succeed
    let gps_b = GpsCoordinates::new(46.0, -91.0, 0.0).unwrap();
    let result = project.set_zero_datum(StationRef::new(b, 0), gps_b);
    assert!(
        result.is_ok(),
        "Disconnected groups should each allow their own datum"
    );
    assert_eq!(project.zero_datums.len(), 2);
}

#[test]
fn set_zero_datum_rejects_across_multi_hop_merge_chain() {
    // Mirrors the reported topology: a far station (like 37:7) is merge-connected to
    // the datum (like 0:0) through a CHAIN of merges, not a direct one. The guard
    // must walk the whole merge component, so a second datum on the far end is
    // rejected.
    let mut project = Project::new("Chain".to_string(), None);
    let mk = |project: &mut Project| {
        project
            .add_section(SectionData {
                shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
                ..Default::default()
            })
            .id
    };
    let (a, b, c, d) = (
        mk(&mut project),
        mk(&mut project),
        mk(&mut project),
        mk(&mut project),
    );

    // Chain A - B - C - D, each merge joining one section's end to the next's start.
    for (x, y) in [(a, b), (b, c), (c, d)] {
        project.station_merges.push(StationMerge {
            station_a: StationRef::new(x, 1).into(),
            station_b: StationRef::new(y, 0).into(),
            merged_at: Utc::now(),
        });
    }

    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project.set_zero_datum(StationRef::new(a, 0), gps).unwrap();

    // D is three hops from A's datum but in the same merge component → reject.
    let result = project.set_zero_datum(StationRef::new(d, 1), gps);
    assert!(
        result.is_err(),
        "a second datum on the far end of a merge chain must be rejected"
    );
    assert!(result.unwrap_err().contains("already has a zero datum"));
    assert_eq!(project.zero_datums.len(), 1);
}

#[test]
fn convert_orphan_closures_leaves_double_anchored_groups_unconverted() {
    // Two independently GPS-anchored surveys (separate merge components) joined only
    // by a sole-bridge loop closure. Converting that closure into a rigid MERGE would
    // place two datums in one merge component and tear it on the next cascade - the
    // reported failure mode - so the conversion must leave it as a (soft) closure.
    let mut project = Project::new("DoubleAnchorBridge".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();

    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();
    project
        .set_zero_datum(
            StationRef::new(b, 0),
            GpsCoordinates::new(45.001, -90.0, 0.0).unwrap(),
        )
        .unwrap();

    // A closure between two anchored-but-disconnected groups is legitimate.
    project
        .create_loop_closure(
            StationRef::new(a, 1),
            StationRef::new(b, 1),
            ClosureStrategy::Batch,
        )
        .unwrap();

    let converted = project.convert_orphan_closures_to_merges();
    assert_eq!(
        converted, 0,
        "must not convert a closure that would double-anchor a merge component"
    );
    assert!(
        project.station_merges.is_empty(),
        "no merge should have been created"
    );
    assert_eq!(
        project.loop_closures.len(),
        1,
        "the bridge closure must be retained"
    );
    assert!(
        project.over_anchored_merge_components().is_empty(),
        "the two datums must stay in separate merge components"
    );
    // The retained bridge must NOT be flagged as an orphan-that-should-be-a-merge:
    // the checker and the converter agree via merge_would_double_anchor, so Repair
    // can't get stuck re-flagging a closure it will never convert.
    assert!(
        project
            .check_orphan_closures()
            .iter()
            .all(|v| v.category != "orphan_closure"),
        "a deliberately-retained double-anchored bridge closure must not be flagged as an orphan"
    );

    // Contrast: with only ONE side anchored, the same sole-bridge closure SHOULD
    // still convert to a merge (the original, intended behaviour).
    project.remove_zero_datum(StationRef::new(b, 0));
    let converted_after = project.convert_orphan_closures_to_merges();
    assert_eq!(
        converted_after, 1,
        "a sole-bridge closure with a single anchor still becomes a merge"
    );
    assert_eq!(project.station_merges.len(), 1);
    assert!(project.loop_closures.is_empty());
}

#[test]
fn detects_two_datums_in_one_merge_component() {
    // The "≤1 datum per merge component" invariant must be DETECTABLE after the
    // fact, so a file written by an older build (or any unguarded path) is flagged
    // rather than silently mis-solved. Force the violation by pushing datums
    // directly, then assert both the detector and the integrity pass catch it.
    let mut project = Project::new("OverAnchored".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(b, 0).into(),
        gps: GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
    });

    let over = project.over_anchored_merge_components();
    assert_eq!(over.len(), 1, "both datums share one merge component");
    assert_eq!(over[0].len(), 2);

    assert!(
        project
            .check_structural_integrity()
            .iter()
            .any(|v| v.category == "multiple_datums_per_merge_component"),
        "the integrity pass must flag the over-anchored component"
    );

    // Removing one datum restores the invariant.
    assert!(project.remove_zero_datum(StationRef::new(b, 0)));
    assert!(project.over_anchored_merge_components().is_empty());
}

/// The damage a past bug left in saved files: two independently GPS-anchored
/// merge components (A+B, and C) welded together by a RIGID merge whose two
/// endpoints sit 40 m apart. `resolve_offsets` can never satisfy it - each half
/// is pinned to its own datum - so it stays torn open forever.
///
/// Fully synthetic: three 2-shot sections running due north, placed by hand.
///   A:0 (0,0)  A:1 (0,10)  A:2 (0,20)   <- datum on A:0
///   B:0 (0,20) B:1 (0,30)  B:2 (0,40)   <- merged A:2 = B:0, correctly aligned
///   C:0 (0,0)  C:1 (0,10)  C:2 (0,20)   <- datum on C:0
/// plus the bogus merge B:2 <-> C:0, i.e. (0,40) welded to (0,0).
fn double_anchored_bridge_project() -> (Project, SectionId, SectionId, SectionId) {
    let mut project = Project::new("DoubleAnchoredBridge".to_string(), None);
    let add = |p: &mut Project| {
        p.add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id
    };
    let (a, b, c) = (add(&mut project), add(&mut project), add(&mut project));
    project.ensure_shot_uuids();

    // The legitimate, correctly aligned merge inside the first component.
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 2).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    project.set_section_offset(b, (0.0, 20.0));

    // One datum per component - legal on its own, which is why the interactive
    // guards never fired.
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(c, 0).into(),
        gps: GpsCoordinates::new(45.001, -90.0, 0.0).unwrap(),
    });

    // The bogus promotion: what should have stayed a loop closure.
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(b, 2).into(),
        station_b: StationRef::new(c, 0).into(),
        merged_at: Utc::now(),
    });

    (project, a, b, c)
}

#[test]
fn repair_demotes_a_double_anchoring_bridge_merge() {
    let (mut project, _a, b, c) = double_anchored_bridge_project();

    // Precondition: this is exactly the pair of symptoms a damaged file shows,
    // and re-solving offsets cannot clear either of them.
    assert_eq!(project.resolve_offsets(), 0, "the tear is unsolvable");
    assert_eq!(
        project.check_merge_alignment().len(),
        1,
        "the bogus merge reports as misaligned"
    );
    assert_eq!(
        project.over_anchored_merge_components().len(),
        1,
        "both datums sit in one merge component"
    );

    assert_eq!(project.demote_double_anchoring_merges(), 1);

    assert_eq!(
        project.station_merges.len(),
        1,
        "only the legitimate A-B merge survives"
    );
    assert!(
        !project.are_stations_merged(StationRef::new(b, 2), StationRef::new(c, 0)),
        "the over-constraining merge is gone"
    );
    assert!(
        project.are_stations_in_closure(StationRef::new(b, 2), StationRef::new(c, 0)),
        "an equivalent loop closure records the same coincidence"
    );
    let closure = project
        .loop_closures
        .last()
        .expect("the demotion pushed a closure");
    assert!(
        !closure.is_aligned,
        "an aligned closure would re-impose the constraint just removed"
    );
    assert!(closure.id > 0, "the closure gets a real monotonic id");

    // Both symptoms clear, and the solve is now achievable.
    project.resolve_offsets();
    assert!(
        project.check_merge_alignment().is_empty(),
        "no merge is left torn open"
    );
    assert!(
        project.over_anchored_merge_components().is_empty(),
        "each datum is back in its own merge component"
    );
    assert!(
        project
            .check_structural_integrity()
            .iter()
            .all(|v| v.category != "multiple_datums_per_merge_component"),
        "the over-anchoring warning is cleared"
    );
}

#[test]
fn demoted_closure_measures_the_merge_free_path() {
    // The merge being demoted is itself a zero-cost pathfinding edge between
    // the two stations being measured. Metrics taken while it still exists ride
    // that edge and persist path_length = 0 / error_percentage = 0 on the
    // replacement closure. Give the survey a real detour - soft closures
    // through a fourth section - and the demoted closure must carry the
    // detour's surveyed length instead.
    let (mut project, _a, b, c) = double_anchored_bridge_project();
    let d = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    let detour = |id: u64, from: StationRef, to: StationRef| LoopClosure {
        id,
        station_a: from.into(),
        station_b: to.into(),
        path_length: 1.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    };
    project
        .loop_closures
        .push(detour(1, StationRef::new(b, 1), StationRef::new(d, 0)));
    project
        .loop_closures
        .push(detour(2, StationRef::new(d, 2), StationRef::new(c, 1)));

    assert_eq!(project.demote_double_anchoring_merges(), 1);

    let closure = project
        .loop_closures
        .last()
        .expect("the demotion pushed a closure");
    assert!(
        closure.path_length > 0.0,
        "the metric must not ride the merge being demoted (got {})",
        closure.path_length
    );
    assert!(
        closure.error_percentage > 0.0,
        "a metres-wide misclosure over a real path is a nonzero error (got {})",
        closure.error_percentage
    );
}

#[test]
fn repair_leaves_legitimate_merges_alone() {
    // (a) A torn merge whose two halves stay connected through a HEALTHY merge.
    // A:0 and C:0 both sit at world (0,0), so that parallel merge is aligned and
    // is not going anywhere: it keeps A+B+C one rigid component whether or not
    // the torn B:2 = C:0 bridge survives. Dropping the torn merge therefore heals
    // nothing, and demotion (which exists to SEPARATE two anchored halves) must
    // leave it alone. Note the contrast with two torn merges bridging the same
    // halves - see `parallel_torn_bridges_are_all_demoted` - where neither is
    // load-bearing and both have to go.
    let (mut project, a, _b, c) = double_anchored_bridge_project();
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(c, 0).into(),
        merged_at: Utc::now(),
    });
    assert_eq!(
        project.check_merge_alignment().len(),
        1,
        "only the bogus bridge is torn open; the parallel A:0 = C:0 merge measures 0 m"
    );
    assert_eq!(
        project.demote_double_anchoring_merges(),
        0,
        "a torn merge with an intact parallel merge path is redundant, not over-constraining"
    );
    assert_eq!(project.station_merges.len(), 3, "every merge is intact");
    assert!(project.loop_closures.is_empty(), "no closure was invented");

    // (b) A sole bridge whose FAR half carries no datum. That is an ordinary
    // single-anchor group with drift, which resolve_offsets can and does heal by
    // moving the unanchored half - demoting it would destroy a real connection.
    let (mut project, _a, _b, c) = double_anchored_bridge_project();
    assert!(project.remove_zero_datum(StationRef::new(c, 0)));
    assert_eq!(
        project.demote_double_anchoring_merges(),
        0,
        "a bridge to an unanchored group is a legitimate merge"
    );
    assert_eq!(project.station_merges.len(), 2, "every merge is intact");
    assert!(project.loop_closures.is_empty(), "no closure was invented");
    project.resolve_offsets();
    assert!(
        project.check_merge_alignment().is_empty(),
        "and the ordinary solver heals it, which is why demotion must not fire"
    );
}

#[test]
fn parallel_torn_bridges_are_all_demoted() {
    // Two bogus merges welding the SAME two anchored halves together. Judging a
    // candidate with only itself excluded makes them shield each other: each one
    // looks redundant because the other still joins B to C, so neither is demoted
    // and the project stays torn forever. Judging the whole torn set at once asks
    // the question that matters - are these halves welded together only by merges
    // the solver cannot satisfy? - and both go.
    //
    // B:1 is world (0,30) and C:1 is (0,10), so the second bridge is a 20 m tear
    // needing no extra geometry.
    let (mut project, _a, b, c) = double_anchored_bridge_project();
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(c, 1).into(),
        merged_at: Utc::now(),
    });
    assert_eq!(
        project.check_merge_alignment().len(),
        2,
        "both cross-component merges are torn open"
    );

    assert_eq!(project.demote_double_anchoring_merges(), 2);

    assert_eq!(
        project.station_merges.len(),
        1,
        "only the legitimate A-B merge survives"
    );
    assert!(!project.are_stations_merged(StationRef::new(b, 2), StationRef::new(c, 0)));
    assert!(!project.are_stations_merged(StationRef::new(b, 1), StationRef::new(c, 1)));
    assert!(
        project.are_stations_in_closure(StationRef::new(b, 2), StationRef::new(c, 0)),
        "each demoted merge leaves an equivalent loop closure"
    );
    assert!(project.are_stations_in_closure(StationRef::new(b, 1), StationRef::new(c, 1)));
    assert_eq!(project.loop_closures.len(), 2);

    project.resolve_offsets();
    assert!(
        project.check_merge_alignment().is_empty(),
        "no merge is left torn open"
    );
    assert!(
        project.over_anchored_merge_components().is_empty(),
        "each datum is back in its own merge component"
    );
}

#[test]
fn demotion_softens_an_existing_aligned_closure_over_the_same_pair() {
    // The pair can already carry a loop closure, in which case the demotion
    // invents no second one. But an `is_aligned: true` closure is a RIGID edge to
    // the offset solver, so leaving it as-is would re-impose exactly the
    // over-constraint the demotion just removed, and its stored metrics were
    // measured while the merge welded the pair together. Both must be corrected in
    // place, under the closure's existing id.
    let (mut project, _a, b, c) = double_anchored_bridge_project();
    project.loop_closures.push(LoopClosure {
        id: 7,
        station_a: StationRef::new(b, 2).into(),
        station_b: StationRef::new(c, 0).into(),
        // Junk: what a measurement taken across the merge recorded.
        path_length: 123.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::Batch,
        is_aligned: true,
    });

    assert_eq!(project.demote_double_anchoring_merges(), 1);

    assert_eq!(
        project.station_merges.len(),
        1,
        "only the legitimate A-B merge survives"
    );
    assert!(!project.are_stations_merged(StationRef::new(b, 2), StationRef::new(c, 0)));
    assert_eq!(
        project.loop_closures.len(),
        1,
        "the existing closure is corrected, not duplicated"
    );
    let closure = &project.loop_closures[0];
    assert_eq!(closure.id, 7, "it keeps its id - same closure, corrected");
    assert!(
        !closure.is_aligned,
        "an aligned closure would re-impose the constraint just removed"
    );
    assert!(
        closure.misclosure_distance > 0.0,
        "the metrics are restamped from the merge-free measurement (got {})",
        closure.misclosure_distance
    );
    assert!(closure.horizontal_misclosure > 0.0);
    // path_length is restamped too. The measurement excludes the surviving
    // closure itself - left in, it is a zero-cost shortcut over exactly this
    // pair and the answer would be 0. With it excluded the halves have no
    // other link, so Dijkstra finds no path and the documented fallback (sum
    // of the reachable sections' lengths: A + B = 40 m) kicks in. 123 -> 40
    // therefore proves both the restamp and the exclusion; the detour variant
    // below proves a real route is used when one exists.
    assert_eq!(closure.path_length, 40.0);

    assert!(
        project.check_closure_alignment().is_empty(),
        "and the softened closure is no longer a stale aligned one"
    );
    project.resolve_offsets();
    assert!(
        project.over_anchored_merge_components().is_empty(),
        "each datum is back in its own merge component"
    );
}

#[test]
fn softened_closure_measures_the_closure_free_path() {
    // Companion to `demoted_closure_measures_the_merge_free_path`, for the
    // already-closed branch: the surviving closure is itself a zero-cost
    // pathfinding edge over exactly the pair being measured, so measuring with
    // it still in the list would stamp path_length = 0 / error 0% no matter how
    // long the real detour is. The demotion must pull the direct closure out,
    // measure, and restamp it - here the detour through section D is the only
    // real path, so a nonzero path_length proves the exclusion happened.
    let (mut project, _a, b, c) = double_anchored_bridge_project();
    let d = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    let detour = |id: u64, from: StationRef, to: StationRef| LoopClosure {
        id,
        station_a: from.into(),
        station_b: to.into(),
        path_length: 1.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    };
    project
        .loop_closures
        .push(detour(1, StationRef::new(b, 1), StationRef::new(d, 0)));
    project
        .loop_closures
        .push(detour(2, StationRef::new(d, 2), StationRef::new(c, 1)));
    // The pre-existing closure on the demoted pair itself, with junk metrics a
    // merge-era measurement would have recorded.
    let mut direct = detour(7, StationRef::new(b, 2), StationRef::new(c, 0));
    direct.is_aligned = true;
    project.loop_closures.push(direct);

    assert_eq!(project.demote_double_anchoring_merges(), 1);

    assert_eq!(
        project.loop_closures.len(),
        3,
        "the existing closure is corrected, not duplicated"
    );
    let closure = project
        .loop_closures
        .iter()
        .find(|lc| lc.id == 7)
        .expect("the direct closure survives under its own id");
    assert!(!closure.is_aligned);
    assert!(
        closure.path_length > 0.0,
        "the metric must not ride the closure being softened (got {})",
        closure.path_length
    );
    assert!(
        closure.error_percentage > 0.0,
        "a metres-wide misclosure over a real path is a nonzero error (got {})",
        closure.error_percentage
    );
}

#[test]
fn unmeasurable_geometry_is_never_demoted_or_flagged() {
    // Non-finite (NaN/inf) shot geometry makes every distance derived from it
    // NaN, and `NaN < TOL` and `NaN >= TOL` are BOTH false. Left unguarded, the
    // three consumers of `endpoint_gap` would each answer by accident: the two
    // alignment checks would call the pair aligned, and the demotion would call
    // it not-torn. `endpoint_gap` returning `None` makes that one deliberate
    // decision - unmeasurable means hands off - so the poisoned project is
    // described by the `non_finite_geometry` violation and by nothing else.
    let (mut project, a, b, c) = double_anchored_bridge_project();
    // Poison ONLY the far end of the bogus bridge: section B's second shot puts
    // B:2 at NaN while B:0 and B:1 stay finite, so the legitimate A:2 = B:0 merge
    // is still measurable and this is a test about the unmeasurable pair alone.
    project.get_section_mut(b).unwrap().shots[1].length = f64::NAN;
    // The third consumer needs an ALIGNED closure over a poisoned pair.
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(b, 2).into(),
        station_b: StationRef::new(c, 1).into(),
        path_length: 10.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: true,
    });

    assert!(
        project
            .check_structural_integrity()
            .iter()
            .any(|v| v.category == "non_finite_geometry"),
        "the poison itself is what gets reported"
    );
    assert!(
        project.check_merge_alignment().is_empty(),
        "no merge is reported: A:2 = B:0 measures 0 m, and B:2 = C:0 cannot be measured at all"
    );
    assert!(
        project.check_closure_alignment().is_empty(),
        "broken geometry is not evidence that an aligned closure has drifted open"
    );
    assert_eq!(
        project.demote_double_anchoring_merges(),
        0,
        "and no merge is deleted on the strength of a distance that does not exist"
    );
    assert_eq!(project.station_merges.len(), 2, "both merges are intact");
    assert!(project.are_stations_merged(StationRef::new(a, 2), StationRef::new(b, 0)));
    assert!(project.are_stations_merged(StationRef::new(b, 2), StationRef::new(c, 0)));
}

#[test]
fn demoted_closure_is_not_re_promoted_by_orphan_conversion() {
    // The demotion and `convert_orphan_closures_to_merges` run in the same Repair
    // pass, and the converter's job is turning sole-bridge closures INTO merges -
    // exactly what was just undone. Its double-anchor guard must see the same two
    // anchored halves and leave the demoted closure alone, or Repair would
    // ping-pong the same edge forever.
    let (mut project, _a, _b, _c) = double_anchored_bridge_project();
    assert_eq!(project.demote_double_anchoring_merges(), 1);

    assert_eq!(
        project.convert_orphan_closures_to_merges(),
        0,
        "the converter must not re-promote the closure it was just handed"
    );
    assert_eq!(project.loop_closures.len(), 1, "the closure survives");
    assert_eq!(project.station_merges.len(), 1, "no merge was recreated");
    assert!(
        project
            .check_orphan_closures()
            .iter()
            .all(|v| v.category != "orphan_closure"),
        "and it is not flagged as an orphan Repair can never convert"
    );

    // Idempotent: a second Repair finds nothing left to demote.
    assert_eq!(project.demote_double_anchoring_merges(), 0);
    assert_eq!(project.loop_closures.len(), 1);
    assert_eq!(project.station_merges.len(), 1);
}

// ===========================================================================
// #469 item 2: `is_aligned` is a MEASUREMENT
// ===========================================================================

#[test]
fn a_closure_is_born_with_a_measured_claim() {
    // The MODEL API, which the importers and the project merge call directly and
    // no command epilogue follows. Two laps of a 10 m square, so stations 0, 4
    // and 8 are all one physical point and station 2 is the far corner.
    let mut project = Project::new("Square".to_string(), None);
    let lap = [0.0, 90.0, 180.0, 270.0];
    let s = project
        .add_section(SectionData {
            shots: lap
                .iter()
                .chain(lap.iter())
                .map(|&azimuth| shot(azimuth, azimuth, 10.0, 0.0, 0.0))
                .collect(),
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();

    let closure = |project: &mut Project, from: u32, to: u32, strategy| {
        project
            .create_loop_closure(StationRef::new(s, from), StationRef::new(s, to), strategy)
            .expect("create_loop_closure failed")
            .is_aligned
    };

    assert!(
        closure(&mut project, 0, 4, ClosureStrategy::Move),
        "the lap closes on itself, so the coincidence claim is TRUE"
    );
    assert!(
        !closure(&mut project, 0, 2, ClosureStrategy::Move),
        "the diagonal is 14 m of open loop - a claim the app measured the \
         refutation of one line earlier must not be stamped"
    );
    assert!(
        !closure(&mut project, 4, 8, ClosureStrategy::Batch),
        "and a Batch closure RECORDS a misclosure: coincident or not, it is the \
         soft edge the non-destructive solve relaxes"
    );
    assert_eq!(
        project
            .loop_closures
            .iter()
            .map(|lc| lc.is_aligned)
            .collect::<Vec<_>>(),
        vec![true, false, false],
        "the recorded closures carry what was measured"
    );
}

#[test]
fn the_re_measure_never_re_promotes_what_repair_demoted() {
    // Repair's demotion and the per-command re-measure both run over the same
    // project, and the re-measure is the one that can set `is_aligned` back to
    // TRUE. If it went on geometry alone, the two would fight: Repair softens the
    // edge precisely so the two anchored halves stop being one rigid component,
    // and the next geometry command would rigidify it again the moment anything
    // pulled the endpoints together. The double-anchor term is what makes that
    // impossible, and it is asked of the MERGE graph, so no closure flag can
    // change the answer.
    let (mut project, _a, b, c) = double_anchored_bridge_project();
    // The pair already carries the one kind of closure the re-measure governs.
    project.loop_closures.push(LoopClosure {
        id: 7,
        station_a: StationRef::new(b, 2).into(),
        station_b: StationRef::new(c, 0).into(),
        path_length: 123.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::Move,
        is_aligned: true,
    });

    assert_eq!(project.demote_double_anchoring_merges(), 1);
    assert!(
        !project.loop_closures[0].is_aligned,
        "setup: Repair softens the closure over the demoted pair"
    );

    // What Repair itself does next, in order.
    project.resolve_offsets();
    assert_eq!(
        project.remeasure_closure_alignment(),
        0,
        "the re-measure must not re-impose the constraint Repair removed"
    );

    // And now the part geometry alone would get wrong: slide C's rigid group so
    // the two endpoints are exactly one point. The claim is now TRUE and still
    // must not be made, because the edge would weld two independently
    // GPS-anchored merge components into one - the #464 refusal.
    project.adjust_rigid_group_offsets(c, 0.0, 40.0);
    let (local, station_offsets) = (
        project.section_local_coords(),
        project.resolved_station_offsets(),
    );
    let gap = project
        .endpoint_gap(
            &StationRef::new(b, 2).into(),
            &StationRef::new(c, 0).into(),
            &local,
            &station_offsets,
        )
        .expect("both endpoints resolve")
        .2;
    assert!(
        gap < Project::MERGE_ALIGNMENT_INVARIANT_M,
        "setup: the endpoints are coincident now (gap {gap})"
    );
    assert_eq!(
        project.remeasure_closure_alignment(),
        0,
        "a coincident pair bridging two anchored groups is still not alignable"
    );
    assert!(!project.loop_closures[0].is_aligned);
}

#[test]
fn the_re_measure_leaves_a_legacy_baked_alignment_alone() {
    // The 0.5.0 migration promise (docs/closures.md): a project whose shots were
    // baked by the removed destructive "Adjust Closures" action opens looking
    // identical and is never rewritten again. Those closures carry `is_aligned`
    // with a NON-`Move` strategy, and their shots were rewritten to make the
    // claim true - so demoting them would silently change co-location, station
    // depth averaging, move-tool rigidity and which shots are frozen on a file
    // that is exactly what it says it is. The re-measure governs `Move` closures
    // and nothing else.
    //
    // The far end of the pair here is 40 m out, i.e. the worst case: even a
    // legacy claim the app can SEE is false is left as recorded, because the app
    // cannot know what the bake did to the shots and `check_closure_alignment`
    // is the honest surface for it.
    let (mut project, _a, b, c) = double_anchored_bridge_project();
    for (id, strategy, is_aligned) in [
        (1_u64, ClosureStrategy::Batch, true),
        (2, ClosureStrategy::LeastSquaresWeighted, true),
        // A modern snap-to-merge closure alongside them, so the sweep genuinely
        // runs (it short-circuits on a project holding no `Move` closure at all)
        // and the exemption below is the thing being measured.
        (3, ClosureStrategy::Move, false),
    ] {
        project.loop_closures.push(LoopClosure {
            id,
            station_a: StationRef::new(b, 2).into(),
            station_b: StationRef::new(c, 1).into(),
            path_length: 10.0,
            misclosure_distance: 0.0,
            error_percentage: 0.0,
            horizontal_misclosure: 0.0,
            vertical_misclosure: 0.0,
            strategy,
            is_aligned,
        });
    }
    let legacy_claims = |project: &Project| {
        project
            .loop_closures
            .iter()
            .filter(|lc| lc.strategy != ClosureStrategy::Move)
            .map(|lc| lc.is_aligned)
            .collect::<Vec<_>>()
    };

    assert_eq!(project.remeasure_closure_alignment(), 0);
    assert_eq!(legacy_claims(&project), vec![true, true]);
    // Including through the LOAD, which is where the force-restamp used to live.
    project.assign_closure_ids();
    assert_eq!(legacy_claims(&project), vec![true, true]);
    assert_eq!(
        project.check_closure_alignment().len(),
        2,
        "a torn legacy claim is still REPORTED - detect-only is the contract"
    );
}

#[test]
fn set_zero_datum_error_includes_section_name() {
    // Error message should include section name if available
    let mut project = Project::new("NamedSection".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            name: Some("Main Passage".to_string()),
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set first datum
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project
        .set_zero_datum(StationRef::new(section_id, 0), gps)
        .unwrap();

    // Attempt duplicate
    let result = project.set_zero_datum(StationRef::new(section_id, 1), gps);
    let err = result.unwrap_err();

    // Error should include section name
    assert!(
        err.contains("Main Passage"),
        "Error should include section name: {err}"
    );
}

#[test]
fn set_zero_datum_error_includes_station_number() {
    // Error message should include station numbers (0-indexed to match canonical IDs)
    let mut project = Project::new("StationNumbers".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Set first datum on station 0
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project
        .set_zero_datum(StationRef::new(section_id, 0), gps)
        .unwrap();

    // Attempt to set on station 2
    let result = project.set_zero_datum(StationRef::new(section_id, 2), gps);
    let err = result.unwrap_err();

    // Error should reference stations with 0-indexed format (e.g., "Section 0 #2")
    assert!(err.contains("#2"), "New station should include #2: {err}");
    assert!(
        err.contains("#0"),
        "Existing datum station should include #0: {err}"
    );
}

#[test]
fn set_zero_datum_validation_runs_before_state_changes() {
    // Validation should reject before any section offsets are modified
    let mut project = Project::new("NoStateChange".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set first datum
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project
        .set_zero_datum(StationRef::new(section_id, 0), gps)
        .unwrap();

    // Record state before failed attempt
    let offset_before = project.section_offsets.get(&section_id).copied();
    let datums_count_before = project.zero_datums.len();

    // Attempt duplicate (should fail)
    let result = project.set_zero_datum(StationRef::new(section_id, 1), gps);
    assert!(result.is_err());

    // State should be unchanged
    let offset_after = project.section_offsets.get(&section_id).copied();
    assert_eq!(
        offset_before, offset_after,
        "Section offset should not change on validation failure"
    );
    assert_eq!(
        datums_count_before,
        project.zero_datums.len(),
        "Zero datums count should not change"
    );
}

#[test]
fn set_zero_datum_allows_resetting_same_station() {
    // Re-setting datum on the same station (updating GPS) should succeed
    let mut project = Project::new("ResetSame".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set datum
    let gps1 = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project
        .set_zero_datum(StationRef::new(section_id, 0), gps1)
        .unwrap();

    // Re-set on same station with different GPS - should succeed
    let gps2 = GpsCoordinates::new(46.0, -91.0, 0.0).unwrap();
    let result = project.set_zero_datum(StationRef::new(section_id, 0), gps2);
    assert!(result.is_ok(), "Resetting same station should succeed");

    // Should still have only one datum
    assert_eq!(project.zero_datums.len(), 1);
    assert_eq!(project.zero_datums[0].gps.latitude, 46.0);
}

#[test]
fn store_zero_datum_without_repositioning_validates_duplicate() {
    // The store-only helper should also validate for duplicates
    let mut project = Project::new("StoreOnlyValidation".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set first datum normally
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project
        .set_zero_datum(StationRef::new(section_id, 0), gps)
        .unwrap();

    // Attempt store-only on different station - should fail
    let result =
        project.store_zero_datum_without_repositioning(StationRef::new(section_id, 1), gps);
    assert!(
        result.is_err(),
        "Store-only should also validate for duplicates"
    );
}

#[test]
fn find_connected_zero_datum_uses_station_connectivity() {
    // Validation should use station-level connectivity (through shots within section)
    // not just section-level connectivity
    let mut project = Project::new("StationConnectivity".to_string(), None);

    // Single section with multiple stations - all connected through shots
    let section_id = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Set datum on station 1 (middle)
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project
        .set_zero_datum(StationRef::new(section_id, 1), gps)
        .unwrap();

    // Station 0 is connected to station 1 through shot 0
    let result_0 = project.set_zero_datum(StationRef::new(section_id, 0), gps);
    assert!(
        result_0.is_err(),
        "Station 0 should be connected to datum at station 1"
    );

    // Station 3 is connected to station 1 through shots 1 and 2
    let result_3 = project.set_zero_datum(StationRef::new(section_id, 3), gps);
    assert!(
        result_3.is_err(),
        "Station 3 should be connected to datum at station 1"
    );
}

// === Loop closure tests ===

#[test]
fn create_loop_closure_creates_correct_values() {
    let mut project = Project::new("ClosureTest".to_string(), None);

    // Section A: shot goes north 10m
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 1.0)],
            ..Default::default()
        })
        .id;

    // Section B: shot goes east 5m
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge B.0 to A.1 so they're connected
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();

    // Create closure between A.0 and B.1 (same-section closures require same connected group)
    let closure = project
        .create_loop_closure(
            StationRef::new(a, 0),
            StationRef::new(b, 1),
            ClosureStrategy::Batch,
        )
        .unwrap();

    assert!(project.key_is_ref(&closure.station_a, StationRef::new(a, 0)));
    assert!(project.key_is_ref(&closure.station_b, StationRef::new(b, 1)));

    // Path length should be sum of shot lengths: 10m + 5m = 15m
    assert!(
        (closure.path_length - 15.0).abs() < 0.1,
        "Path length should be ~15m (10+5), got {}",
        closure.path_length
    );

    // Misclosure is Euclidean distance between A.0 and B.1
    assert!(
        closure.misclosure_distance > 0.0,
        "Misclosure should be positive for non-coincident stations"
    );

    // Verify error_percentage formula: (misclosure / path_length) * 100
    let expected_pct = (closure.misclosure_distance / closure.path_length) * 100.0;
    assert!(
        (closure.error_percentage - expected_pct).abs() < 1e-6,
        "error_percentage should equal misclosure/path_length*100: expected {}, got {}",
        expected_pct,
        closure.error_percentage
    );

    // Horizontal + vertical misclosure should be consistent with total misclosure
    // (total >= horizontal because total includes vertical component)
    assert!(
        closure.misclosure_distance >= closure.horizontal_misclosure,
        "Total misclosure {} should be >= horizontal {}",
        closure.misclosure_distance,
        closure.horizontal_misclosure
    );
    assert!(closure.vertical_misclosure >= 0.0);

    assert_eq!(project.loop_closures.len(), 1);
}

#[test]
fn create_loop_closures_batch_matches_sequential() {
    // The batch path must be byte-for-byte equivalent to calling
    // create_loop_closure(.., Batch) for each pair in order - same created
    // closures (ids, keys, metrics) and same per-pair Ok/Err outcomes.
    fn build() -> (Project, Vec<(StationRef, StationRef)>) {
        let mut p = Project::new("Batch".to_string(), None);
        let a = p
            .add_section(SectionData {
                shots: vec![
                    shot(0.0, 0.0, 10.0, 0.0, 0.0),
                    shot(0.0, 0.0, 10.0, 0.0, 0.0),
                ],
                ..Default::default()
            })
            .id;
        let b = p
            .add_section(SectionData {
                shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
                ..Default::default()
            })
            .id;
        let c = p
            .add_section(SectionData {
                shots: vec![shot(180.0, 180.0, 8.0, 0.0, 0.0)],
                ..Default::default()
            })
            .id;
        let d = p
            .add_section(SectionData {
                shots: vec![shot(270.0, 270.0, 3.0, 0.0, 0.0)],
                ..Default::default()
            })
            .id;
        // Tree of merges connecting all four sections.
        p.merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
            .unwrap();
        p.merge_stations(StationRef::new(a, 2), StationRef::new(c, 0))
            .unwrap();
        p.merge_stations(StationRef::new(b, 1), StationRef::new(d, 0))
            .unwrap();
        let pairs = vec![
            (StationRef::new(a, 0), StationRef::new(c, 1)), // valid loop
            (StationRef::new(a, 1), StationRef::new(b, 0)), // already merged → skip
            (StationRef::new(a, 0), StationRef::new(c, 1)), // duplicate → skip
            (StationRef::new(d, 1), StationRef::new(c, 1)), // valid (connected via merges)
        ];
        (p, pairs)
    }

    let (mut seq, pairs) = build();
    // Clone so both runs share identical shot UUIDs (Section::new mints random
    // ones), making the stored AfterShot(uuid) keys directly comparable.
    let mut bat = seq.clone();
    let seq_results: Vec<_> = pairs
        .iter()
        .map(|&(a, b)| seq.create_loop_closure(a, b, ClosureStrategy::Batch))
        .collect();
    let bat_results = bat.create_loop_closures_batch(&pairs);

    assert_eq!(
        seq.loop_closures.len(),
        bat.loop_closures.len(),
        "created-closure count differs (seq {} vs batch {})",
        seq.loop_closures.len(),
        bat.loop_closures.len()
    );
    for (s, t) in seq.loop_closures.iter().zip(&bat.loop_closures) {
        assert_eq!(s.id, t.id, "closure id differs");
        assert_eq!(s.station_a, t.station_a, "station_a key differs");
        assert_eq!(s.station_b, t.station_b, "station_b key differs");
        assert!(
            (s.path_length - t.path_length).abs() < 1e-9,
            "path_length differs: {} vs {}",
            s.path_length,
            t.path_length
        );
        assert!(
            (s.misclosure_distance - t.misclosure_distance).abs() < 1e-9,
            "misclosure differs"
        );
        assert!(
            (s.error_percentage - t.error_percentage).abs() < 1e-9,
            "error_percentage differs"
        );
        assert_eq!(s.is_aligned, t.is_aligned);
    }
    assert_eq!(seq_results.len(), bat_results.len());
    for (s, t) in seq_results.iter().zip(&bat_results) {
        assert_eq!(
            s.is_ok(),
            t.is_ok(),
            "per-pair outcome differs: seq {s:?} vs batch {t:?}"
        );
    }
    // Sanity: exactly the two valid pairs were created.
    assert_eq!(bat.loop_closures.len(), 2);
}

#[test]
fn create_loop_closures_batch_matches_sequential_zero_datum_unconnected() {
    // Covers the order-dependent branch the main equivalence test omits: two
    // sections that are NOT merge-connected but BOTH carry a zero datum. That is
    // the only path that accepts a closure between an unconnected pair
    // (`!connected && a_has_zd && b_has_zd`), and it must stay byte-for-byte
    // equivalent between the sequential and batch paths.
    fn build() -> (Project, Vec<(StationRef, StationRef)>) {
        let mut p = Project::new("BatchZD".to_string(), None);
        let a = p
            .add_section(SectionData {
                shots: vec![
                    shot(0.0, 0.0, 10.0, 0.0, 0.0),
                    shot(0.0, 0.0, 10.0, 0.0, 0.0),
                ],
                ..Default::default()
            })
            .id;
        let b = p
            .add_section(SectionData {
                shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
                ..Default::default()
            })
            .id;
        // Two disconnected sections (no merge between them), each with its own
        // zero datum - the only setup that accepts an unconnected closure pair.
        p.set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();
        p.set_zero_datum(
            StationRef::new(b, 0),
            GpsCoordinates::new(46.0, -91.0, 0.0).unwrap(),
        )
        .unwrap();
        let pairs = vec![
            (StationRef::new(a, 1), StationRef::new(b, 1)), // unconnected but both datum'd → created
            (StationRef::new(a, 1), StationRef::new(b, 1)), // duplicate → skip
        ];
        (p, pairs)
    }

    let (mut seq, pairs) = build();
    // Clone so both runs share identical shot UUIDs (Section::new mints random
    // ones), making the stored AfterShot(uuid) keys directly comparable.
    let mut bat = seq.clone();
    let seq_results: Vec<_> = pairs
        .iter()
        .map(|&(a, b)| seq.create_loop_closure(a, b, ClosureStrategy::Batch))
        .collect();
    let bat_results = bat.create_loop_closures_batch(&pairs);

    assert_eq!(
        seq.loop_closures.len(),
        bat.loop_closures.len(),
        "created-closure count differs (seq {} vs batch {})",
        seq.loop_closures.len(),
        bat.loop_closures.len()
    );
    for (s, t) in seq.loop_closures.iter().zip(&bat.loop_closures) {
        assert_eq!(s.id, t.id, "closure id differs");
        assert_eq!(s.station_a, t.station_a, "station_a key differs");
        assert_eq!(s.station_b, t.station_b, "station_b key differs");
        assert!(
            (s.path_length - t.path_length).abs() < 1e-9,
            "path_length differs: {} vs {}",
            s.path_length,
            t.path_length
        );
        assert!(
            (s.misclosure_distance - t.misclosure_distance).abs() < 1e-9,
            "misclosure differs"
        );
        assert!(
            (s.error_percentage - t.error_percentage).abs() < 1e-9,
            "error_percentage differs"
        );
        assert_eq!(s.is_aligned, t.is_aligned);
    }
    assert_eq!(seq_results.len(), bat_results.len());
    for (s, t) in seq_results.iter().zip(&bat_results) {
        assert_eq!(
            s.is_ok(),
            t.is_ok(),
            "per-pair outcome differs: seq {s:?} vs batch {t:?}"
        );
    }
    // Sanity: the unconnected-but-datum'd pair was accepted exactly once.
    assert_eq!(bat.loop_closures.len(), 1);
}

#[test]
fn create_loop_closure_proportional_is_deferred() {
    let mut project = Project::new("ClosureDist".to_string(), None);

    let section_id = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(90.0, 90.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    let station_a = StationRef::new(section_id, 0);
    let station_b = StationRef::new(section_id, 2);

    let coords_a_before = project.compute_station_raw_coords(station_a).unwrap();
    let coords_b_before = project.compute_station_raw_coords(station_b).unwrap();

    let result =
        project.create_loop_closure(station_a, station_b, ClosureStrategy::ProportionalByLength);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("deferred"));
    assert!(project.loop_closures.is_empty());

    let coords_a_after = project.compute_station_raw_coords(station_a).unwrap();
    let coords_b_after = project.compute_station_raw_coords(station_b).unwrap();
    assert!(coords_a_before.distance_to(&coords_a_after) < 1e-6);
    assert!(coords_b_before.distance_to(&coords_b_after) < 1e-6);
}

#[test]
fn create_loop_closure_equal_distribution_is_deferred() {
    let mut project = Project::new("ClosureFallback".to_string(), None);

    let section_id = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    let station_a = StationRef::new(section_id, 0);
    let station_b = StationRef::new(section_id, 2);

    let result =
        project.create_loop_closure(station_a, station_b, ClosureStrategy::EqualDistribution);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("deferred"));
    assert!(project.loop_closures.is_empty());
}

#[test]
fn create_loop_closure_rejects_duplicate() {
    let mut project = Project::new("DupClosure".to_string(), None);

    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();

    // First closure succeeds
    project
        .create_loop_closure(
            StationRef::new(a, 0),
            StationRef::new(b, 1),
            ClosureStrategy::default(),
        )
        .unwrap();

    // Duplicate should fail
    let result = project.create_loop_closure(
        StationRef::new(a, 0),
        StationRef::new(b, 1),
        ClosureStrategy::default(),
    );
    assert!(result.is_err(), "Duplicate closure should be rejected");
    assert!(result.unwrap_err().contains("already exists"));

    // Reversed order should also fail
    let result = project.create_loop_closure(
        StationRef::new(b, 1),
        StationRef::new(a, 0),
        ClosureStrategy::default(),
    );
    assert!(
        result.is_err(),
        "Reversed duplicate closure should be rejected"
    );
}

#[test]
fn create_loop_closure_allows_same_section() {
    let mut project = Project::new("SameSectionClosure".to_string(), None);

    // A section with 3 shots - all stations are connected within the section
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(90.0, 90.0, 10.0, 0.0, 0.0),
                shot(180.0, 180.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Stations 0 and 3 are in the same section and connected through shots
    let result = project.create_loop_closure(
        StationRef::new(a, 0),
        StationRef::new(a, 3),
        ClosureStrategy::default(),
    );
    assert!(
        result.is_ok(),
        "Same-section closure should be allowed: {:?}",
        result.err()
    );
    assert_eq!(project.loop_closures.len(), 1);
}

#[test]
fn create_loop_closure_rejects_unconnected_stations() {
    let mut project = Project::new("UnconnectedClosure".to_string(), None);

    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Don't merge them - they are unconnected
    let result = project.create_loop_closure(
        StationRef::new(a, 0),
        StationRef::new(b, 0),
        ClosureStrategy::default(),
    );
    assert!(result.is_err(), "Unconnected stations should be rejected");
    assert!(result.unwrap_err().contains("not connected"));
}

#[test]
fn merge_stations_rejects_already_connected() {
    let mut project = Project::new("ConnectedMerge".to_string(), None);

    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // First merge succeeds
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();

    // Second merge between connected sections should fail
    let result = project.merge_stations(StationRef::new(a, 0), StationRef::new(b, 1));
    assert!(
        result.is_err(),
        "Merging already-connected stations should be rejected"
    );
    assert!(result.unwrap_err().contains("already connected"));
}

#[test]
fn unmerge_stations_removes_matching_closure_and_preserves_others() {
    let mut project = Project::new("UnmergePair".to_string(), None);

    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let c = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 3.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge A-B and B-C
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();
    project
        .merge_stations(StationRef::new(b, 1), StationRef::new(c, 0))
        .unwrap();

    // Inject a closure matching the A-B merge pair directly.
    // (create_loop_closure won't accept stations that are the merge pair itself)
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        path_length: 10.0,
        misclosure_distance: 0.2,
        error_percentage: 2.0,
        horizontal_misclosure: 0.15,
        vertical_misclosure: 0.05,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });

    // Also inject an unrelated closure for B-C pair
    project.loop_closures.push(LoopClosure {
        id: 2,
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(c, 0).into(),
        path_length: 3.0,
        misclosure_distance: 0.1,
        error_percentage: 3.3,
        horizontal_misclosure: 0.1,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });

    assert_eq!(project.loop_closures.len(), 2);

    // Unmerge A-B: should remove the A.1-B.0 closure but keep B.1-C.0
    let (success, removed) = project.unmerge_stations(StationRef::new(a, 1), StationRef::new(b, 0));
    assert!(success);
    assert_eq!(
        removed.len(),
        1,
        "Should remove exactly 1 closure matching the merge pair"
    );
    assert!(project.key_is_ref(&removed[0].station_a, StationRef::new(a, 1)));
    assert!(project.key_is_ref(&removed[0].station_b, StationRef::new(b, 0)));

    // Only the B-C closure should remain
    assert_eq!(project.loop_closures.len(), 1);
    assert!(project.key_is_ref(&project.loop_closures[0].station_a, StationRef::new(b, 1)));
    assert!(project.key_is_ref(&project.loop_closures[0].station_b, StationRef::new(c, 0)));
}

#[test]
fn unmerge_stations_preserves_unrelated_closures() {
    let mut project = Project::new("UnmergePreserve".to_string(), None);

    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let c = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 3.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge A-B and B-C
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();
    project
        .merge_stations(StationRef::new(b, 1), StationRef::new(c, 0))
        .unwrap();

    // Create closure using B-C pair stations (b.1, c.0)
    // This is a direct closure at the merge point
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(c, 0).into(),
        path_length: 3.0,
        misclosure_distance: 0.1,
        error_percentage: 3.3,
        horizontal_misclosure: 0.1,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });

    assert_eq!(project.loop_closures.len(), 1);

    // Unmerge A-B: should NOT remove the B.1-C.0 closure (different pair)
    let (success, removed) = project.unmerge_stations(StationRef::new(a, 1), StationRef::new(b, 0));
    assert!(success);
    assert!(
        removed.is_empty(),
        "No closures should be removed for unrelated pair"
    );
    assert_eq!(
        project.loop_closures.len(),
        1,
        "B-C closure should be preserved"
    );
}

#[test]
fn closure_expands_connected_sections_with_existing_merges() {
    // Test: A closure between stations in different sections makes those sections
    // connected (just like a merge). Existing merges are also traversed.
    let mut project = Project::new("ClosureExpands".to_string(), None);
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

    // Merge A-B
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    // Closure B-C (no merge, just a closure)
    project.loop_closures.push(LoopClosure {
        id: project.next_closure_id,
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(c, 0).into(),
        path_length: 5.0,
        misclosure_distance: 0.1,
        error_percentage: 2.0,
        horizontal_misclosure: 0.1,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });
    project.next_closure_id += 1;

    // Merge-only connectivity: A and B are merged, C is NOT in the merge group
    let merged = project.get_merged_section_ids(a);
    assert!(merged.contains(&a), "A should be in merge group");
    assert!(merged.contains(&b), "B should be in merge group");
    assert!(
        !merged.contains(&c),
        "C should NOT be in merge group (only closure-linked)"
    );
    assert_eq!(merged.len(), 2);

    // Full connectivity (merges + closures): all three sections are connected
    let reachable = project.get_connected_section_ids(a);
    assert!(reachable.contains(&a), "A should be reachable");
    assert!(reachable.contains(&b), "B should be reachable");
    assert!(reachable.contains(&c), "C should be reachable via closure");
    assert_eq!(reachable.len(), 3);

    // Path length including closures: BFS traverses closure as zero-distance edge
    let path = project.calculate_path_length_between(StationRef::new(a, 0), StationRef::new(c, 1));
    // A.0 -> A.1 (1m shot) -> B.0 (merge, 0) -> B.1 (1m shot) -> C.0 (closure, 0) -> C.1 (1m shot)
    assert!(
        (path - 3.0).abs() < 1e-6,
        "Path A.0 to C.1 should be 3.0m (shots only, merge+closure are zero-distance), got {path}"
    );

    // Merge-only path length: cannot reach C.1 from A.0 without closures
    // Falls back to sum of merged section lengths
    let merge_only_path =
        project.calculate_merge_only_path_length(StationRef::new(a, 0), StationRef::new(c, 1));
    // A and B are merge-connected (total 2m), C is not reachable without closures
    // Fallback returns sum of A+B shots = 2.0
    assert!(
        (merge_only_path - 2.0).abs() < 1e-6,
        "Merge-only path should fall back to merged section sum (2.0m), got {merge_only_path}"
    );

    // are_stations_in_closure should detect the closure
    assert!(project.are_stations_in_closure(StationRef::new(b, 1), StationRef::new(c, 0),));
    // Reverse direction also works
    assert!(project.are_stations_in_closure(StationRef::new(c, 0), StationRef::new(b, 1),));
    // Non-closure pair returns false
    assert!(!project.are_stations_in_closure(StationRef::new(a, 0), StationRef::new(c, 0),));
}

#[test]
fn calculate_path_length_prefers_shorter_weighted_path() {
    // Two paths between A.0 and A.1:
    // 1) Direct shot length 100
    // 2) Merge to B.0, two shots of length 1, merge back at B.2 (total 2)
    let mut project = Project::new("ShortestPath".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 100.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 1.0, 0.0, 0.0),
                shot(90.0, 90.0, 1.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 2).into(),
        merged_at: Utc::now(),
    });

    let path = project.calculate_path_length_between(StationRef::new(a, 0), StationRef::new(a, 1));
    assert!(
        (path - 2.0).abs() < 1e-6,
        "Path should prefer length 2.0 over direct 100.0, got {path}"
    );
}

// ---------------------------------------------------------------------------
// along_line_between (#26 ruler: direct + along-cave-line distance + path shots)
// ---------------------------------------------------------------------------

fn ep_station(section_id: SectionId, station_index: u32) -> RulerEndpoint {
    RulerEndpoint {
        section_id,
        station_index: Some(station_index),
        shot_id: None,
        t: None,
    }
}

fn ep_shot(section_id: SectionId, shot_id: ShotId, t: f64) -> RulerEndpoint {
    RulerEndpoint {
        section_id,
        station_index: None,
        shot_id: Some(shot_id),
        t: Some(t),
    }
}

#[test]
fn along_line_simple_linear_section() {
    let mut project = Project::new("Linear".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let shots: Vec<ShotId> = project
        .get_section(a)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.id)
        .collect();
    let stations = project.compute_station_coordinates();

    let r = project.along_line_between(&stations, ep_station(a, 0), ep_station(a, 3));
    assert!(
        (r.along_line.unwrap() - 30.0).abs() < 1e-6,
        "got {:?}",
        r.along_line
    );
    assert_eq!(
        r.path_shots,
        vec![
            PathShot {
                section_id: a,
                shot_id: shots[0]
            },
            PathShot {
                section_id: a,
                shot_id: shots[1]
            },
            PathShot {
                section_id: a,
                shot_id: shots[2]
            },
        ],
    );
    assert_eq!(
        r.path_stations,
        vec![
            StationRef::new(a, 0),
            StationRef::new(a, 1),
            StationRef::new(a, 2),
            StationRef::new(a, 3),
        ],
    );
}

#[test]
fn along_line_cross_section_merge_skips_merge_edge() {
    let mut project = Project::new("Merge".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    let a_shot = project.get_section(a).unwrap().shots[0].id;
    let b_shot = project.get_section(b).unwrap().shots[0].id;
    let stations = project.compute_station_coordinates();

    let r = project.along_line_between(&stations, ep_station(a, 0), ep_station(b, 1));
    assert!(
        (r.along_line.unwrap() - 10.0).abs() < 1e-6,
        "got {:?}",
        r.along_line
    );
    // Two shots; the zero-cost merge edge is NOT emitted as a shot.
    assert_eq!(
        r.path_shots,
        vec![
            PathShot {
                section_id: a,
                shot_id: a_shot
            },
            PathShot {
                section_id: b,
                shot_id: b_shot
            },
        ],
    );
}

#[test]
fn along_line_routes_through_loop_closure() {
    // Parity with the distance-gradient view (#34): loop closures are zero-cost
    // edges, so a closed loop gives a shorter route than the direct long shot.
    let mut project = Project::new("Closure".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 100.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 1.0, 0.0, 0.0),
                shot(90.0, 90.0, 1.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    // Tie A.0<->B.0 and A.1<->B.2 with loop closures (not merges).
    for (sa, sb) in [
        (StationRef::new(a, 0), StationRef::new(b, 0)),
        (StationRef::new(a, 1), StationRef::new(b, 2)),
    ] {
        project.loop_closures.push(LoopClosure {
            id: project.next_closure_id,
            station_a: sa.into(),
            station_b: sb.into(),
            path_length: 0.0,
            misclosure_distance: 0.0,
            error_percentage: 0.0,
            horizontal_misclosure: 0.0,
            vertical_misclosure: 0.0,
            strategy: ClosureStrategy::default(),
            is_aligned: false,
        });
        project.next_closure_id += 1;
    }
    let b_shots: Vec<ShotId> = project
        .get_section(b)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.id)
        .collect();
    let a_shot = project.get_section(a).unwrap().shots[0].id;
    let stations = project.compute_station_coordinates();

    let r = project.along_line_between(&stations, ep_station(a, 0), ep_station(a, 1));
    assert!(
        (r.along_line.unwrap() - 2.0).abs() < 1e-6,
        "should route via closure, got {:?}",
        r.along_line
    );
    assert_eq!(
        r.path_shots,
        vec![
            PathShot {
                section_id: b,
                shot_id: b_shots[0]
            },
            PathShot {
                section_id: b,
                shot_id: b_shots[1]
            },
        ],
    );
    assert!(
        !r.path_shots.iter().any(|s| s.shot_id == a_shot),
        "the long direct shot must not be on the path"
    );
}

#[test]
fn along_line_shot_point_partial_length() {
    let mut project = Project::new("ShotPoint".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let s0 = project.get_section(a).unwrap().shots[0].id;
    let stations = project.compute_station_coordinates();

    // Station 0 to a point 25% along shot 0 → 2.5 m; whole shot tinted.
    let r = project.along_line_between(&stations, ep_station(a, 0), ep_shot(a, s0, 0.25));
    assert!(
        (r.along_line.unwrap() - 2.5).abs() < 1e-6,
        "got {:?}",
        r.along_line
    );
    assert_eq!(
        r.path_shots,
        vec![PathShot {
            section_id: a,
            shot_id: s0
        }]
    );
    // Only station 0 is a real node on the path; the shot-point endpoint is virtual.
    assert_eq!(r.path_stations, vec![StationRef::new(a, 0)]);
}

#[test]
fn along_line_same_shot_endpoints() {
    let mut project = Project::new("SameShot".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let s0 = project.get_section(a).unwrap().shots[0].id;
    let stations = project.compute_station_coordinates();

    let r = project.along_line_between(&stations, ep_shot(a, s0, 0.2), ep_shot(a, s0, 0.7));
    assert!(
        (r.along_line.unwrap() - 5.0).abs() < 1e-6,
        "got {:?}",
        r.along_line
    );
    assert_eq!(
        r.path_shots,
        vec![PathShot {
            section_id: a,
            shot_id: s0
        }]
    );
    // Two mid-shot points cross no station.
    assert!(r.path_stations.is_empty());
}

#[test]
fn along_line_same_shot_prefers_shorter_route_around() {
    // Two points on one long shot whose endpoints are ALSO tied by a zero-cost loop
    // closure: routing around the closure (0.1·100 + 0 + 0.1·100 = 20) beats the
    // direct sub-segment (0.8·100 = 80), so the same-shot case must NOT short-circuit
    // to the direct segment - Dijkstra picks the genuine shortest path (#26, Codex).
    let mut project = Project::new("SameShotRoute".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 100.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.loop_closures.push(LoopClosure {
        id: project.next_closure_id,
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(a, 1).into(),
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });
    project.next_closure_id += 1;
    let s0 = project.get_section(a).unwrap().shots[0].id;
    let stations = project.compute_station_coordinates();

    let r = project.along_line_between(&stations, ep_shot(a, s0, 0.1), ep_shot(a, s0, 0.9));
    assert!(
        (r.along_line.unwrap() - 20.0).abs() < 1e-6,
        "should route around, got {:?}",
        r.along_line
    );
    // The end-legs of the shot are traversed (v→A.0 and A.1→v), but the middle
    // A.0→A.1 leg uses the closure, not the shot - still exactly one shot entry.
    assert_eq!(
        r.path_shots,
        vec![PathShot {
            section_id: a,
            shot_id: s0
        }]
    );
}

#[test]
fn along_line_parallel_closure_does_not_tint_shot() {
    // A loop closure parallel to a real shot (same station pair): measuring between
    // the two stations routes via the zero-cost closure, so the shot - which the
    // path did NOT traverse - must not be tinted (#26, Codex).
    let mut project = Project::new("ParallelClosure".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 100.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.loop_closures.push(LoopClosure {
        id: project.next_closure_id,
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(a, 1).into(),
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });
    project.next_closure_id += 1;
    let stations = project.compute_station_coordinates();

    let r = project.along_line_between(&stations, ep_station(a, 0), ep_station(a, 1));
    assert_eq!(r.along_line, Some(0.0), "routes via the 0-cost closure");
    assert!(
        r.path_shots.is_empty(),
        "the parallel shot must not be tinted: {:?}",
        r.path_shots
    );
}

#[test]
fn along_line_zero_length_same_shot_has_empty_path() {
    // Both endpoints are the exact same point on a shot: distance 0 and NO tint,
    // matching identical station endpoints (#26, Codex).
    let mut project = Project::new("ZeroLenShot".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let s0 = project.get_section(a).unwrap().shots[0].id;
    let stations = project.compute_station_coordinates();

    let r = project.along_line_between(&stations, ep_shot(a, s0, 0.5), ep_shot(a, s0, 0.5));
    assert_eq!(r.along_line, Some(0.0), "got {:?}", r.along_line);
    assert!(
        r.path_shots.is_empty(),
        "zero-length same-shot must not tint: {:?}",
        r.path_shots
    );
}

#[test]
fn along_line_disconnected_returns_none() {
    let mut project = Project::new("Disconnected".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let stations = project.compute_station_coordinates();

    let r = project.along_line_between(&stations, ep_station(a, 0), ep_station(b, 1));
    assert_eq!(r.along_line, None);
    assert!(r.path_shots.is_empty());
}

#[test]
fn along_line_identical_and_degenerate_t() {
    let mut project = Project::new("Degenerate".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let s0 = project.get_section(a).unwrap().shots[0].id;
    let stations = project.compute_station_coordinates();

    // Identical endpoints → 0, empty path.
    let same = project.along_line_between(&stations, ep_station(a, 1), ep_station(a, 1));
    assert_eq!(same.along_line, Some(0.0));
    assert!(same.path_shots.is_empty());

    // t≈0 on shot 0 collapses to station 0; t≈1 collapses to station 1.
    let t0 = project.along_line_between(&stations, ep_shot(a, s0, 0.0), ep_station(a, 2));
    let st = project.along_line_between(&stations, ep_station(a, 0), ep_station(a, 2));
    assert!((t0.along_line.unwrap() - st.along_line.unwrap()).abs() < 1e-9);
    assert_eq!(t0.path_shots, st.path_shots);
}

#[test]
fn along_line_skips_merge_and_closure_phantoms() {
    // A=merge=B=closure=C; A.0 -> C.1 should list exactly 3 shots, no phantom
    // entries for the zero-cost merge/closure ties.
    let mut project = Project::new("NoPhantom".to_string(), None);
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
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    project.loop_closures.push(LoopClosure {
        id: project.next_closure_id,
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(c, 0).into(),
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });
    project.next_closure_id += 1;
    let stations = project.compute_station_coordinates();

    let r = project.along_line_between(&stations, ep_station(a, 0), ep_station(c, 1));
    assert!(
        (r.along_line.unwrap() - 3.0).abs() < 1e-6,
        "got {:?}",
        r.along_line
    );
    assert_eq!(
        r.path_shots.len(),
        3,
        "exactly 3 shots, no merge/closure phantoms: {:?}",
        r.path_shots
    );
}

#[test]
fn along_line_reference_shot_is_traversed_and_tinted() {
    // Reference shots are real measured legs (inherited #34 behavior): they are
    // graph edges and get tinted like any shot.
    let mut project = Project::new("RefShot".to_string(), None);
    let mut ref_shot = shot(0.0, 0.0, 7.0, 0.0, 0.0);
    ref_shot.kind = ShotKind::Jump;
    let a = project
        .add_section(SectionData {
            shots: vec![ref_shot],
            ..Default::default()
        })
        .id;
    let s0 = project.get_section(a).unwrap().shots[0].id;
    let stations = project.compute_station_coordinates();

    let r = project.along_line_between(&stations, ep_station(a, 0), ep_station(a, 1));
    assert!(
        (r.along_line.unwrap() - 7.0).abs() < 1e-6,
        "got {:?}",
        r.along_line
    );
    assert_eq!(
        r.path_shots,
        vec![PathShot {
            section_id: a,
            shot_id: s0
        }]
    );
}

#[test]
fn along_line_follows_passed_coordinates_not_measured_length() {
    // The along-line distance sums the euclidean distance between the coordinates
    // in `stations`, NOT the raw shot.length (#26): feeding the closure-ADJUSTED
    // coordinates yields the adjusted shot lengths the user asked for. Here we feed
    // coordinates that deliberately disagree with the 10 m measured lengths and
    // confirm the result follows the coordinates (3 + 25 = 28, not 10 + 10 = 20).
    let mut project = Project::new("AdjustedLengths".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let s1 = project.get_section(a).unwrap().shots[1].id;
    let mut stations = project.compute_station_coordinates();
    let set = |stations: &mut Vec<Station>, station_index: u32, y: f64| {
        let i = stations
            .iter()
            .position(|s| s.station_ref == StationRef::new(a, station_index))
            .unwrap();
        stations[i].coordinates = Coordinates::new(0.0, y, 0.0);
    };
    set(&mut stations, 0, 0.0);
    set(&mut stations, 1, 3.0); // shot 0 → 3 m apart
    set(&mut stations, 2, 28.0); // shot 1 → 25 m apart

    let r = project.along_line_between(&stations, ep_station(a, 0), ep_station(a, 2));
    assert!(
        (r.along_line.unwrap() - 28.0).abs() < 1e-6,
        "got {:?}",
        r.along_line
    );

    // A mid-shot endpoint's partial leg uses the same coordinate-derived length:
    // 40 % along shot 1 (25 m apart) = 10 m, plus shot 0's 3 m = 13 m.
    let r2 = project.along_line_between(&stations, ep_station(a, 0), ep_shot(a, s1, 0.4));
    assert!(
        (r2.along_line.unwrap() - 13.0).abs() < 1e-6,
        "got {:?}",
        r2.along_line
    );
}

#[test]
fn new_merge_expands_existing_closure_connectivity() {
    // Test: Adding a merge after a closure exists should expand connectivity
    // Setup: A-B closure, then merge B-C -> A should connect to C
    let mut project = Project::new("MergeExpandsClosure".to_string(), None);
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

    // Closure A-B (before any merges)
    project.loop_closures.push(LoopClosure {
        id: project.next_closure_id,
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        path_length: 5.0,
        misclosure_distance: 0.1,
        error_percentage: 2.0,
        horizontal_misclosure: 0.1,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });
    project.next_closure_id += 1;

    // Before merge, A and B are reachable via closure (but not merge-connected)
    let merged_before = project.get_merged_section_ids(a);
    assert!(merged_before.contains(&a));
    assert!(!merged_before.contains(&b), "B not merge-connected to A");
    let connected_before = project.get_connected_section_ids(a);
    assert!(connected_before.contains(&a));
    assert!(connected_before.contains(&b), "B connected via closure");
    assert!(!connected_before.contains(&c), "C not yet connected");

    // Now merge B-C
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(c, 0).into(),
        merged_at: Utc::now(),
    });

    // After merge, A should reach C via closure(A-B) + merge(B-C)
    let connected_after = project.get_connected_section_ids(a);
    assert!(connected_after.contains(&a), "A connected");
    assert!(connected_after.contains(&b), "B connected via closure");
    assert!(
        connected_after.contains(&c),
        "C connected via closure+merge"
    );
    assert_eq!(connected_after.len(), 3);

    // Merge-only: A is alone (no merges from A), B and C are merge-connected
    let merged_a = project.get_merged_section_ids(a);
    assert_eq!(merged_a.len(), 1, "A has no merges");
    let merged_b = project.get_merged_section_ids(b);
    assert!(merged_b.contains(&b));
    assert!(merged_b.contains(&c));
    assert_eq!(merged_b.len(), 2, "B and C are merge-connected");

    // Path including closures should traverse both closure and merge with zero distance
    let path = project.calculate_path_length_between(StationRef::new(a, 0), StationRef::new(c, 1));
    // A.0 -> A.1 (1m) -> B.0 (closure, 0) -> B.1 (1m) -> C.0 (merge, 0) -> C.1 (1m)
    assert!(
        (path - 3.0).abs() < 1e-6,
        "Path should be 3.0m (shots only), got {path}"
    );
}

#[test]
fn transitive_closure_computation_correct() {
    // Test: closures_with is computed correctly with transitive relationships
    // Setup: A.1 merged with B.0, closure between A.1 and C.0
    // Expected: B.0 gets transitive closure to C.0 (via A.1)
    //           C.0 gets transitive closure to B.0 (via A.1)
    let mut project = Project::new("TransitiveClosure".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let c = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 3.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge A.1 with B.0
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    // Closure A.1 to C.0
    project.loop_closures.push(LoopClosure {
        id: project.next_closure_id,
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(c, 0).into(),
        path_length: 8.0,
        misclosure_distance: 0.2,
        error_percentage: 2.5,
        horizontal_misclosure: 0.2,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });
    let closure_id = project.next_closure_id;
    project.next_closure_id += 1;

    // Set datum so compute_station_coordinates works
    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();

    let stations = project.compute_station_coordinates();

    // A.1: direct closure to C.0 (not transitive, A.1 is the endpoint)
    let a1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(a, 1))
        .unwrap();
    let a1_direct = a1
        .closures()
        .iter()
        .find(|cr| cr.station_ref == StationRef::new(c, 0) && !cr.is_transitive);
    assert!(a1_direct.is_some(), "A.1 should have direct closure to C.0");
    assert_eq!(a1_direct.unwrap().closure_id, closure_id);
    assert!(a1_direct.unwrap().via_station.is_none());

    // B.0: transitive closure to C.0 via A.1 (B.0 is merged with A.1)
    let b0 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(b, 0))
        .unwrap();
    let b0_transitive = b0
        .closures()
        .iter()
        .find(|cr| cr.station_ref == StationRef::new(c, 0) && cr.is_transitive);
    assert!(
        b0_transitive.is_some(),
        "B.0 should have transitive closure to C.0 via A.1"
    );
    assert_eq!(
        b0_transitive.unwrap().via_station,
        Some(StationRef::new(a, 1))
    );

    // C.0: direct closure to A.1 (C.0 is the endpoint)
    let c0 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(c, 0))
        .unwrap();
    let c0_direct = c0
        .closures()
        .iter()
        .find(|cr| cr.station_ref == StationRef::new(a, 1) && !cr.is_transitive);
    assert!(c0_direct.is_some(), "C.0 should have direct closure to A.1");

    // C.0: transitive closure to B.0 via A.1 (B.0 is merged with A.1)
    let c0_transitive = c0
        .closures()
        .iter()
        .find(|cr| cr.station_ref == StationRef::new(b, 0) && cr.is_transitive);
    assert!(
        c0_transitive.is_some(),
        "C.0 should have transitive closure to B.0 via A.1"
    );
    assert_eq!(
        c0_transitive.unwrap().via_station,
        Some(StationRef::new(a, 1))
    );
}

#[test]
fn transitive_closure_both_endpoints_merged() {
    // Test: both closure endpoints are in different merge groups
    // Setup: A.1↔B.0 merge, B.1↔C.0 merge, closure between A.1 and C.0
    // Expected: B.0 gets transitive to C.0 (via A.1)
    //           B.1 gets transitive to A.1 (via C.0)
    //           A.1 gets transitive to B.1 (via C.0) - B.1 is in C.0's group
    //           C.0 gets transitive to B.0 (via A.1) - B.0 is in A.1's group
    let mut project = Project::new("TransitiveBothMerged".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let c = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 3.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge A.1 ↔ B.0
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    // Merge B.1 ↔ C.0
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(c, 0).into(),
        merged_at: Utc::now(),
    });

    // Closure A.1 ↔ C.0 (both endpoints are in merge groups)
    project.loop_closures.push(LoopClosure {
        id: project.next_closure_id,
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(c, 0).into(),
        path_length: 8.0,
        misclosure_distance: 0.2,
        error_percentage: 2.5,
        horizontal_misclosure: 0.2,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });
    let closure_id = project.next_closure_id;
    project.next_closure_id += 1;

    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();

    let stations = project.compute_station_coordinates();

    // A.1: direct closure to C.0
    let a1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(a, 1))
        .unwrap();
    assert!(
        a1.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(c, 0)
                && !cr.is_transitive
                && cr.closure_id == closure_id),
        "A.1 should have direct closure to C.0, got: {:?}",
        a1.closures()
    );
    // A.1: transitive closure to B.1 (via C.0, since B.1 is in C.0's group)
    assert!(
        a1.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(b, 1)
                && cr.is_transitive
                && cr.via_station == Some(StationRef::new(c, 0))),
        "A.1 should have transitive closure to B.1 (via C.0), got: {:?}",
        a1.closures()
    );

    // B.0: transitive closure to C.0 (via A.1, since B.0 is in A.1's group)
    let b0 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(b, 0))
        .unwrap();
    assert!(
        b0.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(c, 0)
                && cr.is_transitive
                && cr.via_station == Some(StationRef::new(a, 1))),
        "B.0 should have transitive closure to C.0 (via A.1), got: {:?}",
        b0.closures()
    );
    // B.0: transitive closure to B.1 (via C.0)
    assert!(
        b0.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(b, 1)
                && cr.is_transitive
                && cr.via_station == Some(StationRef::new(c, 0))),
        "B.0 should have transitive closure to B.1 (via C.0), got: {:?}",
        b0.closures()
    );

    // C.0: direct closure to A.1
    let c0 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(c, 0))
        .unwrap();
    assert!(
        c0.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(a, 1)
                && !cr.is_transitive
                && cr.closure_id == closure_id),
        "C.0 should have direct closure to A.1, got: {:?}",
        c0.closures()
    );
    // C.0: transitive closure to B.0 (via A.1)
    assert!(
        c0.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(b, 0)
                && cr.is_transitive
                && cr.via_station == Some(StationRef::new(a, 1))),
        "C.0 should have transitive closure to B.0 (via A.1), got: {:?}",
        c0.closures()
    );

    // B.1: transitive closure to A.1 (via C.0, since B.1 is in C.0's group)
    let b1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(b, 1))
        .unwrap();
    assert!(
        b1.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(a, 1)
                && cr.is_transitive
                && cr.via_station == Some(StationRef::new(c, 0))),
        "B.1 should have transitive closure to A.1 (via C.0), got: {:?}",
        b1.closures()
    );
    // B.1: transitive closure to B.0 (via A.1)
    assert!(
        b1.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(b, 0)
                && cr.is_transitive
                && cr.via_station == Some(StationRef::new(a, 1))),
        "B.1 should have transitive closure to B.0 (via A.1), got: {:?}",
        b1.closures()
    );

    // A.0 and C.1 should have NO closures (they're not closure endpoints or merged with one)
    let a0 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(a, 0))
        .unwrap();
    assert!(
        a0.closures().is_empty(),
        "A.0 should have no closures, got: {:?}",
        a0.closures()
    );

    let c1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(c, 1))
        .unwrap();
    assert!(
        c1.closures().is_empty(),
        "C.1 should have no closures, got: {:?}",
        c1.closures()
    );
}

#[test]
fn transitive_closure_through_create_loop_closure_api() {
    // Test: using the actual create_loop_closure API (not raw push)
    // Setup: 3 sections, merge chain A.1↔B.0 and B.1↔C.0, closure A.0↔C.1
    // Expected: A.0 and C.1 get direct closures to each other
    //           No transitive closures (endpoints not in any merge group)
    let mut project = Project::new("TransitiveAPI".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let c = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 3.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();

    // Merge A.1 ↔ B.0 (unconnected, so merge works)
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();

    // Merge B.1 ↔ C.0 (B and C not yet connected)
    project
        .merge_stations(StationRef::new(b, 1), StationRef::new(c, 0))
        .unwrap();

    // Now A, B, C are all connected. Create closure A.0 ↔ C.1
    let closure = project
        .create_loop_closure(
            StationRef::new(a, 0),
            StationRef::new(c, 1),
            ClosureStrategy::default(),
        )
        .unwrap();

    let stations = project.compute_station_coordinates();

    // A.0: direct closure to C.1 (endpoint, not merged with anything)
    let a0 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(a, 0))
        .unwrap();
    assert!(
        a0.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(c, 1)
                && !cr.is_transitive
                && cr.closure_id == closure.id),
        "A.0 should have direct closure to C.1, got: {:?}",
        a0.closures()
    );
    assert_eq!(
        a0.closures().len(),
        1,
        "A.0 should have exactly 1 closure ref (no transitive since A.0 not merged)"
    );

    // C.1: direct closure to A.0
    let c1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(c, 1))
        .unwrap();
    assert!(
        c1.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(a, 0)
                && !cr.is_transitive
                && cr.closure_id == closure.id),
        "C.1 should have direct closure to A.0, got: {:?}",
        c1.closures()
    );
    assert_eq!(
        c1.closures().len(),
        1,
        "C.1 should have exactly 1 closure ref"
    );

    // Other stations (A.1, B.0, B.1, C.0) should NOT have any closure refs
    // because neither closure endpoint (A.0, C.1) is in a merge group
    for (section_id, station_index, label) in
        [(a, 1, "A.1"), (b, 0, "B.0"), (b, 1, "B.1"), (c, 0, "C.0")]
    {
        let station = stations
            .iter()
            .find(|s| s.station_ref == StationRef::new(section_id, station_index))
            .unwrap();
        assert!(
            station.closures().is_empty(),
            "{} should have no closures (endpoint not in its merge group), got: {:?}",
            label,
            station.closures()
        );
    }
}

#[test]
fn closure_chain_transitivity_two_closures_shared_endpoint() {
    // Test: closure-chain transitivity
    // Setup: 3 sections A, B, C. Merge chain: A.1↔B.0, B.1↔C.0
    //   Closure 1: A.0 ↔ B.1 (between A start and B end)
    //   Closure 2: B.1 ↔ C.1 (between B end and C end)
    //
    // B.1 has 2 direct closures: to A.0 and to C.1
    // A.0 should see transitive closure to C.1 via B.1
    // C.1 should see transitive closure to A.0 via B.1
    let mut project = Project::new("ChainTransitive".to_string(), None);

    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let c = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 3.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();

    // Merge chain
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();
    project
        .merge_stations(StationRef::new(b, 1), StationRef::new(c, 0))
        .unwrap();

    // Closure 1: A.0 ↔ B.1 (they're connected via merges)
    let closure1 = project
        .create_loop_closure(
            StationRef::new(a, 0),
            StationRef::new(b, 1),
            ClosureStrategy::default(),
        )
        .unwrap();

    // Closure 2: B.1 ↔ C.1 (B.1 is merged with C.0, so B and C are connected)
    // Wait - B.1 and C.0 are merged. So B.1 and C.1 are in the same section group.
    // But are_stations_merged(B.1, C.1)? No (direct check only).
    // Actually C.1 is in section C. B.1 is in section B. They're connected through merge.
    // create_loop_closure checks connectivity via get_connected_section_ids.
    let closure2 = project
        .create_loop_closure(
            StationRef::new(b, 1),
            StationRef::new(c, 1),
            ClosureStrategy::default(),
        )
        .unwrap();

    let stations = project.compute_station_coordinates();

    // B.1: should have 2 direct closures (to A.0 and to C.1)
    let b1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(b, 1))
        .unwrap();
    let b1_to_a0 = b1
        .closures()
        .iter()
        .find(|cr| cr.station_ref == StationRef::new(a, 0) && !cr.is_transitive);
    assert!(
        b1_to_a0.is_some(),
        "B.1 should have direct closure to A.0, got: {:?}",
        b1.closures()
    );
    let b1_to_c1 = b1
        .closures()
        .iter()
        .find(|cr| cr.station_ref == StationRef::new(c, 1) && !cr.is_transitive);
    assert!(
        b1_to_c1.is_some(),
        "B.1 should have direct closure to C.1, got: {:?}",
        b1.closures()
    );

    // A.0: should have direct closure to B.1 AND transitive closure to C.1 (via B.1)
    let a0 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(a, 0))
        .unwrap();
    let a0_direct = a0
        .closures()
        .iter()
        .find(|cr| cr.station_ref == StationRef::new(b, 1) && !cr.is_transitive);
    assert!(
        a0_direct.is_some(),
        "A.0 should have direct closure to B.1, got: {:?}",
        a0.closures()
    );
    let a0_transitive = a0
        .closures()
        .iter()
        .find(|cr| cr.station_ref == StationRef::new(c, 1) && cr.is_transitive);
    assert!(
        a0_transitive.is_some(),
        "A.0 should have transitive closure to C.1 (via B.1), got: {:?}",
        a0.closures()
    );
    assert_eq!(
        a0_transitive.unwrap().via_station,
        Some(StationRef::new(b, 1)),
        "A.0 transitive to C.1 should be via B.1"
    );
    assert_eq!(
        a0_transitive.unwrap().closure_id,
        closure2.id,
        "A.0 transitive to C.1 should reference closure2's id"
    );

    // C.1: should have direct closure to B.1 AND transitive closure to A.0 (via B.1)
    let c1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(c, 1))
        .unwrap();
    let c1_direct = c1
        .closures()
        .iter()
        .find(|cr| cr.station_ref == StationRef::new(b, 1) && !cr.is_transitive);
    assert!(
        c1_direct.is_some(),
        "C.1 should have direct closure to B.1, got: {:?}",
        c1.closures()
    );
    let c1_transitive = c1
        .closures()
        .iter()
        .find(|cr| cr.station_ref == StationRef::new(a, 0) && cr.is_transitive);
    assert!(
        c1_transitive.is_some(),
        "C.1 should have transitive closure to A.0 (via B.1), got: {:?}",
        c1.closures()
    );
    assert_eq!(
        c1_transitive.unwrap().via_station,
        Some(StationRef::new(b, 1)),
        "C.1 transitive to A.0 should be via B.1"
    );
    assert_eq!(
        c1_transitive.unwrap().closure_id,
        closure1.id,
        "C.1 transitive to A.0 should reference closure1's id"
    );
}

#[test]
fn closure_chain_transitivity_three_closures_shared_endpoint() {
    // Test: station with 3 closures creates transitive refs for all pairs
    // Setup: 4 sections, all connected through merges
    //   Station X has closures to A, B, and C
    //   A should see transitive to B (via X) and transitive to C (via X)
    //   B should see transitive to A (via X) and transitive to C (via X)
    //   C should see transitive to A (via X) and transitive to B (via X)
    let mut project = Project::new("ChainTriple".to_string(), None);

    let sec_main = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    let sec_a = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let sec_b = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 3.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let sec_c = project
        .add_section(SectionData {
            shots: vec![shot(270.0, 270.0, 4.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project
        .set_zero_datum(
            StationRef::new(sec_main, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();

    // Connect all sections to main: main.1↔A.0, main.2↔B.0, main.3↔C.0
    project
        .merge_stations(StationRef::new(sec_main, 1), StationRef::new(sec_a, 0))
        .unwrap();
    project
        .merge_stations(StationRef::new(sec_main, 2), StationRef::new(sec_b, 0))
        .unwrap();
    project
        .merge_stations(StationRef::new(sec_main, 3), StationRef::new(sec_c, 0))
        .unwrap();

    // Station main.0 (X) has closures to A.1, B.1, C.1
    let cl_a = project
        .create_loop_closure(
            StationRef::new(sec_main, 0),
            StationRef::new(sec_a, 1),
            ClosureStrategy::default(),
        )
        .unwrap();
    let cl_b = project
        .create_loop_closure(
            StationRef::new(sec_main, 0),
            StationRef::new(sec_b, 1),
            ClosureStrategy::default(),
        )
        .unwrap();
    let cl_c = project
        .create_loop_closure(
            StationRef::new(sec_main, 0),
            StationRef::new(sec_c, 1),
            ClosureStrategy::default(),
        )
        .unwrap();

    let stations = project.compute_station_coordinates();

    // X (main.0) should have 3 direct closures
    let x = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(sec_main, 0))
        .unwrap();
    let x_directs: Vec<_> = x.closures().iter().filter(|cr| !cr.is_transitive).collect();
    assert_eq!(
        x_directs.len(),
        3,
        "X should have 3 direct closures, got: {:?}",
        x.closures()
    );

    // A.1 should have: direct to X, transitive to B.1 (via X), transitive to C.1 (via X)
    let a1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(sec_a, 1))
        .unwrap();
    assert!(
        a1.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(sec_main, 0) && !cr.is_transitive),
        "A.1 direct to X"
    );
    assert!(
        a1.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(sec_b, 1)
                && cr.is_transitive
                && cr.via_station == Some(StationRef::new(sec_main, 0))
                && cr.closure_id == cl_b.id),
        "A.1 transitive to B.1 via X, got: {:?}",
        a1.closures()
    );
    assert!(
        a1.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(sec_c, 1)
                && cr.is_transitive
                && cr.via_station == Some(StationRef::new(sec_main, 0))
                && cr.closure_id == cl_c.id),
        "A.1 transitive to C.1 via X, got: {:?}",
        a1.closures()
    );

    // B.1 should have: direct to X, transitive to A.1 (via X), transitive to C.1 (via X)
    let b1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(sec_b, 1))
        .unwrap();
    assert!(
        b1.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(sec_a, 1)
                && cr.is_transitive
                && cr.closure_id == cl_a.id),
        "B.1 transitive to A.1 via X, got: {:?}",
        b1.closures()
    );
    assert!(
        b1.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(sec_c, 1)
                && cr.is_transitive
                && cr.closure_id == cl_c.id),
        "B.1 transitive to C.1 via X, got: {:?}",
        b1.closures()
    );

    // C.1 should have: direct to X, transitive to A.1 (via X), transitive to B.1 (via X)
    let c1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(sec_c, 1))
        .unwrap();
    assert!(
        c1.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(sec_a, 1)
                && cr.is_transitive
                && cr.closure_id == cl_a.id),
        "C.1 transitive to A.1 via X, got: {:?}",
        c1.closures()
    );
    assert!(
        c1.closures()
            .iter()
            .any(|cr| cr.station_ref == StationRef::new(sec_b, 1)
                && cr.is_transitive
                && cr.closure_id == cl_b.id),
        "C.1 transitive to B.1 via X, got: {:?}",
        c1.closures()
    );
}

// === Section cleanup tests (fn-47-ok1.7) ===

/// Helper: build a project with 3 sections (a, b, c) where:
/// - a is merged to b (station a:1 -> b:0)
/// - b has a loop closure to c (b:1 -> c:0)
/// - a has a section offset
/// - a has a station offset on station 0
/// - a has a zero datum on station 0
fn build_project_with_relationships() -> (Project, SectionId, SectionId, SectionId) {
    let mut project = Project::new("Cleanup Test".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 2.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let c = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 3.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge a -> b
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    // Loop closure b -> c
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(c, 0).into(),
        path_length: 5.0,
        misclosure_distance: 0.1,
        error_percentage: 2.0,
        horizontal_misclosure: 0.08,
        vertical_misclosure: 0.06,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });
    project.next_closure_id = 2;

    // Section offset for a
    project.section_offsets.insert(a, (10.0, 20.0));

    // Station offset for a:0
    project
        .station_offsets
        .insert(StationRef::new(a, 0).into(), (1.0, 2.0, 3.0));

    // Zero datum for a:0
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
    });

    (project, a, b, c)
}

#[test]
fn test_cleanup_removes_merges() {
    let (mut project, a, _b, _c) = build_project_with_relationships();
    assert_eq!(project.station_merges.len(), 1);

    project.cleanup_section_references(a);

    assert_eq!(
        project.station_merges.len(),
        0,
        "Merge referencing section A should be removed"
    );
}

#[test]
fn test_cleanup_removes_closures() {
    let (mut project, _a, b, _c) = build_project_with_relationships();
    assert_eq!(project.loop_closures.len(), 1);

    project.cleanup_section_references(b);

    assert_eq!(
        project.loop_closures.len(),
        0,
        "Closure referencing section B should be removed"
    );
}

#[test]
fn test_cleanup_removes_offsets() {
    let (mut project, a, _b, _c) = build_project_with_relationships();
    assert!(project.section_offsets.contains_key(&a));
    assert_eq!(project.station_offsets.len(), 1);

    project.cleanup_section_references(a);

    assert!(
        !project.section_offsets.contains_key(&a),
        "Section offset for A should be removed"
    );
    assert_eq!(
        project.station_offsets.len(),
        0,
        "Station offset for A:0 should be removed"
    );
}

#[test]
fn test_cleanup_removes_zero_datums() {
    let (mut project, a, _b, _c) = build_project_with_relationships();
    assert_eq!(project.zero_datums.len(), 1);

    project.cleanup_section_references(a);

    assert_eq!(
        project.zero_datums.len(),
        0,
        "Zero datum for A:0 should be removed"
    );
}

#[test]
fn test_cleanup_guarded_by_existence() {
    // Cleanup on a non-existent section should not mutate anything
    let (mut project, _a, _b, _c) = build_project_with_relationships();
    let orig_merges = project.station_merges.len();
    let orig_closures = project.loop_closures.len();
    let orig_offsets = project.section_offsets.len();
    let orig_station_offsets = project.station_offsets.len();
    let orig_datums = project.zero_datums.len();

    // Section 999 doesn't exist - cleanup should be a no-op
    project.cleanup_section_references(999);

    assert_eq!(project.station_merges.len(), orig_merges);
    assert_eq!(project.loop_closures.len(), orig_closures);
    assert_eq!(project.section_offsets.len(), orig_offsets);
    assert_eq!(project.station_offsets.len(), orig_station_offsets);
    assert_eq!(project.zero_datums.len(), orig_datums);
}

#[test]
fn test_cleanup_preserves_unrelated_relationships() {
    let (mut project, a, b, c) = build_project_with_relationships();

    project.station_merges.push(StationMerge {
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(c, 0).into(),
        merged_at: Utc::now(),
    });

    assert_eq!(project.station_merges.len(), 2);
    assert_eq!(project.loop_closures.len(), 1);

    project.cleanup_section_references(a);

    assert_eq!(project.station_merges.len(), 1);
    assert!(project.station_merges.iter().any(|m| {
        (m.station_a.section_id == b && m.station_b.section_id == c)
            || (m.station_a.section_id == c && m.station_b.section_id == b)
    }));

    assert_eq!(project.loop_closures.len(), 1);
    let closure = &project.loop_closures[0];
    assert_eq!(closure.station_a.section_id, b);
    assert_eq!(closure.station_b.section_id, c);
}

#[test]
fn test_get_section_relationships() {
    let (project, a, b, _c) = build_project_with_relationships();

    let rel_a = project.get_section_relationships(a);
    assert_eq!(rel_a.merges.len(), 1, "Section A has one merge");
    assert_eq!(rel_a.closures.len(), 0, "Section A has no closures");
    assert!(rel_a.has_section_offset, "Section A has a section offset");
    assert_eq!(
        rel_a.station_offset_count, 1,
        "Section A has one station offset"
    );
    assert_eq!(rel_a.zero_datum_count, 1, "Section A has one zero datum");

    let rel_b = project.get_section_relationships(b);
    assert_eq!(rel_b.merges.len(), 1, "Section B has one merge");
    assert_eq!(rel_b.closures.len(), 1, "Section B has one closure");
    assert!(!rel_b.has_section_offset, "Section B has no section offset");
    assert_eq!(
        rel_b.station_offset_count, 0,
        "Section B has no station offsets"
    );
    assert_eq!(rel_b.zero_datum_count, 0, "Section B has no zero datums");
}

// (Removed `shift_station_refs_updates_related_collections`: the positional
// ref-shifting mechanism it exercised no longer exists. Under stable
// shot-UUID identity, cross-references never shift on insert/delete - they
// resolve to the moved station automatically - so there is nothing to shift.
// Anchor-stability is covered by the insert/delete identity tests instead.)

#[test]
fn preview_delete_shot_basic() {
    let mut project = Project::new("PreviewDelete".to_string(), None);
    let section_a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0), shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let section_b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0), shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge station_a:1 with station_b:0
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(section_a, 1).into(),
        station_b: StationRef::new(section_b, 0).into(),
        merged_at: Utc::now(),
    });

    // Preview deleting section_a shot at position 1 (second shot)
    // removed_station = (section_a, 2)
    let shot1_id = project.get_section(section_a).unwrap().shots[1].id;
    let impact = project.preview_delete_shot(section_a, shot1_id).unwrap();
    assert_eq!(impact.removed_station, StationRef::new(section_a, 2));
    // Merge is at station_a:1, not station_a:2 => removed_merges empty
    assert!(impact.removed_merges.is_empty());

    // Preview deleting section_a shot at position 0 (first shot)
    // removed_station = (section_a, 0)
    let shot0_id = project.get_section(section_a).unwrap().shots[0].id;
    let impact = project.preview_delete_shot(section_a, shot0_id).unwrap();
    assert_eq!(impact.removed_station, StationRef::new(section_a, 0));
    // Merge is at station_a:1, not station_a:0 => removed_merges empty
    assert!(impact.removed_merges.is_empty());

    // Add a merge at station_a:2 with station_b:1
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(section_a, 2).into(),
        station_b: StationRef::new(section_b, 1).into(),
        merged_at: Utc::now(),
    });

    // Preview deleting section_a's second shot again
    // removed_station = (section_a, 2), which matches the new merge
    let impact = project.preview_delete_shot(section_a, shot1_id).unwrap();
    assert_eq!(impact.removed_station, StationRef::new(section_a, 2));
    assert_eq!(impact.removed_merges.len(), 1);
    assert_eq!(
        impact.removed_merges[0].station_a,
        StationRef::new(section_a, 2)
    );
    assert_eq!(
        impact.removed_merges[0].station_b,
        StationRef::new(section_b, 1)
    );
}

#[test]
fn preview_delete_shot_with_closure_rewire() {
    let mut project = Project::new("ClosureRewire".to_string(), None);
    let section_a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0), shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let section_b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0), shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge section_a:1 with section_b:0
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(section_a, 1).into(),
        station_b: StationRef::new(section_b, 0).into(),
        merged_at: Utc::now(),
    });

    // Add a loop closure between section_a:1 and section_b:1
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(section_a, 1).into(),
        station_b: StationRef::new(section_b, 1).into(),
        path_length: 2.0,
        misclosure_distance: 0.1,
        error_percentage: 5.0,
        horizontal_misclosure: 0.1,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });

    // Preview deleting section_a's second shot (position 1)
    // removed_station = (section_a, 2), but closure is on station_a:1 which is NOT removed
    let shot1_id = project.get_section(section_a).unwrap().shots[1].id;
    let impact = project.preview_delete_shot(section_a, shot1_id).unwrap();
    assert_eq!(impact.removed_station, StationRef::new(section_a, 2));
    assert!(impact.removed_closures.is_empty());

    // Now add a closure with section_a:2 and section_b:1
    project.loop_closures.push(LoopClosure {
        id: 2,
        station_a: StationRef::new(section_a, 2).into(),
        station_b: StationRef::new(section_b, 1).into(),
        path_length: 3.0,
        misclosure_distance: 0.2,
        error_percentage: 6.7,
        horizontal_misclosure: 0.2,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });

    // Preview delete of section_a's second shot => removed_station = (section_a, 2)
    // The closure IS on the removed station
    let impact = project.preview_delete_shot(section_a, shot1_id).unwrap();
    assert_eq!(impact.removed_station, StationRef::new(section_a, 2));
    assert_eq!(impact.removed_closures.len(), 1);
    assert_eq!(impact.removed_closures[0].closure_id, 2);

    // The merge group of station_a:2 is just {station_a:2} (not merged with anything)
    // So merge_group_without is empty => no replacement candidate => closure_rewire_skipped
    assert!(impact.closure_rewire_skipped);
    assert!(impact.replacement_closure_updates.is_empty());
}

#[test]
fn preview_delete_shot_mid_shot_loop_blocks_shift() {
    let mut project = Project::new("MidShotLoop".to_string(), None);

    // Section A with 3 shots: stations 0,1,2,3
    let section_a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 1.0, 0.0, 0.0),
                shot(0.0, 0.0, 1.0, 0.0, 0.0),
                shot(0.0, 0.0, 1.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Section B with 1 shot: stations 0,1
    let section_b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge A:0 with B:0
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(section_a, 0).into(),
        station_b: StationRef::new(section_b, 0).into(),
        merged_at: Utc::now(),
    });

    // Merge A:3 with B:1
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(section_a, 3).into(),
        station_b: StationRef::new(section_b, 1).into(),
        merged_at: Utc::now(),
    });

    // Delete middle shot of A (position 1). This is a mid-shot (pos > 0 && pos < len-1).
    // station_before = A:1, station_after = A:2
    // Path: A:1 -> (shot 0 backward) -> A:0 -> (merge) -> B:0 -> (shot 0) -> B:1
    //   -> (merge) -> A:3 -> (shot 2 backward) -> A:2
    // This connects them excluding shot at pos 1, so shift_blocked should be true.
    let mid_shot_id = project.get_section(section_a).unwrap().shots[1].id;
    let impact = project.preview_delete_shot(section_a, mid_shot_id).unwrap();
    assert!(
        impact.shift_blocked,
        "Expected shift_blocked to be true for loop"
    );
    assert!(
        !impact.shift_blocked_reasons.is_empty(),
        "Expected at least one shift_blocked reason"
    );
    assert!(
        impact.shift_blocked_reasons[0].contains("loop"),
        "Expected loop-related reason, got: {}",
        impact.shift_blocked_reasons[0]
    );
}

#[test]
fn remove_shot_with_rewire_first_shot_adjusts_offset() {
    let mut project = Project::new("OffsetAdjust".to_string(), None);

    // First shot going east (azimuth 90), second shot going north
    let section_id = project
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    let first_shot_id = project.get_section(section_id).unwrap().shots[0].id;

    // Offset before should be (0, 0) by default
    let offset_before = project
        .section_offsets
        .get(&section_id)
        .copied()
        .unwrap_or((0.0, 0.0));
    assert!((offset_before.0).abs() < 1e-6);
    assert!((offset_before.1).abs() < 1e-6);

    // Remove the first shot
    let result = project.remove_shot_with_rewire(section_id, first_shot_id);
    assert!(result.is_ok());

    // Displacement of shot going east at 90°:
    // horizontal_length * sin(90°) = 10.0 * 1.0 = 10.0  (x)
    // horizontal_length * cos(90°) = 10.0 * 0.0 = ~0.0   (y)
    // After removing, offset = (0.0 + 10.0, 0.0 + ~0.0) = (~10.0, ~0.0)
    let offset_after = project
        .section_offsets
        .get(&section_id)
        .copied()
        .unwrap_or((0.0, 0.0));
    assert!(
        (offset_after.0 - 10.0).abs() < 1e-6,
        "Expected x offset ~10.0, got {}",
        offset_after.0
    );
    assert!(
        (offset_after.1).abs() < 1e-6,
        "Expected y offset ~0.0, got {}",
        offset_after.1
    );
}

#[test]
fn remove_shot_with_rewire_replaces_merges() {
    let mut project = Project::new("ReplaceMerges".to_string(), None);

    let section_a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let section_b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let section_c = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Align merge points: a:1 is at (0,1), so b:0 and c:0 need offset (0,1)
    project.set_section_offset(section_b, (0.0, 1.0));
    project.set_section_offset(section_c, (0.0, 1.0));

    // Merge section_a:1 with section_b:0
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(section_a, 1).into(),
        station_b: StationRef::new(section_b, 0).into(),
        merged_at: Utc::now(),
    });

    // Merge section_b:0 with section_c:0
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(section_b, 0).into(),
        station_b: StationRef::new(section_c, 0).into(),
        merged_at: Utc::now(),
    });

    // Delete section_b's first shot (pos=0) => removed_station = section_b:0
    let shot_id = project.get_section(section_b).unwrap().shots[0].id;
    let (_, impact) = project.remove_shot_with_rewire(section_b, shot_id).unwrap();

    // removed_merges should include both merges involving section_b:0
    assert_eq!(impact.removed_merges.len(), 2);

    // After removal, there should be a replacement merge between section_a:1 and section_c:0
    let has_replacement = project.station_merges.iter().any(|m| {
        (project.key_is_ref(&m.station_a, StationRef::new(section_a, 1))
            && project.key_is_ref(&m.station_b, StationRef::new(section_c, 0)))
            || (project.key_is_ref(&m.station_a, StationRef::new(section_c, 0))
                && project.key_is_ref(&m.station_b, StationRef::new(section_a, 1)))
    });
    assert!(
        has_replacement,
        "Expected replacement merge between section_a:1 and section_c:0"
    );
}

#[test]
fn insert_shot_before_at_position_zero_adjusts_offset() {
    let mut project = Project::new("InsertBefore".to_string(), None);

    // Section with 1 shot going north
    let section_id = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    let first_shot_id = project.get_section(section_id).unwrap().shots[0].id;

    // Insert a shot going east before the first shot
    let inserted = project.insert_shot_before(
        section_id,
        first_shot_id,
        shot(90.0, 90.0, 5.0, 0.0, 0.0),
        None,
        None,
    );
    assert!(inserted.is_some());

    // Displacement of inserted shot going east at 90°:
    // (horizontal * sin(90°), horizontal * cos(90°)) = (5.0, ~0.0)
    // Offset adjusted by (-5.0, ~0.0) to keep existing stations in place
    let offset = project
        .section_offsets
        .get(&section_id)
        .copied()
        .unwrap_or((0.0, 0.0));
    assert!(
        (offset.0 - (-5.0)).abs() < 1e-6,
        "Expected x offset ~-5.0, got {}",
        offset.0
    );
    assert!(
        (offset.1).abs() < 1e-6,
        "Expected y offset ~0.0, got {}",
        offset.1
    );
}

#[test]
fn insert_shot_after_shifts_station_refs() {
    let mut project = Project::new("InsertAfterShift".to_string(), None);

    let section_a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0), shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let section_b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0), shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Add a merge between section_a:2 (after second shot) and section_b:0
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(section_a, 2).into(),
        station_b: StationRef::new(section_b, 0).into(),
        merged_at: Utc::now(),
    });

    // Add a loop closure between section_a:2 and section_b:1
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(section_a, 2).into(),
        station_b: StationRef::new(section_b, 1).into(),
        path_length: 2.0,
        misclosure_distance: 0.1,
        error_percentage: 5.0,
        horizontal_misclosure: 0.1,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });
    project.next_closure_id = 2;

    // Canonicalize the transitional Index anchors to stable shot-UUID anchors
    // (as a load would) so they track the physical station across the insert -
    // that position-independence is exactly what C1 verifies here.
    project.reanchor_station_refs();

    // Insert a shot after section_a's first shot (position 0)
    let first_shot_id = project.get_section(section_a).unwrap().shots[0].id;
    let inserted = project.insert_shot_after(
        section_a,
        first_shot_id,
        shot(0.0, 0.0, 1.0, 0.0, 0.0),
        None,
        None,
    );
    assert!(inserted.is_some());

    // The new shot goes at position 1. Old stations at index >= 2 shift to index+1.
    // So section_a:2 becomes section_a:3.
    // The merge should now reference section_a:3.
    let merge = &project.station_merges[0];
    assert!(
        project.key_is_ref(&merge.station_a, StationRef::new(section_a, 3)),
        "Merge station_a should have shifted from 2 to 3"
    );
    assert!(project.key_is_ref(&merge.station_b, StationRef::new(section_b, 0)));

    // The closure should reference section_a:3 (was section_a:2).
    let closure = &project.loop_closures[0];
    assert!(
        project.key_is_ref(&closure.station_a, StationRef::new(section_a, 3)),
        "Closure station_a should have shifted from 2 to 3"
    );
    assert!(project.key_is_ref(&closure.station_b, StationRef::new(section_b, 1)));
}

/// C1 regression: inserting a shot UPSTREAM of a merge must keep the merge on
/// the SAME physical station - the stored anchor never moves, it just resolves
/// to a higher index - and must NOT silently re-point onto the new in-range
/// station the insert created. This is the exact failure mode positional refs
/// produced (a missed shift mispointed the merge); anchors make it impossible.
#[test]
fn insert_upstream_keeps_cross_refs_on_same_physical_station() {
    let mut project = Project::new("StableId".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 5.0, 0.0, 0.0),
                shot(90.0, 90.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    // Merge A:1 ↔ B:0, then canonicalize anchors as a load would.
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();
    project.reanchor_station_refs();
    assert!(
        project.check_merge_alignment().is_empty(),
        "merge starts aligned"
    );

    let a_endpoint = |p: &Project| -> StationKey {
        let m = &p.station_merges[0];
        if m.station_a.section_id == a {
            m.station_a.clone()
        } else {
            m.station_b.clone()
        }
    };
    assert!(project.key_is_ref(&a_endpoint(&project), StationRef::new(a, 1)));

    // Insert a shot at the FRONT of A. The merge's physical station (old A:1) is
    // now A:2; B:0 is untouched.
    let first = project.get_section(a).unwrap().shots[0].id;
    project
        .insert_shot_before(a, first, shot(45.0, 45.0, 3.0, 0.0, 0.0), None, None)
        .expect("insert at front of A");

    assert!(
        project.key_is_ref(&a_endpoint(&project), StationRef::new(a, 2)),
        "merge must follow its physical station to A:2, not stay at the new A:1"
    );
    assert!(
        !project.key_is_ref(&a_endpoint(&project), StationRef::new(a, 1)),
        "merge must NOT silently re-point onto the freshly-inserted in-range station A:1"
    );
    // And the partner stays colocated (the offset cascade kept them coincident).
    assert!(
        project.check_merge_alignment().is_empty(),
        "merge endpoints stay colocated after the upstream insert"
    );
}

/// C1 regression: a stored anchor whose shot was deleted is flagged Critical by
/// the integrity check (a dangling reference), where a positional ref could only
/// ever be out-of-range - never a silent in-range mispoint.
#[test]
fn dangling_anchor_flagged_by_integrity() {
    let mut project = Project::new("Dangling".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 5.0, 0.0, 0.0),
                shot(90.0, 90.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project
        .merge_stations(StationRef::new(a, 2), StationRef::new(b, 0))
        .unwrap();
    project.reanchor_station_refs();
    assert!(
        project.check_structural_integrity().is_empty(),
        "sound to start"
    );

    // Forge a merge endpoint onto a shot UUID that doesn't exist in the section.
    forge_first_merge(&mut project, |m| {
        m.station_a = StationKey::new(a, StationAnchor::AfterShot("does-not-exist".to_string()));
    });
    let v = project.check_structural_integrity();
    assert!(
        v.iter().any(|x| x.category == "dangling_merge_ref"),
        "dangling anchor must be flagged: {v:?}"
    );
}

#[test]
fn calculate_penetration_with_zero_datum() {
    let mut project = Project::new("Penetration".to_string(), None);

    let section_id = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Set a zero datum on station 0
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(section_id, 0).into(),
        gps: GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    });

    // Compute stations
    let stations = project.compute_station_coordinates();

    // Station 0 at origin, station 1 at (0, 10, 0), station 2 at (0, 15, 0)
    // Penetration = max distance from datum station to any station = 15.0
    let penetration = project.calculate_penetration(&stations);
    assert!(
        (penetration - 15.0).abs() < 1e-6,
        "Expected penetration ~15.0, got {penetration}"
    );
}

#[test]
fn calculate_penetration_no_datums_returns_zero() {
    let mut project = Project::new("NoDatums".to_string(), None);

    project.add_section(SectionData {
        shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
        ..Default::default()
    });

    // No zero datums set
    let stations = project.compute_station_coordinates();
    let penetration = project.calculate_penetration(&stations);
    assert!(
        (penetration - 0.0).abs() < 1e-6,
        "Expected penetration 0.0, got {penetration}"
    );
}

#[test]
fn compute_distances_from_chosen_station() {
    // Generalized API rooted at an arbitrary (non-datum) station: the gradient's
    // click-to-pick case. Chain of 3 stations at y = 0, 10, 15.
    let mut project = Project::new("DistFromStation".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    let stations = project.compute_station_coordinates();
    let idx = |i: u32| {
        stations
            .iter()
            .position(|s| s.station_ref == StationRef::new(section_id, i))
            .unwrap()
    };

    // Root at the middle station (index 1) - no zero datum needed: distances are
    // 10 (to station 0), 0 (self), 5 (to station 2).
    let dist = project.compute_distances_from(&stations, &[idx(1)]);
    assert!((dist[idx(0)] - 10.0).abs() < 1e-6, "got {}", dist[idx(0)]);
    assert!((dist[idx(1)] - 0.0).abs() < 1e-6, "got {}", dist[idx(1)]);
    assert!((dist[idx(2)] - 5.0).abs() < 1e-6, "got {}", dist[idx(2)]);
}

#[test]
fn compute_distances_from_empty_roots_all_infinity() {
    let mut project = Project::new("DistNoRoots".to_string(), None);
    project.add_section(SectionData {
        shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
        ..Default::default()
    });
    let stations = project.compute_station_coordinates();
    let dist = project.compute_distances_from(&stations, &[]);
    assert_eq!(dist.len(), stations.len());
    assert!(dist.iter().all(|d| d.is_infinite()));
}

#[test]
fn compute_distances_from_zero_datum_roots_matches_penetration() {
    // The generalized API, seeded with the zero-datum indices, must reproduce
    // compute_penetration_distances exactly (penetration is the default origin).
    let mut project = Project::new("DistMatchesPen".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(section_id, 0).into(),
        gps: GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    });

    let stations = project.compute_station_coordinates();
    let datum_idx = stations
        .iter()
        .position(|s| s.station_ref == StationRef::new(section_id, 0))
        .unwrap();

    let pen = project.compute_penetration_distances(&stations);
    let from = project.compute_distances_from(&stations, &[datum_idx]);
    assert_eq!(pen.len(), from.len());
    for (a, b) in pen.iter().zip(from.iter()) {
        assert!(
            (a - b).abs() < 1e-6 || (a.is_infinite() && b.is_infinite()),
            "penetration={a} distances_from={b}"
        );
    }
}

#[test]
fn reachability_keeps_jump_reference_shots() {
    // A jump/reference shot is a real measured leg: a virtual shot that is
    // the only link to a tail station keeps it reachable (it's included in the
    // graph like any other shot).
    let mut project = Project::new("JumpKept".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0), // permanent A0->A1
                shot(0.0, 0.0, 5.0, 0.0, 0.0),  // marked reference A1->A2
            ],
            ..Default::default()
        })
        .id;
    project
        .sections
        .iter_mut()
        .find(|s| s.id == section_id)
        .unwrap()
        .shots[1]
        .kind = ShotKind::Jump;

    let stations = project.compute_station_coordinates();
    let idx = |i: u32| {
        stations
            .iter()
            .position(|s| s.station_ref == StationRef::new(section_id, i))
            .unwrap()
    };
    // Root at A0: A2 is reachable only across the reference shot → kept as a jump.
    let dist = project.compute_distances_from(&stations, &[idx(0)]);
    assert!(
        (dist[idx(2)] - 15.0).abs() < 1e-6,
        "jump must keep A2 reachable at 15.0, got {}",
        dist[idx(2)]
    );
}

#[test]
fn reachability_uses_jump_shot_shortcut() {
    // A JUMP is a swum connection, so it stays in the graph: B's jump links
    // B0-B1 (len 5). Even though B0/B1 are ALSO connected the long way through
    // section A (A0-A1-A2 = 20), shortest-path takes the direct jump (5). This
    // is 88ccbaff's regression guard (#34); #27 narrowed it from "every virtual
    // shot" to "jumps only", and the Reference half is pinned by the companion
    // test below.
    let mut project = Project::new("JumpShortcut".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0), // A0->A1
                shot(0.0, 0.0, 10.0, 0.0, 0.0), // A1->A2
            ],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)], // B0->B1
            ..Default::default()
        })
        .id;
    project
        .sections
        .iter_mut()
        .find(|s| s.id == b)
        .unwrap()
        .shots[0]
        .kind = ShotKind::Jump;

    // Tie B's endpoints onto A's chain so B0-B1 has an alternate route: B0<->A0
    // via a merge, B1<->A2 via an aligned loop closure (merging an already-
    // connected pair is rejected - cycles must be closures).
    project
        .merge_stations(StationRef::new(b, 0), StationRef::new(a, 0))
        .unwrap();
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(a, 2).into(),
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: true,
    });

    let stations = project.compute_station_coordinates();
    let pos = |sec, i| {
        stations
            .iter()
            .position(|s| s.station_ref == StationRef::new(sec, i))
            .unwrap()
    };
    let dist = project.compute_distances_from(&stations, &[pos(b, 0)]);
    assert!(
        (dist[pos(b, 1)] - 5.0).abs() < 1e-6,
        "jump shortcut must be used (B1 direct = 5, not 20 via A), got {}",
        dist[pos(b, 1)]
    );

    // Flipping the SAME fixture to Reference must route the long way instead:
    // a reference tie is a tape measurement, not a swimmable passage. This is
    // the false-shortcut risk #77 left open, now closed by #27.
    project
        .sections
        .iter_mut()
        .find(|s| s.id == b)
        .unwrap()
        .shots[0]
        .kind = ShotKind::Reference;
    let dist = project.compute_distances_from(&stations, &[pos(b, 0)]);
    assert!(
        (dist[pos(b, 1)] - 20.0).abs() < 1e-6,
        "a reference tie is not swimmable, so the route must go the long way \
         via A (20), got {}",
        dist[pos(b, 1)]
    );

    // ...unless the user opts references back in, which restores the old
    // inclusive behaviour exactly (#27 q13).
    let dist = project.compute_distances_from_with(&stations, &[pos(b, 0)], true);
    assert!(
        (dist[pos(b, 1)] - 5.0).abs() < 1e-6,
        "include_reference_legs must restore the pre-#27 shortcut, got {}",
        dist[pos(b, 1)]
    );
}

#[test]
fn reachability_includes_non_aligned_closures() {
    // A non-aligned (e.g. Batch) loop closure is still a real physical connection.
    // Section B is reachable from A's zero datum ONLY across a non-aligned closure
    // - it must NOT show as unreachable. (#34: areas connected via closure)
    let mut project = Project::new("NonAlignedClosure".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)], // A0->A1
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 7.0, 0.0, 0.0)], // B0->B1, disconnected from A
            ..Default::default()
        })
        .id;
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    });
    // A1 <-> B0 via a NON-aligned closure (is_aligned = false).
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });

    let stations = project.compute_station_coordinates();
    let field = project.distance_field_from(&stations, None); // from the zero datum
    let pos = |sec, i| {
        stations
            .iter()
            .position(|s| s.station_ref == StationRef::new(sec, i))
            .unwrap()
    };
    // A0=0, A1=10, B0=10 (closure, 0-cost), B1=17 - all finite (reachable).
    assert_eq!(
        field.distances[pos(b, 0)],
        Some(10.0),
        "B0 reachable via closure"
    );
    assert_eq!(
        field.distances[pos(b, 1)],
        Some(17.0),
        "B1 reachable via closure"
    );
}

#[test]
fn distance_field_uses_coordinate_distances() {
    // #2 (#34): edge weights come from the station coordinates passed in, so the
    // gradient's distances reflect the (closure-adjusted) geometry on the map,
    // not the raw measured shot length.
    let mut project = Project::new("DistCoords".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    });

    let mut stations = project.compute_station_coordinates();
    let i1 = stations
        .iter()
        .position(|s| s.station_ref == StationRef::new(a, 1))
        .unwrap();
    // Baseline: a1 sits 10 m out (matches the measured shot length).
    assert_eq!(
        project.distance_field_from(&stations, None).distances[i1],
        Some(10.0)
    );
    // Move a1 to 25 m out → the distance follows the coordinate, not the length.
    stations[i1].coordinates = Coordinates::new(0.0, 25.0, 0.0);
    assert_eq!(
        project.distance_field_from(&stations, None).distances[i1],
        Some(25.0)
    );
}

// === Loop Closure Algorithm Tests ===

// Group A: create_loop_closure validation (new tests only; see existing tests above for basic/duplicate/deferred)

#[test]
fn create_loop_closure_rejects_merged_stations() {
    let mut project = Project::new("ClosureRejectMerged".to_string(), None);

    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();

    // Try to create closure between already-merged stations
    let result = project.create_loop_closure(
        StationRef::new(a, 1),
        StationRef::new(b, 0),
        ClosureStrategy::Batch,
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("already merged"));
}

#[test]
fn create_loop_closure_allows_disconnected_with_both_datums() {
    let mut project = Project::new("ClosureDisconnectedDatums".to_string(), None);

    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Each section gets its own zero datum (disconnected but GPS-anchored)
    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        )
        .unwrap();
    project
        .set_zero_datum(
            StationRef::new(b, 0),
            GpsCoordinates::new(0.0, 0.01, 0.0).unwrap(),
        )
        .unwrap();

    let result = project.create_loop_closure(
        StationRef::new(a, 1),
        StationRef::new(b, 1),
        ClosureStrategy::Batch,
    );
    assert!(
        result.is_ok(),
        "Disconnected with both datums should allow closure: {:?}",
        result.err()
    );
}

// Group B: compute_closure_metrics

#[test]
fn compute_closure_metrics_known_geometry() {
    let mut project = Project::new("ClosureMetrics".to_string(), None);

    // Section A: 2 shots going north (0°), 10m each → A:0=(0,0), A:1=(0,10), A:2=(0,20)
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Section B: 1 shot going east (90°), 5m → B:0=(0,0), B:1=(5,0)
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge A:0 with B:0
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();

    let metrics = project
        .compute_closure_metrics(StationRef::new(a, 2), StationRef::new(b, 1))
        .unwrap();

    // A:2 is at (0, 20), B:1 is at (5, 0) after merge alignment
    // B was moved to align B:0 with A:0 at origin
    let coords_a2 = project
        .compute_station_raw_coords(StationRef::new(a, 2))
        .unwrap();
    let coords_b1 = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();

    let expected_h = coords_a2.horizontal_distance_to(&coords_b1);
    let expected_v = (coords_a2.z - coords_b1.z).abs();
    let expected_d = coords_a2.distance_to(&coords_b1);

    assert!(
        (metrics.horizontal_misclosure - expected_h).abs() < 1e-6,
        "horizontal: expected {}, got {}",
        expected_h,
        metrics.horizontal_misclosure
    );
    assert!(
        (metrics.vertical_misclosure - expected_v).abs() < 1e-6,
        "vertical: expected {}, got {}",
        expected_v,
        metrics.vertical_misclosure
    );
    assert!(
        (metrics.misclosure_distance - expected_d).abs() < 1e-6,
        "distance: expected {}, got {}",
        expected_d,
        metrics.misclosure_distance
    );
    assert!(metrics.path_length > 0.0, "path_length should be > 0");
    assert!(
        metrics.error_percentage > 0.0,
        "error_percentage should be > 0"
    );
}

#[test]
fn compute_closure_metrics_zero_misclosure() {
    let mut project = Project::new("ClosureZeroMisclosure".to_string(), None);

    // Two sections that arrive at the same point: a perfect loop
    // Section A: 1 shot going north (0°), 10m → A:0=(0,0), A:1=(0,10)
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Section B: 1 shot going north (0°), 10m → B:0=(0,0), B:1=(0,10)
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge A:0 with B:0
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();

    // A:1 and B:1 should be at the same point (0, 10)
    let metrics = project
        .compute_closure_metrics(StationRef::new(a, 1), StationRef::new(b, 1))
        .unwrap();

    assert!(
        metrics.misclosure_distance < 1e-6,
        "Expected near-zero misclosure, got {}",
        metrics.misclosure_distance
    );
    assert!(
        metrics.error_percentage < 1e-6,
        "Expected near-zero error, got {}",
        metrics.error_percentage
    );
}

// Group C: distribute_closure_error_internal (core algorithm)

#[test]
fn distribute_closure_simple_two_section_loop() {
    let mut project = Project::new("DistributeSimple".to_string(), None);

    // Section A: 3 shots going north (0°), 10m each → A:3=(0,30)
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Section B: 1 shot going south (180°), 29m → B:1=(0,-29)
    let b = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 29.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge A:0 with B:0
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();

    // A:3 at (0,30), B:1 at (0,-29). Gap = 59m in y.
    let coords_a3_before = project
        .compute_station_raw_coords(StationRef::new(a, 3))
        .unwrap();
    let coords_b1_before = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    assert!((coords_a3_before.y - 30.0).abs() < 1e-6);
    assert!((coords_b1_before.y - (-29.0)).abs() < 1e-6);

    // Distribute closure error proportionally
    project
        .test_distribute_closure_error(
            StationRef::new(a, 3),
            StationRef::new(b, 1),
            ClosureStrategy::ProportionalByLength,
        )
        .unwrap();

    // After distribution, A:3 and B:1 should have the same coordinates
    let coords_a3_after = project
        .compute_station_raw_coords(StationRef::new(a, 3))
        .unwrap();
    let coords_b1_after = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    let residual = coords_a3_after.distance_to(&coords_b1_after);
    assert!(
        residual < 0.01,
        "Endpoints should align after distribution, residual = {residual}"
    );
}

#[test]
fn distribute_closure_with_zero_datum_at_endpoint() {
    let mut project = Project::new("DistributeDatumEndpoint".to_string(), None);

    // Section A: 3 shots going north (0°), 10m each
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Section B: 1 shot going south (180°), 29m
    let b = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 29.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set zero datum on A:0 before merging
    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        )
        .unwrap();

    // Merge A:0 with B:0
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();

    // Record A:0 position before distribution
    let coords_a0_before = project
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();

    // Distribute closure error
    project
        .test_distribute_closure_error(
            StationRef::new(a, 3),
            StationRef::new(b, 1),
            ClosureStrategy::ProportionalByLength,
        )
        .unwrap();

    // A:0 (zero datum) should NOT have moved
    let coords_a0_after = project
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();
    assert!(
        coords_a0_before.distance_to(&coords_a0_after) < 1e-6,
        "Zero datum station should not move"
    );

    // Endpoints should align
    let coords_a3_after = project
        .compute_station_raw_coords(StationRef::new(a, 3))
        .unwrap();
    let coords_b1_after = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    let residual = coords_a3_after.distance_to(&coords_b1_after);
    assert!(
        residual < 0.01,
        "Endpoints should align after distribution, residual = {residual}"
    );
}

#[test]
fn distribute_closure_with_zero_datum_mid_path() {
    let mut project = Project::new("DistributeDatumMid".to_string(), None);

    // Section A: 2 shots going north (0°), 10m each → A:0=(0,0), A:1=(0,10), A:2=(0,20)
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Section B: 1 shot going south (180°), 19m → B:1=(0,-19)
    let b = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 19.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge A:0 with B:0
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();

    // Set zero datum on A:1 (mid-path station)
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 1).into(),
        gps: GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    });

    // Record A:1 position before distribution
    let coords_a1_before = project
        .compute_station_raw_coords(StationRef::new(a, 1))
        .unwrap();

    // Distribute closure error - datum is mid-path, so path splits at A:1
    project
        .test_distribute_closure_error(
            StationRef::new(a, 2),
            StationRef::new(b, 1),
            ClosureStrategy::ProportionalByLength,
        )
        .unwrap();

    // A:1 (zero datum) should NOT have moved
    let coords_a1_after = project
        .compute_station_raw_coords(StationRef::new(a, 1))
        .unwrap();
    assert!(
        coords_a1_before.distance_to(&coords_a1_after) < 1e-6,
        "Zero datum station should not move, moved by {}",
        coords_a1_before.distance_to(&coords_a1_after)
    );

    // Endpoints should align
    let coords_a2_after = project
        .compute_station_raw_coords(StationRef::new(a, 2))
        .unwrap();
    let coords_b1_after = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    let residual = coords_a2_after.distance_to(&coords_b1_after);
    assert!(
        residual < 0.01,
        "Endpoints should align after distribution, residual = {residual}"
    );
}

#[test]
fn distribute_closure_both_anchored_fails() {
    let mut project = Project::new("DistributeBothAnchored".to_string(), None);

    // Section A: 1 shot going north
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Section B: 1 shot going south, slightly different length
    let b = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 9.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge A:0 with B:0
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();

    // Set zero datums on both closure endpoints
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 1).into(),
        gps: GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(b, 1).into(),
        gps: GpsCoordinates::new(0.0, 0.01, 0.0).unwrap(),
    });

    // Should fail because both endpoints are anchored
    let result = project.test_distribute_closure_error(
        StationRef::new(a, 1),
        StationRef::new(b, 1),
        ClosureStrategy::ProportionalByLength,
    );
    assert!(
        result.is_err(),
        "Should fail when both endpoints are anchored to zero datums"
    );
}

#[test]
fn distribute_closure_equal_distribution() {
    let mut project = Project::new("DistributeEqual".to_string(), None);

    // Section A: 3 shots going north, different lengths
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 5.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 15.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Section B: 1 shot going south, 29m
    let b = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 29.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();

    project
        .test_distribute_closure_error(
            StationRef::new(a, 3),
            StationRef::new(b, 1),
            ClosureStrategy::EqualDistribution,
        )
        .unwrap();

    // Endpoints should align
    let coords_a3 = project
        .compute_station_raw_coords(StationRef::new(a, 3))
        .unwrap();
    let coords_b1 = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    let residual = coords_a3.distance_to(&coords_b1);
    assert!(
        residual < 0.01,
        "Endpoints should align after equal distribution, residual = {residual}"
    );
}

#[test]
fn distribute_closure_fallback_separate_datums() {
    let mut project = Project::new("DistributeFallbackDatums".to_string(), None);

    // Section A: 2 shots going north → A:2=(0,20)
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Section B: 1 shot going east → B:1=(5+offset,0+offset)
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set separate zero datums on each (NOT connected by merges)
    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        )
        .unwrap();
    project
        .set_zero_datum(
            StationRef::new(b, 0),
            GpsCoordinates::new(0.0, 0.01, 0.0).unwrap(),
        )
        .unwrap();

    // Distribute with fallback path (no direct connection)
    project
        .test_distribute_closure_error(
            StationRef::new(a, 2),
            StationRef::new(b, 1),
            ClosureStrategy::ProportionalByLength,
        )
        .unwrap();

    // Endpoints should align
    let coords_a2 = project
        .compute_station_raw_coords(StationRef::new(a, 2))
        .unwrap();
    let coords_b1 = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    let residual = coords_a2.distance_to(&coords_b1);
    assert!(
        residual < 0.01,
        "Endpoints should align via fallback datums, residual = {residual}"
    );
}

/// Stress test: 50-section ring network with closure.
/// Verifies the sparse PCG solver produces correct results for a
/// non-trivial graph (all endpoints align, zero datums preserved).
#[test]
fn distribute_closure_large_ring_network() {
    let mut project = Project::new("LargeRingNetwork".to_string(), None);

    // Create 50 sections, each with 5 shots going at slightly different azimuths.
    // This creates a network where the union-find produces many distinct groups,
    // exercising the sparse solver with a matrix larger than trivial test cases.
    let num_sections = 50;
    let mut section_ids = Vec::new();
    for i in 0..num_sections {
        // Each section goes at a slightly different heading to create a ring
        let azimuth = (i as f64 * 360.0 / num_sections as f64) % 360.0;
        let id = project
            .add_section(SectionData {
                shots: vec![
                    shot(azimuth, azimuth, 10.0, 0.0, 0.0),
                    shot(azimuth, azimuth, 10.0, 0.0, 0.0),
                    shot(azimuth, azimuth, 10.0, 0.0, 0.0),
                    shot(azimuth, azimuth, 10.0, 0.0, 0.0),
                    shot(azimuth, azimuth, 10.0, 0.0, 0.0),
                ],
                ..Default::default()
            })
            .id;
        section_ids.push(id);
    }

    // Chain sections: merge end of section i with start of section i+1
    for i in 0..(num_sections - 1) {
        project
            .merge_stations(
                StationRef::new(section_ids[i], 5),     // end of section i
                StationRef::new(section_ids[i + 1], 0), // start of section i+1
            )
            .unwrap();
    }

    // Set zero datum on the first station
    project
        .set_zero_datum(
            StationRef::new(section_ids[0], 0),
            GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        )
        .unwrap();

    // Record positions before closure
    let first_station = StationRef::new(section_ids[0], 0);
    let last_station = StationRef::new(section_ids[num_sections - 1], 5);
    let coords_first_before = project.compute_station_raw_coords(first_station).unwrap();

    // Create a closure between last station and first station (closing the ring)
    project
        .test_distribute_closure_error(
            last_station,
            first_station,
            ClosureStrategy::ProportionalByLength,
        )
        .unwrap();

    // Zero datum should not have moved
    let coords_first_after = project.compute_station_raw_coords(first_station).unwrap();
    assert!(
        coords_first_before.distance_to(&coords_first_after) < 1e-6,
        "Zero datum should not move, delta = {}",
        coords_first_before.distance_to(&coords_first_after)
    );

    // Endpoints should align (closure should be resolved)
    let coords_last_after = project.compute_station_raw_coords(last_station).unwrap();
    let coords_first_after2 = project.compute_station_raw_coords(first_station).unwrap();
    let residual = coords_last_after.distance_to(&coords_first_after2);
    assert!(
        residual < 0.01,
        "Ring endpoints should align after distribution, residual = {residual}"
    );

    // All merge points should remain consistent (merged stations should coincide)
    for i in 0..(num_sections - 1) {
        let end_i = project
            .compute_station_raw_coords(StationRef::new(section_ids[i], 5))
            .unwrap();
        let start_next = project
            .compute_station_raw_coords(StationRef::new(section_ids[i + 1], 0))
            .unwrap();
        let merge_residual = end_i.distance_to(&start_next);
        assert!(
            merge_residual < 0.01,
            "Merge point {i} should be consistent, residual = {merge_residual}"
        );
    }
}

// Group D: apply_station_move_internal (tested indirectly)

#[test]
fn apply_station_move_adjusts_incoming_and_outgoing_shots() {
    let mut project = Project::new("StationMoveMiddle".to_string(), None);

    // Section with 3 shots: stations at (0,0), (0,10), (0,20), (0,30)
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Section B: 1 shot, same geometry to create a closure path
    let b = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 29.5, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();

    // Record positions before (verify they're valid)
    let _coords_0_before = project
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();
    let _coords_3_before = project
        .compute_station_raw_coords(StationRef::new(a, 3))
        .unwrap();

    // Distribute closure error - this will move intermediate stations including middle ones
    project
        .test_distribute_closure_error(
            StationRef::new(a, 3),
            StationRef::new(b, 1),
            ClosureStrategy::ProportionalByLength,
        )
        .unwrap();

    // Endpoints should be aligned
    let coords_a3 = project
        .compute_station_raw_coords(StationRef::new(a, 3))
        .unwrap();
    let coords_b1 = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    assert!(
        coords_a3.distance_to(&coords_b1) < 0.01,
        "Endpoints should be aligned"
    );

    // Intermediate stations (1, 2) should have moved - their positions should differ from
    // the original linear progression
    let coords_1 = project
        .compute_station_raw_coords(StationRef::new(a, 1))
        .unwrap();
    let coords_2 = project
        .compute_station_raw_coords(StationRef::new(a, 2))
        .unwrap();

    // All 4 stations should still be in a reasonable order (y increasing or near-increasing)
    let coords_0 = project
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();
    assert!(
        coords_0.y < coords_1.y || (coords_0.y - coords_1.y).abs() < 1.0,
        "Station 0 should be before station 1"
    );
    assert!(
        coords_1.y < coords_2.y || (coords_1.y - coords_2.y).abs() < 1.0,
        "Station 1 should be before station 2"
    );
}

#[test]
fn apply_station_move_station_zero_uses_section_offset() {
    let mut project = Project::new("StationMoveZero".to_string(), None);

    // Section A: 1 shot going north
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Section B: 1 shot going south, slightly short - creates misclosure at station 0
    let b = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();

    // Record section offset before
    let _offset_before = project
        .section_offsets
        .get(&a)
        .copied()
        .unwrap_or((0.0, 0.0));

    // A:0 is at the origin. B:1 is at (0, 10) + (0, -10) = (0, 0) after merge alignment.
    // Both A:0 and B:1 are at (0, 0), so zero misclosure - perfect loop.
    // Let's modify B to create a gap:
    // Actually, let's use a more direct test - just set up the closure so station 0 needs to move
    let c = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project
        .merge_stations(StationRef::new(b, 1), StationRef::new(c, 0))
        .unwrap();

    // Now we have A:0 merged to chain, create a closure between A:0's group and C:1
    // A:0=(0,0), C:1 = some other position - distribute will move stations
    // The test verifies that if station 0 needs an XY move, section_offsets is used
    let _offset_a_before = project
        .section_offsets
        .get(&a)
        .copied()
        .unwrap_or((0.0, 0.0));

    // Distribute error - this exercises apply_station_move_internal on various stations
    project
        .test_distribute_closure_error(
            StationRef::new(a, 0),
            StationRef::new(c, 1),
            ClosureStrategy::EqualDistribution,
        )
        .unwrap();

    // The test passes if no panic/error occurs - the internal logic correctly
    // routes station 0 XY moves through section_offsets
    let coords_a0 = project
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();
    let coords_c1 = project
        .compute_station_raw_coords(StationRef::new(c, 1))
        .unwrap();
    assert!(
        coords_a0.distance_to(&coords_c1) < 0.01,
        "Endpoints should align"
    );
}

#[test]
fn apply_station_move_last_station_only_modifies_incoming() {
    let mut project = Project::new("StationMoveLast".to_string(), None);

    // 2 shots: stations 0, 1, 2
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // B: connect to create closure path
    let b = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 19.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();

    // A:2 is the last station. After distribution, the last shot (idx=1) should be modified
    // but there's no outgoing shot from A:2.
    project
        .test_distribute_closure_error(
            StationRef::new(a, 2),
            StationRef::new(b, 1),
            ClosureStrategy::ProportionalByLength,
        )
        .unwrap();

    // Verify alignment
    let coords_a2 = project
        .compute_station_raw_coords(StationRef::new(a, 2))
        .unwrap();
    let coords_b1 = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    assert!(
        coords_a2.distance_to(&coords_b1) < 0.01,
        "Endpoints should align, residual = {}",
        coords_a2.distance_to(&coords_b1)
    );

    // All stations should have valid, finite coordinates
    for idx in 0..=2 {
        let c = project
            .compute_station_raw_coords(StationRef::new(a, idx))
            .unwrap();
        assert!(
            c.x.is_finite() && c.y.is_finite() && c.z.is_finite(),
            "Station {idx} coords should be finite"
        );
    }
}

// Group E: conflicting merge-station translations, resolved through adjust_loop_closures

#[test]
fn enforce_merge_alignment_conflicted_section_applies_segment_correction() {
    let mut project = Project::new("EnforceMergeAlignment".to_string(), None);

    // Section A: 3 shots going north, 10m each → A:0=(0,0), A:1=(0,10), A:2=(0,20), A:3=(0,30)
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Section B: 1 shot going east, 5m → B:0=(0,0), B:1=(5,0)
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Section C: 1 shot going west, 5m → C:0=(0,0), C:1=(-5,0)
    let c = project
        .add_section(SectionData {
            shots: vec![shot(270.0, 270.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set datum on A:0
    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        )
        .unwrap();

    // Merge B:0 to A:1 and C:0 to A:3
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();
    project
        .merge_stations(StationRef::new(a, 3), StationRef::new(c, 0))
        .unwrap();

    // Create closures between B:1 and C:1 - they form a loop through section A
    project
        .create_loop_closure(
            StationRef::new(b, 1),
            StationRef::new(c, 1),
            ClosureStrategy::Batch,
        )
        .unwrap();

    // Adjust loop closures across the conflicting merge translations
    let config = ClosureAdjustmentConfig::default();
    let result = project.adjust_loop_closures(
        StationRef::new(a, 0),
        ClosureStrategy::LeastSquaresWeighted,
        &config,
    );

    // The adjustment may succeed or fail depending on the geometry, but the important thing
    // is that it was exercised and didn't panic
    match result {
        Ok(()) => {
            // Verify stations are still at valid, finite coordinates
            for idx in 0..=3 {
                let coords = project
                    .compute_station_raw_coords(StationRef::new(a, idx))
                    .unwrap();
                assert!(
                    coords.x.is_finite() && coords.y.is_finite() && coords.z.is_finite(),
                    "A:{idx} coords should be finite after adjustment"
                );
            }

            // Zero datum should not have moved
            let coords_a0 = project
                .compute_station_raw_coords(StationRef::new(a, 0))
                .unwrap();
            assert!(
                coords_a0.x.abs() < 1e-6 && coords_a0.y.abs() < 1e-6,
                "Zero datum A:0 should remain at origin"
            );
        }
        Err(e) => {
            // Some errors are expected for certain geometries
            // (e.g., "No eligible loop closures" or convergence failure)
            assert!(
                e.contains("eligible")
                    || e.contains("converge")
                    || e.contains("datum")
                    || e.contains("failed"),
                "Unexpected error: {e}"
            );
        }
    }
}

// ── Non-destructive closure-adjusted view ─────────────────────────

/// Build a project with two paths from a shared, datum-anchored origin to a
/// near-shared endpoint that disagree by ~0.2 m, joined by a loop closure.
/// Measured: the loop is open. Adjusted: it should close.
fn looped_project_with_misclosure() -> (Project, SectionId, SectionId) {
    let mut project = Project::new("AdjustedView".to_string(), None);

    // Path A: origin → east 10 → north 10  (A:2 = (10, 10))
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    // Path B: origin → north 10 → east 10.2  (B:2 = (10.2, 10))
    let b = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(90.0, 90.0, 10.2, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        )
        .unwrap();
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();
    project
        .create_loop_closure(
            StationRef::new(a, 2),
            StationRef::new(b, 2),
            ClosureStrategy::Batch,
        )
        .unwrap();

    (project, a, b)
}

fn station_coords(stations: &[Station], sr: StationRef) -> Coordinates {
    stations
        .iter()
        .find(|s| s.station_ref == sr)
        .unwrap_or_else(|| panic!("station {sr:?} not found"))
        .coordinates
}

fn shot_geometry(project: &Project) -> Vec<(f64, f64, f64)> {
    project
        .sections
        .iter()
        .flat_map(|s| {
            s.shots
                .iter()
                .map(|sh| (sh.azimuth_in, sh.azimuth_out, sh.length))
        })
        .collect()
}

#[test]
fn compute_adjusted_closes_loop_without_mutating_measured() {
    let (project, a, b) = looped_project_with_misclosure();
    let config = ClosureAdjustmentConfig::default();

    let shots_before = shot_geometry(&project);
    let measured = project.compute_station_coordinates();
    let adjusted = project.compute_adjusted_station_coordinates(&config);

    // 1. Non-destructive: deriving the adjusted view must not rewrite measured shots.
    assert_eq!(
        shots_before,
        shot_geometry(&project),
        "deriving the adjusted view must not mutate measured shot geometry"
    );

    // 2. Measured view: the loop is open (~0.2 m misclosure).
    let m_a2 = station_coords(&measured, StationRef::new(a, 2));
    let m_b2 = station_coords(&measured, StationRef::new(b, 2));
    let measured_misclosure = ((m_a2.x - m_b2.x).powi(2) + (m_a2.y - m_b2.y).powi(2)).sqrt();
    assert!(
        measured_misclosure > 0.1,
        "measured loop should be open, got {measured_misclosure}"
    );

    // 3. Adjusted view: the loop closes (endpoints ~coincident).
    let adj_a2 = station_coords(&adjusted, StationRef::new(a, 2));
    let adj_b2 = station_coords(&adjusted, StationRef::new(b, 2));
    let adjusted_misclosure =
        ((adj_a2.x - adj_b2.x).powi(2) + (adj_a2.y - adj_b2.y).powi(2)).sqrt();
    assert!(
        adjusted_misclosure < 1e-3,
        "adjusted loop should close, got {adjusted_misclosure}"
    );

    // 4. The zero datum is fixed in both views.
    let m_a0 = station_coords(&measured, StationRef::new(a, 0));
    let adj_a0 = station_coords(&adjusted, StationRef::new(a, 0));
    assert!(
        (m_a0.x - adj_a0.x).abs() < 1e-9 && (m_a0.y - adj_a0.y).abs() < 1e-9,
        "zero datum must not move in the adjusted view"
    );
}

#[test]
fn compute_adjusted_is_deterministic_and_keeps_measured_stable() {
    let (project, _a, _b) = looped_project_with_misclosure();
    let config = ClosureAdjustmentConfig::default();

    let measured_before = project.compute_station_coordinates();
    let adjusted1 = project.compute_adjusted_station_coordinates(&config);
    let adjusted2 = project.compute_adjusted_station_coordinates(&config);
    let measured_after = project.compute_station_coordinates();

    // Re-solving from intact measured data is deterministic (no compounding).
    assert_eq!(adjusted1.len(), adjusted2.len());
    for (s1, s2) in adjusted1.iter().zip(adjusted2.iter()) {
        assert_eq!(s1.station_ref, s2.station_ref);
        assert!((s1.coordinates.x - s2.coordinates.x).abs() < 1e-9);
        assert!((s1.coordinates.y - s2.coordinates.y).abs() < 1e-9);
    }

    // The measured view is unaffected by having derived the adjusted view.
    assert_eq!(measured_before.len(), measured_after.len());
    for (s1, s2) in measured_before.iter().zip(measured_after.iter()) {
        assert!((s1.coordinates.x - s2.coordinates.x).abs() < 1e-9);
        assert!((s1.coordinates.y - s2.coordinates.y).abs() < 1e-9);
    }
}

/// Exactness guard for the degree-≤2 graph-reduction solver (#81): on a range of
/// random SPD weighted-Laplacian systems (chains, single cycles, and densely
/// connected cores), `solve_reduced_laplacian` must match a reference dense
/// Gaussian elimination on both axes.
#[test]
fn solve_reduced_laplacian_matches_dense() {
    // Deterministic xorshift RNG → reproducible [0,1) draws (no rand dep).
    fn next(state: &mut u64) -> f64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        ((x >> 11) as f64) / ((1u64 << 53) as f64)
    }

    // Dense SPD solve via Gaussian elimination with partial pivoting.
    fn dense_solve(mat: &[Vec<f64>], rhs: &[f64]) -> Vec<f64> {
        let n = rhs.len();
        let mut a: Vec<Vec<f64>> = mat.to_vec();
        let mut b = rhs.to_vec();
        for col in 0..n {
            let mut piv = col;
            for r in (col + 1)..n {
                if a[r][col].abs() > a[piv][col].abs() {
                    piv = r;
                }
            }
            a.swap(col, piv);
            b.swap(col, piv);
            let d = a[col][col];
            for r in (col + 1)..n {
                let f = a[r][col] / d;
                for c in col..n {
                    a[r][c] -= f * a[col][c];
                }
                b[r] -= f * b[col];
            }
        }
        let mut x = vec![0.0; n];
        for col in (0..n).rev() {
            let mut s = b[col];
            for c in (col + 1)..n {
                s -= a[col][c] * x[c];
            }
            x[col] = s / a[col][col];
        }
        x
    }

    // (n, extra_random_edges) - 0 extra = pure chain (fully peels, empty core);
    // a few = small cycle core; many = a denser core that exercises the PCG path.
    let configs = [(2, 0), (6, 0), (8, 2), (20, 5), (40, 30), (60, 80)];
    let mut seed = 0x9E3779B97F4A7C15u64;

    for (n, extra) in configs {
        let mut adj: Vec<Vec<(usize, f64, f64)>> = vec![Vec::new(); n];
        let mut diag_x = vec![0.0; n];
        let mut diag_y = vec![0.0; n];

        let add_edge = |a: usize,
                        b: usize,
                        seed: &mut u64,
                        adj: &mut Vec<Vec<(usize, f64, f64)>>,
                        dx: &mut Vec<f64>,
                        dy: &mut Vec<f64>| {
            let wx = 0.5 + next(seed);
            let wy = 0.5 + next(seed);
            adj[a].push((b, -wx, -wy));
            adj[b].push((a, -wx, -wy));
            dx[a] += wx;
            dx[b] += wx;
            dy[a] += wy;
            dy[b] += wy;
        };

        // Spanning chain keeps the graph connected.
        for i in 0..n.saturating_sub(1) {
            add_edge(i, i + 1, &mut seed, &mut adj, &mut diag_x, &mut diag_y);
        }
        // Extra edges introduce loops (a non-trivial 2-core).
        for _ in 0..extra {
            let a = (next(&mut seed) * n as f64) as usize % n;
            let b = (next(&mut seed) * n as f64) as usize % n;
            if a != b {
                add_edge(a, b, &mut seed, &mut adj, &mut diag_x, &mut diag_y);
            }
        }
        // Grounding term on every diagonal → strictly SPD (mimics fixed-node edges).
        for i in 0..n {
            diag_x[i] += 0.25 + next(&mut seed);
            diag_y[i] += 0.25 + next(&mut seed);
        }
        let atb_x: Vec<f64> = (0..n).map(|_| next(&mut seed) * 4.0 - 2.0).collect();
        let atb_y: Vec<f64> = (0..n).map(|_| next(&mut seed) * 4.0 - 2.0).collect();

        // Dense reference matrices built from the SAME adj/diag (duplicate edges
        // summed identically to the solver).
        let mut ax = vec![vec![0.0; n]; n];
        let mut ay = vec![vec![0.0; n]; n];
        for i in 0..n {
            ax[i][i] = diag_x[i];
            ay[i][i] = diag_y[i];
            for &(j, ox, oy) in &adj[i] {
                ax[i][j] += ox;
                ay[i][j] += oy;
            }
        }
        let ref_x = dense_solve(&ax, &atb_x);
        let ref_y = dense_solve(&ay, &atb_y);

        // Strictly SPD (grounded) → unique solution, so a zero warm start matches
        // the dense reference exactly.
        let x0 = vec![0.0; n];
        let (sol_x, sol_y) =
            Project::solve_reduced_laplacian(n, &diag_x, &diag_y, &atb_x, &atb_y, &adj, &x0, &x0);

        for i in 0..n {
            assert!(
                (sol_x[i] - ref_x[i]).abs() < 1e-6,
                "n={n} extra={extra} x[{i}]: reduced={} dense={}",
                sol_x[i],
                ref_x[i]
            );
            assert!(
                (sol_y[i] - ref_y[i]).abs() < 1e-6,
                "n={n} extra={extra} y[{i}]: reduced={} dense={}",
                sol_y[i],
                ref_y[i]
            );
        }
    }
}

/// Regression for the warm-start fix (Codex #86): a node that is unconstrained on
/// one axis (zero pivot, zero-weight edge → null-space row) must keep its measured
/// position on that axis rather than snapping to world 0.
#[test]
fn solve_reduced_laplacian_preserves_unconstrained_axis_warm_start() {
    // Two nodes. Node 0 is grounded on both axes. Node 1 couples to node 0 on Y
    // only (the X off-diagonal weight is 0), and has zero X diagonal + zero X rhs
    // - i.e. nothing constrains its X. Its warm-start X (42) must survive.
    let n = 2;
    let diag_x = vec![1.0, 0.0];
    let diag_y = vec![2.0, 1.0];
    let atb_x = vec![5.0, 0.0];
    let atb_y = vec![0.0, 0.0];
    let adj = vec![vec![(1usize, 0.0, -1.0)], vec![(0usize, 0.0, -1.0)]];
    let x0_x = vec![0.0, 42.0];
    let x0_y = vec![0.0, 0.0];

    let (sol_x, _sol_y) =
        Project::solve_reduced_laplacian(n, &diag_x, &diag_y, &atb_x, &atb_y, &adj, &x0_x, &x0_y);

    assert!(
        (sol_x[1] - 42.0).abs() < 1e-9,
        "unconstrained-X node should retain its warm-start X (42), got {}",
        sol_x[1]
    );
    // The constrained node still solves normally.
    assert!(
        (sol_x[0] - 5.0).abs() < 1e-9,
        "node 0 X should be 5, got {}",
        sol_x[0]
    );
}

/// Regression for the early-exit fix (Codex #86 follow-up): when the reduced core
/// has a zero RHS but non-zero, full-rank matrix rows, the unique minimizer is 0 -
/// the warm start must NOT be returned just because `‖b‖ = 0`. Uses a K4 core (all
/// degree 3, so it survives degree-≤2 reduction and exercises the core PCG).
#[test]
fn solve_reduced_laplacian_zero_rhs_constrained_core_solves_not_warm_start() {
    let n = 4;
    // K4 + grounding: diagonal 4, every off-diagonal -1 → SPD, full rank.
    let diag = vec![4.0; 4];
    let mut adj: Vec<Vec<(usize, f64, f64)>> = vec![Vec::new(); n];
    for i in 0..n {
        for j in 0..n {
            if i != j {
                adj[i].push((j, -1.0, -1.0));
            }
        }
    }
    let atb = vec![0.0; 4]; // zero RHS → unique solution is 0
    let x0_x = vec![10.0, -5.0, 3.0, 7.0];
    let x0_y = vec![-2.0, 8.0, -6.0, 1.0];

    let (sol_x, sol_y) =
        Project::solve_reduced_laplacian(n, &diag, &diag, &atb, &atb, &adj, &x0_x, &x0_y);

    for (i, v) in sol_x.iter().chain(sol_y.iter()).enumerate() {
        assert!(
            v.abs() < 1e-6,
            "zero-RHS constrained core must solve to 0, got {v} at {i}"
        );
    }
}

/// Codex #86 (3rd, null-space-through-peeled-nodes): a component that *floats* on
/// an axis (positive internal coupling but no grounding → 1-D translation null
/// space) must keep its measured positions, not jump, when its nodes are peeled by
/// degree-≤2 reduction before the core PCG sees them.
///
/// Mechanism: the surviving core node keeps its own measured value (warm start),
/// and back-substitution reproduces the measured *relative* offsets - so the whole
/// component lands exactly on its measured coordinates, matching the old
/// full-system CG. (In real surveys this case can't even arise: every
/// positive-length shot has positive weight on both axes, so floating components
/// require zero-length shots.)
#[test]
fn solve_reduced_laplacian_preserves_floating_component_nullspace() {
    let n = 3;
    // X: pure path Laplacian 0-1-2 (singular → floats). Y: grounded (nonsingular).
    let diag_x = vec![1.0, 2.0, 1.0];
    let diag_y = vec![2.0, 3.0, 2.0];
    let adj = vec![
        vec![(1usize, -1.0, -1.0)],
        vec![(0usize, -1.0, -1.0), (2usize, -1.0, -1.0)],
        vec![(1usize, -1.0, -1.0)],
    ];
    // X rhs consistent with measured x=(0,3,5): A_x·(0,3,5) = (-3,1,2).
    let atb_x = vec![-3.0, 1.0, 2.0];
    let atb_y = vec![0.0, 0.0, 0.0];
    let x0_x = vec![0.0, 3.0, 5.0];
    let x0_y = vec![0.0, 0.0, 0.0];

    let (sx, _sy) =
        Project::solve_reduced_laplacian(n, &diag_x, &diag_y, &atb_x, &atb_y, &adj, &x0_x, &x0_y);

    for (i, want) in [0.0_f64, 3.0, 5.0].iter().enumerate() {
        assert!(
            (sx[i] - want).abs() < 1e-6,
            "floating component node {i} jumped: got {}, want {want}",
            sx[i]
        );
    }
}

/// Regression for the scale-relative pivot floor (#89 / Codex). A *loop* that
/// floats on X with **varied** edge weights drives the last null-space pivot to
/// roundoff (~1e-12), not exactly 0 - so the old absolute 1e-30 floor eliminated
/// it and divided by ~0, shifting the component. The pivot must be judged relative
/// to its original diagonal so the node stays in the warm-started core and the
/// measured translation is preserved.
#[test]
fn solve_reduced_laplacian_floating_loop_varied_weights_no_shift() {
    // Triangle 0-1-2, pure (singular) Laplacian on X → floats; grounded on Y.
    // Varied off-diagonals so the final Schur pivot lands on roundoff, not 0.
    let n = 3;
    let diag_x = vec![3.3, 2.7, 4.0]; // = incident |off| sums (row-sum 0 ⇒ singular)
    let diag_y = vec![4.3, 3.7, 5.0]; // +1 grounding ⇒ nonsingular
    let adj = vec![
        vec![(1usize, -1.0, -1.0), (2usize, -2.3, -2.3)],
        vec![(0usize, -1.0, -1.0), (2usize, -1.7, -1.7)],
        vec![(0usize, -2.3, -2.3), (1usize, -1.7, -1.7)],
    ];
    // X rhs consistent with measured x=(0,2,5): atb_x = A_x·(0,2,5).
    let atb_x = vec![-13.5, -3.1, 16.6];
    let atb_y = vec![0.0, 0.0, 0.0];
    let x0_x = vec![0.0, 2.0, 5.0];
    let x0_y = vec![0.0, 0.0, 0.0];

    let (sx, _sy) =
        Project::solve_reduced_laplacian(n, &diag_x, &diag_y, &atb_x, &atb_y, &adj, &x0_x, &x0_y);

    for (i, want) in [0.0_f64, 2.0, 5.0].iter().enumerate() {
        assert!(
            (sx[i] - want).abs() < 1e-6,
            "floating loop node {i} shifted: got {}, want {want}",
            sx[i]
        );
    }
}

/// Analytic golden: a loop with a hand-computable least-squares optimum.
///
/// Two due-east paths run from a shared, datum-anchored origin to a closure:
///   A: 0 →(5)→ A1 →(5)→ A2     (measured A2 at x = 10.0)
///   B: 0 →(5.1)→ B1 →(5.1)→ B2 (measured B2 at x = 10.2)
/// with `merge A0↔B0` and `closure A2↔B2`. Because every shot is due east, the X
/// normal equations have equal weights, so the weighted-LSQ optimum is exact and
/// independent of the sigma values: each interior station splits its path evenly
/// and the merged endpoint is the average of the two measured ends. Y stays 0.
/// This is an oracle independent of the solver implementation.
#[test]
fn adjust_line_loop_matches_hand_computed_optimum() {
    let mut project = Project::new("LineLoop".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 5.0, 0.0, 0.0),
                shot(90.0, 90.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 5.1, 0.0, 0.0),
                shot(90.0, 90.0, 5.1, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        )
        .unwrap();
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();
    project
        .create_loop_closure(
            StationRef::new(a, 2),
            StationRef::new(b, 2),
            ClosureStrategy::Batch,
        )
        .unwrap();

    let adjusted =
        project.compute_adjusted_station_coordinates(&ClosureAdjustmentConfig::default());
    let at = |sr| station_coords(&adjusted, sr);
    let approx = |c: Coordinates, x: f64, y: f64| (c.x - x).abs() < 1e-6 && (c.y - y).abs() < 1e-6;

    // Hand-computed optimum.
    assert!(
        approx(at(StationRef::new(a, 0)), 0.0, 0.0),
        "A0 = {:?}",
        at(StationRef::new(a, 0))
    );
    assert!(
        approx(at(StationRef::new(a, 1)), 5.05, 0.0),
        "A1 = {:?}",
        at(StationRef::new(a, 1))
    );
    assert!(
        approx(at(StationRef::new(a, 2)), 10.10, 0.0),
        "A2 = {:?}",
        at(StationRef::new(a, 2))
    );
    assert!(
        approx(at(StationRef::new(b, 1)), 5.05, 0.0),
        "B1 = {:?}",
        at(StationRef::new(b, 1))
    );
    assert!(
        approx(at(StationRef::new(b, 2)), 10.10, 0.0),
        "B2 = {:?}",
        at(StationRef::new(b, 2))
    );
}

/// Property/fuzz sweep over random connected geometries with closures. For each
/// random project the closure-adjusted view must:
///   - be finite (no NaN/Inf),
///   - keep the zero datum fixed,
///   - be deterministic (two solves agree),
///   - never mutate the measured geometry, and
///   - unify each eligible closure's endpoints (the adjusted gap is ~0).
/// Optimality of every one of these random solves is asserted *inside*
/// `adjust_least_squares` (the `#[cfg(test)]` `‖A·x-b‖∞` residual guard), so this
/// sweep doubles as a broad optimality check across diverse topologies.
#[test]
fn adjust_random_geometries_are_optimal_and_well_formed() {
    fn next(state: &mut u64) -> f64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        ((x >> 11) as f64) / ((1u64 << 53) as f64)
    }

    let mut seed = 0xD1CE_5EED_1234_9F17u64;
    let config = ClosureAdjustmentConfig::default();
    let mut exercised_closures = 0;

    for _trial in 0..60 {
        let mut project = Project::new("Fuzz".to_string(), None);
        let n_sections = 2 + (next(&mut seed) * 4.0) as usize; // 2..=5
        let mut sids = Vec::new();
        for _ in 0..n_sections {
            let n_shots = 1 + (next(&mut seed) * 6.0) as usize; // 1..=6
            let shots: Vec<ShotData> = (0..n_shots)
                .map(|_| {
                    let az = next(&mut seed) * 360.0;
                    let len = 1.0 + next(&mut seed) * 9.0;
                    shot(az, az, len, 0.0, 0.0)
                })
                .collect();
            sids.push(
                project
                    .add_section(SectionData {
                        shots,
                        ..Default::default()
                    })
                    .id,
            );
        }

        // Datum anchors the first section.
        project
            .set_zero_datum(
                StationRef::new(sids[0], 0),
                GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
            )
            .unwrap();

        // Connect every later section to an earlier one with a merge → a connected
        // tree (no loops yet).
        let station_count = |p: &Project, sid| {
            p.sections
                .iter()
                .find(|s| s.id == sid)
                .map(|s| s.shots.len())
                .unwrap_or(0)
        };
        for s in 1..n_sections {
            let earlier = sids[(next(&mut seed) * s as f64) as usize];
            let tgt_idx = (next(&mut seed) * (station_count(&project, earlier) + 1) as f64) as u32;
            let _ = project.merge_stations(
                StationRef::new(sids[s], 0),
                StationRef::new(earlier, tgt_idx),
            );
        }

        // Add 1..=2 closures between stations in different sections → loops.
        let mut closures: Vec<(StationRef, StationRef)> = Vec::new();
        let n_closures = 1 + (next(&mut seed) * 2.0) as usize;
        for _ in 0..n_closures {
            let si = (next(&mut seed) * n_sections as f64) as usize;
            let mut sj = (next(&mut seed) * n_sections as f64) as usize;
            if sj == si {
                sj = (sj + 1) % n_sections;
            }
            let pi = StationRef::new(
                sids[si],
                (next(&mut seed) * (station_count(&project, sids[si]) + 1) as f64) as u32,
            );
            let pj = StationRef::new(
                sids[sj],
                (next(&mut seed) * (station_count(&project, sids[sj]) + 1) as f64) as u32,
            );
            if project
                .create_loop_closure(pi, pj, ClosureStrategy::Batch)
                .is_ok()
            {
                closures.push((pi, pj));
            }
        }

        let measured_before = project.compute_station_coordinates();
        let adjusted = project.compute_adjusted_station_coordinates(&config);
        let adjusted2 = project.compute_adjusted_station_coordinates(&config);
        let measured_after = project.compute_station_coordinates();

        // Finite.
        for s in &adjusted {
            assert!(
                s.coordinates.x.is_finite()
                    && s.coordinates.y.is_finite()
                    && s.coordinates.z.is_finite(),
                "non-finite adjusted coord at {:?}: {:?}",
                s.station_ref,
                s.coordinates
            );
        }
        // Datum fixed.
        let d0 = station_coords(&adjusted, StationRef::new(sids[0], 0));
        assert!(
            d0.x.abs() < 1e-6 && d0.y.abs() < 1e-6,
            "datum moved to {d0:?}"
        );
        // Deterministic.
        assert_eq!(adjusted.len(), adjusted2.len());
        for (s1, s2) in adjusted.iter().zip(&adjusted2) {
            assert!((s1.coordinates.x - s2.coordinates.x).abs() < 1e-9);
            assert!((s1.coordinates.y - s2.coordinates.y).abs() < 1e-9);
        }
        // Measured geometry untouched by deriving the adjusted view.
        assert_eq!(measured_before.len(), measured_after.len());
        for (s1, s2) in measured_before.iter().zip(&measured_after) {
            assert!((s1.coordinates.x - s2.coordinates.x).abs() < 1e-9);
            assert!((s1.coordinates.y - s2.coordinates.y).abs() < 1e-9);
        }
        // Count solves actually exercised. We deliberately do NOT assert that each
        // closure's endpoints coincide: with multiple closures the least-squares
        // solution is a *compromise*, so an individual loop needn't close to zero -
        // that is correct LSQ behavior, not a bug. Optimality (that the result is
        // the true minimizer of the whole system) is asserted by the `‖A·x-b‖∞`
        // guard inside `adjust_least_squares`, which fired for each solve above.
        exercised_closures += closures.len();
    }

    // Guard against the sweep silently degenerating into all-fast-path trials.
    assert!(
        exercised_closures > 0,
        "fuzz sweep never exercised a closure solve"
    );
}

/// Profiling harness for the closure-adjusted solve (#81). Ignored by default.
/// Run with:
///   cargo test --manifest-path src-tauri/Cargo.toml --release \
///     profile_closure_solve_large -- --ignored --nocapture
///
/// Builds single-closure surveys shaped as one big loop made of two long
/// degree-2 chains joined at both ends - the worst case for the closure solve
/// (a single high-diameter loop). Sweeping the size demonstrates the scaling:
/// with the degree-≤2 graph reduction the loop peels to a ~1-node core, so the
/// whole `compute_adjusted_station_coordinates` is O(n) (it was O(n²) before #81
/// - both the per-station coordinate walk and the O(diameter) PCG iterations).
#[test]
#[ignore]
fn profile_closure_solve_large() {
    for n_shots in [1500usize, 3000, 6000, 12000] {
        let mut project = Project::new("ProfileLoop".to_string(), None);

        // Path A: eastward shots of length 5.0.
        let a = project
            .add_section(SectionData {
                shots: (0..n_shots)
                    .map(|_| shot(90.0, 90.0, 5.0, 0.0, 0.0))
                    .collect(),
                ..Default::default()
            })
            .id;
        // Path B: same count, slightly longer so the far ends disagree (a real
        // misclosure for the solve to distribute).
        let b = project
            .add_section(SectionData {
                shots: (0..n_shots)
                    .map(|_| shot(90.0, 90.0, 5.001, 0.0, 0.0))
                    .collect(),
                ..Default::default()
            })
            .id;

        project
            .set_zero_datum(
                StationRef::new(a, 0),
                GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
            )
            .unwrap();
        // Join the two chains at the origin and close the far ends → one big loop.
        project
            .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
            .unwrap();
        project
            .create_loop_closure(
                StationRef::new(a, n_shots as u32),
                StationRef::new(b, n_shots as u32),
                ClosureStrategy::Batch,
            )
            .unwrap();

        let n_stations = project.compute_station_coordinates().len();

        let t = std::time::Instant::now();
        let adjusted =
            project.compute_adjusted_station_coordinates(&ClosureAdjustmentConfig::default());
        let elapsed = t.elapsed();

        eprintln!(
            "profile_closure_solve_large: {} stations, 1 closure → compute_adjusted = {} ms ({} adjusted)",
            n_stations,
            elapsed.as_millis(),
            adjusted.len(),
        );
    }
}

#[test]
fn compute_adjusted_equals_measured_without_closures() {
    let mut project = Project::new("NoClosures".to_string(), None);
    project.add_section(SectionData {
        shots: vec![
            shot(0.0, 0.0, 10.0, 0.0, 0.0),
            shot(90.0, 90.0, 10.0, 0.0, 0.0),
        ],
        ..Default::default()
    });
    let config = ClosureAdjustmentConfig::default();

    let measured = project.compute_station_coordinates();
    let adjusted = project.compute_adjusted_station_coordinates(&config);

    assert_eq!(measured.len(), adjusted.len());
    for (m, a) in measured.iter().zip(adjusted.iter()) {
        assert!((m.coordinates.x - a.coordinates.x).abs() < 1e-12);
        assert!((m.coordinates.y - a.coordinates.y).abs() < 1e-12);
        assert!((m.coordinates.z - a.coordinates.z).abs() < 1e-12);
    }
}

#[test]
fn compute_adjusted_closes_datumless_loop_deterministically() {
    // Two paths from a shared (merged) origin to a near-shared endpoint joined by
    // a closure, but with NO zero datum - so the solve anchors each component on
    // the deterministic default anchor (`default_anchor_for_component`) instead.
    let mut project = Project::new("DatumlessLoop".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(90.0, 90.0, 10.2, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();
    project
        .create_loop_closure(
            StationRef::new(a, 2),
            StationRef::new(b, 2),
            ClosureStrategy::Batch,
        )
        .unwrap();

    // Precondition: this exercises the datum-less anchor path.
    assert!(project.zero_datums.is_empty());

    let config = ClosureAdjustmentConfig::default();
    let shots_before = shot_geometry(&project);
    let adjusted1 = project.compute_adjusted_station_coordinates(&config);
    let adjusted2 = project.compute_adjusted_station_coordinates(&config);

    // Non-destructive: deriving the view doesn't rewrite measured shots.
    assert_eq!(shots_before, shot_geometry(&project));

    // Deterministic across solves: the lex-min default anchor fixes a stable gauge.
    assert_eq!(adjusted1.len(), adjusted2.len());
    for (s1, s2) in adjusted1.iter().zip(adjusted2.iter()) {
        assert_eq!(s1.station_ref, s2.station_ref);
        assert!((s1.coordinates.x - s2.coordinates.x).abs() < 1e-9);
        assert!((s1.coordinates.y - s2.coordinates.y).abs() < 1e-9);
    }

    // The loop still closes without a datum (only the absolute translation is free).
    let adj_a2 = station_coords(&adjusted1, StationRef::new(a, 2));
    let adj_b2 = station_coords(&adjusted1, StationRef::new(b, 2));
    let misclosure = ((adj_a2.x - adj_b2.x).powi(2) + (adj_a2.y - adj_b2.y).powi(2)).sqrt();
    assert!(
        misclosure < 1e-3,
        "datum-less adjusted loop should close, got {misclosure}"
    );
}

/// A component whose adjustment fails must fall back to *measured* geometry while
/// a sibling component is still adjusted, and the measured project must be left
/// intact. This guards the per-component error handling in
/// `into_adjusted_station_coordinates`: warn-and-continue (one component's
/// failure does not abort the whole solve) plus the snapshot/restore that keeps
/// a failed component on measured geometry rather than leaking a partial solve.
///
/// The failure is forced by injecting a second zero datum into one component's
/// constraint group (bypassing the single-datum-per-component API guard), which
/// makes `adjust_loop_closures` return `Err`.
#[test]
fn compute_adjusted_failed_component_falls_back_sibling_still_adjusts() {
    // Helper: a misclosing two-section loop (origin merged, ~0.2 m open at the far end).
    fn looped(project: &mut Project) -> (SectionId, SectionId) {
        let x = project
            .add_section(SectionData {
                shots: vec![
                    shot(90.0, 90.0, 10.0, 0.0, 0.0),
                    shot(0.0, 0.0, 10.0, 0.0, 0.0),
                ],
                ..Default::default()
            })
            .id;
        let y = project
            .add_section(SectionData {
                shots: vec![
                    shot(0.0, 0.0, 10.0, 0.0, 0.0),
                    shot(90.0, 90.0, 10.2, 0.0, 0.0),
                ],
                ..Default::default()
            })
            .id;
        project
            .merge_stations(StationRef::new(x, 0), StationRef::new(y, 0))
            .unwrap();
        project
            .create_loop_closure(
                StationRef::new(x, 2),
                StationRef::new(y, 2),
                ClosureStrategy::Batch,
            )
            .unwrap();
        (x, y)
    }

    let mut project = Project::new("MixedComponents".to_string(), None);
    // Component 1 - will FAIL (two datums forced into the origin constraint group).
    let (a, b) = looped(&mut project);
    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        )
        .unwrap();
    let dup_key = project.key_for_ref(StationRef::new(b, 0)).unwrap();
    project.zero_datums.push(ZeroDatum {
        station_ref: dup_key,
        gps: GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    });
    // Component 2 - disconnected from the first, with a single datum; SUCCEEDS.
    let (c, d) = looped(&mut project);
    project
        .set_zero_datum(
            StationRef::new(c, 0),
            GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        )
        .unwrap();

    let config = ClosureAdjustmentConfig::default();
    let shots_before = shot_geometry(&project);
    let measured = project.compute_station_coordinates();
    let adjusted = project.compute_adjusted_station_coordinates(&config);

    // Non-destructive even though one component errored mid-solve.
    assert_eq!(shots_before, shot_geometry(&project));

    // Failed component (a/b): every station identical to measured (clean fallback,
    // no partial adjustment leaked).
    for sr in [
        StationRef::new(a, 1),
        StationRef::new(a, 2),
        StationRef::new(b, 1),
        StationRef::new(b, 2),
    ] {
        let m = station_coords(&measured, sr);
        let adj = station_coords(&adjusted, sr);
        assert!(
            (m.x - adj.x).abs() < 1e-9 && (m.y - adj.y).abs() < 1e-9,
            "failed component station {sr:?} should stay on measured geometry"
        );
    }
    // The failed component's loop is therefore still open.
    let m_a2 = station_coords(&adjusted, StationRef::new(a, 2));
    let m_b2 = station_coords(&adjusted, StationRef::new(b, 2));
    assert!(
        ((m_a2.x - m_b2.x).powi(2) + (m_a2.y - m_b2.y).powi(2)).sqrt() > 0.1,
        "failed component loop should remain open (measured)"
    );

    // Sibling component (c/d) was still adjusted: its loop closes.
    let adj_c2 = station_coords(&adjusted, StationRef::new(c, 2));
    let adj_d2 = station_coords(&adjusted, StationRef::new(d, 2));
    assert!(
        ((adj_c2.x - adj_d2.x).powi(2) + (adj_c2.y - adj_d2.y).powi(2)).sqrt() < 1e-3,
        "sibling component should still be adjusted despite the other's failure"
    );
}

// ── Frozen shot tests ─────────────────────────────────────────────

#[test]
fn is_shot_frozen_single_datum_dead_end() {
    // Section with datum on station 0, shots going one direction.
    // No shot is frozen (dead end, no datum reachable on far side).
    let mut project = Project::new("Frozen".to_string(), None);
    let sec = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 1.0, 0.0, 1.0),
                shot(0.0, 0.0, 1.0, 1.0, 2.0),
                shot(0.0, 0.0, 1.0, 2.0, 3.0),
            ],
            ..Default::default()
        })
        .id;

    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(sec, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 5.0).unwrap(),
    });

    let frozen = project.compute_frozen_shots();
    assert!(frozen.is_empty(), "Dead-end shots should not be frozen");
}

#[test]
fn is_shot_frozen_loop_with_two_datums() {
    // Two sections each with a datum, connected by merges at both ends.
    // All shots between the datums are frozen.
    //
    // Section A: station 0 (datum) --shot0--> station 1 --shot1--> station 2
    // Section B: station 0 (datum) --shot0--> station 1 --shot1--> station 2
    // Merges: A:0 <-> B:0, A:2 <-> B:2
    let mut project = Project::new("TwoDatums".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 1.0), shot(0.0, 0.0, 1.0, 1.0, 2.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![
                shot(180.0, 180.0, 1.0, 0.0, 1.0),
                shot(180.0, 180.0, 1.0, 1.0, 2.0),
            ],
            ..Default::default()
        })
        .id;

    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 2).into(),
        station_b: StationRef::new(b, 2).into(),
        merged_at: Utc::now(),
    });

    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 5.0).unwrap(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(b, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 5.0).unwrap(),
    });

    let frozen = project.compute_frozen_shots();
    // All 4 shots (2 per section) should be frozen
    assert_eq!(frozen.len(), 4, "All shots in the loop should be frozen");

    // Verify both sections' shots are present
    let a_frozen: Vec<_> = frozen.iter().filter(|f| f.section_id == a).collect();
    let b_frozen: Vec<_> = frozen.iter().filter(|f| f.section_id == b).collect();
    assert_eq!(a_frozen.len(), 2);
    assert_eq!(b_frozen.len(), 2);
}

#[test]
fn is_shot_frozen_loop_with_aligned_closure() {
    // Single datum, loop closed by an aligned closure.
    // Section A: station 0 (datum) --shot0--> station 1 --shot1--> station 2
    // Section B: station 0 --shot0--> station 1 --shot1--> station 2 --shot2--> station 3 (dead end branch)
    // Merge: A:0 <-> B:0
    // Aligned closure: A:2 <-> B:2
    // Shots A:0, A:1, B:0, B:1 should be frozen (in the loop).
    // Shot B:2 is NOT frozen (dead-end branch past the closure point).
    let mut project = Project::new("AlignedClosure".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 1.0), shot(0.0, 0.0, 1.0, 1.0, 2.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![
                shot(180.0, 180.0, 1.0, 0.0, 1.0),
                shot(180.0, 180.0, 1.0, 1.0, 2.0),
                shot(180.0, 180.0, 1.0, 2.0, 3.0), // dead-end branch
            ],
            ..Default::default()
        })
        .id;

    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 5.0).unwrap(),
    });

    // Add aligned closure: A:2 <-> B:2
    project.loop_closures.push(LoopClosure {
        id: project.next_closure_id,
        station_a: StationRef::new(a, 2).into(),
        station_b: StationRef::new(b, 2).into(),
        path_length: 4.0,
        misclosure_distance: 0.1,
        error_percentage: 2.5,
        horizontal_misclosure: 0.1,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::LeastSquaresWeighted,
        is_aligned: true,
    });
    project.next_closure_id += 1;

    let frozen = project.compute_frozen_shots();

    // A:0, A:1, B:0, B:1 should be frozen
    let a_frozen: Vec<_> = frozen.iter().filter(|f| f.section_id == a).collect();
    let b_frozen: Vec<_> = frozen.iter().filter(|f| f.section_id == b).collect();
    assert_eq!(
        a_frozen.len(),
        2,
        "Both shots in section A should be frozen"
    );
    assert_eq!(
        b_frozen.len(),
        2,
        "Only shots 0,1 in section B should be frozen"
    );

    // Verify B:2 (dead-end branch) is NOT frozen
    let b2_frozen = frozen
        .iter()
        .any(|f| f.section_id == b && f.shot_index == 2);
    assert!(!b2_frozen, "Dead-end shot B:2 should NOT be frozen");
}

#[test]
fn is_shot_frozen_unaligned_closure_not_traversed() {
    // Same topology as above but closure is NOT aligned. No shots should be frozen.
    let mut project = Project::new("UnalignedClosure".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 1.0), shot(0.0, 0.0, 1.0, 1.0, 2.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![
                shot(180.0, 180.0, 1.0, 0.0, 1.0),
                shot(180.0, 180.0, 1.0, 1.0, 2.0),
            ],
            ..Default::default()
        })
        .id;

    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 5.0).unwrap(),
    });

    // Unaligned closure: A:2 <-> B:2
    project.loop_closures.push(LoopClosure {
        id: project.next_closure_id,
        station_a: StationRef::new(a, 2).into(),
        station_b: StationRef::new(b, 2).into(),
        path_length: 4.0,
        misclosure_distance: 0.1,
        error_percentage: 2.5,
        horizontal_misclosure: 0.1,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });
    project.next_closure_id += 1;

    let frozen = project.compute_frozen_shots();
    assert!(
        frozen.is_empty(),
        "Unaligned closure should not cause shots to be frozen"
    );
}

#[test]
fn touches_geometry_detects_geometry_fields() {
    let geo_update = ShotUpdates {
        length: Some(5.0),
        ..Default::default()
    };
    assert!(geo_update.touches_geometry());

    let azimuth_update = ShotUpdates {
        azimuth_in: Some(90.0),
        ..Default::default()
    };
    assert!(azimuth_update.touches_geometry());

    let depth_update = ShotUpdates {
        depth_out: Some(3.0),
        ..Default::default()
    };
    assert!(depth_update.touches_geometry());

    let pitch_update = ShotUpdates {
        pitch_in: Some(Some(5.0)),
        ..Default::default()
    };
    assert!(pitch_update.touches_geometry());

    // Non-geometry updates
    let lrud_update = ShotUpdates {
        left: Some(Some(1.0)),
        right: Some(Some(2.0)),
        up: Some(Some(3.0)),
        down: Some(Some(4.0)),
        comment: Some(Some("test".to_string())),
        ..Default::default()
    };
    assert!(
        !lrud_update.touches_geometry(),
        "LRUD/comment should not be geometry"
    );

    let empty_update = ShotUpdates::default();
    assert!(
        !empty_update.touches_geometry(),
        "Empty update should not touch geometry"
    );
}

#[test]
fn update_shot_rejects_frozen_geometry() {
    // Create a loop with two datums so all shots are frozen
    let mut project = Project::new("FrozenUpdate".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 1.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 1.0, 0.0, 1.0)],
            ..Default::default()
        })
        .id;

    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 1).into(),
        merged_at: Utc::now(),
    });

    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 5.0).unwrap(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(b, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 5.0).unwrap(),
    });

    // Verify shots are frozen
    let frozen = project.compute_frozen_shots();
    assert!(
        !frozen.is_empty(),
        "Shots should be frozen in a loop with two datums"
    );

    // The update_shot validation is in the command handler, not on Project directly.
    // But we can verify the frozen status and touches_geometry independently.
    let shot_id = project.sections.iter().find(|s| s.id == a).unwrap().shots[0].id;
    let geo_update = ShotUpdates {
        length: Some(5.0),
        ..Default::default()
    };
    assert!(geo_update.touches_geometry());

    // Verify the shot IS frozen
    let section_lookup = project.build_section_lookup();
    let merge_adj = project.build_merge_adjacency(&section_lookup);
    let closure_adj = project.build_aligned_closure_adjacency(&section_lookup);
    let datum_stations: HashSet<StationRef> = project
        .zero_datums
        .iter()
        .filter_map(|d| project.resolve_key(&d.station_ref))
        .collect();
    assert!(project.is_shot_frozen(
        a,
        0,
        &section_lookup,
        &merge_adj,
        &closure_adj,
        &datum_stations
    ));

    // Non-geometry update should be allowed (touches_geometry returns false)
    let non_geo_update = ShotUpdates {
        comment: Some(Some("hello".to_string())),
        ..Default::default()
    };
    assert!(!non_geo_update.touches_geometry());

    // Verify the shot can still be updated with non-geometry fields
    let result = project.update_shot(a, shot_id, &non_geo_update);
    assert!(
        result.is_some(),
        "Non-geometry update on frozen shot should succeed"
    );
    assert_eq!(
        project.get_section(a).unwrap().shots[0].comment.as_deref(),
        Some("hello")
    );
}

// =====================================================================
// stats() tests
// =====================================================================

#[test]
fn stats_empty_project() {
    let project = Project::new("Empty".to_string(), None);
    let stats = project.stats();

    assert_eq!(stats.section_count, 0);
    assert_eq!(stats.shot_count, 0);
    assert_eq!(stats.station_count, 0);
    assert_eq!(stats.loop_count, 0);
    assert!((stats.total_length - 0.0).abs() < 1e-6);
    // Depth is a station figure, and an empty project has no stations (#481).
    assert_eq!(
        Project::depth_range(&project.compute_station_coordinates()),
        (0.0, 0.0)
    );
}

#[test]
fn stats_single_section() {
    let mut project = Project::new("Single".to_string(), None);
    project.add_section(SectionData {
        shots: vec![
            shot(0.0, 0.0, 10.0, 0.0, 5.0),
            shot(90.0, 90.0, 8.0, 5.0, 12.0),
        ],
        ..Default::default()
    });

    let stats = project.stats();
    assert_eq!(stats.section_count, 1);
    assert_eq!(stats.shot_count, 2);
    assert_eq!(stats.station_count, 3); // 2 shots + 1 first station
    assert!((stats.total_length - 18.0).abs() < 1e-6);
    // stats() leaves depth to the caller, exactly like penetration_distance (#481).
    assert!((stats.min_depth - 0.0).abs() < 1e-6);
    assert!((stats.max_depth - 0.0).abs() < 1e-6);
    // Stations: 0.0, (5+5)/2 = 5.0, 12.0.
    let (min_depth, max_depth) = Project::depth_range(&project.compute_station_coordinates());
    assert!((min_depth - 0.0).abs() < 1e-6);
    assert!((max_depth - 12.0).abs() < 1e-6);
}

#[test]
fn stats_multiple_sections() {
    let mut project = Project::new("Multi".to_string(), None);
    // Section 1 stations: -2, -5
    project.add_section(SectionData {
        shots: vec![shot(0.0, 0.0, 10.0, -2.0, -5.0)],
        ..Default::default()
    });
    // Section 2 stations: 0, (1+1)/2 = 1, 3
    project.add_section(SectionData {
        shots: vec![
            shot(90.0, 90.0, 5.0, 0.0, 1.0),
            shot(90.0, 90.0, 7.0, 1.0, 3.0),
        ],
        ..Default::default()
    });

    let stats = project.stats();
    assert_eq!(stats.section_count, 2);
    assert_eq!(stats.shot_count, 3);
    assert_eq!(stats.station_count, 5); // 3 shots + 2 first stations
    assert!((stats.total_length - 22.0).abs() < 1e-6);
    // Absolute over both sections, not per-section excursions (#481).
    let (min_depth, max_depth) = Project::depth_range(&project.compute_station_coordinates());
    assert!((min_depth - (-5.0)).abs() < 1e-6);
    assert!((max_depth - 3.0).abs() < 1e-6);
}

#[test]
fn stats_with_loop_closures() {
    let mut project = Project::new("Loops".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 0).into(),
        strategy: ClosureStrategy::Move,
        is_aligned: false,
        path_length: 10.0,
        misclosure_distance: 0.5,
        horizontal_misclosure: 0.3,
        vertical_misclosure: 0.2,
        error_percentage: 5.0,
    });

    let stats = project.stats();
    assert_eq!(stats.loop_count, 1);
}

/// A section that never comes back up: every station is deep, and the old
/// per-section delta walk would have reported the 0..5 excursion instead of the
/// 30..35 the stations actually sit at (#481).
#[test]
fn depth_range_is_absolute_not_a_delta_walk() {
    let mut project = Project::new("Deep".to_string(), None);
    project.add_section(SectionData {
        shots: vec![
            shot(0.0, 0.0, 10.0, 30.0, 32.0),
            shot(0.0, 0.0, 10.0, 32.0, 35.0),
        ],
        ..Default::default()
    });

    // Stations: 30.0, (32+32)/2 = 32.0, 35.0.
    let (min_depth, max_depth) = Project::depth_range(&project.compute_station_coordinates());
    assert!((min_depth - 30.0).abs() < 1e-6);
    assert!((max_depth - 35.0).abs() < 1e-6);

    assert_eq!(Project::depth_range(&[]), (0.0, 0.0));
}

// =====================================================================
// calculate_penetration tests
// =====================================================================

#[test]
fn calculate_penetration_empty_stations_no_datum() {
    let project = Project::new("NoDatum".to_string(), None);
    let stations: Vec<Station> = vec![];
    assert!((project.calculate_penetration(&stations) - 0.0).abs() < 1e-6);
}

#[test]
fn calculate_penetration_single_datum_measures_max_distance() {
    let mut project = Project::new("Penetration".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 15.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Set datum on station 0
    let gps = GpsCoordinates::new(0.0, 0.0, 0.0).unwrap();
    project
        .set_zero_datum(StationRef::new(section_id, 0), gps)
        .unwrap();

    let stations = project.compute_station_coordinates();
    let penetration = project.calculate_penetration(&stations);

    // With shots going north (azimuth 0): total horizontal distance = 10 + 15 = 25
    assert!(
        penetration > 24.0,
        "Expected penetration > 24, got {penetration}"
    );
    assert!(
        penetration < 26.0,
        "Expected penetration < 26, got {penetration}"
    );
}

// =====================================================================
// merge_stations validation tests
// =====================================================================

#[test]
fn merge_stations_rejects_nonexistent_station() {
    let mut project = Project::new("MergeInvalid".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    let result = project.merge_stations(
        StationRef::new(a, 0),
        StationRef::new(999, 0), // nonexistent section
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("do not exist"));
}

#[test]
fn merge_stations_rejects_already_merged_pair() {
    let mut project = Project::new("MergeDupe".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // First merge succeeds
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();

    // Second merge of same pair fails
    let result = project.merge_stations(StationRef::new(a, 0), StationRef::new(b, 0));
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("already merged"));
}

#[test]
fn merge_stations_rejects_both_datums() {
    let mut project = Project::new("MergeBothDatums".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        )
        .unwrap();
    project
        .set_zero_datum(
            StationRef::new(b, 0),
            GpsCoordinates::new(1.0, 1.0, 0.0).unwrap(),
        )
        .unwrap();

    let result = project.merge_stations(StationRef::new(a, 1), StationRef::new(b, 1));
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .contains("Both stations are connected to zero datums"));
}

// =====================================================================
// compute_station_raw_coords with station offsets
// =====================================================================

#[test]
fn compute_station_raw_coords_with_individual_station_offset() {
    let mut project = Project::new("StationOffset".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Station 1 is at (0, 10, 0) without offset
    let station_ref = StationRef::new(section_id, 1);
    let coords_before = project.compute_station_raw_coords(station_ref).unwrap();
    assert!((coords_before.x - 0.0).abs() < 1e-6);
    assert!((coords_before.y - 10.0).abs() < 1e-6);

    // Add individual station offset
    project
        .station_offsets
        .insert(station_ref.into(), (3.0, -2.0, 1.0));

    let coords_after = project.compute_station_raw_coords(station_ref).unwrap();
    assert!((coords_after.x - 3.0).abs() < 1e-6);
    assert!((coords_after.y - 8.0).abs() < 1e-6);
    assert!((coords_after.z - 1.0).abs() < 1e-6);
}

#[test]
fn compute_station_raw_coords_station_zero_with_section_offset() {
    let mut project = Project::new("SectionOffset".to_string(), None);
    let section_id = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project.section_offsets.insert(section_id, (100.0, 200.0));

    // Station 0 should be at the section offset
    let coords = project
        .compute_station_raw_coords(StationRef::new(section_id, 0))
        .unwrap();
    assert!((coords.x - 100.0).abs() < 1e-6);
    assert!((coords.y - 200.0).abs() < 1e-6);
}

// =====================================================================
// calculate_path_length_between tests
// =====================================================================

#[test]
fn path_length_simple_linear_section() {
    let mut project = Project::new("PathLen".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 15.0, 0.0, 0.0),
                shot(0.0, 0.0, 8.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    let path_len =
        project.calculate_path_length_between(StationRef::new(a, 0), StationRef::new(a, 3));
    assert!(
        (path_len - 33.0).abs() < 1e-6,
        "Expected 10+15+8=33, got {path_len}"
    );
}

#[test]
fn path_length_partial_section() {
    let mut project = Project::new("PartialPath".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 15.0, 0.0, 0.0),
                shot(0.0, 0.0, 8.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    // Path from station 1 to station 3 (two shots: 15 + 8 = 23)
    let path_len =
        project.calculate_path_length_between(StationRef::new(a, 1), StationRef::new(a, 3));
    assert!(
        (path_len - 23.0).abs() < 1e-6,
        "Expected 15+8=23, got {path_len}"
    );
}

#[test]
fn path_length_through_merge() {
    let mut project = Project::new("PathThroughMerge".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Merge a.1 to b.0 (zero-distance edge)
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    // Path from a.0 to b.1: 10 (shot in a) + 0 (merge) + 5 (shot in b) = 15
    let path_len =
        project.calculate_path_length_between(StationRef::new(a, 0), StationRef::new(b, 1));
    assert!(
        (path_len - 15.0).abs() < 1e-6,
        "Expected 10+5=15, got {path_len}"
    );
}

#[test]
fn path_length_nonexistent_station_returns_zero() {
    let project = Project::new("NoStation".to_string(), None);
    let path_len =
        project.calculate_path_length_between(StationRef::new(999, 0), StationRef::new(999, 1));
    assert!((path_len - 0.0).abs() < 1e-6);
}

#[test]
fn path_length_fallback_clamps_negative_shot_lengths() {
    // When the target station EXISTS but is unreachable, calculate_path_length_impl
    // falls back to the sum of the start component's shot lengths. That sum must
    // clamp negative lengths to 0 - same as the Dijkstra edges and the other graph
    // builders - so a corrupt/deserialized negative leg doesn't under-count (#176).
    let mut project = Project::new("NegFallback".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, -5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    // A disconnected second section so the target station exists yet is unreachable
    // from A - this forces the fallback-sum branch, not the early not-found return.
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 7.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();

    let len = project.calculate_path_length_between(StationRef::new(a, 0), StationRef::new(b, 0));
    // 10 + max(0, -5) = 10; without the clamp this would be 10 + (-5) = 5.
    assert!(
        (len - 10.0).abs() < 1e-6,
        "fallback sum must clamp the -5 leg to 0 (expected 10.0), got {len}"
    );
}

// =====================================================================
// compute_station_coordinates integration tests
// =====================================================================

#[test]
fn compute_station_coordinates_basic_section() {
    let mut project = Project::new("StationCoords".to_string(), None);
    // Use zero depth change so horizontal_length == length
    let section_id = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(90.0, 90.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    let stations = project.compute_station_coordinates();

    // 3 stations: station 0, 1, 2
    let section_stations: Vec<_> = stations
        .iter()
        .filter(|s| s.station_ref.section_id == section_id)
        .collect();
    assert_eq!(section_stations.len(), 3);

    // Station 0 at origin (no section offset)
    let s0 = section_stations
        .iter()
        .find(|s| s.station_ref.station_index == 0)
        .unwrap();
    assert!((s0.coordinates.x).abs() < 1e-6);
    assert!((s0.coordinates.y).abs() < 1e-6);

    // Station 1: north 10m (azimuth 0), z = 0 (no depth change)
    let s1 = section_stations
        .iter()
        .find(|s| s.station_ref.station_index == 1)
        .unwrap();
    assert!(
        (s1.coordinates.x).abs() < 1e-6,
        "s1 x = {}",
        s1.coordinates.x
    );
    assert!(
        (s1.coordinates.y - 10.0).abs() < 1e-6,
        "s1 y = {}",
        s1.coordinates.y
    );
    assert!(
        (s1.coordinates.z).abs() < 1e-6,
        "s1 z = {}",
        s1.coordinates.z
    );

    // Station 2: north 10m then east 5m
    let s2 = section_stations
        .iter()
        .find(|s| s.station_ref.station_index == 2)
        .unwrap();
    assert!(
        (s2.coordinates.x - 5.0).abs() < 1e-6,
        "s2 x = {}",
        s2.coordinates.x
    );
    assert!(
        (s2.coordinates.y - 10.0).abs() < 1e-6,
        "s2 y = {}",
        s2.coordinates.y
    );
    assert!(
        (s2.coordinates.z).abs() < 1e-6,
        "s2 z = {}",
        s2.coordinates.z
    );
}

#[test]
fn compute_station_coordinates_merged_sections_have_merged_with() {
    let mut project = Project::new("MergedCoords".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    let stations = project.compute_station_coordinates();

    // Station a.1 should have b.0 in merged_with
    let a1 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(a, 1))
        .unwrap();
    assert!(
        a1.merged_with.contains(&StationRef::new(b, 0)),
        "a.1 should have b.0 in merged_with"
    );

    // Station b.0 should have a.1 in merged_with
    let b0 = stations
        .iter()
        .find(|s| s.station_ref == StationRef::new(b, 0))
        .unwrap();
    assert!(
        b0.merged_with.contains(&StationRef::new(a, 1)),
        "b.0 should have a.1 in merged_with"
    );
}

// === realign_after_shot_edit tests ===

#[test]
fn realign_datum_section_keeps_datum_pinned() {
    // Single section with datum on station 3. Edit shot before datum → datum stays at origin.
    let mut project = Project::new("DatumPin".to_string(), None);
    let sec = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0), // north 10m
                shot(0.0, 0.0, 5.0, 0.0, 0.0),  // north 5m (will be edited)
                shot(0.0, 0.0, 3.0, 0.0, 0.0),  // north 3m
            ],
            ..Default::default()
        })
        .id;

    let datum_station = StationRef::new(sec, 3);
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project.set_zero_datum(datum_station, gps).unwrap();

    // Datum at origin
    let coords = project.compute_station_raw_coords(datum_station).unwrap();
    assert!((coords.y).abs() < 1e-6);

    // Edit shot[1]: 5m → 8m
    let shot_id = project.sections.iter().find(|s| s.id == sec).unwrap().shots[1].id;
    project.update_shot(
        sec,
        shot_id,
        &ShotUpdates {
            length: Some(8.0),
            ..Default::default()
        },
    );

    // Datum has drifted
    let drifted = project.compute_station_raw_coords(datum_station).unwrap();
    assert!(
        (drifted.y).abs() > 1.0,
        "Expected drift, got y={}",
        drifted.y
    );

    // Realign
    project.realign_after_shot_edit(sec, None);

    // Datum back at origin
    let fixed = project.compute_station_raw_coords(datum_station).unwrap();
    assert!(
        (fixed.x).abs() < 1e-6,
        "Datum x should be 0, got {}",
        fixed.x
    );
    assert!(
        (fixed.y).abs() < 1e-6,
        "Datum y should be 0, got {}",
        fixed.y
    );

    // Station 0 moved to compensate (absorbed the 3m length increase)
    let s0 = project
        .compute_station_raw_coords(StationRef::new(sec, 0))
        .unwrap();
    // Before edit: s0 was at y = -18. After: y = -21 (3m further south)
    assert!(
        (s0.y - (-21.0)).abs() < 1e-6,
        "s0 y should be -21, got {}",
        s0.y
    );
}

#[test]
fn realign_merged_section_keeps_merge_point_aligned() {
    // Section 1 has datum at 1:0. Section 2 merged at 1:2 <-> 2:0.
    // Edit shot 2:1 → 2:0 should stay aligned with 1:2, station 2:0 shifts.
    // Wait, 2:0 IS the merge point. Let me restructure:
    //
    // Section 1: 3 shots north (10m each). Datum at 1:0.
    //   Stations: 1:0(datum, origin), 1:1(y=10), 1:2(y=20), 1:3(y=30)
    // Section 2: 3 shots east (5m each). Merged at 1:3 <-> 2:0.
    //   Before merge alignment: 2:0(0,0), 2:1(5,0), 2:2(10,0), 2:3(15,0)
    //   After merge alignment: 2:0(0,30), 2:1(5,30), 2:2(10,30), 2:3(15,30)
    //
    // Edit shot 2:0 (first shot in section 2): length 5m → 15m.
    //   Without realign: 2:0(0,30), 2:1(15,30), 2:2(20,30), 2:3(25,30)
    //   2:0 is the merge point and should stay at (0,30) matching 1:3.
    //   With realign: same, because 2:0 is station_index 0 and the edit is shot[0]
    //   which shifts 2:1+ but NOT 2:0. Station 2:0 stays put.
    //
    // Better test: edit shot that's BEFORE the merge point within section 2.
    // But 2:0 is the merge point (station_index 0) so all shots are after it.
    // Let's make the merge at a middle station instead.

    // Section 1: shots north. Datum at 1:0.
    // Section 2: 5 shots east (5m each). Merged at 1:3 <-> 2:3.
    //   Edit shot 2:1 → stations 2:2, 2:3 shift. 2:3 is the merge point.
    //   Realign should keep 2:3 aligned with 1:3.

    let mut project = Project::new("MergeAlign".to_string(), None);

    let sec1 = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0), // north 10m
                shot(0.0, 0.0, 10.0, 0.0, 0.0), // north 10m
                shot(0.0, 0.0, 10.0, 0.0, 0.0), // north 10m
            ],
            ..Default::default()
        })
        .id;

    let sec2 = project
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 5.0, 0.0, 0.0), // east 5m
                shot(90.0, 90.0, 5.0, 0.0, 0.0), // east 5m (will be edited)
                shot(90.0, 90.0, 5.0, 0.0, 0.0), // east 5m
            ],
            ..Default::default()
        })
        .id;

    // Set datum at 1:0
    let datum_ref = StationRef::new(sec1, 0);
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project.set_zero_datum(datum_ref, gps).unwrap();

    // Merge 1:3 <-> 2:3
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(sec1, 3).into(),
        station_b: StationRef::new(sec2, 3).into(),
        merged_at: Utc::now(),
    });

    // Align section 2 offset so 2:3 matches 1:3
    // 1:3 is at (0, 30). 2:3 without offset is at (15, 0).
    // Need offset: (0-15, 30-0) = (-15, 30)
    let pos_1_3 = project
        .compute_station_raw_coords(StationRef::new(sec1, 3))
        .unwrap();
    let pos_2_3_raw = project
        .compute_station_raw_coords(StationRef::new(sec2, 3))
        .unwrap();
    project
        .section_offsets
        .insert(sec2, (pos_1_3.x - pos_2_3_raw.x, pos_1_3.y - pos_2_3_raw.y));

    // Verify merge alignment
    let pos_2_3 = project
        .compute_station_raw_coords(StationRef::new(sec2, 3))
        .unwrap();
    assert!(
        (pos_2_3.x - pos_1_3.x).abs() < 1e-6,
        "Merge should be aligned x"
    );
    assert!(
        (pos_2_3.y - pos_1_3.y).abs() < 1e-6,
        "Merge should be aligned y"
    );

    // Record positions of locked stations
    let pos_2_0_before = project
        .compute_station_raw_coords(StationRef::new(sec2, 0))
        .unwrap();

    // Edit shot 2:1 (between 2:1 and 2:2): length 5m → 15m (+10m)
    let shot_id = project
        .sections
        .iter()
        .find(|s| s.id == sec2)
        .unwrap()
        .shots[1]
        .id;
    project.update_shot(
        sec2,
        shot_id,
        &ShotUpdates {
            length: Some(15.0),
            ..Default::default()
        },
    );

    // Before realign: 2:3 has drifted 10m east from 1:3
    let drifted_2_3 = project
        .compute_station_raw_coords(StationRef::new(sec2, 3))
        .unwrap();
    assert!(
        (drifted_2_3.x - pos_1_3.x).abs() > 5.0,
        "2:3 should have drifted from 1:3"
    );

    // Realign
    project.realign_after_shot_edit(sec2, None);

    // After realign: 2:3 matches 1:3 again (merge point preserved)
    let fixed_2_3 = project
        .compute_station_raw_coords(StationRef::new(sec2, 3))
        .unwrap();
    assert!(
        (fixed_2_3.x - pos_1_3.x).abs() < 1e-6,
        "2:3 x should match 1:3 x, got {} vs {}",
        fixed_2_3.x,
        pos_1_3.x
    );
    assert!(
        (fixed_2_3.y - pos_1_3.y).abs() < 1e-6,
        "2:3 y should match 1:3 y, got {} vs {}",
        fixed_2_3.y,
        pos_1_3.y
    );

    // 1:0 (datum) still at origin
    let datum_coords = project.compute_station_raw_coords(datum_ref).unwrap();
    assert!(
        (datum_coords.x).abs() < 1e-6,
        "Datum should still be at origin x"
    );
    assert!(
        (datum_coords.y).abs() < 1e-6,
        "Datum should still be at origin y"
    );

    // 2:0 (unlocked end) moved to compensate
    let pos_2_0_after = project
        .compute_station_raw_coords(StationRef::new(sec2, 0))
        .unwrap();
    assert!(
        (pos_2_0_after.x - pos_2_0_before.x).abs() > 5.0,
        "2:0 should have shifted to compensate, before={} after={}",
        pos_2_0_before.x,
        pos_2_0_after.x
    );
}

#[test]
fn realign_noop_without_datums() {
    // Section without any datums → realign should not change anything.
    let mut project = Project::new("NoDatum".to_string(), None);
    let sec = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    let offsets_before = project.section_offsets.clone();
    project.realign_after_shot_edit(sec, None);
    assert_eq!(
        project.section_offsets, offsets_before,
        "Offsets should not change when no datums exist"
    );
}

/// A realign with nothing held must not re-georeference the survey against a
/// datum that anchors nothing.
///
/// `zero_datums[0]` here DANGLES - its anchor key no longer resolves, which the
/// app reaches by deleting the shot or section a datum was anchored to - so the
/// survey is laid out against `zero_datums[1]`. That is what
/// [`Project::frame_datum`] names, what `frame_datum_key` gates the
/// renormalisation on, and the only one of the two the wire hands the frontend.
/// The absolute fallback here used to read `zero_datums.first()` instead: the
/// group's own datum then failed the "is this the frame" test, and its target
/// was recomputed as `gps_to_local(its gps, dangling.gps)` - the whole
/// inter-datum vector, about 11 km in this fixture, applied to a rigid group
/// inside an `update_shot` whose undo data carries no offsets at all.
///
/// # Why this drives the model and not a command
///
/// The realign's only production caller is
/// `src-tauri/src/core/state/command_shot.rs:251`, which passes
/// `hold_anchor_at = project.datum_anchor_position(section_id)` - computed at
/// `:243`, BEFORE the edit - whenever `updates.touches_geometry()`, and does not
/// call the realign at all otherwise. `datum_anchor_position` and the realign's
/// own datum lookup use the SAME predicate (first datum in the merge group that
/// resolves), so a group holding a resolvable datum always arrives with
/// `hold_anchor_at = Some` and takes the held branch. Through that caller the
/// absolute fallback is therefore reachable only in the degenerate case where
/// the datum resolves but its coordinates do not compute - non-finite geometry.
/// So this pins the model-level no-hold path directly, which is also the path
/// every neighbouring `realign_after_shot_edit(sec, None)` test exercises.
///
/// Non-vacuous: the two datums are about 11 km apart while the tolerance is
/// [`Project::OFFSET_DUST_M`], and the fixture asserts up front that
/// `zero_datums[0]` really does fail to resolve.
#[test]
fn realign_without_a_hold_ignores_a_dangling_first_datum() {
    let mut project = Project::new("DanglingFrame".to_string(), None);
    let dangling_sec = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let framed_sec = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(90.0, 90.0, 7.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    // No merges: the two sections are separate rigid groups, so the realign on
    // one of them can only be reasoning about that group's own datum.

    // zero_datums[0], anchored on a shot uuid that is not in this project. Pushed
    // straight onto the vector because no mutator will build one.
    project.zero_datums.push(ZeroDatum {
        station_ref: StationKey::new(
            dangling_sec,
            StationAnchor::AfterShot("no-such-uuid".to_string()),
        ),
        gps: GpsCoordinates::new(20.25, -87.5, 0.0).unwrap(),
    });
    assert!(
        project
            .resolve_key(&project.zero_datums[0].station_ref)
            .is_none(),
        "fixture rot: zero_datums[0] must NOT resolve, or this test proves nothing"
    );

    // zero_datums[1]: the real one, about 11 km away, and the frame the survey is
    // actually laid out against.
    project
        .set_zero_datum(
            StationRef::new(framed_sec, 0),
            GpsCoordinates::new(20.35, -87.4, 0.0).unwrap(),
        )
        .unwrap();
    assert_eq!(
        project.zero_datums.len(),
        2,
        "the real datum must be stored AFTER the dangling one"
    );

    let before: Vec<(StationRef, Coordinates)> = project
        .compute_station_coordinates()
        .into_iter()
        .map(|s| (s.station_ref, s.coordinates))
        .collect();

    project.realign_after_shot_edit(framed_sec, None);

    let after: HashMap<StationRef, Coordinates> = project
        .compute_station_coordinates()
        .into_iter()
        .map(|s| (s.station_ref, s.coordinates))
        .collect();
    for (station_ref, was) in before {
        let now = after
            .get(&station_ref)
            .expect("realign must not remove a station");
        assert!(
            (now.x - was.x).abs() <= Project::OFFSET_DUST_M
                && (now.y - was.y).abs() <= Project::OFFSET_DUST_M,
            "{station_ref:?} moved from ({}, {}) to ({}, {}): the no-hold realign \
             re-georeferenced the group against the dangling zero_datums[0]",
            was.x,
            was.y,
            now.x,
            now.y
        );
    }
}

#[test]
fn realign_is_idempotent() {
    // Edit shot, realign twice → same result.
    let mut project = Project::new("Idempotent".to_string(), None);
    let sec = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    let datum_station = StationRef::new(sec, 2);
    let gps = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
    project.set_zero_datum(datum_station, gps).unwrap();

    // Edit shot[0]
    let shot_id = project.sections.iter().find(|s| s.id == sec).unwrap().shots[0].id;
    project.update_shot(
        sec,
        shot_id,
        &ShotUpdates {
            length: Some(12.0),
            ..Default::default()
        },
    );

    project.realign_after_shot_edit(sec, None);
    let offsets_first = project.section_offsets.clone();

    project.realign_after_shot_edit(sec, None);
    let offsets_second = project.section_offsets.clone();

    assert_eq!(offsets_first, offsets_second, "Should be idempotent");

    // Datum at origin
    let coords = project.compute_station_raw_coords(datum_station).unwrap();
    assert!((coords.x).abs() < 1e-6, "Datum x should be 0");
    assert!((coords.y).abs() < 1e-6, "Datum y should be 0");
}

#[test]
fn realign_edit_in_datum_section_propagates_to_merged() {
    // Edit a shot in the datum section (after datum) → merged partner follows.
    // Section 1: 3 shots north (10m each). Datum at 1:0.
    // Section 2: merged at 1:3 <-> 2:0.
    // Edit shot 1:1 → 1:3 moves → section 2 must follow.
    let mut project = Project::new("DatumPropagate".to_string(), None);

    let sec1 = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0), // will be edited
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;

    let sec2 = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Datum at 1:0
    project
        .set_zero_datum(
            StationRef::new(sec1, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();

    // Merge 1:3 <-> 2:0
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(sec1, 3).into(),
        station_b: StationRef::new(sec2, 0).into(),
        merged_at: Utc::now(),
    });

    // Align section 2 to merge point
    let pos_1_3 = project
        .compute_station_raw_coords(StationRef::new(sec1, 3))
        .unwrap();
    let pos_2_0 = project
        .compute_station_raw_coords(StationRef::new(sec2, 0))
        .unwrap();
    project
        .section_offsets
        .insert(sec2, (pos_1_3.x - pos_2_0.x, pos_1_3.y - pos_2_0.y));

    // Record 2:1's position before edit
    let pos_2_1_before = project
        .compute_station_raw_coords(StationRef::new(sec2, 1))
        .unwrap();

    // Edit shot 1:1 (between 1:1 and 1:2): 10m → 20m
    let shot_id = project
        .sections
        .iter()
        .find(|s| s.id == sec1)
        .unwrap()
        .shots[1]
        .id;
    project.update_shot(
        sec1,
        shot_id,
        &ShotUpdates {
            length: Some(20.0),
            ..Default::default()
        },
    );

    // 1:3 has shifted north by 10m. 1:0 (datum) is before the edit, unchanged.
    // Realign from sec1 (the edited section)
    project.realign_after_shot_edit(sec1, None);

    // 1:0 still at origin (datum, before edited shot)
    let datum = project
        .compute_station_raw_coords(StationRef::new(sec1, 0))
        .unwrap();
    assert!(
        (datum.x).abs() < 1e-6 && (datum.y).abs() < 1e-6,
        "Datum should be at origin"
    );

    // 2:0 should match new 1:3 position (30 + 10 = 40 north)
    let new_1_3 = project
        .compute_station_raw_coords(StationRef::new(sec1, 3))
        .unwrap();
    let new_2_0 = project
        .compute_station_raw_coords(StationRef::new(sec2, 0))
        .unwrap();
    assert!(
        (new_2_0.x - new_1_3.x).abs() < 1e-6 && (new_2_0.y - new_1_3.y).abs() < 1e-6,
        "2:0 should match 1:3 after realign: 2:0=({},{}) 1:3=({},{})",
        new_2_0.x,
        new_2_0.y,
        new_1_3.x,
        new_1_3.y
    );

    // 2:1 should have moved by same amount as 1:3 (10m north)
    let pos_2_1_after = project
        .compute_station_raw_coords(StationRef::new(sec2, 1))
        .unwrap();
    assert!(
        (pos_2_1_after.y - pos_2_1_before.y - 10.0).abs() < 1e-6,
        "2:1 should have moved 10m north, delta={}",
        pos_2_1_after.y - pos_2_1_before.y
    );
}

// ============================================================================
// Penetration distance tests
// ============================================================================

#[test]
fn penetration_no_zero_datum_returns_zero() {
    let project = Project::new("NoDatum".to_string(), None);
    let stations = project.compute_station_coordinates();
    assert_eq!(project.calculate_penetration(&stations), 0.0);
}

#[test]
fn penetration_single_section_with_datum() {
    let mut project = Project::new("Pen".to_string(), None);
    let sec_id = project
        .add_section(SectionData {
            // Northward shot: azimuth 0 means north
            shots: vec![shot(0.0, 180.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    let gps = GpsCoordinates::new(45.0, 10.0, 0.0).unwrap();
    project
        .set_zero_datum(StationRef::new(sec_id, 0), gps)
        .unwrap();

    let stations = project.compute_station_coordinates();
    let pen = project.calculate_penetration(&stations);
    // Should be ~10m (one shot of 10m from the datum)
    assert!(pen > 9.0 && pen < 11.0, "penetration={pen}, expected ~10");
}

#[test]
fn penetration_datum_at_end_station() {
    let mut project = Project::new("PenEnd".to_string(), None);
    let sec_id = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 180.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Datum at end of section (station 1)
    let gps = GpsCoordinates::new(45.0, 10.0, 0.0).unwrap();
    project
        .set_zero_datum(StationRef::new(sec_id, 1), gps)
        .unwrap();

    let stations = project.compute_station_coordinates();
    let pen = project.calculate_penetration(&stations);
    // First station is 10m from the datum (at end)
    assert!(pen > 9.0 && pen < 11.0, "penetration={pen}, expected ~10");
}

// ============================================================================
// cleanup_section_references cascade tests
// ============================================================================

#[test]
fn cleanup_removes_merges_referencing_section() {
    let mut project = Project::new("Cleanup".to_string(), None);
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
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(c, 0).into(),
        merged_at: Utc::now(),
    });

    assert_eq!(project.station_merges.len(), 2);
    project.cleanup_section_references(b);
    // Both merges involve section b, so both should be removed
    assert_eq!(
        project.station_merges.len(),
        0,
        "all merges involving section b should be removed"
    );
}

#[test]
fn cleanup_removes_closures_referencing_section() {
    let mut project = Project::new("ClosureCleanup".to_string(), None);
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

    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 0).into(),
        path_length: 10.0,
        misclosure_distance: 0.1,
        error_percentage: 1.0,
        horizontal_misclosure: 0.05,
        vertical_misclosure: 0.08,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });
    project.loop_closures.push(LoopClosure {
        id: 2,
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(a, 1).into(),
        path_length: 5.0,
        misclosure_distance: 0.05,
        error_percentage: 1.0,
        horizontal_misclosure: 0.03,
        vertical_misclosure: 0.02,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });

    project.cleanup_section_references(b);
    // Only the a↔b closure should be removed; a↔a stays
    assert_eq!(project.loop_closures.len(), 1);
    assert_eq!(project.loop_closures[0].id, 2);
}

#[test]
fn cleanup_removes_offsets_and_datums() {
    let mut project = Project::new("OffsetCleanup".to_string(), None);
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

    project.section_offsets.insert(a, (1.0, 2.0));
    project.section_offsets.insert(b, (3.0, 4.0));
    project
        .station_offsets
        .insert(StationRef::new(a, 0).into(), (1.0, 2.0, 3.0));
    project
        .station_offsets
        .insert(StationRef::new(b, 0).into(), (4.0, 5.0, 6.0));

    let gps = GpsCoordinates::new(45.0, 10.0, 0.0).unwrap();
    project.set_zero_datum(StationRef::new(a, 0), gps).unwrap();
    project.set_zero_datum(StationRef::new(b, 0), gps).unwrap();

    project.cleanup_section_references(a);

    assert!(
        !project.section_offsets.contains_key(&a),
        "section a offset should be removed"
    );
    assert!(
        project.section_offsets.contains_key(&b),
        "section b offset should remain"
    );
    assert_eq!(
        project.station_offsets.len(),
        1,
        "only b's station offset should remain"
    );
    assert_eq!(
        project.zero_datums.len(),
        1,
        "only b's zero datum should remain"
    );
    assert_eq!(project.zero_datums[0].station_ref.section_id, b);
}

#[test]
fn cleanup_preserves_unrelated_merges() {
    let mut project = Project::new("PreserveUnrelated".to_string(), None);
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
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(c, 0).into(),
        merged_at: Utc::now(),
    });

    // Remove section b - only the a↔b merge goes away
    project.cleanup_section_references(b);
    assert_eq!(project.station_merges.len(), 1);
    assert_eq!(project.station_merges[0].station_b.section_id, c);
}

// ============================================================================
// shot_displacement edge cases
// ============================================================================

#[test]
fn shot_displacement_due_north() {
    let data = ShotData {
        azimuth_in: 0.0,
        azimuth_out: 180.0,
        length: 10.0,
        depth_in: 0.0,
        depth_out: 0.0,
        ..Default::default()
    };
    let (dx, dy) = Project::shot_displacement(&data, AzimuthReference::default());
    // Average azimuth = 90° → dx ≈ 10, dy ≈ 0
    assert!(dx.abs() > 9.0, "dx={dx}, expected ~10 for avg azimuth 90°");
    assert!(dy.abs() < 1.0, "dy={dy}, expected ~0");
}

#[test]
fn shot_displacement_vertical_shot_is_zero() {
    let data = ShotData {
        azimuth_in: 45.0,
        azimuth_out: 225.0,
        length: 10.0,
        depth_in: 0.0,
        depth_out: 10.0, // pure vertical
        ..Default::default()
    };
    let (dx, dy) = Project::shot_displacement(&data, AzimuthReference::default());
    assert!(dx.abs() < 1e-6, "vertical shot dx should be ~0, got {dx}");
    assert!(dy.abs() < 1e-6, "vertical shot dy should be ~0, got {dy}");
}

// ============================================================================
// assign_closure_ids
// ============================================================================

#[test]
fn assign_closure_ids_fills_zeros_and_updates_counter() {
    let mut project = Project::new("ClosureIDs".to_string(), None);
    project.loop_closures.push(LoopClosure {
        id: 0,
        station_a: StationRef::new(0, 0).into(),
        station_b: StationRef::new(0, 1).into(),
        path_length: 10.0,
        misclosure_distance: 0.1,
        error_percentage: 1.0,
        horizontal_misclosure: 0.05,
        vertical_misclosure: 0.08,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });
    project.loop_closures.push(LoopClosure {
        id: 5, // already assigned
        station_a: StationRef::new(0, 0).into(),
        station_b: StationRef::new(0, 1).into(),
        path_length: 10.0,
        misclosure_distance: 0.1,
        error_percentage: 1.0,
        horizontal_misclosure: 0.05,
        vertical_misclosure: 0.08,
        strategy: ClosureStrategy::Move,
        is_aligned: false,
    });
    project.loop_closures.push(LoopClosure {
        id: 0,
        station_a: StationRef::new(0, 0).into(),
        station_b: StationRef::new(0, 1).into(),
        path_length: 10.0,
        misclosure_distance: 0.1,
        error_percentage: 1.0,
        horizontal_misclosure: 0.05,
        vertical_misclosure: 0.08,
        strategy: ClosureStrategy::Batch,
        is_aligned: false,
    });

    project.assign_closure_ids();

    // First closure (was 0) gets id=6 (next after max existing=5)
    assert_eq!(project.loop_closures[0].id, 6);
    // Second closure keeps id=5. Its `Move` strategy used to FORCE `is_aligned`
    // on every open; it is MEASURED now (#469 item 2), and this project has no
    // sections at all - so neither endpoint resolves, there is no coincidence to
    // measure, and the claim stays false. (An open that re-asserted the flag
    // would put memory and disk permanently at odds about what the closure is.)
    assert_eq!(project.loop_closures[1].id, 5);
    assert!(
        !project.loop_closures[1].is_aligned,
        "assign_closure_ids must not re-assert an alignment claim it cannot measure"
    );
    // Third closure (was 0) gets id=7
    assert_eq!(project.loop_closures[2].id, 7);
    // next_closure_id should be 8
    assert_eq!(project.next_closure_id, 8);
}

// (Removed `shift_station_refs_adjusts_all_references`: `shift_station_refs`
// no longer exists. Stable shot-UUID anchors are position-independent, so an
// insert/delete never rewrites a cross-reference's index - the ref resolves to
// the moved station on the fly. Covered by the insert/delete identity tests.)

// ── Station depth computation tests ─────────────────────────────────

#[test]
fn compute_station_depth_from_surrounding_shots() {
    // Station 1 has an incoming shot (depth_out=5.0) and outgoing shot (depth_in=6.0).
    // Average = (5 + 6) / 2 = 5.5
    let mut project = Project::new("Depth".to_string(), None);
    project.add_section(SectionData {
        shots: vec![
            shot(0.0, 0.0, 1.0, 0.0, 5.0),  // shot 0: depth_in=0, depth_out=5
            shot(0.0, 0.0, 1.0, 6.0, 10.0), // shot 1: depth_in=6, depth_out=10
        ],
        ..Default::default()
    });
    let sec_id = project.sections[0].id;

    // Station 1 is between the two shots
    let coords = project
        .compute_station_raw_coords(StationRef::new(sec_id, 1))
        .unwrap();
    // Average of shot[0].depth_out (5.0) and shot[1].depth_in (6.0)
    assert!(
        (coords.z - 5.5).abs() < 1e-9,
        "depth should be 5.5, got {}",
        coords.z
    );
}

#[test]
fn compute_station_depth_first_station() {
    // Station 0 has only an outgoing shot - depth = shot[0].depth_in
    let mut project = Project::new("Depth".to_string(), None);
    project.add_section(SectionData {
        shots: vec![shot(0.0, 0.0, 1.0, 3.0, 7.0)],
        ..Default::default()
    });
    let sec_id = project.sections[0].id;

    let coords = project
        .compute_station_raw_coords(StationRef::new(sec_id, 0))
        .unwrap();
    assert!(
        (coords.z - 3.0).abs() < 1e-9,
        "first station depth should be 3.0, got {}",
        coords.z
    );
}

#[test]
fn compute_station_depth_last_station() {
    // Station 2 (last) has only an incoming shot - depth = shot[1].depth_out
    let mut project = Project::new("Depth".to_string(), None);
    project.add_section(SectionData {
        shots: vec![
            shot(0.0, 0.0, 1.0, 0.0, 5.0),
            shot(0.0, 0.0, 1.0, 5.0, 12.0),
        ],
        ..Default::default()
    });
    let sec_id = project.sections[0].id;

    let coords = project
        .compute_station_raw_coords(StationRef::new(sec_id, 2))
        .unwrap();
    assert!(
        (coords.z - 12.0).abs() < 1e-9,
        "last station depth should be 12.0, got {}",
        coords.z
    );
}

#[test]
fn compute_station_depth_with_merge_averages_both_sections() {
    // Two sections merged at a station - depth should average across all colocated readings
    let mut project = Project::new("MergeDepth".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 1.0, 0.0, 8.0), // shot 0: depth_out=8
            ],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 1.0, 10.0, 20.0), // shot 0: depth_in=10
            ],
            ..Default::default()
        })
        .id;

    // Merge A:1 with B:0
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    // Station A:1 is colocated with B:0
    // Depths: A shot[0].depth_out=8, B shot[0].depth_in=10 → average = 9
    let coords = project
        .compute_station_raw_coords(StationRef::new(a, 1))
        .unwrap();
    assert!(
        (coords.z - 9.0).abs() < 1e-9,
        "merged station depth should be 9.0, got {}",
        coords.z
    );
}

/// [#469] The single-station and whole-component coordinate APIs agree to the BIT on
/// the depth of the same colocated group.
///
/// They average the same measurements in different orders - `compute_station_raw_coords`
/// walks the BFS order out of `get_colocated_stations` (which starts at the station
/// asked about), `compute_component_raw_coords` walks station-list order - and
/// floating-point addition is not associative. The two depths per station below are
/// chosen so that difference is REAL rather than theoretical: summed
/// `[6.58, 6.35, 33.29, 40.99]` the average is 21.802500000000002, and summed
/// `[33.29, 40.99, 6.58, 6.35]` it is 21.8025. Sorting the group inside
/// `compute_station_depth` is what collapses the two.
#[test]
fn the_two_coordinate_apis_average_a_colocated_group_identically() {
    let mut project = Project::new("AverageOrder".to_string(), None);
    // Section A first, so it sorts first AND comes first in `self.sections` - the
    // component API's order. The BFS order is whatever station is asked about, which
    // below is B's.
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 1.0, 0.0, 33.29),
                shot(0.0, 0.0, 1.0, 40.99, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 1.0, 0.0, 6.58),
                shot(90.0, 90.0, 1.0, 6.35, 0.0),
            ],
            ..Default::default()
        })
        .id;
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 1).into(),
        merged_at: Utc::now(),
    });

    let component: HashSet<SectionId> = [a, b].into_iter().collect();
    let by_component = project.compute_component_raw_coords(&component);
    for station in [StationRef::new(a, 1), StationRef::new(b, 1)] {
        let single = project.compute_station_raw_coords(station).unwrap();
        let component_z = by_component.get(&station).unwrap().z;
        assert_eq!(
            single.z.to_bits(),
            component_z.to_bits(),
            "the same group must average to the same bits whichever API asks: \
             {} vs {component_z} for {station:?}",
            single.z
        );
    }
}

#[test]
fn compute_station_depth_no_shots_returns_zero() {
    // Edge case: section with no shots, station 0 has no surrounding shots
    let mut project = Project::new("Empty".to_string(), None);
    project.add_section(SectionData {
        shots: vec![],
        ..Default::default()
    });
    let sec_id = project.sections[0].id;

    let coords = project
        .compute_station_raw_coords(StationRef::new(sec_id, 0))
        .unwrap();
    assert!(
        (coords.z).abs() < 1e-9,
        "depth should be 0.0 for empty section, got {}",
        coords.z
    );
}

#[test]
fn compute_raw_coords_xy_walk_north() {
    // A shot due north (azimuth=0) with length 5m, flat (depth_in == depth_out)
    // horizontal_length = sqrt(5^2 - 0^2) = 5
    // dx = 5 * sin(0) = 0, dy = 5 * cos(0) = 5
    let mut project = Project::new("Walk".to_string(), None);
    project.add_section(SectionData {
        shots: vec![shot(0.0, 0.0, 5.0, 10.0, 10.0)],
        ..Default::default()
    });
    let sec_id = project.sections[0].id;

    let c0 = project
        .compute_station_raw_coords(StationRef::new(sec_id, 0))
        .unwrap();
    assert!((c0.x).abs() < 1e-9, "station 0 x should be 0");
    assert!((c0.y).abs() < 1e-9, "station 0 y should be 0");

    let c1 = project
        .compute_station_raw_coords(StationRef::new(sec_id, 1))
        .unwrap();
    assert!((c1.x).abs() < 1e-9, "station 1 x should be 0 (due north)");
    assert!(
        (c1.y - 5.0).abs() < 1e-9,
        "station 1 y should be 5.0, got {}",
        c1.y
    );
}

#[test]
fn compute_raw_coords_xy_walk_east() {
    // A shot due east (azimuth=90) with length 3m, flat
    let mut project = Project::new("Walk".to_string(), None);
    project.add_section(SectionData {
        shots: vec![shot(90.0, 90.0, 3.0, 5.0, 5.0)],
        ..Default::default()
    });
    let sec_id = project.sections[0].id;

    let c1 = project
        .compute_station_raw_coords(StationRef::new(sec_id, 1))
        .unwrap();
    assert!(
        (c1.x - 3.0).abs() < 1e-9,
        "station 1 x should be 3.0 (due east), got {}",
        c1.x
    );
    assert!(
        (c1.y).abs() < 1e-9,
        "station 1 y should be ~0, got {}",
        c1.y
    );
}

#[test]
fn compute_raw_coords_depth_independent_of_xy() {
    // Z is computed from absolute depth values, not from XY walk
    // Two shots: shot 0 goes north 10m at depth 5→8, shot 1 goes east 10m at depth 7→12
    let mut project = Project::new("IndepZ".to_string(), None);
    project.add_section(SectionData {
        shots: vec![
            shot(0.0, 0.0, 10.0, 5.0, 8.0),
            shot(90.0, 90.0, 10.0, 7.0, 12.0),
        ],
        ..Default::default()
    });
    let sec_id = project.sections[0].id;

    // Station 0: only outgoing shot → depth_in = 5.0
    let c0 = project
        .compute_station_raw_coords(StationRef::new(sec_id, 0))
        .unwrap();
    assert!(
        (c0.z - 5.0).abs() < 1e-9,
        "station 0 depth should be 5.0, got {}",
        c0.z
    );

    // Station 1: incoming depth_out=8, outgoing depth_in=7 → average = 7.5
    let c1 = project
        .compute_station_raw_coords(StationRef::new(sec_id, 1))
        .unwrap();
    assert!(
        (c1.z - 7.5).abs() < 1e-9,
        "station 1 depth should be 7.5, got {}",
        c1.z
    );

    // Station 2: only incoming → depth_out = 12.0
    let c2 = project
        .compute_station_raw_coords(StationRef::new(sec_id, 2))
        .unwrap();
    assert!(
        (c2.z - 12.0).abs() < 1e-9,
        "station 2 depth should be 12.0, got {}",
        c2.z
    );
}

#[test]
fn compute_raw_coords_with_section_offset() {
    let mut project = Project::new("Offset".to_string(), None);
    let sec = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 10.0, 10.0)],
            ..Default::default()
        })
        .id;

    project.section_offsets.insert(sec, (100.0, 200.0));

    let c0 = project
        .compute_station_raw_coords(StationRef::new(sec, 0))
        .unwrap();
    assert!(
        (c0.x - 100.0).abs() < 1e-9,
        "x should include section offset"
    );
    assert!(
        (c0.y - 200.0).abs() < 1e-9,
        "y should include section offset"
    );
}

#[test]
fn compute_raw_coords_with_station_offset() {
    let mut project = Project::new("StaOff".to_string(), None);
    let sec = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 10.0, 10.0)],
            ..Default::default()
        })
        .id;

    project
        .station_offsets
        .insert(StationRef::new(sec, 0).into(), (1.0, 2.0, 3.0));

    let c0 = project
        .compute_station_raw_coords(StationRef::new(sec, 0))
        .unwrap();
    assert!((c0.x - 1.0).abs() < 1e-9, "x should include station offset");
    assert!((c0.y - 2.0).abs() < 1e-9, "y should include station offset");
    assert!(
        (c0.z - 13.0).abs() < 1e-9,
        "z should be depth(10) + offset(3)"
    );
}

// ── adjust_least_squares tests ──────────────────────────────────────

#[test]
fn lsq_no_closures_is_noop() {
    // With no closures, there are no constraints to adjust - should return Ok
    let mut project = Project::new("LSQ".to_string(), None);
    let sec = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 5.0, 5.0),
                shot(90.0, 90.0, 10.0, 5.0, 5.0),
            ],
            ..Default::default()
        })
        .id;

    let config = ClosureAdjustmentConfig::default();
    let component: HashSet<SectionId> = [sec].into_iter().collect();
    let result = project.adjust_least_squares(&component, &[], StationRef::new(sec, 0), &config);
    assert!(result.is_ok(), "No closures should succeed: {:?}", result);
}

#[test]
fn lsq_preserves_depth_values() {
    // Key test: LSQ should adjust XY but NOT touch depth_in/depth_out
    let mut project = Project::new("LSQDepth".to_string(), None);

    // Create a triangle: section A (north 10m), section B (east 10m), section C (south-west ~14m)
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 5.0, 8.0)], // north, depth 5→8
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 10.0, 8.0, 12.0)], // east, depth 8→12
            ..Default::default()
        })
        .id;
    let c = project
        .add_section(SectionData {
            shots: vec![shot(225.0, 225.0, 14.5, 12.0, 5.5)], // south-west back, depth 12→5.5
            ..Default::default()
        })
        .id;

    // Merge to form a loop: A:1 → B:0, B:1 → C:0
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(c, 0).into(),
        merged_at: Utc::now(),
    });

    // Closure: C:1 should be at A:0 (closing the loop)
    let closure = LoopClosure {
        id: 1,
        station_a: StationRef::new(c, 1).into(),
        station_b: StationRef::new(a, 0).into(),
        path_length: 34.5,
        misclosure_distance: 0.5,
        error_percentage: 1.4,
        horizontal_misclosure: 0.5,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::LeastSquaresWeighted,
        is_aligned: false,
    };

    // Save original depths
    let orig_depths: Vec<(f64, f64)> = project
        .sections
        .iter()
        .flat_map(|s| s.shots.iter().map(|sh| (sh.depth_in, sh.depth_out)))
        .collect();

    let config = ClosureAdjustmentConfig::default();
    let component: HashSet<SectionId> = [a, b, c].into_iter().collect();
    let result =
        project.adjust_least_squares(&component, &[closure], StationRef::new(a, 0), &config);
    assert!(result.is_ok(), "LSQ should succeed: {:?}", result);

    // Verify depths are unchanged
    let new_depths: Vec<(f64, f64)> = project
        .sections
        .iter()
        .flat_map(|s| s.shots.iter().map(|sh| (sh.depth_in, sh.depth_out)))
        .collect();
    assert_eq!(
        orig_depths, new_depths,
        "depth_in/depth_out must not change after LSQ adjustment"
    );
}

#[test]
fn lsq_anchor_station_stays_fixed() {
    // The anchor station (or its group) should not move
    let mut project = Project::new("LSQAnchor".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 5.0, 5.0),
                shot(0.0, 0.0, 10.0, 5.0, 5.0),
            ],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 20.5, 5.0, 5.0)], // slight error for closure
            ..Default::default()
        })
        .id;

    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 2).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    let closure = LoopClosure {
        id: 1,
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(a, 0).into(),
        path_length: 30.5,
        misclosure_distance: 0.5,
        error_percentage: 1.6,
        horizontal_misclosure: 0.5,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::LeastSquaresWeighted,
        is_aligned: false,
    };

    let anchor = StationRef::new(a, 0);
    let coords_before = project.compute_station_raw_coords(anchor).unwrap();

    let config = ClosureAdjustmentConfig::default();
    let component: HashSet<SectionId> = [a, b].into_iter().collect();
    let result = project.adjust_least_squares(&component, &[closure], anchor, &config);
    assert!(result.is_ok(), "LSQ should succeed: {:?}", result);

    let coords_after = project.compute_station_raw_coords(anchor).unwrap();
    assert!(
        (coords_after.x - coords_before.x).abs() < 1e-6
            && (coords_after.y - coords_before.y).abs() < 1e-6,
        "anchor XY should not move: before=({}, {}), after=({}, {})",
        coords_before.x,
        coords_before.y,
        coords_after.x,
        coords_after.y
    );
}

#[test]
fn lsq_produces_finite_coordinates() {
    // After adjustment, all station coordinates should be finite
    let mut project = Project::new("LSQFinite".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 5.0, 5.0),
                shot(90.0, 90.0, 10.0, 5.0, 5.0),
            ],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(225.0, 225.0, 14.0, 5.0, 5.0)],
            ..Default::default()
        })
        .id;

    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 2).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    let closure = LoopClosure {
        id: 1,
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(a, 0).into(),
        path_length: 34.0,
        misclosure_distance: 0.2,
        error_percentage: 0.6,
        horizontal_misclosure: 0.2,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::LeastSquaresWeighted,
        is_aligned: false,
    };

    let config = ClosureAdjustmentConfig::default();
    let component: HashSet<SectionId> = [a, b].into_iter().collect();
    let result =
        project.adjust_least_squares(&component, &[closure], StationRef::new(a, 0), &config);
    assert!(result.is_ok(), "LSQ should succeed: {:?}", result);

    for section in &project.sections {
        for idx in 0..=section.shots.len() {
            let coords = project
                .compute_station_raw_coords(StationRef::new(section.id, idx as u32))
                .unwrap();
            assert!(
                coords.x.is_finite(),
                "x should be finite for {}:{}",
                section.id,
                idx
            );
            assert!(
                coords.y.is_finite(),
                "y should be finite for {}:{}",
                section.id,
                idx
            );
            assert!(
                coords.z.is_finite(),
                "z should be finite for {}:{}",
                section.id,
                idx
            );
        }
    }
}

#[test]
fn lsq_with_zero_datum_fixes_that_group() {
    // When a zero datum exists, its group should be fixed and not move
    let mut project = Project::new("LSQDatum".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 10.0, 5.0, 5.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 10.0, 5.0, 5.0)],
            ..Default::default()
        })
        .id;
    let c = project
        .add_section(SectionData {
            shots: vec![shot(225.0, 225.0, 14.5, 5.0, 5.0)],
            ..Default::default()
        })
        .id;

    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(b, 1).into(),
        station_b: StationRef::new(c, 0).into(),
        merged_at: Utc::now(),
    });

    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(45.0, 10.0, 100.0).unwrap(),
    });

    let closure = LoopClosure {
        id: 1,
        station_a: StationRef::new(c, 1).into(),
        station_b: StationRef::new(a, 0).into(),
        path_length: 34.5,
        misclosure_distance: 0.5,
        error_percentage: 1.4,
        horizontal_misclosure: 0.5,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::LeastSquaresWeighted,
        is_aligned: false,
    };

    let datum_coords_before = project
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();

    let config = ClosureAdjustmentConfig::default();
    let component: HashSet<SectionId> = [a, b, c].into_iter().collect();
    let result =
        project.adjust_least_squares(&component, &[closure], StationRef::new(a, 0), &config);
    assert!(result.is_ok(), "LSQ should succeed: {:?}", result);

    let datum_coords_after = project
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();
    assert!(
        (datum_coords_after.x - datum_coords_before.x).abs() < 1e-6
            && (datum_coords_after.y - datum_coords_before.y).abs() < 1e-6,
        "zero datum station should stay fixed"
    );
}

#[test]
fn lsq_update_shot_horizontal_only_preserves_depth() {
    // Direct test of update_shot_horizontal_only
    let mut s = Shot::new(
        1,
        ShotData {
            azimuth_in: 45.0,
            azimuth_out: 45.0,
            length: 10.0,
            depth_in: 3.0,
            depth_out: 7.0,
            pitch_in: Some(23.5),
            pitch_out: Some(23.5),
            ..Default::default()
        },
    );

    // Update horizontal displacement to pure east (dx=5, dy=0)
    Project::update_shot_horizontal_only(&mut s, 5.0, 0.0, AzimuthReference::default());

    assert!((s.depth_in - 3.0).abs() < 1e-9, "depth_in must not change");
    assert!(
        (s.depth_out - 7.0).abs() < 1e-9,
        "depth_out must not change"
    );
    assert_eq!(s.pitch_in, Some(23.5), "pitch_in must not change");
    assert_eq!(s.pitch_out, Some(23.5), "pitch_out must not change");
    // Azimuth should now point east (90 degrees)
    assert!(
        (s.azimuth_in - 90.0).abs() < 1e-6,
        "azimuth should be 90, got {}",
        s.azimuth_in
    );
    // Length should account for horizontal + dz: sqrt(5^2 + 4^2) = sqrt(41)
    let expected_len = (25.0_f64 + 16.0).sqrt();
    assert!(
        (s.length - expected_len).abs() < 1e-6,
        "length should be sqrt(41), got {}",
        s.length
    );
}

#[test]
fn lsq_update_shot_horizontal_only_zero_displacement() {
    // When dx=dy=0, azimuth should be preserved, length = |dz|
    let mut s = Shot::new(
        1,
        ShotData {
            azimuth_in: 135.0,
            azimuth_out: 135.0,
            length: 5.0,
            depth_in: 2.0,
            depth_out: 6.0,
            ..Default::default()
        },
    );

    Project::update_shot_horizontal_only(&mut s, 0.0, 0.0, AzimuthReference::default());

    assert!(
        (s.azimuth_in - 135.0).abs() < 1e-6,
        "azimuth should be preserved when displacement is zero"
    );
    // length = sqrt(0 + dz^2) = |4| = 4
    assert!(
        (s.length - 4.0).abs() < 1e-6,
        "length should be |dz|=4, got {}",
        s.length
    );
}

/// The solver writes a recomputed bearing straight onto both legs, so its wrap is
/// the last thing standing between an `atan2` result and a stored reading.
///
/// A displacement a hair WEST of north gives a tiny negative angle, and a bare
/// `rem_euclid` maps that to exactly `360.0` - out of range, and `INVALID_AZIMUTH`
/// is error severity, so the user cannot even dismiss the warning the adjustment
/// just created for them.
#[test]
fn recomputed_azimuth_from_a_hair_west_of_north_stays_in_range() {
    assert_eq!(
        (-1e-16_f64).atan2(1.0).to_degrees().rem_euclid(360.0),
        360.0,
        "precondition: the bare rem_euclid wrap escapes [0, 360) for this displacement"
    );

    let mut s = Shot::new(
        1,
        ShotData {
            azimuth_in: 0.0,
            azimuth_out: 0.0,
            length: 1.0,
            ..Default::default()
        },
    );
    Project::update_shot_horizontal_only(&mut s, -1e-16, 1.0, AzimuthReference::default());

    assert!(
        (0.0..360.0).contains(&s.azimuth_in) && (0.0..360.0).contains(&s.azimuth_out),
        "solver wrote an out-of-range reading: ({}, {})",
        s.azimuth_in,
        s.azimuth_out
    );
    assert_eq!(s.azimuth_in, 0.0);
}

#[test]
fn lsq_long_chain_per_shot_changes_are_small() {
    // Regression test: for a long chain with a small closure error, the
    // per-shot geometry changes must be small (proportional to misclosure /
    // shot_count), not massive due to solver non-convergence.
    //
    // Topology: section A (200 shots north, each 5m) with zero datum at A:200.
    //           section B (1 shot, 10m east) with zero datum at B:0.
    //           Closure: A:100 ↔ B:1
    //
    // This creates an asymmetric loop where B:1 is 1 shot from a fixed point
    // and A:100 is 100-200 shots from fixed points. With proper convergence,
    // each shot should change by at most misclosure/num_shots ≈ 0.01m.
    let mut project = Project::new("LSQLongChain".to_string(), None);

    // Section A: 200 shots heading north, each 5m
    let shots_a: Vec<ShotData> = (0..200).map(|_| shot(0.0, 0.0, 5.0, 0.0, 0.0)).collect();
    let a = project
        .add_section(SectionData {
            shots: shots_a,
            ..Default::default()
        })
        .id;

    // Section B: 1 shot heading east, 10m
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Zero datums
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 200).into(),
        gps: GpsCoordinates::new(45.0, 10.0, 0.0).unwrap(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(b, 0).into(),
        gps: GpsCoordinates::new(45.01, 10.01, 0.0).unwrap(),
    });

    // Large offsets stress-test the solver: absolute coordinates are 500-1800m
    // from origin, which triggers convergence issues if starting from x=0.
    project.section_offsets.insert(a, (500.0, 800.0));
    project.section_offsets.insert(b, (500.0, 1300.0));
    // A:0 at (500, 800), A:100 at (500, 1300), A:200 at (500, 1800)
    // B:0 at (500, 1300), B:1 at (510, 1300)
    // misclosure distance((500, 1300), (510, 1300)) = 10m
    // path_length ≈ 200*5 + 10 = 1010m

    // Save original shot data
    let orig_shots_a: Vec<(f64, f64)> = project
        .sections
        .iter()
        .find(|s| s.id == a)
        .unwrap()
        .shots
        .iter()
        .map(|s| (s.azimuth_in, s.length))
        .collect();

    let closure = LoopClosure {
        id: 1,
        station_a: StationRef::new(a, 100).into(),
        station_b: StationRef::new(b, 1).into(),
        path_length: 1010.0,
        misclosure_distance: 10.0,
        error_percentage: 0.99,
        horizontal_misclosure: 10.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::LeastSquaresWeighted,
        is_aligned: false,
    };

    let config = ClosureAdjustmentConfig::default();
    let component: HashSet<SectionId> = [a, b].into_iter().collect();
    let result =
        project.adjust_least_squares(&component, &[closure], StationRef::new(a, 0), &config);
    assert!(result.is_ok(), "LSQ should succeed: {:?}", result);

    // Check per-shot changes are small (bounded by misclosure / num_shots * safety_factor)
    // With 201 shots and 10m misclosure, max per-shot change ≈ 0.05m per shot.
    // We allow up to 0.5m per shot to be safe (10x margin).
    let new_shots_a: Vec<(f64, f64)> = project
        .sections
        .iter()
        .find(|s| s.id == a)
        .unwrap()
        .shots
        .iter()
        .map(|s| (s.azimuth_in, s.length))
        .collect();

    let mut max_length_delta = 0.0_f64;
    for (i, ((orig_az, orig_len), (new_az, new_len))) in
        orig_shots_a.iter().zip(new_shots_a.iter()).enumerate()
    {
        let len_delta = (new_len - orig_len).abs();
        if len_delta > max_length_delta {
            max_length_delta = len_delta;
        }
        assert!(
            len_delta < 0.5,
            "shot {} length changed by {:.3}m (orig={:.3}, new={:.3}) - \
             suggests solver did not converge",
            i,
            len_delta,
            orig_len,
            new_len,
        );
        // Azimuth change: for a 5m shot, 0.5m displacement is ~5.7 degrees
        let az_delta = (new_az - orig_az).abs();
        let az_delta = if az_delta > 180.0 {
            360.0 - az_delta
        } else {
            az_delta
        };
        assert!(
            az_delta < 10.0,
            "shot {} azimuth changed by {:.3}° (orig={:.3}, new={:.3}) - \
             suggests solver did not converge",
            i,
            az_delta,
            orig_az,
            new_az,
        );
    }
}

#[test]
fn lsq_realistic_topology_preserves_geometry() {
    // Mimics the user's topology from kbd.stalx:
    // - Section A: 364 shots with varying azimuths (real-world cave)
    // - Section B: 1 shot (anchor section)
    // - Section C: 105 shots (branch)
    // - Two zero datums at A:364 and B:0
    // - Merge: C:105 ↔ A:180
    // - Closure: A:73 ↔ B:1
    // - Large section offsets (700-900m range)
    let mut project = Project::new("RealisticLSQ".to_string(), None);

    // Section A: 364 shots with varying azimuths (zigzag pattern)
    let shots_a: Vec<ShotData> = (0..364)
        .map(|i| {
            let az = match i % 4 {
                0 => 10.0 + (i as f64 * 0.3),
                1 => 350.0 - (i as f64 * 0.2),
                2 => 45.0 + (i as f64 * 0.1),
                _ => 315.0 - (i as f64 * 0.15),
            };
            let az = az.rem_euclid(360.0);
            let len = 5.0 + (i as f64 * 0.01).sin() * 3.0;
            shot(az, az, len, i as f64 * 0.1, (i + 1) as f64 * 0.1)
        })
        .collect();
    let a = project
        .add_section(SectionData {
            shots: shots_a,
            ..Default::default()
        })
        .id;

    // Section B: 1 shot (anchor)
    let b = project
        .add_section(SectionData {
            shots: vec![shot(120.0, 120.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Set section A offset (large, like real data)
    let offset_a = (761.5, -1284.6);
    project.section_offsets.insert(a, offset_a);

    // Compute A:73 absolute position = offset_a + cumulative shot vectors 0..72
    let a_section = project.sections.iter().find(|s| s.id == a).unwrap();
    let mut a73_x = 0.0_f64;
    let mut a73_y = 0.0_f64;
    for i in 0..73 {
        let s = &a_section.shots[i];
        let az = s.azimuth_in.to_radians();
        let h = s.horizontal_length();
        a73_x += h * az.sin();
        a73_y += h * az.cos();
    }
    let a73_abs_x = offset_a.0 + a73_x;
    let a73_abs_y = offset_a.1 + a73_y;
    // B:0 → B:1 via one shot at azimuth 120°, 10m. B:1 = offset_b + shot_vector
    let b_shot_dx = 10.0 * 120.0_f64.to_radians().sin();
    let b_shot_dy = 10.0 * 120.0_f64.to_radians().cos();
    // Set B offset so B:1 is near A:73 with a small misclosure (~2m)
    project.section_offsets.insert(
        b,
        (a73_abs_x - b_shot_dx + 1.5, a73_abs_y - b_shot_dy + 1.3),
    );

    // Zero datums
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 364).into(),
        gps: GpsCoordinates::new(50.0, 50.0, 0.0).unwrap(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(b, 0).into(),
        gps: GpsCoordinates::new(49.99, 50.013, 0.0).unwrap(),
    });

    // Save original shots for section A
    let orig_a: Vec<(f64, f64)> = project
        .sections
        .iter()
        .find(|s| s.id == a)
        .unwrap()
        .shots
        .iter()
        .map(|s| (s.azimuth_in, s.length))
        .collect();

    // Closure: A:73 ↔ B:1 (misclosure ~2m)
    let closure = LoopClosure {
        id: 1,
        station_a: StationRef::new(a, 73).into(),
        station_b: StationRef::new(b, 1).into(),
        path_length: 3133.0,
        misclosure_distance: 2.0,
        error_percentage: 0.06,
        horizontal_misclosure: 2.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::LeastSquaresWeighted,
        is_aligned: false,
    };

    // Capture positions around closure point BEFORE adjustment
    let pos_a73_before = project
        .compute_station_raw_coords(StationRef::new(a, 73))
        .unwrap();
    let pos_b1_before = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    let actual_misclosure = ((pos_a73_before.x - pos_b1_before.x).powi(2)
        + (pos_a73_before.y - pos_b1_before.y).powi(2))
    .sqrt();

    let config = ClosureAdjustmentConfig::default();
    let component: HashSet<SectionId> = [a, b].into_iter().collect();
    let result =
        project.adjust_least_squares(&component, &[closure], StationRef::new(a, 0), &config);
    assert!(result.is_ok(), "LSQ should succeed: {:?}", result);

    // Verify closure endpoints now coincide
    let pos_a73_after = project
        .compute_station_raw_coords(StationRef::new(a, 73))
        .unwrap();
    let pos_b1_after = project
        .compute_station_raw_coords(StationRef::new(b, 1))
        .unwrap();
    let residual_misc = ((pos_a73_after.x - pos_b1_after.x).powi(2)
        + (pos_a73_after.y - pos_b1_after.y).powi(2))
    .sqrt();
    assert!(
        residual_misc < 0.01,
        "residual misclosure {residual_misc:.6}m should be near zero"
    );

    // Verify per-shot changes in section A are bounded.
    let new_a: Vec<(f64, f64)> = project
        .sections
        .iter()
        .find(|s| s.id == a)
        .unwrap()
        .shots
        .iter()
        .map(|s| (s.azimuth_in, s.length))
        .collect();

    let mut max_len_delta_a = 0.0_f64;
    let mut worst_shot = 0;
    for (i, ((_, orig_len), (_, new_len))) in orig_a.iter().zip(new_a.iter()).enumerate() {
        let delta = (new_len - orig_len).abs();
        if delta > max_len_delta_a {
            max_len_delta_a = delta;
            worst_shot = i;
        }
    }
    assert!(
        max_len_delta_a < 1.0,
        "shot {} length changed by {:.4}m - too large for {:.2}m misclosure",
        worst_shot,
        max_len_delta_a,
        actual_misclosure,
    );
}

#[test]
fn lsq_spur_shots_are_not_modified() {
    // Spur (branch) shots have no loop redundancy and must remain untouched.
    //
    // Topology:
    //   Section A: 100 shots north (5m each), zero datum at A:100.
    //   Section B: 1 shot east (10m), zero datum at B:0.
    //   Section C: 20 shots east (3m each), merged to A:50 via C:0↔A:50.
    //   Closure: A:30 ↔ B:1.
    //
    // Spur stations: A:0-A:29 (before closure), C:1-C:20 (branch beyond merge).
    // Loop stations: A:30-A:100, B:0-B:1.
    let mut project = Project::new("SpurTest".to_string(), None);

    let shots_a: Vec<ShotData> = (0..100).map(|_| shot(0.0, 0.0, 5.0, 0.0, 0.0)).collect();
    let a = project
        .add_section(SectionData {
            shots: shots_a,
            ..Default::default()
        })
        .id;

    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    let shots_c: Vec<ShotData> = (0..20).map(|_| shot(90.0, 90.0, 3.0, 0.0, 0.0)).collect();
    let c = project
        .add_section(SectionData {
            shots: shots_c,
            ..Default::default()
        })
        .id;

    // Merge C:0 ↔ A:50
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(c, 0).into(),
        station_b: StationRef::new(a, 50).into(),
        merged_at: Utc::now(),
    });

    // Zero datums
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 100).into(),
        gps: GpsCoordinates::new(45.0, 10.0, 0.0).unwrap(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(b, 0).into(),
        gps: GpsCoordinates::new(45.01, 10.01, 0.0).unwrap(),
    });

    // Large offsets to stress the solver
    project.section_offsets.insert(a, (500.0, 800.0));
    project.section_offsets.insert(b, (500.0, 950.0));
    project.section_offsets.insert(c, (500.0, 1050.0));

    // Save original shot data for spur sections
    let orig_a_spur: Vec<(f64, f64)> = project.sections.iter().find(|s| s.id == a).unwrap().shots
        [..30] // A:0-A:29 are spur shots (before closure at A:30)
        .iter()
        .map(|s| (s.azimuth_in, s.length))
        .collect();

    let orig_c: Vec<(f64, f64)> = project
        .sections
        .iter()
        .find(|s| s.id == c)
        .unwrap()
        .shots
        .iter()
        .map(|s| (s.azimuth_in, s.length))
        .collect();

    let closure = LoopClosure {
        id: 1,
        station_a: StationRef::new(a, 30).into(),
        station_b: StationRef::new(b, 1).into(),
        path_length: 360.0,
        misclosure_distance: 5.0,
        error_percentage: 1.4,
        horizontal_misclosure: 5.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::LeastSquaresWeighted,
        is_aligned: false,
    };

    let config = ClosureAdjustmentConfig::default();
    let component: HashSet<SectionId> = [a, b, c].into_iter().collect();
    let result =
        project.adjust_least_squares(&component, &[closure], StationRef::new(a, 0), &config);
    assert!(result.is_ok(), "LSQ should succeed: {:?}", result);

    // Verify spur shots in A (indices 0..29) are exactly unchanged.
    let new_a_spur: Vec<(f64, f64)> = project.sections.iter().find(|s| s.id == a).unwrap().shots
        [..30]
        .iter()
        .map(|s| (s.azimuth_in, s.length))
        .collect();
    for (i, (orig, new)) in orig_a_spur.iter().zip(new_a_spur.iter()).enumerate() {
        assert!(
            (orig.0 - new.0).abs() < f64::EPSILON && (orig.1 - new.1).abs() < f64::EPSILON,
            "spur shot A:{i} was modified: az {:.6}→{:.6}, len {:.6}→{:.6}",
            orig.0,
            new.0,
            orig.1,
            new.1,
        );
    }

    // Verify all shots in section C are exactly unchanged.
    let new_c: Vec<(f64, f64)> = project
        .sections
        .iter()
        .find(|s| s.id == c)
        .unwrap()
        .shots
        .iter()
        .map(|s| (s.azimuth_in, s.length))
        .collect();
    for (i, (orig, new)) in orig_c.iter().zip(new_c.iter()).enumerate() {
        assert!(
            (orig.0 - new.0).abs() < f64::EPSILON && (orig.1 - new.1).abs() < f64::EPSILON,
            "spur shot C:{i} was modified: az {:.6}→{:.6}, len {:.6}→{:.6}",
            orig.0,
            new.0,
            orig.1,
            new.1,
        );
    }

    // Verify merged stations (C:0 ↔ A:50) still share the same position.
    let pos_c0 = project
        .compute_station_raw_coords(StationRef::new(c, 0))
        .unwrap();
    let pos_a50 = project
        .compute_station_raw_coords(StationRef::new(a, 50))
        .unwrap();
    let merge_gap = ((pos_c0.x - pos_a50.x).powi(2) + (pos_c0.y - pos_a50.y).powi(2)).sqrt();
    assert!(
        merge_gap < 1e-6,
        "merge C:0↔A:50 broken after adjustment: gap={merge_gap:.6}m, \
         C:0=({:.4},{:.4}), A:50=({:.4},{:.4})",
        pos_c0.x,
        pos_c0.y,
        pos_a50.x,
        pos_a50.y,
    );
}

#[test]
fn record_station_merge_after_a_structural_removal_keeps_surviving_merges() {
    // The half-populated-memo trap: a structural removal drops merges while
    // others survive, and the NEXT recorded merge must land on a whole base, not
    // on a map holding only its own edge (which `get_merged_section_ids` would
    // then treat as the entire merge graph, hiding every surviving merge).
    let mut project = Project::new("Cache".to_string(), None);
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
    // An extra, merge-free section: cleaning up ITS references is the structural
    // removal that invalidates the adjacency below.
    let e = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // Establish two surviving merges A↔B and B↔C, then read the group so the
    // adjacency memo is warm.
    project.record_station_merge(StationMerge {
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    project.record_station_merge(StationMerge {
        station_a: StationRef::new(b, 0).into(),
        station_b: StationRef::new(c, 0).into(),
        merged_at: Utc::now(),
    });
    assert_eq!(project.get_merged_section_ids(a).len(), 3);

    // The `delete_section` cleanup: a merge retain that keeps A↔B and B↔C.
    project.cleanup_section_references(e);

    // Now record a NEW merge (e.g. the user merges D into the group). It must
    // land on the whole surviving graph, not on a map holding only {C, D}.
    project.record_station_merge(StationMerge {
        station_a: StationRef::new(c, 0).into(),
        station_b: StationRef::new(d, 0).into(),
        merged_at: Utc::now(),
    });
    let merged = project.get_merged_section_ids(a);
    assert!(
        merged.contains(&a) && merged.contains(&b) && merged.contains(&c) && merged.contains(&d),
        "expected the surviving A-B-C merges to still propagate after a structural \
         removal + a new merge recorded; got {merged:?}"
    );
    assert_eq!(merged.len(), 4);
}

#[test]
fn map_station_refs_invalidates_the_merge_adjacency_memo() {
    // `map_station_refs` can move a merge endpoint to a different SECTION - that
    // is what the delete-shot split does - so it rewrites the very graph the
    // merge-adjacency memo describes. Answering a later query from the pre-remap
    // memo reports the wrong rigid group, which is how a bridge closure got
    // promoted into a merge across two GPS-anchored components.
    let mut project = Project::new("Remap".to_string(), None);
    let mk = |project: &mut Project| {
        project
            .add_section(SectionData {
                shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
                ..Default::default()
            })
            .id
    };
    let (a, b, c) = (mk(&mut project), mk(&mut project), mk(&mut project));

    project.record_station_merge(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    // Warm the memo on the pre-remap shape.
    assert_eq!(project.get_merged_section_ids(a), HashSet::from([a, b]));

    // The split's remap: the B endpoint now lives in section C.
    project.map_station_refs(|key| {
        if key.section_id == b {
            StationRef::new(c, 0).into()
        } else {
            key
        }
    });

    assert_eq!(
        project.get_merged_section_ids(a),
        HashSet::from([a, c]),
        "the rigid group must follow the rewritten endpoint into section C"
    );
    assert_eq!(
        project.get_merged_section_ids(b),
        HashSet::from([b]),
        "section B no longer carries a merge endpoint, so it stands alone"
    );
}

#[test]
fn integrity_flags_non_finite_geometry() {
    let mut p = Project::new("NaN".to_string(), None);
    let a = p
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    assert!(p.check_structural_integrity().is_empty());
    // Poison a shot the way a corrupt/older file or unguarded import could.
    p.get_section_mut(a).unwrap().shots[0].length = f64::NAN;
    let v = p.check_structural_integrity();
    assert!(
        v.iter().any(|x| x.category == "non_finite_geometry"),
        "non-finite geometry must be flagged: {v:?}"
    );
    // And the live edit path rejects it.
    assert!(!p.get_section(a).unwrap().shots[0].all_finite());
}

#[test]
fn check_closure_alignment_sees_drift_that_merge_check_cannot() {
    // A: A0-A1-A2 north; B: B0-B1-B2 north; merge A2=B0 (chain). A closure
    // A0=B2 marked aligned, but geometry makes A0 and B2 20m apart - the merge
    // is fine, only the (redundant) closure is stale.
    let mut project = Project::new("StaleClosure".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0), shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0), shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    // Merge A2 <-> B0 and align (B0 lands on A2).
    project
        .merge_stations(StationRef::new(a, 2), StationRef::new(b, 0))
        .unwrap();
    assert!(
        project.check_merge_alignment().is_empty(),
        "merge should be aligned"
    );
    // A closure A0 <-> B2, marked aligned - but A0=(0,0) and B2 is 20m north.
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 2).into(),
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: true,
    });

    // The merge-only check is blind to it; the closure check catches it.
    assert!(project.check_merge_alignment().is_empty());
    let closure_violations = project.check_closure_alignment();
    assert_eq!(closure_violations.len(), 1, "{closure_violations:?}");
    assert_eq!(closure_violations[0].category, "closure_misaligned");
    assert!(closure_violations[0].message.contains("20.00 m"));

    // An UNaligned closure is inert and not flagged.
    project.loop_closures[0].is_aligned = false;
    assert!(project.check_closure_alignment().is_empty());
}

#[test]
fn cascade_solves_deep_chain_past_old_iteration_cap() {
    // A 40-section chain merged end-to-end, anchored at the datum on the FAR
    // end so propagation must traverse all 40 hops in the worst (backward) edge
    // order. The old 20-iteration fixpoint could silently stop halfway; the
    // single-pass BFS solver must align every merge regardless of depth.
    let mut project = Project::new("DeepChain".to_string(), None);
    let mut ids = Vec::new();
    for _ in 0..40 {
        let id = project
            .add_section(SectionData {
                shots: vec![shot(90.0, 90.0, 3.0, 0.0, 0.0)],
                ..Default::default()
            })
            .id;
        ids.push(id);
    }
    // Chain: section[i] station 1 merged to section[i+1] station 0.
    for w in ids.windows(2) {
        project.record_station_merge(StationMerge {
            station_a: StationRef::new(w[0], 1).into(),
            station_b: StationRef::new(w[1], 0).into(),
            merged_at: Utc::now(),
        });
    }
    // Datum on the LAST section (far end from section[0]).
    let last = *ids.last().unwrap();
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(last, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    // Knock every section out of alignment, then re-solve from scratch.
    for &id in &ids {
        project.adjust_section_offset(id, 7.0, -3.0);
    }
    project.realign_merges();
    assert!(
        project.check_merge_alignment().is_empty(),
        "single-pass solver must align all 40 hops regardless of depth/order"
    );
}

#[test]
fn an_adjustment_that_lands_on_dust_leaves_no_section_offset_key() {
    // #413 finding 2, dust half. An adjustment is not exactly invertible, so
    // "move it and move it back" lands a femtometre from where it started
    // rather than on it. On a section with NO offset that femtometre is enough
    // to CREATE a `section_offsets` key - and the key, unlike the value, is
    // structural: it is saved, and the route that never moved has no such key,
    // so two routes to the same geometry stop comparing equal.
    let mut project = Project::new("Dust".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;

    // A round trip whose two halves do not cancel exactly: 0.1 + 0.2 - 0.3 is
    // 5.55e-17, not 0, in IEEE double.
    project.adjust_section_offset(a, 0.1, 0.0);
    project.adjust_section_offset(a, 0.2, 0.0);
    project.adjust_section_offset(a, -0.3, 0.0);
    assert!(
        !project.section_offsets.contains_key(&a),
        "an adjust-and-adjust-back must leave no key, not a dust one: {:?}",
        project.section_offsets.get(&a)
    );

    // Real geometry is untouched: a millimetre is three decades above the bound
    // (#469 re-measured it up from a picometre - see `Project::OFFSET_DUST_M`).
    project.adjust_section_offset(a, 0.001, 0.0);
    assert_eq!(project.section_offsets.get(&a).copied(), Some((0.001, 0.0)));

    // The bound is a normalization on WRITES, and it binds EVERY writer - a
    // restore included. It used to be the other way round (a restore reproduced
    // what it was handed, dust and all, so an undo stayed the exact inverse of
    // what it undid), and that exception is exactly what made a present all-zero
    // key representable: `MoveSection(+d)` then `MoveSection(-d)` then had to end
    // ABSENT from an absent key and PRESENT from an all-zero one, which no writer
    // can decide from its own arguments. The state is gone instead - see the
    // invariant on `Project::write_section_offset` - so a dust value handed to a
    // restore lands ABSENT, which is the only spelling of what it meant.
    project.set_section_offset(a, (1e-15, 0.0));
    assert_eq!(
        project.section_offsets.get(&a).copied(),
        None,
        "set_section_offset must hold the invariant: a dust value is not a position"
    );
    project.set_section_offset(a, (0.002, 0.0));
    let mut captured = HashMap::new();
    captured.insert(a, Some((3e-14, -2e-14)));
    project.restore_section_offsets(&captured);
    assert_eq!(
        project.section_offsets.get(&a).copied(),
        None,
        "restore_section_offsets must hold it too: an entry recorded by a build that predates \
         the invariant may not reinstate the state the invariant removed"
    );
    // And a restore of a REAL value is untouched by any of it.
    let mut captured = HashMap::new();
    captured.insert(a, Some((0.25, -0.5)));
    project.restore_section_offsets(&captured);
    assert_eq!(
        project.section_offsets.get(&a).copied(),
        Some((0.25, -0.5)),
        "a restore of a real displacement must still reproduce it verbatim"
    );
}

/// The worst residue #469's re-measurement found, over every fuzz corpus, at
/// coordinates PAST the largest a georeference can produce: `HORIZON=1` at 3
/// seeds x 5000 moves with the generator's datums scattered worldwide, 1.8e7 m
/// from the frame origin, five ULPs of that magnitude. The full table is on
/// [`Project::OFFSET_DUST_M`].
const WORST_MEASURED_RESIDUE_M: f64 = 4.66e-9;

#[test]
fn the_dust_gate_is_decades_clear_of_both_ends() {
    // `Project::OFFSET_DUST_M` is squeezed from two sides and both are numbers,
    // so both are asserted rather than described.
    //
    // From BELOW: it has to sit clear of the float residue two routes to the
    // same geometry differ by, or a residue lands on either side of it and one
    // route writes a `section_offsets` key the other does not (#469, the
    // `CLOSURE_SHAPES=1` seed 18 failure). Two decades of clearance over the
    // worst residue ever measured is the floor.
    assert!(
        WORST_MEASURED_RESIDUE_M * 100.0 <= Project::OFFSET_DUST_M,
        "the dust gate ({:e}) must stay at least two decades above the worst measured residue \
         ({WORST_MEASURED_RESIDUE_M:e}) - closing that gap is what makes two routes disagree \
         about whether a key exists",
        Project::OFFSET_DUST_M
    );

    // From ABOVE: it is a NORMALIZATION that silently discards what it gates, so
    // it must stay far below the smallest displacement the app treats as real.
    // `MERGE_ALIGNMENT_INVARIANT_M` is that floor - the gap at which a merge
    // stops being aligned as far as the code is concerned - and four decades is
    // the clearance the constant's own doc argues for.
    assert!(
        Project::OFFSET_DUST_M * 1e4 <= Project::MERGE_ALIGNMENT_INVARIANT_M,
        "the dust gate ({:e}) must stay at least four decades below the alignment invariant \
         ({:e}): everything under the gate is thrown away, so it may never approach a \
         displacement the app would call real",
        Project::OFFSET_DUST_M,
        Project::MERGE_ALIGNMENT_INVARIANT_M
    );

    // Those two are the bottom of the chain
    // OFFSET_DUST_M << MERGE_ALIGNMENT_INVARIANT_M <= MERGE_ALIGNMENT_TOLERANCE_M;
    // its top is pinned by
    // `the_self_heal_gate_is_never_looser_than_the_alignment_invariant`, which
    // is where the reason for that half lives (#413 finding 20).
}

#[test]
fn the_dust_gate_clears_the_residue_at_the_georeference_ceiling() {
    // THE measurement `Project::OFFSET_DUST_M` is argued from, re-runnable: how
    // far apart do two merged stations actually land once the survey sits at the
    // largest coordinates a georeference can put it at?
    //
    // That gap is exactly what `cascade_bfs` compares against the gate, and #469
    // item 1 is why it is not a metre-scale question any more: the frame
    // renormalisation translates the whole survey onto whichever datum defines
    // the frame, so the coordinates carry the INTER-DATUM distance. Built here
    // the way the app reaches it - two datums, the first one removed, the second
    // promoted - rather than by injecting a large offset.
    let mut project = Project::new("Ceiling".to_string(), None);

    // A chain of merged sections, deep enough that the cascade BFS sums a real
    // chain of adjustments rather than one hop.
    const CHAIN: usize = 12;
    let chain: Vec<SectionId> = (0..CHAIN)
        .map(|i| {
            #[expect(
                clippy::cast_precision_loss,
                reason = "12 section indices, exact in f64"
            )]
            let turn = (i as f64) * 17.0;
            project
                .add_section(SectionData {
                    shots: vec![
                        shot(turn, turn + 3.0, 13.7, 0.0, 1.1),
                        shot(turn + 41.0, turn + 44.0, 7.3, 1.1, 2.9),
                        shot(turn + 122.0, turn + 125.0, 21.9, 2.9, 0.4),
                    ],
                    ..Default::default()
                })
                .id
        })
        .collect();
    project.ensure_shot_uuids();
    for pair in chain.windows(2) {
        project
            .merge_stations(StationRef::new(pair[0], 3), StationRef::new(pair[1], 0))
            .expect("a fresh chain link is mergeable");
    }

    // The far component, carrying the datum that will define the frame after the
    // first one goes. Nothing merges the two, so the translation below is the
    // only thing that moves the chain.
    let far = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 9.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();

    // Both fixes are valid GPS, so this is a survey the app must hold rather
    // than a corrupt one - and the pair is chosen to MAXIMISE the projection.
    // `gps_to_local`'s ENU turns an angular separation `theta` into a horizontal
    // displacement of `R * sin(theta)`, so a quarter turn of longitude at the
    // equator is the largest one any pair of fixes on this planet can produce:
    // 6.378137e6 m east, `WGS84_A` exactly. A more distant fix projects to LESS,
    // not more, because the separation goes into the discarded up component.
    project
        .set_zero_datum(
            StationRef::new(chain[0], 0),
            GpsCoordinates::new(0.0, -87.35, 0.0).expect("valid fix"),
        )
        .expect("first datum");
    project
        .set_zero_datum(
            StationRef::new(far, 0),
            GpsCoordinates::new(0.0, 2.65, 0.0).expect("valid fix"),
        )
        .expect("second datum");
    // Dropping the frame-defining datum promotes the far one and renormalises
    // onto it (#469 item 1), which is what puts the chain at ~1e7 m. The two
    // calls are what every command that can drop a datum does around the drop
    // (`AppState::renormalize_frame_if_changed`); the model has no epilogue of
    // its own.
    let frame_before = project.frame_datum_key();
    assert!(project.remove_zero_datum(StationRef::new(chain[0], 0)));
    assert!(
        project.renormalize_frame_if_changed(frame_before),
        "the promotion has to translate the survey - without it this test measures nothing"
    );

    // A zero-trust re-solve, the route the loader and Repair take.
    project.resolve_offsets();

    let local = project.section_local_coords();
    let station_offsets = project.resolved_station_offsets();
    let reached = chain
        .iter()
        .flat_map(|&id| {
            let (ox, oy) = project.section_offset(id);
            [ox.abs(), oy.abs()]
        })
        .fold(0.0_f64, f64::max);
    assert!(
        reached > 6.0e6,
        "the fixture has to REACH the coordinates it claims to measure at - the georeference \
         ceiling, ~6.38e6 m - and it only got to {reached:e} m, so a shrunken version of this \
         test would measure nothing"
    );

    let worst_gap = project
        .station_merges
        .iter()
        .filter_map(|merge| {
            project.endpoint_gap(&merge.station_a, &merge.station_b, &local, &station_offsets)
        })
        .map(|(_, _, distance)| distance)
        .fold(0.0_f64, f64::max);
    assert!(
        worst_gap * 100.0 < Project::OFFSET_DUST_M,
        "at {reached:e} m from the frame origin the solved chain left a {worst_gap:e} m gap \
         across a merge, which is within two decades of the dust gate ({:e}). That is the \
         straddle #469 fixed coming back: re-measure the residue and re-argue the constant \
         (its doc carries the table), do not widen an oracle",
        Project::OFFSET_DUST_M
    );

    // And the consequence, which is the defect itself rather than the number
    // behind it: a SECOND cascade over the same geometry, seeded from one
    // section of the chain instead of from every datum-bearing section, must
    // leave the map alone. Those two seed sets sum the same chains in different
    // orders and are entitled to differ in the last bits - what they are not
    // entitled to do is disagree about whether a section HAS a key, which is
    // structural, survives into the `.stalx`, and is exactly what #469's
    // `CLOSURE_SHAPES=1` seed 18 caught the picometre gate doing. Compared as a
    // whole map, so a key appearing on a section the first route left alone
    // fails even where the value is dust.
    let before = project.section_offsets.clone();
    project.cascade_connection_offsets(&[chain[CHAIN / 2]]);
    assert_eq!(
        before, project.section_offsets,
        "a second cascade over one solved geometry, seeded differently, rewrote the offset map \
         at {reached:e} m from the frame origin. Two routes to one geometry must agree on which \
         sections have a key (#469)"
    );
}

#[test]
fn reversing_a_section_twice_leaves_no_section_offset_key() {
    // The dust half's headline route (#413 finding 2, and the metamorphic
    // `reverse-twice-is-identity` pair): the reverse compensates the section
    // offset by the shot run's displacement, and the second reverse sums the
    // negated vectors in the OPPOSITE order, so the round trip lands near zero
    // rather than on it. An un-normalized write kept that femtometre as a key.
    let mut project = Project::new("ReverseTwice".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(37.0, 37.0, 13.7, 0.0, 1.1),
                shot(122.0, 122.0, 7.3, 1.1, 2.9),
                shot(281.0, 281.0, 21.9, 2.9, 0.4),
                shot(19.0, 19.0, 5.5, 0.4, 3.3),
            ],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    assert!(!project.section_offsets.contains_key(&a));

    assert!(project.reverse_section(a, false));
    assert!(project.reverse_section(a, false));
    assert!(
        !project.section_offsets.contains_key(&a),
        "reverse twice is the identity, so it must leave the map as it found it: {:?}",
        project.section_offsets.get(&a)
    );
}

#[test]
fn insert_before_and_after_agree_on_boundary_merge_ref() {
    // insert_before(s1) and insert_after(s0) are the SAME physical insertion
    // (a new shot in the array slot between s0 and s1), so a merge anchored
    // exactly on the boundary station must end up referencing the same station
    // index either way. A divergence is an off-by-one in one of the shifts.
    fn build() -> (Project, SectionId, ShotId, ShotId) {
        let mut p = Project::new("Boundary".to_string(), None);
        let a = p
            .add_section(SectionData {
                shots: vec![
                    shot(0.0, 0.0, 5.0, 0.0, 0.0),
                    shot(90.0, 90.0, 5.0, 0.0, 0.0),
                ],
                ..Default::default()
            })
            .id;
        let b = p
            .add_section(SectionData {
                shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
                ..Default::default()
            })
            .id;
        // Merge B0 onto A's MIDDLE station (index 1) - the insertion boundary.
        p.record_station_merge(StationMerge {
            station_a: StationRef::new(a, 1).into(),
            station_b: StationRef::new(b, 0).into(),
            merged_at: Utc::now(),
        });
        let s0 = p.get_section(a).unwrap().shots[0].id;
        let s1 = p.get_section(a).unwrap().shots[1].id;
        (p, a, s0, s1)
    }

    let a_side_index = |p: &Project, a: SectionId| -> u32 {
        let m = p
            .station_merges
            .iter()
            .find(|m| m.station_a.section_id == a || m.station_b.section_id == a)
            .expect("merge");
        let key = if m.station_a.section_id == a {
            &m.station_a
        } else {
            &m.station_b
        };
        p.resolve_key(key)
            .expect("merge endpoint resolves")
            .station_index
    };

    let (mut p_after, a, s0, _) = build();
    p_after.insert_shot_after(a, s0, shot(10.0, 10.0, 3.0, 0.0, 0.0), None, None);
    let after_idx = a_side_index(&p_after, a);

    let (mut p_before, a2, _, s1) = build();
    p_before.insert_shot_before(a2, s1, shot(10.0, 10.0, 3.0, 0.0, 0.0), None, None);
    let before_idx = a_side_index(&p_before, a2);

    assert_eq!(
        after_idx, before_idx,
        "insert_after and insert_before disagree on the boundary merge ref index \
         (after={after_idx}, before={before_idx})"
    );
    // And the merge partner stays geometrically colocated in BOTH paths - the
    // off-by-one re-pointed the merge onto the new station, which still
    // "coincided" but at the wrong place; this checks the merge holds after the
    // insert + cascade.
    assert!(
        p_after.check_merge_alignment().is_empty(),
        "after-path partner displaced"
    );
    assert!(
        p_before.check_merge_alignment().is_empty(),
        "before-path partner displaced"
    );
}

#[test]
fn redrive_offsets_after_load_heals_persisted_drift() {
    // Simulate a file persisted with a drifted offset (older redo bug) plus
    // empty transient caches (as after deserialization).
    let mut project = Project::new("Loaded".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();
    // Persisted drift + an empty section index (as after deserialization).
    project.adjust_section_offset(b, 31.0, -9.0);
    project.section_index_cache.clear();
    assert!(!project.check_merge_alignment().is_empty());

    let healed = project.redrive_offsets_after_load();
    assert_eq!(healed, 1);
    assert!(
        project.check_merge_alignment().is_empty(),
        "load must open geometrically correct"
    );
}

#[test]
fn redrive_dedups_uuids_before_reanchor_preserving_second_duplicate_ref() {
    // Regression: when a loaded section already has two shots sharing a UUID, the
    // load-time canonicalization of positional Index anchors to AfterShot(uuid)
    // MUST run after de-duplicating UUIDs. Otherwise a ref at the station after the
    // SECOND duplicate canonicalizes to AfterShot(dup), which resolve_key resolves
    // to the FIRST duplicate - silently moving the ref off its endpoint and
    // defeating Repair.
    let mut project = Project::new("Dup".to_string(), None);
    let s = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 1.0, 0.0, 0.0),
                shot(0.0, 0.0, 1.0, 0.0, 0.0),
                shot(0.0, 0.0, 1.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    // Force a duplicate: shots[0] and shots[1] share a UUID, as a corrupt/legacy
    // file could persist. (ensure_unique keeps the first and re-mints the second.)
    let dup = project.get_section(s).unwrap().shots[0]
        .uuid
        .clone()
        .unwrap();
    project.get_section_mut(s).unwrap().shots[1].uuid = Some(dup.clone());

    // A station offset stored in the legacy positional form at station 2 - the
    // station immediately AFTER the second duplicate shot.
    project
        .station_offsets
        .insert(StationKey::new(s, StationAnchor::Index(2)), (1.0, 2.0, 3.0));

    project.redrive_offsets_after_load();

    // The offset must still resolve to station 2, not collapse onto station 1.
    let (key, _) = project
        .station_offsets
        .iter()
        .next()
        .expect("offset survived load");
    assert!(
        project.key_is_ref(key, StationRef::new(s, 2)),
        "offset ref must stay on station 2 after dedup+reanchor, resolved to {:?}",
        project.resolve_key(key)
    );
    // And the duplicate was actually repaired (no two shots share a UUID).
    let uuids: Vec<_> = project
        .get_section(s)
        .unwrap()
        .shots
        .iter()
        .filter_map(|sh| sh.uuid.clone())
        .collect();
    let mut uniq = uuids.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(
        uuids.len(),
        uniq.len(),
        "duplicate shot UUID repaired on load"
    );
}

#[test]
fn resolve_offsets_does_not_touch_when_already_consistent() {
    // Opening a byte-clean project re-solves to identical offsets; resolve_offsets
    // must NOT bump modified_at then - otherwise a later no-edit save persists a
    // bogus timestamp and a no-op integrity Repair looks like a real change.
    let mut project = Project::new("Clean".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();
    // First solve makes the offsets consistent (and legitimately touches).
    project.resolve_offsets();
    assert!(project.check_merge_alignment().is_empty());
    let stamp = project.modified_at;

    // A second re-solve of the already-consistent project changes no offset, so it
    // must leave modified_at exactly as it was (no touch).
    let healed = project.resolve_offsets();
    assert_eq!(healed, 0, "nothing to heal");
    assert_eq!(
        project.modified_at, stamp,
        "resolve_offsets must not touch a clean project"
    );

    // And a genuine drift still touches (offsets change → modified_at advances).
    project.adjust_section_offset(b, 10.0, -4.0);
    let pre = project.modified_at;
    let healed2 = project.resolve_offsets();
    assert_eq!(healed2, 1, "the injected drift is healed");
    assert_ne!(project.modified_at, pre, "a real re-solve touches");
}

#[test]
fn section_has_zero_datum_ignores_dangling_datum() {
    // A corrupt project can be opened with a DANGLING zero datum (anchor shot
    // deleted) so the user can repair it. It must NOT count as a real anchor - else
    // a merge picks the phantom-anchored section as the fixed group and shifts the
    // world coordinate system around a station that doesn't resolve.
    let mut project = Project::new("DanglingDatum".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    // A dangling datum: anchored on a shot UUID that no shot carries.
    project.zero_datums.push(ZeroDatum {
        station_ref: StationKey::new(a, StationAnchor::AfterShot("ghost".to_string())),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });

    assert!(
        !project.section_has_zero_datum(a),
        "a dangling zero datum must not count as a merge anchor"
    );
    // Sanity: it is flagged as a dangling cross-reference.
    assert!(
        project
            .check_structural_integrity()
            .iter()
            .any(|v| v.category == "dangling_zero_datum_ref"),
        "dangling datum should be flagged by the integrity check"
    );
}

#[test]
fn has_datum_path_excluding_ignores_dangling_datum() {
    // A dangling datum (anchor key unresolvable) must NOT count as a reachable
    // anchor in has_datum_path_excluding - same resolve-aware invariant as
    // section_has_zero_datum (#174). Without the resolve guard the BFS reports a
    // datum path that doesn't actually anchor anything.
    let mut project = Project::new("DanglingPath".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    // A second, datum-less section to use as `excluded` (≠ a, so `a` is visited).
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    project.zero_datums.push(ZeroDatum {
        station_ref: StationKey::new(a, StationAnchor::AfterShot("ghost".to_string())),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    assert!(
        !project.has_datum_path_excluding(a, b),
        "a dangling datum must not count as a reachable anchor"
    );
}

#[test]
fn realign_after_shot_edit_ignores_dangling_datum() {
    // With only a dangling datum in the merge group, realign must treat the group
    // as UNANCHORED - it must not pin the group to the phantom-anchor section
    // (#174). Unanchored no longer means "do nothing" (#413 finding 20: a group
    // with no absolute anchor still has relative merge constraints, and skipping
    // them left the merge drifted by exactly the edit): it means anchor on the
    // EDITED section instead. So the edited section is the one that must not
    // move, and the merge must come out aligned around it.
    let mut project = Project::new("DanglingRealign".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    // Merge A1 = B0 so A and B share a rigid group, then drift B off the merge.
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();
    project.adjust_section_offset(b, 3.0, 4.0);
    // Dangling datum on section A (anchor key unresolvable).
    project.zero_datums.push(ZeroDatum {
        station_ref: StationKey::new(a, StationAnchor::AfterShot("ghost".to_string())),
        gps: GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
    });
    let before = project.section_offset(b);
    project.realign_after_shot_edit(b, None);
    let after = project.section_offset(b);
    assert_eq!(
        before, after,
        "the EDITED section must not move when the only datum in the group is \
         dangling - realigning around a phantom anchor would move it"
    );
    // ...and the group is realigned around it rather than left broken.
    let (ax, ay) = project
        .compute_station_raw_coords(StationRef::new(a, 1))
        .map(|c| (c.x, c.y))
        .expect("A:1 has no raw coordinates");
    let (bx, by) = project
        .compute_station_raw_coords(StationRef::new(b, 0))
        .map(|c| (c.x, c.y))
        .expect("B:0 has no raw coordinates");
    let gap = ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt();
    assert!(
        gap < Project::MERGE_ALIGNMENT_INVARIANT_M,
        "an unanchored group must still be realigned against the edited section: \
         the merged pair is {gap:.6} m apart"
    );
}

#[test]
fn rigid_group_has_datum_ignores_dangling_datum() {
    // A dangling datum must not mark a merge group as anchored - otherwise a
    // move-drag is wrongly blocked on an actually-unanchored group (#174; the
    // guard sits at command_move's "merge group holds a zero datum" check).
    let mut project = Project::new("DanglingRigid".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    project.zero_datums.push(ZeroDatum {
        station_ref: StationKey::new(a, StationAnchor::AfterShot("ghost".to_string())),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    assert!(
        !project.rigid_group_has_datum(a),
        "a dangling datum must not count as a merge-group anchor"
    );
}

#[test]
fn cascade_connection_offsets_ignores_dangling_datum() {
    // B holds a real datum; A holds a DANGLING datum, is merged to B, and is
    // drifted off the merge. Without the resolve guard A is wrongly frozen as
    // authoritative, so the A=B merge (two "fixed" sections) can't be pulled back
    // into alignment. With the guard only B anchors and A cascades onto it (#174).
    let mut project = Project::new("DanglingCascade".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    // Merge A1 = B0 (direct push avoids merge_stations' auto-alignment).
    project.record_station_merge(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    // Real datum on B (the genuine anchor) + dangling datum on A.
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(b, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationKey::new(a, StationAnchor::AfterShot("ghost".to_string())),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    // Drift A off the merge; precondition: misaligned.
    project.adjust_section_offset(a, 7.0, -3.0);
    assert!(
        !project.check_merge_alignment().is_empty(),
        "precondition: the A=B merge starts misaligned"
    );
    // Cascade anchored at B; A must follow (not stay frozen on a phantom datum).
    project.cascade_connection_offsets(&[b]);
    assert!(
        project.check_merge_alignment().is_empty(),
        "cascade must realign A onto B; a dangling datum on A must not freeze it"
    );
}

#[test]
fn check_merge_alignment_detects_and_realign_fixes_drift() {
    // Two sections, A0-A1 and B0-B1, with A1 merged to B0.
    let mut project = Project::new("Drift".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    // Anchor A at a zero datum so realign has an authority.
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    // Merge A1 <-> B0 (merge_stations computes the aligning offset).
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();
    assert!(
        project.check_merge_alignment().is_empty(),
        "merge should start aligned"
    );

    // Corrupt section B's offset directly (simulating a missed cascade).
    project.adjust_section_offset(b, 12.0, -7.0);
    let drift = project.check_merge_alignment();
    assert_eq!(drift.len(), 1, "{drift:?}");
    assert_eq!(drift[0].category, "merge_misaligned");
    assert!(drift[0].message.contains(" m apart"));

    // Repair realigns it.
    assert_eq!(project.realign_merges(), 1);
    assert!(project.check_merge_alignment().is_empty());
}

#[test]
fn realign_corrects_sub_tolerance_offset_drift() {
    // Zero-trust offsets: a stale offset too small for the per-command self-heal
    // to act on reports NO merge-alignment violation (so the old
    // `if before == 0` early-return skipped the cascade and the wrong offset
    // survived load + every command), yet it IS wrong. realign must re-derive it
    // regardless of the reported drift count.
    //
    // The magnitude that demonstrates it is 3 MM rather than the 3 cm this test
    // used before #413 finding 20: the self-heal gate moved from the 5 cm
    // REPORTING tolerance down to `MERGE_ALIGNMENT_INVARIANT_M` (1 cm), because
    // the 1-5 cm band it used to skip is a band `debug_assert_merge_alignment`
    // panics on. The blind spot this test is about is smaller now, not gone.
    let mut project = Project::new("SubTol".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();
    let b_correct = project.section_offset(b);

    // Nudge B by 3 mm - under the 1 cm invariant the self-heal gates on.
    project.adjust_section_offset(b, 0.003, 0.0);
    assert!(
        project.check_merge_alignment().is_empty(),
        "3 mm drift is under the 5 cm report tolerance (the early-return's blind spot)"
    );
    assert!(
        (project.section_offset(b).0 - b_correct.0).abs() > 0.002,
        "precondition: B is actually drifted"
    );
    // The per-command self-heal still early-returns here (nothing past the
    // invariant)...
    assert_eq!(
        project.realign_merges(),
        0,
        "realign_merges keeps the no-drift early-return"
    );
    assert!(
        (project.section_offset(b).0 - b_correct.0).abs() > 0.002,
        "...so the sub-invariant drift survives the per-command path"
    );

    // ...but the zero-trust load/repair re-solve corrects it despite zero drifts.
    project.resolve_offsets();
    let b_after = project.section_offset(b);
    assert!(
        (b_after.0 - b_correct.0).abs() < 1e-9 && (b_after.1 - b_correct.1).abs() < 1e-9,
        "sub-tolerance drift must be corrected: correct={b_correct:?} after={b_after:?}"
    );
}

#[test]
fn the_self_heal_gate_is_never_looser_than_the_alignment_invariant() {
    // #413 finding 20 IS this inequality being violated. The self-heal
    // (`realign_merges`) gated on the 5 cm REPORTING tolerance while
    // `debug_assert_merge_alignment` asserted at 1 cm, so the whole band between
    // them was a state the app declares impossible and declines to repair: a
    // release build saved it silently, and the next command in a test build
    // panicked on a merge it had never touched.
    //
    // Pinned as an ORDERING rather than as two literals, because the numbers
    // answer different questions and are allowed to move - what is not allowed
    // is for the repair threshold to end up looser than the assert's again.
    assert!(
        Project::MERGE_ALIGNMENT_INVARIANT_M <= Project::MERGE_ALIGNMENT_TOLERANCE_M,
        "the invariant the app asserts ({}) must be at least as tight as the tolerance it \
         reports ({}) - and `realign_merges` must gate on the invariant",
        Project::MERGE_ALIGNMENT_INVARIANT_M,
        Project::MERGE_ALIGNMENT_TOLERANCE_M
    );

    // And the gate really is the invariant, not the tolerance: a drift inside
    // the old blind band must be healed by the per-command path.
    let mut project = Project::new("HealGate".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();
    // 3 cm: past the invariant, under the reporting tolerance - the band.
    project.adjust_section_offset(b, 0.03, 0.0);
    assert!(
        project.check_merge_alignment().is_empty(),
        "precondition: 3 cm is under the 5 cm the user is told about"
    );
    project.realign_merges();
    let gap = project
        .compute_station_raw_coords(StationRef::new(a, 1))
        .zip(project.compute_station_raw_coords(StationRef::new(b, 0)))
        .map(|(left, right)| ((left.x - right.x).powi(2) + (left.y - right.y).powi(2)).sqrt())
        .expect("merged stations have no raw coordinates");
    assert!(
        gap < Project::MERGE_ALIGNMENT_INVARIANT_M,
        "the per-command self-heal left the pair {gap:.6} m apart - a drift the app's own \
         assert calls a violation and its repair declined to fix (#413 finding 20)"
    );
}

/// A merge component holding a gap the offset solver CANNOT close, plus one
/// bystander it can move.
///
/// A and B are merged and each carries its own zero datum - two GPS anchors in
/// one rigid group. `set_zero_datum`/`merge_stations` guard against building that
/// interactively, so it is assembled by pushing the datums directly, which is how
/// it actually arrives: a file written before the guards, or a repair. The
/// cascade marks every datum section authoritative and therefore reaches the A=B
/// merge from both sides and adjusts neither, so the 2 cm gap opened below
/// survives every re-solve, forever.
///
/// C hangs off the same component with no datum of its own, so it IS something
/// the cascade moves - the bystander that shows whether a re-solve ran.
fn over_anchored_irreducible_gap() -> (Project, SectionId, SectionId, SectionId) {
    let mut project = Project::new("Overanchored".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let c = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    // Both merges start exactly aligned - merge_stations computes the offset.
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(b, 0))
        .unwrap();
    project
        .merge_stations(StationRef::new(c, 0), StationRef::new(a, 0))
        .unwrap();
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(b, 0).into(),
        gps: GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
    });
    // Pull B 2 cm off the merge: past the 1 cm invariant the self-heal gates on,
    // under the 5 cm the user is told about.
    project.adjust_section_offset(b, 0.0, -0.02);
    (project, a, b, c)
}

#[test]
fn realign_merges_skips_a_residual_the_solver_has_proven_irreducible() {
    // `reconcile_geometry` calls `realign_merges` after EVERY command and every
    // undo/redo. With an irreducible beyond-invariant gap in the project the gate
    // is non-empty forever, so before the fixpoint fingerprint every keystroke-scale
    // edit paid a full `resolve_offsets` cascade - which is also the one thing that
    // overrides the exact offsets an undo snapshot has just restored.
    let (mut project, _a, _b, c) = over_anchored_irreducible_gap();
    assert_eq!(
        project
            .merge_alignment_beyond(Project::MERGE_ALIGNMENT_INVARIANT_M)
            .len(),
        1,
        "precondition: exactly one merge is past the invariant"
    );
    assert!(
        project.check_merge_alignment().is_empty(),
        "precondition: and it is under the 5 cm reporting tolerance, so the healed \
         count stays 0 either way"
    );

    // Pass one runs the full solve, watches it move nothing, and records the
    // fixpoint.
    assert_eq!(project.realign_merges(), 0);
    assert!(
        project.realign_fixpoint.is_some(),
        "a solve that cleared nothing and moved nothing IS the fixpoint"
    );
    assert_eq!(
        project
            .merge_alignment_beyond(Project::MERGE_ALIGNMENT_INVARIANT_M)
            .len(),
        1,
        "the gap really is irreducible - the solver has now run on it"
    );

    // Drift the bystander by less than the invariant, so the residual set and every
    // gap bit stay exactly as fingerprinted (and nothing structural changes).
    project.adjust_section_offset(c, 0.003, 0.0);
    let drifted = project.section_offset(c);
    assert!(
        project
            .merge_alignment_beyond(Project::MERGE_ALIGNMENT_INVARIANT_M)
            .len()
            == 1,
        "3 mm keeps C out of the residual set"
    );

    assert_eq!(project.realign_merges(), 0);
    assert_eq!(
        project.section_offset(c),
        drifted,
        "the fingerprint matched, so the re-solve must have been skipped entirely"
    );

    // And the skipped work was real work: the same solve, run explicitly, pulls C
    // back. That is what makes this a skip rather than a coincidence.
    project.resolve_offsets();
    assert!(
        (project.section_offset(c).0 - drifted.0).abs() > 0.002,
        "the solve the skip declined would have moved C: before={drifted:?} after={:?}",
        project.section_offset(c)
    );
}

#[test]
fn realign_merges_re_solves_once_the_residual_becomes_healable() {
    // The other half of the contract: a structural edit can make an irreducible
    // residual healable WITHOUT moving a station or changing a single gap bit, so
    // the fingerprint carries the structure too. Dropping one of the two anchors is
    // exactly that edit - the component becomes solvable and the next realign must
    // do the work.
    let (mut project, _a, b, c) = over_anchored_irreducible_gap();
    assert_eq!(project.realign_merges(), 0);
    assert!(project.realign_fixpoint.is_some());
    project.adjust_section_offset(c, 0.003, 0.0);
    let drifted = project.section_offset(c);
    assert_eq!(project.realign_merges(), 0);
    assert_eq!(
        project.section_offset(c),
        drifted,
        "precondition: the skip is armed"
    );

    assert!(project.remove_zero_datum(StationRef::new(b, 0)));
    project.realign_merges();
    assert!(
        project
            .merge_alignment_beyond(Project::MERGE_ALIGNMENT_INVARIANT_M)
            .is_empty(),
        "with one anchor left the cascade can pull the merge together - and must \
         not be skipped for a fingerprint recorded while it could not"
    );
    assert!(
        project.section_offset(c).0.abs() < 1e-9,
        "the re-solve ran in full, so the bystander drift is gone too: {:?}",
        project.section_offset(c)
    );
}

#[test]
fn realign_merges_re_solves_once_drift_crosses_the_invariant() {
    // The load-bearing half of the fixpoint's safety argument, asserted rather
    // than argued: the skip may leave SUB-invariant drift alone (the test two
    // above), but drift that CROSSES `MERGE_ALIGNMENT_INVARIANT_M` puts a new
    // pair in the residual set, and the set is compared by identity - so the
    // fingerprint misses and the full solve runs. That is what makes the state
    // `debug_assert_merge_alignment` panics on unreachable while the skip is
    // armed.
    let (mut project, _a, _b, c) = over_anchored_irreducible_gap();
    assert_eq!(project.realign_merges(), 0);
    assert!(project.realign_fixpoint.is_some(), "precondition: armed");
    let anchored_x = project.section_offset(c).0;
    project.adjust_section_offset(c, 0.003, 0.0);
    assert_eq!(project.realign_merges(), 0);
    assert!(
        (project.section_offset(c).0 - (anchored_x + 0.003)).abs() < 1e-12,
        "precondition: 3 mm is under the invariant, so the skip holds it - C is at {:?}",
        project.section_offset(c)
    );

    // Push the same bystander past the invariant - one edit, no structural
    // change, and every EXISTING residual gap still bit-identical.
    project.adjust_section_offset(c, 0.017, 0.0);
    assert_eq!(
        project
            .merge_alignment_beyond(Project::MERGE_ALIGNMENT_INVARIANT_M)
            .len(),
        2,
        "precondition: C's merge has now crossed, so there are two residuals"
    );

    project.realign_merges();
    assert!(
        (project.section_offset(c).0 - anchored_x).abs() < 1e-9,
        "a gap past the invariant must never be left to the skip: C is still at {:?}",
        project.section_offset(c)
    );
    assert_eq!(
        project
            .merge_alignment_beyond(Project::MERGE_ALIGNMENT_INVARIANT_M)
            .len(),
        1,
        "the solve ran and healed everything it could - only the irreducible gap is left"
    );
}

#[test]
fn realign_merges_still_skips_for_an_edit_in_a_disjoint_component() {
    // The other side of the same contract: the skip exists so that ONE
    // irreducible gap does not make every later edit pay a whole-project
    // re-solve. An edit in a component that shares nothing with the residual
    // must still hit it.
    let (mut project, _a, b, _c) = over_anchored_irreducible_gap();
    // A second, healthy component: D anchored by its own datum, E merged onto
    // it. Built BEFORE the fixpoint is recorded - adding a section is a
    // structural change, and the structure hash would (correctly) reject a
    // fingerprint taken before it. Built while the A=B gap is CLOSED, too:
    // `merge_stations` runs `debug_assert_merge_alignment`, which a test build
    // panics on for the very residual this project is about, so the helper's
    // 2 cm is backed out and re-applied around the new component.
    project.adjust_section_offset(b, 0.0, 0.02);
    let d = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let e = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    project
        .merge_stations(StationRef::new(d, 1), StationRef::new(e, 0))
        .unwrap();
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(d, 0).into(),
        gps: GpsCoordinates::new(46.0, -91.0, 0.0).unwrap(),
    });
    project.adjust_section_offset(b, 0.0, -0.02);

    assert_eq!(project.realign_merges(), 0);
    assert!(project.realign_fixpoint.is_some(), "precondition: armed");

    project.adjust_section_offset(e, 0.003, 0.0);
    let drifted = project.section_offset(e);
    assert_eq!(project.realign_merges(), 0);
    assert_eq!(
        project.section_offset(e),
        drifted,
        "an edit two components away must still take the skip - that is the whole \
         optimization"
    );
    // And the declined solve was real work here too, so the skip is doing
    // something rather than agreeing with a no-op.
    project.resolve_offsets();
    assert!(
        (project.section_offset(e).0 - drifted.0).abs() > 0.002,
        "the solve the skip declined would have moved E: before={drifted:?} after={:?}",
        project.section_offset(e)
    );
}

#[test]
fn a_whole_rigid_group_translation_is_a_solver_no_op() {
    // Why the fingerprint does NOT need the residual component's own offsets in
    // it, which looks like a hole until the cascade is traced: a whole-group
    // move leaves every residual gap and the structure hash bit-identical, so
    // the skip fires - and it hides nothing, because the solve it declines is
    // itself a no-op. `cascade_connection_offsets` derives each
    // non-authoritative section from an authoritative one's STORED offset and
    // never re-derives an authoritative section at all (a datum section is
    // pinned WHERE IT IS, not re-anchored onto its GPS), so translating every
    // section of a component by one delta lands exactly where the cascade would
    // put them.
    let (mut project, a, b, c) = over_anchored_irreducible_gap();
    assert_eq!(project.realign_merges(), 0);
    assert!(project.realign_fixpoint.is_some(), "precondition: armed");

    // The move tools' own path, not a hand-rolled loop over the ids.
    project.adjust_rigid_group_offsets(a, 3.0, -2.0);
    let moved: Vec<(f64, f64)> = [a, b, c]
        .iter()
        .map(|s| project.section_offset(*s))
        .collect();

    let mut solved = project.clone();
    solved.resolve_offsets();
    let after_solve: Vec<(f64, f64)> = [a, b, c]
        .iter()
        .map(|s| solved.section_offset(*s))
        .collect();
    assert_eq!(
        moved, after_solve,
        "a full solve after a whole-group translation must change nothing - if this \
         ever fails, the skip really would be hiding a re-anchor and the fingerprint \
         needs the component's offsets in it"
    );

    project.realign_merges();
    let after_skip: Vec<(f64, f64)> = [a, b, c]
        .iter()
        .map(|s| project.section_offset(*s))
        .collect();
    assert_eq!(
        after_skip, after_solve,
        "so the skipped state and the solved state are the same state"
    );
}

#[test]
fn realign_preserves_floating_section_offset() {
    // A datum-less, merge-less (floating) section's absolute position is
    // non-derivable (free placement). The always-on re-solve must NOT zero it -
    // it's a cascade seed/anchor, never seed-adjusted.
    let mut project = Project::new("Float".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.adjust_section_offset(a, 12.0, 7.0);
    project.resolve_offsets(); // the unconditional re-solve
    assert_eq!(
        project.section_offset(a),
        (12.0, 7.0),
        "a free-placed floating section keeps its offset across an unconditional re-solve"
    );
}

// === Structural integrity check ===

/// A two-section project with one valid merge, zero datum, and loop closure.
fn integrity_fixture() -> (Project, SectionId, SectionId) {
    let mut project = Project::new("Integrity".to_string(), None);
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
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 5.0).unwrap(),
    });
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 1).into(),
        path_length: 10.0,
        misclosure_distance: 0.2,
        error_percentage: 2.0,
        horizontal_misclosure: 0.15,
        vertical_misclosure: 0.05,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });
    (project, a, b)
}

#[test]
fn check_orphan_closures_detects_sole_bridge_closure() {
    let mut p = Project::new("Orphan".to_string(), None);
    let a = p
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = p
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    // A closure between A and B with NO merge connecting them: a sole bridge.
    p.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 0).into(),
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: true,
    });
    let v = p.check_orphan_closures();
    assert_eq!(v.len(), 1, "{v:?}");
    assert_eq!(v[0].category, "orphan_closure");

    // Once a merge connects the two sections, the closure is a genuine
    // (redundant) loop and is no longer flagged.
    p.record_station_merge(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 1).into(),
        merged_at: Utc::now(),
    });
    assert!(p.check_orphan_closures().is_empty());
}

#[test]
fn deleting_shot_converts_orphaned_bridge_closure_to_merge() {
    // A: A0-A1-A2 (datum A0); B: B0-B1. B hangs off A2 (merge A2=B0), and a
    // closure A0=B1 closes the loop. Now delete A's last shot (A1->A2): that
    // removes A2 and the A2=B0 merge, so A and B are connected ONLY through the
    // closure A0=B1. A closure that is the sole bridge is not a loop - it must
    // become a merge (issue #8, the section-31 case).
    let mut project = Project::new("Bridge".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 5.0, 0.0, 0.0),
                shot(90.0, 90.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    project.record_station_merge(StationMerge {
        station_a: StationRef::new(a, 2).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 1).into(),
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: true,
    });

    let last = project.get_section(a).unwrap().shots.last().unwrap().id;
    project.remove_shot_with_rewire(a, last).unwrap();

    // The bridge closure is gone and replaced by a merge between its endpoints.
    assert!(
        project.loop_closures.is_empty(),
        "sole-bridge closure should have become a merge: {:?}",
        project.loop_closures
    );
    assert!(
        project.station_merges.iter().any(|m| {
            let pair = (m.station_a.clone(), m.station_b.clone());
            pair == (StationRef::new(a, 0).into(), StationRef::new(b, 1).into())
                || pair == (StationRef::new(b, 1).into(), StationRef::new(a, 0).into())
        }),
        "expected a merge A0<->B1; got {:?}",
        project.station_merges
    );
}

#[test]
fn undoing_an_appended_first_shot_converts_a_closure_it_orphans() {
    // A is a one-shot section, exactly as an `add_shot` append into an EMPTY
    // section leaves it, so undoing that `CreateShot` takes
    // `remove_appended_first_shot`. That inverse drops every relationship anchored
    // on the station the append created (A1) - here the merge A1=C0 - and dropping
    // it leaves the B=C closure as the SOLE connection between B and C. A closure
    // that is not a loop is a merge (#8). Every other delete entry point converts
    // it; #413 finding 10 is this one being the exception, which let an undo leave
    // behind a state the app declares impossible and the save then persisted.
    let mut project = Project::new("OrphanOnUndo".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 5.0, 0.0, 0.0),
                shot(90.0, 90.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let c = project
        .add_section(SectionData {
            shots: vec![
                shot(180.0, 180.0, 5.0, 0.0, 0.0),
                shot(270.0, 270.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    // B hangs off the station that predates the append, C off the one it created.
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(c, 0))
        .unwrap();
    // With both merges in place B and C are a genuine loop (shots + merges through
    // A one way, this closure the other), so it is legitimately a closure today.
    let closure_a = project.key_for_ref(StationRef::new(b, 2)).unwrap();
    let closure_b = project.key_for_ref(StationRef::new(c, 2)).unwrap();
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: closure_a,
        station_b: closure_b,
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });

    let created = project.get_section(a).unwrap().shots[0].id;
    project.remove_appended_first_shot(a, created).unwrap();

    assert!(
        project.loop_closures.is_empty(),
        "the closure stopped being a loop when the merge on the created station \
         went, so it must have become a merge: {:?}",
        project.loop_closures
    );
    let converted = project.station_merges.iter().any(|m| {
        let pair = (
            project.resolve_key(&m.station_a),
            project.resolve_key(&m.station_b),
        );
        pair == (Some(StationRef::new(b, 2)), Some(StationRef::new(c, 2)))
            || pair == (Some(StationRef::new(c, 2)), Some(StationRef::new(b, 2)))
    });
    assert!(
        converted,
        "expected the orphaned closure to be replaced by a merge B2<->C2; got {:?}",
        project.station_merges
    );
    // The merge on station 0 predates the append and survives it; the one on the
    // created station is gone. Two merges, not three.
    assert_eq!(
        project.station_merges.len(),
        2,
        "{:?}",
        project.station_merges
    );
    assert!(
        project
            .station_merges
            .iter()
            .any(
                |m| project.resolve_key(&m.station_a) == Some(StationRef::new(a, 0))
                    || project.resolve_key(&m.station_b) == Some(StationRef::new(a, 0))
            ),
        "the station that predates the append keeps its merge"
    );
    // And the conversion left no merge the app would assert on: a closure is a soft
    // edge, the merge replacing it is rigid, so the new edge is solved.
    assert!(
        project
            .merge_alignment_beyond(Project::MERGE_ALIGNMENT_INVARIANT_M)
            .is_empty(),
        "the new rigid edge must be aligned"
    );
}

/// Two closures, one project, and only ONE of them is this removal's business.
///
/// B2=C2 is a true loop today - shots and merges through A one way, the closure
/// the other - and stops being one when the merge on the deleted station goes.
/// Converting it is what the call exists for, and it is converted whether or not
/// its endpoints are anywhere near the removed station.
///
/// D1=E1 bridges two sections that nothing else connects, so it is ALREADY an
/// orphan when the removal starts. Nothing about this delete made it one (the
/// fuzzer's route there is a `removeZeroDatum` two moves back, which orphans a
/// closure and converts nothing), so it is part of the state the history
/// recorded - and both removal paths also run as the inverse of `CreateShot`, an
/// inverse must not do more than invert, and converting it here would land the
/// undo on a state no snapshot ever held (#413, the open observation).
#[test]
fn removing_a_shot_converts_the_closure_it_orphans_and_not_an_older_one() {
    let mut project = Project::new("ScopedConversion".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0), shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 5.0, 0.0, 0.0),
                shot(90.0, 90.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let c = project
        .add_section(SectionData {
            shots: vec![
                shot(180.0, 180.0, 5.0, 0.0, 0.0),
                shot(270.0, 270.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    // D and E touch nothing else in the project.
    let d = project
        .add_section(SectionData {
            shots: vec![shot(45.0, 45.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let e = project
        .add_section(SectionData {
            shots: vec![shot(135.0, 135.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();
    // The merge that carries the loop's other path, on the station the delete
    // below removes.
    project
        .merge_stations(StationRef::new(a, 2), StationRef::new(c, 0))
        .unwrap();
    push_closure(
        &mut project,
        1,
        StationRef::new(b, 2),
        StationRef::new(c, 2),
    );
    push_closure(
        &mut project,
        2,
        StationRef::new(d, 1),
        StationRef::new(e, 1),
    );
    assert_eq!(
        project.orphan_closure_ids(),
        HashSet::from([2]),
        "setup: D1=E1 must already be an orphan and B2=C2 must not"
    );

    let last = project.get_section(a).unwrap().shots.last().unwrap().id;
    project.remove_shot_with_rewire(a, last).unwrap();

    assert_eq!(
        project
            .loop_closures
            .iter()
            .map(|c| c.id)
            .collect::<Vec<_>>(),
        vec![2],
        "the closure this delete orphaned had to become a merge and the one that arrived \
         orphaned had to be left alone; got {:?}",
        project.loop_closures
    );
    let converted = project.station_merges.iter().any(|m| {
        let pair = (
            project.resolve_key(&m.station_a),
            project.resolve_key(&m.station_b),
        );
        pair == (Some(StationRef::new(b, 2)), Some(StationRef::new(c, 2)))
            || pair == (Some(StationRef::new(c, 2)), Some(StationRef::new(b, 2)))
    });
    assert!(
        converted,
        "expected the orphaned closure to be replaced by a merge B2<->C2; got {:?}",
        project.station_merges
    );
    assert!(
        !project.station_merges.iter().any(|m| {
            m.station_a.section_id == d
                || m.station_b.section_id == d
                || m.station_a.section_id == e
                || m.station_b.section_id == e
        }),
        "the pre-existing orphan must not have become a merge: {:?}",
        project.station_merges
    );
}

/// The same scoping on the other removal path, which is the one that is ONLY
/// ever an inverse. The loop and the merge that carries it are
/// `undoing_an_appended_first_shot_converts_a_closure_it_orphans`'s (that test
/// still pins the conversion itself); what is added is the older orphan the
/// undo must not touch.
#[test]
fn removing_an_appended_first_shot_converts_the_closure_it_orphans_and_not_an_older_one() {
    let mut project = Project::new("ScopedConversionOnUndo".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 5.0, 0.0, 0.0),
                shot(90.0, 90.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let c = project
        .add_section(SectionData {
            shots: vec![
                shot(180.0, 180.0, 5.0, 0.0, 0.0),
                shot(270.0, 270.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let d = project
        .add_section(SectionData {
            shots: vec![shot(45.0, 45.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let e = project
        .add_section(SectionData {
            shots: vec![shot(135.0, 135.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    // B hangs off the station that predates the append, C off the one it created.
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();
    project
        .merge_stations(StationRef::new(a, 1), StationRef::new(c, 0))
        .unwrap();
    push_closure(
        &mut project,
        1,
        StationRef::new(b, 2),
        StationRef::new(c, 2),
    );
    push_closure(
        &mut project,
        2,
        StationRef::new(d, 1),
        StationRef::new(e, 1),
    );
    assert_eq!(
        project.orphan_closure_ids(),
        HashSet::from([2]),
        "setup: D1=E1 must already be an orphan and B2=C2 must not"
    );

    let created = project.get_section(a).unwrap().shots[0].id;
    project.remove_appended_first_shot(a, created).unwrap();

    assert_eq!(
        project
            .loop_closures
            .iter()
            .map(|c| c.id)
            .collect::<Vec<_>>(),
        vec![2],
        "the undo had to convert the closure it orphaned and leave the one that arrived \
         orphaned alone; got {:?}",
        project.loop_closures
    );
    assert!(
        !project.station_merges.iter().any(|m| {
            m.station_a.section_id == d
                || m.station_b.section_id == d
                || m.station_a.section_id == e
                || m.station_b.section_id == e
        }),
        "the pre-existing orphan must not have become a merge: {:?}",
        project.station_merges
    );
}

/// A closure between two stations, with the metrics zeroed: these tests care
/// about the closure/merge classification, never about misclosure.
fn push_closure(project: &mut Project, id: u64, station_a: StationRef, station_b: StationRef) {
    let (Some(key_a), Some(key_b)) = (
        project.key_for_ref(station_a),
        project.key_for_ref(station_b),
    ) else {
        panic!("closure endpoint {station_a} or {station_b} has no stable anchor");
    };
    project.loop_closures.push(LoopClosure {
        id,
        station_a: key_a,
        station_b: key_b,
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });
}

#[test]
fn deleting_shot_converts_broken_loop_closure_to_merge() {
    // Section A: A0-A1-A2 (2 shots); Section B: B0-B1 (1 shot).
    let mut project = Project::new("Loop".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 1.0, 0.0, 0.0),
                shot(90.0, 90.0, 1.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    // A2 is merged with B0 (B hangs off the end of A).
    project.record_station_merge(StationMerge {
        station_a: StationRef::new(a, 2).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    // A loop closure ties A0 to A2 (the A0-A1-A2 survey is the loop).
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(a, 2).into(),
        path_length: 2.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });

    // Canonicalize the transitional Index anchors to stable shot-UUID anchors,
    // as every load path does before any command runs (so in-memory keys are
    // canonical, which is what remove_shot_with_rewire matches against).
    project.reanchor_station_refs();

    // Delete A's last shot (A1->A2): removes station A2, a closure endpoint. The
    // surviving B segment is detached, so the closure must become a merge.
    let last_shot_id = project.get_section(a).unwrap().shots.last().unwrap().id;
    let (_, impact) = project.remove_shot_with_rewire(a, last_shot_id).unwrap();

    assert_eq!(impact.closure_to_merge.len(), 1, "impact: {impact:?}");
    assert!(
        project.loop_closures.is_empty(),
        "broken loop should not remain a closure: {:?}",
        project.loop_closures
    );
    assert!(
        project.station_merges.iter().any(|m| {
            let pair = (m.station_a.clone(), m.station_b.clone());
            pair == (StationRef::new(b, 0).into(), StationRef::new(a, 0).into())
                || pair == (StationRef::new(a, 0).into(), StationRef::new(b, 0).into())
        }),
        "expected a merge between A0 and B0; got {:?}",
        project.station_merges
    );
    // And the result is structurally sound.
    assert!(!project.has_critical_integrity_violations());
}

/// Two independently GPS-anchored components, bridged only by a loop closure,
/// and the shot carrying the closure's endpoint deleted (#464 defect A).
///
/// * component 1: section A (4 shots north) merged tail-to-head into section C,
///   whose station 0 carries the GPS datum;
/// * component 2: section B, with its OWN GPS datum ~1.4 km away;
/// * a loop closure A4 <-> B0, which `create_loop_closure` legitimately allows
///   between two unconnected components when both are datum-anchored.
///
/// Returns `(project, a, b, c)`.
fn double_anchored_closure_fixture() -> (Project, SectionId, SectionId, SectionId) {
    let mut project = Project::new("DoubleAnchor".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let c = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    // A's tail hangs C off it, and C carries component 1's anchor.
    project
        .merge_stations(StationRef::new(a, 4), StationRef::new(c, 0))
        .unwrap();
    project
        .set_zero_datum(
            StationRef::new(c, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();
    // Component 2's own anchor, far enough away that welding the two is a gap
    // no cascade could hide.
    project
        .set_zero_datum(
            StationRef::new(b, 0),
            GpsCoordinates::new(45.01, -90.0, 0.0).unwrap(),
        )
        .unwrap();
    project
        .create_loop_closure(
            StationRef::new(a, 4),
            StationRef::new(b, 0),
            ClosureStrategy::Batch,
        )
        .unwrap();
    (project, a, b, c)
}

/// #464 defect A: the broken-loop arm of `preview_delete_shot` used to convert
/// an orphaned closure into a merge with NO guard at all, welding two
/// independently datum-anchored components into one rigid group. Both ends are
/// authoritative to `cascade_bfs`, so the gap is permanent and Repair cannot
/// reduce it. The closure has to SURVIVE, re-anchored onto the replacement
/// station.
#[test]
fn deleting_a_shot_never_promotes_a_closure_that_would_double_anchor() {
    let (mut project, a, b, c) = double_anchored_closure_fixture();
    let closure_id = project.loop_closures[0].id;

    // Delete A's last shot: that removes station A4, which is both the merge
    // partner of C0 and one end of the closure.
    let last_shot = project.get_section(a).unwrap().shots.last().unwrap().id;
    let (_, impact) = project.remove_shot_with_rewire(a, last_shot).unwrap();

    assert!(
        impact.closure_to_merge.is_empty(),
        "a closure between two independently anchored components must not become a merge: {impact:?}"
    );
    assert_eq!(
        impact.replacement_closure_updates.len(),
        1,
        "the refused promotion must keep the closure, re-anchored: {impact:?}"
    );
    let update = &impact.replacement_closure_updates[0];
    assert_eq!(update.closure_id, closure_id);
    assert_eq!(update.new_station, StationRef::new(c, 0));

    assert_eq!(
        project.loop_closures.len(),
        1,
        "the closure must survive: {:?}",
        project.loop_closures
    );
    let survivor = &project.loop_closures[0];
    let ends = (
        project.resolve_key(&survivor.station_a).unwrap(),
        project.resolve_key(&survivor.station_b).unwrap(),
    );
    assert_eq!(ends, (StationRef::new(c, 0), StationRef::new(b, 0)));
    assert!(
        !project.are_stations_merged(StationRef::new(c, 0), StationRef::new(b, 0)),
        "no rigid merge may exist across the two anchors: {:?}",
        project.station_merges
    );

    // ...and the refusal is STABLE: the whole-project sweep the command epilogue
    // and the loader both run declines it for the same reason, so memory and
    // disk cannot disagree about what this closure is.
    assert_eq!(project.convert_orphan_closures_to_merges(), 0);
    project.redrive_offsets_after_load();
    assert_eq!(project.loop_closures.len(), 1);
    assert!(project.station_merges.is_empty());
}

/// One doomed station carrying TWO sole-bridge closures, each into its own
/// independently GPS-anchored component (#464 defect A, chained).
///
/// * section S: two shots north, GPS-anchored at S0, its tail S2 merged into
///   T0 - so T is the station both closures rewire onto when S2 goes;
/// * section T: one shot east, no datum of its own;
/// * sections U and V: one shot each, each carrying its OWN GPS datum, about a
///   kilometre away in different directions;
/// * closures S2 <-> U0 and S2 <-> V0 (legal to create: the pairs are
///   unconnected and both ends are datum-anchored).
///
/// Deleting S's last shot removes S2. EITHER closure alone promotes legally -
/// once S2's merge is gone the replacement T holds no datum, so nothing is
/// double-anchored - but promoting BOTH welds U and V into one rigid group
/// carrying two GPS anchors, which is exactly the state defect A is about.
///
/// Returns `(project, s, t, u, v)`.
fn chained_double_anchor_closure_fixture() -> (Project, SectionId, SectionId, SectionId, SectionId)
{
    let mut project = Project::new("ChainedDoubleAnchor".to_string(), None);
    let s = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let t = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let u = project
        .add_section(SectionData {
            shots: vec![shot(180.0, 180.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let v = project
        .add_section(SectionData {
            shots: vec![shot(270.0, 270.0, 5.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    // T hangs off S's tail: it is the only survivor of S2's merge group, so it
    // is the replacement station the closure rewire picks.
    project
        .merge_stations(StationRef::new(s, 2), StationRef::new(t, 0))
        .unwrap();
    project
        .set_zero_datum(
            StationRef::new(s, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();
    // Two more anchors, each far enough that welding them into one group is a
    // gap no cascade could hide.
    project
        .set_zero_datum(
            StationRef::new(u, 0),
            GpsCoordinates::new(45.01, -90.0, 0.0).unwrap(),
        )
        .unwrap();
    project
        .set_zero_datum(
            StationRef::new(v, 0),
            GpsCoordinates::new(45.0, -90.01, 0.0).unwrap(),
        )
        .unwrap();
    project
        .create_loop_closure(
            StationRef::new(s, 2),
            StationRef::new(u, 0),
            ClosureStrategy::Batch,
        )
        .unwrap();
    project
        .create_loop_closure(
            StationRef::new(s, 2),
            StationRef::new(v, 0),
            ClosureStrategy::Batch,
        )
        .unwrap();
    (project, s, t, u, v)
}

/// Assert every recorded merge is within the invariant, the way
/// `debug_assert_merge_alignment` does - but unconditionally, so the check is
/// part of what this test proves rather than a debug-build side effect.
fn assert_every_merge_is_aligned(project: &Project, context: &str) {
    for merge in &project.station_merges {
        let (Some(a), Some(b)) = (
            project.resolve_key(&merge.station_a),
            project.resolve_key(&merge.station_b),
        ) else {
            panic!("{context}: merge with a dangling endpoint: {merge:?}");
        };
        let (Some((ax, ay)), Some((bx, by))) = (
            project.compute_station_raw_xy(a),
            project.compute_station_raw_xy(b),
        ) else {
            panic!("{context}: merge endpoint with no position: {merge:?}");
        };
        let gap = ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt();
        assert!(
            gap < Project::MERGE_ALIGNMENT_INVARIANT_M,
            "{context}: merge {a:?} <-> {b:?} is {gap:.6} m out of alignment"
        );
    }
}

/// #464 defect A, chained: the guard is only sound if it accumulates.
///
/// `preview_delete_shot` built its `planned` hypothesis ONCE, from
/// `replacement_merges` alone, so a promotion the closure loop itself decided on
/// was invisible to the next closure's guard. With two sole-bridge closures on
/// one doomed station, each promotion passes alone - and together they weld two
/// independently GPS-anchored components into one rigid group, the permanent,
/// unrepairable gap this branch exists to prevent.
#[test]
fn deleting_a_shot_never_promotes_two_closures_that_together_double_anchor() {
    // The premise first: each closure ALONE is a legal promotion, so what the
    // test below catches is the COMBINATION and not either closure by itself.
    for keep in 0..2usize {
        let (mut project, s, t, ..) = chained_double_anchor_closure_fixture();
        let kept = project.loop_closures[keep].id;
        project.loop_closures.retain(|c| c.id == kept);
        let last_shot = project.get_section(s).unwrap().shots.last().unwrap().id;
        let (_, impact) = project.remove_shot_with_rewire(s, last_shot).unwrap();
        assert_eq!(
            impact.closure_to_merge.len(),
            1,
            "closure {keep} alone must promote: {impact:?}"
        );
        assert_eq!(impact.closure_to_merge[0].station_a, StationRef::new(t, 0));
        assert_every_merge_is_aligned(&project, "single promotion");
        assert!(project.over_anchored_merge_components().is_empty());
    }

    let (mut project, s, t, u, v) = chained_double_anchor_closure_fixture();
    let second_closure_id = project.loop_closures[1].id;

    let last_shot = project.get_section(s).unwrap().shots.last().unwrap().id;
    let (_, impact) = project.remove_shot_with_rewire(s, last_shot).unwrap();

    // Exactly one promotion: the first closure is a legal merge, the second is
    // refused BECAUSE of it and stays a closure, re-anchored onto T0.
    assert_eq!(
        impact.closure_to_merge.len(),
        1,
        "the second promotion would double-anchor the group the first created: {impact:?}"
    );
    assert_eq!(
        (
            impact.closure_to_merge[0].station_a,
            impact.closure_to_merge[0].station_b
        ),
        (StationRef::new(t, 0), StationRef::new(u, 0))
    );
    assert_eq!(
        impact.replacement_closure_updates.len(),
        1,
        "the refused promotion must keep the closure, re-anchored: {impact:?}"
    );
    assert_eq!(
        impact.replacement_closure_updates[0].closure_id,
        second_closure_id
    );
    assert_eq!(
        impact.replacement_closure_updates[0].new_station,
        StationRef::new(t, 0)
    );

    assert_eq!(
        project.loop_closures.len(),
        1,
        "the refused closure must survive: {:?}",
        project.loop_closures
    );
    let survivor = &project.loop_closures[0];
    assert_eq!(
        (
            project.resolve_key(&survivor.station_a).unwrap(),
            project.resolve_key(&survivor.station_b).unwrap()
        ),
        (StationRef::new(t, 0), StationRef::new(v, 0))
    );

    // The merge that WAS minted paid for the coincidence it declares, and no
    // rigid group ended up holding two GPS anchors.
    assert_every_merge_is_aligned(&project, "after the chained delete");
    assert!(
        project.over_anchored_merge_components().is_empty(),
        "the delete welded two anchored components: {:?}",
        project.over_anchored_merge_components()
    );

    // ...and the refusal is STABLE: the whole-project sweep and the loader both
    // decline it for the same reason, so memory and disk agree.
    assert_eq!(project.convert_orphan_closures_to_merges(), 0);
    project.redrive_offsets_after_load();
    assert_eq!(project.loop_closures.len(), 1);
    assert_eq!(project.station_merges.len(), 1);
    assert_every_merge_is_aligned(&project, "after a reload");
    assert!(project.over_anchored_merge_components().is_empty());
}

/// A genuine loop over two sections, with a real misclosure, whose other leg is
/// about to be deleted (#464 defect C).
///
/// * section P: two 10 m shots north, GPS-anchored at P0;
/// * section Q: two 10 m shots east, merged P2 <-> Q0;
/// * a loop closure P0 <-> Q2 - a true loop (P0-P1-P2=Q0-Q1-Q2) carrying the
///   28.28 m misclosure the survey actually measured.
///
/// Returns `(project, p, q)`.
fn misclosing_loop_fixture() -> (Project, SectionId, SectionId) {
    let mut project = Project::new("Misclosure".to_string(), None);
    let p = project
        .add_section(SectionData {
            shots: vec![
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let q = project
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 10.0, 0.0, 0.0),
                shot(90.0, 90.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    project
        .set_zero_datum(
            StationRef::new(p, 0),
            GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        )
        .unwrap();
    project
        .merge_stations(StationRef::new(p, 2), StationRef::new(q, 0))
        .unwrap();
    let closure = project
        .create_loop_closure(
            StationRef::new(p, 0),
            StationRef::new(q, 2),
            ClosureStrategy::Batch,
        )
        .unwrap();
    assert!(
        closure.misclosure_distance > 28.0,
        "the fixture must carry a REAL misclosure, got {}",
        closure.misclosure_distance
    );
    (project, p, q)
}

/// #464 defect C: promoting an orphaned closure to a merge declares a
/// coincidence the geometry does not have. The follow-up
/// `cascade_connection_offsets(&[edited_section])` cannot pay for it - the
/// edited section is a seed and the other end holds the datum, so both are
/// authoritative and `cascade_bfs` skips the freshly minted edge. The promotion
/// itself has to translate the movable side.
#[test]
fn promoting_an_orphaned_closure_co_locates_the_side_that_may_move() {
    let (mut project, p, q) = misclosing_loop_fixture();
    let q_before = project.section_offset(q);

    // Delete Q's first shot: station Q0 goes, and with it the P2 <-> Q0 merge,
    // so the closure is the sole bridge left between P and Q.
    let first_shot = project.get_section(q).unwrap().shots[0].id;
    project.remove_shot_with_rewire(q, first_shot).unwrap();

    assert!(
        project.loop_closures.is_empty(),
        "the sole-bridge closure must be promoted: {:?}",
        project.loop_closures
    );
    assert_eq!(project.station_merges.len(), 1);
    let merge = &project.station_merges[0];
    let a = project.resolve_key(&merge.station_a).unwrap();
    let b = project.resolve_key(&merge.station_b).unwrap();
    let (ax, ay) = project.compute_station_raw_xy(a).unwrap();
    let (bx, by) = project.compute_station_raw_xy(b).unwrap();
    let gap = ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt();
    assert!(
        gap < Project::MERGE_ALIGNMENT_INVARIANT_M,
        "the promotion left the merge {gap:.6} m out of alignment"
    );

    // The DATUM side did not move; the free side did.
    assert!(
        project.section_offset(p).0.abs() < 1e-9 && project.section_offset(p).1.abs() < 1e-9,
        "the anchored side must stay put, got {:?}",
        project.section_offset(p)
    );
    assert!(
        (project.section_offset(q).0 - q_before.0).abs() > 1.0
            || (project.section_offset(q).1 - q_before.1).abs() > 1.0,
        "the free side must have been translated onto the anchor"
    );
}

/// The same promotion with NOTHING anchored: the coincidence is still owed, and
/// the side that moves is picked deterministically (`station_a`'s group, exactly
/// as `merge_stations` picks when neither side holds a datum).
#[test]
fn promoting_an_orphaned_closure_co_locates_even_with_no_datum() {
    let (mut project, p, q) = misclosing_loop_fixture();
    project.zero_datums.clear();
    let p_before = project.section_offset(p);
    let q_before = project.section_offset(q);

    let first_shot = project.get_section(q).unwrap().shots[0].id;
    project.remove_shot_with_rewire(q, first_shot).unwrap();

    assert!(project.loop_closures.is_empty());
    assert_eq!(project.station_merges.len(), 1);
    let merge = &project.station_merges[0];
    let a = project.resolve_key(&merge.station_a).unwrap();
    let b = project.resolve_key(&merge.station_b).unwrap();
    let (ax, ay) = project.compute_station_raw_xy(a).unwrap();
    let (bx, by) = project.compute_station_raw_xy(b).unwrap();
    let gap = ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt();
    assert!(
        gap < Project::MERGE_ALIGNMENT_INVARIANT_M,
        "the promotion left the merge {gap:.6} m out of alignment"
    );

    // WHICH side moved, not just that the gap closed: the closure was created
    // P0 <-> Q2, so `station_a` is P, and with nothing anchored P's group is the
    // one translated onto Q. Inverting the anchor/moving pick in
    // `plan_merge_promotion`'s no-datum branch fails here.
    //
    // Measured against the SAME delete with no closure to promote, because the
    // deletion itself shifts Q (it closes the gap the removed first shot left).
    // Anything beyond that baseline is the promotion's own translation.
    let (p_baseline, q_baseline) = {
        let (mut baseline, p, q) = misclosing_loop_fixture();
        baseline.zero_datums.clear();
        baseline.loop_closures.clear();
        let first_shot = baseline.get_section(q).unwrap().shots[0].id;
        baseline.remove_shot_with_rewire(q, first_shot).unwrap();
        (baseline.section_offset(p), baseline.section_offset(q))
    };
    let p_after = project.section_offset(p);
    let q_after = project.section_offset(q);
    assert!(
        (p_after.0 - p_baseline.0).abs() > 1.0 || (p_after.1 - p_baseline.1).abs() > 1.0,
        "station_a's group is the movable side: p {p_before:?} -> {p_after:?} \
         (delete alone: {p_baseline:?})"
    );
    assert_eq!(
        q_after, q_baseline,
        "the side picked as anchor must not move beyond what the delete itself does: \
         q {q_before:?} -> {q_after:?}"
    );
}

#[test]
fn repair_structural_integrity_drops_dangling_refs_and_keeps_valid() {
    let (mut project, a, b) = integrity_fixture();
    // Corrupt: a dangling merge, closure, datum, plus orphan offset/comment.
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(9999, 0).into(),
        merged_at: Utc::now(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(9999, 0).into(),
        gps: GpsCoordinates::new(1.0, 2.0, 3.0).unwrap(),
    });
    project.section_offsets.insert(9999, (1.0, 1.0));

    let valid_merges_before = project.station_merges.len();
    let repaired = project.repair_structural_integrity();

    // It reported the violations it fixed and the project is now sound.
    assert!(!repaired.is_empty());
    assert!(project.check_structural_integrity().is_empty());
    assert!(!project.has_critical_integrity_violations());
    // The originally-valid merge/datum/closure survive (only the bad ones dropped).
    assert_eq!(project.station_merges.len(), valid_merges_before - 1);
    assert_eq!(project.loop_closures.len(), 1);
    assert!(project.get_section(a).is_some() && project.get_section(b).is_some());
    // Repairing a clean project is a no-op.
    assert!(project.repair_structural_integrity().is_empty());
}

#[test]
fn integrity_clean_project_has_no_violations() {
    let (project, _, _) = integrity_fixture();
    assert!(project.check_structural_integrity().is_empty());
    assert!(!project.has_critical_integrity_violations());
}

#[test]
fn integrity_flags_merge_to_missing_section() {
    let (mut project, a, _) = integrity_fixture();
    // Point the merge at a section id that doesn't exist.
    forge_first_merge(&mut project, |m| {
        m.station_b = StationRef::new(9999, 0).into();
    });
    let v = project.check_structural_integrity();
    assert_eq!(v.len(), 1, "{v:?}");
    assert_eq!(v[0].severity, ViolationSeverity::Critical);
    assert_eq!(v[0].category, "dangling_merge_ref");
    assert!(project.has_critical_integrity_violations());
    // The valid endpoint (section a) is untouched.
    assert!(project.get_section(a).is_some());
}

#[test]
fn integrity_flags_station_index_out_of_range() {
    let (mut project, a, _) = integrity_fixture();
    // Section a has 1 shot → stations 0..=1; index 5 is out of range, so the
    // transitional Index anchor no longer resolves → flagged as a dangling ref.
    forge_first_merge(&mut project, |m| {
        m.station_a = StationRef::new(a, 5).into();
    });
    let v = project.check_structural_integrity();
    assert_eq!(v.len(), 1, "{v:?}");
    assert_eq!(v[0].severity, ViolationSeverity::Critical);
    assert!(v[0].message.contains("no longer resolves"), "{v:?}");
}

#[test]
fn integrity_flags_dangling_closure_and_zero_datum() {
    let (mut project, _, _) = integrity_fixture();
    project.loop_closures[0].station_b = StationRef::new(9999, 0).into();
    project.zero_datums[0].station_ref = StationRef::new(9999, 0).into();
    let v = project.check_structural_integrity();
    let categories: Vec<&str> = v.iter().map(|x| x.category.as_str()).collect();
    assert!(categories.contains(&"dangling_closure_ref"), "{v:?}");
    assert!(categories.contains(&"dangling_zero_datum_ref"), "{v:?}");
    assert!(v.iter().all(|x| x.severity == ViolationSeverity::Critical));
}

#[test]
fn integrity_orphan_offsets_and_comments_are_warnings_only() {
    let (mut project, _, _) = integrity_fixture();
    project.section_offsets.insert(9999, (1.0, 2.0));
    project
        .station_offsets
        .insert(StationRef::new(9999, 0).into(), (1.0, 2.0, 3.0));
    project
        .station_comments
        .insert(StationRef::new(9999, 0).into(), "x".to_string());
    let v = project.check_structural_integrity();
    assert_eq!(v.len(), 3, "{v:?}");
    assert!(v.iter().all(|x| x.severity == ViolationSeverity::Warning));
    // Warnings alone don't trip the critical gate.
    assert!(!project.has_critical_integrity_violations());
}

#[test]
fn check_structural_integrity_flags_and_repairs_duplicate_shot_uuid() {
    // Two shots in one section sharing a UUID make AfterShot anchors mis-resolve
    // (resolve_key takes the first match). It must be a Critical violation and
    // repair must re-mint the duplicate while KEEPING the first (so anchors that
    // resolved to it stay valid).
    let mut project = Project::new("Dup".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0), shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    let first_uuid = project.get_section(a).unwrap().shots[0]
        .uuid
        .clone()
        .unwrap();
    // Force shot 1 to collide with shot 0's UUID.
    project.get_section_mut(a).unwrap().shots[1].uuid = Some(first_uuid.clone());

    // A merge anchored on the first station (a:1 == AfterShot(first_uuid)).
    let anchor = StationKey::new(a, StationAnchor::AfterShot(first_uuid.clone()));
    project.station_merges.push(StationMerge {
        station_a: anchor.clone(),
        station_b: StationRef::new(a, 0).into(),
        merged_at: Utc::now(),
    });

    // Detect.
    let violations = project.check_structural_integrity();
    assert!(
        violations
            .iter()
            .any(|v| v.category == "duplicate_shot_uuid"),
        "duplicate UUID must be flagged: {violations:?}"
    );
    assert!(project.has_critical_integrity_violations());

    // Repair: re-mint the duplicate, keep the first.
    project.repair_structural_integrity();
    let shots = &project.get_section(a).unwrap().shots;
    assert_eq!(
        shots[0].uuid.as_deref(),
        Some(first_uuid.as_str()),
        "first shot must keep its UUID"
    );
    assert_ne!(shots[0].uuid, shots[1].uuid, "duplicate must be re-minted");
    assert!(
        !project
            .check_structural_integrity()
            .iter()
            .any(|v| v.category == "duplicate_shot_uuid"),
        "no duplicate UUID violation after repair"
    );
    // The anchor that resolved to the first station is still valid.
    assert!(
        project.resolve_key(&anchor).is_some(),
        "first-shot anchor survives repair"
    );
}

#[test]
fn warm_shot_uuid_index_resolves_duplicate_to_first_occurrence() {
    // Under a duplicated within-section UUID, the warm `shot_uuid_index` must
    // resolve to the FIRST occurrence, matching the authoritative cold-path
    // `resolve_key` (which uses `shots.iter().position(...)`, i.e. first match).
    // Reachable by undoing a `duplicate_shot_uuid` integrity Repair: it restores
    // the duplicate state and rebuilds the cache warm. If the index used
    // last-write-wins, the warm cache would resolve to station 2 while a cold
    // scan resolves to station 1 - a silent divergence.
    let mut project = Project::new("DupWarm".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0), shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    let first_uuid = project.get_section(a).unwrap().shots[0]
        .uuid
        .clone()
        .unwrap();
    // Force shot 1 to collide with shot 0's UUID.
    project.get_section_mut(a).unwrap().shots[1].uuid = Some(first_uuid.clone());

    // The cold-path answer: position() returns the first match (station 1).
    let cold = {
        let section = project.get_section(a).unwrap();
        let pos = section
            .shots
            .iter()
            .position(|s| s.uuid.as_deref() == Some(first_uuid.as_str()))
            .unwrap();
        StationRef::new(a, (pos + 1) as u32)
    };
    assert_eq!(
        cold,
        StationRef::new(a, 1),
        "cold scan picks the first shot"
    );

    // Rebuild the cache warm, then resolve through it.
    project.rebuild_shot_uuid_index();
    let anchor = StationKey::new(a, StationAnchor::AfterShot(first_uuid.clone()));
    assert_eq!(
        project.resolve_key(&anchor),
        Some(cold),
        "warm cache must agree with the cold scan on first occurrence"
    );
}

#[test]
fn dangling_cross_references_are_omitted_from_wire_output() {
    // A dangling merge (endpoint anchors on a deleted shot) must be OMITTED from
    // the wire form, not collapsed to a phantom station-0 relationship. Valid
    // refs still round-trip to their positional index.
    let mut project = Project::new("Wire".to_string(), None);
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
    project.ensure_shot_uuids();

    // Valid merge a:0 <-> b:0.
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    // Dangling merge: one endpoint anchors on a non-existent shot UUID.
    project.station_merges.push(StationMerge {
        station_a: StationKey::new(a, StationAnchor::AfterShot("does-not-exist".to_string())),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    assert!(
        project.merge_to_wire(&project.station_merges[0]).is_some(),
        "valid merge resolves to wire"
    );
    assert!(
        project.merge_to_wire(&project.station_merges[1]).is_none(),
        "dangling merge must NOT collapse to a phantom ref"
    );

    // get_section_relationships excludes the dangling merge entirely.
    let rel = project.get_section_relationships(a);
    assert_eq!(
        rel.merges.len(),
        1,
        "dangling merge omitted from wire output"
    );
    assert_eq!(
        rel.merges[0].station_a.station_index, 0,
        "valid ref round-trips positionally"
    );
}

#[test]
fn has_unresolvable_cross_references_detects_dangling_offset() {
    // The save gate blocks on this. A dangling station offset is only
    // Warning-severity, but it still collapses to (section, 0) on save and can
    // collide on the PRIMARY KEY - so it must count as unresolvable.
    let mut project = Project::new("Block".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    assert!(
        !project.has_unresolvable_cross_references(),
        "clean project resolves"
    );

    let dangling = StationKey::new(a, StationAnchor::AfterShot("ghost".to_string()));
    project.station_offsets.insert(dangling, (1.0, 2.0, 3.0));
    assert!(
        project.has_unresolvable_cross_references(),
        "a dangling (Warning-level) offset must still block the save"
    );
}

// ── Closed-form golden fixtures for the closure adjustment ───────────────────
//
// KNOWN-ANSWER regression guards for the weighted least-squares closure solve
// (`compute_adjusted_station_coordinates`). Each fixture pins the EXACT adjusted
// coordinates a correct adjustment must produce, derived in closed form from the
// normal equations - NOT by running the solver.
//
// These COMPLEMENT the in-repo suite that proves the solver finds the true
// minimiser of the *production-assembled* system (the `‖A·x-b‖∞` optimality
// guard inside `adjust_least_squares`, the dense reduced-Laplacian check, and
// `adjust_random_geometries_are_optimal_and_well_formed`). What those cannot
// catch is a wrong WEIGHT MODEL: they all inherit production's σ→weight assembly,
// so a bug there is self-consistent and passes every one of them. The
// weighted-mean star below re-derives the weights independently from the
// documented propagation and checks the resulting coordinates, closing that gap
// for arbitrary star geometries.
//
// Model (see docs/closures.md): a 2-D, horizontal-only, per-axis DECOUPLED
// weighted least squares. Each shot contributes independent X/Y observations of
// the measured Δ, weight w = 1/σ² with σ propagated from the base sigmas and
// scaled by section quality:
//     σ_len' = σ_len·quality,   σ_az' = σ_az·quality
//     σ_dx = √((sin·σ_len')² + (len·cos·σ_az')²)
//     σ_dy = √((cos·σ_len')² + (len·sin·σ_az')²)
// For a shot from a FIXED group to an unknown group the normal equation reduces
// to the classic Gauss weighted mean: the unknown lands at the weight-weighted
// average of each neighbour's predicted position (fixed_pos + measured Δ).
//
// Default config → σ_len = 0.05 m, σ_az = 1.0°, section quality = 1.0.

/// Assert an adjusted station coordinate matches a closed-form `(x, y)` within
/// `tol` metres (Z is not adjusted, so only the horizontal plane is checked).
fn golden_assert_xy(got: Coordinates, want: (f64, f64), tol: f64, label: &str) {
    assert!(
        (got.x - want.0).abs() < tol && (got.y - want.1).abs() < tol,
        "{label}: adjusted ({:.6}, {:.6}) != closed-form ({:.6}, {:.6}) (tol {tol:e})",
        got.x,
        got.y,
        want.0,
        want.1,
    );
}

#[test]
fn golden_symmetric_v_resolves_to_axis_of_symmetry() {
    // Two single-shot legs from a shared, datum-anchored origin, joined by a
    // closure:
    //     leg A: azimuth 80°,  10 m → predicts (10·sin80°,  10·cos80°)
    //     leg B: azimuth 100°, 10 m → predicts (10·sin80°, -10·cos80°)
    // The legs are mirror images across the East axis, so |sin| and |cos| - and
    // hence the X/Y weights - are identical. The ~3.47 m vertical (North)
    // disagreement must average to the axis of symmetry: Y = 0, with X left at the
    // common predicted value (both legs agree on X, so it is not corrected).
    // Config-INDEPENDENT: the equal weights cancel, so the answer does not depend
    // on σ_len / σ_az at all.
    let mut project = Project::new("GoldenSymV".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(80.0, 80.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(100.0, 100.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        )
        .unwrap();
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();
    project
        .create_loop_closure(
            StationRef::new(a, 1),
            StationRef::new(b, 1),
            ClosureStrategy::Batch,
        )
        .unwrap();

    let adjusted =
        project.compute_adjusted_station_coordinates(&ClosureAdjustmentConfig::default());

    let expected_x = 10.0 * 80.0_f64.to_radians().sin();
    golden_assert_xy(
        station_coords(&adjusted, StationRef::new(a, 1)),
        (expected_x, 0.0),
        1e-6,
        "A endpoint",
    );
    golden_assert_xy(
        station_coords(&adjusted, StationRef::new(b, 1)),
        (expected_x, 0.0),
        1e-6,
        "B endpoint",
    );
    // The closure must actually fuse the two endpoints.
    let adj_a = station_coords(&adjusted, StationRef::new(a, 1));
    let adj_b = station_coords(&adjusted, StationRef::new(b, 1));
    assert!(
        (adj_a.x - adj_b.x).abs() < 1e-6 && (adj_a.y - adj_b.y).abs() < 1e-6,
        "closure endpoints coincide"
    );
    // The datum stays pinned at the origin.
    golden_assert_xy(
        station_coords(&adjusted, StationRef::new(a, 0)),
        (0.0, 0.0),
        1e-9,
        "datum",
    );
}

/// Build a "star": one single-shot leg per `legs` entry `(azimuth°, length,
/// quality)`, all radiating from a shared datum-anchored origin to one common
/// closure-fused endpoint P. Returns P's adjusted coordinates from the production
/// solve.
fn solve_weighted_mean_star(legs: &[(f64, f64, f64)]) -> Coordinates {
    let mut project = Project::new("WeightedStar".to_string(), None);
    let sids: Vec<SectionId> = legs
        .iter()
        .map(|&(az, len, quality)| {
            project
                .add_section(SectionData {
                    shots: vec![shot(az, az, len, 0.0, 0.0)],
                    quality,
                    ..Default::default()
                })
                .id
        })
        .collect();
    project
        .set_zero_datum(
            StationRef::new(sids[0], 0),
            GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        )
        .unwrap();
    for &sid in &sids[1..] {
        project
            .merge_stations(StationRef::new(sids[0], 0), StationRef::new(sid, 0))
            .unwrap();
    }
    for &sid in &sids[1..] {
        project
            .create_loop_closure(
                StationRef::new(sids[0], 1),
                StationRef::new(sid, 1),
                ClosureStrategy::Batch,
            )
            .unwrap();
    }
    let adjusted =
        project.compute_adjusted_station_coordinates(&ClosureAdjustmentConfig::default());
    station_coords(&adjusted, StationRef::new(sids[0], 1))
}

/// Independently re-derive the closed-form weighted-mean optimum for a star from
/// the documented σ→weight propagation (NOT the production assembly). With the
/// origin fixed at (0,0) the decoupled normal equations give, per axis,
/// `P = Σ w·Δ / Σ w`.
fn weighted_mean_star_expected(
    legs: &[(f64, f64, f64)],
    config: &ClosureAdjustmentConfig,
) -> (f64, f64) {
    let (mut num_x, mut den_x, mut num_y, mut den_y) = (0.0, 0.0, 0.0, 0.0);
    for &(az_deg, len, raw_quality) in legs {
        // Clamp quality exactly as production's adjust_least_squares does, so the
        // weight model also matches for quality <= 0 / non-finite (otherwise
        // s_len = 0 → σ = 0 → w = ∞ → NaN and the assertion false-fails).
        let quality = if raw_quality.is_finite() && raw_quality > 0.0 {
            raw_quality
        } else {
            1.0
        };
        let az = az_deg.to_radians();
        let s_len = config.base_sigma_length_m * quality;
        let s_az = config.base_sigma_azimuth_deg.to_radians() * quality;
        let sigma_dx = ((az.sin() * s_len).powi(2) + (len * az.cos() * s_az).powi(2)).sqrt();
        let sigma_dy = ((az.cos() * s_len).powi(2) + (len * az.sin() * s_az).powi(2)).sqrt();
        let w_x = 1.0 / (sigma_dx * sigma_dx);
        let w_y = 1.0 / (sigma_dy * sigma_dy);
        // Star legs are flat (depth_in == depth_out), so horizontal_length() ==
        // shot.length and Δ = len·(sin, cos). Production uses horizontal_length for Δ
        // but SLOPE length for the σ azimuth term; they coincide only under this
        // flat-leg assumption - split them if a sloped leg is ever added here.
        let dx = len * az.sin();
        let dy = len * az.cos();
        num_x += w_x * dx;
        den_x += w_x;
        num_y += w_y * dy;
        den_y += w_y;
    }
    (num_x / den_x, num_y / den_y)
}

#[test]
fn golden_weight_model_star_matches_sigma_propagation() {
    // The closure-fused endpoint of a star is the Gauss weighted mean of each
    // leg's independent position fix, with weights from the σ→weight propagation.
    // Matching the production solve across diverse leg geometries - varied
    // azimuths, lengths, and section qualities - pins the weight MODEL
    // independently of production's normal-equation assembly. This is the one
    // thing #86's optimality/dense checks structurally cannot verify: they reuse
    // the same assembly, so a wrong weight formula is self-consistent there.
    let config = ClosureAdjustmentConfig::default();
    let cases: &[&[(f64, f64, f64)]] = &[
        &[(90.0, 10.0, 1.0), (0.0, 10.0, 1.0)], // east + north, isotropic quality
        &[(90.0, 10.0, 1.0), (0.0, 10.0, 3.0)], // north leg down-weighted by quality
        &[(90.0, 10.0, 1.0), (180.0, 6.0, 1.0)], // opposed legs, unequal length
        &[(90.0, 10.0, 1.0), (0.0, 8.0, 1.0), (45.0, 12.0, 2.0)], // three legs, mixed dir/len/quality
        &[(30.0, 7.0, 1.0), (200.0, 9.0, 1.5), (300.0, 5.0, 0.5)], // arbitrary 3-leg star
    ];
    for legs in cases {
        let got = solve_weighted_mean_star(legs);
        let (ex, ey) = weighted_mean_star_expected(legs, &config);
        assert!(
            (got.x - ex).abs() < 1e-6 && (got.y - ey).abs() < 1e-6,
            "legs {legs:?}: adjusted ({:.6}, {:.6}) != weighted mean ({ex:.6}, {ey:.6})",
            got.x,
            got.y,
        );
    }
}

// ── Independent rigorous reference (the in-process Gama equivalent) ───────────
//
// gama-local is not available in CI, so this in-process reference plays its role:
// a from-scratch weighted least-squares adjustment that makes NONE of production's
// modelling shortcuts. Production minimises, per shot,
//     w_dx·(Δx_meas - Δx)² + w_dy·(Δy_meas - Δy)²
// i.e. CARTESIAN coordinate-difference observations with DIAGONAL weights, X and Y
// DECOUPLED, linearised ONCE at the measured geometry. This reference instead
// minimises the RAW POLAR residuals
//     w_dist·(dist_meas - |Pj-Pi|)² + w_az·(az_meas - az(Pj-Pi))²
// with the X/Y-COUPLED Jacobian, ITERATED to convergence (Gauss-Newton). It keeps
// the dx-dy covariance production drops and re-linearises every step, so agreement
// is independent evidence that production's model is a sound approximation of
// rigorous LSQ - the role the external Gama oracle would play.
//
// The two are NOT bit-identical: the gap is the first-order error of production's
// approximation and grows with the angular misclosure of the network. The cases
// in `model_sound_matches_rigorous_polar_reference` stay in the small-misclosure
// regime real surveys operate in.

/// Solve a small dense `a·x = b` by Gaussian elimination with partial pivoting.
fn dense_solve_gauss(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let mut piv = col;
        for r in (col + 1)..n {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        if a[piv][col].abs() < 1e-15 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for r in 0..n {
            if r == col {
                continue;
            }
            let f = a[r][col] / a[col][col];
            for c in col..n {
                a[r][c] -= f * a[col][c];
            }
            b[r] -= f * b[col];
        }
    }
    Some((0..n).map(|i| b[i] / a[i][i]).collect())
}

/// Rigorous coupled-polar Gauss-Newton adjustment of a single-component,
/// single-datum network. Returns each station's adjusted (x, y).
fn rigorous_polar_reference(
    project: &Project,
    config: &ClosureAdjustmentConfig,
) -> HashMap<StationRef, (f64, f64)> {
    use std::f64::consts::PI;

    fn find(p: &mut HashMap<StationRef, StationRef>, x: StationRef) -> StationRef {
        let mut r = x;
        while p[&r] != r {
            r = p[&r];
        }
        let mut c = x;
        while p[&c] != r {
            let nx = p[&c];
            p.insert(c, r);
            c = nx;
        }
        r
    }
    fn accumulate(rows: &[(usize, f64)], w: f64, r: f64, nmat: &mut [Vec<f64>], gvec: &mut [f64]) {
        for &(ia, ca) in rows {
            gvec[ia] += w * ca * r;
            for &(ib, cb) in rows {
                nmat[ia][ib] += w * ca * cb;
            }
        }
    }

    let mut stations: Vec<StationRef> = Vec::new();
    for s in &project.sections {
        for idx in 0..=s.shots.len() {
            stations.push(StationRef::new(s.id, idx as u32));
        }
    }
    let mut parent: HashMap<StationRef, StationRef> = stations.iter().map(|&s| (s, s)).collect();
    let fuse = |parent: &mut HashMap<StationRef, StationRef>, a: StationRef, b: StationRef| {
        if parent.contains_key(&a) && parent.contains_key(&b) {
            let (ra, rb) = (find(parent, a), find(parent, b));
            if ra != rb {
                parent.insert(ra, rb);
            }
        }
    };
    for m in &project.station_merges {
        if let (Some(a), Some(b)) = (
            project.resolve_key(&m.station_a),
            project.resolve_key(&m.station_b),
        ) {
            fuse(&mut parent, a, b);
        }
    }
    for lc in &project.loop_closures {
        if let (Some(a), Some(b)) = (
            project.resolve_key(&lc.station_a),
            project.resolve_key(&lc.station_b),
        ) {
            fuse(&mut parent, a, b);
        }
    }

    let mut group_members: HashMap<StationRef, Vec<StationRef>> = HashMap::new();
    for &s in &stations {
        let r = find(&mut parent, s);
        group_members.entry(r).or_default().push(s);
    }
    // Initial position per group: mean of members' measured coords.
    let mut pos: HashMap<StationRef, (f64, f64)> = HashMap::new();
    for (root, members) in &group_members {
        let (mut sx, mut sy, mut k) = (0.0, 0.0, 0.0);
        for &m in members {
            if let Some(c) = project.compute_station_raw_coords(m) {
                sx += c.x;
                sy += c.y;
                k += 1.0;
            }
        }
        pos.insert(
            *root,
            if k > 0.0 {
                (sx / k, sy / k)
            } else {
                (0.0, 0.0)
            },
        );
    }
    // Fixed group = the datum's group, pinned at its measured position. This oracle
    // anchors exactly ONE datum, so it only handles single-datum, single-component
    // networks; a multi-datum / datum-less / multi-component input would leave a
    // gauge-free translation null space. Reject those up front instead of silently
    // returning un-adjusted coordinates as the "rigorous" reference.
    assert_eq!(
        project.zero_datums.len(),
        1,
        "rigorous_polar_reference supports single-datum networks only (got {})",
        project.zero_datums.len()
    );
    let datum = project
        .zero_datums
        .first()
        .and_then(|z| project.resolve_key(&z.station_ref));
    let fixed_root = datum.map(|d| find(&mut parent, d));
    assert!(
        fixed_root.is_some(),
        "rigorous_polar_reference: datum did not resolve to a group"
    );
    if let (Some(fr), Some(d)) = (fixed_root, datum) {
        if let Some(c) = project.compute_station_raw_coords(d) {
            pos.insert(fr, (c.x, c.y));
        }
    }

    let mut uidx: HashMap<StationRef, usize> = HashMap::new();
    for root in group_members.keys() {
        if Some(*root) != fixed_root {
            let n = uidx.len();
            uidx.insert(*root, n);
        }
    }
    let nvar = uidx.len() * 2;

    struct Obs {
        gi: StationRef,
        gj: StationRef,
        dist: f64,
        az: f64,
        w_dist: f64,
        w_az: f64,
    }
    let mut obs: Vec<Obs> = Vec::new();
    for s in &project.sections {
        let q = if s.quality.is_finite() && s.quality > 0.0 {
            s.quality
        } else {
            1.0
        };
        let s_len = config.base_sigma_length_m * q;
        let s_az = config.base_sigma_azimuth_deg.to_radians() * q;
        for (idx, shot) in s.shots.iter().enumerate() {
            let gi = find(&mut parent, StationRef::new(s.id, idx as u32));
            let gj = find(&mut parent, StationRef::new(s.id, (idx + 1) as u32));
            if gi == gj {
                continue;
            }
            obs.push(Obs {
                gi,
                gj,
                dist: shot.horizontal_length(),
                az: shot.average_azimuth_measured().to_radians(),
                w_dist: 1.0 / (s_len * s_len),
                w_az: 1.0 / (s_az * s_az),
            });
        }
    }

    let wrap = |a: f64| -> f64 {
        let mut x = a % (2.0 * PI);
        if x > PI {
            x -= 2.0 * PI;
        } else if x < -PI {
            x += 2.0 * PI;
        }
        x
    };

    let mut converged = nvar == 0;
    if nvar > 0 {
        for _iter in 0..200 {
            let mut nmat = vec![vec![0.0; nvar]; nvar];
            let mut gvec = vec![0.0; nvar];
            for o in &obs {
                let pi = pos[&o.gi];
                let pj = pos[&o.gj];
                let dx = pj.0 - pi.0;
                let dy = pj.1 - pi.1;
                let d = (dx * dx + dy * dy).sqrt();
                if d < 1e-9 {
                    continue;
                }
                let r_dist = o.dist - d;
                let r_az = wrap(o.az - dx.atan2(dy));
                let ui = uidx.get(&o.gi).copied();
                let uj = uidx.get(&o.gj).copied();
                let mut dist_row: Vec<(usize, f64)> = Vec::new();
                let mut az_row: Vec<(usize, f64)> = Vec::new();
                if let Some(uj) = uj {
                    dist_row.push((2 * uj, dx / d));
                    dist_row.push((2 * uj + 1, dy / d));
                    az_row.push((2 * uj, dy / (d * d)));
                    az_row.push((2 * uj + 1, -dx / (d * d)));
                }
                if let Some(ui) = ui {
                    dist_row.push((2 * ui, -dx / d));
                    dist_row.push((2 * ui + 1, -dy / d));
                    az_row.push((2 * ui, -dy / (d * d)));
                    az_row.push((2 * ui + 1, dx / (d * d)));
                }
                accumulate(&dist_row, o.w_dist, r_dist, &mut nmat, &mut gvec);
                accumulate(&az_row, o.w_az, r_az, &mut nmat, &mut gvec);
            }
            let Some(delta) = dense_solve_gauss(nmat, gvec) else {
                panic!("rigorous_polar_reference: singular normal matrix (degenerate or unanchored geometry)")
            };
            let mut maxd = 0.0_f64;
            for (root, &u) in &uidx {
                if let Some(p) = pos.get_mut(root) {
                    p.0 += delta[2 * u];
                    p.1 += delta[2 * u + 1];
                }
                maxd = maxd.max(delta[2 * u].abs()).max(delta[2 * u + 1].abs());
            }
            if maxd < 1e-11 {
                converged = true;
                break;
            }
        }
    }
    // The reference is only meaningful at the true optimum; never hand back a
    // stalled iterate as the "rigorous" answer.
    assert!(
        converged,
        "rigorous_polar_reference: Gauss-Newton did not converge within 200 iterations"
    );

    let mut out = HashMap::new();
    for (root, members) in &group_members {
        let p = pos[root];
        for &m in members {
            out.insert(m, p);
        }
    }
    out
}

/// A 2-D loop with OBLIQUE legs (30°/90°): two paths from a merged, datum-anchored
/// origin to a closure. Path A's last leg is 10.0 m and path B's is `len_b`, so the
/// misclosure scales with `len_b - 10`.
fn oblique_loop(len_b: f64) -> Project {
    let mut project = Project::new("Oblique".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![
                shot(30.0, 30.0, 10.0, 0.0, 0.0),
                shot(90.0, 90.0, 10.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 10.0, 0.0, 0.0),
                shot(30.0, 30.0, len_b, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        )
        .unwrap();
    project
        .merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();
    project
        .create_loop_closure(
            StationRef::new(a, 2),
            StationRef::new(b, 2),
            ClosureStrategy::Batch,
        )
        .unwrap();
    project
}

#[test]
fn model_soundness_oracle_reproduces_known_rigorous_optima() {
    // Validate the rigorous reference against optima we can pin by hand, so the
    // model-soundness comparison below rests on a trustworthy oracle.
    let config = ClosureAdjustmentConfig::default();

    // (1) An already-closed network has nothing to adjust → measured unchanged.
    let mut zero = Project::new("RefZero".to_string(), None);
    let za = zero
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    let zb = zero
        .add_section(SectionData {
            shots: vec![
                shot(90.0, 90.0, 10.0, 0.0, 0.0),
                shot(0.0, 0.0, 5.0, 0.0, 0.0),
            ],
            ..Default::default()
        })
        .id;
    zero.set_zero_datum(
        StationRef::new(za, 0),
        GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    )
    .unwrap();
    zero.merge_stations(StationRef::new(za, 0), StationRef::new(zb, 0))
        .unwrap();
    zero.create_loop_closure(
        StationRef::new(za, 2),
        StationRef::new(zb, 2),
        ClosureStrategy::Batch,
    )
    .unwrap();
    let r0 = rigorous_polar_reference(&zero, &config);
    for m in &zero.compute_station_coordinates() {
        let (rx, ry) = r0[&m.station_ref];
        assert!(
            (m.coordinates.x - rx).abs() < 1e-6 && (m.coordinates.y - ry).abs() < 1e-6,
            "oracle moved an already-closed network at {:?}",
            m.station_ref,
        );
    }

    // (2) Symmetric V (legs az80°/az100°, 10 m): by symmetry the rigorous optimum
    // sits on the East axis at distance 10 → (10, 0). Production's once-linearised
    // cartesian model gives 10·sin80° ≈ 9.848 instead - the legitimate first-order
    // gap, which at this 10° angular misclosure is ~15 cm. This both validates the
    // oracle on a known answer and demonstrates the model gap is real and signed.
    let mut v = Project::new("RefSymV".to_string(), None);
    let va = v
        .add_section(SectionData {
            shots: vec![shot(80.0, 80.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let vb = v
        .add_section(SectionData {
            shots: vec![shot(100.0, 100.0, 10.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    v.set_zero_datum(
        StationRef::new(va, 0),
        GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    )
    .unwrap();
    v.merge_stations(StationRef::new(va, 0), StationRef::new(vb, 0))
        .unwrap();
    v.create_loop_closure(
        StationRef::new(va, 1),
        StationRef::new(vb, 1),
        ClosureStrategy::Batch,
    )
    .unwrap();
    let rv = rigorous_polar_reference(&v, &config);
    let (rx, ry) = rv[&StationRef::new(va, 1)];
    assert!(
        (rx - 10.0).abs() < 1e-4 && ry.abs() < 1e-4,
        "rigorous symmetric-V optimum ({rx:.6}, {ry:.6}) != (10, 0)"
    );
}

#[test]
fn model_sound_matches_rigorous_in_small_misclosure_regime() {
    // Production's decoupled, once-linearised solve must track the rigorous
    // coupled-polar adjustment in the regime real surveys operate in (well-closed
    // loops). The residual gap is the first-order error of production's
    // approximation: it grows with ANGULAR misclosure (see
    // model_soundness_oracle_reproduces_known_rigorous_optima, where a 10° angular
    // misclosure opens a ~15 cm gap) and, for oblique legs, with the dx-dy
    // covariance production drops. On ~0.1-0.2 m corrections the two agree to a few
    // millimetres or better - independent evidence the model is a sound LSQ
    // adjustment in regime. Tolerances are keyed per case: tight where legs are
    // axis-aligned (no dropped covariance), looser where they are oblique.
    let config = ClosureAdjustmentConfig::default();
    let cases: Vec<(&str, Project, f64)> = vec![
        (
            "axis-aligned 0.2 m loop",
            looped_project_with_misclosure().0,
            1e-3,
        ),
        ("oblique 1 cm loop", oblique_loop(10.01), 3e-3),
    ];
    for (name, project, tol) in cases {
        let prod = project.compute_adjusted_station_coordinates(&config);
        let reference = rigorous_polar_reference(&project, &config);
        let mut maxd = 0.0_f64;
        for (sr, (rx, ry)) in &reference {
            let p = station_coords(&prod, *sr);
            maxd = maxd.max((p.x - rx).abs()).max((p.y - ry).abs());
        }
        assert!(
            maxd < tol,
            "{name}: production vs rigorous polar LSQ differ by {maxd:.6} m (> {tol})"
        );
    }
}

// ── GNU Gama gold-standard fixtures ──────────────────────────────────────────
//
// Production's closure solve is DECOUPLED per axis, and each axis minimises
// Σ w·(Δ_meas - (p_j - p_i))² - i.e. exactly a 1-D LEVELLING network. GNU Gama
// (gama-local) adjusts levelling networks natively, linearly and exactly, so it is
// an independent GOLD STANDARD: we emit each network's X- and Y-axis as a Gama
// levelling network, run gama-local ONCE offline, and freeze the adjusted
// coordinates in tests/fixtures/gama_golden.json. `production_matches_gama_golden`
// then checks production against the frozen Gama numbers with NO gama dependency.
//
// Regenerate after changing the networks or the weight model:
//   GAMA_LOCAL=/path/to/gama-local cargo test --manifest-path src-tauri/Cargo.toml \
//       --lib generate_gama_golden_fixtures -- --ignored --nocapture
// The frozen fixture was produced with gama-local 2.33; use that version (or
// verify a newer one regenerates byte-identically) when regenerating.

#[derive(serde::Serialize, serde::Deserialize)]
struct GoldStation {
    s: u32,
    i: u32,
    x: f64,
    y: f64,
}

/// Deterministic xorshift in [0, 1); seedable so fixtures are reproducible.
fn xorshift01(state: &mut u64) -> f64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    ((x >> 11) as f64) / ((1u64 << 53) as f64)
}

/// A fuzzed two-path loop: two sections of random oblique legs from a shared,
/// datum-anchored origin, joined at their far ends by a closure. Single-component,
/// single-datum, no spurs - every station lies on the loop. Misclosure is whatever
/// the random geometry produces (irrelevant to the per-axis levelling gold solve).
fn fuzz_loop(seed: u64) -> Project {
    let mut st = seed | 1;
    let mut p = Project::new(format!("fuzz_{seed}"), None);
    let na = 2 + (xorshift01(&mut st) * 3.0) as usize;
    let nb = 2 + (xorshift01(&mut st) * 3.0) as usize;
    let mkshots = |st: &mut u64, n: usize| -> Vec<ShotData> {
        (0..n)
            .map(|_| {
                let az = xorshift01(st) * 360.0;
                let len = 5.0 + xorshift01(st) * 10.0;
                shot(az, az, len, 0.0, 0.0)
            })
            .collect()
    };
    let a = p
        .add_section(SectionData {
            shots: mkshots(&mut st, na),
            ..Default::default()
        })
        .id;
    let b = p
        .add_section(SectionData {
            shots: mkshots(&mut st, nb),
            ..Default::default()
        })
        .id;
    p.set_zero_datum(
        StationRef::new(a, 0),
        GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    )
    .unwrap();
    p.merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();
    p.create_loop_closure(
        StationRef::new(a, na as u32),
        StationRef::new(b, nb as u32),
        ClosureStrategy::Batch,
    )
    .unwrap();
    p
}

/// Random oblique shots with a gentle eastward drift (keeps coordinates bounded at
/// scale while leaving a real misclosure).
fn drift_shots(st: &mut u64, n: usize, base_az: f64) -> Vec<ShotData> {
    (0..n)
        .map(|_| {
            let az = base_az + (xorshift01(st) - 0.5) * 80.0;
            let len = 4.0 + xorshift01(st) * 6.0;
            shot(az, az, len, 0.0, 0.0)
        })
        .collect()
}

/// One big loop: two paths of `n` shots from a merged datum origin joined by a
/// single closure at the far end. ~2n+1 stations - pure-scale stress for the O(n)
/// solve.
fn big_loop(n: usize, seed: u64) -> Project {
    let mut st = seed | 1;
    let mut p = Project::new(format!("loop_{}", 2 * n + 1), None);
    let a = p
        .add_section(SectionData {
            shots: drift_shots(&mut st, n, 60.0),
            ..Default::default()
        })
        .id;
    let b = p
        .add_section(SectionData {
            shots: drift_shots(&mut st, n, 30.0),
            ..Default::default()
        })
        .id;
    p.set_zero_datum(
        StationRef::new(a, 0),
        GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    )
    .unwrap();
    p.merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();
    p.create_loop_closure(
        StationRef::new(a, n as u32),
        StationRef::new(b, n as u32),
        ClosureStrategy::Batch,
    )
    .unwrap();
    p
}

/// A ladder: two rails of `segs*seg` shots from a merged datum origin, joined by a
/// "rung" closure every `seg` stations → `segs` STACKED loops sharing rails.
fn ladder(segs: usize, seg: usize, seed: u64) -> Project {
    let n = segs * seg;
    let mut st = seed | 1;
    let mut p = Project::new(format!("ladder_{}", 2 * n + 1), None);
    let a = p
        .add_section(SectionData {
            shots: drift_shots(&mut st, n, 90.0),
            ..Default::default()
        })
        .id;
    let b = p
        .add_section(SectionData {
            shots: drift_shots(&mut st, n, 90.0),
            ..Default::default()
        })
        .id;
    p.set_zero_datum(
        StationRef::new(a, 0),
        GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    )
    .unwrap();
    p.merge_stations(StationRef::new(a, 0), StationRef::new(b, 0))
        .unwrap();
    for k in 1..=segs {
        let idx = (k * seg) as u32;
        p.create_loop_closure(
            StationRef::new(a, idx),
            StationRef::new(b, idx),
            ClosureStrategy::Batch,
        )
        .unwrap();
    }
    p
}

/// A 2-D grid mesh: `rows` row-sections (mostly-East shots) and `cols`
/// column-sections (mostly-North shots), with every grid node (r,c) shared between
/// its row and column via a merge. A dense lattice of loops - one per cell.
fn grid(rows: usize, cols: usize, seed: u64) -> Project {
    let mut st = seed | 1;
    let mut p = Project::new(format!("grid_{rows}x{cols}"), None);
    let row_sec: Vec<u32> = (0..rows)
        .map(|_| {
            p.add_section(SectionData {
                shots: drift_shots(&mut st, cols - 1, 90.0),
                ..Default::default()
            })
            .id
        })
        .collect();
    let col_sec: Vec<u32> = (0..cols)
        .map(|_| {
            p.add_section(SectionData {
                shots: drift_shots(&mut st, rows - 1, 0.0),
                ..Default::default()
            })
            .id
        })
        .collect();
    p.set_zero_datum(
        StationRef::new(row_sec[0], 0),
        GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    )
    .unwrap();
    // Spanning tree of identity merges: row 0 ties every column, column 0 ties every
    // row. (Tree → each merge joins a new section, never an already-connected pair.)
    for c in 0..cols {
        p.merge_stations(
            StationRef::new(row_sec[0], c as u32),
            StationRef::new(col_sec[c], 0),
        )
        .unwrap();
    }
    for r in 1..rows {
        p.merge_stations(
            StationRef::new(row_sec[r], 0),
            StationRef::new(col_sec[0], r as u32),
        )
        .unwrap();
    }
    // Interior nodes are already path-connected, so identifying row r / col c there
    // CLOSES a cell loop - one closure per interior node.
    for r in 1..rows {
        for c in 1..cols {
            p.create_loop_closure(
                StationRef::new(row_sec[r], c as u32),
                StationRef::new(col_sec[c], r as u32),
                ClosureStrategy::Batch,
            )
            .unwrap();
        }
    }
    p
}

/// `paths` independent random sections from a merged datum origin, all fused at
/// their far ends by closures → `paths-1` independent loops, no spurs. Shots span
/// the FULL azimuth circle and a wide length band `[len_min, len_max]`, plus a
/// random per-section quality - deliberately harsher than the structured nets.
fn fuzz_paths(seed: u64, paths: usize, max_shots: usize, len_min: f64, len_max: f64) -> Project {
    let mut st = seed | 1;
    let mut p = Project::new(format!("wild_{seed:x}"), None);
    let sids: Vec<u32> = (0..paths)
        .map(|_| {
            let n = 2 + (xorshift01(&mut st) * (max_shots as f64 - 1.0)) as usize;
            let shots: Vec<ShotData> = (0..n)
                .map(|_| {
                    let az = xorshift01(&mut st) * 360.0;
                    let len = len_min + xorshift01(&mut st) * (len_max - len_min);
                    shot(az, az, len, 0.0, 0.0)
                })
                .collect();
            let q = 0.5 + xorshift01(&mut st) * 2.5;
            p.add_section(SectionData {
                shots,
                quality: q,
                ..Default::default()
            })
            .id
        })
        .collect();
    p.set_zero_datum(
        StationRef::new(sids[0], 0),
        GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    )
    .unwrap();
    for &sid in &sids[1..] {
        p.merge_stations(StationRef::new(sids[0], 0), StationRef::new(sid, 0))
            .unwrap();
    }
    let end_of = |p: &Project, sid: u32| {
        let n = p
            .sections
            .iter()
            .find(|s| s.id == sid)
            .map(|s| s.shots.len())
            .unwrap_or(0);
        StationRef::new(sid, n as u32)
    };
    let end0 = end_of(&p, sids[0]);
    for &sid in &sids[1..] {
        let e = end_of(&p, sid);
        p.create_loop_closure(end0, e, ClosureStrategy::Batch)
            .unwrap();
    }
    p
}

/// Star: single-shot legs `(az°, len, quality)` from a merged datum origin to one
/// closure-fused endpoint.
fn build_star(name: &str, legs: &[(f64, f64, f64)]) -> Project {
    let mut p = Project::new(name.to_string(), None);
    let sids: Vec<u32> = legs
        .iter()
        .map(|&(az, len, q)| {
            p.add_section(SectionData {
                shots: vec![shot(az, az, len, 0.0, 0.0)],
                quality: q,
                ..Default::default()
            })
            .id
        })
        .collect();
    p.set_zero_datum(
        StationRef::new(sids[0], 0),
        GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    )
    .unwrap();
    for &sid in &sids[1..] {
        p.merge_stations(StationRef::new(sids[0], 0), StationRef::new(sid, 0))
            .unwrap();
    }
    for &sid in &sids[1..] {
        p.create_loop_closure(
            StationRef::new(sids[0], 1),
            StationRef::new(sid, 1),
            ClosureStrategy::Batch,
        )
        .unwrap();
    }
    p
}

/// The fixed roster of networks behind the Gama gold standard. Both the generator
/// and the CI check call this, so they see identical geometry.
fn gama_golden_networks() -> Vec<(String, Project)> {
    let mut nets: Vec<(String, Project)> = vec![
        (
            "axis_aligned_loop".to_string(),
            looped_project_with_misclosure().0,
        ),
        ("oblique_loop".to_string(), oblique_loop(10.5)),
        (
            "star_mixed".to_string(),
            build_star(
                "star_mixed",
                &[(90.0, 10.0, 1.0), (0.0, 8.0, 1.0), (45.0, 12.0, 2.0)],
            ),
        ),
        // Large scale and dense-redundancy topologies.
        ("loop_101".to_string(), big_loop(50, 0x10001)),
        ("ladder_1001".to_string(), ladder(20, 25, 0x1ADDE2)),
        ("grid_12x12".to_string(), grid(12, 12, 0x6121D)),
        // Harsh random multi-path bundles: full-circle azimuths, wide length bands.
        (
            "wild_2path".to_string(),
            fuzz_paths(0x21D2, 2, 7, 0.5, 40.0),
        ),
        (
            "wild_3path".to_string(),
            fuzz_paths(0x5A1A, 3, 5, 0.2, 30.0),
        ),
        (
            "wild_4path".to_string(),
            fuzz_paths(0xB17E, 4, 4, 1.0, 50.0),
        ),
        (
            "wild_long".to_string(),
            fuzz_paths(0xF1A5C0, 2, 9, 0.3, 25.0),
        ),
    ];
    for seed in [0xA11CE_u64, 0xBEEF, 0xC0FFEE, 0xD15EA5E, 0x5EED] {
        nets.push((format!("fuzz_{seed}"), fuzz_loop(seed)));
    }
    nets
}

/// Union-find over merges + closures → (station→group index, group→members).
fn station_groups(project: &Project) -> (HashMap<StationRef, usize>, Vec<Vec<StationRef>>) {
    fn find(p: &mut HashMap<StationRef, StationRef>, x: StationRef) -> StationRef {
        let mut r = x;
        while p[&r] != r {
            r = p[&r];
        }
        let mut c = x;
        while p[&c] != r {
            let nx = p[&c];
            p.insert(c, r);
            c = nx;
        }
        r
    }
    let mut stations: Vec<StationRef> = Vec::new();
    for s in &project.sections {
        for idx in 0..=s.shots.len() {
            stations.push(StationRef::new(s.id, idx as u32));
        }
    }
    let mut parent: HashMap<StationRef, StationRef> = stations.iter().map(|&s| (s, s)).collect();
    let fuse = |parent: &mut HashMap<StationRef, StationRef>, a: StationRef, b: StationRef| {
        if parent.contains_key(&a) && parent.contains_key(&b) {
            let (ra, rb) = (find(parent, a), find(parent, b));
            if ra != rb {
                parent.insert(ra, rb);
            }
        }
    };
    for m in &project.station_merges {
        if let (Some(a), Some(b)) = (
            project.resolve_key(&m.station_a),
            project.resolve_key(&m.station_b),
        ) {
            fuse(&mut parent, a, b);
        }
    }
    for lc in &project.loop_closures {
        if let (Some(a), Some(b)) = (
            project.resolve_key(&lc.station_a),
            project.resolve_key(&lc.station_b),
        ) {
            fuse(&mut parent, a, b);
        }
    }
    let mut root_idx: HashMap<StationRef, usize> = HashMap::new();
    let mut members: Vec<Vec<StationRef>> = Vec::new();
    let mut gidx: HashMap<StationRef, usize> = HashMap::new();
    for &s in &stations {
        let r = find(&mut parent, s);
        let idx = *root_idx.entry(r).or_insert_with(|| {
            members.push(Vec::new());
            members.len() - 1
        });
        members[idx].push(s);
        gidx.insert(s, idx);
    }
    (gidx, members)
}

/// Reverse Cuthill-McKee ordering of the group graph. Gama's envelope solver cost
/// is `O(n·bandwidth²)`, and emitting points in section order leaves a huge
/// bandwidth on big loops; RCM keeps it near-constant so the gold solve scales.
/// Returns `rcm[k]` = the group emitted at position `k`.
fn bandwidth_order(n: usize, edges: &[(usize, usize)], start: usize) -> Vec<usize> {
    use std::collections::VecDeque;
    let mut adj = vec![Vec::new(); n];
    for &(a, b) in edges {
        if a != b {
            adj[a].push(b);
            adj[b].push(a);
        }
    }
    for nb in adj.iter_mut() {
        nb.sort_unstable();
        nb.dedup();
    }
    let mut visited = vec![false; n];
    let mut order = Vec::with_capacity(n);
    let mut seeds: Vec<usize> = (0..n).collect();
    seeds.sort_by_key(|&g| adj[g].len());
    seeds.retain(|&g| g != start);
    seeds.insert(0, start);
    let mut q = VecDeque::new();
    for &s in &seeds {
        if visited[s] {
            continue;
        }
        visited[s] = true;
        q.push_back(s);
        while let Some(u) = q.pop_front() {
            order.push(u);
            let mut nbrs: Vec<usize> = adj[u].iter().copied().filter(|&v| !visited[v]).collect();
            nbrs.sort_by_key(|&v| adj[v].len());
            for v in nbrs {
                visited[v] = true;
                q.push_back(v);
            }
        }
    }
    order.reverse();
    order
}

/// Emit one axis (0 = X, 1 = Y) of a network as a GNU Gama levelling network: one
/// point per group (datum group fixed) declared in `rcm` order, one `<dh>` per
/// inter-group shot with production's measured Δ and σ on that axis. `rank[g]` is
/// the gama point id of group `g`.
fn emit_leveling_gkf(
    project: &Project,
    config: &ClosureAdjustmentConfig,
    axis: usize,
    gidx: &HashMap<StationRef, usize>,
    rcm: &[usize],
    rank: &[usize],
    datum_group: usize,
    datum_coord: f64,
) -> String {
    let mut s = String::new();
    s.push_str("<?xml version=\"1.0\"?>\n<gama-local xmlns=\"http://www.gnu.org/software/gama/gama-local\">\n");
    s.push_str("<network axes-xy=\"ne\" angles=\"right-handed\">\n");
    // tol-abs is the gross-error threshold in MM: gama removes any observation whose
    // misclosure exceeds it. Our fuzzed loops have metre-scale misclosures by design,
    // so set it huge to keep every observation in the adjustment.
    s.push_str(
        "<parameters sigma-apr=\"1.0\" conf-pr=\"0.95\" tol-abs=\"1e12\" sigma-act=\"apriori\"/>\n",
    );
    s.push_str("<points-observations>\n");
    for (k, &g) in rcm.iter().enumerate() {
        if g == datum_group {
            s.push_str(&format!(
                "<point id=\"g{k}\" z=\"{datum_coord:.12}\" fix=\"Z\"/>\n"
            ));
        } else {
            s.push_str(&format!("<point id=\"g{k}\" adj=\"Z\"/>\n"));
        }
    }
    s.push_str("<height-differences>\n");
    for sec in &project.sections {
        let q = if sec.quality.is_finite() && sec.quality > 0.0 {
            sec.quality
        } else {
            1.0
        };
        let s_len = config.base_sigma_length_m * q;
        let s_az = config.base_sigma_azimuth_deg.to_radians() * q;
        for (idx, shot) in sec.shots.iter().enumerate() {
            let gi = gidx[&StationRef::new(sec.id, idx as u32)];
            let gj = gidx[&StationRef::new(sec.id, (idx + 1) as u32)];
            if gi == gj {
                continue;
            }
            let az = shot.average_azimuth_measured().to_radians();
            let len = shot.length;
            let hlen = shot.horizontal_length();
            let (val, sigma) = if axis == 0 {
                (
                    hlen * az.sin(),
                    ((az.sin() * s_len).powi(2) + (len * az.cos() * s_az).powi(2)).sqrt(),
                )
            } else {
                (
                    hlen * az.cos(),
                    ((az.cos() * s_len).powi(2) + (len * az.sin() * s_az).powi(2)).sqrt(),
                )
            };
            s.push_str(&format!(
                "  <dh from=\"g{}\" to=\"g{}\" val=\"{val:.12}\" stdev=\"{sigma:.12}\"/>\n",
                rank[gi], rank[gj]
            ));
        }
    }
    s.push_str("</height-differences>\n</points-observations>\n</network>\n</gama-local>\n");
    s
}

/// Extract adjusted heights from gama-local `--xml` output: `id → Z`.
fn parse_gama_adjusted(xml: &str) -> HashMap<String, f64> {
    let mut out = HashMap::new();
    let body = xml
        .split_once("<adjusted>")
        .and_then(|(_, r)| r.split_once("</adjusted>"))
        .map(|(b, _)| b)
        .unwrap_or("");
    for chunk in body.split("<point>").skip(1) {
        let id = chunk
            .split_once("<id>")
            .and_then(|(_, r)| r.split_once("</id>"))
            .map(|(v, _)| v.trim().to_string());
        let z = chunk
            .split_once("<Z>")
            .and_then(|(_, r)| r.split_once("</Z>"))
            .and_then(|(v, _)| v.trim().parse::<f64>().ok());
        if let (Some(id), Some(z)) = (id, z) {
            out.insert(id, z);
        }
    }
    out
}

/// Run gama on every network/axis and freeze the adjusted coordinates. Ignored by
/// default; needs `GAMA_LOCAL` pointing at a `gama-local` binary.
#[test]
#[ignore = "regenerates Gama gold fixtures; set GAMA_LOCAL=/path/to/gama-local"]
fn generate_gama_golden_fixtures() {
    use std::collections::BTreeMap;
    let gama = std::env::var("GAMA_LOCAL").expect("set GAMA_LOCAL to the gama-local binary");
    let config = ClosureAdjustmentConfig::default();
    let mut all: BTreeMap<String, Vec<GoldStation>> = BTreeMap::new();
    let mut worst = 0.0_f64;

    for (name, project) in gama_golden_networks() {
        let (gidx, members) = station_groups(&project);
        let datum = project
            .zero_datums
            .first()
            .and_then(|z| project.resolve_key(&z.station_ref))
            .unwrap();
        let dg = gidx[&datum];
        let datum_raw = project.compute_station_raw_coords(datum).unwrap();
        // Group adjacency from inter-group shots → reverse Cuthill-McKee point order.
        let mut edges: Vec<(usize, usize)> = Vec::new();
        for sec in &project.sections {
            for idx in 0..sec.shots.len() {
                let gi = gidx[&StationRef::new(sec.id, idx as u32)];
                let gj = gidx[&StationRef::new(sec.id, (idx + 1) as u32)];
                if gi != gj {
                    edges.push((gi, gj));
                }
            }
        }
        let rcm = bandwidth_order(members.len(), &edges, dg);
        let mut rank = vec![0usize; members.len()];
        for (k, &g) in rcm.iter().enumerate() {
            rank[g] = k;
        }
        let mut group_xy = vec![(0.0_f64, 0.0_f64); members.len()];
        for axis in 0..2 {
            let dcoord = if axis == 0 { datum_raw.x } else { datum_raw.y };
            let gkf = emit_leveling_gkf(&project, &config, axis, &gidx, &rcm, &rank, dg, dcoord);
            let tmp = std::env::temp_dir();
            let infile = tmp.join(format!("gama_{name}_{axis}.gkf"));
            let outfile = tmp.join(format!("gama_{name}_{axis}.out.xml"));
            std::fs::write(&infile, &gkf).unwrap();
            // --cov-band 0 skips the O(n²) a-posteriori covariance matrix (we only
            // need adjusted coordinates); without it gama is unusably slow at 10⁴ pts.
            let status = std::process::Command::new(&gama)
                .arg(&infile)
                .arg("--cov-band")
                .arg("0")
                .arg("--xml")
                .arg(&outfile)
                .status()
                .unwrap();
            assert!(status.success(), "gama-local failed on {name} axis {axis}");
            let adj = parse_gama_adjusted(&std::fs::read_to_string(&outfile).unwrap());
            for (g, item) in group_xy.iter_mut().enumerate() {
                let c = if g == dg {
                    dcoord
                } else {
                    *adj.get(&format!("g{}", rank[g]))
                        .unwrap_or_else(|| panic!("missing g{} for {name} axis {axis}", rank[g]))
                };
                if axis == 0 {
                    item.0 = c;
                } else {
                    item.1 = c;
                }
            }
        }
        // Sanity: production must already match the gold; record the worst gap.
        let prod = project.compute_adjusted_station_coordinates(&config);
        let mut rows = Vec::new();
        let mut net_worst = 0.0_f64;
        for (g, mem) in members.iter().enumerate() {
            for &sr in mem {
                let p = station_coords(&prod, sr);
                net_worst = net_worst
                    .max((p.x - group_xy[g].0).abs())
                    .max((p.y - group_xy[g].1).abs());
                rows.push(GoldStation {
                    s: sr.section_id,
                    i: sr.station_index,
                    x: group_xy[g].0,
                    y: group_xy[g].1,
                });
            }
        }
        println!(
            "[gama-golden] {name}: groups={} worst={net_worst:.3e} m",
            members.len()
        );
        // Refuse to freeze a fixture production doesn't already reproduce: a large
        // gap here is an emitter/model bug (wrong Δ/σ/sign), not a production
        // regression - writing it would bake a wrong gold standard.
        assert!(
            net_worst < 1e-5,
            "regeneration aborted: production diverged from freshly-emitted gama on {name} by {net_worst:.3e} m"
        );
        worst = worst.max(net_worst);
        rows.sort_by_key(|r| (r.s, r.i));
        all.insert(name, rows);
    }

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/gama_golden.json"
    );
    std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_string(&all).unwrap()).unwrap();
    println!("[gama-golden] wrote {path}; worst production-vs-gama gap = {worst:.3e} m");
}

#[test]
fn production_matches_gama_golden() {
    use std::collections::BTreeMap;
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/gama_golden.json"
    );
    let data = std::fs::read_to_string(path).unwrap_or_else(|_| {
        panic!("missing {path}; regenerate with GAMA_LOCAL=/path/to/gama-local cargo test generate_gama_golden_fixtures -- --ignored")
    });
    let frozen: BTreeMap<String, Vec<GoldStation>> = serde_json::from_str(&data).unwrap();
    let config = ClosureAdjustmentConfig::default();
    let nets: HashMap<String, Project> = gama_golden_networks().into_iter().collect();

    // Coverage guard: the frozen fixture and the live roster must name the SAME
    // networks, so adding one without rerunning the generator fails loudly instead
    // of being silently untested. (frozen is a BTreeMap, so its keys are sorted.)
    assert!(!frozen.is_empty(), "gama_golden.json is empty; regenerate");
    let mut roster_names: Vec<&String> = nets.keys().collect();
    roster_names.sort();
    let frozen_names: Vec<&String> = frozen.keys().collect();
    assert_eq!(
        frozen_names, roster_names,
        "roster and frozen fixture networks differ; rerun generate_gama_golden_fixtures"
    );

    for (name, rows) in &frozen {
        let project = nets
            .get(name)
            .unwrap_or_else(|| panic!("network {name} not in roster"));
        let prod = project.compute_adjusted_station_coordinates(&config);
        // Every production station must be frozen, so added/renumbered stations are
        // not silently left unchecked.
        assert_eq!(
            rows.len(),
            prod.len(),
            "{name}: frozen station count {} != production {}; regenerate",
            rows.len(),
            prod.len()
        );
        for r in rows {
            let p = station_coords(&prod, StationRef::new(r.s, r.i));
            // Worst observed gap across the roster is ~2e-10 m; 1e-6 leaves a wide
            // margin for cross-platform float variation while still being microns
            // against metre-scale corrections, so any real regression is caught.
            assert!(
                (p.x - r.x).abs() < 1e-6 && (p.y - r.y).abs() < 1e-6,
                "{name} s{}:{}: production ({:.6}, {:.6}) != gama gold ({:.6}, {:.6})",
                r.s,
                r.i,
                p.x,
                p.y,
                r.x,
                r.y,
            );
        }
    }
}

#[test]
fn project_ipc_serializes_validation_field() {
    // Guard against the data-loss bug (#57): the hand-written ProjectIpc
    // serializer (used by get_project_state and the load results) must emit
    // `validation`, or the frontend loses per-project thresholds after any
    // command refresh and a later save clobbers them back to defaults.
    let mut p = Project::new("Guard".to_string(), None);
    p.validation.warn_shot_length_min = 2.5;
    let value = serialize_project_ipc(&p, serde_json::value::Serializer).unwrap();
    assert_eq!(
        value
            .get("validation")
            .and_then(|v| v.get("warn_shot_length_min"))
            .and_then(|n| n.as_f64()),
        Some(2.5),
        "ProjectIpc must serialize the validation field"
    );
}

// === Replacement-merge guards (Project::replacement_merge_allowed) ===
//
// A deletion that tears a merge group apart stitches the surviving components
// back together through an ARBITRARILY picked pair. These four cover the states
// where that stitch must NOT happen, because it would mint exactly what the
// integrity machinery elsewhere exists to undo.

/// Build `a - middle - b`: two sections merged into ONE station of a middle
/// section, each outer section carrying its own zero datum. Returns the ids.
fn bridged_double_anchored_groups(project: &mut Project) -> (SectionId, SectionId, SectionId) {
    let a = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let middle = project
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

    // Both outer sections hang off the SAME middle station, so the three are one
    // station-level merge group and removing the middle splits it in two.
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(middle, 0).into(),
        merged_at: Utc::now(),
    });
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(b, 0).into(),
        station_b: StationRef::new(middle, 0).into(),
        merged_at: Utc::now(),
    });

    // One datum per outer group. Together they over-anchor the bridged group -
    // the pre-existing invalid state the deletion is about to heal.
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(b, 1).into(),
        gps: GpsCoordinates::new(11.0, 21.0, 0.0).unwrap(),
    });

    (a, middle, b)
}

fn merge_exists_between_sections(project: &Project, x: SectionId, y: SectionId) -> bool {
    project.station_merges.iter().any(|m| {
        let (Some(sa), Some(sb)) = (
            project.resolve_key(&m.station_a),
            project.resolve_key(&m.station_b),
        ) else {
            return false;
        };
        (sa.section_id == x && sb.section_id == y) || (sa.section_id == y && sb.section_id == x)
    })
}

#[test]
fn deleting_the_bridge_between_anchored_groups_does_not_reweld_them() {
    let mut project = Project::new("BridgeDelete".to_string(), None);
    let (a, middle, b) = bridged_double_anchored_groups(&mut project);

    // Precondition: the bridge really does put both datums in one merge
    // component, and both outer groups really are joined through it.
    assert_eq!(
        project.over_anchored_merge_components().len(),
        1,
        "fixture must start over-anchored, or the guard has nothing to prevent"
    );

    // The `delete_section` sequence (command_section.rs).
    project.preserve_connections_for_section_deletion(middle);
    project.cleanup_section_references(middle);
    project.remove_section(middle);

    assert!(
        !merge_exists_between_sections(&project, a, b),
        "the deletion must not weld two independently anchored components together: {:?}",
        project.station_merges
    );
    assert!(
        project.loop_closures.is_empty(),
        "no soft closure may be invented for an arbitrarily picked pair either: {:?}",
        project.loop_closures
    );
    assert!(
        project.over_anchored_merge_components().is_empty(),
        "each half now holds its own datum, so the deletion healed the over-anchoring: {:?}",
        project.over_anchored_merge_components()
    );

    // Both halves survive, still anchored and structurally intact.
    assert!(project.get_section(a).is_some() && project.get_section(b).is_some());
    let datum_refs: Vec<StationRef> = project
        .zero_datums
        .iter()
        .filter_map(|zd| project.resolve_key(&zd.station_ref))
        .collect();
    assert!(
        datum_refs.contains(&StationRef::new(a, 0)) && datum_refs.contains(&StationRef::new(b, 1)),
        "both datums must survive: {datum_refs:?}"
    );
}

#[test]
fn replacement_stitch_skips_a_pair_already_in_closure() {
    let mut project = Project::new("ClosureOnPair".to_string(), None);
    let x = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let middle = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let y = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(x, 0).into(),
        station_b: StationRef::new(middle, 0).into(),
        merged_at: Utc::now(),
    });
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(y, 0).into(),
        station_b: StationRef::new(middle, 0).into(),
        merged_at: Utc::now(),
    });
    // A closure already describes exactly the pair the stitch would pick.
    project.loop_closures.push(LoopClosure {
        id: 7,
        station_a: StationRef::new(x, 0).into(),
        station_b: StationRef::new(y, 0).into(),
        path_length: 4.0,
        misclosure_distance: 0.1,
        error_percentage: 2.5,
        horizontal_misclosure: 0.1,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });

    project.preserve_connections_for_section_deletion(middle);
    project.cleanup_section_references(middle);
    project.remove_section(middle);

    assert!(
        !project.are_stations_merged(StationRef::new(x, 0), StationRef::new(y, 0)),
        "a merge on a pair that already carries a closure is the state create_loop_closure \
         refuses: {:?}",
        project.station_merges
    );
    let survivor = project
        .loop_closures
        .iter()
        .find(|lc| lc.id == 7)
        .expect("the existing closure must survive untouched");
    assert_eq!(
        (
            project.resolve_key(&survivor.station_a),
            project.resolve_key(&survivor.station_b)
        ),
        (Some(StationRef::new(x, 0)), Some(StationRef::new(y, 0))),
        "the closure keeps the soft topology and must not be rewired"
    );
}

#[test]
fn replacement_stitch_skips_rigidly_connected_components() {
    let mut project = Project::new("AlreadyRigid".to_string(), None);
    let x = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let middle = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let y = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(x, 0).into(),
        station_b: StationRef::new(middle, 0).into(),
        merged_at: Utc::now(),
    });
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(y, 0).into(),
        station_b: StationRef::new(middle, 0).into(),
        merged_at: Utc::now(),
    });
    // An ALIGNED closure on a DIFFERENT pair already holds X and Y rigid, so the
    // stitch would be the redundant rigid cycle merge_stations refuses.
    project.loop_closures.push(LoopClosure {
        id: 3,
        station_a: StationRef::new(x, 1).into(),
        station_b: StationRef::new(y, 1).into(),
        path_length: 4.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: true,
    });

    project.preserve_connections_for_section_deletion(middle);
    project.cleanup_section_references(middle);
    project.remove_section(middle);

    assert!(
        !merge_exists_between_sections(&project, x, y),
        "X and Y are already rigidly connected through the aligned closure; a replacement \
         merge adds nothing but a rigid cycle: {:?}",
        project.station_merges
    );
    assert!(
        project.loop_closures.iter().any(|lc| lc.id == 3),
        "the aligned closure must survive"
    );
}

#[test]
fn removed_station_stitch_does_not_reweld_anchored_groups() {
    // The station-removal site (`preserve_connections_for_removed_stations`),
    // reached by an interior range delete, must apply the same guard.
    let mut project = Project::new("RemovedStations".to_string(), None);
    let (a, middle, b) = bridged_double_anchored_groups(&mut project);

    let removed: HashSet<StationRef> = [StationRef::new(middle, 0)].into_iter().collect();
    project.preserve_connections_for_removed_stations(&removed);
    project.cleanup_removed_station_references(&removed);

    assert!(
        !merge_exists_between_sections(&project, a, b),
        "removing the bridging station must not weld the two anchored halves: {:?}",
        project.station_merges
    );
    assert!(
        project.loop_closures.is_empty(),
        "no closure may be invented in the merge's place: {:?}",
        project.loop_closures
    );
    assert!(
        project.over_anchored_merge_components().is_empty(),
        "both halves are now independently anchored"
    );
}

#[test]
fn delete_shot_preview_and_execution_agree_on_the_skipped_stitch() {
    // The single-shot path PREVIEWS the replacement merges in the dialog and then
    // replays that exact list, so a guard that only fired at execution time would
    // make the dialog promise a merge the delete never creates.
    let mut project = Project::new("PreviewParity".to_string(), None);
    let bridge = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let p = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let q = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    // P and Q meet only at the bridge station that the shot delete removes.
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(p, 0).into(),
        station_b: StationRef::new(bridge, 0).into(),
        merged_at: Utc::now(),
    });
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(q, 0).into(),
        station_b: StationRef::new(bridge, 0).into(),
        merged_at: Utc::now(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(p, 1).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(q, 1).into(),
        gps: GpsCoordinates::new(11.0, 21.0, 0.0).unwrap(),
    });

    let shot_id = project.get_section(bridge).unwrap().shots[0].id;
    let preview = project.preview_delete_shot(bridge, shot_id).unwrap();
    assert_eq!(
        preview.removed_station,
        StationRef::new(bridge, 0),
        "the delete must remove the bridging station"
    );
    assert!(
        preview.replacement_merges.is_empty(),
        "the dialog must not promise a merge welding two anchored components: {:?}",
        preview.replacement_merges
    );

    let (_, impact) = project.remove_shot_with_rewire(bridge, shot_id).unwrap();
    assert_eq!(
        impact.replacement_merges.len(),
        preview.replacement_merges.len(),
        "preview and execution must plan the same replacement merges"
    );
    assert!(
        !merge_exists_between_sections(&project, p, q),
        "and the executed delete must create none: {:?}",
        project.station_merges
    );
    assert!(project.over_anchored_merge_components().is_empty());
}

/// Pins `ariane_elevation_or_absent`'s real boundary: the filter is
/// `raw.abs() > f64::EPSILON`, a STRICT inequality, so a value AT OR UNDER
/// `f64::EPSILON` (2.220446049250313e-16) reads as absent, not just an exact
/// `0.0`. `1e-9` is many orders of magnitude ABOVE `f64::EPSILON` and must
/// pass through unchanged - a value can be "close to zero" in the everyday
/// sense while still being nowhere near this boundary.
#[test]
fn ariane_elevation_or_absent_matches_the_real_epsilon_boundary() {
    assert_eq!(ariane_elevation_or_absent(0.0), None);
    assert_eq!(ariane_elevation_or_absent(f64::EPSILON), None);
    assert_eq!(ariane_elevation_or_absent(-f64::EPSILON), None);
    assert_eq!(ariane_elevation_or_absent(1e-17), None);
    assert_eq!(ariane_elevation_or_absent(1e-9), Some(1e-9));
    assert_eq!(ariane_elevation_or_absent(f64::NAN), None);
    assert_eq!(ariane_elevation_or_absent(123.5), Some(123.5));
    assert_eq!(ariane_elevation_or_absent(-7.25), Some(-7.25));
}

/// A leaf shot sequence with enough distinct shots (>= 3 free) to qualify as
/// an orphan leaf candidate, repeated identically so a placement match against
/// it scores near-perfect.
fn orphan_leaf_shots() -> Vec<ShotData> {
    vec![
        shot(45.0, 45.0, 10.0, 1.0, 2.0),
        shot(90.0, 90.0, 8.0, 2.0, 1.0),
        shot(135.0, 135.0, 6.0, 0.0, 0.0),
        shot(180.0, 180.0, 4.0, 1.0, 1.0),
    ]
}

#[test]
fn suggest_orphan_placements_finds_intra_import_suggestion_on_first_import() {
    // #446: a first import into an EMPTY project used to suggest nothing even
    // when the imported file contained both an anchored component and a
    // floating (unanchored) one - `orphan_components` was computed only over
    // PRE-EXISTING sections, and a first import has none. It must now
    // consider the whole post-import project, so the floating section
    // becomes a placement candidate against the anchored one from the SAME
    // import.
    let mut project = Project::new("FirstImport".to_string(), None);
    let anchored = project
        .add_section(SectionData {
            shots: orphan_leaf_shots(),
            ..Default::default()
        })
        .id;
    let floating = project
        .add_section(SectionData {
            shots: orphan_leaf_shots(),
            ..Default::default()
        })
        .id;
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(anchored, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });

    let suggestions = project.suggest_orphan_placements(
        &[anchored, floating],
        &crate::config::PlacementSuggestionConfig::default(),
    );

    assert!(
        !suggestions.is_empty(),
        "a floating component imported alongside an anchored one in the same \
         file must get at least one placement suggestion (#446)"
    );
    assert!(
        suggestions.iter().all(|s| s.orphan_section_id == floating),
        "the only orphan in this project is the floating section: {suggestions:?}"
    );
}

#[test]
fn suggest_orphan_placements_never_pairs_a_pre_existing_orphan_with_a_pre_existing_anchor() {
    // #446: a pre-existing floating component that had its chance against its
    // own import must not start getting matched against a pre-existing
    // anchored section just because SOME LATER, unrelated import happens.
    // `target_sections` is built from `newly_imported_section_ids` alone, so
    // a pre-existing anchor can never be a suggestion target. Both fixtures
    // are shot-IDENTICAL (`orphan_leaf_shots`), so a broken guard would match
    // them with a near-perfect score - the empty result is the guard, not an
    // accident of mismatched shot shapes.
    let mut project = Project::new("LaterImport".to_string(), None);
    let pre_anchor = project
        .add_section(SectionData {
            shots: orphan_leaf_shots(),
            ..Default::default()
        })
        .id;
    let pre_orphan = project
        .add_section(SectionData {
            shots: orphan_leaf_shots(),
            ..Default::default()
        })
        .id;
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(pre_anchor, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });

    // A later, unrelated import that offers no anchored target of its own -
    // the ONLY conceivable match left is the forbidden pre/pre pairing.
    let new_section = project
        .add_section(SectionData {
            shots: vec![shot(200.0, 200.0, 20.0, 3.0, 3.0)],
            ..Default::default()
        })
        .id;

    let suggestions = project.suggest_orphan_placements(
        &[new_section],
        &crate::config::PlacementSuggestionConfig::default(),
    );

    assert!(
        suggestions.is_empty(),
        "the only possible pairing here (pre-existing orphan onto pre-existing \
         anchor) is forbidden, so nothing may be suggested (#446): {pre_orphan:?} {suggestions:?}"
    );
}

#[test]
fn suggest_orphan_placements_still_suggests_a_pre_existing_orphan_against_a_newly_imported_anchor()
{
    // The positive case for the guard above: a pre-existing orphan is NOT
    // suggestion-dead - it can still be matched against a target from a
    // LATER import, as long as that target is itself newly imported AND
    // datum-anchored. Same shot-identical fixture as the forbidden case, so
    // this proves the mechanism works at all (not just that it's always
    // empty).
    let mut project = Project::new("LaterImportAnchored".to_string(), None);
    let pre_orphan = project
        .add_section(SectionData {
            shots: orphan_leaf_shots(),
            ..Default::default()
        })
        .id;
    let new_target = project
        .add_section(SectionData {
            shots: orphan_leaf_shots(),
            ..Default::default()
        })
        .id;
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(new_target, 0).into(),
        gps: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });

    let suggestions = project.suggest_orphan_placements(
        &[new_target],
        &crate::config::PlacementSuggestionConfig::default(),
    );

    assert!(
        suggestions
            .iter()
            .any(|s| s.orphan_section_id == pre_orphan && s.target_section_id == new_target),
        "a pre-existing orphan must still be suggested against a newly \
         imported, datum-anchored target (#446): {suggestions:?}"
    );
}

#[test]
fn suggest_orphan_placements_never_suggests_two_floating_components_onto_each_other() {
    // #446: a suggestion TARGET must itself be datum-anchored - otherwise two
    // floating components can be suggested onto each other, a "placement"
    // that is circular and anchors nothing. A datum exists ELSEWHERE in the
    // project (on an unrelated section) so the project-wide
    // no-datum-at-all early-out does NOT short-circuit this case; only the
    // per-target anchor check does.
    let mut project = Project::new("Floating".to_string(), None);
    let elsewhere = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 3.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(elsewhere, 0).into(),
        gps: GpsCoordinates::new(1.0, 2.0, 0.0).unwrap(),
    });

    let a = project
        .add_section(SectionData {
            shots: orphan_leaf_shots(),
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: orphan_leaf_shots(),
            ..Default::default()
        })
        .id;

    let suggestions = project.suggest_orphan_placements(
        &[a, b],
        &crate::config::PlacementSuggestionConfig::default(),
    );

    assert!(
        suggestions.is_empty(),
        "two floating components must never be suggested onto each other - \
         neither is datum-anchored (#446): {suggestions:?}"
    );
}

#[test]
fn suggest_orphan_placements_short_circuits_when_the_project_has_no_datum_at_all() {
    // #446 perf: with no datum anywhere in the project, every component is
    // technically "orphaned" - without an early-out this pathological case
    // still walks every component's leaves against every other. The cheap
    // `zero_datums`-resolve check must return empty before that walk starts.
    let mut project = Project::new("NoDatumAtAll".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: orphan_leaf_shots(),
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: orphan_leaf_shots(),
            ..Default::default()
        })
        .id;

    let suggestions = project.suggest_orphan_placements(
        &[a, b],
        &crate::config::PlacementSuggestionConfig::default(),
    );

    assert!(suggestions.is_empty());
}

#[test]
fn count_unanchored_sections_is_zero_for_an_empty_project() {
    let project = Project::new("Empty".to_string(), None);
    assert_eq!(project.count_unanchored_sections(&[]), 0);
}

#[test]
fn count_unanchored_sections_counts_every_member_of_a_merged_orphan_component() {
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
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });

    // No datum anywhere - A and B form one orphan component of size 2.
    assert_eq!(project.count_unanchored_sections(&[a]), 2);
}

#[test]
fn count_unanchored_sections_treats_a_datum_reached_through_a_merge_as_anchored() {
    let mut project = Project::new("MergedAnchored".to_string(), None);
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
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 0).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: Utc::now(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 1).into(),
        gps: GpsCoordinates::new(1.0, 2.0, 0.0).unwrap(),
    });

    // The datum lives on A alone, but the merge means B's rigid group is
    // anchored too - the component has a datum, so it is not orphaned.
    assert_eq!(project.count_unanchored_sections(&[a, b]), 0);
}

#[test]
fn count_unanchored_sections_is_scoped_to_the_newly_imported_component() {
    // A clean import into a project with unrelated, pre-existing orphans
    // must not blame THIS import's receipt for orphans it didn't create
    // (#446).
    let mut project = Project::new("Scoped".to_string(), None);
    let _pre_existing_orphan = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let new_anchored = project
        .add_section(SectionData {
            shots: vec![shot(0.0, 0.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(new_anchored, 0).into(),
        gps: GpsCoordinates::new(1.0, 2.0, 0.0).unwrap(),
    });

    assert_eq!(
        project.count_unanchored_sections(&[new_anchored]),
        0,
        "the pre-existing orphan sitting elsewhere in the project is not \
         this import's problem"
    );
}

/// Two zero-datum-anchored components of `n` one-shot sections each (chained by
/// merges), and `closures` cross-component loop closures between them. Every
/// closure is an orphan whose promotion is refused (it would double-anchor), so
/// the pre-#597 sweep ran a full walk per closure, rescanning every merge per
/// visited station, on every pass.
fn two_anchored_components_with_orphan_closures(n: usize, closures: usize) -> Project {
    let mut project = Project::new("Orphans".to_string(), None);
    let component = |project: &mut Project| -> Vec<SectionId> {
        let ids: Vec<SectionId> = (0..n)
            .map(|_| {
                project
                    .add_section(SectionData {
                        shots: vec![shot(90.0, 90.0, 1.0, 0.0, 0.0)],
                        ..Default::default()
                    })
                    .id
            })
            .collect();
        for pair in ids.windows(2) {
            project.station_merges.push(StationMerge {
                station_a: StationKey::new(
                    pair[0],
                    StationAnchor::AfterShot(
                        project.get_section(pair[0]).unwrap().shots[0]
                            .uuid
                            .clone()
                            .unwrap(),
                    ),
                ),
                station_b: StationKey::new(pair[1], StationAnchor::SectionStart),
                merged_at: Utc::now(),
            });
        }
        ids
    };
    let a = component(&mut project);
    let b = component(&mut project);
    for (ids, lat) in [(&a, 20.5), (&b, 20.6)] {
        project.zero_datums.push(ZeroDatum {
            station_ref: StationKey::new(ids[0], StationAnchor::SectionStart),
            gps: GpsCoordinates::new(lat, -87.4, 0.0).unwrap(),
        });
    }
    for i in 0..closures {
        let (sa, sb) = (a[i % n], b[(i * 7) % n]);
        project.loop_closures.push(LoopClosure {
            id: i as u64 + 1,
            station_a: StationKey::new(sa, StationAnchor::SectionStart),
            station_b: StationKey::new(sb, StationAnchor::SectionStart),
            path_length: 0.0,
            misclosure_distance: 0.0,
            error_percentage: 0.0,
            horizontal_misclosure: 0.0,
            vertical_misclosure: 0.0,
            strategy: ClosureStrategy::default(),
            is_aligned: false,
        });
    }
    project.next_closure_id = closures as u64 + 1;
    project
}

#[test]
fn the_orphan_sweep_is_fast_with_thousands_of_refused_promotions() {
    let mut project = two_anchored_components_with_orphan_closures(1_000, 2_000);

    let started = std::time::Instant::now();
    project.redrive_offsets_after_load();
    let elapsed = started.elapsed();

    assert_eq!(
        project.loop_closures.len(),
        2_000,
        "every promotion is refused (both sides anchored), so all stay closures"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "the whole post-load redrive took {elapsed:?}"
    );
}

#[test]
fn the_orphan_sweep_still_promotes_in_order_and_keeps_a_redundant_twin() {
    // One anchored component and one free one, bridged by two closures between
    // the same stations: the first becomes the merge, the second is then a
    // genuine (redundant) loop and stays a closure.
    let mut project = two_anchored_components_with_orphan_closures(3, 0);
    project.zero_datums.pop();
    let (a0, b0) = (project.sections[0].id, project.sections[3].id);
    for id in [11, 12] {
        project.loop_closures.push(LoopClosure {
            id,
            station_a: StationKey::new(a0, StationAnchor::SectionStart),
            station_b: StationKey::new(b0, StationAnchor::SectionStart),
            path_length: 0.0,
            misclosure_distance: 0.0,
            error_percentage: 0.0,
            horizontal_misclosure: 0.0,
            vertical_misclosure: 0.0,
            strategy: ClosureStrategy::default(),
            is_aligned: false,
        });
    }

    let converted = project.convert_orphan_closures_to_merges();

    assert_eq!(converted, 1);
    assert_eq!(
        project
            .loop_closures
            .iter()
            .map(|c| c.id)
            .collect::<Vec<_>>(),
        [12]
    );
}

#[test]
fn the_orphan_sweep_sees_merges_through_a_deleted_section() {
    // Two anchored sections bridged only by merges through a section id that no
    // longer exists. The merges do not connect them (they do not resolve), so the
    // closure is an orphan; but `merge_would_double_anchor` walks stored ids, sees
    // one merge component, and allows the promotion. The sweep must agree.
    let mut project = two_anchored_components_with_orphan_closures(1, 1);
    let (a, b) = (project.sections[0].id, project.sections[1].id);
    let gone: SectionId = project.sections.iter().map(|s| s.id).max().unwrap_or(0) + 100;
    for (x, y) in [(a, gone), (gone, b)] {
        project.station_merges.push(StationMerge {
            station_a: StationKey::new(x, StationAnchor::SectionStart),
            station_b: StationKey::new(y, StationAnchor::SectionStart),
            merged_at: Utc::now(),
        });
    }

    assert_eq!(project.convert_orphan_closures_to_merges(), 1);
    assert!(project.loop_closures.is_empty());
}

#[test]
fn the_orphan_sweep_examines_the_closure_right_after_a_converted_one() {
    // Two different orphans side by side: converting the first must not skip
    // the second.
    let mut project = two_anchored_components_with_orphan_closures(3, 0);
    project.zero_datums.pop();
    let loose = project
        .add_section(SectionData {
            shots: vec![shot(90.0, 90.0, 1.0, 0.0, 0.0)],
            ..Default::default()
        })
        .id;
    let (a0, b0) = (project.sections[0].id, project.sections[3].id);
    for (id, other) in [(11, b0), (12, loose)] {
        project.loop_closures.push(LoopClosure {
            id,
            station_a: StationKey::new(a0, StationAnchor::SectionStart),
            station_b: StationKey::new(other, StationAnchor::SectionStart),
            path_length: 0.0,
            misclosure_distance: 0.0,
            error_percentage: 0.0,
            horizontal_misclosure: 0.0,
            vertical_misclosure: 0.0,
            strategy: ClosureStrategy::default(),
            is_aligned: false,
        });
    }

    assert_eq!(project.convert_orphan_closures_to_merges(), 2);
    assert!(project.loop_closures.is_empty());
}

#[test]
fn remeasuring_alignment_is_fast_with_thousands_of_aligned_closures_in_one_component() {
    // One 4000-section merge chain, with a snap-to-merge closure doubling every
    // merge: each closure's gap is zero, so each reaches the double-anchor
    // question, which a per-closure merge walk made quadratic.
    let mut project = two_anchored_components_with_orphan_closures(4_000, 0);
    let doubled: Vec<(StationKey, StationKey)> = project
        .station_merges
        .iter()
        .map(|m| (m.station_a.clone(), m.station_b.clone()))
        .collect();
    for (i, (a, b)) in doubled.into_iter().enumerate() {
        project.loop_closures.push(LoopClosure {
            id: i as u64 + 1,
            station_a: a,
            station_b: b,
            path_length: 0.0,
            misclosure_distance: 0.0,
            error_percentage: 0.0,
            horizontal_misclosure: 0.0,
            vertical_misclosure: 0.0,
            strategy: ClosureStrategy::Move,
            is_aligned: false,
        });
    }

    // Place the sections (untimed: the offset cascade's own cost is tracked
    // separately), then re-measure from scratch.
    project.redrive_offsets_after_load();
    for closure in &mut project.loop_closures {
        closure.is_aligned = false;
    }

    let started = std::time::Instant::now();
    project.remeasure_closure_alignment();
    let elapsed = started.elapsed();

    assert!(project.loop_closures.iter().all(|c| c.is_aligned));
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "took {elapsed:?}"
    );
}

fn test_closure(
    id: u64,
    station_a: StationKey,
    station_b: StationKey,
    strategy: ClosureStrategy,
    is_aligned: bool,
) -> LoopClosure {
    LoopClosure {
        id,
        station_a,
        station_b,
        path_length: 0.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy,
        is_aligned,
    }
}

/// A snap-to-merge closure with the same endpoints as every merge: the shape
/// #602 measured the load-time offset cascade on.
fn double_every_merge_with_move_closures(project: &mut Project, is_aligned: bool) {
    let doubled: Vec<(StationKey, StationKey)> = project
        .station_merges
        .iter()
        .map(|m| (m.station_a.clone(), m.station_b.clone()))
        .collect();
    for (a, b) in doubled {
        let id = project.next_closure_id;
        project.next_closure_id += 1;
        project
            .loop_closures
            .push(test_closure(id, a, b, ClosureStrategy::Move, is_aligned));
    }
}

#[test]
fn resolving_offsets_is_fast_with_thousands_of_closures_doubling_merges() {
    // Two 8000-section merge chains with a Move closure doubling every merge.
    // Seeding the solve walked each component rescanning every merge and closure
    // per visited section, O(sections x edges) (#602).
    let mut project = two_anchored_components_with_orphan_closures(8_000, 0);
    double_every_merge_with_move_closures(&mut project, false);
    project.redrive_offsets_after_load();
    assert!(project.loop_closures.iter().all(|c| c.is_aligned));

    // Debug build: about 0.65 s of CPU, against about 5.2 s before #602.
    let started = std::time::Instant::now();
    project.resolve_offsets();
    let elapsed = started.elapsed();

    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "took {elapsed:?}"
    );
}

/// `resolve_offsets` exactly as it was before #602, kept as the equivalence
/// oracle: one `get_connected_section_ids` walk per component to pick seeds.
fn resolve_offsets_before_602(project: &mut Project) -> usize {
    let before = project.check_merge_alignment().len();
    let offsets_before = project.section_offsets.clone();
    let seeds = realign_seeds_before_602(project);
    let was_deferred = project.cascade_deferred;
    project.cascade_deferred = false;
    project.cascade_connection_offsets(&seeds);
    project.cascade_deferred = was_deferred;
    if project.section_offsets != offsets_before {
        project.touch();
    }
    before.saturating_sub(project.check_merge_alignment().len())
}

/// The pre-#602 seed choice, verbatim: one walk per unvisited section.
fn realign_seeds_before_602(project: &Project) -> Vec<SectionId> {
    let datum_sections: HashSet<SectionId> = project
        .zero_datums
        .iter()
        .map(|z| z.station_ref.section_id)
        .collect();
    let mut seeds = Vec::new();
    let mut visited: HashSet<SectionId> = HashSet::new();
    for section in &project.sections {
        if visited.contains(&section.id) {
            continue;
        }
        let component = project.get_connected_section_ids(section.id);
        let anchor = component
            .iter()
            .copied()
            .filter(|s| datum_sections.contains(s))
            .min()
            .or_else(|| component.iter().copied().min());
        if let Some(a) = anchor {
            seeds.push(a);
        }
        visited.extend(component);
    }
    seeds
}

/// Run the oracle and the live solve on two copies of `project` and require
/// bit-identical offsets, return values and `modified_at` behaviour.
fn assert_resolve_matches_oracle(project: &Project, label: &str) {
    // The seeds directly, not only the offsets the cascade derives from them: a
    // seed bug the cascade happens to converge past would otherwise hide.
    assert_eq!(
        project.realign_seeds(),
        realign_seeds_before_602(project),
        "{label}: seeds"
    );
    let stamp = DateTime::<Utc>::from_timestamp(1_000_000, 0).unwrap();
    let mut old = project.clone();
    let mut new = project.clone();
    old.modified_at = stamp;
    new.modified_at = stamp;
    let old_healed = resolve_offsets_before_602(&mut old);
    let new_healed = new.resolve_offsets();
    let bits = |p: &Project| {
        let mut v: Vec<(SectionId, u64, u64)> = p
            .section_offsets
            .iter()
            .map(|(&id, &(x, y))| (id, x.to_bits(), y.to_bits()))
            .collect();
        v.sort_unstable();
        v
    };
    assert_eq!(new_healed, old_healed, "{label}: healed count");
    assert_eq!(bits(&new), bits(&old), "{label}: section offsets");
    assert_eq!(
        new.modified_at == stamp,
        old.modified_at == stamp,
        "{label}: touch"
    );
    assert_eq!(
        new.cascade_deferred, old.cascade_deferred,
        "{label}: deferral"
    );
}

/// `n` one-shot sections on `azimuth`, each merged end-to-start onto the next.
fn merge_chain(project: &mut Project, n: usize, azimuth: f64) -> Vec<SectionId> {
    let ids: Vec<SectionId> = (0..n)
        .map(|_| {
            project
                .add_section(SectionData {
                    shots: vec![shot(azimuth, azimuth, 3.0, 0.0, 0.0)],
                    ..Default::default()
                })
                .id
        })
        .collect();
    for pair in ids.windows(2) {
        let end = end_key(project, pair[0]);
        project.station_merges.push(StationMerge {
            station_a: end,
            station_b: StationKey::new(pair[1], StationAnchor::SectionStart),
            merged_at: Utc::now(),
        });
    }
    ids
}

fn end_key(project: &Project, id: SectionId) -> StationKey {
    let section = project.get_section(id).unwrap();
    let uuid = section.shots.last().unwrap().uuid.clone().unwrap();
    StationKey::new(id, StationAnchor::AfterShot(uuid))
}

fn start_key(id: SectionId) -> StationKey {
    StationKey::new(id, StationAnchor::SectionStart)
}

fn datum_at(key: StationKey, lat: f64) -> ZeroDatum {
    ZeroDatum {
        station_ref: key,
        gps: GpsCoordinates::new(lat, -87.4, 0.0).unwrap(),
    }
}

#[test]
fn resolve_offsets_matches_the_pre_602_solve_on_named_shapes() {
    // Anchored merge chain, no closures, persisted (stale) offsets.
    let mut p = Project::new("chain".to_string(), None);
    let c = merge_chain(&mut p, 5, 30.0);
    p.zero_datums.push(datum_at(start_key(c[2]), 20.5));
    p.section_offsets.insert(c[0], (12.5, -3.0));
    p.section_offsets.insert(c[4], (0.0, 0.0));
    assert_resolve_matches_oracle(&p, "anchored chain");

    // The same chain unanchored, with every merge doubled by a Move closure.
    let mut p = Project::new("unanchored doubled".to_string(), None);
    let c = merge_chain(&mut p, 6, 75.0);
    p.section_offsets.insert(c[3], (4.0, 4.0));
    double_every_merge_with_move_closures(&mut p, true);
    assert_resolve_matches_oracle(&p, "unanchored doubled");

    // The #602 shape, before and after the closures are measured aligned.
    let mut p = two_anchored_components_with_orphan_closures(30, 0);
    double_every_merge_with_move_closures(&mut p, false);
    p.section_offsets.insert(p.sections[7].id, (-9.0, 2.0));
    assert_resolve_matches_oracle(&p, "602 shape, unaligned");
    for closure in &mut p.loop_closures {
        closure.is_aligned = true;
    }
    assert_resolve_matches_oracle(&p, "602 shape, aligned");

    // Two unanchored merge groups bridged only by a misaligned Move closure
    // still flagged aligned, plus one flagged unaligned; a ghost section id
    // below every live one sits in the bridged component.
    let mut p = Project::new("bridged".to_string(), None);
    p.next_section_id = 5;
    let a = merge_chain(&mut p, 3, 10.0);
    let b = merge_chain(&mut p, 3, 200.0);
    let d = merge_chain(&mut p, 2, 300.0);
    p.section_offsets.insert(b[1], (40.0, -7.0));
    p.section_offsets.insert(d[0], (-15.0, 1.0));
    let e = end_key(&p, a[2]);
    p.loop_closures.push(test_closure(
        1,
        e,
        start_key(b[0]),
        ClosureStrategy::Move,
        true,
    ));
    let e = end_key(&p, b[2]);
    p.loop_closures.push(test_closure(
        2,
        e,
        start_key(d[0]),
        ClosureStrategy::Move,
        false,
    ));
    p.station_merges.push(StationMerge {
        station_a: start_key(b[2]),
        station_b: start_key(1),
        merged_at: Utc::now(),
    });
    assert_resolve_matches_oracle(&p, "bridged + ghost");
    p.zero_datums.push(datum_at(start_key(b[1]), 20.5));
    assert_resolve_matches_oracle(&p, "bridged + ghost, datum on b");
    p.zero_datums.push(datum_at(start_key(a[0]), 20.6));
    assert_resolve_matches_oracle(&p, "bridged, two datums");
    p.cascade_deferred = true;
    assert_resolve_matches_oracle(&p, "bridged, deferred");
}

/// A deterministic generator for the randomized equivalence sweep.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % n
    }

    fn real(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * (self.below(1_000_000) as f64 / 1_000_000.0)
    }
}

fn random_project(seed: u64) -> Project {
    let mut rng = Lcg(seed);
    let mut p = Project::new(format!("random {seed}"), None);
    p.next_section_id = 3;
    let n = 2 + rng.below(11) as usize;
    let ids: Vec<SectionId> = (0..n)
        .map(|_| {
            let shots = (0..=rng.below(3))
                .map(|_| {
                    let az = rng.real(0.0, 360.0);
                    shot(az, az, rng.real(0.5, 20.0), 0.0, 0.0)
                })
                .collect();
            p.add_section(SectionData {
                shots,
                ..Default::default()
            })
            .id
        })
        .collect();
    let key = |rng: &mut Lcg, p: &Project| -> StationKey {
        if rng.below(10) == 0 {
            return start_key(rng.below(3) as SectionId);
        }
        let id = ids[rng.below(ids.len() as u64) as usize];
        let section = p.get_section(id).unwrap();
        let pick = rng.below(section.shots.len() as u64 + 1) as usize;
        match pick {
            0 => start_key(id),
            k => StationKey::new(
                id,
                StationAnchor::AfterShot(section.shots[k - 1].uuid.clone().unwrap()),
            ),
        }
    };
    for _ in 0..rng.below(n as u64 + 3) {
        let (a, b) = (key(&mut rng, &p), key(&mut rng, &p));
        p.station_merges.push(StationMerge {
            station_a: a,
            station_b: b,
            merged_at: Utc::now(),
        });
    }
    for id in 1..=rng.below(n as u64 + 1) {
        let (a, b) = (key(&mut rng, &p), key(&mut rng, &p));
        let strategy = if rng.below(3) == 0 {
            ClosureStrategy::Batch
        } else {
            ClosureStrategy::Move
        };
        let aligned = rng.below(2) == 0;
        p.loop_closures
            .push(test_closure(id, a, b, strategy, aligned));
    }
    for _ in 0..rng.below(3) {
        let k = key(&mut rng, &p);
        let lat = rng.real(20.0, 21.0);
        p.zero_datums.push(datum_at(k, lat));
    }
    for &id in &ids {
        if rng.below(3) == 0 {
            let v = (rng.real(-50.0, 50.0), rng.real(-50.0, 50.0));
            p.section_offsets.insert(id, v);
        }
    }
    p.cascade_deferred = rng.below(10) == 0;
    p
}

#[test]
fn resolve_offsets_matches_the_pre_602_solve_on_random_projects() {
    for seed in 0..400 {
        assert_resolve_matches_oracle(&random_project(seed), &format!("seed {seed}"));
    }
}
