//! Wedge healing, update_config normalization, #413 findings 26, 7, 24 and 1.
//!
//! Lines 24771-26041 of the former single-file `core/state/tests.rs` (#576 E1),
//! moved verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;
use crate::test_fixtures::{section_with_shots, shot, state_with_project, straight_shot};

// === Wedge healing: entries `undo()` cannot apply (Q8) ===

/// A stack persisted before the forward guard existed still carries the poison on
/// disk, and `load_from_persisted_with_timestamps` used to restore it verbatim:
/// `undo()` pops the `UndoData::None` entry, maps it to a failure, and the epilogue
/// pushes it straight back, so every older entry is unreachable forever.
#[test]
fn a_persisted_none_entry_does_not_block_the_history_beneath_it() {
    let state = state_with_project("Wedged");
    for name in ["First", "Second"] {
        let created = state.execute(Command::CreateSection {
            name: Some(name.to_string()),
            surveyor: None,
            date: None,
            color: "#4ecdc4".to_string(),
            direction: None,
            shots: vec![shot(5.0)],
        });
        assert!(created.success);
    }

    // Serialize the live stack, then splice the poison on top - exactly the shape
    // a pre-guard build wrote to the database.
    let mut persisted: Vec<(String, String, Option<String>)> = {
        let stack = state.undo_stack.lock();
        assert_eq!(stack.len(), 2);
        stack.rows_to_persist(PersistStack::Undo, Some(20))
    };
    let poison_timestamp = persisted
        .last()
        .and_then(|(_, _, timestamp)| timestamp.as_deref())
        .and_then(|timestamp| chrono::DateTime::parse_from_rfc3339(timestamp).ok())
        .map(|timestamp| timestamp.with_timezone(&chrono::Utc) + chrono::Duration::nanoseconds(1));
    persisted.push(
        UndoStack::serialize_entry_with_timestamp(&UndoEntry {
            replayed_at: None,
            command_bytes: None,
            command: Command::ReverseSection {
                section_id: 1,
                swap_left_right: false,
            },
            undo_data: UndoData::None,
            timestamp: poison_timestamp,
        })
        .unwrap(),
    );

    // Restore it the way `project_load.rs` does.
    state
        .undo_stack
        .lock()
        .load_from_persisted_with_timestamps(persisted, vec![]);

    let first = state.undo();
    assert!(
        first.success,
        "undo blocked by the persisted unundoable entry: {:?}",
        first.error
    );
    let second = state.undo();
    assert!(
        second.success,
        "history beneath the persisted unundoable entry was unreachable: {:?}",
        second.error
    );

    let project = state.get_project_state().unwrap();
    assert!(
        project.sections.is_empty(),
        "both sections should be undone, still have: {:?}",
        project.sections.iter().map(|s| s.id).collect::<Vec<_>>()
    );
}

