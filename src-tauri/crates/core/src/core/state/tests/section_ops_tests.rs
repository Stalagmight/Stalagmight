//! Section operations: create, the adjusted-station and distance caches, snapshot restore.
//!
//! Lines 2914-3509 of the former single-file `core/state/tests.rs` (#576 E1),
//! moved verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;
use crate::test_fixtures::{shot, state_with_project};

// === Section Operations Tests ===

#[test]
fn test_create_section() {
    let state = state_with_project("Test");

    let result = state.execute(Command::CreateSection {
        name: Some("Section A".to_string()),
        surveyor: Some("John".to_string()),
        date: None,
        color: "#4ecdc4".to_string(),
        direction: Some(SurveyDirection::In),
        shots: vec![shot(5.0), shot(7.0)],
    });

    assert!(result.success);
    assert!(result.section_id.is_some());

    let project = state.get_project_state().unwrap();
    assert_eq!(project.sections.len(), 1);
    assert_eq!(project.sections[0].name, Some("Section A".to_string()));
    assert_eq!(project.sections[0].shots.len(), 2);
}

#[test]
fn get_adjusted_stations_none_without_project() {
    let state = AppState::new().unwrap();
    assert!(state.get_adjusted_stations().is_none());
}

#[test]
fn get_adjusted_stations_caches_and_invalidates() {
    let state = state_with_project("Adj");
    state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#111111".to_string(),
        direction: Some(SurveyDirection::In),
        shots: vec![shot(5.0), shot(5.0)],
    });

    // Two consecutive calls return the same result (second served from cache).
    let adj1 = state.get_adjusted_stations().expect("adjusted stations");
    let adj1_again = state
        .get_adjusted_stations()
        .expect("adjusted stations (cached)");
    assert_eq!(adj1.len(), adj1_again.len());
    assert_eq!(adj1.len(), 3, "two shots → three stations");

    // A mutating command must invalidate the cache: the adjusted view reflects
    // the newly added section.
    state.execute(Command::CreateSection {
        name: Some("B".to_string()),
        surveyor: None,
        date: None,
        color: "#222222".to_string(),
        direction: Some(SurveyDirection::In),
        shots: vec![shot(5.0)],
    });
    let adj2 = state
        .get_adjusted_stations()
        .expect("adjusted stations after edit");
    assert_eq!(
        adj2.len(),
        5,
        "adjusted view must reflect the added section after cache invalidation"
    );
}

#[test]
fn distance_field_none_without_project() {
    let state = AppState::new().unwrap();
    assert!(state.distance_field(None).is_none());
}

#[test]
fn distance_field_from_origin_over_appstate() {
    // End-to-end through the atomic accessor (#34): one reconciled snapshot →
    // adjusted coords → distance_field_from, all at a single consistent version.
    let state = state_with_project("Dist");
    state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#111111".to_string(),
        direction: Some(SurveyDirection::In),
        shots: vec![shot(5.0), shot(5.0)],
    });
    let section_id = state.get_project_state().unwrap().sections[0].id;
    let origin = StationRef::new(section_id, 0);

    let field = state.distance_field(Some(&origin)).expect("distance field");
    assert_eq!(field.distances.len(), 3, "two shots → three stations");
    assert_eq!(field.station_refs.len(), 3);
    let oi = field
        .station_refs
        .iter()
        .position(|r| *r == origin)
        .expect("origin present");
    assert_eq!(field.distances[oi], Some(0.0), "origin distance is zero");
    assert!(
        field.distances.iter().all(Option::is_some),
        "single connected section: every station reachable"
    );
    assert!(field.max_reachable > 0.0);
}

#[test]
fn repair_invalidates_adjusted_cache() {
    use std::sync::atomic::Ordering::SeqCst;
    let state = state_with_project("Repair");
    state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#111111".to_string(),
        direction: Some(SurveyDirection::In),
        shots: vec![shot(5.0), shot(5.0)],
    });

    // Populate the adjusted cache.
    let _ = state.get_adjusted_stations().expect("adjusted stations");
    assert!(
        state.adjusted_cache.lock().is_some(),
        "cache should be populated after a solve"
    );
    let gen_before = state.adjusted_cache_generation.load(SeqCst);

    // Repair mutates measured geometry without necessarily bumping modified_at, so
    // it must drop the cached adjusted solve (otherwise a later read serves stale
    // geometry keyed on an unchanged version).
    let _ = state.repair_open_project_integrity();
    assert!(
        state.adjusted_cache.lock().is_none(),
        "repair must invalidate the adjusted cache"
    );
    assert!(
        state.adjusted_cache_generation.load(SeqCst) > gen_before,
        "repair must bump the invalidation generation"
    );
}

