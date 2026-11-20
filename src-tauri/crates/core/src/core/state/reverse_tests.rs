//! `Command::ReverseLine` at the state layer (issue #24) on the station graph.
//!
//! The model-level transform is covered in `survey::edit::reverse_tests`;
//! these tests pin the command's contract through the execute pipeline: no
//! station moves (up to the flip's documented ulp rounding), undo restores
//! the line exactly, frozen shots do not block it, warnings are re-emitted,
//! a shot-less line is a provable no-op, and the result reports the lines
//! renumbered (`reversed_lines`) through a batch (parity-reduced), undo, redo
//! and the history jumps, partial failures included.
//!
//! The import toggle's cases are the import package's behaviour (the funnel
//! reverses before the line exists); they are ported here and ignored until
//! it lands.

use std::collections::HashMap;

use super::{AppState, VirtualShotMode};
use crate::core::{Command, ReversedLine};
use crate::models::survey::{LineMeta, Project, StartAt};
use crate::models::{
    Coordinates, GpsCoordinates, ImportLineConfig, ImportPlacement, LineId, LineUpdates, ShotData,
    StationRef,
};

/// One expected [`ReversedLine`] entry.
fn reversed_entry(line_id: LineId, shot_count: u32) -> ReversedLine {
    ReversedLine {
        line_id,
        shot_count,
    }
}

fn shot(azimuth: f64, length: f64, depth_in: f64, depth_out: f64) -> ShotData {
    ShotData {
        azimuth_in: azimuth,
        azimuth_out: azimuth,
        length,
        depth_in,
        depth_out,
        ..Default::default()
    }
}

/// Every slot's raw position.
fn abs_coord_map(project: &Project) -> HashMap<StationRef, Coordinates> {
    project
        .raw_positions()
        .slots(project)
        .into_iter()
        .map(|s| (s.slot, Coordinates::new(s.x, s.y, s.z)))
        .collect()
}

fn assert_coords_close(a: Coordinates, b: Coordinates) {
    let eps = 1e-6;
    assert!(
        (a.x - b.x).abs() < eps && (a.y - b.y).abs() < eps && (a.z - b.z).abs() < eps,
        "coordinate moved: {a:?} vs {b:?}"
    );
}

/// `shot`, plus per-shot LRUD and pitch that are distinct from each other and
/// from every other shot's, so assertions about them are not vacuous.
fn shot_with_lrud(azimuth: f64, length: f64, depth_in: f64, depth_out: f64, n: f64) -> ShotData {
    ShotData {
        left: Some(n),
        right: Some(n + 0.5),
        up: Some(n + 1.0),
        down: Some(n + 1.5),
        pitch_in: Some(n + 2.0),
        pitch_out: Some(-(n + 3.0)),
        ..shot(azimuth, length, depth_in, depth_out)
    }
}

fn create(state: &AppState, name: &str, shots: Vec<ShotData>) -> LineId {
    let r = state.execute(Command::CreateLine {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#ff6b6b".to_string(),
        direction: None,
        shots,
        cave_section_id: None,
    });
    assert!(r.success, "{:?}", r.error);
    r.line_id.unwrap()
}

/// A project with one three-shot line that turns a corner and changes depth,
/// so a botched reverse shows up as a moved station rather than a coincidence.
fn state_with_line() -> (AppState, LineId) {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Reverse".to_string(),
        location: None,
    });
    let line_id = create(
        &state,
        "Main line",
        vec![
            shot_with_lrud(0.0, 10.0, 0.0, 2.0, 1.0),
            shot_with_lrud(90.0, 5.0, 2.0, 2.0, 10.0),
            shot_with_lrud(210.0, 7.5, 2.0, 1.0, 20.0),
        ],
    );
    (state, line_id)
}

fn reverse(line_id: LineId) -> Command {
    Command::ReverseLine {
        line_id,
        swap_left_right: false,
    }
}

fn project(state: &AppState) -> Project {
    state.get_project_state().unwrap()
}

fn recolour(line_id: LineId, color: &str) -> Command {
    Command::UpdateLineMetadata {
        line_id,
        updates: LineUpdates {
            color: Some(color.to_string()),
            ..Default::default()
        },
    }
}

#[test]
fn reverse_keeps_every_station_where_it_was() {
    let (state, line_id) = state_with_line();
    let before = abs_coord_map(&project(&state));

    let result = state.execute(reverse(line_id));
    assert!(result.success, "reverse failed: {:?}", result.error);

    let after = abs_coord_map(&project(&state));
    assert_eq!(before.len(), after.len(), "station count changed");
    // Station i is now station N - i, at the same place.
    let n = 3;
    for i in 0..=n {
        assert_coords_close(
            before[&StationRef::new(line_id, i)],
            after[&StationRef::new(line_id, n - i)],
        );
    }
}

