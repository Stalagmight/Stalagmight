//! Add .stalx / Merge Projects (plan 5.8, 4.4) on the station graph: ids,
//! names, uuids, Sections, the rigid `place_by_datum` move, copied
//! placements and recomputed corrections.

use crate::{gps_to_local, AzimuthReference, Section, StationRef};

use super::ops_test_helpers::{
    assert_still, assert_valid, free_line, gps_at, gps_from, near, ok, origin, run, xy, xy_all,
};
use super::test_helpers::{at, project};
use super::{ClosureStrategy, LineId, OpError, Project};

const TOL: f64 = 1e-9;

fn with_section(p: &mut Project, line: LineId, name: &str) {
    let id = p.mint_section_id().unwrap();
    p.sections.push(Section {
        id,
        name: name.to_string(),
        color: "#4ecdc4".to_string(),
    });
    p.line_mut(line).unwrap().section_id = Some(id);
}

#[test]
fn an_add_offsets_ids_re_mints_closures_and_keeps_names_unique() {
    let mut target = project();
    let t = free_line(&mut target, [0.0, 0.0], run(90.0, 3, 10.0));
    with_section(&mut target, t, "Upper level");
    let (t0, t3) = (at(&target, t, 0), at(&target, t, 3));
    ok(target.create_closure(t0, None, t3, None, ClosureStrategy::Batch, None, &[]));
    target.settings.default_azimuth_reference = Some(AzimuthReference {
        correction: 1.0,
        ..AzimuthReference::default()
    });

    let mut source = project();
    source.location = Some("North valley".to_string());
    let s = free_line(&mut source, [7.0, 8.0], run(0.0, 3, 5.0));
    with_section(&mut source, s, "Upper level");
    let (s0, s3) = (at(&source, s, 0), at(&source, s, 3));
    let source_closure =
        ok(source.create_closure(s0, Some(s), s3, None, ClosureStrategy::Move, None, &[]))
            .value
            .closure;

    let base = target.counters.next_station_id;
    let target_before = xy_all(&target);
    let out = ok(target.append_project(&source));
    assert_valid(&target);
    let summary = out.value;
    let copied = summary.new_lines()[0];
    assert_eq!(summary.line_map, vec![(s, copied)]);
    assert_eq!(summary.station_offset, base);
    assert_eq!(summary.stations, vec![base, base + 1, base + 2, base + 3]);
    assert_eq!(target.counters.next_station_id, base + 4);
    assert_eq!(at(&target, copied, 0), s0 + base);

    // The closure is re-minted, its hints follow the line map.
    assert_eq!(summary.closure_map, vec![(source_closure.id, 2)]);
    let closure = target.closures[1];
    assert_eq!((closure.a, closure.b), (s0 + base, s3 + base));
    assert_eq!((closure.a_via, closure.b_via), (Some(copied), None));
    assert_eq!(closure.strategy, ClosureStrategy::Move);

    // Sections and line names made unique; uuids the target held re-minted.
    assert_eq!(target.sections[1].name, "Upper level (2)");
    assert_eq!(
        target.line(copied).unwrap().section_id,
        Some(summary.section_map[0].1)
    );
    assert_eq!(target.line(copied).unwrap().name.as_deref(), Some("L0 (2)"));
    assert_eq!(summary.renamed_lines, vec![copied]);
    assert_eq!(summary.shot_uuid_renames.len(), 3, "shot-0-* is taken");
    for rename in &summary.shot_uuid_renames {
        assert_eq!(rename.line, copied);
        assert!(!target
            .line(t)
            .unwrap()
            .shots
            .iter()
            .any(|x| x.uuid == rename.target));
    }
    assert!(target.duplicate_shot_uuids().is_empty());

    // The copy keeps its own (resolved) north reference.
    assert_eq!(
        target.line(copied).unwrap().azimuth_reference,
        Some(AzimuthReference::default())
    );
    assert_eq!(target.location.as_deref(), Some("North valley"));

    // No datum on either side: nothing moves, the copy keeps its placement.
    assert_eq!(summary.translation, [0.0, 0.0]);
    assert_still(&target_before, &xy_all(&target), 0.0);
    near(xy(&target, s0 + base), [7.0, 8.0], TOL, "copied placement");
}

#[test]
fn a_georeferenced_source_lands_on_its_gps_in_this_frame() {
    let mut target = project();
    let t = free_line(&mut target, [0.0, 0.0], run(90.0, 2, 10.0));
    ok(target.set_datum_at(StationRef::new(t, 0), origin(), None));
    let before = xy_all(&target);

    let mut source = project();
    let s = free_line(&mut source, [0.0, 0.0], run(0.0, 2, 10.0));
    let g = gps_at(1000.0, -250.0);
    ok(source.set_datum_at(StationRef::new(s, 0), g, None));

    let out = ok(target.append_project(&source));
    assert_valid(&target);
    let copied = out.value.new_lines()[0];
    let (ex, ey) = gps_to_local(&g, &origin());
    near(
        xy(&target, at(&target, copied, 0)),
        [ex, ey],
        1e-9,
        "on its GPS",
    );
    near(out.value.translation, [ex, ey], 1e-9, "translation");
    assert_eq!(out.value.datums, 1..2);
    assert_eq!(target.datums[1].correction, None);
    assert_eq!(target.frame_datum(), Some(0), "the target keeps its frame");
    assert_still(&before, &xy_all(&target), 0.0);
}