#[test]
fn unmerge_converts_orphaned_bridge_closure() {
    // Deleting the merge that was a loop's other path strands the section onto
    // the closure - which must then become a merge (same #8 principle as delete).
    let state = state_with_project("X");
    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#111111".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![shot(5.0), shot(5.0)],
        })
        .section_id
        .unwrap();
    let b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#222222".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![shot(5.0)],
        })
        .section_id
        .unwrap();
    state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 2),
        station_b: StationRef::new(b, 0),
    });
    let closure = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(a, 0),
        station_b: StationRef::new(b, 1),
        closure_strategy: ClosureStrategy::default(),
    });
    assert!(closure.success, "{:?}", closure.error);
    assert_eq!(state.get_project_state().unwrap().loop_closures.len(), 1);

    // Delete the merge that closed the loop's other path.
    let r = state.execute(Command::UnmergeStationPair {
        station_a: StationRef::new(a, 2),
        station_b: StationRef::new(b, 0),
    });
    assert!(r.success, "{:?}", r.error);

    let p = state.get_project_state().unwrap();
    assert!(
        p.loop_closures.is_empty(),
        "unmerge should convert the now-sole-bridge closure to a merge: {:?}",
        p.loop_closures
    );
    assert!(p.check_orphan_closures().is_empty());
}

#[test]
fn shot_sanitize_non_finite_neutralizes_geometry() {
    // The load-boundary sanitizer (used by the SQLite loader) replaces non-finite
    // core fields with 0.0 and non-finite optional fields with None.
    use crate::models::ShotData;
    let mut shot = crate::models::Shot::new(
        1,
        ShotData {
            azimuth_in: 1.0,
            azimuth_out: 1.0,
            length: 5.0,
            depth_in: 0.0,
            depth_out: 0.0,
            ..Default::default()
        },
    );
    shot.length = f64::NAN;
    shot.depth_out = f64::INFINITY;
    shot.pitch_in = Some(f64::NEG_INFINITY);
    shot.left = Some(2.0); // finite optional stays

    assert!(shot.sanitize_non_finite(), "must report a change");
    assert!(shot.all_finite(), "result must be finite");
    assert_eq!(shot.length, 0.0);
    assert_eq!(shot.depth_out, 0.0);
    assert_eq!(shot.pitch_in, None);
    assert_eq!(shot.left, Some(2.0), "finite optional preserved");
    assert!(
        !shot.sanitize_non_finite(),
        "already-finite shot reports no change"
    );
}

#[test]
fn coordinates_new_checked_rejects_non_finite() {
    assert!(Coordinates::new_checked(1.0, 2.0, 3.0).is_ok());
    assert!(Coordinates::new_checked(f64::NAN, 0.0, 0.0).is_err());
    assert!(Coordinates::new_checked(0.0, f64::INFINITY, 0.0).is_err());
    assert!(Coordinates::new_checked(0.0, 0.0, f64::NEG_INFINITY).is_err());
}

#[test]
fn move_section_and_segment_reject_non_finite_offset() {
    let state = state_with_project("X");
    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#111111".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![shot(5.0)],
        })
        .section_id
        .unwrap();
    let nan = Coordinates::new(f64::NAN, 0.0, 0.0);
    assert!(
        !state
            .execute(Command::MoveSection {
                section_id: a,
                offset: nan
            })
            .success
    );
    assert!(
        !state
            .execute(Command::MoveSegment {
                section_id: a,
                offset: nan
            })
            .success
    );
    // A finite move still works.
    assert!(
        state
            .execute(Command::MoveSegment {
                section_id: a,
                offset: Coordinates::new(1.0, 2.0, 0.0),
            })
            .success
    );
}

