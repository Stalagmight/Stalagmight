use super::*;
use crate::test_fixtures::bare_shot;
use crate::{ClosureStrategy, ShotKind};

fn shot(azimuth_in: f64, azimuth_out: f64, length: f64, depth_in: f64, depth_out: f64) -> ShotData {
    bare_shot(length)
        .with_azimuths(azimuth_in, azimuth_out)
        .with_depths(depth_in, depth_out)
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

// The topic files (#576 AFK-D3): each is one contiguous run of the former single
// file, and its module doc names the lines it came from. The helpers above are
// the ones more than one of them uses; a helper only one file needs lives in that
// file. `#[cfg(test)]` on each declaration is redundant for the compiler (this
// whole module is test-only) but is what marks them as test files for the Rust
// file-size ratchet and the source scans that skip `*_tests.rs`.
#[cfg(test)]
mod adjusted_view_tests;
#[cfg(test)]
mod along_line_tests;
#[cfg(test)]
mod cleanup_cascade_tests;
#[cfg(test)]
mod closure_algorithm_tests;
#[cfg(test)]
mod closure_reference_tests;
#[cfg(test)]
mod datum_error_tests;
#[cfg(test)]
mod distance_field_tests;
#[cfg(test)]
mod double_anchor_repair_tests;
#[cfg(test)]
mod drift_heal_tests;
#[cfg(test)]
mod duplicate_datum_tests;
#[cfg(test)]
mod frozen_shot_tests;
#[cfg(test)]
mod gama_golden_tests;
#[cfg(test)]
mod least_squares_tests;
#[cfg(test)]
mod loop_closure_tests;
#[cfg(test)]
mod merge_and_offset_tests;
#[cfg(test)]
mod orphan_closure_tests;
#[cfg(test)]
mod placement_suggestion_tests;
#[cfg(test)]
mod realign_tests;
#[cfg(test)]
mod replacement_stitch_tests;
#[cfg(test)]
mod section_cleanup_tests;
#[cfg(test)]
mod shot_rewire_tests;
#[cfg(test)]
mod station_depth_tests;
#[cfg(test)]
mod stats_and_coordinates_tests;
#[cfg(test)]
mod structural_integrity_tests;
#[cfg(test)]
mod structure_tests;
#[cfg(test)]
mod transitive_closure_tests;
#[cfg(test)]
mod zero_datum_positioning_tests;
