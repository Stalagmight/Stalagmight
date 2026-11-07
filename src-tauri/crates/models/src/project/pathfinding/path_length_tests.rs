//! [#576 FZ-DET-5] `calculate_path_length_between`'s fallback, when no route
//! joins the two stations, sums station A's reachable sections in project order.

use chrono::Utc;

use crate::test_fixtures::bare_shot;
use crate::{Project, SectionData, ShotData, StationMerge, StationRef};

/// A level shot due north of the given length.
fn flat_shot(length: f64) -> ShotData {
    bare_shot(length)
        .with_azimuths(0.0, 0.0)
        .with_depths(0.0, 0.0)
}

/// [#576 FZ-DET-5] With no route between the stations the path length falls
/// back to the sum of station A's reachable sections. That sum ran in the
/// HashSet's order, and float addition is not associative: 0.1 + 0.2 + 0.3 is
/// 0.6000000000000001 in this order and 0.6 in two of the other five, so the
/// same closure stored a different path length (and error percentage) from one
/// run to the next. Every set below is freshly keyed, so on the old code some
/// of the 64 calls came out in another order.
#[test]
fn path_length_fallback_sums_in_project_order() {
    let mut project = Project::new("FallbackOrder".to_string(), None);
    let mut ids = Vec::new();
    for length in [0.1, 0.2, 0.3] {
        ids.push(
            project
                .add_section(SectionData {
                    shots: vec![flat_shot(length)],
                    ..Default::default()
                })
                .id,
        );
    }
    let island = project
        .add_section(SectionData {
            shots: vec![flat_shot(1.0)],
            ..Default::default()
        })
        .id;
    for pair in ids.windows(2) {
        project.station_merges.push(StationMerge {
            station_a: StationRef::new(pair[0], 1).into(),
            station_b: StationRef::new(pair[1], 0).into(),
            merged_at: Utc::now(),
        });
    }

    let project_order: f64 = [0.1, 0.2, 0.3].iter().sum();
    for _ in 0..64 {
        let path_len = project
            .calculate_path_length_between(StationRef::new(ids[0], 0), StationRef::new(island, 1));
        assert_eq!(
            path_len.to_bits(),
            project_order.to_bits(),
            "the fallback summed out of project order: {path_len} vs {project_order}"
        );
    }
}