#[test]
fn reverse_reverses_the_shot_order() {
    let (state, line_id) = state_with_line();
    let lengths: Vec<_> = project(&state)
        .line(line_id)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.length)
        .collect();
    assert!(state.execute(reverse(line_id)).success);
    let reversed: Vec<_> = project(&state)
        .line(line_id)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.length)
        .collect();
    let mut expected = lengths;
    expected.reverse();
    assert_eq!(reversed, expected);
}

#[test]
fn undo_restores_the_original_line_exactly() {
    let (state, line_id) = state_with_line();
    let before = project(&state);
    let original = before.line(line_id).unwrap().clone();
    let coords_before = abs_coord_map(&before);

    assert!(
        state
            .execute(Command::ReverseLine {
                line_id,
                swap_left_right: true,
            })
            .success
    );
    assert!(state.undo().success, "reverse must be undoable");

    let restored_project = project(&state);
    // Every field of every shot (session ids, uuids, LRUD, pitches) and the
    // chain itself, to the bit.
    assert_eq!(restored_project.line(line_id).unwrap(), &original);
    let restored = abs_coord_map(&restored_project);
    for (sr, coord) in coords_before {
        assert_coords_close(coord, restored[&sr]);
    }
}

#[test]
fn redo_reapplies_the_reverse() {
    let (state, line_id) = state_with_line();
    assert!(state.execute(reverse(line_id)).success);
    let reversed = project(&state).line(line_id).unwrap().clone();

    assert!(state.undo().success);
    assert!(state.redo().success, "reverse must be redoable");
    assert_eq!(project(&state).line(line_id).unwrap(), &reversed);
}

#[test]
fn reverse_twice_is_the_identity() {
    let (state, line_id) = state_with_line();
    let original = project(&state).line(line_id).unwrap().clone();

    for _ in 0..2 {
        assert!(
            state
                .execute(Command::ReverseLine {
                    line_id,
                    swap_left_right: true,
                })
                .success
        );
    }

    let p = project(&state);
    let round_tripped = p.line(line_id).unwrap();
    assert_eq!(round_tripped.start, original.start);
    for (a, b) in round_tripped.shots.iter().zip(&original.shots) {
        assert_eq!(a.id, b.id);
        assert_eq!(a.uuid, b.uuid);
        assert_eq!(a.to, b.to);
        assert_eq!(a.azimuth_in.to_bits(), b.azimuth_in.to_bits());
        assert_eq!(a.azimuth_out.to_bits(), b.azimuth_out.to_bits());
        assert_eq!(a.depth_in.to_bits(), b.depth_in.to_bits());
        assert_eq!(a.depth_out.to_bits(), b.depth_out.to_bits());
        // `swap_left_right: true` twice lands back on the recorded values,
        // and the pitch double-negation cancels.
        assert_eq!(a.left, b.left);
        assert_eq!(a.right, b.right);
        assert_eq!(a.pitch_in, b.pitch_in);
        assert_eq!(a.pitch_out, b.pitch_out);
    }
}

/// A zero datum freezes the shots it anchors; `update_shot` rejects geometry
/// edits on them. Reverse must still work, because it leaves each physical
/// line untouched - and the datum stays on the station the surveyor pinned.
#[test]
fn reverse_works_on_a_datum_anchored_line_and_keeps_the_datum_in_place() {
    let (state, line_id) = state_with_line();
    let mut p = project(&state);
    p.set_datum_at(
        StationRef::new(line_id, 0),
        GpsCoordinates {
            latitude: 20.2,
            longitude: -87.4,
            depth_reference: 0.0,
        },
        None,
    )
    .unwrap();
    state.set_project_for_tests(p);
    let pinned = project(&state)
        .station_at(StationRef::new(line_id, 0))
        .unwrap();
    let before = abs_coord_map(&project(&state));

    let result = state.execute(reverse(line_id));
    assert!(
        result.success,
        "frozen shots must not block reverse: {:?}",
        result.error
    );

    // Nothing moved, and the datum now sits on station 3 - the same physical
    // station it was pinned to before the reverse.
    let p = project(&state);
    let after = abs_coord_map(&p);
    for i in 0..=3u32 {
        assert_coords_close(
            before[&StationRef::new(line_id, i)],
            after[&StationRef::new(line_id, 3 - i)],
        );
    }
    assert_eq!(p.datums.len(), 1);
    assert_eq!(p.datums[0].station, pinned);
    assert_eq!(p.station_at(StationRef::new(line_id, 3)), Some(pinned));
}