#[test]
fn merge_allowed_across_inert_closure_but_rejected_across_aligned() {
    // A merge is rejected only when the two sections are already RIGIDLY connected
    // (merges or ALIGNED closures). An inert (unaligned) closure does not constrain
    // them, so merging across one - the first real link - must be allowed; the old
    // connectivity walk followed inert closures and wrongly rejected it, forcing the
    // sole-bridge-closure-that-should-be-a-merge bug (catalog #8).
    fn setup(aligned: bool) -> (AppState, SectionId, SectionId) {
        let state = state_with_project("X");
        let a = state
            .execute(Command::CreateSection {
                name: Some("A".to_string()),
                surveyor: None,
                date: None,
                color: "#111111".to_string(),
                direction: Some(SurveyDirection::In),
                shots: vec![shot(5.0)],
            })
            .section_id
            .unwrap();
        let b = state
            .execute(Command::CreateSection {
                name: Some("B".to_string()),
                surveyor: None,
                date: None,
                color: "#222222".to_string(),
                direction: Some(SurveyDirection::In),
                shots: vec![shot(5.0)],
            })
            .section_id
            .unwrap();
        // Inject a single closure A0 <-> B1 between the otherwise-unconnected sections.
        {
            let mut g = state.project.lock();
            let p = g.as_mut().unwrap();
            let id = p.next_closure_id;
            p.loop_closures.push(LoopClosure {
                id,
                station_a: StationRef::new(a, 0).into(),
                station_b: StationRef::new(b, 1).into(),
                path_length: 10.0,
                misclosure_distance: 0.1,
                error_percentage: 1.0,
                horizontal_misclosure: 0.08,
                vertical_misclosure: 0.06,
                strategy: ClosureStrategy::default(),
                is_aligned: aligned,
            });
            p.next_closure_id += 1;
        }
        (state, a, b)
    }

    // Inert closure → the merge is the first rigid link → ALLOWED, result sound.
    let (state, a, b) = setup(false);
    let r = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 0),
    });
    assert!(
        r.success,
        "merge across an inert closure must be allowed: {:?}",
        r.error
    );
    assert!(!state
        .get_project_state()
        .unwrap()
        .has_critical_integrity_violations());

    // Aligned closure → already rigidly connected → merge REJECTED (would be a cycle).
    let (state2, a2, b2) = setup(true);
    let r2 = state2.execute(Command::MergeStations {
        station_a: StationRef::new(a2, 1),
        station_b: StationRef::new(b2, 0),
    });
    assert!(
        !r2.success,
        "merge across an aligned closure must be rejected (rigid cycle - use a loop closure)"
    );
}

#[test]
fn restore_snapshot_into_rebuilds_transient_caches() {
    // The single restore chokepoint must rebuild the serde-skipped section index,
    // so no rollback/undo/redo path can install a project that silently
    // mis-resolves a section (catalog #14/#39). The merge adjacency is not a
    // field here - it is derived on demand - so the restored project must answer
    // merge-group queries correctly with no rebuild call at all.
    let state = state_with_project("X");
    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#111111".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![shot(5.0), shot(5.0)],
        })
        .section_id
        .unwrap();
    let b = state
        .execute(Command::CreateSection {
            name: Some("B".to_string()),
            surveyor: None,
            date: None,
            color: "#222222".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![shot(5.0)],
        })
        .section_id
        .unwrap();
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(a, 2),
                station_b: StationRef::new(b, 0),
            })
            .success
    );

    let snapshot = match state.capture_project_snapshot() {
        Ok(s) => s,
        Err(e) => panic!("snapshot failed: {e:?}"),
    };

    // Deserialize into a fresh slot via the chokepoint; serde drops the caches, so
    // restore_snapshot_into must repopulate them.
    let mut slot = project_slot::ProjectSlot::default();
    assert!(AppState::restore_snapshot_into(&mut slot, &snapshot));
    let p = slot.into_project().expect("restored");
    assert!(
        p.section_index_cache_len() > 0,
        "section index cache must be rebuilt"
    );
    let merged = p.get_merged_section_ids(a);
    assert_eq!(
        merged,
        HashSet::from([a, b]),
        "a restored project must resolve the merge-connected component from its \
         own merges, with no rebuild step: {merged:?}"
    );
    // Sanity: the rebuilt index resolves both sections.
    assert!(p.get_section(a).is_some() && p.get_section(b).is_some());
}