/// The redo stack is deliberately NOT filtered - a `UndoData::None` redo entry
/// re-executes its command correctly. But the redo epilogue must not then move it
/// onto the undo stack, or the wedge is recreated at runtime.
#[test]
fn redoing_an_unundoable_entry_does_not_re_wedge_the_undo_stack() {
    let state = state_with_project("Redo Wedge");

    // A shot-less (waypoint-style) section, so the redone reverse is a no-op.
    let waypoint = state.execute(Command::CreateSection {
        name: Some("Waypoint".to_string()),
        surveyor: None,
        date: None,
        color: "#ff6b6b".to_string(),
        direction: None,
        shots: Vec::new(),
    });
    let waypoint_id = waypoint.section_id.unwrap();

    let real = state.execute(Command::CreateSection {
        name: Some("Real".to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    let real_id = real.section_id.unwrap();

    // The kind of entry a pre-guard build could leave on the persisted redo stack.
    {
        let mut stack = state.undo_stack.lock();
        stack.push_redo(UndoEntry {
            replayed_at: None,
            command_bytes: None,
            command: Command::ReverseSection {
                section_id: waypoint_id,
                swap_left_right: false,
            },
            undo_data: UndoData::None,
            timestamp: None,
        });
    }

    let redone = state.redo();
    assert!(redone.success, "redo failed: {:?}", redone.error);

    let undone = state.undo();
    assert!(
        undone.success,
        "the redone unundoable entry wedged the undo stack: {:?}",
        undone.error
    );
    let project = state.get_project_state().unwrap();
    assert!(
        !project.sections.iter().any(|s| s.id == real_id),
        "undo should have reached the real entry beneath"
    );
}

/// `UnmergeStations` on a station that is in no merge returns success
/// unconditionally while `generate_undo_data` yields `UndoData::None`, and it is
/// classified as affecting geometry - so the epilogue reconciles derived state with
/// no undo entry pushed. The redo branch replays onto a state the project has left,
/// so it must be cleared even though nothing was pushed.
#[test]
fn a_no_undo_data_command_still_clears_the_redo_branch() {
    let state = state_with_project("Redo Clear");

    let mut ids: Vec<SectionId> = Vec::new();
    for name in ["A", "B", "C"] {
        let created = state.execute(Command::CreateSection {
            name: Some(name.to_string()),
            surveyor: None,
            date: None,
            color: "#4ecdc4".to_string(),
            direction: None,
            shots: vec![shot(5.0)],
        });
        assert!(created.success);
        ids.push(created.section_id.unwrap());
    }

    assert!(state.undo().success);
    assert!(state.undo().success);
    assert!(state.can_redo(), "two undos should leave a redo branch");

    // Section A survived both undos, and A:0 is in no merge.
    let unmerge = state.execute(Command::UnmergeStations {
        station_ref: StationRef::new(ids[0], 0),
    });
    assert!(unmerge.success, "unmerge failed: {:?}", unmerge.error);

    assert!(
        !state.can_redo(),
        "a successful forward command must invalidate the redo branch even when it \
         produced no undo entry"
    );
}

/// A shot-less `ReverseSection` must NOT drop the redo branch (#24).
///
/// "A successful forward command invalidates redo" is the right rule for a command that did
/// something - including `UnmergeStations` on an unmerged station, which reconciles derived
/// state (see the test above). A reverse of a section with no shots is different in kind:
/// `Project::reverse_section` returns `true` without touching a field, so there is no state for
/// the redo branch to have diverged from. Dropping it - and stamping `has_unpushed_edits`, which
/// proliferates a phantom edit to SpeleoDB - was pure loss.
///
/// Guarded in the backend, not the dialog: the batch path, the raw IPC command and the agent
/// harness all reach `execute` without passing the dialog's filter.
#[test]
fn a_shotless_reverse_leaves_the_redo_branch_intact() {
    let state = state_with_project("No-op Reverse");

    let waypoint = state.execute(Command::CreateSection {
        name: Some("Waypoint".to_string()),
        surveyor: None,
        date: None,
        color: "#ff6b6b".to_string(),
        direction: None,
        shots: Vec::new(),
    });
    let waypoint_id = waypoint.section_id.unwrap();

    let real = state.execute(Command::CreateSection {
        name: Some("Real".to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        shots: vec![shot(5.0)],
    });
    assert!(real.success);

    assert!(state.undo().success);
    assert!(state.can_redo());

    let reversed = state.execute(Command::ReverseSection {
        section_id: waypoint_id,
        swap_left_right: false,
    });
    assert!(reversed.success, "reverse failed: {:?}", reversed.error);

    assert!(
        state.can_redo(),
        "a provable no-op must leave the redo branch alone"
    );
}

/// `rows_to_persist` copies the stack verbatim, so an unfiltered load would
/// write the poison straight back to disk on the next save.
#[test]
fn a_loaded_none_entry_is_not_re_persisted() {
    let state = AppState::new().unwrap();

    let persisted: Vec<(String, String, Option<String>)> = vec![
        UndoStack::serialize_entry_with_timestamp(&UndoEntry {
            replayed_at: None,
            command_bytes: None,
            command: Command::DeleteSection { section_id: 7 },
            undo_data: UndoData::InverseCommand(Command::DeleteSection { section_id: 7 }),
            timestamp: None,
        })
        .unwrap(),
        UndoStack::serialize_entry_with_timestamp(&UndoEntry {
            replayed_at: None,
            command_bytes: None,
            command: Command::ReverseSection {
                section_id: 1,
                swap_left_right: false,
            },
            undo_data: UndoData::None,
            timestamp: None,
        })
        .unwrap(),
    ];

    let mut stack = state.undo_stack.lock();
    stack.load_from_persisted_with_timestamps(persisted, vec![]);

    let to_persist = stack.rows_to_persist(PersistStack::Undo, Some(20));
    assert_eq!(to_persist.len(), 1);
    // The rows are already serialized, and a `UndoData::None` is the unit variant, so
    // it reaches the database as the bare JSON string `"None"`.
    assert!(
        !to_persist
            .iter()
            .any(|(_, undo_data_json, _)| undo_data_json == r#""None""#),
        "the poison must not be written back to disk"
    );
}

/// The load-time filter must not weaken the deliberate push-back-on-failure
/// contract: an entry that CAN be applied but fails while applying it stays on the
/// undo stack. `PreviousValues` with unparseable JSON is such an entry, and it is
/// fed through the same load path so the filter is provably in the way.
#[test]
fn a_failed_undo_still_pushes_its_entry_back() {
    let state = AppState::new().unwrap();

    let persisted: Vec<(String, String, Option<String>)> =
        vec![UndoStack::serialize_entry_with_timestamp(&UndoEntry {
            replayed_at: None,
            command_bytes: None,
            command: Command::UpdateShot {
                section_id: 1,
                shot_id: 1,
                updates: ShotUpdates::default(),
            },
            undo_data: UndoData::PreviousValues {
                section_id: 1,
                shot_id: Some(1),
                shot_uuid: None,
                values_json: "not-valid-json".to_string(),
                previous_offsets: HashMap::new(),
            },
            timestamp: None,
        })
        .unwrap()];

    {
        let mut stack = state.undo_stack.lock();
        stack.load_from_persisted_with_timestamps(persisted, vec![]);
        assert_eq!(stack.len(), 1, "an applicable entry must survive the load");
    }

    let result = state.undo();
    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some("Failed to parse undo data"));

    let stack = state.undo_stack.lock();
    assert_eq!(stack.len(), 1, "a failed undo must preserve its entry");
    assert!(
        !stack.can_redo(),
        "the entry must not move to the redo stack"
    );
}

// ========================================================================
// `AppState::update_config` normalization tests (#357 item 1:
// `normalize_except_shortcuts()` now runs on the whole-`Config` IPC write
// path, which previously normalized nothing).
// ========================================================================

#[test]
fn test_update_config_repairs_a_degenerate_whole_config_payload() {
    // Guard A: a whole-`Config` payload carrying junk in several unrelated
    // sections must be repaired on the way in, exactly as the load path
    // repairs a hand-edited config.toml - not persisted as-is and left for
    // the NEXT launch to quietly fix.
    crate::test_helpers::with_isolated_config(|| {
        let state = AppState::new().unwrap();
        let mut payload = state.get_config();
        payload.panels.sections_panel_width = 5; // out of the 150..400 bound
        payload.placement_suggestion.azimuth_sigma = f64::NAN;
        payload.general.locale = "   ".to_string(); // blank after trim

        state.update_config(payload).expect("settings saved");

        let after = state.get_config();
        assert_eq!(
            after.panels.sections_panel_width, 150,
            "an out-of-range panel width must be clamped on write, not just on the next load"
        );
        assert_eq!(
            after.placement_suggestion.azimuth_sigma,
            crate::config::default_ps_azimuth_sigma(),
            "a NaN sigma must be repaired on write"
        );
        assert_eq!(
            after.general.locale, "system",
            "a blank locale must be repaired on write to the default (\"system\")"
        );
    });
}

#[test]
fn test_update_config_leaves_an_explicitly_cleared_shortcut_empty() {
    // Guard B - the important one: `normalize_shortcut_field` reads an EMPTY
    // binding list as "key missing from the file" and re-seeds the stock
    // binding, which is right on LOAD and wrong on WRITE (the Settings UI
    // sends an empty array when the user clears the last binding for an
    // action - ShortcutsSettings.tsx). `update_config` must go through
    // `normalize_except_shortcuts`, not the full `normalize`, so a
    // deliberately cleared shortcut survives the write path.
    crate::test_helpers::with_isolated_config(|| {
        let state = AppState::new().unwrap();
        let mut payload = state.get_config();
        assert!(
            !payload.shortcuts.select.is_empty(),
            "test assumes the stock binding is non-empty by default"
        );
        payload.shortcuts.select = Vec::new();

        state.update_config(payload).expect("settings saved");

        let after = state.get_config();
        assert!(
            after.shortcuts.select.is_empty(),
            "a shortcut the user explicitly cleared must not be re-defaulted by a settings save"
        );
    });

    // The other direction, in the same test so the distinction stays pinned:
    // the LOAD path's plain `normalize()` DOES re-seed an empty binding, since
    // there an empty list can only mean "missing from the file".
    let mut loaded = crate::config::Config::default();
    loaded.shortcuts.select = Vec::new();
    loaded.normalize();
    assert_eq!(
        loaded.shortcuts.select,
        crate::config::Config::default().shortcuts.select,
        "the load path must still re-seed an empty binding read from config.toml"
    );
}

// === #413 finding 26: a shot edit must not re-georeference the survey ===

/// FINDING 26's reduction: removing the FIRST zero datum re-defines the world
/// frame, and the next geometry edit in a promoted group silently translates the
/// whole rigid group onto the new frame - unundoably.
///
/// `zero_datums[0]` IS the origin: `Project::set_zero_datum` places every later
/// datum at `gps_to_local(its gps, first.gps)`. `Project::remove_zero_datum` is
/// a bare `retain`, so dropping the first one promotes the next and moves
/// nothing - `resolve_offsets` cannot repair it either, because the cascade
/// marks datum sections AUTHORITATIVE and never re-derives their offsets. The
/// promoted group is then sitting kilometres from the position its own datum
/// implies, and nothing notices until the next shot edit, when
/// `realign_after_shot_edit` re-derives the ABSOLUTE target and yanks the group
/// onto it. That happens inside `update_shot`, whose `UndoData::PreviousValues`
/// carries the shot's fields and no offsets, so the undo cannot put the survey
/// back: it restores the shot, re-enters the same realign, and re-anchors to the
/// origin a second time.
///
/// This is the horizon-mode component-translation observation at 20 lines: the
/// fuzzer reached it at seed 2 depth 1189 (`HORIZON=1 SEEDS=2 ITERATIONS=2000`),
/// where sections 65 and 81 were both off by `(22230.09, -10480.74)` with 81 on
/// the origin. Four earlier synthetic reductions missed it because none of them
/// removed the FIRST datum, which is the whole mechanism.
///
/// Both halves are asserted: the forward edit must not move the group, and the
/// undo must restore every offset exactly.
#[test]
fn finding_26_a_shot_edit_does_not_translate_a_group_whose_first_datum_was_removed() {
    let state = state_with_project("datum frame");

    let section = |name: &str| {
        state
            .execute(Command::CreateSection {
                name: Some(name.to_string()),
                surveyor: None,
                date: None,
                color: "#111111".to_string(),
                direction: Some(SurveyDirection::In),
                shots: vec![shot(5.0), shot(5.0)],
            })
            .section_id
            .unwrap()
    };
    let anchor = section("anchor");
    let a = section("a");
    let b = section("b");

    // {a, b} is one rigid group.
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(a, 2),
                station_b: StationRef::new(b, 0),
            })
            .success
    );

    // The FIRST datum defines the world origin.
    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(anchor, 0),
                coordinates: GpsCoordinates::new(20.30, -87.45, 0.0).unwrap(),
            })
            .success
    );
    // The SECOND datum places {a, b} at gps_to_local(G2, G1) - kilometres out.
    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(a, 0),
                coordinates: GpsCoordinates::new(20.20, -87.24, 0.0).unwrap(),
            })
            .success
    );
    let placed = state.get_project_state().unwrap().section_offset(a);
    assert!(
        placed.0.abs() > 1000.0,
        "the second datum should place the group kilometres out: {placed:?}"
    );

    // Promote a's datum to first WITHOUT moving anything.
    assert!(
        state
            .execute(Command::RemoveZeroDatum {
                station_ref: StationRef::new(anchor, 0),
            })
            .success
    );

    // The promotion is no longer silent: local (0, 0) means `zero_datums[0]`'s
    // GPS, so the removal translates the survey onto the new origin and a's datum
    // station - the new `zero_datums[0]` - sits there (#469, item 1). Everything
    // below is measured against that state.
    let promoted = state
        .get_project_state()
        .unwrap()
        .compute_station_raw_coords(StationRef::new(a, 0))
        .unwrap();
    assert!(
        promoted.x.abs() < 1e-6 && promoted.y.abs() < 1e-6,
        "the promoted datum's station must sit at the local origin after the removal, got \
         ({}, {})",
        promoted.x,
        promoted.y
    );

    let before = state.get_project_state().unwrap().section_offsets().clone();
    let shot_id = state
        .get_project_state()
        .unwrap()
        .get_section(a)
        .unwrap()
        .shots[1]
        .id;

    assert!(
        state
            .execute(Command::UpdateShot {
                section_id: a,
                shot_id,
                updates: ShotUpdates {
                    length: Some(6.0),
                    ..Default::default()
                },
            })
            .success
    );

    let edited = state.get_project_state().unwrap().section_offset(a);
    let placed_before = before.get(&a).copied().unwrap_or((0.0, 0.0));
    assert!(
        (edited.0 - placed_before.0).abs() < 1.0 && (edited.1 - placed_before.1).abs() < 1.0,
        "a shot edit re-georeferenced the survey: {placed_before:?} -> {edited:?}"
    );

    assert!(state.undo().success);
    let after = state.get_project_state().unwrap().section_offsets().clone();
    for (section_id, offset) in &before {
        let now = after.get(section_id).copied().unwrap_or((0.0, 0.0));
        assert!(
            (now.0 - offset.0).abs() < 1e-6 && (now.1 - offset.1).abs() < 1e-6,
            "undo of the shot edit did not restore section {section_id}: {offset:?} -> {now:?}"
        );
    }
}