/// Reversing a multi-selection goes through a batch so it is one user action:
/// every line or none, and a single Undo to get back.
#[test]
fn batched_multi_line_reverse_is_atomic_and_undoes_in_one_step() {
    let (state, first_id) = state_with_line();
    let second_id = create(
        &state,
        "Side passage",
        vec![shot(45.0, 8.0, 0.0, 1.0), shot(315.0, 6.0, 1.0, 3.0)],
    );
    let before = project(&state);
    let first_lengths = |p: &Project| -> Vec<f64> {
        p.line(first_id)
            .unwrap()
            .shots
            .iter()
            .map(|s| s.length)
            .collect()
    };
    let lengths_before = first_lengths(&before);

    // A batch whose LAST command names a line that does not exist: the two
    // good reverses must not survive it.
    let failed = state.execute(Command::Batch {
        commands: vec![reverse(first_id), reverse(second_id), reverse(9999)],
        label: None,
    });
    assert!(!failed.success, "batch with a missing line should fail");
    assert_eq!(first_lengths(&project(&state)), lengths_before);

    // The all-good batch applies to both lines and costs exactly one undo.
    let steps = state.get_undo_stack().len();
    let ok = state.execute(Command::Batch {
        commands: vec![reverse(first_id), reverse(second_id)],
        label: None,
    });
    assert!(ok.success, "batch failed: {:?}", ok.error);
    assert_eq!(state.get_undo_stack().len(), steps + 1);
    let mut expected = lengths_before.clone();
    expected.reverse();
    assert_eq!(first_lengths(&project(&state)), expected);

    assert!(state.undo().success);
    let restored = project(&state);
    assert_eq!(first_lengths(&restored), lengths_before);
    assert_eq!(restored.line(second_id), before.line(second_id));
    let before_coords = abs_coord_map(&before);
    let restored_coords = abs_coord_map(&restored);
    assert_eq!(restored_coords.len(), before_coords.len());
    for (station_ref, coords) in &before_coords {
        assert_coords_close(*coords, restored_coords[station_ref]);
    }
}

/// A shot-less line (GPX imports one per waypoint) reverses to itself: the
/// command succeeds and records nothing, so the history underneath stays
/// reachable.
#[test]
fn reversing_a_shotless_line_leaves_the_undo_stack_usable() {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: "Waypoints".to_string(),
        location: None,
    });
    create(&state, "Real line", vec![shot(0.0, 10.0, 0.0, 2.0)]);
    let empty_id = create(&state, "Waypoint", Vec::new());
    let reversed = state.execute(reverse(empty_id));
    assert!(reversed.success, "reverse failed: {:?}", reversed.error);
    assert!(reversed.reversed_lines.is_none(), "nothing was renumbered");
    assert_eq!(reversed.line_id, None, "0.6.0's early success");
    assert_eq!(state.get_undo_stack().len(), 2, "no entry for the no-op");

    assert!(state.undo().success);
    assert!(state.undo().success);
    assert!(project(&state).lines().is_empty());
}

/// Reversing swaps each shot's depths, negates its pitch and normalises its
/// azimuths, so the discrepancy checks can read differently afterwards - and
/// the shot ids stay with POSITIONS. The reverse re-runs the validation over
/// the reversed line for exactly that reason.
///
/// The fixture: the first shot has out-of-range azimuths (a warning the
/// reverse's normalisation REMOVES) and the last records an 80-degree descent
/// on a level leg (a mismatch the reverse carries to position 0 with the sign
/// flipped).
#[test]
fn reverse_re_emits_warnings_computed_from_the_reversed_geometry() {
    let state = AppState::new().unwrap();
    // Built directly so the thresholds are the library defaults.
    let mut p = Project::new("Warnings", None);
    let mut out_of_range = shot(0.0, 10.0, 0.0, 0.0);
    out_of_range.azimuth_in = 400.0;
    out_of_range.azimuth_out = 400.0;
    let mut mismatch = shot(90.0, 10.0, 1.0, 1.0);
    mismatch.pitch_in = Some(80.0);
    mismatch.pitch_out = Some(80.0);
    let line_id = p
        .create_line(
            LineMeta {
                name: Some("Main".to_string()),
                ..LineMeta::default()
            },
            StartAt::New,
            vec![out_of_range, shot(45.0, 8.0, 0.0, 1.0), mismatch],
            None,
        )
        .unwrap()
        .value;
    state.set_project_for_tests(p);

    let result = state.execute(reverse(line_id));
    assert!(result.success, "reverse failed: {:?}", result.error);

    assert_eq!(
        result.warnings.len(),
        1,
        "warnings must be recomputed from the reversed line: {:?}",
        result.warnings
    );
    let warning = &result.warnings[0];
    assert_eq!(warning.code, "INCLINATION_DEPTH_MISMATCH");
    // The mismatched shot is now FIRST, named by its stations as they read now.
    let p = project(&state);
    let name = |i: u32| {
        let s = p.station_at(StationRef::new(line_id, i)).unwrap();
        p.stations[&s].name.clone()
    };
    let label = format!("Shot Main: {} \u{2192} {} ", name(0), name(1));
    assert!(
        warning.message.starts_with(&label),
        "{label:?} vs {}",
        warning.message
    );
    assert!(
        warning.message.contains("-80.0"),
        "the reverse negates pitch, so the recorded inclination is now -80: {}",
        warning.message
    );
}

