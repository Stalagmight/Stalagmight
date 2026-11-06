//! Deleting a shot whose doomed station carries TWO loop closures into one
//! rigid group (#576 fuzzers, the undo fuzzer's seed 34425 under `COMBINED=1`).
//!
//! `preview_delete_shot` decides, closure by closure, whether a closure on the
//! removed station stays a loop (re-anchored onto the replacement station) or
//! has become the sole bridge and is promoted to a merge. It used to ask that
//! connectivity question of the graph WITHOUT the promotions it had already
//! planned, so two closures from one station into two different stations of
//! one rigid section were BOTH promoted. The first promotion is settled by
//! translating the movable side; the second then joins two stations of one
//! rigid group that are a shot apart, `plan_merge_promotion` hands it an empty
//! `movable`, and the merge is left asserting a coincidence that is a whole
//! shot long - which no cascade, realign or Repair can close.
//!
//! The commands below are the fuzzer's own shrunk repro (`undo x1;
//! mergeStations(8/1,5/0); reverseSection(s=8); deleteShot(8:800000)` on top
//! of its `closure-order-a`/`closure-order-b` shape), rebuilt from synthetic
//! straight shots.

use super::fuzz_undo_tests::{run_fuzz, Plan, GATE_ITERATIONS};
use super::AppState;
use crate::core::Command;
use crate::models::{ClosureStrategy, GpsCoordinates, Project, SectionId, ShotData, StationRef};
use crate::test_fixtures::{state_with_project, straight_shot};
use crate::test_helpers::isolated_config;

fn create_section(state: &AppState, name: &str, shots: Vec<ShotData>) -> SectionId {
    let created = state.execute(Command::CreateSection {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        shots,
    });
    assert!(created.success, "create {name}: {:?}", created.error);
    created.section_id.unwrap()
}

/// Every merge's horizontal gap, from the app's own raw coordinates.
fn merge_gaps(project: &Project) -> Vec<(StationRef, StationRef, f64)> {
    project
        .station_merges
        .iter()
        .map(|merge| {
            let a = project.resolve_key(&merge.station_a).unwrap();
            let b = project.resolve_key(&merge.station_b).unwrap();
            let pa = project.compute_station_raw_coords(a).unwrap();
            let pb = project.compute_station_raw_coords(b).unwrap();
            (a, b, (pa.x - pb.x).hypot(pa.y - pb.y))
        })
        .collect()
}

/// Section `a` (two shots, zero datum on `a:0`) merged at its far end to `b`
/// (one shot); two closures from `a:0` and `a:1` onto `b:1`, in `closure_order`;
/// `c` merged onto `b:1`. Then `b` is reversed, which moves both closures and
/// `c`'s merge onto `b:0`, and `b`'s only shot is deleted - which removes `b:0`.
///
/// Returns the state just before the delete, with `(a, b, c)`.
fn two_closures_on_the_doomed_station(
    closure_order: [u32; 2],
) -> (AppState, SectionId, SectionId, SectionId) {
    let state = state_with_project("two closures on a doomed station");
    let a = create_section(
        &state,
        "a",
        vec![straight_shot(0.0, 10.0), straight_shot(90.0, 5.0)],
    );
    let b = create_section(&state, "b", vec![straight_shot(180.0, 4.0)]);
    let datum = state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: GpsCoordinates::new(20.9, -87.1, 0.0).unwrap(),
    });
    assert!(datum.success, "datum: {:?}", datum.error);
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 2),
        station_b: StationRef::new(b, 0),
    });
    assert!(merged.success, "merge a:2~b:0: {:?}", merged.error);
    for index in closure_order {
        let closure = state.execute(Command::CreateLoopClosure {
            station_a: StationRef::new(a, index),
            station_b: StationRef::new(b, 1),
            closure_strategy: ClosureStrategy::Move,
        });
        assert!(
            closure.success,
            "closure a:{index}~b:1: {:?}",
            closure.error
        );
    }
    let c = create_section(&state, "c", vec![straight_shot(45.0, 3.0)]);
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(b, 1),
        station_b: StationRef::new(c, 0),
    });
    assert!(merged.success, "merge b:1~c:0: {:?}", merged.error);
    let reversed = state.execute(Command::ReverseSection {
        section_id: b,
        swap_left_right: true,
    });
    assert!(reversed.success, "reverse b: {:?}", reversed.error);
    (state, a, b, c)
}

