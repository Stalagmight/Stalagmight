//! [#737] Loop closures across one shot, at the app-state level: a delete
//! with shift removes a closure it leaves across one shot, inside the same
//! undo unit as the delete, and reports it by the stations the user created
//! it between; the range preview predicts exactly what the gap-close
//! removes.
//!
//! Ported onto the station-graph model, with the closure dialog's guard and
//! the Add / Merge cases of the 0.6.0 file. Its warning and adjustment-report
//! cases are in `warnings_tests`. A tie onto a closed junction is the closure
//! already there (one station), refused as a duplicate where 0.6.0 created a
//! redundant second closure.
//!
//! All data is synthetic: projects built in memory, no fixture read or written.

use std::collections::HashSet;

use super::command_shot::{closure_removed_message, ClosureNames};
use super::AppState;
use crate::core::{Command, CommandResult, WarningSeverity};
use crate::models::survey::{ClosureId, ClosureStrategy, LineMeta, Project, StartAt, ZeroDatum};
use crate::models::{
    GpsCoordinates, LineId, ShotData, ShotId, StationId, StationRef,
    CLOSURE_REMOVED_ACROSS_ONE_SHOT,
};

fn shot(azimuth: f64, length: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        ..ShotData::default()
    }
}

fn meta(name: &str) -> LineMeta {
    LineMeta {
        name: Some(name.to_string()),
        ..LineMeta::default()
    }
}

/// A free line named `name`, its start placed at the origin.
fn add(p: &mut Project, name: &str, shots: Vec<ShotData>) -> LineId {
    let line = p.add_line(meta(name), StartAt::New, shots).unwrap();
    let start = p.line(line).unwrap().start;
    p.set_placement(start, 0.0, 0.0).unwrap();
    line
}

/// A line named `name` starting at `from` (a junction).
fn add_from(p: &mut Project, name: &str, from: StationId, shots: Vec<ShotData>) -> LineId {
    p.add_line(meta(name), StartAt::Station(from), shots)
        .unwrap()
}

fn st(p: &Project, line: LineId, index: usize) -> StationId {
    p.line(line).unwrap().station(index).unwrap()
}

fn state_with(project: Project) -> AppState {
    let violations = project.validate();
    assert!(violations.is_empty(), "invalid test survey: {violations:?}");
    let state = AppState::new().unwrap();
    state.set_project_for_tests(project);
    state
}

fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

fn shot_id(state: &AppState, line: LineId, position: usize) -> ShotId {
    project(state).line(line).unwrap().shots[position].id
}

/// A closure through the model's create operation (its refusals apply).
fn close(p: &mut Project, a: StationId, b: StationId, strategy: ClosureStrategy) -> ClosureId {
    p.create_closure(a, None, b, None, strategy)
        .unwrap()
        .value
        .closure
        .id
}

/// Plant a closure directly, as a file written before the creation-time
/// refusal can carry it.
fn plant(p: &mut Project, a: StationId, b: StationId) -> ClosureId {
    p.push_closure(a, None, b, None, ClosureStrategy::Batch)
}

/// The issue's loop: A two shots east, B north/east/south from A's start,
/// closed B:3-A:2.
fn issue_loop() -> (Project, LineId, LineId) {
    let mut p = Project::new("737", None);
    let a = add(&mut p, "Line 1", vec![shot(90.0, 10.0), shot(90.0, 10.0)]);
    let a0 = st(&p, a, 0);
    let b = add_from(
        &mut p,
        "Line 2",
        a0,
        vec![shot(0.0, 10.0), shot(90.0, 20.5), shot(180.0, 10.0)],
    );
    let (b3, a2) = (st(&p, b, 3), st(&p, a, 2));
    close(&mut p, b3, a2, ClosureStrategy::Batch);
    (p, a, b)
}

/// A line of `n` shots with varied bearings (so a gap-close moves
/// something), and nothing else.
fn chain(n: usize) -> (Project, LineId) {
    let mut p = Project::new("chain", None);
    let shots = (0..n)
        .map(|i| shot(30.0 * i as f64, 4.0 + i as f64))
        .collect();
    let a = add(&mut p, "Line 1", shots);
    (p, a)
}