/// A reverse of a line at a junction carries the junction with the physical
/// station (it is one station on both lines) and drags nothing.
#[test]
fn reversing_a_merge_anchored_line_moves_nothing_through_the_full_execute() {
    let (state, main_id) = state_with_line();
    let side_id = create(
        &state,
        "Side passage",
        vec![shot(45.0, 8.0, 2.0, 2.0), shot(135.0, 4.0, 2.0, 3.0)],
    );
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(side_id, 0),
        station_b: StationRef::new(main_id, 1),
    });
    assert!(merged.success, "merge failed: {:?}", merged.error);
    let before = abs_coord_map(&project(&state));

    let result = state.execute(reverse(main_id));
    assert!(result.success, "reverse failed: {:?}", result.error);

    let p = project(&state);
    let after = abs_coord_map(&p);
    assert_eq!(before.len(), after.len(), "station count changed");
    for i in 0..=3u32 {
        assert_coords_close(
            before[&StationRef::new(main_id, i)],
            after[&StationRef::new(main_id, 3 - i)],
        );
    }
    for i in 0..=2u32 {
        assert_coords_close(
            before[&StationRef::new(side_id, i)],
            after[&StationRef::new(side_id, i)],
        );
    }
    assert!(p.validate().is_empty(), "{:?}", p.validate());
    // The junction followed the physical station: the main line's middle
    // station is now its index 2, still the side passage's station 0.
    assert_eq!(p.merge_pairs().len(), 1);
    assert_eq!(
        p.station_at(StationRef::new(main_id, 2)),
        p.station_at(StationRef::new(side_id, 0))
    );
}

#[test]
fn reverse_of_a_missing_line_fails() {
    let (state, _) = state_with_line();
    let result = state.execute(reverse(9999));
    assert!(!result.success);
    assert!(result.error.unwrap().contains("no longer exists"));
}

#[test]
fn reverse_reports_the_line_it_touched() {
    let (state, line_id) = state_with_line();
    let result = state.execute(reverse(line_id));
    assert_eq!(result.line_id, Some(line_id));
    assert_eq!(
        result.reversed_lines,
        Some(vec![reversed_entry(line_id, 3)])
    );
}

/// The renumbering signal survives the shapes a reverse arrives in: batched
/// and replayed through undo/redo, where nothing in the geometry says so.
#[test]
fn reverse_reports_every_renumbered_line_through_batch_undo_and_redo() {
    let (state, first) = state_with_line();
    let second = create(
        &state,
        "Side",
        vec![shot(45.0, 8.0, 0.0, 1.0), shot(135.0, 4.0, 1.0, 1.0)],
    );

    let batch = state.execute(Command::Batch {
        commands: vec![reverse(first), reverse(second)],
        label: None,
    });
    assert!(batch.success, "{:?}", batch.error);
    assert_eq!(
        batch.reversed_lines,
        Some(vec![reversed_entry(first, 3), reversed_entry(second, 2)])
    );

    let undone = state.undo();
    assert!(undone.success, "{:?}", undone.error);
    let mut lines = undone.reversed_lines.unwrap();
    lines.sort_by_key(|r| r.line_id);
    assert_eq!(
        lines,
        vec![reversed_entry(first, 3), reversed_entry(second, 2)],
        "undoing a reverse re-reverses the numbering, so it reports the same lines"
    );

    let redone = state.redo();
    assert!(redone.success, "{:?}", redone.error);
    let mut lines = redone.reversed_lines.unwrap();
    lines.sort_by_key(|r| r.line_id);
    assert_eq!(
        lines,
        vec![reversed_entry(first, 3), reversed_entry(second, 2)]
    );

    // A command that renumbers nothing leaves the field off the wire.
    let recoloured = state.execute(recolour(first, "#000000"));
    assert!(recoloured.success, "{:?}", recoloured.error);
    assert!(recoloured.reversed_lines.is_none());
}

/// The history dropdown undoes several entries at once: a reverse buried in
/// the middle of the range must still be reported.
#[test]
fn undo_to_and_redo_to_report_every_reverse_in_the_range() {
    let (state, line) = state_with_line();
    assert!(state.execute(reverse(line)).success);
    // Bury the reverse behind a later, non-renumbering edit.
    assert!(state.execute(recolour(line, "#000000")).success);

    let undone = state.undo_to(1);
    assert!(undone.success, "{:?}", undone.error);
    assert_eq!(
        undone.reversed_lines,
        Some(vec![reversed_entry(line, 3)]),
        "a reverse inside the undone range must reach the frontend"
    );

    let redone = state.redo_to(1);
    assert!(redone.success, "{:?}", redone.error);
    assert_eq!(redone.reversed_lines, Some(vec![reversed_entry(line, 3)]));
}