// ===========================================================================
// #413 finding 7, second half: a jump reconciles after EVERY step
// ===========================================================================

/// How many merges the project currently reports as misaligned.
fn misaligned_merges(state: &AppState) -> usize {
    let guard = state.project.lock();
    guard
        .as_ref()
        .expect("no project open")
        .check_merge_alignment()
        .len()
}

/// Two merged sections, the first anchored by a zero datum. Returns
/// `(anchored, partner)`.
fn two_merged_sections(state: &AppState) -> (SectionId, SectionId) {
    let a = section_with_shots(
        state,
        "A",
        vec![straight_shot(0.0, 5.0), straight_shot(0.0, 5.0)],
    );
    let b = section_with_shots(state, "B", vec![straight_shot(90.0, 5.0)]);
    let datum = state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    assert!(datum.success, "SetZeroDatum failed: {:?}", datum.error);
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 2),
        station_b: StationRef::new(b, 0),
    });
    assert!(merged.success, "MergeStations failed: {:?}", merged.error);
    (a, b)
}

/// Seven metres of drift written straight into the partner's offset - two
/// orders of magnitude past the alignment invariant, and far past anything the
/// command API can leave behind in a project this small.
fn inject_drift(state: &AppState, partner: SectionId) {
    let mut guard = state.project.lock();
    let project = guard.as_mut().expect("no project open");
    project.set_section_offset(partner, (7.0, 0.0));
    assert!(
        !project.check_merge_alignment().is_empty(),
        "setup: the injected offset should leave the merge misaligned"
    );
}