fn closure_ends(p: &Project, id: ClosureId) -> Option<(StationId, StationId)> {
    p.closures.iter().find(|c| c.id == id).map(|c| (c.a, c.b))
}

#[test]
fn the_report_is_empty_without_a_project() {
    let state = AppState::new().unwrap();
    assert!(state.get_adjustment_report().is_none());
}

// ---- delete with shift removes the closure it degenerates ------------------------

/// Assert `result` reported exactly the removal of closures with the given
/// endpoint labels, in that order.
fn assert_removed(result: &CommandResult, labels: &[&str]) {
    let removed: Vec<&str> = result
        .warnings
        .iter()
        .filter(|w| w.code == CLOSURE_REMOVED_ACROSS_ONE_SHOT)
        .map(|w| {
            assert_eq!(w.severity, WarningSeverity::Warning);
            w.message.as_str()
        })
        .collect();
    let expected: Vec<String> = labels
        .iter()
        .map(|l| {
            format!(
                "Removed loop closure {l}: closing the gap left its two stations at the two ends \
                 of one shot."
            )
        })
        .collect();
    assert_eq!(removed, expected, "{:?}", result.warnings);
}

#[test]
fn a_range_gap_close_removes_a_closure_it_leaves_across_one_shot_and_one_undo_restores_it() {
    // Closure A:1-A:4 closes three shots. Gap-closing shots 1..2 takes
    // stations 2 and 3 off the line, so station 4 follows station 1 and the
    // closure would span one shot.
    let (mut p, a) = chain(5);
    let (a1, a4) = (st(&p, a, 1), st(&p, a, 4));
    let id = close(&mut p, a1, a4, ClosureStrategy::Batch);
    let state = state_with(p);
    let before = project(&state);
    let depth = state.get_undo_stack().len();
    let (s1, s2) = (shot_id(&state, a, 1), shot_id(&state, a, 2));

    let result = state.execute_delete_shot_range_shift(a, s1, s2);
    assert!(result.success, "{:?}", result.error);
    assert_removed(&result, &["Line 1 / 1 to Line 1 / 4"]);
    assert_eq!(state.get_undo_stack().len(), depth + 1, "one undo unit");
    let after = project(&state);
    assert_eq!(after.line(a).unwrap().shots.len(), 3);
    assert!(closure_ends(&after, id).is_none());
    assert!(after.closures_across_one_shot().is_empty());

    // ONE undo restores the shots and the closure, exactly.
    assert!(state.undo().success);
    let undone = project(&state);
    assert_eq!(undone.line(a).unwrap().shots.len(), 5);
    assert_eq!(undone.closures, before.closures);
    // Redo removes it again; undo once more is still the same restore.
    assert!(state.redo().success);
    assert!(closure_ends(&project(&state), id).is_none());
    assert!(state.undo().success);
    assert_eq!(closure_ends(&project(&state), id), Some((a1, a4)));
}

#[test]
fn a_one_shot_range_does_what_the_single_delete_refuses() {
    // The single delete refuses to close the gap of a shot that is part of
    // ANY loop, closure loops included, so it never shortens a closure to
    // one shot. The range gap-close is not blocked by a closure loop, so a
    // range of the one middle shot under closure A:1-A:3 is allowed - and
    // removes the closure.
    let (mut p, a) = chain(3);
    let (a1, a3) = (st(&p, a, 1), st(&p, a, 3));
    let id = close(&mut p, a1, a3, ClosureStrategy::Batch);
    let state = state_with(p);
    let middle = shot_id(&state, a, 1);

    let single = state.execute(Command::DeleteShot {
        line_id: a,
        shot_id: middle,
    });
    assert!(!single.success, "the single delete is shift-blocked");
    assert!(closure_ends(&project(&state), id).is_some());

    let range = state.execute_delete_shot_range_shift(a, middle, middle);
    assert!(range.success, "{:?}", range.error);
    assert_removed(&range, &["Line 1 / 1 to Line 1 / 3"]);
    assert!(project(&state).closures.is_empty());
    assert!(state.undo().success);
    assert_eq!(closure_ends(&project(&state), id), Some((a1, a3)));
}