/// `undo_to`'s aggregate is an ordered per-step log, not a parity-reduced
/// set: two reverses of one line are listed twice.
#[test]
fn undo_to_repeats_a_line_reversed_twice() {
    let (state, line) = state_with_line();
    for _ in 0..2 {
        assert!(state.execute(reverse(line)).success);
    }
    let undone = state.undo_to(1);
    assert!(undone.success, "{:?}", undone.error);
    assert_eq!(
        undone.reversed_lines,
        Some(vec![reversed_entry(line, 3), reversed_entry(line, 3)])
    );
}

/// A `Batch` that reverses ONE line TWICE leaves the numbering as it found
/// it: nothing to report, and nothing recorded.
#[test]
fn batch_reversing_one_line_twice_reports_nothing() {
    let (state, line_id) = state_with_line();
    let before = project(&state);
    let steps = state.get_undo_stack().len();
    let batch = state.execute(Command::Batch {
        commands: vec![reverse(line_id), reverse(line_id)],
        label: None,
    });
    assert!(batch.success, "{:?}", batch.error);
    assert!(
        batch.reversed_lines.is_none(),
        "a net no-op reverse renumbered nothing: {:?}",
        batch.reversed_lines
    );
    assert_eq!(project(&state).line(line_id), before.line(line_id));
    assert_eq!(state.get_undo_stack().len(), steps, "no entry for a no-op");
}

/// Parity is resolved PER LINE, not per batch: `[A, B, A]` nets to `B` alone.
#[test]
fn batch_a_b_a_reports_only_b() {
    let (state, first) = state_with_line();
    let second = create(
        &state,
        "Side",
        vec![shot(45.0, 8.0, 0.0, 1.0), shot(135.0, 4.0, 1.0, 1.0)],
    );
    let before = project(&state);
    let batch = state.execute(Command::Batch {
        commands: vec![reverse(first), reverse(second), reverse(first)],
        label: None,
    });
    assert!(batch.success, "{:?}", batch.error);
    assert_eq!(
        batch.reversed_lines,
        Some(vec![reversed_entry(second, 2)]),
        "only the line reversed an ODD number of times was renumbered"
    );
    assert_eq!(project(&state).line(first), before.line(first));
}

/// Tamper with the open project behind the engine's back so that the history
/// step recorded for Section `id` no longer applies.
fn rename_section_behind_the_engine(state: &AppState, name: &str) {
    let mut p = project(state);
    p.sections[0].name = name.to_string();
    state.set_project_for_tests(p);
}

/// `undo_to` stops at the first refused step, but the steps BEFORE it have
/// applied: if one re-reversed a line, the failure result still carries it.
#[test]
fn undo_to_reports_the_reverses_it_already_applied_when_a_later_step_fails() {
    let (state, line_id) = state_with_line();
    // An older entry that will refuse: a Section the project is later changed
    // under, so its undo's preconditions no longer hold.
    let created = state.execute(Command::CreateCaveSection {
        name: "Upper".to_string(),
        color: "#336699".to_string(),
        line_ids: Vec::new(),
    });
    assert!(created.success, "{:?}", created.error);
    assert!(state.execute(reverse(line_id)).success);
    rename_section_behind_the_engine(&state, "Tampered");

    let result = state.undo_to(1);
    assert!(!result.success, "step 2 must fail: {:?}", result.error);
    assert!(
        result
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("step 2 of 2"),
        "{:?}",
        result.error
    );
    assert_eq!(
        result.reversed_lines,
        Some(vec![reversed_entry(line_id, 3)]),
        "step 1 re-reversed the line and is not rolled back, so the failure \
         must still report it"
    );
}

/// The twin for `redo_to`.
#[test]
fn redo_to_reports_the_reverses_it_already_applied_when_a_later_step_fails() {
    let (state, line_id) = state_with_line();
    assert!(state.execute(reverse(line_id)).success);
    let created = state.execute(Command::CreateCaveSection {
        name: "Upper".to_string(),
        color: "#336699".to_string(),
        line_ids: Vec::new(),
    });
    assert!(created.success, "{:?}", created.error);
    assert!(state.undo().success);
    assert!(state.undo().success);
    // The Section's redo now finds a list it was not recorded against.
    let mut p = project(&state);
    p.sections.push(crate::models::Section {
        id: 40,
        name: "In the way".to_string(),
        color: "#000000".to_string(),
    });
    p.counters.next_section_id = 41;
    state.set_project_for_tests(p);

    let result = state.redo_to(1);
    assert!(!result.success, "step 2 must fail: {:?}", result.error);
    assert_eq!(
        result.reversed_lines,
        Some(vec![reversed_entry(line_id, 3)]),
        "step 1 re-applied the reverse and is not rolled back, so the failure \
         must still report it"
    );
}

