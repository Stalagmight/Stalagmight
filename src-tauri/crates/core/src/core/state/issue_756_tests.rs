//! [#756] Combine Lines beside a line held to the rest by loop
//! closures alone, through `AppState`. The issue's survey cannot be opened:
//! the open's orphan-closure sweep settles it first, which is why the issue
//! is not reachable in the app. What a survey the app leaves can show is a
//! snap torn into a part a zero datum holds, which the plan now gives the
//! datum's reason, built here with the app's own commands.
//!
//! All data is synthetic: the projects are built in memory (the issue's as
//! a file can carry it; the app snaps only stations already connected), and
//! each file written goes to a temporary directory.

use super::AppState;
use crate::models::{
    ClosureStrategy, LineData, LineId, LoopClosure, Project, ShotData, StationRef,
};

fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        ..Default::default()
    }
}

fn add(project: &mut Project, name: &str, shots: Vec<ShotData>) -> LineId {
    project
        .add_line(LineData {
            name: Some(name.to_string()),
            shots,
            ..Default::default()
        })
        .id
}

fn st(line: LineId, index: u32) -> StationRef {
    StationRef::new(line, index)
}

fn xy(project: &Project, at: StationRef) -> (f64, f64) {
    let c = project.compute_station_raw_coords(at).unwrap();
    (c.x, c.y)
}