#[test]
fn a_closure_on_a_junction_is_removed_by_the_shot_it_collapses() {
    // B branches off A:1. A closure from that junction (picked on B's slot)
    // to A:4 closes three shots of A; gap-closing A's shots 1..2 leaves it
    // across the one shot from A:1 to A:4. The message names the closure's
    // own ends, each on the slot it was picked on.
    let (mut p, a) = chain(5);
    let junction = st(&p, a, 1);
    let b = add_from(&mut p, "Line 2", junction, vec![shot(200.0, 6.0)]);
    let a4 = st(&p, a, 4);
    let id = p
        .create_closure(
            junction,
            Some(b),
            a4,
            None,
            ClosureStrategy::LeastSquaresWeighted,
        )
        .unwrap()
        .value
        .closure
        .id;
    let state = state_with(p);
    let (s1, s2) = (shot_id(&state, a, 1), shot_id(&state, a, 2));
    let result = state.execute_delete_shot_range_shift(a, s1, s2);
    assert!(result.success, "{:?}", result.error);
    assert_removed(&result, &["Line 2 / 1 to Line 1 / 4"]);
    assert!(closure_ends(&project(&state), id).is_none());
    assert!(state.undo().success);
    assert_eq!(closure_ends(&project(&state), id), Some((junction, a4)));
}

#[test]
fn a_gap_close_that_keeps_a_real_loop_keeps_its_closure() {
    // Closure A:0-A:3 over a one-shot range in the middle: still two shots,
    // still a loop. Nothing is removed or reported.
    let (mut p, a) = chain(4);
    let (a0, a3) = (st(&p, a, 0), st(&p, a, 3));
    let id = close(&mut p, a0, a3, ClosureStrategy::Batch);
    let state = state_with(p);
    let middle = shot_id(&state, a, 1);
    let result = state.execute_delete_shot_range_shift(a, middle, middle);
    assert!(result.success, "{:?}", result.error);
    assert_removed(&result, &[]);
    let after = project(&state);
    assert_eq!(closure_ends(&after, id), Some((a0, a3)));
    assert_eq!(
        after.topology().slot_on(a3, a),
        Some(StationRef::new(a, 2)),
        "the closure's far end is two shots on now"
    );
}

#[test]
fn a_delete_leaves_a_closure_that_already_spanned_one_shot_alone() {
    // A closure that was across one shot BEFORE the delete keeps its
    // warning; the delete only undoes what it caused.
    let (mut p, a, _) = issue_loop();
    let (a1, a2) = (st(&p, a, 1), st(&p, a, 2));
    let old = plant(&mut p, a1, a2);
    let a0 = st(&p, a, 0);
    let c = add_from(
        &mut p,
        "Line 3",
        a0,
        vec![shot(10.0, 3.0), shot(20.0, 3.0), shot(30.0, 3.0)],
    );
    let state = state_with(p);
    let tail = shot_id(&state, c, 2);
    let result = state.execute(Command::DeleteShot {
        line_id: c,
        shot_id: tail,
    });
    assert!(result.success, "{:?}", result.error);
    assert_removed(&result, &[]);
    assert!(closure_ends(&project(&state), old).is_some());
}