/// A reverse of a shot-less line is a TRUE no-op in the backend (#24): the
/// batch path, the raw IPC command and the agent harness reach `execute`
/// without the dialog's filter. It keeps the redo branch and leaves the
/// project unmodified.
#[test]
fn reversing_a_shotless_line_leaves_redo_and_the_dirty_bit_alone() {
    let temp = tempfile::TempDir::new().unwrap();
    let (state, real) = state_with_line();
    let empty = create(&state, "Waypoint", Vec::new());

    // Give the redo stack something to lose: a real edit, then undo it.
    assert!(state.execute(reverse(real)).success);
    assert!(state.undo().success);
    let redo_before = state.get_redo_stack().len();
    assert_eq!(redo_before, 1, "fixture: there is a redo branch to protect");
    let saved = state.execute(Command::SaveProject {
        path: Some(temp.path().join("cave.stalx")),
    });
    assert!(saved.success, "{:?}", saved.error);
    assert!(!state.has_unsaved_changes());

    // The no-op, by both routes that skip the dialog's filter.
    assert!(state.execute(reverse(empty)).success);
    assert!(
        state
            .execute(Command::Batch {
                commands: vec![reverse(empty)],
                label: None,
            })
            .success
    );

    assert_eq!(
        state.get_redo_stack().len(),
        redo_before,
        "a provable no-op must not consume the redo branch"
    );
    assert!(!state.has_unsaved_changes(), "nor dirty the project");
}

/// 0.6.0 pinned a datum-less piece by a gauge pair the reverse had to keep in
/// step and the save had to carry. On the station graph the piece's
/// `Placement` names a station, which the reverse does not touch: after a
/// reverse, a save and a reopen, every station is where it was.
#[test]
fn the_placement_survives_a_reverse_and_a_save_and_reload() {
    let temp = tempfile::TempDir::new().unwrap();
    let path = temp.path().join("gauge.stalx");
    let (state, line_id) = state_with_line();
    let placements = project(&state).placements;

    assert!(state.execute(reverse(line_id)).success);
    assert_eq!(project(&state).placements, placements, "nothing re-keyed");
    assert!(
        state
            .execute(Command::SaveProject {
                path: Some(path.clone()),
            })
            .success
    );
    let in_memory = abs_coord_map(&project(&state));

    let reopened = AppState::new().unwrap();
    assert!(reopened.execute(Command::OpenProject { path }).success);
    let after = project(&reopened);
    assert_eq!(after.placements, placements);
    let after = abs_coord_map(&after);
    assert_eq!(after.len(), in_memory.len());
    for (slot, coords) in in_memory {
        let reloaded = after[&slot];
        assert_eq!(
            (coords.x.to_bits(), coords.y.to_bits(), coords.z.to_bits()),
            (
                reloaded.x.to_bits(),
                reloaded.y.to_bits(),
                reloaded.z.to_bits()
            ),
            "{slot:?} moved on reopen"
        );
    }
}

// === The import toggle (the import package's funnel) ===

fn import_config(
    name: &str,
    shots: Vec<ShotData>,
    placement: ImportPlacement,
    reverse: bool,
) -> ImportLineConfig {
    ImportLineConfig {
        cave_section: None,
        line_index: 0,
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        quality: 1.0,
        shots,
        placement,
        station_comments: Vec::new(),
        station_names: Vec::new(),
        reverse,
        azimuth_reference: None,
        explorers: None,
        override_date: None,
        surveyors: None,
    }
}