/// Two merged sections with drift injected into the partner, and then ONE
/// `MoveStation` recorded over the top of it.
///
/// `MoveStation` captures a `ProjectSnapshotPair` BEFORE it mutates, so its
/// `before` half holds the drifted project while its own epilogue heals the
/// live one. After this runs, the only way back to the drift is that restore -
/// which is exactly the shape a jump's INTERMEDIATE step has.
fn history_recorded_over_drift() -> (AppState, SectionId) {
    let state = state_with_project("JumpEpilogue");
    let (anchored, partner) = two_merged_sections(&state);
    inject_drift(&state, partner);
    let moved = state.execute(Command::MoveStation {
        station_ref: StationRef::new(anchored, 1),
        offset: Coordinates {
            x: 1.0,
            y: 2.0,
            z: 0.0,
        },
    });
    assert!(moved.success, "MoveStation failed: {:?}", moved.error);
    assert_eq!(
        misaligned_merges(&state),
        0,
        "setup: the command's own epilogue should have healed the live drift"
    );
    (state, partner)
}

/// An undo entry that always fails to undo, for slipping under a good one.
fn unusable_undo_entry() -> UndoEntry {
    UndoEntry {
        replayed_at: None,
        command_bytes: None,
        command: Command::UpdateShot {
            section_id: 999,
            shot_id: 999,
            updates: ShotUpdates::default(),
        },
        undo_data: UndoData::PreviousValues {
            section_id: 999,
            shot_id: Some(999),
            shot_uuid: None,
            values_json: "invalid-json".to_string(),
            previous_offsets: HashMap::new(),
        },
        timestamp: None,
    }
}

