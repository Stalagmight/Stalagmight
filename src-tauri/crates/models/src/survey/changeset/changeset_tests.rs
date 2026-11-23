//! The engine on hand-built cases: what a capture records for each kind of
//! edit (runs, flips, literal rows, hunks), refusals, counters, session ids,
//! the list diff, and the preview digest.

use serde_json::Value;

use crate::{LineId, ShotData, ShotUpdates, SplitAt, StationRef};

use super::super::test_helpers::{flat, gps, meta, project, shot};
use super::super::{ClosureStrategy, Project, SplitOptions, StartAt};
use super::lists::ListChange;
use super::testkit::{
    gate_shot, persisted, same_state, session_ids, session_ids_sound, set_credit, Rng,
};
use super::{capture, redo, undo, Changeset, Direction, Refusal, ShotField, Val};

fn with_uuids(prefix: &str, shots: Vec<ShotData>) -> Vec<ShotData> {
    shots
        .into_iter()
        .enumerate()
        .map(|(i, mut s)| {
            s.uuid = Some(format!("{prefix}-{i}"));
            s
        })
        .collect()
}

/// A free line of `n` shots with awkward readings, its start placed.
fn survey(n: usize) -> (Project, LineId) {
    let mut p = project();
    let shots = (0..n)
        .map(|i| {
            let mut s = shot(
                (i as f64 * 37.3 + 0.1) % 360.0,
                2.0 + i as f64 * 0.7,
                i as f64 * 0.1,
                i as f64 * 0.1 + 0.3,
            );
            s.left = Some(1.0 + i as f64 * 0.01);
            s.right = (i % 3 == 0).then_some(-0.0);
            s.pitch_in = (i % 4 == 0).then_some(12.5);
            s
        })
        .collect();
    let line = p
        .create_line(
            meta("Main"),
            StartAt::New,
            with_uuids("m", shots),
            Some([10.0, 20.0]),
        )
        .unwrap()
        .value;
    (p, line)
}

/// Undo then redo `changes` from `after`, bit for bit, in the session and
/// through the persisted entry.
fn round_trip(before: &Project, after: &Project, changes: &Changeset) {
    for (form, cs) in [
        ("session", changes.clone()),
        ("persisted", persisted(changes)),
    ] {
        let mut live = after.clone();
        undo(&mut live, &cs).unwrap_or_else(|e| panic!("{form} undo: {e}"));
        same_state(&live, before).unwrap_or_else(|e| panic!("{form} undo: {e}"));
        session_ids_sound(&live).unwrap();
        redo(&mut live, &cs).unwrap_or_else(|e| panic!("{form} redo: {e}"));
        same_state(&live, after).unwrap_or_else(|e| panic!("{form} redo: {e}"));
        session_ids_sound(&live).unwrap();
    }
}

/// The first `n` elements of a JSON array.
fn head(v: &Value, n: usize) -> Value {
    Value::Array(v.as_array().unwrap()[..n].to_vec())
}

fn json(changes: &Changeset) -> Value {
    serde_json::to_value(changes).unwrap()
}

#[test]
fn capturing_two_equal_states_records_nothing() {
    let (p, _) = survey(10);
    let changes = capture(&p, &p.clone());
    assert!(changes.is_empty());
    assert_eq!(json(&changes), serde_json::json!({}));
}

#[test]
fn a_cell_edit_is_one_run_per_side_holding_the_two_values() {
    let (before, line) = survey(10);
    let mut after = before.clone();
    let id = after.line(line).unwrap().shots[4].id;
    let updates = ShotUpdates {
        length: Some(9.75),
        ..ShotUpdates::default()
    };
    after.update_shot(line, id, &updates).unwrap();
    let changes = capture(&before, &after);
    let v = json(&changes);
    let change = &v["lines"][0];
    assert_eq!(change["id"], line);
    for (side, value) in [("after", 9.75), ("before", 2.0 + 4.0 * 0.7)] {
        let runs = change[side]["s"].as_array().unwrap();
        assert_eq!(runs.len(), 1, "{side}: one run: {runs:?}");
        assert_eq!(head(&runs[0], 5), serde_json::json!([line, 0, 10, 0, null]));
        assert_eq!(runs[0][5], serde_json::json!({"len": [[4, [value]]]}));
    }
    let digest = changes.digest();
    assert_eq!(digest.shot_cells.len(), 1);
    let cell = &digest.shot_cells[0];
    assert_eq!(
        (cell.line, cell.index, cell.field),
        (line, 4, ShotField::Length)
    );
    assert_eq!(cell.new.as_f64(), Some(9.75));
    assert_eq!(
        cell.old.as_ref().and_then(Val::as_f64),
        Some(2.0 + 4.0 * 0.7)
    );
    round_trip(&before, &after, &changes);
}