/// The reviewer's case, through the preview AND the command: line C goes
/// east 5, east 5, south 5, west 5, north 4.8 and closure C:1-C:5 closes the
/// loop. Close Gap & Delete on shots 1..=3 leaves the closure across the one
/// shot from C:1 to C:5. The bulk-delete dialog must list it BEFORE the user
/// confirms, the toast after, and both by the stations the user knows.
#[test]
fn the_range_preview_lists_the_closure_the_gap_close_removes_by_its_stations() {
    let mut p = Project::new("preview", None);
    let c = add(
        &mut p,
        "Line 1",
        vec![
            shot(90.0, 5.0),
            shot(90.0, 5.0),
            shot(180.0, 5.0),
            shot(270.0, 5.0),
            shot(0.0, 4.8),
        ],
    );
    let (c1, c5) = (st(&p, c, 1), st(&p, c, 5));
    let id = close(&mut p, c1, c5, ClosureStrategy::Batch);
    let state = state_with(p);
    let (first, last) = (shot_id(&state, c, 1), shot_id(&state, c, 3));

    let before = project(&state);
    let preview = before.preview_delete_shot_range(c, first, last).unwrap();
    assert!(
        !preview.shift_blocked,
        "{:?}",
        preview.shift_blocked_reasons
    );
    let predicted = preview
        .closures_removed_across_one_shot
        .clone()
        .expect("a gap-close is offered, so it is predicted");
    assert_eq!(predicted.len(), 1);
    assert_eq!(
        (predicted[0].id, predicted[0].a, predicted[0].b),
        (id, c1, c5)
    );
    // On the wire for the dialog: the closure's slots before the delete.
    let impact = before.delete_range_impact(&preview);
    let wire = serde_json::to_value(&impact).unwrap();
    assert_eq!(
        wire["closures_removed_across_one_shot"][0]["station_b"]["station_index"], 5,
        "{wire}"
    );

    let result = state.execute_delete_shot_range_shift(c, first, last);
    assert!(result.success, "{:?}", result.error);
    assert_removed(&result, &["Line 1 / 1 to Line 1 / 5"]);
    assert_eq!(
        result.warnings[0].message,
        closure_removed_message(&ClosureNames::of(&before), &before, &predicted[0])
    );
    assert!(project(&state).closures.is_empty());
    // Undo puts it back as C:1-C:5.
    assert!(state.undo().success);
    assert_eq!(closure_ends(&project(&state), id), Some((c1, c5)));
}

#[test]
fn the_range_preview_predicts_nothing_where_nothing_goes() {
    // A gap-close that keeps a real loop: predicted empty, not absent.
    let (mut p, a) = chain(4);
    let (a0, a3) = (st(&p, a, 0), st(&p, a, 3));
    close(&mut p, a0, a3, ClosureStrategy::Batch);
    let state = state_with(p);
    let middle = shot_id(&state, a, 1);
    let p = project(&state);
    let preview = p.preview_delete_shot_range(a, middle, middle).unwrap();
    assert!(preview
        .closures_removed_across_one_shot
        .is_some_and(|v| v.is_empty()));
    // The whole line offers no gap-close, so nothing is predicted at all,
    // and the key stays off the wire.
    let (first, last) = (shot_id(&state, a, 0), shot_id(&state, a, 3));
    let whole = p.preview_delete_shot_range(a, first, last).unwrap();
    assert!(whole.closures_removed_across_one_shot.is_none());
    let wire = serde_json::to_value(p.delete_range_impact(&whole)).unwrap();
    assert!(
        wire.get("closures_removed_across_one_shot").is_none(),
        "{wire}"
    );
    // A blocked shift offers no gap-close either: here it would drop the zero
    // datum on station 2, which the one-shot range removes.
    let mut p = Project::new("blocked", None);
    let a = p
        .add_line(
            meta("Line 1"),
            StartAt::New,
            (0..4).map(|i| shot(30.0 * f64::from(i), 4.0)).collect(),
        )
        .unwrap();
    let (a1, a2, a3) = (st(&p, a, 1), st(&p, a, 2), st(&p, a, 3));
    p.push_datum(ZeroDatum::new(
        a2,
        GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
    ));
    close(&mut p, a1, a3, ClosureStrategy::Batch);
    let state = state_with(p);
    let middle = shot_id(&state, a, 1);
    let preview = project(&state)
        .preview_delete_shot_range(a, middle, middle)
        .unwrap();
    assert!(preview.shift_blocked, "the fixture must block the shift");
    assert!(preview.closures_removed_across_one_shot.is_none());
}

/// Adversarial: random small surveys (lines branching off each other,
/// closures through the model's create operation) and random single and
/// range deletes. After every delete that succeeds, no closure may be across
/// one shot that was not before it, every removal is reported, and one undo
/// restores the closures exactly.
#[test]
fn no_delete_leaves_a_new_closure_across_one_shot() {
    let run = delete_fuzz(0x737_d1e7e, 40, false);
    // The invariant has to be exercised, not only vacuously true.
    assert!(
        run.removals > 0,
        "no case removed a closure; widen the generator"
    );
    assert!(run.agreed > 0, "no removal was predicted by a preview");
}