/// A jump that dies half way must still leave the steps it DID apply
/// reconciled (#413, finding 7).
///
/// `undo_to` reconciled once after its loop and returned early on a failed step
/// without ever reaching it, so a jump whose first step restored a drifted
/// snapshot and whose second step failed left the user holding a survey network
/// the app's own `debug_assert_merge_alignment` calls a violation. Pressing
/// Ctrl+Z twice never could: `undo()` reconciles per press.
#[test]
fn undo_to_reconciles_after_every_step_so_a_failed_jump_leaves_no_drift() {
    let (state, _partner) = history_recorded_over_drift();

    // Slip an unusable entry UNDER the MoveStation, so the jump completes one
    // real step (which restores the drift) and then fails on the second.
    {
        let mut stack = state.undo_stack.lock();
        let move_entry = stack.pop_undo().expect("the MoveStation entry");
        stack.push_undo(unusable_undo_entry());
        stack.push_undo(move_entry);
    }

    let result = state.undo_to(1);
    assert!(
        !result.success,
        "the second step should have failed, so the jump should report failure"
    );
    let error = result.error.as_deref().unwrap_or_default();
    assert!(
        error.contains("Undo failed at step 2 of 2"),
        "expected the step-counted message, got: {error}"
    );
    assert_eq!(
        misaligned_merges(&state),
        0,
        "the first step restored a snapshot carrying 7 m of drift and the jump then failed: \
         reconcile_geometry has to run per step, or the state the user is left holding is one \
         no sequence of single Ctrl+Z presses can produce"
    );
}

/// The redo mirror of the test above.
///
/// The pair is built by hand because a genuine redo entry's `after` half is
/// captured AFTER the forward command's own epilogue healed the project, so
/// nothing a command records can restore drift on the way forward. What the
/// fuzzer reaches - and what a persisted entry written by an older build can
/// hold - is a snapshot that carries it.
#[test]
fn redo_to_reconciles_after_every_step_so_a_failed_jump_leaves_no_drift() {
    let state = state_with_project("JumpEpilogue");
    let (_anchored, partner) = two_merged_sections(&state);
    inject_drift(&state, partner);
    let drifted = state
        .capture_project_snapshot()
        .expect("capturing the drifted project");
    state.reconcile_geometry();
    assert_eq!(
        misaligned_merges(&state),
        0,
        "setup: the reconcile should have healed the live drift"
    );
    let healed = state
        .capture_project_snapshot()
        .expect("capturing the healed project");

    {
        let mut stack = state.undo_stack.lock();
        // Bottom: an entry that cannot be redone. Top: the drift-carrying pair.
        stack.push_redo(UndoEntry {
            replayed_at: None,
            command_bytes: None,
            command: Command::CloseProject,
            undo_data: UndoData::ProjectSnapshotPair {
                before: "{}".to_string(),
                after: "not-json".to_string(),
            },
            timestamp: None,
        });
        stack.push_redo(UndoEntry {
            replayed_at: None,
            command_bytes: None,
            command: Command::MoveStation {
                station_ref: StationRef::new(partner, 0),
                offset: Coordinates {
                    x: 1.0,
                    y: 0.0,
                    z: 0.0,
                },
            },
            undo_data: UndoData::ProjectSnapshotPair {
                before: healed,
                after: drifted,
            },
            timestamp: None,
        });
    }

    let result = state.redo_to(1);
    assert!(
        !result.success,
        "the second step should have failed, so the jump should report failure"
    );
    let error = result.error.as_deref().unwrap_or_default();
    assert!(
        error.contains("Redo failed at step 2 of 2"),
        "expected the step-counted message, got: {error}"
    );
    assert_eq!(
        misaligned_merges(&state),
        0,
        "the first redo step restored a snapshot carrying 7 m of drift and the jump then \
         failed: redo_to has to reconcile per step exactly as redo() does per press"
    );
}

