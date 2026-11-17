//! Matching the datums before a command with the datums after it (#778):
//! the order-preserving alignment itself, and the rule that a placement
//! datum as alike to a dropped GPS fix still gets placed. Synthetic surveys
//! only.

use super::datum_placement::align;
use crate::{
    gps_to_local, local_to_gps, GpsCoordinates, Project, SectionData, ShotData, StationRef,
};

/// `align` over a table of ranks: `ranks[i][j]` for earlier datum `i` and
/// current datum `j`.
fn aligned(ranks: &[&[Option<usize>]]) -> Vec<Option<usize>> {
    let after = ranks.first().map_or(0, |row| row.len());
    align(ranks.len(), after, |i, j| ranks[i][j])
}

#[test]
fn untouched_datums_pair_one_to_one() {
    let s = Some(0);
    assert_eq!(
        aligned(&[&[s, None, None], &[None, s, None], &[None, None, s]]),
        vec![Some(0), Some(1), Some(2)]
    );
}

#[test]
fn a_removed_datum_is_skipped_and_an_appended_one_is_new() {
    // [A, B, C] -> [A, C, N]: B removed, N appended.
    let s = Some(0);
    assert_eq!(
        aligned(&[&[s, None, None], &[None, None, None], &[None, s, None]]),
        vec![Some(0), Some(2), None]
    );
}

#[test]
fn twins_pair_in_list_order_even_when_a_crossing_pair_shares_a_key() {
    // Twins F (fix) then P (placement), one GPS and one stored value. A
    // combine gives F P's key and moves it (P-F': key and GPS, rank 4) and
    // re-keys P where it stands (P-P': GPS and position, rank 2). F-F' and
    // F-P' share only the GPS (rank 6). The pairs cannot cross, and F-F' +
    // P-P' beats P-F' alone by its rank 2 pair.
    assert_eq!(
        aligned(&[&[Some(6), Some(6)], &[Some(4), Some(2)]]),
        vec![Some(0), Some(1)]
    );
}

#[test]
fn one_stronger_pair_outweighs_any_number_of_weaker_ones() {
    // A is untouched as Y (rank 0); A-X and B-Y share only a GPS (rank 7).
    assert_eq!(
        aligned(&[&[Some(7), Some(0)], &[None, Some(7)]]),
        vec![None, Some(0)]
    );
}

#[test]
fn on_a_tie_the_later_current_datum_is_the_new_one() {
    // [B, C] -> [C', B']: B-B' and C-C' are both untouched pairs but cross,
    // so only one can stand. New datums are appended, so B' (last) is new.
    let s = Some(0);
    assert_eq!(aligned(&[&[None, s], &[s, None]]), vec![Some(1), None]);
}

#[test]
fn on_a_tie_the_earlier_datum_is_paired() {
    // One current datum as alike to two earlier ones: the first pairs.
    assert_eq!(aligned(&[&[Some(6)], &[Some(6)]]), vec![Some(0)]);
}

#[test]
fn nothing_to_align_is_no_pairs() {
    assert_eq!(aligned(&[]), Vec::<Option<usize>>::new());
    assert_eq!(align(2, 0, |_, _| Some(0)), Vec::<Option<usize>>::new());
    assert_eq!(align(0, 2, |_, _| Some(0)), vec![None, None]);
}

const ORIGIN: (f64, f64) = (20.5, -87.25);

fn origin() -> GpsCoordinates {
    GpsCoordinates::new(ORIGIN.0, ORIGIN.1, 0.0).unwrap()
}

fn gps_at(east: f64, north: f64) -> GpsCoordinates {
    let (lat, lon) = local_to_gps(east, north, &origin()).unwrap();
    GpsCoordinates::new(lat, lon, 0.0).unwrap()
}

fn north_section(project: &mut Project, shots: usize) -> crate::SectionId {
    let shots = (0..shots)
        .map(|_| ShotData {
            azimuth_in: 0.0,
            azimuth_out: 0.0,
            length: 20.0,
            ..Default::default()
        })
        .collect();
    let id = project
        .add_section(SectionData {
            shots,
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    id
}

#[test]
fn a_placement_datum_as_alike_to_a_dropped_fix_is_placed() {
    // E is the frame. S is placed at S:0 by GPS `twin`, and S:2 carries a fix
    // with the very same GPS (40 m off). Then one of the two datums goes and
    // the other ends up on S:1: every earlier datum is as alike to it (the
    // GPS only), so the alignment cannot say which survived. It pairs the
    // placement datum, but the dropped one was a fix, so S:1's datum is
    // placed all the same: on its GPS, which is right whichever it was.
    let mut project = Project::new("Twins".to_string(), None);
    let e = north_section(&mut project, 1);
    project
        .set_zero_datum(StationRef::new(e, 0), origin())
        .unwrap();
    let s = north_section(&mut project, 2);
    let twin = gps_at(100.0, 0.0);
    project.set_zero_datum(StationRef::new(s, 0), twin).unwrap();
    project.set_zero_datum(StationRef::new(s, 2), twin).unwrap();
    let before = project.datum_layout();

    let s1 = project.key_for_ref(StationRef::new(s, 1)).unwrap();
    project.zero_datums.remove(1);
    project.zero_datums[1].station_ref = s1;
    assert!(project.settle_datum_layout(&before));

    let (x, y) = project
        .compute_station_raw_xy(StationRef::new(s, 1))
        .unwrap();
    let (gx, gy) = gps_to_local(&twin, &origin());
    assert!(
        (x - gx).abs() < 1e-6 && (y - gy).abs() < 1e-6,
        "S:1 at ({x}, {y}), its GPS at ({gx}, {gy})"
    );
}