/// The same with snapped closures in the survey (#736): the solve ties an
/// aligned snap's two stations as one point, so a snap takes part in the
/// knots the delete's removal reads. Each snap is real: a one-shot line from
/// station x that ends exactly on station y, closed with a `Move` closure;
/// one in three is torn (ends a few centimetres off y, not aligned).
/// `STALAGMIGHT_737_DELETE_CASES` widens it.
#[test]
fn no_delete_leaves_a_new_closure_across_one_shot_with_snaps() {
    let cases = std::env::var("STALAGMIGHT_737_DELETE_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(120);
    let run = delete_fuzz(0x736_05a1, cases, true);
    eprintln!(
        "{cases} cases: {} snaps and {} torn snaps planted, {} deletes beside a snap, \
         {} closures removed ({} checked as needed), {} predicted",
        run.snaps,
        run.torn_snaps,
        run.deletes_with_snaps,
        run.removals,
        run.removals_checked,
        run.agreed
    );
    assert!(run.snaps > 0, "no snap was planted; widen the generator");
    assert!(
        run.torn_snaps > 0,
        "no torn snap was planted; widen the generator"
    );
    assert!(
        run.deletes_with_snaps > 0,
        "no delete ran beside a snap; widen the generator"
    );
}

#[derive(Default)]
struct DeleteFuzz {
    removals: usize,
    agreed: usize,
    snaps: usize,
    torn_snaps: usize,
    deletes_with_snaps: usize,
    removals_checked: usize,
}

/// Random branching networks with random `Batch` closures (and, with
/// `snaps`, snapped closures), then one delete - a single shot, or a range
/// with shift. The bulk-delete preview names exactly what the delete
/// removes, no closure is newly across one shot once the command has run,
/// and one Undo restores every closure.
fn delete_fuzz(seed0: u64, cases: usize, snaps: bool) -> DeleteFuzz {
    let mut seed: u64 = seed0;
    let mut next = |m: u64| -> u64 {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) % m.max(1)
    };
    let mut run = DeleteFuzz::default();
    for _case in 0..cases {
        let mut p = Project::new("fuzz", None);
        let mut lines: Vec<LineId> = Vec::new();
        for i in 0..(2 + next(2)) {
            let shots = (0..(3 + next(4)))
                .map(|_| shot(next(360) as f64, 3.0 + next(8) as f64))
                .collect();
            let name = format!("Line {}", i + 1);
            let id = if i == 0 {
                add(&mut p, &name, shots)
            } else {
                let prev = lines[next(lines.len() as u64) as usize];
                let len = p.line(prev).unwrap().shots.len() as u64;
                let at = st(&p, prev, next(len + 1) as usize);
                add_from(&mut p, &name, at, shots)
            };
            lines.push(id);
        }
        let stations: Vec<StationId> = p.topology().stations().to_vec();
        let mut planted = 0;
        if snaps {
            for _ in 0..=next(2) {
                let x = stations[next(stations.len() as u64) as usize];
                let y = stations[next(stations.len() as u64) as usize];
                let raw = p.raw_positions();
                let (Some([ax, ay]), Some([bx, by])) = (raw.xy(x), raw.xy(y)) else {
                    continue;
                };
                // One in three is a torn snap: the line ends 1.5 to 5.5 cm
                // off y, so the `Move` closure is not aligned.
                let torn = next(3) == 0;
                let (mut dx, mut dy) = (bx - ax, by - ay);
                if torn {
                    let off = 0.015 + next(5) as f64 * 0.01;
                    let bearing = (next(360) as f64).to_radians();
                    dx += off * bearing.sin();
                    dy += off * bearing.cos();
                }
                let length = dx.hypot(dy);
                if length < 0.5 {
                    continue;
                }
                let azimuth = dx.atan2(dy).to_degrees().rem_euclid(360.0);
                let name = format!("Snap {}", lines.len() + 1);
                let s = add_from(&mut p, &name, x, vec![shot(azimuth, length)]);
                lines.push(s);
                let end = st(&p, s, 1);
                let Ok(created) = p.create_closure(end, Some(s), y, None, ClosureStrategy::Move)
                else {
                    continue;
                };
                let id = created.value.closure.id;
                let aligned = p.closure_alignment(&p.raw_positions());
                let position = p.closures.iter().position(|c| c.id == id);
                match position.map(|i| aligned[i]) {
                    Some(true) => planted += 1,
                    Some(false) if torn => run.torn_snaps += 1,
                    _ => {}
                }
            }
        }
        run.snaps += planted;
        let stations: Vec<StationId> = p.topology().stations().to_vec();
        for _ in 0..(2 + next(4)) {
            let x = stations[next(stations.len() as u64) as usize];
            let y = stations[next(stations.len() as u64) as usize];
            let _ = p.create_closure(x, None, y, None, ClosureStrategy::Batch);
        }
        let state = state_with(p);
        let before = project(&state);
        let before_across: HashSet<ClosureId> = before
            .closures_across_one_shot()
            .into_iter()
            .map(|c| c.closure_id)
            .collect();
        let line = lines[next(lines.len() as u64) as usize];
        let n = before.line(line).unwrap().shots.len();
        let mut predicted = None;
        let result = if next(2) == 0 || n < 3 {
            let pos = next(n as u64) as usize;
            state.execute(Command::DeleteShot {
                line_id: line,
                shot_id: shot_id(&state, line, pos),
            })
        } else {
            let start = 1 + next((n - 2) as u64) as usize;
            let end = (start + next(2) as usize).min(n - 2);
            let (first, last) = (shot_id(&state, line, start), shot_id(&state, line, end));
            // What the bulk-delete dialog will show, taken before the delete.
            predicted = before
                .preview_delete_shot_range(line, first, last)
                .ok()
                .and_then(|preview| preview.closures_removed_across_one_shot);
            state.execute_delete_shot_range_shift(line, first, last)
        };
        if !result.success {
            continue;
        }
        run.deletes_with_snaps += usize::from(planted > 0);
        // The preview predicts exactly what the gap-close removed, by the same
        // pre-delete names (#737).
        if let Some(predicted) = predicted {
            let names = ClosureNames::of(&before);
            let shown: Vec<String> = predicted
                .iter()
                .map(|closure| closure_removed_message(&names, &before, closure))
                .collect();
            let removed: Vec<String> = result
                .warnings
                .iter()
                .filter(|w| w.code == CLOSURE_REMOVED_ACROSS_ONE_SHOT)
                .map(|w| w.message.clone())
                .collect();
            assert_eq!(shown, removed, "preview and execute disagree");
            run.agreed += usize::from(!removed.is_empty());
        }
        let after = project(&state);
        assert!(after.validate().is_empty());
        for c in after.closures_across_one_shot() {
            assert!(
                before_across.contains(&c.closure_id),
                "closure {} is newly across one shot after {:?}",
                c.closure_id,
                result.warnings
            );
        }
        let reported = result
            .warnings
            .iter()
            .filter(|w| w.code == CLOSURE_REMOVED_ACROSS_ONE_SHOT)
            .count();
        run.removals += reported;
        // Every closure the delete removed as across one shot is across one
        // shot in the project the command left: put it back where it was in
        // the list and ask. Only when every closure that went is one it
        // reported, so a closure the orphan sweep took (#738) or one deleted
        // with its station is not mistaken for one.
        let gone: Vec<ClosureId> = before
            .closures
            .iter()
            .map(|c| c.id)
            .filter(|id| !after.closures.iter().any(|c| c.id == *id))
            .collect();
        if reported > 0 && gone.len() == reported {
            let order = |id: ClosureId| before.closures.iter().position(|c| c.id == id);
            for &id in &gone {
                let mut probe = after.clone();
                let back = before.closures.iter().find(|c| c.id == id).unwrap();
                probe.closures.push(*back);
                probe.closures.sort_by_key(|c| order(c.id));
                assert!(
                    probe
                        .closures_across_one_shot()
                        .iter()
                        .any(|c| c.closure_id == id),
                    "closure {id} was removed as across one shot, but it is not once the \
                     command has run: {:?}",
                    result.warnings
                );
                run.removals_checked += 1;
            }
        }
        assert!(state.undo().success);
        assert_eq!(
            project(&state).closures,
            before.closures,
            "undo did not restore the closures"
        );
    }
    run
}

