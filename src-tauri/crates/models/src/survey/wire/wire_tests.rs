//! The wire projection on hand-built station graphs: shapes (JSON keys as
//! today's payloads carry them), junction identity, derived merge pairs,
//! closure references, datum flags, previews, slot resolution and the load
//! envelope input. The equivalence with the legacy model's own payloads is
//! in the persistence crate (`migrate/wire_equivalence_tests.rs`).

use std::collections::BTreeSet;

use serde_json::Value;

use crate::config::ClosureAdjustmentConfig;
use crate::{StationRef, ZeroDatumWire};

use super::super::ops_test_helpers::{free_line, run};
use super::super::test_helpers::{add, at, flat, gps, project, three_line_junction};
use super::super::{ClosureStrategy, OpError, Project, StartAt, ZeroDatum};

fn keys(value: &Value) -> BTreeSet<String> {
    value
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| (*n).to_string()).collect()
}

fn slot(line: u32, index: u32) -> StationRef {
    StationRef::new(line, index)
}

fn station_at(stations: &[crate::Station], at: StationRef) -> &crate::Station {
    stations.iter().find(|s| s.station_ref == at).unwrap()
}

/// The three-line junction with a comment on the junction s1 and a datum on
/// L0's start (so the survey is placed on a GPS frame).
fn placed_junction() -> (Project, [u32; 3]) {
    let (mut p, lines) = three_line_junction();
    let s1 = at(&p, lines[0], 1);
    p.station_mut(s1).unwrap().comment = Some("junction".to_string());
    let s0 = at(&p, lines[0], 0);
    p.placements.clear();
    p.push_datum(ZeroDatum::new(s0, gps(20.0, -87.0)));
    assert!(p.validate().is_empty(), "{:?}", p.validate());
    (p, lines)
}