/// The contract itself, over a jump whose drifted state is an INTERMEDIATE one:
/// a jump lands where the same number of single presses lands.
///
/// Two `MoveStation`s over the injected drift means the step that restores it
/// still has a step above it. Both routes are built on their own copy of the
/// same project.
#[test]
fn undo_to_lands_where_the_same_number_of_single_undos_lands_over_a_drifted_step() {
    fn build() -> (AppState, SectionId, SectionId) {
        let state = state_with_project("JumpEpilogue");
        let (anchored, partner) = two_merged_sections(&state);
        inject_drift(&state, partner);
        for step in 0..2 {
            let moved = state.execute(Command::MoveStation {
                station_ref: StationRef::new(anchored, 1),
                offset: Coordinates {
                    x: 1.0,
                    y: 2.0,
                    z: 0.0,
                },
            });
            assert!(
                moved.success,
                "MoveStation {step} failed: {:?}",
                moved.error
            );
        }
        (state, anchored, partner)
    }

    let (jumped, jumped_anchored, jumped_partner) = build();
    // index 2 = three entries: both moves and the merge under them.
    let jump = jumped.undo_to(2);
    assert!(jump.success, "undo_to(2) failed: {:?}", jump.error);

    let (stepped, stepped_anchored, stepped_partner) = build();
    for step in 0..3 {
        let single = stepped.undo();
        assert!(
            single.success,
            "single undo {step} failed: {:?}",
            single.error
        );
    }

    assert_eq!(
        (jumped_anchored, jumped_partner),
        (stepped_anchored, stepped_partner),
        "the two routes were not built on the same topology, so nothing below compares anything"
    );
    let jumped_project = jumped.get_project_state().unwrap();
    let stepped_project = stepped.get_project_state().unwrap();
    assert_eq!(
        jumped_project.section_offsets(),
        stepped_project.section_offsets(),
        "a three-step undo_to left the survey somewhere three single undos do not"
    );
    assert_eq!(
        (misaligned_merges(&jumped), misaligned_merges(&stepped)),
        (0, 0),
        "a history move left the survey network broken"
    );
}

// ===========================================================================
// #413 finding 24's shape one file over: a persisted shot id is unbounded
// ===========================================================================

/// Redoing a `CreatedShotSnapshot` whose `shot_id` is `ShotId::MAX` must not
/// overflow the section's id counter.
///
/// The id arrives verbatim from a persisted entry, and the append branch bumped
/// `next_shot_id` to `id + 1` - a panic in a debug build, a wrap in a release
/// one, after which `Section::take_shot_id` hands the wrapped value straight
/// back out as an id another shot already holds.
///
/// A18(d) (#576): saturating only moved the overflow to the user's NEXT append
/// (the counter sat on `u32::MAX`). The id now goes through the section's mint,
/// which honours a recorded id only inside the section's window, so the shot
/// comes back under a fresh id there, keeping its uuid.
#[test]
fn redo_of_a_created_shot_with_the_maximum_id_remints_it_inside_the_window() {
    let state = state_with_project("JumpEpilogue");
    let section = section_with_shots(&state, "A", vec![]);

    {
        let mut stack = state.undo_stack.lock();
        stack.push_redo(UndoEntry {
            replayed_at: None,
            command_bytes: None,
            command: Command::CreateShot {
                section_id: section,
                shot: straight_shot(0.0, 5.0),
                after_shot_id: None,
                before_shot_id: None,
            },
            undo_data: UndoData::CreatedShotSnapshot {
                section_id: section,
                shot_id: ShotId::MAX,
                shot_data: straight_shot(0.0, 5.0),
                position: 0,
                station_shift_from: 0,
                after_shot_id: None,
                before_shot_id: None,
                after_shot_uuid: None,
                before_shot_uuid: None,
                uuid: Some("11111111-1111-4111-8111-111111111111".to_string()),
                previous_offsets: HashMap::new(),
            },
            timestamp: None,
        });
    }

    // Panics with "attempt to add with overflow" before the fix.
    let redone = state.redo();
    assert!(redone.success, "redo failed: {:?}", redone.error);

    let project = state.get_project_state().unwrap();
    let restored = project.get_section(section).expect("section missing");
    let base = section * crate::models::FEATURE_ID_STRIDE;
    assert_eq!(
        restored.shots.iter().map(|s| s.id).collect::<Vec<_>>(),
        vec![base],
        "the redo should have appended the recorded shot under a fresh id from its window"
    );
    assert_eq!(
        restored.shots[0].uuid.as_deref(),
        Some("11111111-1111-4111-8111-111111111111"),
        "the shot keeps the identity the entry recorded"
    );
    assert_eq!(
        restored.next_shot_id,
        base + 1,
        "the counter stays inside the window, one past the id it minted"
    );
}