// ---- the closure dialog's guard -----------------------------------------------------

#[test]
fn the_connectivity_check_carries_the_backend_refusal() {
    let (p, a, b) = issue_loop();
    let state = state_with(p);
    let slot = StationRef::new;
    let across = state
        .check_merge_connectivity(slot(a, 1), slot(a, 2))
        .unwrap();
    assert!(across.connected && !across.can_merge);
    let refusal = across.closure_refusal.expect("a refusal for one shot");
    assert!(
        refusal.starts_with(
            "This closure would join the two ends of one shot (Line 1 / 1 to Line 1 / 2) and \
             squeeze that shot to zero length."
        ),
        "{refusal}"
    );
    // The very words the create command answers with.
    let created = state.execute(Command::CreateLoopClosure {
        station_a: slot(a, 1),
        station_b: slot(a, 2),
        closure_strategy: crate::models::ClosureStrategy::Batch,
    });
    assert!(!created.success);
    assert_eq!(created.error.as_deref(), Some(refusal.as_str()));

    // A real loop: no refusal, and the key is absent on the wire.
    let fine = state
        .check_merge_connectivity(slot(a, 1), slot(b, 2))
        .unwrap();
    assert!(fine.closure_refusal.is_none());
    let wire = serde_json::to_value(&fine).unwrap();
    assert!(wire.get("closure_refusal").is_none(), "{wire}");
    // Not connected: a merge is offered, and no closure refusal.
    let mut other = project(&state);
    let lone = add(&mut other, "Lone", vec![shot(0.0, 3.0)]);
    state.set_project_for_tests(other);
    let merge = state
        .check_merge_connectivity(slot(a, 1), slot(lone, 0))
        .unwrap();
    assert!(merge.can_merge && merge.closure_refusal.is_none());
}