/// A snapshot restore must return the project BIT for BIT, not to within a
/// printed decimal.
///
/// `serde_json`'s default float parser is best-effort: it writes
/// `-26.759999999999998` faithfully and reads it back as `-26.76`, one ULP away.
/// Every snapshot-pair command (Reverse section, Delete shot, Delete section,
/// every Batch import) stores a whole serialized `Project` and undoes by parsing
/// it back through [`AppState::restore_snapshot_into`], so with the default
/// parser an undo silently rewrote survey measurements in their last bit. It is
/// not cosmetic: the closure-adjusted view is a least-squares solve whose
/// elimination order turns on scale-relative pivot tests, so a one-ULP input
/// change moved a station 28 microns while the stored project compared equal
/// (issue #413, finding 25). `serde_json`'s `float_roundtrip` feature is what
/// makes the parse exact, and this test is the guard on that feature staying on.
#[test]
fn a_snapshot_restore_is_bit_exact() {
    // Doubles whose shortest round-trip decimal the best-effort parser gets
    // wrong. `pitch_out` is the one issue #413 hit; the others are on the same
    // ULP boundary and cover the plain (non-optional) measurement fields.
    let pitch = -26.759_999_999_999_998_f64;
    let length = 26.759_999_999_999_998_f64;
    let state = state_with_project("bits");
    state.execute(Command::CreateSection {
        name: Some("A".to_string()),
        surveyor: None,
        date: None,
        color: "#111111".to_string(),
        direction: Some(SurveyDirection::In),
        shots: vec![ShotData {
            azimuth_in: 12.0,
            azimuth_out: 12.0,
            length,
            depth_in: 1.0,
            depth_out: 2.0,
            pitch_out: Some(pitch),
            ..Default::default()
        }],
    });
    let before = state.get_project_state().unwrap();
    assert_eq!(
        before.sections[0].shots[0].pitch_out.map(f64::to_bits),
        Some(pitch.to_bits()),
        "fixture: the shot must hold the exact double under test"
    );

    let snapshot = AppState::serialize_project_snapshot(&before).unwrap();
    let mut slot = project_slot::ProjectSlot::new(Some(before));
    assert!(AppState::restore_snapshot_into(&mut slot, &snapshot));
    let after = slot.into_project().unwrap();

    assert_eq!(
        after.sections[0].shots[0].pitch_out.map(f64::to_bits),
        Some(pitch.to_bits()),
        "restoring a snapshot changed pitch_out by {} ULP",
        (after.sections[0].shots[0].pitch_out.unwrap().to_bits() as i64 - pitch.to_bits() as i64)
            .abs()
    );
    assert_eq!(
        after.sections[0].shots[0].length.to_bits(),
        length.to_bits(),
        "restoring a snapshot changed length in its last bits"
    );
}

/// A snapshot persisted by a build predating #431/#448 has no
/// "surface_elevation" key at all, so serde's #[serde(default)] would
/// otherwise silently restore it to None. Mirrors the `validation`
/// legacy-carry-over immediately above `restore_snapshot_into`.
#[test]
fn restore_snapshot_carries_current_surface_elevation_over_a_legacy_blob() {
    let mut project = Project::new("Legacy".to_string(), None);
    project.add_section(SectionData {
        shots: vec![shot(1.0)],
        ..Default::default()
    });
    let mut legacy_json = serde_json::to_value(&project).unwrap();
    legacy_json
        .as_object_mut()
        .unwrap()
        .remove("surface_elevation");
    let snapshot = serde_json::to_string(&legacy_json).unwrap();
    assert!(
        !snapshot.contains("\"surface_elevation\":"),
        "precondition: the fixture blob must look pre-#431/#448"
    );

    let mut slot = project_slot::ProjectSlot::new(Some(project.clone()));
    slot.as_mut().unwrap().surface_elevation = Some(123.5);
    assert!(AppState::restore_snapshot_into(&mut slot, &snapshot));

    assert_eq!(
        slot.into_project().unwrap().surface_elevation,
        Some(123.5),
        "restoring a legacy snapshot must carry the CURRENT surface_elevation forward, not drop it to None"
    );
}

/// The inverse of the legacy-carry-over above: a MODERN snapshot that
/// genuinely serializes `"surface_elevation":null` (an explicit clear, not an
/// absent key) must restore `None`, not the pre-undo project's `Some` value.
/// `surface_elevation` has no `skip_serializing_if`, so this is safe today -
/// but nothing guarded it, and the legacy-carry-over above reads similar
/// enough on a diff to invite a regression that carries the current value
/// forward unconditionally.
#[test]
fn restore_snapshot_clears_surface_elevation_when_the_modern_snapshot_says_none() {
    let mut project = Project::new("Cleared".to_string(), None);
    project.add_section(SectionData {
        shots: vec![shot(1.0)],
        ..Default::default()
    });
    project.surface_elevation = None;
    let snapshot = AppState::serialize_project_snapshot(&project).unwrap();
    assert!(
        snapshot.contains("\"surface_elevation\":null"),
        "precondition: a modern snapshot must serialize the explicit null, not omit the key: {snapshot}"
    );

    let mut slot = project_slot::ProjectSlot::new(Some(project));
    slot.as_mut().unwrap().surface_elevation = Some(123.5);
    assert!(AppState::restore_snapshot_into(&mut slot, &snapshot));

    assert_eq!(
        slot.into_project().unwrap().surface_elevation,
        None,
        "a modern snapshot's explicit null must clear surface_elevation, not carry the pre-undo value forward"
    );
}
