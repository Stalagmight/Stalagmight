//! A merge group survives a save and a reopen, transitively (#576 E13-D2).
//!
//! Replaces two `#[ignore]`d tests that read `fractal_plant_iter2.stalx` and
//! `fractal_plant_iter4.stalx` from `e2e/fixtures/`, files the repo does not
//! carry, so neither could run on any clone. The property they checked is
//! rebuilt here on synthetic data: a station where a branch leaves its parent
//! and a second branch leaves the first reports both other stations as
//! `merged_with` - including the pair joined only through it - after the
//! project went through `Database` and back.

use tempfile::TempDir;

use crate::models::{Project, SectionData, ShotData, StationRef};
use crate::persistence::Database;

/// Three sections saved and reopened: section 1 starts at section 0's station
/// `branch_at`, and section 2 starts at section 1's first station - the shape
/// of a fractal plant's first fork. Returns the reopened project and the three
/// stations of the merge group, parent first.
fn reopened_fork(branch_at: u32) -> (Project, [StationRef; 3]) {
    let shot = |azimuth: f64| ShotData {
        length: 4.0,
        azimuth_in: azimuth,
        azimuth_out: (azimuth + 180.0) % 360.0,
        depth_in: 2.0,
        depth_out: 2.0,
        ..Default::default()
    };
    let mut project = Project::new("CaveA".to_string(), None);
    let mut add = |azimuth: f64, shots: u32| {
        project
            .add_section(SectionData {
                shots: (0..shots).map(|_| shot(azimuth)).collect(),
                ..Default::default()
            })
            .id
    };
    let (parent, branch, twig) = (add(0.0, branch_at + 1), add(90.0, 3), add(180.0, 2));
    let group = [
        StationRef::new(parent, branch_at),
        StationRef::new(branch, 0),
        StationRef::new(twig, 0),
    ];
    project.merge_stations(group[0], group[1]).unwrap();
    project.merge_stations(group[1], group[2]).unwrap();

    let dir = TempDir::new().unwrap();
    let path = dir.path().join("CaveA.stalx");
    Database::open(&path)
        .unwrap()
        .save_project(&project)
        .unwrap();
    let reopened = Database::open_existing(&path)
        .unwrap()
        .load_project()
        .unwrap();
    (reopened, group)
}

/// Every station of `group` lists exactly the other two as `merged_with`.
fn assert_one_merge_group(project: &Project, group: [StationRef; 3]) {
    assert_eq!(project.station_merges.len(), 2, "both merges were saved");
    let stations = project.compute_station_coordinates();
    for (i, station_ref) in group.iter().enumerate() {
        let station = stations
            .iter()
            .find(|s| s.station_ref == *station_ref)
            .unwrap_or_else(|| panic!("station {station_ref} exists"));
        let others: Vec<StationRef> = group
            .iter()
            .enumerate()
            .filter(|&(j, _)| j != i)
            .map(|(_, r)| *r)
            .collect();
        println!("{station_ref} merged_with {:?}", station.merged_with);
        for other in &others {
            assert!(
                station.merged_with.contains(other),
                "{station_ref} must be merged with {other} (transitively for the \
                 parent/twig pair), got {:?}",
                station.merged_with
            );
        }
        assert_eq!(
            station.merged_with.len(),
            others.len(),
            "{station_ref} is in exactly one group, got {:?}",
            station.merged_with
        );
    }
}

/// The iter2 shape: the branch leaves the parent at its third station.
#[test]
fn a_fork_off_an_early_station_keeps_its_merge_group_through_a_reopen() {
    let (project, group) = reopened_fork(2);
    assert_one_merge_group(&project, group);
}

/// The iter4 shape: the branch leaves a longer parent further along.
#[test]
fn a_fork_off_a_later_station_keeps_its_merge_group_through_a_reopen() {
    let (project, group) = reopened_fork(8);
    assert_one_merge_group(&project, group);
}