#[test]
fn free_pieces_and_second_entrances_of_the_source_move_with_it() {
    // R1 scenario: a source with its frame D_s, a free piece Q at
    // source-local (100, 0) and a second placed piece D2 2 km north of D_s,
    // added to a target whose frame is 5 km away. Today the whole copy moves
    // rigidly: Q lands next to its cave, and D2 keeps its place relative to
    // D_s even though the target's tangent plane disagrees.
    let mut target = project();
    let t = free_line(&mut target, [0.0, 0.0], run(90.0, 1, 10.0));
    ok(target.set_datum_at(StationRef::new(t, 0), origin(), None));

    let ds_gps = gps_at(5000.0, 0.0);
    let mut source = project();
    let s = free_line(&mut source, [0.0, 0.0], run(0.0, 2, 10.0));
    ok(source.set_datum_at(StationRef::new(s, 0), ds_gps, None));
    free_line(&mut source, [100.0, 0.0], run(90.0, 1, 5.0));
    let d2 = free_line(&mut source, [0.0, 999.0], run(0.0, 1, 5.0));
    ok(source.set_datum_at(StationRef::new(d2, 0), gps_from(&ds_gps, 0.0, 2000.0), None));
    let source_xy = xy_all(&source);

    let out = ok(target.append_project(&source));
    assert_valid(&target);
    let translation = out.value.translation;
    let (tx, ty) = gps_to_local(&ds_gps, &origin());
    near(translation, [tx, ty], 1e-9, "the source frame onto its GPS");
    let base = out.value.station_offset;
    for (station, [x, y]) in source_xy {
        near(
            xy(&target, station + base),
            [x + translation[0], y + translation[1]],
            1e-6,
            &format!("copied station {station}"),
        );
    }
    let q0 = at(&target, out.value.new_lines()[1], 0);
    assert!(target.placements.iter().any(|p| p.station == q0));
    let d2_index = out.value.datums.end - 1;
    assert!(
        target.datums[d2_index].correction.is_some(),
        "the tangent planes disagree at 2 km: a correction holds D2 in place"
    );
}

#[test]
fn a_target_without_a_frame_takes_the_source_frame_and_keeps_its_free_pieces() {
    let mut target = project();
    free_line(&mut target, [50.0, 50.0], run(90.0, 2, 10.0));
    let before = xy_all(&target);
    let mut source = project();
    let s = free_line(&mut source, [0.0, 0.0], run(0.0, 2, 10.0));
    ok(source.set_datum_at(StationRef::new(s, 1), gps_at(10.0, 10.0), None));
    let out = ok(target.append_project(&source));
    assert_valid(&target);
    let copied = out.value.new_lines()[0];
    assert_eq!(target.frame_datum(), Some(0));
    assert_eq!(xy(&target, at(&target, copied, 1)), [0.0, 0.0]);
    assert_eq!(target.datums[0].correction, None);
    assert_still(&before, &xy_all(&target), 0.0);
}

#[test]
fn an_invalid_source_is_refused_and_nothing_changes() {
    let mut target = project();
    free_line(&mut target, [0.0, 0.0], run(90.0, 2, 10.0));
    let before = target.clone();
    let mut source = project();
    free_line(&mut source, [0.0, 0.0], run(0.0, 2, 10.0));
    source.placements.clear();
    assert!(matches!(
        target.append_project(&source),
        Err(OpError::SourceInvalid(_))
    ));
    assert_eq!(target, before);
}

#[test]
fn the_entrance_elevation_first_one_wins_and_a_disagreement_is_reported() {
    let mut target = project();
    free_line(&mut target, [0.0, 0.0], run(90.0, 1, 10.0));
    let mut source = project();
    free_line(&mut source, [0.0, 0.0], run(0.0, 1, 10.0));
    source.settings.surface_elevation = Some(12.0);
    let out = ok(target.append_project(&source));
    assert_eq!(out.value.elevation_conflict, None);
    assert_eq!(target.settings.surface_elevation, Some(12.0));
    source.settings.surface_elevation = Some(15.0);
    let out = ok(target.append_project(&source));
    assert_eq!(out.value.elevation_conflict, Some((15.0, 12.0)));
    assert_eq!(target.settings.surface_elevation, Some(12.0));
    assert_valid(&target);
}