#[test]
fn a_tie_onto_a_closed_junction_is_the_closure_already_there() {
    // 0.6.0: C:0 merged onto B:3, which the loop's closure ties to A:2, so a
    // closure C:0-A:2 added a redundant tie and was created. On the station
    // graph C:0 IS B:3's station, so C:0-A:2 names the closure that already
    // exists: the dialog's check refuses it as a duplicate (no #737 refusal),
    // and so does the command.
    let (mut p, a, b) = issue_loop();
    let b3 = st(&p, b, 3);
    let c = add_from(&mut p, "Line 3", b3, vec![shot(0.0, 5.0)]);
    let state = state_with(p);
    let slot = StationRef::new;
    let duplicate = "A loop closure already exists between these stations.";
    assert_eq!(
        state
            .check_merge_connectivity(slot(c, 0), slot(a, 2))
            .unwrap_err(),
        duplicate
    );
    let created = state.execute(Command::CreateLoopClosure {
        station_a: slot(c, 0),
        station_b: slot(a, 2),
        closure_strategy: crate::models::ClosureStrategy::Batch,
    });
    assert!(!created.success);
    assert_eq!(created.error.as_deref(), Some(duplicate));
    let after = project(&state);
    assert_eq!(after.closures.len(), 1);
    // No warning, and the Computed View solves cleanly.
    let warnings = state.compute_project_warnings(&after);
    assert!(warnings
        .iter()
        .all(|w| w.code != crate::models::CLOSURE_ACROSS_ONE_SHOT
            && w.code != crate::models::CLOSURE_ADJUSTMENT_FAILED));
    state.get_adjusted_stations().unwrap();
    let report = state.get_adjustment_report().unwrap();
    assert!(
        report.skipped_closures.is_empty() && report.failures.is_empty(),
        "{report:?}"
    );
}

// ---- closures that arrive without create_loop_closure -------------------------------