#[test]
fn the_payload_carries_todays_keys_without_line_offsets() {
    let (p, _) = placed_junction();
    let state = serde_json::to_value(p.project_state_wire(false)).unwrap();
    assert_eq!(
        keys(&state),
        set(&["project", "stations", "stats", "frozen_shots"])
    );
    let project = &state["project"];
    assert_eq!(
        keys(project),
        set(&[
            "name",
            "location",
            "created_at",
            "modified_at",
            "lines",
            "station_merges",
            "zero_datums",
            "loop_closures",
            "file_path",
            "next_line_id",
            "imported_file_hashes",
            "imported_line_hashes",
            "next_closure_id",
            "validation",
            "surface_elevation",
            "default_azimuth_reference",
            "cave_sections",
            "next_cave_section_id",
        ])
    );
    assert_eq!(
        keys(&project["lines"][0]),
        set(&[
            "id",
            "name",
            "color",
            "direction",
            "quality",
            "shots",
            "next_shot_id",
            "reverse_parity",
            "gauge_shot_uuid",
            "gauge_neighbour_uuid",
            "azimuth_reference",
        ])
    );
    assert_eq!(project["lines"][0]["reverse_parity"], Value::Bool(false));
    assert_eq!(project["lines"][0]["gauge_shot_uuid"], Value::Null);
    assert!(project["lines"][0]["shots"][0]["uuid"].is_string());
    // A plain slot: no closures_with, no comment (both omitted when empty).
    let plain = state["stations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["station_ref"] == serde_json::json!({"line_id": 0, "station_index": 2}))
        .unwrap();
    assert_eq!(
        keys(plain),
        set(&[
            "station_ref",
            "coordinates",
            "is_zero_datum",
            "gps_coordinates",
            "merged_with",
            "id",
            "name",
        ])
    );
    assert_eq!(
        keys(&project["zero_datums"][0]),
        set(&["station_ref", "gps", "role", "is_entrance"])
    );
}

#[test]
fn a_junction_shares_its_identity_and_lists_its_other_slots() {
    let (p, [l0, l1, l2]) = placed_junction();
    let raw = p.raw_positions();
    let stations = p.stations_wire(&raw);
    assert_eq!(stations.len(), 4 + 3 + 2, "one entry per slot");
    let on_l0 = station_at(&stations, slot(l0, 1));
    let on_l1 = station_at(&stations, slot(l1, 0));
    assert_eq!(on_l0.id, on_l1.id);
    assert_eq!(on_l0.name, on_l1.name);
    assert_eq!(on_l0.comment.as_deref(), Some("junction"));
    assert_eq!(on_l1.comment.as_deref(), Some("junction"));
    assert_eq!(on_l0.merged_with, [slot(l1, 0)]);
    assert_eq!(on_l1.merged_with, [slot(l0, 1)]);
    let end = station_at(&stations, slot(l0, 3));
    assert_eq!(end.merged_with, [slot(l2, 1)]);
    assert!(station_at(&stations, slot(l0, 2)).merged_with.is_empty());
    // Every slot of a station sits at one point.
    assert_eq!(on_l0.coordinates.x, on_l1.coordinates.x);
    assert_eq!(on_l0.coordinates.y, on_l1.coordinates.y);
    // The wire list is the raw layout's slot list.
    for (wire, raw_slot) in stations.iter().zip(raw.slots(&p)) {
        assert_eq!(wire.station_ref, raw_slot.slot);
        assert_eq!(
            [wire.coordinates.x, wire.coordinates.y, wire.coordinates.z],
            [raw_slot.x, raw_slot.y, raw_slot.z]
        );
    }
}

/// A merged station's archived records (each with the lines whose own
/// station it was) ride every slot of it, for the station details (RS-W
/// replaces them with a records DTO); a station without any sends none
/// (H-frontend-wire-5). Every slot shows the kept station's name (RS 4.1):
/// the moving line's slot reads `0` too, and the moving line's own `0`
/// takes the letter (rule L). The Station ID is the junction's one id on
/// both. Injection: send the slot's own record name and the moving line's
/// slot reads `2`.
#[test]
fn a_merged_stations_records_ride_every_slot() {
    let mut p = project();
    let a = free_line(&mut p, [0.0, 0.0], run(90.0, 2, 10.0));
    let b = free_line(&mut p, [100.0, 50.0], run(0.0, 2, 5.0));
    let (a0, a2, b0) = (at(&p, a, 0), at(&p, a, 2), at(&p, b, 0));
    let a2_name = p.stations[&a2].name.clone();
    let merged = p.merge_keeping_stationary(a2, b0).unwrap();
    assert_eq!((merged.value.kept, merged.value.archived), (b0, a2));
    assert_eq!(merged.identities.lettered.len(), 1);
    assert_eq!(merged.identities.lettered[0].station, a0);
    let stations = p.stations_wire(&p.raw_positions());
    assert_eq!(station_at(&stations, slot(a, 2)).name, "0");
    assert_eq!(station_at(&stations, slot(b, 0)).name, "0");
    assert_eq!(station_at(&stations, slot(a, 0)).name, "0a");
    assert_eq!(station_at(&stations, slot(a, 2)).id, b0);
    assert_eq!(station_at(&stations, slot(b, 0)).id, b0);
    let expected = Some(vec![crate::StationAliasWire {
        id: a2,
        name: Some(a2_name),
        line: Some(a),
    }]);
    assert_eq!(station_at(&stations, slot(a, 2)).aliases, expected);
    assert_eq!(station_at(&stations, slot(b, 0)).aliases, expected);
    assert_eq!(station_at(&stations, slot(a, 1)).aliases, None);
    let json = serde_json::to_value(station_at(&stations, slot(b, 0))).unwrap();
    assert_eq!(json["aliases"][0]["id"], serde_json::json!(a2));
}

#[test]
fn station_merges_pair_each_junctions_first_slot_with_its_others() {
    let (mut p, [l0, l1, l2]) = placed_junction();
    let s1 = at(&p, l0, 1);
    let l3 = add(&mut p, StartAt::Station(s1), vec![flat(180.0, 4.0)]);
    let merges = p.station_merges_wire();
    let pairs: Vec<(StationRef, StationRef)> =
        merges.iter().map(|m| (m.station_a, m.station_b)).collect();
    assert_eq!(
        pairs,
        [
            (slot(l0, 1), slot(l1, 0)),
            (slot(l0, 1), slot(l3, 0)),
            (slot(l0, 3), slot(l2, 1)),
        ]
    );
    assert!(merges.iter().all(|m| m.merged_at == p.created_at));
    // A hub's slots list each other transitively, like a merge group.
    let stations = p.stations_wire(&p.raw_positions());
    assert_eq!(
        station_at(&stations, slot(l1, 0)).merged_with,
        [slot(l0, 1), slot(l3, 0)]
    );
}

#[test]
fn closure_references_follow_todays_rule_with_a_station_as_the_merge_group() {
    let (mut p, [l0, l1, l2]) = placed_junction();
    let b = at(&p, l1, 2);
    let s3 = at(&p, l0, 3);
    let id = p.push_closure(b, Some(l1), s3, Some(l2), ClosureStrategy::Batch);
    let stations = p.stations_wire(&p.raw_positions());
    let refs = |at: StationRef| {
        let mut out: Vec<(u64, StationRef, Option<StationRef>, bool)> = station_at(&stations, at)
            .closures()
            .iter()
            .map(|r| (r.closure_id, r.station_ref, r.via_station, r.is_transitive))
            .collect();
        out.sort();
        out
    };
    // The near end lists the far end's own slot (its `via` line) directly
    // and the far station's other slot transitively.
    assert_eq!(
        refs(slot(l1, 2)),
        [
            (id, slot(l0, 3), Some(slot(l2, 1)), true),
            (id, slot(l2, 1), None, false),
        ]
    );
    // The far end's own slot is direct; its other slot reaches it "via" it.
    assert_eq!(refs(slot(l2, 1)), [(id, slot(l1, 2), None, false)]);
    assert_eq!(
        refs(slot(l0, 3)),
        [(id, slot(l1, 2), Some(slot(l2, 1)), true)]
    );
    assert!(station_at(&stations, slot(l0, 0)).closures_with.is_none());

    let wire = p.project_wire();
    let closure = &wire.loop_closures[0];
    assert_eq!(
        (closure.station_a, closure.station_b),
        (slot(l1, 2), slot(l2, 1))
    );
    let metrics = p
        .closure_metrics_via((b, Some(l1)), (s3, Some(l2)))
        .unwrap();
    assert_eq!(closure.misclosure_distance, metrics.misclosure_distance);
    assert_eq!(closure.path_length, metrics.path_length);
    assert!(!closure.is_aligned);
}

#[test]
fn closure_chains_let_two_partners_of_one_slot_see_each_other() {
    let (mut p, [l0, l1, _]) = placed_junction();
    let x = at(&p, l0, 2);
    let y = at(&p, l1, 2);
    let s0 = at(&p, l0, 0);
    let first = p.push_closure(x, Some(l0), y, Some(l1), ClosureStrategy::Batch);
    let second = p.push_closure(x, Some(l0), s0, Some(l0), ClosureStrategy::Batch);
    let stations = p.stations_wire(&p.raw_positions());
    let chained: Vec<_> = station_at(&stations, slot(l1, 2))
        .closures()
        .iter()
        .filter(|r| r.via_station == Some(slot(l0, 2)))
        .map(|r| (r.closure_id, r.station_ref))
        .collect();
    assert_eq!(chained, [(second, slot(l0, 0))]);
    let back: Vec<_> = station_at(&stations, slot(l0, 0))
        .closures()
        .iter()
        .filter(|r| r.via_station == Some(slot(l0, 2)))
        .map(|r| (r.closure_id, r.station_ref))
        .collect();
    assert_eq!(back, [(first, slot(l1, 2))]);
}

#[test]
fn a_tie_is_an_aligned_batch_closure_on_the_wire() {
    let (mut p, [l0, l1, _]) = placed_junction();
    let x = at(&p, l0, 2);
    let y = at(&p, l1, 2);
    p.push_closure(x, None, y, None, ClosureStrategy::Tie);
    let wire = p.project_wire();
    assert_eq!(
        wire.loop_closures[0].strategy,
        crate::ClosureStrategy::Batch
    );
    assert!(wire.loop_closures[0].is_aligned);
}

#[test]
fn every_slot_of_a_datum_station_shows_the_datum_its_dialog_edits() {
    let (mut p, [l0, l1, _]) = placed_junction();
    let s1 = at(&p, l0, 1);
    let mut on_l1 = ZeroDatum::new(s1, gps(20.001, -87.0));
    on_l1.via = Some(l1);
    let fix = p.push_datum(on_l1);
    let stations = p.stations_wire(&p.raw_positions());
    for at in [slot(l0, 1), slot(l1, 0)] {
        let station = station_at(&stations, at);
        assert!(station.is_zero_datum, "{at}");
        assert_eq!(
            station.gps_coordinates.unwrap().latitude,
            p.datums[fix].gps.latitude
        );
    }
    let datums: Vec<ZeroDatumWire> = p.zero_datums_wire();
    assert_eq!(datums[fix].station_ref, slot(l1, 0), "on its via slot");
    assert_eq!(datums[fix].role, crate::DatumRole::GpsFix);
    assert!(datums[fix].gap.is_some(), "a fix carries its gap");
    assert!(datums[0].gap.is_none(), "a placement datum does not");
}

#[test]
fn the_computed_view_list_has_the_same_slots_and_relations() {
    let (mut p, [l0, l1, _]) = placed_junction();
    let x = at(&p, l0, 2);
    let y = at(&p, l1, 2);
    p.push_closure(x, Some(l0), y, Some(l1), ClosureStrategy::Batch);
    let raw = p.stations_wire(&p.raw_positions());
    let adjusted_positions = p.adjusted_positions(&ClosureAdjustmentConfig::default());
    let adjusted = p.adjusted_stations_wire(&adjusted_positions);
    assert_eq!(raw.len(), adjusted.len());
    for ((r, a), s) in raw.iter().zip(&adjusted).zip(adjusted_positions.slots(&p)) {
        assert_eq!(r.station_ref, a.station_ref);
        assert_eq!(r.merged_with, a.merged_with);
        assert_eq!(r.closures().len(), a.closures().len());
        assert_eq!(
            [a.coordinates.x, a.coordinates.y, a.coordinates.z],
            [s.x, s.y, s.z]
        );
    }
}

#[test]
fn slots_resolve_at_the_command_boundary_and_refuse_out_of_range() {
    let (p, [l0, l1, _]) = placed_junction();
    let target = p.resolve_slot(slot(l1, 0)).unwrap();
    assert_eq!(target.station, at(&p, l0, 1));
    assert_eq!(target.line, l1);
    assert!(matches!(
        p.resolve_slot(slot(l0, 4)),
        Err(OpError::UnknownSlot(_))
    ));
    assert!(matches!(
        p.resolve_slot(slot(99, 0)),
        Err(OpError::UnknownSlot(_))
    ));
    assert!(p.resolve_slot_pair(slot(l0, 0), slot(l0, 9)).is_err());
}

#[test]
fn deleting_a_junction_shot_reports_the_slot_leaving_the_junction() {
    let (p, [l0, l1, _]) = placed_junction();
    // Shot 0 of L0 removes station 0 (the datum station, on L0 only).
    let first = p.line(l0).unwrap().shots[0].id;
    let preview = p.preview_delete_shot(l0, first).unwrap();
    let impact = p.delete_shot_impact(&preview);
    assert_eq!(impact.removed_station, slot(l0, 0));
    assert!(impact.removed_merges.is_empty());
    // Shot 0 of L1 removes L1's start, the junction s1: L1 leaves it.
    let first = p.line(l1).unwrap().shots[0].id;
    let preview = p.preview_delete_shot(l1, first).unwrap();
    let impact = p.delete_shot_impact(&preview);
    assert_eq!(impact.removed_station, slot(l1, 0));
    let pairs: Vec<_> = impact
        .removed_merges
        .iter()
        .map(|m| (m.station_a, m.station_b))
        .collect();
    assert_eq!(pairs, [(slot(l1, 0), slot(l0, 1))]);
}

#[test]
fn line_relationships_count_what_the_delete_takes() {
    let (mut p, [l0, l1, l2]) = placed_junction();
    let b = at(&p, l1, 2);
    let s3 = at(&p, l0, 3);
    let x = at(&p, l0, 2);
    p.push_closure(b, Some(l1), s3, Some(l2), ClosureStrategy::Batch);
    p.push_closure(x, Some(l0), b, Some(l1), ClosureStrategy::Batch);
    p.line_mut(l1)
        .unwrap()
        .hand_moves
        .insert(b, [0.0, 0.0, 0.5]);
    let rel = p.line_relationships(l1);
    assert_eq!(rel.merges.len(), 1, "L1's start is a junction with L0");
    assert_eq!(rel.closures.len(), 2, "both closures end at b, only on L1");
    assert_eq!(rel.station_offset_count, 1);
    assert_eq!(rel.zero_datum_count, 0);
    assert!(!rel.has_line_offset);
    let rel = p.line_relationships(l2);
    assert_eq!(rel.closures.len(), 0, "s3 stays on L0");
}

#[test]
fn the_envelope_input_carries_hand_moves_and_comments_per_slot() {
    let (mut p, [l0, l1, _]) = placed_junction();
    let b = at(&p, l1, 2);
    p.line_mut(l1)
        .unwrap()
        .hand_moves
        .insert(b, [0.25, -0.5, 0.75]);
    let input = p.load_envelope_input(false);
    assert_eq!(input.station_offsets, [(slot(l1, 2), (0.25, -0.5, 0.75))]);
    let commented: Vec<StationRef> = input
        .station_comments_map
        .iter()
        .map(|(at, _)| *at)
        .collect();
    assert_eq!(commented, [slot(l0, 1), slot(l1, 0)]);
    assert_eq!(input.state.stations.len(), 9);
    let hand_z = station_at(&input.state.stations, slot(l1, 2)).coordinates.z;
    assert_eq!(hand_z, p.raw_positions().z(b).unwrap() + 0.75);
}

#[test]
fn an_empty_project_projects_to_an_empty_payload() {
    let p = project();
    let state = p.project_state_wire(false);
    assert!(state.stations.is_empty());
    assert!(state.project.lines.is_empty());
    assert_eq!(state.project.next_line_id, 0);
    assert_eq!(state.project.next_closure_id, 1);
    assert_eq!(state.stats.station_count, 0);
}

/// Scale: a 3,000-line, 75,000-shot survey (a junction at every line start,
/// closures and fixes sprinkled in). Prints the wall time of the payload and
/// its JSON; run with `--release --ignored --nocapture`.
#[test]
#[ignore = "timing; run by hand"]
fn the_projection_scales_to_a_large_survey() {
    use std::time::Instant;
    let mut p = project();
    let first = add(
        &mut p,
        StartAt::New,
        (0..25).map(|i| flat(f64::from(i % 360), 3.0)).collect(),
    );
    let s0 = at(&p, first, 0);
    p.set_placement(s0, 0.0, 0.0).unwrap();
    let mut lines = vec![first];
    let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut next = |below: usize| {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) as usize % below
    };
    for n in 1..3_000_u32 {
        let host = lines[next(lines.len())];
        let from = at(&p, host, next(26));
        let line = add(
            &mut p,
            StartAt::Station(from),
            (0..25)
                .map(|i| flat(f64::from((n * 17 + i) % 360), 2.5))
                .collect(),
        );
        lines.push(line);
    }
    for n in 0..200_usize {
        let (a, b) = (lines[n * 11 % 3_000], lines[n * 29 % 3_000]);
        let (sa, sb) = (at(&p, a, 25), at(&p, b, 20));
        if sa != sb {
            p.push_closure(sa, Some(a), sb, Some(b), ClosureStrategy::Batch);
        }
    }
    let started = Instant::now();
    let raw = p.raw_positions();
    let t_raw = started.elapsed();
    let state = p.project_state_wire_with(&raw, false);
    let t_state = started.elapsed();
    let bytes = serde_json::to_vec(&state).unwrap();
    let t_json = started.elapsed();
    let input = p.load_envelope_input(false);
    let t_envelope = started.elapsed().saturating_sub(t_json);
    assert_eq!(state.stations.len(), 3_000 * 26);
    assert_eq!(input.state.stations.len(), 3_000 * 26);
    let merged: usize = state.stations.iter().map(|s| s.merged_with.len()).sum();
    let closures: usize = state.stations.iter().map(|s| s.closures().len()).sum();
    let shots_json = serde_json::to_vec(&state.project.lines).unwrap().len();
    let report = format!(
        "merged {merged} closures {closures} lines json {shots_json} slots {} raw {:?} state {:?} json {:?} ({} MB) envelope input {:?}",
        state.stations.len(),
        t_raw,
        t_state.saturating_sub(t_raw),
        t_json.saturating_sub(t_state),
        bytes.len() / 1_000_000,
        t_envelope
    );
    std::fs::write(
        std::env::temp_dir().join("stalagmight-wire-scale.txt"),
        report.as_bytes(),
    )
    .unwrap();
}