/// Move `id` so its station `index` sits exactly on `target`.
fn place(project: &mut Project, id: LineId, index: u32, target: (f64, f64)) {
    let now = xy(project, st(id, index));
    project.adjust_line_offset(id, target.0 - now.0, target.1 - now.1);
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
    p.set_line_offset(k, (100.0, -40.0));
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

/// Every station's raw position, by line and index.
fn positions(project: &Project) -> Vec<(StationRef, (f64, f64))> {
    project
        .lines
        .iter()
        .flat_map(|s| (0..=s.shots.len() as u32).map(move |i| st(s.id, i)))
        .map(|at| (at, xy(project, at)))
        .collect()
}

/// The one change a survey the app leaves can show (#756), built only with
/// the commands the app sends: two surveys each held by a zero datum (N's
/// station 1 on K's side, D's station 0 on O's side, D merged at O's end),
/// D's far end moved onto O's station 1 and snapped there (Move Station and
/// the snap dialog), and a loop closure from K's end to O's start (allowed
/// between two surveys held by zero datums). Combining K and O keeping K, O
/// moves and tears its merge to D, and the snap from O into D's part tears
/// too. Its reason is the zero datum that holds D, and the dialog names D as
/// the line that stays; it used to give the loop-closure reason. A save
/// and a reopen settle nothing more and move nothing.
#[test]
fn a_snap_into_a_part_a_zero_datum_holds_tears_for_the_datum_through_the_commands() {
    use crate::core::Command;
    use crate::models::{
        CombineJoinEnds, CombineLinesOptions, CombinePick, Coordinates, GpsCoordinates, LineEnd,
        TornTieKind, TornTieReason,
    };
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Held".to_string(),
        location: None,
    });
    let create = |name: &str, shots: Vec<ShotData>| -> LineId {
        let created = state.execute(Command::CreateLine {
            name: Some(name.to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots,
        });
        assert!(created.success, "{:?}", created.error);
        created.line_id.unwrap()
    };
    let at = |s: LineId, i: u32| xy(&project(&state), st(s, i));
    let move_onto = |line: LineId, index: u32, target: StationRef| {
        let (from, to) = (at(line, index), at(target.line_id, target.station_index));
        let moved = state.execute(Command::MoveLine {
            line_id: line,
            offset: Coordinates {
                x: to.0 - from.0,
                y: to.1 - from.1,
                z: 0.0,
            },
        });
        assert!(moved.success, "{:?}", moved.error);
    };
    let merge = |a: StationRef, b: StationRef| {
        let merged = state.execute(Command::MergeStations {
            station_a: a,
            station_b: b,
        });
        assert!(merged.success, "{:?}", merged.error);
    };
    let datum = |at: StationRef, latitude: f64| {
        let set = state.execute(Command::SetZeroDatum {
            station_ref: at,
            coordinates: GpsCoordinates {
                latitude,
                longitude: -87.0,
                depth_reference: 0.0,
            },
            accuracy_m: None,
            entrance: None,
        });
        assert!(set.success, "{:?}", set.error);
    };
    let k = create("K", vec![shot(90.0, 10.0), shot(80.0, 5.0)]);
    let n = create("N", vec![shot(0.0, 3.0)]);
    let o = create("O", vec![shot(0.0, 4.0), shot(300.0, 6.0)]);
    // From O's end back towards O's station 1, 10 cm short.
    let d = create("D", vec![shot(120.0, 5.9)]);
    move_onto(n, 0, st(k, 0));
    merge(st(k, 0), st(n, 0));
    datum(st(n, 1), 20.0);
    move_onto(d, 0, st(o, 2));
    merge(st(o, 2), st(d, 0));
    datum(st(d, 0), 20.0001);
    // Move Station: D's far end onto O's station 1, then the snap dialog.
    let (from, to) = (at(d, 1), at(o, 1));
    let moved = state.execute(Command::MoveStation {
        station_ref: st(d, 1),
        offset: Coordinates {
            x: to.0 - from.0,
            y: to.1 - from.1,
            z: 0.0,
        },
    });
    assert!(moved.success, "{:?}", moved.error);
    let snapped = state.execute(Command::CreateLoopClosure {
        station_a: st(d, 1),
        station_b: st(o, 1),
        closure_strategy: ClosureStrategy::Move,
    });
    assert!(snapped.success, "{:?}", snapped.error);
    assert!(project(&state).loop_closures[0].is_aligned, "a snap");
    let tied = state.execute(Command::CreateLoopClosure {
        station_a: st(k, 2),
        station_b: st(o, 0),
        closure_strategy: ClosureStrategy::Batch,
    });
    assert!(tied.success, "{:?}", tied.error);

    let candidates = state.combine_candidates(st(k, 2)).unwrap();
    let candidate = candidates
        .iter()
        .find(|c| [c.line_a, c.line_b] == [k, o])
        .unwrap();
    let plan = &candidate.plan_keeping_a;
    assert_eq!(plan.moving_line, Some(o));
    let mut torn: Vec<_> = plan
        .torn_ties
        .iter()
        .map(|t| {
            // The end the dialog names as held: the one that does not move.
            let held = (t.reason == TornTieReason::ZeroDatum)
                .then(|| {
                    [t.station_a, t.station_b]
                        .into_iter()
                        .find(|at| !plan.moved_lines.contains(&at.line_id))
                })
                .flatten();
            (t.kind, t.reason, held)
        })
        .collect();
    torn.sort_by_key(|(kind, ..)| *kind == TornTieKind::SnappedClosure);
    assert_eq!(
        torn,
        [
            (TornTieKind::Merge, TornTieReason::ZeroDatum, Some(st(d, 0))),
            (
                TornTieKind::SnappedClosure,
                TornTieReason::ZeroDatum,
                Some(st(d, 1))
            ),
        ]
    );
    let result = state.execute_combine_lines(
        k,
        o,
        CombineLinesOptions {
            name: "Joined".to_string(),
            color: "#abcdef".to_string(),
            swap_lr: true,
            comment_pick: CombinePick::Kept,
            name_pick: CombinePick::Kept,
            join: Some(CombineJoinEnds {
                kept: LineEnd::End,
                other: LineEnd::Start,
            }),
        },
    );
    assert!(result.success, "{:?}", result.error);
    let combined = project(&state);
    assert!(
        combined.loop_closures.iter().all(|c| !c.is_aligned),
        "the snap is an ordinary loop closure now"
    );

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("held.stalx");
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(path.clone())
            })
            .success
    );
    let saved = project(&state);
    assert!(state.execute(Command::CloseProject).success);
    assert!(state.open_project(path).success, "reopen");
    let reopened = project(&state);
    assert_eq!(reopened.station_merges.len(), saved.station_merges.len());
    assert_eq!(reopened.loop_closures.len(), saved.loop_closures.len());
    for ((at, was), (_, now)) in positions(&saved).into_iter().zip(positions(&reopened)) {
        let moved = (was.0 - now.0).hypot(was.1 - now.1);
        assert!(moved < 1e-6, "the reopen moved {at:?} by {moved} m");
    }
}