/// A legacy `.stalx` holding the issue's loop plus a closure across the one
/// shot A:1-A:2, as a build from before the refusal (or another tool) wrote
/// it. Synthetic, in a temp dir; never a fixture.
fn legacy_file_with_closure_across_one_shot(
    dir: &std::path::Path,
    name: &str,
) -> std::path::PathBuf {
    use super::legacy_fixture_kit::{frozen, old, slot, write_legacy_file, Legacy};
    let mut p = Legacy::new(name.to_string(), None);
    let mut add = |shots: Vec<ShotData>| {
        p.add_line(old::LineData {
            shots: shots.iter().map(frozen).collect(),
            ..Default::default()
        })
        .id
    };
    let a = add(vec![shot(90.0, 10.0), shot(90.0, 10.0)]);
    let b = add(vec![shot(0.0, 10.0), shot(90.0, 20.5), shot(180.0, 10.0)]);
    p.merge_stations(slot(a, 0), slot(b, 0)).unwrap();
    p.create_loop_closure(slot(b, 3), slot(a, 2), old::ClosureStrategy::Batch)
        .unwrap();
    let id = p.next_closure_id.max(1);
    p.next_closure_id = id + 1;
    p.loop_closures.push(old::LoopClosure {
        id,
        station_a: p.key_for_ref(slot(a, 1)).unwrap(),
        station_b: p.key_for_ref(slot(a, 2)).unwrap(),
        path_length: 10.0,
        misclosure_distance: 0.0,
        error_percentage: 0.0,
        horizontal_misclosure: 0.0,
        vertical_misclosure: 0.0,
        strategy: old::ClosureStrategy::Batch,
        is_aligned: false,
    });
    let path = dir.join(format!("{name}.stalx"));
    write_legacy_file(&path, &p);
    path
}

/// A version 2 `.stalx` of one free line at `path`.
fn v2_file(path: &std::path::Path, name: &str, shots: Vec<ShotData>) {
    use crate::persistence::stalx_v2::{self, Extras, Mode};
    let mut p = Project::new(name, None);
    add(&mut p, "Line 1", shots);
    let bytes = stalx_v2::encode(&p, &Extras::default(), Mode::Normal).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// `p` carries exactly one closure across one shot, it is warned about, and
/// the Computed View skips it rather than failing.
fn assert_skipped_and_warned(p: Project) {
    let skipped = p.closures_across_one_shot();
    assert_eq!(skipped.len(), 1, "{skipped:?}");
    let state = state_with(p);
    let warnings = state.compute_project_warnings(&project(&state));
    let ours: Vec<_> = warnings
        .iter()
        .filter(|w| w.code == crate::models::CLOSURE_ACROSS_ONE_SHOT)
        .collect();
    assert_eq!(ours.len(), 1, "{warnings:?}");
    assert_eq!(ours[0].closure_id, Some(skipped[0].closure_id));
    state.get_adjusted_stations().unwrap();
    let report = state.get_adjustment_report().unwrap();
    assert_eq!(report.skipped_closures.len(), 1);
    assert!(report.failures.is_empty(), "{report:?}");
}

#[test]
fn adding_a_project_file_with_a_closure_across_one_shot_warns_and_skips_it() {
    // Add .stalx copies closures straight across, past the creation-time
    // refusal: the solve skip and the warning cover them.
    let dir = tempfile::tempdir().unwrap();
    let source = legacy_file_with_closure_across_one_shot(dir.path(), "source");
    let target = dir.path().join("target.stalx");
    v2_file(&target, "target", vec![shot(45.0, 7.0)]);
    let state = AppState::new().unwrap();
    let opened = state.execute(Command::OpenProject { path: target });
    assert!(opened.success, "open the target: {:?}", opened.error);
    let added = state.execute(Command::add_project_file(source));
    assert!(added.success, "{:?}", added.error);
    assert_skipped_and_warned(project(&state));
}

#[test]
fn merging_project_files_with_a_closure_across_one_shot_warns_and_skips_it() {
    // Merge Projects appends every source the same way.
    let dir = tempfile::tempdir().unwrap();
    let one = legacy_file_with_closure_across_one_shot(dir.path(), "one");
    let two = dir.path().join("two.stalx");
    v2_file(&two, "two", vec![shot(10.0, 4.0)]);
    let out = dir.path().join("merged.stalx");
    let state = AppState::new().unwrap();
    let merged = state.merge_projects(&[one, two], out.clone(), "merged", false);
    assert!(merged.success, "{:?}", merged.error);
    let bytes = std::fs::read(&out).unwrap();
    let decoded = crate::persistence::stalx_v2::decode(
        &bytes,
        &crate::persistence::stalx_v2::DecodeOptions::default(),
    )
    .unwrap();
    assert_skipped_and_warned(decoded.project);
}
