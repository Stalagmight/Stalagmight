//! [#756] Combine Sections beside a section held to the rest by loop
//! closures alone, through `AppState`. The issue's survey cannot be opened:
//! the open's orphan-closure sweep settles it first, which is why the issue
//! is not reachable in the app.
//!
//! All data is synthetic: the project is built in memory, as a file can
//! carry it (the app snaps only stations already connected), and the one
//! file written goes to a temporary directory.

use super::AppState;
use crate::models::{
    ClosureStrategy, LoopClosure, Project, SectionData, SectionId, ShotData, StationRef,
};

fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        ..Default::default()
    }
}

fn add(project: &mut Project, name: &str, shots: Vec<ShotData>) -> SectionId {
    project
        .add_section(SectionData {
            name: Some(name.to_string()),
            shots,
            ..Default::default()
        })
        .id
}

fn st(section: SectionId, index: u32) -> StationRef {
    StationRef::new(section, index)
}

fn xy(project: &Project, at: StationRef) -> (f64, f64) {
    let c = project.compute_station_raw_coords(at).unwrap();
    (c.x, c.y)
}

/// Move `id` so its station `index` sits exactly on `target`.
fn place(project: &mut Project, id: SectionId, index: u32, target: (f64, f64)) {
    let now = xy(project, st(id, index));
    project.adjust_section_offset(id, target.0 - now.0, target.1 - now.1);
}

/// A loop closure as a file stores it, measured off the current geometry.
fn plant(project: &mut Project, a: StationRef, b: StationRef, strategy: ClosureStrategy) {
    let id = project.next_closure_id;
    project.next_closure_id += 1;
    let (pa, pb) = (xy(project, a), xy(project, b));
    let gap = (pa.0 - pb.0).hypot(pa.1 - pb.1);
    project.loop_closures.push(LoopClosure {
        id,
        station_a: project.key_for_ref(a).unwrap(),
        station_b: project.key_for_ref(b).unwrap(),
        path_length: 10.0,
        misclosure_distance: gap,
        error_percentage: gap * 10.0,
        horizontal_misclosure: gap,
        vertical_misclosure: 0.0,
        strategy,
        is_aligned: strategy == ClosureStrategy::Move,
    });
}

/// The issue's survey: K's end and O's start 5 mm apart, tied by a loop
/// closure; N merged at K's start, M at O's end; Z snapped to O's station 1
/// and merged to W, which an ordinary loop closure ties to M.
fn survey() -> Project {
    let mut p = Project::new("756".to_string(), None);
    let k = add(&mut p, "K", vec![shot(90.0, 10.0), shot(80.0, 5.0)]);
    let o = add(&mut p, "O", vec![shot(0.0, 4.0), shot(300.0, 6.0)]);
    let n = add(&mut p, "N", vec![shot(0.0, 3.0)]);
    let m = add(&mut p, "M", vec![shot(90.0, 3.0)]);
    let z = add(&mut p, "Z", vec![shot(180.0, 5.0)]);
    let w = add(&mut p, "W", vec![shot(90.0, 4.0)]);
    p.set_section_offset(k, (100.0, -40.0));
    let k2 = xy(&p, st(k, 2));
    place(&mut p, o, 0, (k2.0 + 0.004, k2.1 - 0.003));
    let k0 = xy(&p, st(k, 0));
    place(&mut p, n, 0, k0);
    p.merge_stations(st(k, 0), st(n, 0)).unwrap();
    let o2 = xy(&p, st(o, 2));
    place(&mut p, m, 0, o2);
    p.merge_stations(st(o, 2), st(m, 0)).unwrap();
    let o1 = xy(&p, st(o, 1));
    place(&mut p, z, 0, o1);
    let z1 = xy(&p, st(z, 1));
    place(&mut p, w, 0, z1);
    p.merge_stations(st(w, 0), st(z, 1)).unwrap();
    plant(&mut p, st(o, 1), st(z, 0), ClosureStrategy::Move);
    plant(&mut p, st(w, 1), st(m, 1), ClosureStrategy::Batch);
    plant(&mut p, st(k, 2), st(o, 0), ClosureStrategy::Batch);
    p
}

fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

/// Why the issue's survey cannot be opened to reproduce it: written to a file
/// (as a `loop_closures` row edited by hand would leave it) and opened, the
/// open settles it before anything else. The snap and the join closure, each
/// joining two parts no merge ties, become merges (the join's moving one side
/// the 5 mm onto the other), and W's closure to M then closes a true loop. So
/// the state the issue describes only reaches a combine planted in memory,
/// never from a file.
#[test]
fn the_issue_survey_written_to_a_file_opens_settled() {
    let p = survey();
    let merges = p.station_merges.len();
    let w_to_m = p.loop_closures[1].id;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("planted.stalx");
    crate::persistence::Database::open(&path)
        .unwrap()
        .save_project(&p)
        .unwrap();
    let state = AppState::new().unwrap();
    assert!(state.open_project(path).success, "open");
    let opened = project(&state);
    assert_eq!(
        opened.station_merges.len(),
        merges + 2,
        "the snap and the join closure open as merges"
    );
    let closures: Vec<_> = opened
        .loop_closures
        .iter()
        .map(|c| (c.id, c.strategy, c.is_aligned))
        .collect();
    assert_eq!(
        closures,
        [(w_to_m, ClosureStrategy::Batch, false)],
        "W's closure to M stays"
    );
}