fn import(state: &AppState, lines: Vec<ImportLineConfig>) -> crate::core::CommandResult {
    let result = state.execute_import_lines(
        lines,
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    assert!(result.success, "import failed: {:?}", result.error);
    result
}

fn new_state(name: &str) -> AppState {
    let state = AppState::new().unwrap();
    state.execute(Command::CreateProject {
        name: name.to_string(),
        location: None,
    });
    state
}

/// The comment on a slot's station, if any.
fn comment_at(p: &Project, slot: StationRef) -> Option<String> {
    let s = p.station_at(slot)?;
    p.stations[&s].comment.clone()
}

/// q14: the import toggle reverses BEFORE the line is created, so the
/// wizard's placement indices address the final order. `station_comments`
/// indices are given in the FILE's order and mirrored backend-side.
#[test]
#[ignore = "needs C7: the import funnel is not ported on this branch"]
fn import_reverse_applies_before_placement_and_comments_resolve() {
    let state = new_state("Import");
    let mut config = import_config(
        "Imported",
        vec![
            shot_with_lrud(0.0, 10.0, 0.0, 2.0, 1.0),
            shot_with_lrud(90.0, 5.0, 2.0, 2.0, 10.0),
            shot_with_lrud(210.0, 7.5, 2.0, 1.0, 20.0),
        ],
        ImportPlacement::Origin,
        true,
    );
    // Station 0 of the FILE - the backend mirrors it to station 3.
    config.station_comments = vec![(0, "file station 0".to_string())];
    import(&state, vec![config]);

    let p = project(&state);
    let line = &p.lines()[0];
    // The file's last shot is now first, and its readings are reciprocated.
    assert_eq!(line.shots.len(), 3);
    assert_eq!(line.shots[0].length.to_bits(), 7.5_f64.to_bits());
    assert_eq!(line.shots[2].length.to_bits(), 10.0_f64.to_bits());
    // Left/right are untouched on the import path - there is no dialog to ask.
    assert_eq!(line.shots[0].left, Some(20.0));
    assert_eq!(line.shots[0].right, Some(20.5));
    assert_eq!(line.shots[2].left, Some(1.0));
    assert_eq!(line.shots[2].right, Some(1.5));
    // The comment lands on the physical station that is now numbered 3.
    let commented: Vec<_> = p
        .stations
        .values()
        .filter(|s| s.comment.is_some())
        .collect();
    assert_eq!(commented.len(), 1);
    assert_eq!(
        comment_at(&p, StationRef::new(line.id, 3)).as_deref(),
        Some("file station 0")
    );
}

/// Ticking the import toggle changes the numbering, never where the passage
/// lands.
#[test]
#[ignore = "needs C7: the import funnel is not ported on this branch"]
fn import_reverse_lands_the_line_in_the_same_place() {
    let shots = vec![
        shot(0.0, 10.0, 0.0, 2.0),
        shot(90.0, 5.0, 2.0, 2.0),
        shot(210.0, 7.5, 2.0, 1.0),
    ];
    let run = |reverse: bool, placement: ImportPlacement| {
        let state = new_state("Import");
        import(
            &state,
            vec![import_config("Imported", shots.clone(), placement, reverse)],
        );
        let p = project(&state);
        let line = p.lines()[0].id;
        let coords = abs_coord_map(&p);
        let mut out: Vec<Coordinates> = (0..=3u32)
            .map(|i| coords[&StationRef::new(line, i)])
            .collect();
        if reverse {
            out.reverse();
        }
        out
    };
    for placement in [
        ImportPlacement::Origin,
        ImportPlacement::Manual {
            align_station_index: 0,
            offset: Some(Coordinates {
                x: 40.0,
                y: -15.0,
                z: 0.0,
            }),
        },
    ] {
        let plain = run(false, placement.clone());
        let reversed = run(true, placement.clone());
        assert_eq!(plain.len(), reversed.len(), "station count changed");
        for (i, (a, b)) in plain.iter().zip(&reversed).enumerate() {
            assert!(
                (a.x - b.x).abs() < 1e-6 && (a.y - b.y).abs() < 1e-6,
                "station {i} moved under {placement:?}: {a:?} vs {b:?}"
            );
        }
    }
}

/// The three-shot file every placement test below imports.
fn import_shots() -> Vec<ShotData> {
    vec![
        shot(0.0, 10.0, 0.0, 2.0),
        shot(90.0, 5.0, 2.0, 2.0),
        shot(210.0, 7.5, 2.0, 1.0),
    ]
}

/// Import [`import_shots`] under `placement`, optionally reversed, beside one
/// placed line, and return the imported line's station coordinates in the
/// FILE's order.
fn import_one(reverse: bool, placement: fn(LineId) -> ImportPlacement) -> Vec<Coordinates> {
    let state = new_state("Import");
    let anchor = create(
        &state,
        "Anchor",
        vec![shot(0.0, 6.0, 0.0, 1.0), shot(90.0, 6.0, 1.0, 2.0)],
    );
    import(
        &state,
        vec![import_config(
            "Imported",
            import_shots(),
            placement(anchor),
            reverse,
        )],
    );
    let p = project(&state);
    let imported = p.lines().iter().find(|s| s.id != anchor).unwrap().id;
    let coords = abs_coord_map(&p);
    let mut out: Vec<Coordinates> = (0..=3u32)
        .map(|i| coords[&StationRef::new(imported, i)])
        .collect();
    if reverse {
        out.reverse();
    }
    out
}

/// The placements that re-anchor a line through an INDEX, which the wizard
/// hands over ALREADY MIRRORED for a reversed import: the mirrored index must
/// land on the physical station the user clicked.
#[test]
#[ignore = "needs C7: the import funnel is not ported on this branch"]
fn import_reverse_lands_indexed_placements_on_the_station_the_user_picked() {
    let cases: [(
        &str,
        fn(LineId) -> ImportPlacement,
        fn(LineId) -> ImportPlacement,
    ); 3] = [
        (
            "zero datum on the file's station 1",
            |_| ImportPlacement::ZeroDatum {
                coordinates: GpsCoordinates {
                    latitude: 20.2,
                    longitude: -87.4,
                    depth_reference: 0.0,
                },
                station_index: 1,
            },
            |_| ImportPlacement::ZeroDatum {
                coordinates: GpsCoordinates {
                    latitude: 20.2,
                    longitude: -87.4,
                    depth_reference: 0.0,
                },
                station_index: 2,
            },
        ),
        (
            "the file's station 1 merged onto an existing station",
            |anchor| ImportPlacement::FromExisting {
                import_station: 1,
                existing_station: StationRef::new(anchor, 2),
            },
            |anchor| ImportPlacement::FromExisting {
                import_station: 2,
                existing_station: StationRef::new(anchor, 2),
            },
        ),
        (
            "the file's station 0 snapped onto an existing station",
            |anchor| ImportPlacement::ManualSnap {
                align_station_index: 0,
                snapped_to_station: StationRef::new(anchor, 0),
            },
            |anchor| ImportPlacement::ManualSnap {
                align_station_index: 3,
                snapped_to_station: StationRef::new(anchor, 0),
            },
        ),
    ];
    for (label, plain_placement, reversed_placement) in cases {
        let plain = import_one(false, plain_placement);
        let reversed = import_one(true, reversed_placement);
        assert_eq!(
            plain.len(),
            reversed.len(),
            "{label}: station count changed"
        );
        for (i, (a, b)) in plain.iter().zip(&reversed).enumerate() {
            assert!(
                (a.x - b.x).abs() < 1e-6 && (a.y - b.y).abs() < 1e-6 && (a.z - b.z).abs() < 1e-6,
                "{label}: the file's station {i} moved: {a:?} vs {b:?}"
            );
        }
    }
}

/// Import a two-line batch wired together by `FromImporting`, optionally with
/// BOTH lines reversed, and return each line's stations in FILE order.
fn import_two(
    reverse: bool,
    import_station: u32,
    target_station_index: u32,
) -> (Vec<Coordinates>, Vec<Coordinates>) {
    let state = new_state("Import");
    let mut branch = import_config(
        "Branch",
        // Its far end sits at the trunk's station-1 depth.
        vec![shot(45.0, 8.0, 4.0, 3.0), shot(135.0, 4.0, 3.0, 2.0)],
        ImportPlacement::FromImporting {
            import_station,
            target_line_index: 0,
            target_station_index,
        },
        reverse,
    );
    branch.line_index = 1;
    import(
        &state,
        vec![
            import_config("Trunk", import_shots(), ImportPlacement::Origin, reverse),
            branch,
        ],
    );
    let p = project(&state);
    let coords = abs_coord_map(&p);
    let stations = |index: usize, shot_count: u32| {
        let line_id = p.lines()[index].id;
        let mut out: Vec<Coordinates> = (0..=shot_count)
            .map(|i| coords[&StationRef::new(line_id, i)])
            .collect();
        if reverse {
            out.reverse();
        }
        out
    };
    (stations(0, 3), stations(1, 2))
}

/// The cross-line connection, with BOTH lines reversed and DIFFERENT shot
/// counts: each index addresses the post-reverse numbering of its OWN line.
#[test]
#[ignore = "needs C7: the import funnel is not ported on this branch"]
fn import_reverse_connects_two_reversed_lines_by_the_stations_the_user_picked() {
    let (trunk_plain, branch_plain) = import_two(false, 2, 1);
    let (trunk_reversed, branch_reversed) = import_two(true, 0, 2);
    for (label, plain, reversed) in [
        ("trunk", &trunk_plain, &trunk_reversed),
        ("branch", &branch_plain, &branch_reversed),
    ] {
        assert_eq!(plain.len(), reversed.len(), "{label}: station count");
        for (i, (a, b)) in plain.iter().zip(reversed).enumerate() {
            assert!(
                (a.x - b.x).abs() < 1e-6 && (a.y - b.y).abs() < 1e-6 && (a.z - b.z).abs() < 1e-6,
                "{label}: the file's station {i} moved under a reversed import: {a:?} vs {b:?}"
            );
        }
    }
    // The branch's station 2 sits exactly on the trunk's station 1, both in
    // the file's numbering.
    assert_coords_close(branch_reversed[2], trunk_reversed[1]);
}

/// The mirror is `shot_count - station_index`, so an index past the last
/// station has no mirror: the comment is dropped, not parked on station 0.
#[test]
#[ignore = "needs C7: the import funnel is not ported on this branch"]
fn import_reverse_drops_an_out_of_range_station_comment() {
    let state = new_state("Import");
    let mut config = import_config(
        "Imported",
        vec![shot(0.0, 10.0, 0.0, 2.0), shot(90.0, 5.0, 2.0, 2.0)],
        ImportPlacement::Origin,
        true,
    );
    // Two shots means stations 0..2; station 7 does not exist.
    config.station_comments = vec![(7, "nowhere".to_string()), (1, "the bend".to_string())];
    import(&state, vec![config]);

    let p = project(&state);
    let line = p.lines()[0].id;
    let commented: Vec<_> = p
        .stations
        .values()
        .filter(|s| s.comment.is_some())
        .collect();
    assert_eq!(commented.len(), 1, "the out-of-range comment is dropped");
    assert_eq!(
        comment_at(&p, StationRef::new(line, 1)).as_deref(),
        Some("the bend"),
        "the in-range comment still mirrors to station 1"
    );
}
