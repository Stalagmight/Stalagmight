//! Replacement-merge guards (Project::replacement_merge_allowed), with the IPC validation-field and
//! Ariane elevation checks that sat beside them.
//!
//! Lines 14616-14997 of the former single-file `project/tests.rs` (#576 AFK-D3), moved verbatim;
//! shared helpers live in the module root (`tests.rs`).

use super::*;

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