/// The same unbounded id, on the UNDO side: `AppState::restore_shot` takes the
/// id verbatim out of a `ShotSnapshot` and bumps the section counter past it.
///
/// `ShotSnapshot` is the LEGACY delete-a-shot capture (a live `DeleteShot`
/// records a `ProjectSnapshotPair`), so every entry of this shape reaching the
/// restore came off disk, where nothing bounds `shot_id` - a `.stalx` can carry
/// `4294967295` in that column. `id + 1` panics a debug build there and wraps a
/// release one to 0, after which the section hands 0 out again as a fresh id.
///
/// A18(d) (#576): the restore now goes through the section's mint, so the
/// out-of-window id is replaced by a fresh one from the window and the counter
/// never leaves it.
#[test]
fn undo_of_a_shot_snapshot_with_the_maximum_id_remints_it_inside_the_window() {
    let state = state_with_project("JumpEpilogue");
    let section = section_with_shots(&state, "A", vec![straight_shot(0.0, 5.0)]);

    {
        let mut stack = state.undo_stack.lock();
        stack.push_with_data(
            Command::DeleteShot {
                section_id: section,
                shot_id: ShotId::MAX,
            },
            UndoData::ShotSnapshot {
                section_id: section,
                shot_id: ShotId::MAX,
                shot_data: straight_shot(90.0, 3.0),
                position: 1,
                uuid: Some("33333333-3333-4333-8333-333333333333".to_string()),
            },
        );
    }

    // Panics with "attempt to add with overflow" before the fix.
    let undone = state.undo();
    assert!(undone.success, "undo failed: {:?}", undone.error);

    let project = state.get_project_state().unwrap();
    let restored = project.get_section(section).expect("section missing");
    assert_eq!(
        restored.shots.len(),
        2,
        "the undo should have restored the recorded shot"
    );
    let base = section * crate::models::FEATURE_ID_STRIDE;
    assert_eq!(
        restored.shots[1].id,
        base + 1,
        "the restored shot should come back under a fresh id from its window"
    );
    assert_eq!(
        restored.shots[1].uuid.as_deref(),
        Some("33333333-3333-4333-8333-333333333333"),
        "the restored shot keeps the identity the entry recorded"
    );
    assert_eq!(
        restored.next_shot_id,
        base + 2,
        "the counter stays inside the window, one past the id it minted"
    );
}

/// The third door the same id walks through: `Section::take_shot_id`'s FORCED
/// arm, reached when the redo of a create still has an anchor to insert against.
///
/// The append branch (the test above it) does its own bump; an anchored redo
/// goes through `insert_shot_before`/`insert_shot_after` instead, and the
/// counter bump lives in `take_shot_id`. Same unbounded persisted id, same
/// `id + 1`, so it needs its own saturation and its own regression test.
///
/// A18(d) (#576): `take_shot_id` now honours a forced id only inside the
/// section's window, so the inserted shot gets a fresh id there instead.
#[test]
fn an_anchored_redo_forcing_the_maximum_shot_id_remints_it_inside_the_window() {
    let state = state_with_project("JumpEpilogue");
    let section = section_with_shots(
        &state,
        "A",
        vec![straight_shot(0.0, 5.0), straight_shot(0.0, 7.0)],
    );
    let first_id = state
        .get_project_state()
        .unwrap()
        .get_section(section)
        .expect("section missing")
        .shots[0]
        .id;

    {
        let mut stack = state.undo_stack.lock();
        stack.push_redo(UndoEntry {
            replayed_at: None,
            command_bytes: None,
            command: Command::CreateShot {
                section_id: section,
                shot: straight_shot(180.0, 4.0),
                after_shot_id: None,
                before_shot_id: Some(first_id),
            },
            undo_data: UndoData::CreatedShotSnapshot {
                section_id: section,
                shot_id: ShotId::MAX,
                shot_data: straight_shot(180.0, 4.0),
                position: 0,
                station_shift_from: 0,
                after_shot_id: None,
                // No uuid beside it: a legacy entry, so the anchor resolves by
                // the recorded id - which is what routes the redo through
                // `insert_shot_before` rather than the append branch.
                before_shot_id: Some(first_id),
                after_shot_uuid: None,
                before_shot_uuid: None,
                uuid: Some("44444444-4444-4444-8444-444444444444".to_string()),
                previous_offsets: HashMap::new(),
            },
            timestamp: None,
        });
    }

    // Panics with "attempt to add with overflow" before the fix.
    let redone = state.redo();
    assert!(redone.success, "redo failed: {:?}", redone.error);

    let project = state.get_project_state().unwrap();
    let restored = project.get_section(section).expect("section missing");
    assert_eq!(
        restored.shots.len(),
        3,
        "the redo should have inserted the recorded shot"
    );
    let base = section * crate::models::FEATURE_ID_STRIDE;
    assert_eq!(
        restored.shots[0].uuid.as_deref(),
        Some("44444444-4444-4444-8444-444444444444"),
        "the insert should have landed BEFORE the anchor"
    );
    assert_eq!(
        restored.shots[0].id,
        base + 2,
        "under a fresh id from the window, not the out-of-window forced one"
    );
    assert_eq!(
        restored.next_shot_id,
        base + 3,
        "the counter stays inside the window, one past the id it minted"
    );
}