#[test]
fn a_reverse_is_one_flipped_run_per_side_with_only_the_flips_exceptions() {
    for (n, swap) in [(1, false), (2, true), (40, false), (40, true)] {
        let (before, line) = survey(n);
        let mut after = before.clone();
        after.reverse_line(line, swap).unwrap();
        let changes = capture(&before, &after);
        let v = json(&changes);
        let change = &v["lines"][0];
        let mode = if swap { 3 } else { 2 };
        for side in ["before", "after"] {
            let runs = change[side]["s"].as_array().unwrap();
            assert_eq!(runs.len(), 1, "n {n} {side}: one run");
            assert_eq!(
                head(&runs[0], 5),
                serde_json::json!([line, n - 1, n, mode, null])
            );
        }
        // The exceptions are where the flip does not give the bits back.
        let exceptions: usize = ["before", "after"]
            .iter()
            .filter_map(|side| change[*side]["s"][0].get(5))
            .map(|edits| edits.to_string().len())
            .sum();
        assert!(
            exceptions < 40 * n,
            "n {n}: {exceptions} bytes of exceptions"
        );
        assert_eq!(changes.digest().reversed_lines, vec![(line, n as u32)]);
        round_trip(&before, &after, &changes);
    }
}

#[test]
fn a_split_and_a_combine_move_shots_as_runs_not_rows() {
    let (base, line) = survey(12);
    let mut split = base.clone();
    let options = SplitOptions {
        name_a: "A".to_string(),
        name_b: "B".to_string(),
        color_a: "#000000".to_string(),
        color_b: "#ffffff".to_string(),
    };
    let second = split
        .split_line(line, SplitAt::Station { station_index: 5 }, &options)
        .unwrap()
        .value;
    let changes = capture(&base, &split);
    let text = serde_json::to_string(&changes).unwrap();
    assert!(!text.contains("\"u\":"), "no literal rows: {text}");
    let v = json(&changes);
    let new_line = v["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["id"] == second)
        .unwrap();
    let run = &new_line["after"]["s"][0];
    assert_eq!(head(run, 4), serde_json::json!([line, 5, 7, 0]));
    assert_eq!(
        run[4], "m-5",
        "a run out of another line carries its anchor"
    );
    round_trip(&base, &split, &changes);

    // Combine the halves back (the join is the shared station).
    let mut combined = split.clone();
    let candidate = combined
        .combine_candidate_for_lines(line, second)
        .expect("the halves touch");
    let options = crate::CombineLinesOptions {
        name: "Main".to_string(),
        color: "#4ecdc4".to_string(),
        swap_lr: false,
        comment_pick: crate::CombinePick::Kept,
        name_pick: crate::CombinePick::Kept,
        join: None,
    };
    combined
        .combine_lines(candidate.line_a, candidate.line_b, &options)
        .unwrap();
    let changes = capture(&split, &combined);
    let text = serde_json::to_string(&changes).unwrap();
    assert!(!text.contains("\"u\":"), "no literal rows: {text}");
    round_trip(&split, &combined, &changes);
}

#[test]
fn a_deleted_line_is_carried_whole_and_its_preview_names_it() {
    let (mut before, line) = survey(8);
    let s3 = before.line_stations(line).unwrap()[3];
    let side = before
        .create_line(
            meta("Side"),
            StartAt::Station(s3),
            with_uuids("s", vec![flat(10.0, 4.0), flat(20.0, 5.0)]),
            None,
        )
        .unwrap()
        .value;
    let mut after = before.clone();
    after.delete_line(side).unwrap();
    let changes = capture(&before, &after);
    let digest = changes.digest();
    assert_eq!(digest.lines_deleted.len(), 1);
    let gone = &digest.lines_deleted[0];
    assert_eq!(
        (gone.id, gone.name.as_deref(), gone.shots),
        (side, Some("Side"), 2)
    );
    assert_eq!(
        digest.stations_deleted.len(),
        2,
        "the two stations only it held"
    );
    assert_eq!(digest.shots_deleted_total, 2);
    assert_eq!(digest.shots_deleted[0].shot.uuid, "s-0");
    assert_eq!(digest.shots_deleted[1].shot.length, 5.0);
    round_trip(&before, &after, &changes);
}

#[test]
fn counters_never_go_down_and_a_redo_reuses_its_own_ids() {
    let (before, _) = survey(3);
    let mut after = before.clone();
    after
        .create_line(
            meta("New"),
            StartAt::New,
            with_uuids("n", vec![flat(0.0, 3.0); 4]),
            None,
        )
        .unwrap();
    let changes = capture(&before, &after);
    let mut live = after.clone();
    undo(&mut live, &changes).unwrap();
    assert_eq!(live.counters, after.counters, "undo keeps the counters up");
    same_state(&live, &before).unwrap();
    redo(&mut live, &changes).unwrap();
    assert_eq!(
        live.stations, after.stations,
        "the redo brings the same ids back"
    );
}

#[test]
fn session_ids_come_back_in_the_session_and_are_minted_after_a_reload() {
    let (before, line) = survey(10);
    let ids_before = session_ids(&before);
    let mut after = before.clone();
    let doomed = after.line(line).unwrap().shots[3].id;
    after.delete_shot(line, doomed).unwrap();
    let changes = capture(&before, &after);
    let mut live = after.clone();
    undo(&mut live, &changes).unwrap();
    assert_eq!(
        session_ids(&live),
        ids_before,
        "in the session: the same ids"
    );
    let mut reloaded = after.clone();
    undo(&mut reloaded, &persisted(&changes)).unwrap();
    session_ids_sound(&reloaded).unwrap();
    let restored = reloaded.line(line).unwrap().shots[3].id;
    assert!(
        restored >= after.line(line).unwrap().next_shot_id,
        "after a reload the restored shot gets a fresh id"
    );
    same_state(&reloaded, &before).unwrap();
}

#[test]
fn a_step_on_the_wrong_state_is_refused_with_what_differs() {
    let (before, line) = survey(6);
    let mut after = before.clone();
    let station = after.line_stations(line).unwrap()[2];
    after.station_mut(station).unwrap().name = "Renamed".to_string();
    let changes = capture(&before, &after);
    // Redo on the after state: the station is not the recorded "before".
    let mut live = after.clone();
    let refusal = redo(&mut live, &changes).unwrap_err();
    assert_eq!(refusal, Refusal::Station(station));
    assert!(refusal.to_string().contains(&format!("station {station}")));
    // A line changed behind the engine's back.
    let mut edited = before.clone();
    let id = edited.line(line).unwrap().shots[0].id;
    edited
        .update_shot(
            line,
            id,
            &ShotUpdates {
                length: Some(4.0),
                ..ShotUpdates::default()
            },
        )
        .unwrap();
    let line_change = capture(&before, &edited);
    let mut other = edited.clone();
    let id = other.line(line).unwrap().shots[5].id;
    other
        .set_shot_kind(line, id, crate::ShotKind::Jump)
        .unwrap();
    let text = serde_json::to_string(&other).unwrap();
    assert_eq!(undo(&mut other, &line_change), Err(Refusal::Line(line)));
    assert_eq!(serde_json::to_string(&other).unwrap(), text, "untouched");
}

#[test]
fn a_credit_is_interned_once_and_a_line_wide_edit_is_one_span() {
    let (before, line) = survey(30);
    let mut after = before.clone();
    set_credit(&mut after, line, 0..30, "Surveyor");
    let changes = capture(&before, &after);
    let v = json(&changes);
    assert_eq!(v["credits"].as_array().unwrap().len(), 1);
    assert_eq!(
        v["lines"][0]["after"]["s"][0][5],
        serde_json::json!({"cr": [[0, 30, 0]]})
    );
    assert_eq!(
        v["lines"][0]["before"]["s"][0][5],
        serde_json::json!({"cr": [[0, 30, null]]})
    );
    round_trip(&before, &after, &changes);
}

#[test]
fn closures_datums_and_placements_are_list_hunks() {
    let (mut before, line) = survey(8);
    let chain = before.line_stations(line).unwrap().to_vec();
    before
        .create_closure(chain[1], None, chain[6], None, ClosureStrategy::Batch)
        .unwrap();
    let mut after = before.clone();
    after
        .set_datum(chain[0], Some(line), gps(20.5, -87.0), None)
        .unwrap();
    after.remove_closure(after.closures[0].id).unwrap();
    let changes = capture(&before, &after);
    let digest = changes.digest();
    assert_eq!(digest.datums.added.len(), 1);
    assert_eq!(digest.closures.removed.len(), 1);
    assert_eq!(
        digest.placements.removed.len(),
        1,
        "the datum now places the piece"
    );
    round_trip(&before, &after, &changes);
}

#[test]
fn a_junction_merge_and_detach_round_trip() {
    let (mut before, line) = survey(6);
    let other = before
        .create_line(
            meta("Other"),
            StartAt::New,
            with_uuids("o", vec![flat(90.0, 5.0); 3]),
            Some([50.0, 50.0]),
        )
        .unwrap()
        .value;
    let (a, b) = (
        before.line_stations(other).unwrap()[0],
        before.line_stations(line).unwrap()[3],
    );
    let mut merged = before.clone();
    merged.merge_keeping_stationary(a, b).unwrap();
    let changes = capture(&before, &merged);
    assert_eq!(
        changes.digest().stations_deleted.len(),
        1,
        "the retired station"
    );
    round_trip(&before, &merged, &changes);
    let mut detached = merged.clone();
    let junction = detached.station_at(StationRef::new(other, 0)).unwrap();
    detached.detach_line_auto(junction, other, None).unwrap();
    let changes = capture(&merged, &detached);
    assert_eq!(
        changes.digest().stations_created.len(),
        1,
        "the detached line's new station"
    );
    round_trip(&merged, &detached, &changes);
}

#[test]
fn the_list_diff_steps_both_ways_and_keeps_far_apart_edits_apart() {
    let mut rng = Rng::new(7);
    for _ in 0..400 {
        let a: Vec<u32> = (0..rng.below(60)).map(|_| rng.below(8) as u32).collect();
        let mut b = a.clone();
        for _ in 0..rng.below(6) {
            match rng.below(3) {
                0 if !b.is_empty() => {
                    let i = rng.below(b.len());
                    b.remove(i);
                }
                1 => {
                    let i = rng.below(b.len() + 1);
                    b.insert(i, rng.below(8) as u32);
                }
                _ if !b.is_empty() => {
                    let i = rng.below(b.len());
                    b[i] = 100;
                }
                _ => {}
            }
        }
        let Some(change) = ListChange::between(&a, &b) else {
            assert_eq!(a, b);
            continue;
        };
        assert!(change.shape_ok());
        assert_eq!(
            change.step(&a, Direction::Redo).as_deref(),
            Some(b.as_slice())
        );
        assert_eq!(
            change.step(&b, Direction::Undo).as_deref(),
            Some(a.as_slice())
        );
    }
    // Two edits far apart are two small hunks, not one stretch between them.
    let a: Vec<u32> = (0..1000).collect();
    let mut b = a.clone();
    b.remove(900);
    b.remove(3);
    let change = ListChange::between(&a, &b).unwrap();
    assert_eq!(change.h.len(), 2);
    assert_eq!(
        change
            .h
            .iter()
            .map(|h| h.1.len() + h.2.len())
            .sum::<usize>(),
        2
    );
}

#[test]
fn a_float_change_to_negative_zero_is_recorded_and_restored() {
    let (before, line) = survey(4);
    let mut after = before.clone();
    after.line_mut(line).unwrap().shots[1].left = Some(-0.0);
    after.line_mut(line).unwrap().shots[2].right = Some(0.0);
    let changes = capture(&before, &after);
    assert!(!changes.is_empty());
    round_trip(&before, &after, &changes);
}

#[test]
fn a_gate_shot_has_the_usual_uuid_form() {
    let uuid = gate_shot(3, 4).uuid.unwrap();
    assert_eq!(uuid.len(), 36);
    assert_eq!(uuid, gate_shot(3, 4).uuid.unwrap(), "deterministic");
}

#[test]
fn the_digest_reads_project_station_and_credit_changes() {
    let (before, line) = survey(5);
    let mut after = before.clone();
    after.name = "Renamed project".to_string();
    after.settings.surface_elevation = Some(12.0);
    let station = after.line_stations(line).unwrap()[2];
    after.station_mut(station).unwrap().name = "Sump".to_string();
    after.station_mut(station).unwrap().comment = Some("restriction".to_string());
    set_credit(&mut after, line, 1..3, "Diver");
    let changes = capture(&before, &after);
    let digest = changes.digest();
    let fields: Vec<_> = digest.project.iter().map(|c| c.field).collect();
    assert_eq!(
        fields,
        vec![
            super::ProjectField::Name,
            super::ProjectField::SurfaceElevation
        ]
    );
    assert_eq!(digest.project[0].new.as_str(), Some("Renamed project"));
    assert_eq!(digest.station_fields.len(), 2);
    assert!(digest.station_fields.iter().all(|c| c.station == station));
    let credits: Vec<_> = digest
        .shot_cells
        .iter()
        .filter(|c| c.field == ShotField::Credit)
        .collect();
    assert_eq!(credits.len(), 2);
    let credit = changes
        .credit(&credits[0].new)
        .expect("the credit it names");
    assert_eq!(credit.surveyors[0].name, "Diver");
    assert!(credits[0]
        .old
        .as_ref()
        .is_some_and(|v| matches!(v, Val::Null)));
}