#[test]
fn a_second_closure_into_the_same_rigid_group_stays_a_closure() {
    let _config = isolated_config();
    for closure_order in [[0, 1], [1, 0]] {
        let (state, a, b, c) = two_closures_on_the_doomed_station(closure_order);
        let before = state.get_project_state().unwrap();
        let shot_id = before.get_section(b).unwrap().shots.first().unwrap().id;

        // Precondition: the reverse put both closures and `c`'s merge on `b:0`,
        // the station a delete of `b`'s only shot removes.
        let impact = before.preview_delete_shot(b, shot_id).unwrap();
        assert_eq!(impact.removed_station, StationRef::new(b, 0));
        assert_eq!(impact.removed_closures.len(), 2, "{impact:?}");
        println!(
            "order {closure_order:?}: preview promotes {:?}, re-anchors {:?}",
            impact.closure_to_merge, impact.replacement_closure_updates
        );

        let deleted = state.execute(Command::DeleteShot {
            section_id: b,
            shot_id,
        });
        assert!(deleted.success, "delete: {:?}", deleted.error);

        let after = state.get_project_state().unwrap();
        let gaps = merge_gaps(&after);
        for (x, y, gap) in &gaps {
            println!("order {closure_order:?}: merge {x:?} <-> {y:?} gap {gap:.6} m");
        }
        assert!(
            gaps.iter()
                .all(|(_, _, gap)| *gap < Project::MERGE_ALIGNMENT_INVARIANT_M),
            "order {closure_order:?}: a merge was left out of alignment: {gaps:?}"
        );
        assert!(
            after.check_merge_alignment().is_empty(),
            "order {closure_order:?}: {:?}",
            after.check_merge_alignment()
        );
        // The first closure is the sole bridge to `c` and is promoted; once it
        // is, the second one's ends reach each other through it and a shot of
        // `a`, so it is a loop again and must stay a closure - in the preview
        // the delete dialog shows as well as in what the delete did.
        assert_eq!(impact.closure_to_merge.len(), 1, "{impact:?}");
        assert_eq!(impact.replacement_closure_updates.len(), 1, "{impact:?}");
        // Exactly one rigid edge between `c` and `a`, and the other relationship
        // survives as the closure the surveyor recorded, now on `c:0`.
        let c_to_a = gaps
            .iter()
            .filter(|(x, y, _)| {
                (x.section_id == c && y.section_id == a) || (x.section_id == a && y.section_id == c)
            })
            .count();
        assert_eq!(c_to_a, 1, "order {closure_order:?}: {gaps:?}");
        let closures: Vec<(StationRef, StationRef)> = after
            .loop_closures
            .iter()
            .map(|closure| {
                (
                    after.resolve_key(&closure.station_a).unwrap(),
                    after.resolve_key(&closure.station_b).unwrap(),
                )
            })
            .collect();
        assert_eq!(closures.len(), 1, "order {closure_order:?}: {closures:?}");
        assert!(
            closures
                .iter()
                .all(|(x, y)| [*x, *y].contains(&StationRef::new(c, 0))),
            "order {closure_order:?}: the kept closure is not re-anchored on c:0: {closures:?}"
        );
    }
}

/// The fuzzer seed that found it, both halves, under the exact plan
/// `fuzz_seed_survey` ran it with (`COMBINED=1`, no `CONFIG`): it failed at depth
/// 12 on `deleteShot(s=8,shot=800000) left a station merge out of alignment -
/// merged 5:0 <-> 7:1 is 3.369553 m apart`. `run_fuzz` panics with the full
/// payload on any unclassified failure, so a clean return is the assertion.
#[test]
fn seed_34425_combined_keeps_every_merge_aligned() {
    let _config = isolated_config();
    for persistence in [false, true] {
        run_fuzz(
            34425,
            Plan {
                iterations: GATE_ITERATIONS,
                persistence,
                // `COMBINED_CROSSINGS`, which the survey uses under `COMBINED=1`.
                crossings: 3,
                prologue: None,
                jumps: true,
                closures: true,
                divergence: false,
                closure_shapes: true,
                horizon: true,
                config_fuzz: false,
                combined: true,
            },
        );
    }
}