/// A18(d) (#576): the shot the user adds AFTER such a restore must still get a
/// fresh id inside its section's window.
///
/// Saturating the counter (the three tests above) stopped the restore itself
/// from overflowing, but parked `next_shot_id` on `u32::MAX`: the next append
/// then overflowed it (`next_shot_id += 1`, a debug panic) or, in release,
/// wrapped to 0 - an id another section owns - and with the restored shot
/// already on `u32::MAX` the two could share an id.
#[test]
fn a_shot_added_after_an_out_of_window_restore_gets_a_fresh_id_in_its_window() {
    let state = state_with_project("JumpEpilogue");
    let section = section_with_shots(&state, "A", vec![straight_shot(0.0, 5.0)]);
    {
        let mut stack = state.undo_stack.lock();
        stack.push_with_data(
            Command::DeleteShot {
                section_id: section,
                shot_id: ShotId::MAX,
            },
            UndoData::ShotSnapshot {
                section_id: section,
                shot_id: ShotId::MAX,
                shot_data: straight_shot(90.0, 3.0),
                position: 1,
                uuid: Some("55555555-5555-4555-8555-555555555555".to_string()),
            },
        );
    }
    let undone = state.undo();
    assert!(undone.success, "undo failed: {:?}", undone.error);

    let created = state.execute(Command::CreateShot {
        section_id: section,
        shot: straight_shot(45.0, 2.0),
        after_shot_id: None,
        before_shot_id: None,
    });
    assert!(created.success, "create failed: {:?}", created.error);

    let project = state.get_project_state().unwrap();
    let after = project.get_section(section).expect("section missing");
    let ids: Vec<ShotId> = after.shots.iter().map(|s| s.id).collect();
    println!(
        "section {section} ids={ids:?} next_shot_id={}",
        after.next_shot_id
    );
    let base = section * crate::models::FEATURE_ID_STRIDE;
    let window = base..base + crate::models::FEATURE_ID_STRIDE;
    assert_eq!(ids.len(), 3, "the restore and the create both landed");
    assert_eq!(
        ids.iter().collect::<HashSet<_>>().len(),
        ids.len(),
        "two shots share an id: {ids:?}"
    );
    assert!(
        ids.iter().all(|id| window.contains(id)),
        "ids outside section {section}'s window {window:?}: {ids:?}"
    );
}

// ===========================================================================
// #413 finding 1's residue: only half of a CreatedSectionSnapshot was refreshed
// ===========================================================================

/// Every shot uuid in `section_id`, in order.
fn section_shot_uuids(state: &AppState, section_id: SectionId) -> Vec<Option<String>> {
    state
        .get_project_state()
        .expect("no project open")
        .get_section(section_id)
        .expect("section missing")
        .shots
        .iter()
        .map(|shot| shot.uuid.clone())
        .collect()
}

/// Every shot length in `section_id`, in order - the cheapest way to say WHICH
/// shot each uuid ended up on.
fn section_shot_lengths(state: &AppState, section_id: SectionId) -> Vec<f64> {
    state
        .get_project_state()
        .expect("no project open")
        .get_section(section_id)
        .expect("section missing")
        .shots
        .iter()
        .map(|shot| shot.length)
        .collect()
}

/// The undo of a `CreateSection` re-reads the live section, and it has to
/// re-read BOTH halves of what it re-creates.
///
/// `Section::restore_with_identity` pairs `section_data.shots` against
/// `identity.shot_uuids` POSITIONALLY. Refreshing only the identity - which is
/// what the undo did - hands the redo the shot list as the section was CREATED
/// wearing the uuids the section holds NOW, so a section whose shots moved
/// under it (an undo-less heal, or anything else with no entry of its own)
/// comes back as the original shots carrying somebody else's identity, and any
/// uuid past the end of the shorter list is dropped outright. Those uuids are
/// what every `StationKey::AfterShot` and the Ariane export resolve by.
#[test]
fn undo_of_create_section_re_reads_the_live_shots_so_redo_keeps_uuids_on_their_own_shots() {
    let state = state_with_project("JumpEpilogue");
    let section = section_with_shots(
        &state,
        "A",
        vec![straight_shot(0.0, 5.0), straight_shot(0.0, 7.0)],
    );

    // An undo-less mutation: a shot appears at the FRONT of the section with no
    // undo entry of its own, the same way a save-time heal mutates a section.
    {
        let mut guard = state.project.lock();
        let project = guard.as_mut().expect("no project open");
        let live = project.get_section_mut(section).expect("section missing");
        let id = live.next_shot_id;
        live.next_shot_id += 1;
        live.shots.insert(
            0,
            crate::models::Shot::with_uuid(
                id,
                straight_shot(180.0, 3.0),
                Some("22222222-2222-4222-8222-222222222222".to_string()),
            ),
        );
        project.reindex_section_shot_uuids(section);
    }

    let uuids_before = section_shot_uuids(&state, section);
    let lengths_before = section_shot_lengths(&state, section);
    assert_eq!(
        lengths_before,
        vec![3.0, 5.0, 7.0],
        "setup: the inserted shot should be first"
    );

    let undone = state.undo();
    assert!(undone.success, "undo failed: {:?}", undone.error);
    assert!(
        state
            .get_project_state()
            .unwrap()
            .get_section(section)
            .is_none(),
        "undo of CreateSection should have deleted the section"
    );

    let redone = state.redo();
    assert!(redone.success, "redo failed: {:?}", redone.error);

    assert_eq!(
        section_shot_lengths(&state, section),
        lengths_before,
        "the redo put back the shot list as it was CREATED, not the one the undo destroyed"
    );
    let uuids_after = section_shot_uuids(&state, section);
    assert_eq!(
        uuids_after, uuids_before,
        "a uuid landed on a different shot than the one that owned it: restore_with_identity \
         zips the recorded shots against the recorded uuids, so both halves have to come from \
         the same live section"
    );
    let unique: HashSet<Option<String>> = uuids_after.iter().cloned().collect();
    assert_eq!(
        unique.len(),
        uuids_after.len(),
        "the restored section holds a duplicate shot identity: {uuids_after:?}"
    );
}
