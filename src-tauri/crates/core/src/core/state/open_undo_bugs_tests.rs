//! Regression tests for the undo/redo bugs the agent-harness fuzzer found
//! (`e2e/agent/scenarios/undoRedoFuzz.ts`, whose header carries the per-bug
//! repro commands and root causes).
//!
//! # The rule
//!
//! A test here is committed `#[ignore]`d with the bug it pins, so the default
//! suite stays green while `cargo test -- --ignored` runs it RED. They are not
//! aspirational: each one failed on the mechanism its `#[ignore]` reason named.
//! **When a bug is fixed, delete the `#[ignore]` and the test becomes its
//! regression guard** - that is the whole point of committing them failing. The
//! doc comment then records what the fix was, so the mechanism survives even
//! though nothing reproduces it any more.
//!
//! A test that starts passing while still `#[ignore]`d means the bug was fixed
//! incidentally; un-ignore it rather than deleting it.
//!
//! **Nothing here is `#[ignore]`d today**: every bug this module pinned is
//! fixed. The next red test goes in ignored, exactly as the rule says.
//!
//! # Data
//!
//! Everything is built in code. No fixture is read, copied or derived from.

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test module: see docs/linting.md for the per-test-module expect idiom"
)]

use std::path::PathBuf;

use tempfile::TempDir;

use super::AppState;
use crate::core::Command;
use crate::models::{
    ClosureStrategy, Coordinates, GpsCoordinates, SectionId, SectionUpdates, ShotData, ShotId,
    ShotKind, ShotUpdates, StationRef,
};
// CONFIG ISOLATION. Every test in this module opens with
// `let _config = isolated_config();`: `AppState::new()` LOADS the global config,
// and every save reaches `note_recent_project` -> `Config::save()`, which WRITES
// it. Config resolution is per-thread (`crate::test_helpers` documents what that
// does and does not protect, and why it is not opt-in), so the guard is no longer
// what keeps these tests off other files; what it buys is ONE dir for the whole
// test, whose path the test can name and hand to a worker thread
// (`ConfigHandoff`). It is held for the whole test because the saves are.
use crate::test_fixtures::{state_with_project, straight_shot};
use crate::test_helpers::isolated_config;

/// Create a section with `shots` and return its id.
fn create_section(state: &AppState, name: &str, shots: Vec<ShotData>) -> SectionId {
    let result = state.execute(Command::CreateSection {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#3388FF".to_string(),
        direction: None,
        shots,
    });
    assert!(result.success, "CreateSection failed: {:?}", result.error);
    result
        .section_id
        .expect("CreateSection returned no section id")
}

/// Every shot uuid in `section_id`, in order.
fn shot_uuids(state: &AppState, section_id: SectionId) -> Vec<Option<String>> {
    let project = state.get_project_state().expect("no project open");
    project
        .sections
        .iter()
        .find(|section| section.id == section_id)
        .expect("section missing")
        .shots
        .iter()
        .map(|s| s.uuid.clone())
        .collect()
}

/// Every shot id in `section_id`, in order.
fn shot_ids(state: &AppState, section_id: SectionId) -> Vec<ShotId> {
    let project = state.get_project_state().expect("no project open");
    project
        .sections
        .iter()
        .find(|section| section.id == section_id)
        .expect("section missing")
        .shots
        .iter()
        .map(|s| s.id)
        .collect()
}

// ===========================================================================
// Bug 1 (FIXED): redo of CreateSection re-minted every shot uuid
// ===========================================================================

/// `UndoData::CreatedSectionSnapshot` stored a `SectionData`, which has no uuid
/// field, so redo went `restore_section` -> `Section::new_with_id` ->
/// `Section::new` -> `add_shot` -> `Shot::new` -> `Uuid::new_v4()`. The sibling
/// `CreateShot` path was explicitly fixed for this exact reason
/// (`UndoData::CreatedShotSnapshot { uuid }`), because a fresh uuid dangles
/// every `StationKey::AfterShot` anchor and the Ariane export identity.
///
/// FIXED, and kept as the regression guard: the entry carries a
/// `SectionIdentity` alongside its `SectionData` - the shots' uuids, the reverse
/// parity and the gauge pair - and `Section::restore_with_identity`
/// (models/section.rs) applies it, with the old mint kept as the fallback for a
/// legacy entry that has no such field.
#[test]
fn redo_of_create_section_preserves_shot_uuids() {
    let _config = isolated_config();
    let state = state_with_project("OpenUndoBugs");
    let section_id = create_section(
        &state,
        "Main",
        vec![straight_shot(0.0, 5.0), straight_shot(90.0, 4.0)],
    );

    let before = shot_uuids(&state, section_id);
    assert!(
        before.iter().all(Option::is_some),
        "setup: freshly created shots should all have uuids, got {before:?}"
    );

    assert!(state.undo().success, "undo of CreateSection failed");
    assert!(state.redo().success, "redo of CreateSection failed");

    let after = shot_uuids(&state, section_id);
    assert_eq!(
        before, after,
        "redo of CreateSection re-minted the shot uuids: every StationKey::AfterShot anchor \
         onto this section now dangles"
    );
}

/// The same root cause in a second field. `restore_section` rebuilt through
/// `Section::new`, which hardcodes `gauge_shot_uuid: None` /
/// `gauge_neighbour_uuid: None` (models/section.rs), and `SectionData` carries
/// neither - so a redo discarded the least-squares gauge pair.
///
/// The anchors only exist after a save (`capture_missing_gauge_anchors` is a
/// save-time heal), which is why the fuzzer could not see this half until it
/// started crossing the persistence boundary - and why the fix needs the second
/// half it has: the undo REFRESHES the entry's `SectionIdentity` from the live
/// section immediately before deleting it
/// (`AppState::refresh_created_section_identity`, which re-reads BOTH halves so a
/// redo puts back exactly what the undo removed), so anchors captured by a save
/// that happened after the entry was pushed are carried too.
#[test]
fn redo_of_create_section_preserves_gauge_anchors() {
    let _config = isolated_config();
    let temp = TempDir::new().unwrap();
    let state = state_with_project("OpenUndoBugs");
    let section_id = create_section(
        &state,
        "Gauged",
        vec![straight_shot(0.0, 5.0), straight_shot(90.0, 4.0)],
    );

    // The save is what mints the gauge pair.
    let saved = state.save_project(Some(temp.path().join("gauge.stalx")));
    assert!(saved.success, "save failed: {:?}", saved.error);

    let gauge_before = {
        let project = state.get_project_state().expect("no project");
        let section = project
            .sections
            .iter()
            .find(|s| s.id == section_id)
            .expect("section missing");
        (
            section.gauge_shot_uuid.clone(),
            section.gauge_neighbour_uuid.clone(),
        )
    };
    assert!(
        gauge_before.0.is_some(),
        "setup: a save should have captured a gauge anchor, got {gauge_before:?}"
    );

    assert!(state.undo().success, "undo of CreateSection failed");
    assert!(state.redo().success, "redo of CreateSection failed");

    let gauge_after = {
        let project = state.get_project_state().expect("no project");
        let section = project
            .sections
            .iter()
            .find(|s| s.id == section_id)
            .expect("section missing");
        (
            section.gauge_shot_uuid.clone(),
            section.gauge_neighbour_uuid.clone(),
        )
    };
    assert_eq!(
        gauge_before, gauge_after,
        "redo of CreateSection discarded the captured gauge anchors"
    );
}

// ===========================================================================
// Bug 5: undo of UpdateSectionMetadata cannot restore a previously-null field
// ===========================================================================

/// The capture is correct - `generate_undo_data` builds
/// `SectionUpdates { surveyor: Some(None), .. }` for a section whose surveyor
/// was null - but `SectionUpdates` (models/section.rs) has only
/// `#[serde(default)]` on `name` / `surveyor` / `date`, with no
/// `deserialize_with = "double_option"` and no `skip_serializing_if`. So
/// `Some(None)` and `None` BOTH serialize to `null` and both read back as
/// `None` = "leave it alone".
///
/// This test pins the mechanism at the exact layer the undo path uses: the
/// `values_json` round trip. `ShotUpdates` gets this right; `SectionUpdates` was
/// left out.
#[test]
fn section_updates_round_trip_preserves_clear_to_null() {
    let _config = isolated_config();
    let clear_the_surveyor = SectionUpdates {
        surveyor: Some(None),
        ..Default::default()
    };

    let json = serde_json::to_string(&clear_the_surveyor).expect("serialize SectionUpdates");
    let restored: SectionUpdates = serde_json::from_str(&json).expect("deserialize SectionUpdates");

    assert_eq!(
        restored.surveyor,
        Some(None),
        "SectionUpdates lost the difference between \"clear this field\" (Some(None)) and \
         \"leave it alone\" (None) through values_json: serialized as {json}"
    );
}

/// The end-to-end consequence of the round trip above: set a surveyor on a
/// section that had none, undo, and the value survives the undo.
#[test]
fn undo_restores_a_previously_null_surveyor() {
    let _config = isolated_config();
    let state = state_with_project("OpenUndoBugs");
    let section_id = create_section(&state, "NoSurveyor", vec![straight_shot(0.0, 5.0)]);

    let surveyor_before = {
        let project = state.get_project_state().expect("no project");
        project
            .sections
            .iter()
            .find(|s| s.id == section_id)
            .expect("section missing")
            .surveyor
            .clone()
    };
    assert_eq!(
        surveyor_before, None,
        "setup: the section should start with no surveyor"
    );

    let updated = state.execute(Command::UpdateSectionMetadata {
        section_id,
        updates: SectionUpdates {
            surveyor: Some(Some("Someone".to_string())),
            ..Default::default()
        },
    });
    assert!(updated.success, "UpdateSectionMetadata failed");

    assert!(state.undo().success, "undo of UpdateSectionMetadata failed");

    let surveyor_after = {
        let project = state.get_project_state().expect("no project");
        project
            .sections
            .iter()
            .find(|s| s.id == section_id)
            .expect("section missing")
            .surveyor
            .clone()
    };
    assert_eq!(
        surveyor_after, None,
        "undo left the new surveyor in place instead of clearing it back to null"
    );
}

// ===========================================================================
// Bug 8: after save/reload, undo deletes the WRONG shot
// ===========================================================================

/// The data-loss one.
///
/// Shots are renumbered on load to `section_id * FEATURE_ID_STRIDE +
/// sequence_order` (persistence/sqlite/read.rs), undo entries reload as opaque
/// JSON with no remapping anywhere on the restore path, and
/// `UndoData::CreatedShotSnapshot { section_id, shot_id }` undoes by calling
/// `delete_shot(section_id, shot_id)`. Because the in-session allocator
/// (`take_shot_id`) hands out ids from the SAME `section_id * STRIDE + n` space
/// the loader renumbers into, a stale id does not miss - it COLLIDES with
/// whatever shot now occupies that slot.
///
/// An insert-BEFORE into a one-shot section is the minimal way to make the
/// in-session ids diverge from positional order, which is the precondition.
#[test]
fn undo_after_reload_deletes_the_shot_it_created() {
    let _config = isolated_config();
    let temp = TempDir::new().unwrap();
    let save_path: PathBuf = temp.path().join("renumbered.stalx");

    let original_uuid;
    let inserted_uuid;
    {
        let state = state_with_project("OpenUndoBugs");
        let section_id = create_section(&state, "Diverged", vec![straight_shot(0.0, 5.0)]);
        let first_shot = shot_ids(&state, section_id)[0];

        // Insert BEFORE the existing shot: the new shot takes position 0 while
        // its id comes from the allocator, so ids no longer match positions.
        let created = state.execute(Command::CreateShot {
            section_id,
            shot: straight_shot(180.0, 7.0),
            after_shot_id: None,
            before_shot_id: Some(first_shot),
        });
        assert!(created.success, "CreateShot failed: {:?}", created.error);

        let uuids = shot_uuids(&state, section_id);
        assert_eq!(uuids.len(), 2, "setup: expected two shots");
        inserted_uuid = uuids[0].clone().expect("inserted shot has no uuid");
        original_uuid = uuids[1].clone().expect("original shot has no uuid");

        let saved = state.save_project(Some(save_path.clone()));
        assert!(saved.success, "save failed: {:?}", saved.error);
    }

    // Reopen: the loader renumbers shot ids, the persisted undo entry does not.
    let reopened = AppState::new().unwrap();
    let opened = reopened.execute(Command::OpenProject { path: save_path });
    assert!(opened.success, "reopen failed: {:?}", opened.error);

    let undone = reopened.undo();
    assert!(
        undone.success,
        "undo after reopen failed: {:?}",
        undone.error
    );

    let project = reopened.get_project_state().expect("no project after undo");
    let remaining: Vec<Option<String>> = project
        .sections
        .iter()
        .flat_map(|section| section.shots.iter().map(|s| s.uuid.clone()))
        .collect();

    assert!(
        remaining
            .iter()
            .any(|u| u.as_deref() == Some(original_uuid.as_str())),
        "undo of CreateShot destroyed the ORIGINAL shot {original_uuid} instead of the one it \
         created; surviving uuids: {remaining:?}"
    );
    assert!(
        !remaining
            .iter()
            .any(|u| u.as_deref() == Some(inserted_uuid.as_str())),
        "undo of CreateShot left the shot it created ({inserted_uuid}) in place; surviving \
         uuids: {remaining:?}"
    );
}

/// `section_offsets` for `section_id`, or `None` when the section has no entry.
fn section_offset(state: &AppState, section_id: SectionId) -> Option<(f64, f64)> {
    state
        .get_project_state()?
        .section_offsets()
        .get(&section_id)
        .copied()
}

// ===========================================================================
// Bug 2 (FIXED): undo of a shot insert left an orphan all-zero
// section_offsets entry
// ===========================================================================

/// `adjust_section_offset` CREATED the `section_offsets` key while compensating
/// an insert; the undo put the value back to zero but nothing removed the key.
/// Geometrically inert, but it meant undo was not an exact inverse and the leaked
/// key was serialized into the .stalx.
///
/// FIXED, and kept as the regression guard: `Project::write_section_offset`
/// (models/project/offsets.rs) is now THE single normalizing writer, and a value
/// within `Project::OFFSET_DUST_M` of the origin is REMOVED rather than written.
/// A present all-zero key is unrepresentable, so there is no orphan to leak -
/// which is also why no write-local rule was needed: the two identities the old
/// code had to satisfy at once (`(0 + d) - d` from an absent key must end absent,
/// from a present-zero key must end present-zero) are contradictory, and deleting
/// the second state dissolves the contradiction. Two boundaries enforce it on
/// maps this app did not write - the loader's re-solve and snapshot restore for
/// legacy undo payloads.
#[test]
fn undo_of_a_shot_insert_leaves_no_orphan_offset_entry() {
    let _config = isolated_config();
    let state = state_with_project("OpenUndoBugs");
    let section_id = create_section(
        &state,
        "Inserted",
        vec![straight_shot(0.0, 5.0), straight_shot(90.0, 4.0)],
    );
    let first_shot = shot_ids(&state, section_id)[0];

    let before = section_offset(&state, section_id);

    let created = state.execute(Command::CreateShot {
        section_id,
        shot: straight_shot(180.0, 3.0),
        after_shot_id: None,
        before_shot_id: Some(first_shot),
    });
    assert!(created.success, "CreateShot failed: {:?}", created.error);
    assert!(state.undo().success, "undo of CreateShot failed");

    let after = section_offset(&state, section_id);
    assert_eq!(
        before, after,
        "undo of a shot insert changed the section_offsets map: it was {before:?} and is now \
         {after:?} (the compensating adjust_section_offset creates the key and nothing removes it)"
    );
}

// ===========================================================================
// Bug 3: appending the FIRST shot into an EMPTY section, then undoing,
// displaces the whole section - and the displacement RATCHETS
// ===========================================================================

/// `Project::add_shot` (the append path) is offset-neutral, but the undo routes
/// through `remove_shot_with_rewire`, whose `pos == 0` branch unconditionally
/// calls `adjust_section_offset`. With one shot the appended shot IS index 0, so
/// the delete slides the section by that shot's horizontal vector.
///
/// FIXED: undo of `CreateShot` no longer reuses the general delete for the one
/// shape it is not the inverse of. `AppState::remove_created_shot`
/// (`core/state/command_shot.rs`) recognises the recorded append-into-an-empty-
/// section shape (both anchors absent, position 0, and still the section's sole
/// shot) and calls `Project::remove_appended_first_shot`
/// (`models/project/shots.rs`), which removes the station the append CREATED -
/// station 1, `AfterShot(uuid)` - and leaves `section_offsets` and station 0's
/// refs untouched, exactly as offset-neutral in reverse as `add_shot` is
/// forward.
#[test]
fn undo_of_the_first_shot_in_an_empty_section_does_not_move_it() {
    let _config = isolated_config();
    let state = state_with_project("OpenUndoBugs");
    let section_id = create_section(&state, "Empty", vec![]);

    let before = section_offset(&state, section_id).unwrap_or((0.0, 0.0));

    let created = state.execute(Command::CreateShot {
        section_id,
        shot: straight_shot(90.0, 10.0),
        after_shot_id: None,
        before_shot_id: None,
    });
    assert!(created.success, "CreateShot failed: {:?}", created.error);
    assert!(state.undo().success, "undo of CreateShot failed");

    let after = section_offset(&state, section_id).unwrap_or((0.0, 0.0));
    assert_eq!(
        before, after,
        "undoing the first shot appended into an empty section moved the section from {before:?} \
         to {after:?}"
    );
}

/// The displacement is not a one-off: redo re-inserts without giving the offset
/// back, so every undo/redo cycle slides the section again.
///
/// FIXED with the test above: with nothing taken on the undo there is nothing
/// for the offset-neutral redo to give back, so the cycle is the identity.
#[test]
fn the_empty_section_displacement_does_not_ratchet() {
    let _config = isolated_config();
    let state = state_with_project("OpenUndoBugs");
    let section_id = create_section(&state, "Ratchet", vec![]);

    let created = state.execute(Command::CreateShot {
        section_id,
        shot: straight_shot(90.0, 10.0),
        after_shot_id: None,
        before_shot_id: None,
    });
    assert!(created.success, "CreateShot failed: {:?}", created.error);

    assert!(state.undo().success, "first undo failed");
    let after_one = section_offset(&state, section_id).unwrap_or((0.0, 0.0));

    assert!(state.redo().success, "redo failed");
    assert!(state.undo().success, "second undo failed");
    let after_two = section_offset(&state, section_id).unwrap_or((0.0, 0.0));

    assert_eq!(
        after_one, after_two,
        "a second undo/redo cycle moved the section again ({after_one:?} -> {after_two:?}): the \
         displacement compounds without bound"
    );
}

/// The ids of every loop closure, in `Project::loop_closures` order.
fn closure_ids(state: &AppState) -> Vec<u64> {
    let project = state.get_project_state().expect("no project open");
    project.loop_closures.iter().map(|c| c.id).collect()
}

/// Two sections joined by a merge, with a zero datum anchoring the first, so a
/// loop closure between them is legal. Returns `(section_a, section_b)`.
fn two_merged_sections(state: &AppState) -> (SectionId, SectionId) {
    let a = create_section(
        state,
        "A",
        vec![straight_shot(0.0, 5.0), straight_shot(0.0, 5.0)],
    );
    let b = create_section(state, "B", vec![straight_shot(90.0, 5.0)]);
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

// ===========================================================================
// Bug 4 (FIXED): undo of RemoveLoopClosure appended instead of reinserting
// ===========================================================================

/// The removal takes the closure out with `Vec::remove(pos)`; the undo put it
/// back with `loop_closures.push()` (core/state/undo.rs). Same closure, same id,
/// new position - and the order is persisted to the .stalx, indexed by the
/// frontend, and used to build the adjacency lists `resolve_offsets` traverses.
///
/// FIXED, and kept as the regression guard: `UndoData::ClosureInfo` records the
/// index the removal took it out of and the undo re-inserts there, with `push`
/// kept for a legacy entry that has no recorded position - the same shape
/// `restore_shot` already used.
#[test]
fn undo_of_remove_loop_closure_restores_the_original_order() {
    let _config = isolated_config();
    let state = state_with_project("OpenUndoBugs");
    let (a, b) = two_merged_sections(&state);

    let first = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(a, 0),
        station_b: StationRef::new(b, 1),
        closure_strategy: ClosureStrategy::default(),
    });
    assert!(
        first.success,
        "first CreateLoopClosure failed: {:?}",
        first.error
    );
    let second = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 1),
        closure_strategy: ClosureStrategy::default(),
    });
    assert!(
        second.success,
        "second CreateLoopClosure failed: {:?}",
        second.error
    );

    let before = closure_ids(&state);
    assert_eq!(
        before.len(),
        2,
        "setup: expected two closures, got {before:?}"
    );
    let removed_id = before[0];

    let removed = state.execute(Command::RemoveLoopClosure {
        closure_id: removed_id,
    });
    assert!(
        removed.success,
        "RemoveLoopClosure failed: {:?}",
        removed.error
    );
    assert!(state.undo().success, "undo of RemoveLoopClosure failed");

    let after = closure_ids(&state);
    assert_eq!(
        before, after,
        "undo put closure {removed_id} back at the END of loop_closures instead of its original \
         index; the list order is persisted and is what resolve_offsets traverses"
    );
}

// ===========================================================================
// Bug 6: a CreateShot that is permitted forward but CANNOT be undone
// ===========================================================================

/// Undo of `CreateShot` runs the user-facing `delete_shot`, guards and all, so a
/// shot inserted into a datum-anchored loop is an action that cannot be undone.
/// `core/state/undo.rs` pushes the entry back on failure, so it fails the same
/// way forever and every entry beneath it is unreachable - a WEDGED history,
/// which is worse than any amount of drift.
///
/// FIXED: the undo arm calls `AppState::remove_created_shot`
/// (`core/state/command_shot.rs`), which keeps only the structural rewire
/// (`Project::remove_shot_with_rewire`) and consults NO user-intent guard.
/// Special-casing the loop guard would not have been the fix - `delete_shot`'s
/// frozen-shot check runs before it and wedges the history identically, and it
/// is the one the Rust fuzzer hits most. The guards themselves are untouched
/// and still refuse a USER delete; see
/// `the_delete_guards_still_refuse_a_user_delete` below.
#[test]
fn undo_of_create_shot_in_an_anchored_loop_succeeds() {
    let _config = isolated_config();
    let state = state_with_project("OpenUndoBugs");
    let (a, b) = two_merged_sections(&state);

    // Close the loop. It has to be a CLOSURE, not a second merge: A and B are
    // already connected, and MergeStations refuses that with "Stations are
    // already connected. Use Create Loop Closure instead." (A second zero datum
    // is refused for the same component too, which is why the precondition here
    // is the loop itself - matching the guard the fuzzer's naharon soak hit.)
    let closed = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(a, 0),
        station_b: StationRef::new(b, 1),
        closure_strategy: ClosureStrategy::default(),
    });
    assert!(
        closed.success,
        "loop-closing closure failed: {:?}",
        closed.error
    );

    // MID-CHAIN, not appended: an appended shot ends at a fresh leaf station,
    // which is not part of the cycle, so its removal is unguarded. A shot
    // inserted inside A's chain has both endpoints on the loop, which is the
    // condition `delete_shot` refuses.
    let first_shot_of_a = shot_ids(&state, a)[0];
    let created = state.execute(Command::CreateShot {
        section_id: a,
        shot: straight_shot(45.0, 3.0),
        after_shot_id: Some(first_shot_of_a),
        before_shot_id: None,
    });
    assert!(
        created.success,
        "setup: the forward CreateShot must be permitted for this test to mean anything: {:?}",
        created.error
    );

    let undone = state.undo();
    assert!(
        undone.success,
        "undo of a PERMITTED CreateShot was refused ({:?}); the entry is pushed back, so the \
         history is now wedged and nothing beneath it can ever be undone. A second undo fails \
         identically: {:?}",
        undone.error,
        state.undo().error
    );
}

/// The other half of bug 6's fix: taking the guards off the UNDO path must not
/// take them off the USER's. `Command::DeleteShot` still runs `delete_shot`,
/// both refusals intact.
///
/// Both are exercised, because bug 6's write-up quoted only one and the fuzzer
/// hits the other more often. ALIGNMENT is what separates them: an aligned
/// closure rigidifies the loop, so both endpoints of a shot on it reach the
/// datum and the shot is FROZEN; an unaligned one leaves the shot movable, and
/// the shift-blocked loop/datum pair behind the frozen check is what refuses.
///
/// The `Move` row snaps `(b, 1)` onto the datum station first, because
/// `is_aligned` is a MEASUREMENT now (#469 item 2) rather than a property of the
/// strategy alone: `two_merged_sections` leaves those two stations 11 m apart,
/// and a closure across that gap is not a coincidence claim anybody would
/// believe. Snapping is also what the real workflow does - the strategy is
/// called `Move` because the frontend sends it only behind the move commands
/// that made the two stations one point.
#[test]
fn the_delete_guards_still_refuse_a_user_delete() {
    for (strategy, expected) in [
        (
            ClosureStrategy::Move,
            "Cannot delete a frozen shot. This shot is part of an adjusted survey network.",
        ),
        (ClosureStrategy::Batch, "Use split instead."),
    ] {
        let state = state_with_project("OpenUndoBugs");
        let (a, b) = two_merged_sections(&state);

        // The insert comes FIRST here, before the loop is closed. It lengthens
        // the loop by 3 m, which a closure across it cannot absorb - so closing
        // afterwards is the only order in which the `Move` row's premise (an
        // aligned, i.e. genuinely coincident, closure) survives to the delete.
        let first_shot_of_a = shot_ids(&state, a)[0];
        let created = state.execute(Command::CreateShot {
            section_id: a,
            shot: straight_shot(45.0, 3.0),
            after_shot_id: Some(first_shot_of_a),
            before_shot_id: None,
        });
        assert!(
            created.success,
            "setup ({strategy:?}): CreateShot failed: {:?}",
            created.error
        );
        let inserted = shot_ids(&state, a)[1];

        if strategy == ClosureStrategy::Move {
            // Snap the far end onto the datum station, measured off the geometry
            // as it stands - which is what the frontend's snap-to-merge does
            // before it sends this strategy.
            let (from, to) = station_raw_xy(&state, (b, 1), (a, 0));
            let snapped = state.execute(Command::MoveStation {
                station_ref: StationRef::new(b, 1),
                offset: Coordinates {
                    x: to.0 - from.0,
                    y: to.1 - from.1,
                    z: 0.0,
                },
            });
            assert!(
                snapped.success,
                "setup ({strategy:?}): snap-to-merge move failed: {:?}",
                snapped.error
            );
        }
        let closed = state.execute(Command::CreateLoopClosure {
            station_a: StationRef::new(a, 0),
            station_b: StationRef::new(b, 1),
            closure_strategy: strategy,
        });
        assert!(
            closed.success,
            "setup ({strategy:?}): loop-closing closure failed: {:?}",
            closed.error
        );
        assert_eq!(
            state
                .get_project_state()
                .and_then(|project| project.loop_closures.first().map(|c| c.is_aligned)),
            Some(strategy == ClosureStrategy::Move),
            "setup ({strategy:?}): the row's premise is the closure's alignment"
        );

        let deleted = state.execute(Command::DeleteShot {
            section_id: a,
            shot_id: inserted,
        });
        assert!(
            !deleted.success,
            "a USER delete of a shot on an anchored loop ({strategy:?}) was PERMITTED; the guard \
             undo now skips must still refuse the user"
        );
        let error = deleted.error.unwrap_or_default();
        assert!(
            error.contains(expected),
            "the {strategy:?} delete was refused with {error:?}, which does not mention \
             {expected:?} - the guard that fired is not the one this row pins"
        );
    }
}

// ===========================================================================
// Bug 7: PINNED at last - by a seed, in the fuzz module
// ===========================================================================
//
// `undo()`/`redo()` call `invalidate_adjusted_cache()` before and
// `reconcile_geometry()` after (core/state/undo.rs); `undo_to()`/`redo_to()`
// loop the `*_inner` variants and call NEITHER.
//
// The red test is `bug_7_a_jump_is_n_single_steps` in
// `src-tauri/src/core/state/fuzz_undo_tests.rs`. It does not pattern-match a
// shape: on a jump that diverges the fuzzer REPLAYS the same distance one step
// at a time, through the same `undo()`/`redo()` entry points a user's Ctrl+Z
// would, and records the finding only when the single-step path lands exactly
// on the state the model recorded while the jump did not. That is the contract
// "undo_to(N) == N x undo()" measured directly.
//
// CORRECTION to the mechanism as previously documented. The missing
// `invalidate_adjusted_cache()` is NOT observable: `get_adjusted_stations`
// (core/state/app_state.rs) keys its cache on `project.modified_at` plus a
// config hash, and every undo calls `Project::touch()`, so the next read misses
// and re-solves whether or not the cache was explicitly cleared. Only the
// missing `reconcile_geometry()` half is a real defect.
//
// WHY IT IS NOT PINNED HERE, which is the part worth keeping. It needs an undo
// whose RAW restore leaves a merge beyond MERGE_ALIGNMENT_TOLERANCE_M, and none
// of the drift sources reachable in a small synthetic project qualifies. Each
// was measured, not assumed:
//
//   * bug 3's empty-section displacement DROPPED the merge it would misalign -
//     the `pos == 0` branch removed refs keyed to station 0, and an empty
//     section has no other station to merge on. (Measured while bug 3 was open.
//     It is fixed now and there is no displacement at all, so this route is
//     doubly unavailable.)
//   * `UpdateShot` on a merged section restores the offsets symmetrically: both
//     unwind paths land on the same offset (verified by instrumenting).
//   * an insert-before on a section merged at a SURVIVING station compensates
//     correctly - the offset returns to (0, 0) and `check_merge_alignment()`
//     stays empty on both paths.
//   * undo-of-`RepairIntegrity`, whose `before` snapshot is documented to carry
//     the drift Repair healed and which therefore RELIES on the epilogue. Two
//     repairable shapes were built: bug 3's displaced section (repair returns
//     `changed = false`, so no entry is pushed at all) and finding 10's orphan
//     closure (repair fires, but `undo()` and `undo_to(0)` land on identical
//     projects - `realign_merges` early-returns while `check_merge_alignment()`
//     is empty, and it is empty in both).
//
// That is the shape of the difficulty: `realign_merges` IS `resolve_offsets`
// behind a tolerance gate, and every command's own epilogue has already run it,
// so a persistently misaligned merge is not reachable through the command API in
// a small project. It needs drift produced by ANOTHER bug over a long history -
// which the fuzzer reaches and minimal synthesis does not. Hence a seed.
//
// ===========================================================================
// Finding 10: a delete path strands a loop closure without converting it,
// so save + reopen is not state-preserving
// ===========================================================================

/// The app's invariant is that a loop closure exists only for a TRUE loop; a
/// closure that has become the sole connection is "really" a merge, and
/// `convert_orphan_closures_to_merges` normalizes it. That conversion runs from
/// `execute()`'s epilogue for commands flagged `can_orphan_closures`
/// (core/state/commands.rs), from two of the four delete-family entry points
/// (`execute_delete_shot_and_split`, `execute_delete_shot_range_split`), and
/// unconditionally on LOAD from `redrive_offsets_after_load`.
///
/// It did NOT run from `execute_delete_last_shot_and_section`
/// (command_delete_split.rs), a separate entry point rather than a `Command`
/// variant, so it never saw the `execute()` epilogue either. Deleting the
/// section that carried the loop's other path left an orphan closure in memory,
/// the save persisted it, and the next load silently reclassified it as a merge:
/// `(1 closure, 0 merges)` saved, `(0 closures, 1 merge)` reopened.
///
/// RESOLVED as option 1 of the write-up - make the four delete entry points
/// consistent. `execute_delete_last_shot_and_section` now converts (and then
/// reconciles, so a closure that just became a rigid merge is solved rather than
/// left drifted), which is what its two split siblings already did.
/// `execute_delete_shot_range_shift` needed no call of its own: it removes the
/// range one shot at a time through `remove_shot_with_rewire_excluding_anchors`,
/// which converts after every removal. Memory and disk now agree and the
/// loader's pass is a no-op safety net.
///
/// Undo of these deletes restores from the `ProjectSnapshotPair`, which puts the
/// PRE-conversion state back - correctly: the closure was a true loop before the
/// delete, so restoring it as a closure is the inverse, not a missed conversion.
///
/// Derived from the fuzzer's naharon seed 26 x40, whose history contains
/// `deleteLastShotAndSection` before the reload that diverged.
#[test]
fn save_and_reopen_preserves_an_orphan_loop_closure() {
    let _config = isolated_config();
    let temp = TempDir::new().unwrap();
    let save_path: PathBuf = temp.path().join("orphan-closure.stalx");

    let before = {
        let state = state_with_project("OpenUndoBugs");
        // A - bridge - B, plus a closure straight from A to B: a genuine loop
        // with two independent paths.
        let a = create_section(&state, "A", vec![straight_shot(0.0, 5.0)]);
        let bridge = create_section(&state, "Bridge", vec![straight_shot(90.0, 5.0)]);
        let b = create_section(&state, "B", vec![straight_shot(180.0, 5.0)]);
        let datum = state.execute(Command::SetZeroDatum {
            station_ref: StationRef::new(a, 0),
            coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
        });
        assert!(datum.success, "SetZeroDatum failed: {:?}", datum.error);
        for (left, right) in [((a, 1), (bridge, 0)), ((bridge, 1), (b, 0))] {
            let merged = state.execute(Command::MergeStations {
                station_a: StationRef::new(left.0, left.1),
                station_b: StationRef::new(right.0, right.1),
            });
            assert!(merged.success, "MergeStations failed: {:?}", merged.error);
        }
        let closure = state.execute(Command::CreateLoopClosure {
            station_a: StationRef::new(a, 0),
            station_b: StationRef::new(b, 1),
            closure_strategy: ClosureStrategy::default(),
        });
        assert!(
            closure.success,
            "CreateLoopClosure failed: {:?}",
            closure.error
        );

        // Delete the bridge through the entry point that does NOT convert.
        let bridge_shot = shot_ids(&state, bridge)[0];
        let deleted = state.execute_delete_last_shot_and_section(bridge, bridge_shot);
        assert!(
            deleted.success,
            "delete_last_shot_and_section failed: {:?}",
            deleted.error
        );

        let project = state.get_project_state().expect("no project");
        let counts = (project.loop_closures.len(), project.station_merges.len());
        // FINDING 10, RESOLVED as option 1 (make the four delete entry points
        // consistent): the delete now converts the orphan closure in memory, so the
        // precondition this line pins is the CONVERSION, not the orphan. Before the
        // fix this read `assert!(counts.0 > 0, "setup: the closure should still be
        // present in memory after the delete")` - the state the save then persisted
        // and the loader silently reclassified. The comparison below is unchanged
        // and is still the oracle: whatever the delete leaves in memory has to
        // survive a save + reopen unchanged.
        assert_eq!(
            counts,
            (0, 1),
            "setup: the delete path must convert the orphan closure to a merge in memory \
             (issue #413 finding 10, option 1), got {counts:?}"
        );

        let saved = state.save_project(Some(save_path.clone()));
        assert!(saved.success, "save failed: {:?}", saved.error);
        counts
    };

    let reopened = AppState::new().unwrap();
    let opened = reopened.execute(Command::OpenProject { path: save_path });
    assert!(opened.success, "reopen failed: {:?}", opened.error);

    let project = reopened
        .get_project_state()
        .expect("no project after reopen");
    assert_eq!(
        (project.loop_closures.len(), project.station_merges.len()),
        before,
        "save + reopen reclassified a loop closure as a station merge: the delete path that \
         orphaned it does not run convert_orphan_closures_to_merges, but the loader does"
    );
}

// ===========================================================================
// Finding 11 (FIXED): a save's undo-less heals were reverted by undoing a
// snapshot-pair entry recorded before it
// ===========================================================================

/// FIXED, and kept as the regression guard. Two halves, because the residue
/// outlived the first one: `reapply_heals_after_restore`
/// (core/state/project_io.rs) brings a restored project up to the standard a
/// saved one is held to (which heals it re-applies, and which it deliberately
/// does not, is argued heal by heal on that function), and snapshot PAIRS are
/// self-refreshing walls on top of that - undo re-records the entry's `after`
/// half from the live project before restoring `before`, redo symmetrically, so
/// pair undo/redo are exact inverses by construction even when the restored half
/// re-creates a section that has no live counterpart to copy a heal from.
///
/// `save_project` HEALS live state with no undo entry of its own -
/// `ensure_shot_uuids`, `ensure_unique_shot_uuids`,
/// `capture_missing_gauge_anchors`, the orphan `section_offsets` retain
/// (`core/state/project_io.rs`). A `ProjectSnapshotPair` captured BEFORE that
/// save holds the pre-heal project in BOTH its `before` and its `after`, and
/// neither is refreshed. So undoing (or redoing) an unrelated command recorded
/// before the save silently reverts the heal.
///
/// The consequence is worse than losing the anchors, which the next save would
/// re-capture: it makes the project at a given undo depth depend on the ROUTE
/// taken to reach it. Land on depth N by saving there and it has the anchors;
/// land on the same depth N by undoing a pre-save entry and it does not. An undo
/// stack whose states are not well-defined is the thing the whole history
/// feature promises.
///
/// Found by the fuzzer at fractal seed 4 x60 STRICT on the v2 stream, where a
/// save at depth 1 was followed by a redo/undo of a `moveStation` recorded
/// before it.
#[test]
fn undo_of_a_pre_save_entry_keeps_the_saves_gauge_anchors() {
    let _config = isolated_config();
    let temp = TempDir::new().unwrap();
    let state = state_with_project("OpenUndoBugs");
    let section_id = create_section(
        &state,
        "Gauged",
        vec![straight_shot(0.0, 5.0), straight_shot(90.0, 4.0)],
    );

    let gauge_of = |s: &AppState| -> (Option<String>, Option<String>) {
        let project = s.get_project_state().expect("no project");
        let section = project
            .sections
            .iter()
            .find(|sec| sec.id == section_id)
            .expect("section missing");
        (
            section.gauge_shot_uuid.clone(),
            section.gauge_neighbour_uuid.clone(),
        )
    };

    // A command whose undo data is a ProjectSnapshotPair, recorded BEFORE the
    // save so both of its snapshots predate the heal.
    let moved = state.execute(Command::MoveStation {
        station_ref: StationRef::new(section_id, 1),
        offset: Coordinates {
            x: 1.0,
            y: 2.0,
            z: 0.0,
        },
    });
    assert!(moved.success, "MoveStation failed: {:?}", moved.error);
    assert_eq!(
        gauge_of(&state),
        (None, None),
        "setup: the gauge pair is only captured on save, so it should still be unset"
    );

    // The save mints the gauge pair, with no undo entry of its own.
    let saved = state.save_project(Some(temp.path().join("pre-save-entry.stalx")));
    assert!(saved.success, "save failed: {:?}", saved.error);
    let after_save = gauge_of(&state);
    assert!(
        after_save.0.is_some(),
        "setup: the save should have captured a gauge anchor, got {after_save:?}"
    );

    // Undo the move recorded BEFORE the save.
    assert!(state.undo().success, "undo of MoveStation failed");

    assert_eq!(
        gauge_of(&state),
        after_save,
        "undoing a command recorded BEFORE the save reverted the save's gauge capture: the \
         snapshot pair holds the pre-heal project in both halves, so the heal is lost and the \
         project at this depth now depends on the route taken to reach it"
    );
}

/// The kind of every shot in `section_id`, in order.
fn shot_kinds(state: &AppState, section_id: SectionId) -> Vec<ShotKind> {
    let project = state.get_project_state().expect("no project open");
    project
        .sections
        .iter()
        .find(|section| section.id == section_id)
        .expect("section missing")
        .shots
        .iter()
        .map(|s| s.kind)
        .collect()
}

/// The length of every shot in `section_id`, in order.
fn shot_lengths(state: &AppState, section_id: SectionId) -> Vec<f64> {
    let project = state.get_project_state().expect("no project open");
    project
        .sections
        .iter()
        .find(|section| section.id == section_id)
        .expect("section missing")
        .shots
        .iter()
        .map(|s| s.length)
        .collect()
}

/// Build a section whose shot ids no longer match positional order, save it, and
/// return the path plus the section id.
///
/// Three shots and one insert-BEFORE: the new shot takes position 0 while its id
/// comes from the allocator, so after the save the loader's
/// `section_id * FEATURE_ID_STRIDE + sequence_order` renumbering moves every id
/// by one slot. No shot is DELETED, so the ids the allocator handed out are
/// exactly the ids the loader renumbers into - which is what makes a stale id
/// COLLIDE with a live shot rather than miss it (the miss case is its own test
/// below).
fn diverged_section(state: &AppState) -> SectionId {
    let section_id = create_section(
        state,
        "Diverged",
        vec![
            straight_shot(0.0, 5.0),
            straight_shot(90.0, 6.0),
            straight_shot(180.0, 7.0),
        ],
    );
    let first = shot_ids(state, section_id)[0];
    let created = state.execute(Command::CreateShot {
        section_id,
        shot: straight_shot(270.0, 8.0),
        after_shot_id: None,
        before_shot_id: Some(first),
    });
    assert!(created.success, "CreateShot failed: {:?}", created.error);
    let ids = shot_ids(state, section_id);
    assert_ne!(
        ids,
        (0..ids.len())
            .map(|i| section_id * crate::models::FEATURE_ID_STRIDE + i as u32)
            .collect::<Vec<_>>(),
        "setup: the ids must no longer match positional order, or the reload changes nothing"
    );
    section_id
}

// ===========================================================================
// Finding 12: bug 8's mechanism is NOT confined to CreateShot - after a
// save/reload, undo of SetShotKind or UpdateShot writes to the WRONG SHOT
// ===========================================================================

/// `UndoData::InverseCommand(Command::SetShotKind { shot_id, .. })` names the
/// shot by its IN-SESSION id (`undo_capture.rs`), and undo executes that command
/// verbatim (`execute_without_undo`, core/state/undo.rs). The loader renumbers
/// shots to `section_id * FEATURE_ID_STRIDE + sequence_order`
/// (persistence/sqlite/read.rs) and nothing remaps the persisted entries, so
/// after a reopen the id addresses whichever shot now occupies that slot.
///
/// Quieter than bug 8 and just as wrong: no shot is lost, no uuid changes and no
/// count moves. One shot silently keeps an edit the undo was supposed to take
/// away, and another silently gains one it never had.
///
/// Found by `undo-persistence-fuzz` (e2e/agent/scenarios/undoPersistenceFuzz.ts).
#[test]
fn undo_of_set_shot_kind_after_reload_hits_the_shot_it_changed() {
    let _config = isolated_config();
    let temp = TempDir::new().unwrap();
    let save_path: PathBuf = temp.path().join("wrong-shot-kind.stalx");

    let marked_uuid;
    let section_id;
    {
        let state = state_with_project("OpenUndoBugs");
        section_id = diverged_section(&state);

        // Mark the shot at POSITION 0, whose id is the highest the allocator
        // handed out - so the renumbering will move it to the lowest.
        let target = shot_ids(&state, section_id)[0];
        marked_uuid = shot_uuids(&state, section_id)[0]
            .clone()
            .expect("shot has no uuid");
        let marked = state.execute(Command::SetShotKind {
            section_id,
            shot_id: target,
            kind: ShotKind::Jump,
        });
        assert!(marked.success, "SetShotKind failed: {:?}", marked.error);
        assert_eq!(
            shot_kinds(&state, section_id)[0],
            ShotKind::Jump,
            "setup: the forward command should have marked position 0"
        );

        let saved = state.save_project(Some(save_path.clone()));
        assert!(saved.success, "save failed: {:?}", saved.error);
    }

    let reopened = AppState::new().unwrap();
    let opened = reopened.execute(Command::OpenProject { path: save_path });
    assert!(opened.success, "reopen failed: {:?}", opened.error);

    let undone = reopened.undo();
    assert!(
        undone.success,
        "undo after reopen failed: {:?}",
        undone.error
    );

    let project = reopened.get_project_state().expect("no project after undo");
    let section = project
        .sections
        .iter()
        .find(|s| s.id == section_id)
        .expect("section missing");
    let marked = section
        .shots
        .iter()
        .find(|s| s.uuid.as_deref() == Some(marked_uuid.as_str()))
        .expect("the marked shot vanished");
    assert_eq!(
        marked.kind,
        ShotKind::Normal,
        "undo of SetShotKind left the shot it changed as {:?} and reset a DIFFERENT shot instead: \
         kinds are now {:?} (the entry named the in-session shot id, which the loaders \
         renumbering has since reassigned)",
        marked.kind,
        shot_kinds(&reopened, section_id)
    );
}

/// The same defect through `UndoData::PreviousValues { section_id, shot_id, .. }`,
/// whose undo calls `update_shot(section_id, shot_id, prev)` with the stale id
/// (core/state/undo.rs). `redo_inner`'s catch-all arm re-executes
/// `entry.command` - `Command::UpdateShot { shot_id, .. }` - so the REDO half
/// carries the same stale id, which is what makes an undo/redo cycle after a
/// reload move a measurement onto a shot that never had it.
#[test]
fn undo_of_update_shot_after_reload_restores_the_shot_it_changed() {
    let _config = isolated_config();
    let temp = TempDir::new().unwrap();
    let save_path: PathBuf = temp.path().join("wrong-shot-length.stalx");

    let edited_uuid;
    let original_length;
    let section_id;
    {
        let state = state_with_project("OpenUndoBugs");
        section_id = diverged_section(&state);

        let target = shot_ids(&state, section_id)[0];
        edited_uuid = shot_uuids(&state, section_id)[0]
            .clone()
            .expect("shot has no uuid");
        original_length = shot_lengths(&state, section_id)[0];
        let edited = state.execute(Command::UpdateShot {
            section_id,
            shot_id: target,
            updates: ShotUpdates {
                length: Some(42.5),
                ..Default::default()
            },
        });
        assert!(edited.success, "UpdateShot failed: {:?}", edited.error);

        let saved = state.save_project(Some(save_path.clone()));
        assert!(saved.success, "save failed: {:?}", saved.error);
    }

    let reopened = AppState::new().unwrap();
    let opened = reopened.execute(Command::OpenProject { path: save_path });
    assert!(opened.success, "reopen failed: {:?}", opened.error);

    let undone = reopened.undo();
    assert!(
        undone.success,
        "undo after reopen failed: {:?}",
        undone.error
    );

    let project = reopened.get_project_state().expect("no project after undo");
    let section = project
        .sections
        .iter()
        .find(|s| s.id == section_id)
        .expect("section missing");
    let edited = section
        .shots
        .iter()
        .find(|s| s.uuid.as_deref() == Some(edited_uuid.as_str()))
        .expect("the edited shot vanished");
    assert!(
        (edited.length - original_length).abs() < 1e-9,
        "undo of UpdateShot left the edited shot at {} m instead of restoring {} m, and wrote the \
         old value onto a DIFFERENT shot: lengths are now {:?}",
        edited.length,
        original_length,
        shot_lengths(&reopened, section_id)
    );
}

// ===========================================================================
// Finding 13: when the stale id is PAST the renumbered range, the undo does not
// hit the wrong shot - it fails outright and WEDGES the history
// ===========================================================================

/// Bug 8's mechanism with the opposite outcome, and the reason it needs its own
/// entry: a section that has had a shot DELETED has handed out more ids than it
/// has shots, so the newest persisted id runs past
/// `section_id * FEATURE_ID_STRIDE + (len - 1)`. `delete_shot` then finds
/// nothing, the undo fails, and `undo_inner`'s epilogue pushes the entry back
/// (core/state/undo.rs) - so it fails identically forever and EVERY entry
/// beneath it is unreachable for the rest of the project's life.
///
/// A wedged history is worse than a wrong one: the user cannot even walk past it
/// to reach the edits underneath. The same shape applies to `update_shot` and
/// `set_shot_kind`, whose undo also returns `Shot ... not found in section ...`.
///
/// Found by `undo-persistence-fuzz` with
/// `STALAGMIGHT_PERSIST_FUZZ_DELETE_IN_TARGET=1`.
#[test]
fn undo_after_reload_is_not_wedged_by_an_out_of_range_shot_id() {
    let _config = isolated_config();
    let temp = TempDir::new().unwrap();
    let save_path: PathBuf = temp.path().join("wedged.stalx");

    let section_id;
    {
        let state = state_with_project("OpenUndoBugs");
        section_id = create_section(
            &state,
            "Wedged",
            vec![
                straight_shot(0.0, 5.0),
                straight_shot(90.0, 6.0),
                straight_shot(180.0, 7.0),
            ],
        );

        // The delete is what makes the allocator run ahead of the shot count.
        let middle = shot_ids(&state, section_id)[1];
        let deleted = state.execute(Command::DeleteShot {
            section_id,
            shot_id: middle,
        });
        assert!(deleted.success, "DeleteShot failed: {:?}", deleted.error);

        let first = shot_ids(&state, section_id)[0];
        let created = state.execute(Command::CreateShot {
            section_id,
            shot: straight_shot(270.0, 8.0),
            after_shot_id: None,
            before_shot_id: Some(first),
        });
        assert!(created.success, "CreateShot failed: {:?}", created.error);

        let ids = shot_ids(&state, section_id);
        let highest_renumbered =
            section_id * crate::models::FEATURE_ID_STRIDE + ids.len() as u32 - 1;
        assert!(
            ids.iter().any(|id| *id > highest_renumbered),
            "setup: at least one live id must be past the range the loader renumbers into, \
             got {ids:?} against a ceiling of {highest_renumbered}"
        );

        let saved = state.save_project(Some(save_path.clone()));
        assert!(saved.success, "save failed: {:?}", saved.error);
    }

    let reopened = AppState::new().unwrap();
    let opened = reopened.execute(Command::OpenProject { path: save_path });
    assert!(opened.success, "reopen failed: {:?}", opened.error);

    let depth_before = reopened.get_undo_stack().len();
    let undone = reopened.undo();
    assert!(
        undone.success,
        "undo of a CreateShot failed after the reopen ({:?}); the entry is pushed back, so the \
         history is WEDGED and the {} entr(y/ies) beneath it can never be undone. A second undo \
         fails identically: {:?}",
        undone.error,
        depth_before.saturating_sub(1),
        reopened.undo().error
    );
}

// ===========================================================================
// Finding 14: REAL, and pinned - but by a SEED, not by a reduction
// ===========================================================================
//
// With a datum on section A and a merge chain A -> B -> C, a mid-chain
// `CreateShot` into A leaves the A/B merge metres apart and the app's own
// `debug_assert_merge_alignment` (`models/project/offsets.rs`, whose assert arm
// is `#[cfg(test)]` and therefore exists in NO other tier) panics inside
// `insert_shot_after`.
//
// The red test is `finding_14_no_insert_leaves_a_merge_out_of_alignment` in
// `src-tauri/src/core/state/fuzz_undo_tests.rs`, pinned to one deterministic
// fuzzer seed. It lives there rather than here because two reductions were
// MEASURED not to reproduce, and a red test has to be red:
//
//   * sections A(3 shots, datum at 0), B(1), C(1), merges A:3<->B:0 and
//     B:1<->C:0, then `CreateShot(after = A's first shot)` - every merge stays
//     aligned. The cascade handles the plain chain.
//   * the same with the fuzzer's FULL starting topology transcribed (seven
//     sections of varied length, the zero datum, both merges, the loop closure)
//     and a `MoveStation` on the bridge before the insert - also clean.
//
// So the ingredient is further back in the fuzzer's history than either
// reduction reaches, and pinning the seed pins the defect honestly while a
// reduction would only pin a guess. `models/project/shots.rs`'s
// `insert_shot_after` back-shifts the section offset only when the merged
// partner has an INDEPENDENT datum path (`has_datum_path_excluding`), which on a
// chain it does not, leaving `cascade_connection_offsets` to move the partner
// instead - that is still the suspected mechanism, and it is what a fix should
// start from.
//
// Also part of the finding: `check_merge_alignment`
// (`models/project/integrity.rs`) measures through `resolved_station_offsets()`
// and reports NOTHING about the same state the raw-coordinate assert panics on.
// Whichever is authoritative, the two should not disagree.

// ===========================================================================
// Finding 15: an undo leaves a `section_offsets` entry with REAL geometry -
// which is FINDING 3, re-measured several moves later
// ===========================================================================

/// Finding 15 was filed as its own defect because the move that FAILS is not
/// the move that did it. Finding 3 displaces the empty section on the undo that
/// removes its first shot; nothing puts that back (the redo re-inserts through
/// the offset-neutral append path, which is finding 3's ratchet half), so the
/// leftover is still there several undos later, when the entry being consumed
/// is something unrelated - a `set_station_comment`, an `update_shot` - and the
/// snapshot it is compared against never had the key.
///
/// Root-caused with the Rust fuzzer's own reported instance: seed 7 at 100
/// moves, `section_offsets.1` = `[7.11187043796, 0.315846281796]` at depth 14,
/// whose immediately preceding two moves are `createShot(s=1,n=0,append)` and
/// the undo of it. The write-up read the move log from the wrong end.
///
/// This test pins the CONSEQUENCE rather than the cause, because that is what a
/// user sees and what finding 3's own tests do not cover: unwinding the whole
/// history has to return the project to the state it started in.
///
/// FIXED by fixing finding 3: there is no displacement left to survive. Nothing
/// here was specific to finding 15, which is why it was closed as a duplicate.
#[test]
fn unwinding_the_history_leaves_no_displaced_section_behind() {
    let _config = isolated_config();
    let state = state_with_project("OpenUndoBugs");
    let keeper = create_section(&state, "Keeper", vec![straight_shot(0.0, 5.0)]);
    let empty = create_section(&state, "Empty", vec![]);

    let before = section_offset(&state, empty);
    assert_eq!(
        before, None,
        "setup: a section that has never been moved should have no offset entry"
    );

    // Depth 1: an ordinary edit on an unrelated section.
    let keeper_shot = shot_ids(&state, keeper)[0];
    let edited = state.execute(Command::UpdateShot {
        section_id: keeper,
        shot_id: keeper_shot,
        updates: ShotUpdates {
            length: Some(9.0),
            ..Default::default()
        },
    });
    assert!(edited.success, "UpdateShot failed: {:?}", edited.error);

    // Depth 2: the first shot appended into the empty section.
    let created = state.execute(Command::CreateShot {
        section_id: empty,
        shot: straight_shot(90.0, 10.0),
        after_shot_id: None,
        before_shot_id: None,
    });
    assert!(created.success, "CreateShot failed: {:?}", created.error);

    // All the way back down.
    assert!(state.undo().success, "undo of CreateShot failed");
    assert!(state.undo().success, "undo of UpdateShot failed");

    assert_eq!(
        section_offset(&state, empty),
        before,
        "unwinding the history left section {empty} displaced: the undo of the first shot in an \
         empty section slides it (finding 3) and nothing puts it back, so the offset is still \
         there after the undo of an UNRELATED entry - which is what finding 15 observed"
    );
}

// ===========================================================================
// Finding 16: after a reload, an undo/redo cycle of CreateShot leaves the
// section holding the SAME shot uuid twice
// ===========================================================================

/// The redo half of finding 8, which nothing had exercised.
///
/// After a reload the undo deletes the wrong shot (finding 8) and leaves the
/// created one in place. The redo then re-runs the SAME insert through
/// `redo_inner`'s `CreatedShotSnapshot` arm (`core/state/undo.rs`), which forces
/// both the id AND the uuid from the entry - so the shot it re-creates is a
/// second copy of one that never left. The section ends up holding one
/// `Shot::uuid` twice.
///
/// That is worse than a wrong shot: uuid is the identity `StationKey::AfterShot`
/// resolves stations by (`models/station.rs`), the identity the Ariane export
/// writes, and the identity `ensure_unique_shot_uuids` will silently RE-MINT on
/// the next save - moving whichever anchor loses the coin toss.
///
/// Found by the Rust fuzzer at seed 13 x 60 with persistence.
#[test]
fn redo_after_reload_does_not_duplicate_a_shot_uuid() {
    let _config = isolated_config();
    let temp = TempDir::new().unwrap();
    let save_path: PathBuf = temp.path().join("duplicated.stalx");

    let section_id;
    {
        let state = state_with_project("OpenUndoBugs");
        section_id = diverged_section(&state);
        let saved = state.save_project(Some(save_path.clone()));
        assert!(saved.success, "save failed: {:?}", saved.error);
    }

    let reopened = AppState::new().unwrap();
    let opened = reopened.execute(Command::OpenProject { path: save_path });
    assert!(opened.success, "reopen failed: {:?}", opened.error);

    assert!(reopened.undo().success, "undo after reopen failed");
    assert!(reopened.redo().success, "redo after reopen failed");

    let uuids: Vec<String> = shot_uuids(&reopened, section_id)
        .into_iter()
        .flatten()
        .collect();
    let mut unique = uuids.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        uuids.len(),
        "an undo/redo cycle after the reopen left section {section_id} holding the same shot uuid \
         twice: {uuids:?}. The undo deleted the wrong shot and the redo re-inserted the one that \
         was still there, forcing the persisted uuid onto the copy"
    );
}

// ===========================================================================
// Finding 17 (FIXED): undo of RemoveZeroDatum appended the datum instead of
// restoring its index - finding 4's defect, one collection over
// ===========================================================================

/// `UndoData::ZeroDatumInfo`'s restore arm ended in
/// `project.zero_datums.push(ZeroDatum { .. })` (`core/state/undo.rs`) where the
/// forward path removed the datum in place, so a datum that was not last came
/// back last. FIXED the same way finding 4 was, and kept as the regression
/// guard: the entry records the index the removal took it out of and the undo
/// re-inserts there, with `push` kept for a legacy entry and for a
/// `SetZeroDatum` on a station that had no datum.
///
/// Exactly finding 4's shape and exactly its consequences: `zero_datums` is
/// persisted in order, the frontend indexes it, and `resolve_offsets` anchors
/// connected groups by walking it - so the order decides which datum wins when a
/// component has more than one candidate anchor.
///
/// Found by the Rust fuzzer's closure-shapes mode, which is the first thing that
/// gave a run two datums to reorder.
#[test]
fn undo_of_remove_zero_datum_restores_the_original_order() {
    let _config = isolated_config();
    let state = state_with_project("OpenUndoBugs");
    // Two INDEPENDENT components, so both may carry a datum: a second datum in
    // one component is refused.
    let first = create_section(&state, "First", vec![straight_shot(0.0, 5.0)]);
    let second = create_section(&state, "Second", vec![straight_shot(90.0, 5.0)]);
    for (section_id, latitude) in [(first, 10.0), (second, 11.0)] {
        let datum = state.execute(Command::SetZeroDatum {
            station_ref: StationRef::new(section_id, 0),
            coordinates: GpsCoordinates::new(latitude, 20.0, 0.0).unwrap(),
        });
        assert!(datum.success, "SetZeroDatum failed: {:?}", datum.error);
    }

    let before = datum_sections(&state);
    assert_eq!(
        before,
        vec![first, second],
        "setup: the datums should be in creation order"
    );

    // Remove the FIRST of the two. Removing the last one would come back in the
    // right place by luck and prove nothing.
    let removed = state.execute(Command::RemoveZeroDatum {
        station_ref: StationRef::new(first, 0),
    });
    assert!(
        removed.success,
        "RemoveZeroDatum failed: {:?}",
        removed.error
    );
    assert!(state.undo().success, "undo of RemoveZeroDatum failed");

    assert_eq!(
        datum_sections(&state),
        before,
        "undo put the datum back at the END of zero_datums instead of its original index; the \
         order is persisted, indexed by the frontend, and the order resolve_offsets anchors \
         connected groups in"
    );
}

/// The section of every zero datum, in `Project::zero_datums` order.
fn datum_sections(state: &AppState) -> Vec<SectionId> {
    let project = state.get_project_state().expect("no project open");
    project
        .zero_datums
        .iter()
        .map(|datum| datum.station_ref.section_id)
        .collect()
}

// ===========================================================================
// Finding 20 (FIXED): a geometry edit inside an UNANCHORED merge group left the
// merge drifted, under every threshold that would report it
// ===========================================================================

/// The raw distance between two stations, measured the way
/// `Project::debug_assert_merge_alignment` measures it
/// (`compute_station_raw_coords`, `models/project/offsets.rs`) rather than
/// through `resolved_station_offsets()`. The app's two alignment measures
/// disagree by construction (issue #413 finding 14), so a test about the
/// invariant the app ASSERTS has to use the assert's own measure.
/// The raw `(x, y)` of two stations, in one project read - for a caller that
/// needs the VECTOR between them (a snap-to-merge move) rather than the distance.
fn station_raw_xy(
    state: &AppState,
    a: (SectionId, u32),
    b: (SectionId, u32),
) -> ((f64, f64), (f64, f64)) {
    let project = state.get_project_state().expect("no project open");
    let raw = |at: (SectionId, u32)| {
        let coords = project
            .compute_station_raw_coords(StationRef::new(at.0, at.1))
            .expect("station has no raw coordinates");
        (coords.x, coords.y)
    };
    (raw(a), raw(b))
}

fn raw_station_gap(state: &AppState, a: (SectionId, u32), b: (SectionId, u32)) -> f64 {
    let project = state.get_project_state().expect("no project open");
    let left = project
        .compute_station_raw_coords(StationRef::new(a.0, a.1))
        .expect("station a has no raw coordinates");
    let right = project
        .compute_station_raw_coords(StationRef::new(b.0, b.1))
        .expect("station b has no raw coordinates");
    ((left.x - right.x).powi(2) + (left.y - right.y).powi(2)).sqrt()
}

/// Two sections joined by ONE merge and anchored by NOTHING - no zero datum
/// anywhere in the project. Returns `(a, b)`, merged at `a:1 <-> b:0`.
///
/// The absence of the datum is the whole precondition, so it is not an
/// oversight: `realign_after_shot_edit` looks for a datum in the edited
/// section's merge group and returns early when there is none.
fn two_unanchored_merged_sections(state: &AppState) -> (SectionId, SectionId) {
    let a = create_section(
        state,
        "A",
        vec![straight_shot(0.0, 5.0), straight_shot(0.0, 5.0)],
    );
    let b = create_section(state, "B", vec![straight_shot(90.0, 5.0)]);
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 0),
    });
    assert!(merged.success, "MergeStations failed: {:?}", merged.error);
    assert!(
        raw_station_gap(state, (a, 1), (b, 0)) < 1e-9,
        "setup: MergeStations should leave the pair co-located"
    );
    (a, b)
}

/// FINDING 20: editing a shot's geometry inside a merge group that holds no zero
/// datum moves one side of the merge and realigns nothing, and the drift then
/// sits below every threshold that would report it.
///
/// Two gates decline in sequence:
///
/// 1. `Project::realign_after_shot_edit` (`models/project/mutations.rs:923`) -
///    the only thing `update_shot` calls to repair the merge
///    (`core/state/command_shot.rs:173`) - looks for a zero datum in the edited
///    section's merge group and, finding none, returns at
///    `mutations.rs:941` ("No anchor in this group, nothing to realign").
/// 2. `AppState::execute`'s epilogue (`core/state/commands.rs:654`) then runs
///    `reconcile_geometry()` -> `Project::realign_merges()`
///    (`models/project/offsets.rs:280`), which re-solves ONLY when
///    `check_merge_alignment()` already reports a violation - and that checker's
///    threshold is `Project::MERGE_ALIGNMENT_TOLERANCE_M` = 5 cm
///    (`models/project.rs:1064`).
///
/// So drift under 5 cm survives, and the app's OWN invariant for the same pair,
/// asserted by `debug_assert_merge_alignment` (`models/project/offsets.rs:112`),
/// is 1 cm. The whole band from 1 cm to 5 cm is a state the app asserts cannot
/// exist and produces on purpose.
///
/// Measured band, from the probe this test was reduced from: +0.005 m of shot
/// length leaves 0.005 m, +0.02 leaves 0.02, +0.0499 leaves 0.0499, +0.0501
/// leaves 0 (the epilogue heals it), +0.5 leaves 0. And with a datum in the
/// group every one of those leaves 0.
///
/// Found by the Rust fuzzer's HORIZON mode, seed 1, where an `updateShot` after
/// a `mergeStations` left a pair 1.58 cm apart, raw and resolved agreeing.
///
/// FIXED on both gates, and kept as the regression guard.
/// `realign_after_shot_edit` no longer gives up on an unanchored group: it has no
/// ABSOLUTE anchor but it still has the relative merge constraints, so it anchors
/// on the edited section and BFSes its partners into place. And `realign_merges`
/// heals at `Project::MERGE_ALIGNMENT_INVARIANT_M` (the assert's own 1 cm) rather
/// than at the 5 cm REPORTING tolerance, so the 1-5 cm band is not a band any
/// more. Red on `left the merged pair 0.020000 m apart`.
#[test]
fn a_shot_edit_keeps_an_unanchored_merge_aligned() {
    let _config = isolated_config();
    let state = state_with_project("OpenUndoBugs");
    let (a, b) = two_unanchored_merged_sections(&state);

    // 2 cm of extra length on the shot BEFORE the merged station, so the station
    // moves by exactly that. Deliberately inside the band: 5.1 cm would be
    // healed by the epilogue and would prove nothing.
    let first_shot = shot_ids(&state, a)[0];
    let updated = state.execute(Command::UpdateShot {
        section_id: a,
        shot_id: first_shot,
        updates: ShotUpdates {
            length: Some(5.02),
            ..ShotUpdates::default()
        },
    });
    assert!(updated.success, "UpdateShot failed: {:?}", updated.error);

    let gap = raw_station_gap(&state, (a, 1), (b, 0));
    assert!(
        gap < 0.01,
        "a shot edit left the merged pair {gap:.6} m apart, past the 0.01 m the app's own \
         debug_assert_merge_alignment calls a violation - and under the 0.05 m \
         check_merge_alignment reports, so the user is told nothing and the drift is saved"
    );
}

/// The same defect's CONSEQUENCE, and the reason it is not merely cosmetic: the
/// drift is a landmine for a later, unrelated command.
///
/// `update_shot` is not one of the call sites that runs
/// `debug_assert_merge_alignment` (the eight that do are the insert/remove/split
/// shot paths, `merge_stations`, `set_zero_datum`, `adjust_loop_closures` and
/// `delete_shot_range_shift`), so the edit itself says nothing. The next command
/// that DOES assert - here a `CreateShot` into a third, entirely unrelated
/// section - trips on the merge the edit broke two commands ago, in a test build
/// with no way to attribute it.
///
/// In a release build the same state produces neither the assert nor a user
/// warning: it is simply saved.
///
/// FIXED with the drift itself, and kept: this is the half that says the damage
/// is not confined to the pair, so a future regression that only re-opens the
/// landmine (an edit path that skips the realign) still fails here. Red on
/// `Merge alignment violated after insert_shot_after: ... 0.020000m apart`,
/// raised by an insert into a section that touches neither side of the merge.
#[test]
fn a_shot_edit_does_not_arm_an_alignment_assert_for_a_later_command() {
    let _config = isolated_config();
    let state = state_with_project("OpenUndoBugs");
    let (a, _b) = two_unanchored_merged_sections(&state);
    let elsewhere = create_section(&state, "Elsewhere", vec![straight_shot(45.0, 5.0)]);

    let first_shot = shot_ids(&state, a)[0];
    let updated = state.execute(Command::UpdateShot {
        section_id: a,
        shot_id: first_shot,
        updates: ShotUpdates {
            length: Some(5.02),
            ..ShotUpdates::default()
        },
    });
    assert!(updated.success, "UpdateShot failed: {:?}", updated.error);

    // A command in a DIFFERENT section, touching neither side of the merge.
    let anchor = shot_ids(&state, elsewhere)[0];
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        state.execute(Command::CreateShot {
            section_id: elsewhere,
            shot: straight_shot(10.0, 3.0),
            after_shot_id: Some(anchor),
            before_shot_id: None,
        })
    }));
    let message = match outcome {
        Ok(_) => String::new(),
        Err(payload) => payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
            .unwrap_or_else(|| "<non-string panic>".to_string()),
    };
    assert!(
        message.is_empty(),
        "an unrelated insert into a third section tripped the app's own merge-alignment assert \
         on the merge a shot edit broke two commands earlier: {}",
        message.lines().next().unwrap_or_default()
    );
}

// ===========================================================================
// Finding 21: the ADJUSTED geometry is not a function of the project's value
// ===========================================================================

/// Every adjusted station's coordinates as RAW BITS, keyed by station.
///
/// Bits rather than a tolerance on purpose: the claim under test is that a
/// solve over one unchanging input is a FUNCTION, and a function that returns
/// two different answers is not one at any tolerance. The magnitude is reported
/// separately, so the failure says both "not a function" and "by this much".
fn adjusted_bits(state: &AppState) -> Vec<(String, [u64; 3])> {
    let mut rows: Vec<(String, [u64; 3])> = state
        .get_adjusted_stations()
        .expect("no adjusted stations")
        .iter()
        .map(|station| {
            (
                format!(
                    "{}:{}",
                    station.station_ref.section_id, station.station_ref.station_index
                ),
                [
                    station.coordinates.x.to_bits(),
                    station.coordinates.y.to_bits(),
                    station.coordinates.z.to_bits(),
                ],
            )
        })
        .collect();
    rows.sort();
    rows
}

/// FINDING 21: re-solving the SAME project gives a different answer almost every
/// time, because the least-squares adjustment numbers its unknowns in `HashMap`
/// iteration order.
///
/// `Project::adjust_least_squares` (`models/project/lsq.rs`) builds the solver's
/// node numbering with
///
/// ```text
/// let mut unknown_index: HashMap<StationRef, usize> = HashMap::new();
/// for root in groups.keys() { ... unknown_index.insert(*root, next); }
/// ```
///
/// (`lsq.rs:403-409`), where `groups` is a `HashMap` built fresh inside the
/// call - so its `RandomState` differs on every invocation and the numbering is
/// a fresh random permutation each time. The numbering is not cosmetic: the
/// reduction peels nodes in ascending index order (`lsq.rs:72`), the
/// neighbour sets it sums over are themselves `HashMap`s (`lsq.rs:84-85`,
/// `lsq.rs:148-155`, `lsq.rs:200-208`), and floating-point addition is not
/// associative. The conjugate-gradient stop is RELATIVE (`lsq.rs:253`), so a
/// different summation order can also change the iteration count.
///
/// Why it matters beyond the last bit: the adjusted stations are what the map
/// draws, what every export writes, and what the undo fuzzer's fourth oracle
/// compares per undo depth. A derived view that is not a function of the state
/// cannot be compared against a recorded one without a tolerance, and a
/// tolerance is exactly what hides a real regression.
///
/// The magnitude here is small - the last bits of this five-shot loop's
/// coordinates, reported by the failure rather than promised in prose - because
/// the system is tiny and well conditioned. It is NOT bounded by anything: the
/// amplification is the condition number of the network, and the fuzzer's own
/// adjusted-geometry oracle recorded a station 2.2 km out moving 28 microns
/// between two solves of a state whose stored project roundtripped byte for
/// byte.
///
/// The input is proven not to move: the canonical project snapshot is captured
/// before and after the whole loop and asserted equal, so "the same project"
/// is a measurement here rather than an assumption.
#[test]
fn the_adjusted_solve_is_a_function_of_the_project() {
    let _config = isolated_config();
    let state = state_with_project("OpenUndoBugs");
    let a = create_section(
        &state,
        "A",
        vec![
            straight_shot(0.0, 5.0),
            straight_shot(0.0, 5.0),
            straight_shot(10.0, 4.0),
        ],
    );
    let b = create_section(
        &state,
        "B",
        vec![straight_shot(90.0, 5.0), straight_shot(95.0, 6.0)],
    );
    let datum = state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(a, 0),
        coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
    });
    assert!(datum.success, "SetZeroDatum failed: {:?}", datum.error);
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 3),
        station_b: StationRef::new(b, 0),
    });
    assert!(merged.success, "MergeStations failed: {:?}", merged.error);
    // A least-squares closure is the precondition: without one,
    // `plan_adjustment_anchors` never enters `adjust_least_squares` at all and
    // the solve reduces to a deterministic coordinate walk.
    let closure = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(a, 0),
        station_b: StationRef::new(b, 2),
        closure_strategy: ClosureStrategy::LeastSquaresWeighted,
    });
    assert!(
        closure.success,
        "CreateLoopClosure failed: {:?}",
        closure.error
    );

    let project_before = state
        .capture_project_snapshot()
        .expect("capture_project_snapshot failed");
    let base = adjusted_bits(&state);
    let mut differing = 0;
    let mut worst = 0.0_f64;
    for _ in 0..64 {
        state.invalidate_adjusted_cache();
        let again = adjusted_bits(&state);
        if again == base {
            continue;
        }
        differing += 1;
        for ((_, left), (_, right)) in base.iter().zip(again.iter()) {
            for axis in 0..3 {
                worst = worst.max((f64::from_bits(left[axis]) - f64::from_bits(right[axis])).abs());
            }
        }
    }
    let project_after = state
        .capture_project_snapshot()
        .expect("capture_project_snapshot failed");
    assert_eq!(
        project_before, project_after,
        "the project itself changed during the measurement, so the re-solves did not share an input"
    );

    assert_eq!(
        differing, 0,
        "{differing} of 64 re-solves of the SAME project returned different coordinates \
         (worst {worst:e} m): the adjusted geometry the map draws and every export writes \
         depends on HashMap iteration order, not only on the project"
    );
}

// ===========================================================================
// Finding 22 (RESOLVED): an atomic range delete keeps a cut-loose partner put
// ===========================================================================
//
// HOW THIS ONE ENDED. Finding 22 was reported as a defect: a range shift-delete
// is not the `n` single `Command::DeleteShot`s its own doc comment said it
// "generalizes ... by deleting the range's shots in forward order", because on a
// suffix range the merged PARTNER section ends up seven metres from where the
// single deletes leave it.
//
// The equivalence is false BY DESIGN, and the doc comment was the defect. Two
// deliberately-reviewed tests from PR #95 pin the atomic semantics -
// `test_delete_range_shift_does_not_displace_free_floating_merged_section` and
// `test_delete_range_suffix_shift_keeps_head_with_doomed_tail_anchor`
// (`core/state/tests.rs`) - and they are irreconcilable with the composition on
// this topology. `AppState::execute` runs the geometry epilogue after every
// command, so the single-delete sequence re-anchors the partner while the merge
// is still live, drags it as the section shrinks, and then deletes the merge:
// the partner is left displaced with no edge to pull it back. The range path
// defers to ONE cascade at the end and leaves it where the surveyor put it,
// which is the better answer for the user and the reviewed intent.
//
// So #413 finding 22 is resolved as intended behaviour plus a documentation fix
// (`command_delete_split.rs`, `execute_delete_shot_range_shift`), the red test
// that asserted the false promise is gone, and this green one pins what the code
// actually contracts to do. Its metamorphic counterpart
// (`metamorphic_undo_tests.rs`, pair `range-shift-is-n-single-deletes`) asserts
// the same two halves on random topology and no longer classifies the
// difference away.

/// Every `section_offsets` entry, by section, ascending.
fn section_offsets(state: &AppState) -> Vec<(SectionId, (f64, f64))> {
    let project = state.get_project_state().expect("no project open");
    let mut out: Vec<(SectionId, (f64, f64))> = project
        .section_offsets()
        .iter()
        .map(|(id, offset)| (*id, (offset.0, offset.1)))
        .collect();
    out.sort_by_key(|entry| entry.0);
    out
}

/// A four-shot section merged at its far end to a one-shot section, anchored by
/// nothing. Returns `(a, b)`.
fn merged_chain_for_range_delete(state: &AppState) -> (SectionId, SectionId) {
    let a = create_section(
        state,
        "A",
        vec![
            straight_shot(0.0, 5.0),
            straight_shot(30.0, 6.0),
            straight_shot(60.0, 7.0),
            straight_shot(90.0, 8.0),
        ],
    );
    let b = create_section(state, "B", vec![straight_shot(120.0, 5.0)]);
    let merged = state.execute(Command::MergeStations {
        station_a: StationRef::new(a, 4),
        station_b: StationRef::new(b, 0),
    });
    assert!(merged.success, "MergeStations failed: {:?}", merged.error);
    (a, b)
}

/// Every adjusted station position of one section, by index, ascending.
fn positions_of(state: &AppState, section_id: SectionId) -> Vec<(u32, [f64; 3])> {
    let mut out: Vec<(u32, [f64; 3])> = state
        .get_adjusted_stations()
        .expect("no project open")
        .iter()
        .filter(|station| station.station_ref.section_id == section_id)
        .map(|station| {
            (
                station.station_ref.station_index,
                [
                    station.coordinates.x,
                    station.coordinates.y,
                    station.coordinates.z,
                ],
            )
        })
        .collect();
    out.sort_by_key(|entry| entry.0);
    out
}

/// How far apart two position lists are, in metres, and `f64::INFINITY` when
/// they do not even describe the same stations.
fn position_gap(left: &[(u32, [f64; 3])], right: &[(u32, [f64; 3])]) -> f64 {
    if left.len() != right.len() {
        return f64::INFINITY;
    }
    let mut worst = 0.0_f64;
    for (a, b) in left.iter().zip(right.iter()) {
        if a.0 != b.0 {
            return f64::INFINITY;
        }
        // Euclidean, so the number a failure prints is the distance the station
        // actually moved rather than its longest axis.
        let moved = (0..3)
            .map(|axis| (a.1[axis] - b.1[axis]).powi(2))
            .sum::<f64>()
            .sqrt();
        worst = worst.max(moved);
    }
    worst
}

/// Both routes over `[lo..=hi]`, each on its own freshly-built copy of
/// [`merged_chain_for_range_delete`].
struct Routes {
    /// The state `execute_delete_shot_range_shift` was run on.
    ranged: AppState,
    /// The state the same shots were deleted on one `Command::DeleteShot` at a
    /// time, forward from the range's start position.
    singles: AppState,
    /// The four-shot section that was edited.
    a: SectionId,
    /// The one-shot section merged to A's far station.
    b: SectionId,
}

fn both_routes_over(lo: usize, hi: usize) -> Routes {
    let ranged = state_with_project("OpenUndoBugs");
    let (ranged_a, ranged_b) = merged_chain_for_range_delete(&ranged);
    let ids = shot_ids(&ranged, ranged_a);
    let result = ranged.execute_delete_shot_range_shift(ranged_a, ids[lo], ids[hi]);
    assert!(
        result.success,
        "execute_delete_shot_range_shift([{lo}..={hi}]) failed: {:?}",
        result.error
    );

    let singles = state_with_project("OpenUndoBugs");
    let (singles_a, singles_b) = merged_chain_for_range_delete(&singles);
    assert_eq!(
        (ranged_a, ranged_b),
        (singles_a, singles_b),
        "the two routes were not built on the same topology, so nothing below compares anything"
    );
    // Forward order: delete the shot AT the range's start position, once per
    // shot in the range - the ids do not shift but the positions do.
    for step in lo..=hi {
        let live = shot_ids(&singles, singles_a);
        let single = singles.execute(Command::DeleteShot {
            section_id: singles_a,
            shot_id: live[lo],
        });
        assert!(
            single.success,
            "single delete {step} of [{lo}..={hi}] failed: {:?}",
            single.error
        );
    }
    Routes {
        ranged,
        singles,
        a: ranged_a,
        b: ranged_b,
    }
}

/// FINDING 22, RESOLVED: `execute_delete_shot_range_shift` is ATOMIC, and that
/// is the contract - not the composition its doc comment used to claim.
///
/// Two halves, both measured on `merged_chain_for_range_delete` (a four-shot
/// section A whose far station A:4 is merged to a one-shot, undatumed section B):
///
/// 1. THE DOOMED-ANCHOR SHAPE, `[2..=3]`. The range takes A:3 and A:4 - the
///    station the merge is anchored to - so B is cut loose with no datum and no
///    remaining edge. The range delete leaves B exactly where it was; the two
///    single deletes leave it 7.000000 m away, because the first of them
///    re-anchors B against a section that has already lost one shot and the
///    second then deletes the merge. Keeping B put is the reviewed intent
///    (PR #95, the two tests named in the block comment above), and the
///    difference is documented in `execute_delete_shot_range_shift`.
///
/// 2. EVERY OTHER SHAPE. A prefix `[0..=1]` and an interior `[1..=2]` remove no
///    merged-to station, and there the two routes agree to the last bit - both
///    in `section_offsets` and in the adjusted geometry, for BOTH sections. The
///    atomic path is not free to differ wherever it likes; it differs on exactly
///    the shape the doc comment now names.
#[test]
fn a_doomed_anchor_range_shift_keeps_the_partner_in_place() {
    let _config = isolated_config();
    // --- Half 1: the doomed-anchor suffix range -------------------------------
    let reference = state_with_project("OpenUndoBugs");
    let (_, reference_b) = merged_chain_for_range_delete(&reference);
    let b_before = positions_of(&reference, reference_b);
    let offsets_before = section_offsets(&reference);

    let suffix = both_routes_over(2, 3);
    assert_eq!(
        suffix.b, reference_b,
        "the reference project and the routed ones disagree on which section B is"
    );

    let b_ranged = positions_of(&suffix.ranged, suffix.b);
    let ranged_moved = position_gap(&b_before, &b_ranged);
    assert!(
        ranged_moved <= 1e-9,
        "the range delete cut B loose (no datum, no merge left) and MOVED it {ranged_moved} m: \
         {b_before:?} -> {b_ranged:?}. A deferred cascade must leave a free-floating partner \
         where it was"
    );
    // B is really cut loose - otherwise the assertion above is about a section
    // something else was holding in place, and proves nothing.
    let project = suffix
        .ranged
        .get_project_state()
        .expect("no project open after the range delete");
    assert!(
        project.station_merges.is_empty(),
        "the range delete was supposed to remove the only merge: {:?}",
        project.station_merges
    );
    assert!(
        project.zero_datums.is_empty(),
        "B was supposed to be undatumed: {:?}",
        project.zero_datums
    );
    // The same fact in the STORED project, where the write-up recorded it: B had
    // no offset going in and comes out with none (or, if some other path leaves
    // the key behind, a geometrically inert one - whether an inert key survives
    // at all is finding 2's business, its VALUE is this test's).
    let ranged_offset = section_offsets(&suffix.ranged)
        .into_iter()
        .find(|(id, _)| *id == suffix.b)
        .map(|(_, offset)| offset);
    assert!(
        ranged_offset.is_none_or(|(x, y)| x.abs() <= 1e-9 && y.abs() <= 1e-9),
        "the range delete gave B a section_offsets entry that MOVES it: {:?} -> {:?}",
        offsets_before,
        section_offsets(&suffix.ranged)
    );

    // The other route, and the DIFFERENCE this pins: the singles drag B along.
    let b_singles = positions_of(&suffix.singles, suffix.b);
    let singles_moved = position_gap(&b_before, &b_singles);
    assert!(
        (singles_moved - 7.0).abs() <= 1e-6,
        "the single-delete sequence was expected to strand B 7.000000 m away (the length of the \
         first shot it deletes, applied while the merge was still live); it moved it \
         {singles_moved} m: {b_before:?} -> {b_singles:?}"
    );
    let singles_offset = section_offsets(&suffix.singles)
        .into_iter()
        .find(|(id, _)| *id == suffix.b)
        .map(|(_, offset)| offset)
        .expect("the single deletes were expected to give B a section_offsets entry");
    assert!(
        (singles_offset.0 - -6.062_177_826_491_07).abs() <= 1e-9
            && (singles_offset.1 - -3.5).abs() <= 1e-9,
        "the single deletes put B at an offset this test does not know about: {singles_offset:?}"
    );

    // --- Half 2: every range that removes no merged-to station ----------------
    for (label, lo, hi) in [("prefix", 0, 1), ("interior", 1, 2)] {
        let routes = both_routes_over(lo, hi);
        assert_eq!(
            section_offsets(&routes.ranged),
            section_offsets(&routes.singles),
            "a {label} range [{lo}..={hi}] removes no station a merge is anchored to, so the \
             range delete and the single deletes must leave identical section_offsets"
        );
        for (name, section_id) in [("A", routes.a), ("B", routes.b)] {
            let gap = position_gap(
                &positions_of(&routes.ranged, section_id),
                &positions_of(&routes.singles, section_id),
            );
            assert!(
                gap <= 1e-9,
                "a {label} range [{lo}..={hi}] put section {name} in two different places \
                 depending on the route ({gap} m apart), and only the doomed-anchor shape is \
                 allowed to differ"
            );
        }
    }
}

// ===========================================================================
// Finding 24: describing a command with an out-of-range shot id overflows
// ===========================================================================

/// FINDING 24: `Command::description()` panics on a shot id near `u32::MAX`,
/// before any validation has had a chance to refuse it.
///
/// `shot_display_position` (`models/shot.rs:15-22`) converts a raw `ShotId` to
/// a 1-indexed display position:
///
/// ```text
/// let base = section_id.saturating_mul(FEATURE_ID_STRIDE);
/// if shot_id >= base { shot_id - base + 1 } else { shot_id + 1 }
/// ```
///
/// The multiply saturates; neither add does. `shot_id == u32::MAX` with a
/// section whose base is 0 takes the first arm and computes `u32::MAX + 1`.
///
/// It is reached from `Command::description()` (`core/command.rs:702`, `:710`,
/// `:717`), which `AppState::execute` calls on EVERY command to label the undo
/// entry and to build the corruption guard's message - so the panic happens
/// before the id is ever checked against the section. A debug or test build
/// aborts; a release build wraps and shows the user a nonsense position.
///
/// Found by the adversarial engine (`adversarial_tests.rs`), seeds 4 and 7.
#[test]
fn describing_a_command_with_an_out_of_range_shot_id_does_not_overflow() {
    let _config = isolated_config();
    let state = state_with_project("OpenUndoBugs");
    let section_id = create_section(&state, "A", vec![straight_shot(0.0, 5.0)]);

    // No section holds this shot. The command must be REFUSED, not fatal.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        state.execute(Command::SetShotKind {
            section_id,
            shot_id: ShotId::MAX,
            kind: ShotKind::Jump,
        })
    }));
    let message = match outcome {
        Ok(result) => {
            assert!(
                !result.success,
                "SetShotKind with a shot id no section holds reported success"
            );
            String::new()
        }
        Err(payload) => payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| {
                payload
                    .downcast_ref::<&str>()
                    .map(|text| (*text).to_string())
            })
            .unwrap_or_else(|| "<non-string panic>".to_string()),
    };
    assert!(
        message.is_empty(),
        "describing a command with an out-of-range shot id panicked before the id was validated: \
         {message}"
    );
}

// ===========================================================================
// Finding 25 (FIXED): removing the PRIMARY zero datum silently redefined the
// origin every other datum is expressed in, and nothing re-anchored - so the
// next unrelated shot edit teleported a whole connected component
// ===========================================================================

/// The raw world position of one station, measured the way
/// `Project::datum_anchor_position` measures the anchor a shot edit holds -
/// `compute_station_raw_coords` (models/project/mutations.rs), not the resolved
/// view.
fn raw_station_xy(state: &AppState, station: StationRef) -> (f64, f64) {
    let project = state.get_project_state().expect("no project open");
    let coords = project
        .compute_station_raw_coords(station)
        .expect("station has no raw coordinates");
    (coords.x, coords.y)
}

/// Every zero datum after the first is stored as a GPS position and RESOLVED
/// against the first one: `set_zero_datum` places it at
/// `gps_to_local(gps, zero_datums.first().gps)` and only the first datum sits at
/// the local origin (models/project/mutations.rs). The same rule used to be
/// re-applied, from scratch, by `realign_after_shot_edit` after any
/// geometry-changing shot edit:
///
/// ```text
/// let is_first = self.zero_datums.first().map(|d| d.station_ref == datum.station_ref) ...
/// let (target_x, target_y) = if is_first { (0.0, 0.0) }
///                            else if let Some(first) = self.zero_datums.first() {
///                                gps_to_local(&datum.gps, &first.gps)
///                            } ...
/// ```
///
/// So `zero_datums[0]` IS the project's coordinate origin, and removing it moves
/// the origin onto whichever datum is now first - a change of frame for every
/// other anchored component in the project.
///
/// Nothing re-solved for it. `remove_zero_datum` (core/state/command_station.rs)
/// does call `resolve_offsets()`, but that solver deliberately never touches an
/// anchor's own offset - "only the genuinely non-derivable anchors (datum
/// sections and free-placed floating components) keep their stored offset,
/// because the cascade never seed-adjusts them" (models/project/offsets.rs). A
/// datum section IS such an anchor, so the surviving datum kept the offset it was
/// given while it was still a SECONDARY datum, and the project was left holding a
/// placement its own rule no longer produces.
///
/// The bill arrived on the next unrelated edit: the re-derived ABSOLUTE target
/// rigidly translated the whole connected component onto the new origin in one
/// move - kilometres, for two datums a few hundred metres apart in
/// latitude/longitude.
///
/// FIXED, and kept as the regression guard: `update_shot` measures the group's
/// datum-station world position BEFORE the edit (`Project::datum_anchor_position`,
/// `core/state/command_shot.rs`) and passes it to `realign_after_shot_edit` as
/// `hold_anchor_at`, so the realign holds the anchor exactly where it found it
/// instead of re-georeferencing to the GPS target. The GPS fallback survives for
/// the `None` case, and the two answers are identical for a project whose frame
/// is self-consistent - they part company only on the promoted-datum
/// contradiction this test builds.
///
/// The oracle is the ANCHOR, not the section offset, because the offset is
/// allowed to move: this edit lengthens the shot UPSTREAM of the datum station by
/// 2 m, and holding the station still is precisely what slides the section by 2 m.
/// Both are asserted - the anchor to the bit, the slide to the edit's own
/// magnitude - so a re-georeference (kilometres, in either quantity) still fails.
///
/// Found by the Rust fuzzer's HORIZON mode, seed 2, at undo depth 1189: sections
/// 65 and 81 both off by exactly `(22230.0946076, -10480.744175)` with 81 landing
/// on `[0, 0]` - the offset a section gets when the surviving datum is its own
/// station 0. `fuzz_shrink_tests.rs` minimized that 1770-move trace to seven
/// moves, of which this is the reduction.
#[test]
fn removing_the_primary_zero_datum_re_anchors_the_datums_it_leaves_behind() {
    let _config = isolated_config();
    let state = state_with_project("OpenUndoBugs");
    let origin = create_section(
        &state,
        "Origin",
        vec![straight_shot(10.0, 3.0), straight_shot(20.0, 4.0)],
    );
    let anchored = create_section(
        &state,
        "Anchored",
        vec![straight_shot(70.0, 7.0), straight_shot(100.0, 8.0)],
    );

    // The FIRST datum is the project's local origin.
    let first = state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(origin, 0),
        coordinates: GpsCoordinates::new(20.4, -87.3, 0.0).unwrap(),
    });
    assert!(
        first.success,
        "SetZeroDatum (first) failed: {:?}",
        first.error
    );
    // The SECOND is placed relative to it - about 6.4 km away.
    let second = state.execute(Command::SetZeroDatum {
        station_ref: StationRef::new(anchored, 1),
        coordinates: GpsCoordinates::new(20.4358, -87.3479, 0.0).unwrap(),
    });
    assert!(
        second.success,
        "SetZeroDatum (second) failed: {:?}",
        second.error
    );
    let far = section_offset(&state, anchored).expect("the second datum should place its section");
    assert!(
        far.0.abs() > 1000.0,
        "setup: the second datum should sit kilometres from the first, got {far:?}"
    );

    // The primary datum goes away. `remove_zero_datum` re-solves offsets, so
    // whatever the project holds after this is what it believes - and what the
    // next edit has to hold.
    let removed = state.execute(Command::RemoveZeroDatum {
        station_ref: StationRef::new(origin, 0),
    });
    assert!(
        removed.success,
        "RemoveZeroDatum failed: {:?}",
        removed.error
    );
    let datum_station = StationRef::new(anchored, 1);
    let settled_anchor = raw_station_xy(&state, datum_station);
    let settled = section_offset(&state, anchored).expect("the anchored section lost its offset");

    // The removal promoted this datum to first, and local (0, 0) means
    // `zero_datums[0]`'s GPS - so the survey is translated onto the new origin
    // rather than left contradicting it (#469, item 1). Asserted here because it
    // is the state everything below is measured against.
    assert!(
        settled_anchor.0.abs() <= 1e-9 && settled_anchor.1.abs() <= 1e-9,
        "the promoted datum's station must sit at the local origin after the removal, got \
         {settled_anchor:?}: leaving it kilometres out re-georeferences the whole survey"
    );

    // An edit that has nothing to do with the datum, several moves later: 2 m of
    // extra length on the shot UPSTREAM of the datum station.
    let target = shot_ids(&state, anchored)[0];
    let edited = state.execute(Command::UpdateShot {
        section_id: anchored,
        shot_id: target,
        updates: ShotUpdates {
            length: Some(9.0),
            ..Default::default()
        },
    });
    assert!(edited.success, "UpdateShot failed: {:?}", edited.error);

    let now_anchor = raw_station_xy(&state, datum_station);
    let anchor_moved = (now_anchor.0 - settled_anchor.0).hypot(now_anchor.1 - settled_anchor.1);
    assert!(
        anchor_moved <= 1e-9,
        "an UpdateShot moved the promoted datum's own station {anchor_moved:.3} m (from \
         {settled_anchor:?} to {now_anchor:?}): removing the primary zero datum promoted this one \
         to the origin without re-solving, and the edit re-derived the ABSOLUTE GPS target instead \
         of holding the anchor where it found it, translating the whole connected component"
    );

    let after = section_offset(&state, anchored).expect("the anchored section lost its offset");
    let moved = (after.0 - settled.0).hypot(after.1 - settled.1);
    assert!(
        (moved - 2.0).abs() <= 1e-9,
        "the anchored section slid {moved:.3} m (from {settled:?} to {after:?}) for a 2 m \
         lengthening of the shot upstream of its datum station; with the anchor held, the edit's \
         own magnitude is the only motion allowed"
    );
}

/// The undo half, and the one the fuzzer's roundtrip oracle actually reports:
/// the teleport was not undone.
///
/// `UndoData::PreviousValues` restores the shot's fields, and the geometry
/// epilogue then runs `realign_after_shot_edit` AGAIN - against the same
/// (already promoted) first datum. So the undo re-derived the NEW frame rather
/// than restoring the offsets the edit found, and one `undo()` left the
/// project's geometry kilometres from where it was before the edit it undid.
///
/// This is what makes it an undo/redo finding rather than only a geometry one: a
/// single `undo` of a single `UpdateShot` is not a no-op on `section_offsets`.
///
/// FIXED with the forward half, and kept as the regression guard: the undo takes
/// the same `hold_anchor_at` route as the edit, so it holds the anchor where the
/// undo found it instead of re-georeferencing a second time. The oracle here is
/// still exact offset equality - unlike the forward tests, an undo has no edit of
/// its own to pay for, so every section has to land on the offset it had.
#[test]
fn undoing_the_edit_that_re_anchored_a_component_puts_it_back() {
    let _config = isolated_config();
    let state = state_with_project("OpenUndoBugs");
    let origin = create_section(
        &state,
        "Origin",
        vec![straight_shot(10.0, 3.0), straight_shot(20.0, 4.0)],
    );
    let anchored = create_section(
        &state,
        "Anchored",
        vec![straight_shot(70.0, 7.0), straight_shot(100.0, 8.0)],
    );

    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(origin, 0),
                coordinates: GpsCoordinates::new(20.4, -87.3, 0.0).unwrap(),
            })
            .success,
        "SetZeroDatum (first) failed"
    );
    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(anchored, 1),
                coordinates: GpsCoordinates::new(20.4358, -87.3479, 0.0).unwrap(),
            })
            .success,
        "SetZeroDatum (second) failed"
    );
    assert!(
        state
            .execute(Command::RemoveZeroDatum {
                station_ref: StationRef::new(origin, 0),
            })
            .success,
        "RemoveZeroDatum failed"
    );

    // Same promotion as the forward test: the removal translated the survey so the
    // surviving datum's station is the local origin (#469, item 1).
    let promoted = raw_station_xy(&state, StationRef::new(anchored, 1));
    assert!(
        promoted.0.abs() <= 1e-9 && promoted.1.abs() <= 1e-9,
        "the promoted datum's station must sit at the local origin after the removal, got \
         {promoted:?}"
    );

    let before = section_offset(&state, anchored);
    let target = shot_ids(&state, anchored)[0];
    assert!(
        state
            .execute(Command::UpdateShot {
                section_id: anchored,
                shot_id: target,
                updates: ShotUpdates {
                    length: Some(9.0),
                    ..Default::default()
                },
            })
            .success,
        "UpdateShot failed"
    );
    assert!(state.undo().success, "undo of UpdateShot failed");

    assert_eq!(
        section_offset(&state, anchored),
        before,
        "undoing the UpdateShot did not restore the section offset it found: the edit re-anchored \
         the component onto the datum that the primary datum's removal had promoted, and the undo \
         re-derives the same new frame instead of putting the old one back"
    );
}

/// The same defect reached the way a user reaches it, and the way the fuzzer did:
/// nobody removed a datum, they deleted a SHOT.
///
/// `DeleteShot` on the first shot of a section removes that section's station 0,
/// and the reference cleanup drops every stored ref anchored there - including a
/// zero datum keyed `StationAnchor::SectionStart`. If that datum was the primary
/// one, the project's origin has just moved and no command was issued that says
/// so. Everything after is finding 25 exactly as above.
///
/// This is the shape the shrinker minimized the horizon failure to: an empty
/// section, one appended shot, a second datum, `deleteShot`, a merge, and one
/// `updateShot`.
///
/// FIXED with the test above, and kept because the promotion arrives here through
/// a path that never mentions datums: `hold_anchor_at` is measured from the live
/// project, so it does not care WHICH command left the frame contradicting itself.
/// Same oracle as above - the anchor to the bit, the section's slide to the
/// edit's own 2 m.
#[test]
fn deleting_the_primary_datums_station_does_not_silently_move_the_origin() {
    let _config = isolated_config();
    let state = state_with_project("OpenUndoBugs");
    let origin = create_section(
        &state,
        "Origin",
        vec![straight_shot(10.0, 3.0), straight_shot(20.0, 4.0)],
    );
    let anchored = create_section(
        &state,
        "Anchored",
        vec![straight_shot(70.0, 7.0), straight_shot(100.0, 8.0)],
    );

    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(origin, 0),
                coordinates: GpsCoordinates::new(20.4, -87.3, 0.0).unwrap(),
            })
            .success,
        "SetZeroDatum (first) failed"
    );
    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(anchored, 1),
                coordinates: GpsCoordinates::new(20.4358, -87.3479, 0.0).unwrap(),
            })
            .success,
        "SetZeroDatum (second) failed"
    );

    // Deleting the first shot removes station 0, and with it the datum anchored
    // to `SectionStart`.
    let first_shot = shot_ids(&state, origin)[0];
    let deleted = state.execute(Command::DeleteShot {
        section_id: origin,
        shot_id: first_shot,
    });
    assert!(deleted.success, "DeleteShot failed: {:?}", deleted.error);
    let datum_station = StationRef::new(anchored, 1);
    let settled_anchor = raw_station_xy(&state, datum_station);
    let settled = section_offset(&state, anchored).expect("the anchored section lost its offset");

    let target = shot_ids(&state, anchored)[0];
    assert!(
        state
            .execute(Command::UpdateShot {
                section_id: anchored,
                shot_id: target,
                updates: ShotUpdates {
                    length: Some(9.0),
                    ..Default::default()
                },
            })
            .success,
        "UpdateShot failed"
    );

    let now_anchor = raw_station_xy(&state, datum_station);
    let anchor_moved = (now_anchor.0 - settled_anchor.0).hypot(now_anchor.1 - settled_anchor.1);
    assert!(
        anchor_moved <= 1e-9,
        "a DeleteShot that removed the primary datum's station left the survey on a stale origin, \
         and the next UpdateShot moved the promoted datum's own station {anchor_moved:.3} m (from \
         {settled_anchor:?} to {now_anchor:?}) with no command in between that asked for it"
    );

    let after = section_offset(&state, anchored).expect("the anchored section lost its offset");
    let moved = (after.0 - settled.0).hypot(after.1 - settled.1);
    assert!(
        (moved - 2.0).abs() <= 1e-9,
        "the anchored section slid {moved:.3} m (from {settled:?} to {after:?}) for a 2 m \
         lengthening of the shot upstream of its datum station; with the anchor held, the edit's \
         own magnitude is the only motion allowed"
    );
}

// ===========================================================================
// #469 item 1 (FIXED): promoting a different zero datum to first re-defined the
// georeference frame and moved nothing, so the whole survey slid on the map
// ===========================================================================

/// Every station of every section, keyed by ref. A section with N shots has
/// N + 1 stations.
fn all_station_xy(state: &AppState) -> Vec<(StationRef, (f64, f64))> {
    let project = state.get_project_state().expect("no project open");
    let mut out = Vec::new();
    for section in &project.sections {
        for index in 0..=section.shots.len() {
            let station = StationRef::new(section.id, index as u32);
            let coords = project
                .compute_station_raw_coords(station)
                .expect("station has no raw coordinates");
            out.push((station, (coords.x, coords.y)));
        }
    }
    out
}

/// The whole `section_offsets` map.
fn all_section_offsets(state: &AppState) -> std::collections::HashMap<SectionId, (f64, f64)> {
    state
        .get_project_state()
        .expect("no project open")
        .section_offsets()
        .clone()
}

/// The (latitude, longitude) of the datum that currently defines the frame.
/// `GpsCoordinates` is not `PartialEq`, so the pair is the comparable form.
fn frame_gps(state: &AppState) -> (f64, f64) {
    let project = state.get_project_state().expect("no project open");
    let gps = project
        .zero_datums
        .iter()
        .find(|zd| project.resolve_key(&zd.station_ref).is_some())
        .expect("no resolvable zero datum")
        .gps;
    (gps.latitude, gps.longitude)
}

/// THE property. Local `(0, 0)` means `zero_datums[0]`'s GPS, so when the first
/// datum goes away and the next is promoted, the survey has to be TRANSLATED onto
/// the new origin - otherwise every station's real-world position changes by the
/// full inter-datum vector while no command asked for anything of the kind.
///
/// The oracle is real-world position, expressed in one fixed ENU frame (the frame
/// the OLD datum defined, which is where the survey started): before the removal a
/// station's world position is its local xy; after it, it is
/// `gps_to_local(new_origin_gps, old_origin_gps)` plus its local xy. Both use the
/// app's own conversion, the one `MapCanvas` and the data table's GPS column use.
///
/// The third section is free-floating: no datum, no merge, no closure. It must
/// translate with everything else - its local coordinates are read against the
/// same origin, so leaving it behind would move it RELATIVE to the anchored
/// survey, which is the one thing a georeference change must not do.
#[test]
fn promoting_a_zero_datum_keeps_every_station_where_it_is_in_the_world() {
    let _config = isolated_config();
    let state = state_with_project("OpenUndoBugs");
    let origin = create_section(
        &state,
        "Origin",
        vec![straight_shot(10.0, 3.0), straight_shot(20.0, 4.0)],
    );
    let anchored = create_section(
        &state,
        "Anchored",
        vec![straight_shot(70.0, 7.0), straight_shot(100.0, 8.0)],
    );
    // Free-floating: no datum, no merge, no closure. `all_station_xy` sweeps it
    // like every other section, which is what puts it under the oracle below.
    let _floater = create_section(&state, "Floater", vec![straight_shot(300.0, 12.0)]);

    let first_gps = GpsCoordinates::new(20.4, -87.3, 0.0).unwrap();
    // About 400 m away: far enough that a missed translation is unmissable, close
    // enough that the two ENU tangent frames agree to well under a centimetre.
    let second_gps = GpsCoordinates::new(20.4027, -87.2973, 0.0).unwrap();

    for (station, gps) in [
        (StationRef::new(origin, 0), first_gps),
        (StationRef::new(anchored, 1), second_gps),
    ] {
        let set = state.execute(Command::SetZeroDatum {
            station_ref: station,
            coordinates: gps,
        });
        assert!(set.success, "SetZeroDatum failed: {:?}", set.error);
    }

    let before = all_station_xy(&state);
    let separation = raw_station_xy(&state, StationRef::new(anchored, 1));
    assert!(
        separation.0.hypot(separation.1) > 100.0,
        "setup: the second datum must sit well away from the first, got {separation:?}"
    );
    assert_eq!(
        frame_gps(&state),
        (first_gps.latitude, first_gps.longitude),
        "setup: the frame is the first datum"
    );

    let removed = state.execute(Command::RemoveZeroDatum {
        station_ref: StationRef::new(origin, 0),
    });
    assert!(
        removed.success,
        "RemoveZeroDatum failed: {:?}",
        removed.error
    );

    // The promoted datum's station IS the new local origin.
    let promoted = raw_station_xy(&state, StationRef::new(anchored, 1));
    assert!(
        promoted.0.abs() <= 1e-9 && promoted.1.abs() <= 1e-9,
        "the promoted datum's station must sit at local (0, 0), got {promoted:?}"
    );
    assert_eq!(
        frame_gps(&state),
        (second_gps.latitude, second_gps.longitude),
        "setup: the second datum must have been promoted"
    );

    // Nobody moved in the world.
    let shift = crate::models::gps_to_local(&second_gps, &first_gps);
    let after: std::collections::HashMap<StationRef, (f64, f64)> =
        all_station_xy(&state).into_iter().collect();
    for (station, world_before) in before {
        assert!(
            after.contains_key(&station),
            "station {station:?} vanished when the primary zero datum was removed"
        );
        let local_after = after[&station];
        let world_after = (shift.0 + local_after.0, shift.1 + local_after.1);
        let moved = (world_after.0 - world_before.0).hypot(world_after.1 - world_before.1);
        assert!(
            moved <= 1e-2,
            "station {station:?} moved {moved:.4} m in the WORLD when the primary zero datum was \
             removed (from {world_before:?} to {world_after:?}, both in the ENU frame of the \
             datum that was removed): the frame changed, so the survey has to be translated onto \
             the new origin rather than re-read against it"
        );
    }
}

/// The undo half. `RemoveZeroDatum` records a `ProjectSnapshotPair`, which
/// restores verbatim - so the translation is already exactly invertible, and this
/// pins it rather than fixing it. The oracle is the WHOLE `section_offsets` map,
/// key presence included: the renormalisation drops the promoted datum's own key
/// (it lands on the origin) and creates keys for sections that had none, so an
/// inverse that restored values but not presence would fail here.
#[test]
fn undoing_the_primary_datum_removal_restores_every_section_offset() {
    let _config = isolated_config();
    let state = state_with_project("OpenUndoBugs");
    let origin = create_section(
        &state,
        "Origin",
        vec![straight_shot(10.0, 3.0), straight_shot(20.0, 4.0)],
    );
    let anchored = create_section(
        &state,
        "Anchored",
        vec![straight_shot(70.0, 7.0), straight_shot(100.0, 8.0)],
    );
    let _floater = create_section(&state, "Floater", vec![straight_shot(300.0, 12.0)]);

    for (station, gps) in [
        (
            StationRef::new(origin, 0),
            GpsCoordinates::new(20.4, -87.3, 0.0).unwrap(),
        ),
        (
            StationRef::new(anchored, 1),
            GpsCoordinates::new(20.4027, -87.2973, 0.0).unwrap(),
        ),
    ] {
        assert!(
            state
                .execute(Command::SetZeroDatum {
                    station_ref: station,
                    coordinates: gps,
                })
                .success,
            "SetZeroDatum failed"
        );
    }

    let before = all_section_offsets(&state);
    assert!(
        state
            .execute(Command::RemoveZeroDatum {
                station_ref: StationRef::new(origin, 0),
            })
            .success,
        "RemoveZeroDatum failed"
    );
    let renormalised = all_section_offsets(&state);
    assert_ne!(
        renormalised, before,
        "setup: the removal must have translated the survey, or this proves nothing"
    );

    assert!(state.undo().success, "undo of RemoveZeroDatum failed");
    assert_eq!(
        all_section_offsets(&state),
        before,
        "undoing the removal did not put the section offsets back: the renormalisation moved \
         every section, so the inverse has to restore every one of them - values AND key presence"
    );

    assert!(state.redo().success, "redo of RemoveZeroDatum failed");
    assert_eq!(
        all_section_offsets(&state),
        renormalised,
        "redo has to land on the renormalised state the forward command produced"
    );
}

/// `SetZeroDatum`'s `UndoData::ZeroDatumInfo` now captures EVERY section's offset,
/// not the datum's merge group.
///
/// Undoing the add of the first datum REMOVES it, which is an inverse that
/// performs a frame change - and the frame is the one thing whose blast radius is
/// not bounded by any connectivity. The merge-group capture was exact only as long
/// as nothing outside that group could move; this pins the width directly, because
/// a behavioural test cannot distinguish the two captures on a project where
/// nothing outside the group happens to move.
#[test]
fn the_zero_datum_undo_capture_covers_every_section() {
    let _config = isolated_config();
    let state = state_with_project("OpenUndoBugs");
    let anchored = create_section(
        &state,
        "Anchored",
        vec![straight_shot(10.0, 3.0), straight_shot(20.0, 4.0)],
    );
    let _one = create_section(&state, "One", vec![straight_shot(70.0, 7.0)]);
    let _two = create_section(&state, "Two", vec![straight_shot(300.0, 12.0)]);

    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(anchored, 0),
                coordinates: GpsCoordinates::new(20.4, -87.3, 0.0).unwrap(),
            })
            .success,
        "SetZeroDatum failed"
    );

    let sections = state
        .get_project_state()
        .expect("no project open")
        .sections
        .len();

    let stack = state.undo_stack.lock();
    let recorded = stack.peek_undo_data();
    assert_eq!(
        recorded.map(crate::core::UndoData::variant_name),
        Some("ZeroDatumInfo"),
        "SetZeroDatum must record ZeroDatumInfo"
    );
    let Some(crate::core::UndoData::ZeroDatumInfo {
        previous_offsets, ..
    }) = recorded
    else {
        // Unreachable: the assertion above already named the variant.
        return;
    };
    assert_eq!(
        previous_offsets.len(),
        sections,
        "the capture covers {} of {sections} sections: the datum's merge group is not the set an \
         inverse that changes the frame has to hand back",
        previous_offsets.len()
    );
}

/// The behavioural half: adding the survey's first georeference and undoing it
/// puts every section offset back, presence included - and redoing it lands
/// byte-for-byte back on what the forward add produced.
///
/// The width matters now rather than only in principle. Establishing the frame
/// translates EVERY section (`Project::set_zero_datum` -> `renormalize_frame`),
/// free-floating components included, so an inverse that handed back only the
/// datum's merge group would leave the `Floater` section carrying the
/// establishing translation with nothing left to justify it. The floater is
/// asserted to move on the add, which is what makes the restore assertion below
/// bite: under the pre-fix add it never moved at all and either capture width
/// looked exact.
///
/// The redo half pins the same rule from the other side. `redo` re-executes the
/// command through `execute_without_undo`, which has no epilogue behind it, so a
/// design that left the re-framing to `AppState`'s `renormalize_frame_if_changed`
/// would redo onto the pre-renormalisation geometry and the offsets would differ
/// from the forward result by the full establishing delta.
#[test]
fn adding_the_first_zero_datum_and_undoing_it_restores_every_section_offset() {
    let _config = isolated_config();
    let state = state_with_project("OpenUndoBugs");
    let anchored = create_section(
        &state,
        "Anchored",
        vec![straight_shot(10.0, 3.0), straight_shot(20.0, 4.0)],
    );
    let partner = create_section(
        &state,
        "Partner",
        vec![straight_shot(70.0, 7.0), straight_shot(100.0, 8.0)],
    );
    // Outside the datum's merge group and outside every other connection: the
    // section whose movement separates "the group was placed on the origin" from
    // "the survey was re-framed onto it".
    let floater = create_section(&state, "Floater", vec![straight_shot(300.0, 12.0)]);

    // A merge so the add has a group wider than one section to move.
    assert!(
        state
            .execute(Command::MergeStations {
                station_a: StationRef::new(anchored, 2),
                station_b: StationRef::new(partner, 0),
            })
            .success,
        "MergeStations failed"
    );
    // Displace the group so the add has something to undo.
    assert!(
        state
            .execute(Command::MoveSection {
                section_id: anchored,
                offset: Coordinates {
                    x: 13.0,
                    y: -7.0,
                    z: 0.0,
                },
            })
            .success,
        "MoveSection failed"
    );

    let before = all_section_offsets(&state);
    let datum_station_before = raw_station_xy(&state, StationRef::new(anchored, 1));
    let floater_before = raw_station_xy(&state, StationRef::new(floater, 0));
    assert!(
        datum_station_before.0.hypot(datum_station_before.1) > 1.0,
        "setup: the datum station has to start away from the origin or the establishing \
         translation is nothing, got {datum_station_before:?}"
    );

    assert!(
        state
            .execute(Command::SetZeroDatum {
                station_ref: StationRef::new(anchored, 1),
                coordinates: GpsCoordinates::new(20.4, -87.3, 0.0).unwrap(),
            })
            .success,
        "SetZeroDatum failed"
    );
    assert_ne!(
        all_section_offsets(&state),
        before,
        "setup: adding the first datum must have moved the group"
    );
    let datum_station_after = raw_station_xy(&state, StationRef::new(anchored, 1));
    assert!(
        datum_station_after.0.abs() <= 1e-9 && datum_station_after.1.abs() <= 1e-9,
        "the first datum's station has to land on local (0, 0), got {datum_station_after:?}"
    );
    let delta = (-datum_station_before.0, -datum_station_before.1);
    let floater_after = raw_station_xy(&state, StationRef::new(floater, 0));
    assert!(
        (floater_after.0 - (floater_before.0 + delta.0)).abs() <= 1e-9
            && (floater_after.1 - (floater_before.1 + delta.1)).abs() <= 1e-9,
        "the section OUTSIDE the datum's merge group has to ride the establishing translation \
         {delta:?}: it went {floater_before:?} -> {floater_after:?}"
    );
    let after_add = all_section_offsets(&state);

    assert!(state.undo().success, "undo of SetZeroDatum failed");
    assert_eq!(
        all_section_offsets(&state),
        before,
        "undoing the add of the first zero datum did not restore the section offsets it found"
    );

    assert!(state.redo().success, "redo of SetZeroDatum failed");
    assert_eq!(
        all_section_offsets(&state),
        after_add,
        "redo has to land byte-for-byte on the geometry the forward add produced - it re-executes \
         the command with no epilogue behind it, so anything the re-framing leaves to `AppState` \
         rather than to the model shows up here as a redo that lands somewhere else"
    );
    let project = state.get_project_state().expect("no project open");
    let redone = project
        .zero_datums
        .first()
        .and_then(|zd| project.resolve_key(&zd.station_ref));
    assert_eq!(
        redone,
        Some(StationRef::new(anchored, 1)),
        "redo has to put the datum back on the station the forward add anchored"
    );
}

/// The renormalisation is not a RemoveZeroDatum feature. Deleting the SECTION the
/// primary datum lives in promotes the next datum through
/// `cleanup_section_references`, and the trigger is derived from the state rather
/// than tagged onto a command, so this route is covered by construction.
#[test]
fn deleting_the_primary_datums_section_renormalises_the_frame() {
    let _config = isolated_config();
    let state = state_with_project("OpenUndoBugs");
    let origin = create_section(
        &state,
        "Origin",
        vec![straight_shot(10.0, 3.0), straight_shot(20.0, 4.0)],
    );
    let anchored = create_section(
        &state,
        "Anchored",
        vec![straight_shot(70.0, 7.0), straight_shot(100.0, 8.0)],
    );

    for (station, gps) in [
        (
            StationRef::new(origin, 0),
            GpsCoordinates::new(20.4, -87.3, 0.0).unwrap(),
        ),
        (
            StationRef::new(anchored, 1),
            GpsCoordinates::new(20.4027, -87.2973, 0.0).unwrap(),
        ),
    ] {
        assert!(
            state
                .execute(Command::SetZeroDatum {
                    station_ref: station,
                    coordinates: gps,
                })
                .success,
            "SetZeroDatum failed"
        );
    }
    let separation = raw_station_xy(&state, StationRef::new(anchored, 1));
    assert!(
        separation.0.hypot(separation.1) > 100.0,
        "setup: the two datums must sit well apart, got {separation:?}"
    );

    let deleted = state.execute(Command::DeleteSection { section_id: origin });
    assert!(deleted.success, "DeleteSection failed: {:?}", deleted.error);

    let promoted = raw_station_xy(&state, StationRef::new(anchored, 1));
    assert!(
        promoted.0.abs() <= 1e-9 && promoted.1.abs() <= 1e-9,
        "deleting the primary datum's SECTION promoted the next datum and left the survey on the \
         old origin: the promoted station is at {promoted:?}, not local (0, 0)"
    );

    assert!(state.undo().success, "undo of DeleteSection failed");
    let restored = raw_station_xy(&state, StationRef::new(anchored, 1));
    assert!(
        (restored.0 - separation.0).abs() <= 1e-9 && (restored.1 - separation.1).abs() <= 1e-9,
        "the undo has to put the OLD frame back, not re-derive the new one: {separation:?} -> \
         {restored:?}"
    );
}

/// The delete/split family bypasses `execute()` entirely, so it carries its own
/// frame check. `execute_delete_last_shot_and_section` removes a whole section
/// (and every reference into it, the zero datum included), which is the same
/// promotion reached through a path that never mentions datums.
#[test]
fn the_delete_last_shot_path_renormalises_the_frame() {
    let _config = isolated_config();
    let state = state_with_project("OpenUndoBugs");
    let origin = create_section(&state, "Origin", vec![straight_shot(10.0, 3.0)]);
    let anchored = create_section(
        &state,
        "Anchored",
        vec![straight_shot(70.0, 7.0), straight_shot(100.0, 8.0)],
    );

    for (station, gps) in [
        (
            StationRef::new(origin, 0),
            GpsCoordinates::new(20.4, -87.3, 0.0).unwrap(),
        ),
        (
            StationRef::new(anchored, 1),
            GpsCoordinates::new(20.4027, -87.2973, 0.0).unwrap(),
        ),
    ] {
        assert!(
            state
                .execute(Command::SetZeroDatum {
                    station_ref: station,
                    coordinates: gps,
                })
                .success,
            "SetZeroDatum failed"
        );
    }
    let separation = raw_station_xy(&state, StationRef::new(anchored, 1));
    assert!(
        separation.0.hypot(separation.1) > 100.0,
        "setup: the two datums must sit well apart, got {separation:?}"
    );

    let only_shot = shot_ids(&state, origin)[0];
    let deleted = state.execute_delete_last_shot_and_section(origin, only_shot);
    assert!(
        deleted.success,
        "execute_delete_last_shot_and_section failed: {:?}",
        deleted.error
    );

    let promoted = raw_station_xy(&state, StationRef::new(anchored, 1));
    assert!(
        promoted.0.abs() <= 1e-9 && promoted.1.abs() <= 1e-9,
        "the delete/split path removed the primary datum's section without renormalising: the \
         promoted station is at {promoted:?}, not local (0, 0)"
    );

    assert!(state.undo().success, "undo of the delete failed");
    let restored = raw_station_xy(&state, StationRef::new(anchored, 1));
    assert!(
        (restored.0 - separation.0).abs() <= 1e-9 && (restored.1 - separation.1).abs() <= 1e-9,
        "the undo has to put the OLD frame back: {separation:?} -> {restored:?}"
    );
}

/// The renormalised offsets are what the file gives back. The loader is
/// deliberately NOT taught about the frame - `resolve_offsets`' cascade never
/// re-derives a seed's offset, so a whole-project translation is a provable no-op
/// for it, and a load-time renormalisation would silently translate a merged
/// project (whose datums keep their own offsets on purpose) on every open. This is
/// the gate on that reasoning.
#[test]
fn the_renormalised_frame_survives_save_and_reopen() {
    let _config = isolated_config();
    let temp = TempDir::new().unwrap();
    let save_path: PathBuf = temp.path().join("renormalised-frame.stalx");

    let (before_offsets, before_promoted) = {
        let state = state_with_project("OpenUndoBugs");
        let origin = create_section(
            &state,
            "Origin",
            vec![straight_shot(10.0, 3.0), straight_shot(20.0, 4.0)],
        );
        let anchored = create_section(
            &state,
            "Anchored",
            vec![straight_shot(70.0, 7.0), straight_shot(100.0, 8.0)],
        );
        let _floater = create_section(&state, "Floater", vec![straight_shot(300.0, 12.0)]);

        for (station, gps) in [
            (
                StationRef::new(origin, 0),
                GpsCoordinates::new(20.4, -87.3, 0.0).unwrap(),
            ),
            (
                StationRef::new(anchored, 1),
                GpsCoordinates::new(20.4027, -87.2973, 0.0).unwrap(),
            ),
        ] {
            assert!(
                state
                    .execute(Command::SetZeroDatum {
                        station_ref: station,
                        coordinates: gps,
                    })
                    .success,
                "SetZeroDatum failed"
            );
        }
        assert!(
            state
                .execute(Command::RemoveZeroDatum {
                    station_ref: StationRef::new(origin, 0),
                })
                .success,
            "RemoveZeroDatum failed"
        );

        let saved = state.save_project(Some(save_path.clone()));
        assert!(saved.success, "save failed: {:?}", saved.error);
        (
            all_section_offsets(&state),
            raw_station_xy(&state, StationRef::new(anchored, 1)),
        )
    };
    assert!(
        before_promoted.0.abs() <= 1e-9 && before_promoted.1.abs() <= 1e-9,
        "setup: the promoted datum must sit at the origin in session, got {before_promoted:?}"
    );

    let reopened = AppState::new().unwrap();
    let opened = reopened.execute(Command::OpenProject { path: save_path });
    assert!(opened.success, "reopen failed: {:?}", opened.error);

    assert_eq!(
        all_section_offsets(&reopened),
        before_offsets,
        "save + reopen did not reproduce the renormalised section offsets: the loader must be a \
         no-op on a whole-project translation, not a second solve that undoes it"
    );
}

// ===========================================================================
// #469 item 2 (FIXED): a loop closure ASSERTED that its endpoints coincided,
// and nothing in the app could ever set the claim straight
// ===========================================================================
//
// `create_loop_closure` stamped `is_aligned: strategy == Move` without
// measuring, one line after `compute_closure_metrics` had computed the
// refutation. The flag is a RIGID edge to the offset cascade, to the move
// tools, to co-location and to the frozen-shot rule, and nothing ever set it
// back to false: `realign_merges` looks at merges only, and the cascade
// deliberately leaves a redundant loop's residual intact. So a closure born
// torn - or torn later by a `moveStation` on one endpoint - was reported
// Critical forever, with no way out but deleting a legitimate closure.
//
// It is MEASURED now, by one predicate re-derived after every geometry change
// (`Project::closure_alignment_stamp` / `remeasure_closure_alignment`).

/// The alignment claim of a closure between `a` and `b`, or `None` when there
/// is no closure over that pair.
fn closure_alignment(state: &AppState, a: StationRef, b: StationRef) -> Option<bool> {
    let project = state.get_project_state()?;
    project
        .loop_closures
        .iter()
        .find(|c| {
            (project.key_is_ref(&c.station_a, a) && project.key_is_ref(&c.station_b, b))
                || (project.key_is_ref(&c.station_a, b) && project.key_is_ref(&c.station_b, a))
        })
        .map(|c| c.is_aligned)
}

/// Two merged sections with the far end of B snapped onto A's first station,
/// closing the loop as one physical point, and a `Move` closure recorded over
/// it - the state the snap-to-merge workflow produces. Returns `(a, b)`.
///
/// `anchored` picks whether the loop carries a zero datum. It changes what can
/// be done to the loop afterwards, not what it is: on an ANCHORED loop every
/// shot reaches the datum from both ends, so the frozen-shot guard refuses a
/// geometry edit outright and only a station move can tear it.
fn snapped_loop(state: &AppState, anchored: bool) -> (SectionId, SectionId) {
    let (a, b) = if anchored {
        two_merged_sections(state)
    } else {
        two_unanchored_merged_sections(state)
    };
    let (from, to) = station_raw_xy(state, (b, 1), (a, 0));
    let snapped = state.execute(Command::MoveStation {
        station_ref: StationRef::new(b, 1),
        offset: Coordinates {
            x: to.0 - from.0,
            y: to.1 - from.1,
            z: 0.0,
        },
    });
    assert!(snapped.success, "snap failed: {:?}", snapped.error);
    let closed = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(a, 0),
        station_b: StationRef::new(b, 1),
        closure_strategy: ClosureStrategy::Move,
    });
    assert!(
        closed.success,
        "CreateLoopClosure failed: {:?}",
        closed.error
    );
    (a, b)
}

/// The birth route: the fuzzer's `createLoopClosure(..., Move)` on an arbitrary
/// pair, which is also every IPC and harness caller. The two stations here are
/// 11 m apart and the app knows it - `compute_closure_metrics` records exactly
/// that distance on the closure one line earlier.
///
/// This is the shape the COMBINED regime kept reaching: "marked closed but its
/// ends are 5.86 m apart" in the post-run integrity sweep, with every in-run
/// oracle green because the lie was stable in both directions.
#[test]
fn a_move_closure_over_a_real_gap_is_not_born_aligned() {
    let _config = isolated_config();
    let state = state_with_project("OpenUndoBugs");
    let (a, b) = two_merged_sections(&state);

    let closed = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(a, 0),
        station_b: StationRef::new(b, 1),
        closure_strategy: ClosureStrategy::Move,
    });
    assert!(
        closed.success,
        "CreateLoopClosure failed: {:?}",
        closed.error
    );

    let gap = raw_station_gap(&state, (a, 0), (b, 1));
    assert!(
        gap > 1.0,
        "setup: the endpoints are metres apart (got {gap} m)"
    );
    assert_eq!(
        closure_alignment(&state, StationRef::new(a, 0), StationRef::new(b, 1)),
        Some(false),
        "a closure cannot claim a coincidence the app measured the refutation of"
    );
    let misclosure = state
        .get_project_state()
        .expect("no project open")
        .loop_closures[0]
        .misclosure_distance;
    assert!(
        (misclosure - gap).abs() < 1e-6,
        "nothing is lost: the distance lives in the misclosure the user reads ({misclosure} vs {gap})"
    );
    assert!(
        !state
            .integrity_check()
            .messages
            .iter()
            .any(|m| m.contains("marked closed")),
        "and the Critical banner that had no cure is not raised: {:?}",
        state.integrity_check().messages
    );
}

/// The other half: the REAL workflow, which is a batch - the move commands that
/// snapped the station onto its target, then the closure. The frontend
/// (`ClosureDialog.tsx`) sends `Move` only with those `preCommands`, so the
/// measurement says exactly what the assertion used to, and the user-facing
/// path is unchanged.
#[test]
fn the_snap_to_merge_batch_still_creates_an_aligned_closure() {
    let _config = isolated_config();
    let state = state_with_project("OpenUndoBugs");
    let (a, b) = two_merged_sections(&state);
    let (from, to) = station_raw_xy(&state, (b, 1), (a, 0));

    let batched = state.execute(Command::Batch {
        commands: vec![
            Command::MoveStation {
                station_ref: StationRef::new(b, 1),
                offset: Coordinates {
                    x: to.0 - from.0,
                    y: to.1 - from.1,
                    z: 0.0,
                },
            },
            Command::CreateLoopClosure {
                station_a: StationRef::new(a, 0),
                station_b: StationRef::new(b, 1),
                closure_strategy: ClosureStrategy::Move,
            },
        ],
    });
    assert!(
        batched.success,
        "snap-to-merge batch failed: {:?}",
        batched.error
    );

    assert!(
        raw_station_gap(&state, (a, 0), (b, 1)) < 1e-6,
        "setup: the snap made the two stations one point"
    );
    assert_eq!(
        closure_alignment(&state, StationRef::new(a, 0), StationRef::new(b, 1)),
        Some(true),
        "a measured coincidence is still an alignment - the workflow is unchanged"
    );
}

/// THE CRUX, over the two undo captures that can reach a torn closure.
///
/// The fuzz roundtrip oracle requires that an undo restores the state the
/// history RECORDED, and the recorded state had the claim TRUE. `UpdateShot`
/// is the sharp case: its `UndoData::PreviousValues` carries the shot's old
/// measurements and the rigid group's offsets, and NOT one closure flag - so a
/// sweep that could only DEMOTE would hand the geometry back with the claim
/// stuck false, landing on a state the history never held. `MoveStation`
/// records a whole `ProjectSnapshotPair`, which restores the flag verbatim, so
/// it pins the other half: a sweep that runs on the inverse must not disturb
/// what the snapshot just put back.
///
/// The stamp is total and blind to the flags, so both come out the same way -
/// the undo re-derives exactly the claim the recorded state had.
///
/// The narrow-capture row runs on an UNANCHORED loop, and has to: on an
/// anchored one every shot of the loop reaches the datum from both ends, so the
/// frozen-shot guard refuses the edit before it can tear anything - which is
/// itself one of the things an alignment claim buys, and why an honest claim
/// matters.
#[test]
fn tearing_an_aligned_closure_demotes_it_and_the_undo_restores_the_claim() {
    let _config = isolated_config();
    for narrow_capture in [false, true] {
        let state = state_with_project("OpenUndoBugs");
        let (a, b) = snapped_loop(&state, !narrow_capture);
        let closed_geometry = all_station_xy(&state);
        assert_eq!(
            closure_alignment(&state, StationRef::new(a, 0), StationRef::new(b, 1)),
            Some(true),
            "setup: the loop is closed and the claim is true"
        );

        let torn = if narrow_capture {
            // `UpdateShot` - a narrow capture, and the only route of the two that
            // a demote-only sweep gets wrong.
            state.execute(Command::UpdateShot {
                section_id: b,
                shot_id: shot_ids(&state, b)[0],
                updates: ShotUpdates {
                    length: Some(105.0),
                    ..Default::default()
                },
            })
        } else {
            state.execute(Command::MoveStation {
                station_ref: StationRef::new(b, 1),
                offset: Coordinates {
                    x: 100.0,
                    y: 0.0,
                    z: 0.0,
                },
            })
        };
        assert!(torn.success, "the tearing command failed: {:?}", torn.error);
        assert!(
            raw_station_gap(&state, (a, 0), (b, 1)) > 1.0,
            "setup (narrow_capture={narrow_capture}): the edit tore the loop open"
        );
        assert_eq!(
            closure_alignment(&state, StationRef::new(a, 0), StationRef::new(b, 1)),
            Some(false),
            "the claim is false now, so the flag must be"
        );
        assert!(
            !state
                .integrity_check()
                .messages
                .iter()
                .any(|m| m.contains("marked closed")),
            "the demotion IS the 'recompute' the old Critical asked for: {:?}",
            state.integrity_check().messages
        );

        let undone = state.undo();
        assert!(undone.success, "undo failed: {:?}", undone.error);
        assert_eq!(
            all_station_xy(&state),
            closed_geometry,
            "the undo restores the geometry (narrow_capture={narrow_capture})"
        );
        assert_eq!(
            closure_alignment(&state, StationRef::new(a, 0), StationRef::new(b, 1)),
            Some(true),
            "AND the claim, which is what a demote-only sweep could never do \
             (narrow_capture={narrow_capture})"
        );
    }
}

/// Drift back into coincidence: the flag follows the geometry in BOTH
/// directions, which is the same property the undo above relies on, reached by
/// two forward commands instead of an inverse.
#[test]
fn drifting_back_into_coincidence_re_promotes_the_closure() {
    let _config = isolated_config();
    let state = state_with_project("OpenUndoBugs");
    let (a, b) = snapped_loop(&state, true);

    for (offset, expected) in [(100.0, false), (-100.0, true)] {
        let moved = state.execute(Command::MoveStation {
            station_ref: StationRef::new(b, 1),
            offset: Coordinates {
                x: offset,
                y: 0.0,
                z: 0.0,
            },
        });
        assert!(
            moved.success,
            "MoveStation({offset}) failed: {:?}",
            moved.error
        );
        assert_eq!(
            closure_alignment(&state, StationRef::new(a, 0), StationRef::new(b, 1)),
            Some(expected),
            "after moving by {offset} m the gap is {} m",
            raw_station_gap(&state, (a, 0), (b, 1))
        );
    }
}

/// The double-anchor term, on the one shape where it is the ONLY thing
/// refusing: two independently GPS-anchored sections whose stations are
/// coincident. Aligning the closure would weld two datum-anchored merge
/// components into one rigid group - `cascade_bfs` pre-loads every
/// datum-bearing section as authoritative and never traverses an edge into one,
/// so nothing could ever satisfy it. That is #464 defect A, and
/// `plan_merge_promotion` refuses it for the same reason.
#[test]
fn a_coincident_closure_between_two_anchored_groups_is_never_aligned() {
    let _config = isolated_config();
    let state = state_with_project("OpenUndoBugs");
    let a = create_section(&state, "A", vec![straight_shot(0.0, 5.0)]);
    let c = create_section(&state, "C", vec![straight_shot(90.0, 5.0)]);
    // The SAME GPS on both: `set_zero_datum` puts the first datum's station at
    // the local origin and every later one at `gps_to_local(gps, first.gps)`, so
    // identical coordinates place both stations on exactly one point.
    let gps = GpsCoordinates::new(20.4, -87.3, 0.0).unwrap();
    for station in [StationRef::new(a, 0), StationRef::new(c, 0)] {
        let datum = state.execute(Command::SetZeroDatum {
            station_ref: station,
            coordinates: gps,
        });
        assert!(datum.success, "SetZeroDatum failed: {:?}", datum.error);
    }
    assert!(
        raw_station_gap(&state, (a, 0), (c, 0)) < 1e-6,
        "setup: the two anchored stations are one point"
    );

    let closed = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(a, 0),
        station_b: StationRef::new(c, 0),
        closure_strategy: ClosureStrategy::Move,
    });
    assert!(
        closed.success,
        "CreateLoopClosure failed: {:?}",
        closed.error
    );
    assert_eq!(
        closure_alignment(&state, StationRef::new(a, 0), StationRef::new(c, 0)),
        Some(false),
        "coincident is not enough: a rigid edge here would double-anchor the group"
    );

    // And no later command promotes it either - the term is a function of the
    // merges and the datums, both of which this leaves exactly as they were.
    let commented = state.execute(Command::MoveSection {
        section_id: c,
        offset: Coordinates {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
    });
    assert!(
        commented.success,
        "MoveSection failed: {:?}",
        commented.error
    );
    assert_eq!(
        closure_alignment(&state, StationRef::new(a, 0), StationRef::new(c, 0)),
        Some(false)
    );
}

/// CONVERGENCE. `assign_closure_ids` used to force `is_aligned = true` on every
/// `Move` closure on every open, so a closure honestly saved as `0` came
/// straight back as `1`: memory and disk would have disagreed from that open
/// onward, which is the #413 finding-10 shape and exactly what the persistence
/// oracle exists to catch.
#[test]
fn a_demoted_alignment_claim_survives_save_and_reopen() {
    let _config = isolated_config();
    let temp = TempDir::new().unwrap();
    let save_path: PathBuf = temp.path().join("demoted-claim.stalx");

    let (a, b) = {
        let state = state_with_project("OpenUndoBugs");
        let (a, b) = snapped_loop(&state, true);
        let torn = state.execute(Command::MoveStation {
            station_ref: StationRef::new(b, 1),
            offset: Coordinates {
                x: 100.0,
                y: 0.0,
                z: 0.0,
            },
        });
        assert!(torn.success, "MoveStation failed: {:?}", torn.error);
        assert_eq!(
            closure_alignment(&state, StationRef::new(a, 0), StationRef::new(b, 1)),
            Some(false),
            "setup: the claim is demoted in session"
        );
        let saved = state.save_project(Some(save_path.clone()));
        assert!(saved.success, "save failed: {:?}", saved.error);
        (a, b)
    };

    let reopened = AppState::new().unwrap();
    let opened = reopened.execute(Command::OpenProject { path: save_path });
    assert!(opened.success, "reopen failed: {:?}", opened.error);
    assert_eq!(
        closure_alignment(&reopened, StationRef::new(a, 0), StationRef::new(b, 1)),
        Some(false),
        "the open must not re-assert the claim the session measured away"
    );
    assert!(
        !reopened
            .integrity_check()
            .messages
            .iter()
            .any(|m| m.contains("marked closed")),
        "and the reopened project is clean: {:?}",
        reopened.integrity_check().messages
    );
}

// ===========================================================================
// Finding 12, CASCADING (FIXED): the shot the stale id edited by mistake still
// carried the edit several undos later, when the entry being consumed was
// unrelated and its own persisted id was FRESH
// ===========================================================================

/// Finding 12 is a single move going wrong; this is what a user sees, and it is
/// why the Rust fuzzer reported it as something else for a while.
///
/// After a reload, the undo of `UpdateShot` named its shot by an id the loader
/// had since reassigned, so it wrote the pre-edit values onto a NEIGHBOUR and
/// left the edited shot as it was (finding 12). Nothing put either half back:
/// the corruption sat in the PROJECT, not in the entry, so every undo below it
/// restored a state that still carried it - including undos of entries pushed on
/// a completely different section, whose own persisted ids resolve perfectly.
///
/// FIXED with finding 12 itself, and kept as the regression guard: undo/redo
/// entries address shots by UUID (additive serde fields, with the legacy id as
/// the fallback), so the renumbering the loader performs no longer decides which
/// shot an entry means. There is no first wrong write left for a later undo to
/// preserve.
///
/// That last part is what made this look like a new defect. The fuzzer measures
/// staleness on the entry it is consuming NOW, so at the depth this fails the
/// measurement reads `target=Fresh`: the `SetShotKind` entry below the
/// `UpdateShot` names a shot on another section whose id did not move. The
/// damage was done one move earlier, by an entry that measured `Collides`.
///
/// Root-caused from `fuzz_shrink_tests.rs`, which minimized the 59-move seed 10
/// run (`PERSISTENCE=1 CROSSINGS=3 ITERATIONS=60`) to three moves - reproduced
/// here directly. The insert-BEFORE is the whole setup: it makes the allocator's
/// ids disagree with positional order, and the loader renumbers to
/// `section_id * FEATURE_ID_STRIDE + sequence_order`
/// (persistence/sqlite/read.rs), which swaps the two.
///
/// Pinned separately from the two finding 12 tests above because they assert the
/// FIRST undo and this asserts that the second one cannot clean up after it -
/// exactly the relationship finding 15 has to finding 3.
#[test]
fn unwinding_past_a_stale_id_edit_leaves_no_shot_holding_it() {
    let _config = isolated_config();
    let temp = TempDir::new().unwrap();
    let save_path: PathBuf = temp.path().join("stale-cascade.stalx");

    let edited_section;
    let other_section;
    let inserted_uuid;
    let neighbour_uuid;
    let inserted_length;
    let neighbour_length;
    {
        let state = state_with_project("OpenUndoBugs");
        edited_section = create_section(&state, "Edited", vec![straight_shot(0.0, 5.0)]);
        other_section = create_section(&state, "Other", vec![straight_shot(90.0, 6.0)]);

        // Move 1: insert BEFORE the existing shot, so position 0 holds the
        // HIGHEST id the allocator handed out.
        let existing = shot_ids(&state, edited_section)[0];
        let created = state.execute(Command::CreateShot {
            section_id: edited_section,
            shot: straight_shot(180.0, 7.0),
            after_shot_id: None,
            before_shot_id: Some(existing),
        });
        assert!(created.success, "CreateShot failed: {:?}", created.error);
        let uuids = shot_uuids(&state, edited_section);
        inserted_uuid = uuids[0].clone().expect("inserted shot has no uuid");
        neighbour_uuid = uuids[1].clone().expect("neighbour shot has no uuid");
        let lengths = shot_lengths(&state, edited_section);
        inserted_length = lengths[0];
        neighbour_length = lengths[1];

        // Move 2: an edit on a DIFFERENT section, so the entry below the one
        // that goes wrong is one whose persisted id will still be correct.
        let other_shot = shot_ids(&state, other_section)[0];
        let marked = state.execute(Command::SetShotKind {
            section_id: other_section,
            shot_id: other_shot,
            kind: ShotKind::Jump,
        });
        assert!(marked.success, "SetShotKind failed: {:?}", marked.error);

        // Move 3: the edit whose undo will use the stale id.
        let target = shot_ids(&state, edited_section)[0];
        let updated = state.execute(Command::UpdateShot {
            section_id: edited_section,
            shot_id: target,
            updates: ShotUpdates {
                length: Some(42.5),
                ..Default::default()
            },
        });
        assert!(updated.success, "UpdateShot failed: {:?}", updated.error);

        let saved = state.save_project(Some(save_path.clone()));
        assert!(saved.success, "save failed: {:?}", saved.error);
    }

    let reopened = AppState::new().unwrap();
    let opened = reopened.execute(Command::OpenProject { path: save_path });
    assert!(opened.success, "reopen failed: {:?}", opened.error);

    // Undo the UpdateShot (finding 12 fires here) and then the SetShotKind,
    // which is on the other section entirely.
    assert!(reopened.undo().success, "undo of UpdateShot failed");
    assert!(reopened.undo().success, "undo of SetShotKind failed");

    let project = reopened.get_project_state().expect("no project after undo");
    let section = project
        .sections
        .iter()
        .find(|s| s.id == edited_section)
        .expect("section missing");
    let length_of = |uuid: &str| -> f64 {
        section
            .shots
            .iter()
            .find(|s| s.uuid.as_deref() == Some(uuid))
            .expect("shot vanished")
            .length
    };

    assert!(
        (length_of(&inserted_uuid) - inserted_length).abs() < 1e-9
            && (length_of(&neighbour_uuid) - neighbour_length).abs() < 1e-9,
        "unwinding two entries past the stale-id edit left section {edited_section} holding \
         {:?} instead of {:?}: the undo of UpdateShot wrote the pre-edit length onto the \
         NEIGHBOUR (finding 12) and the undo below it - a SetShotKind on section \
         {other_section}, whose own persisted id is fresh - restores a state that still carries \
         the swap",
        (length_of(&inserted_uuid), length_of(&neighbour_uuid)),
        (inserted_length, neighbour_length)
    );
}

// ===========================================================================
// Finding 5, on the COMMAND side (FIXED): a reloaded UpdateSectionMetadata no
// longer knew it was clearing a field - so the entry mis-described itself, and
// its redo silently under-applied
// ===========================================================================

/// `SectionUpdates::name` / `surveyor` / `date` were `Option<Option<T>>` behind a
/// plain `#[serde(default)]` with no `double_option` (models/section.rs).
/// `Some(None)` ("clear it") and `None` ("leave it alone") therefore BOTH
/// serialized to `null`, and `null` read back as `None` - so the distinction
/// survived the write and died on the read.
///
/// Finding 5 already pins that gap on the entry's `values_json`, where it costs
/// an UNDO. This is the same gap on the entry's stored COMMAND, where it costs a
/// REDO: `redo_inner` re-executes `entry.command` for a `PreviousValues` entry
/// (its catch-all arm, core/state/undo.rs), and a reloaded command that says
/// "leave the surveyor alone" leaves it alone.
///
/// The previous value here is deliberately NON-null ("Ferreira"), so the undo in
/// the middle is one finding 5 does not touch: what this isolates is the redo.
/// Nothing in the sequence is unusual - clear a field, save, reopen, undo, redo -
/// and the survey silently kept a surveyor the user removed.
///
/// FIXED, and kept as the regression guard: the three nullable fields carry
/// `deserialize_with = "double_option"` + `skip_serializing_if = "Option::is_none"`
/// (models/section.rs), so "clear it" writes `null` and "leave it alone" writes
/// nothing at all, and the two survive the round trip apart. Because the meaning
/// of an explicit null FLIPPED, schema v17 drops pre-v17 undo history rather than
/// reinterpreting it - the v13/v15/v16 precedent.
///
/// Found by `fuzz_undo_tests.rs`'s entry-content oracle, which sees the harmless
/// half of the same defect first: the reloaded entry's history-dropdown preview
/// stops listing the Surveyor row (`the_history_preview_still_reports_a_cleared_field`).
#[test]
fn redo_after_reload_clears_the_section_field_it_cleared() {
    let _config = isolated_config();
    let temp = TempDir::new().unwrap();
    let save_path: PathBuf = temp.path().join("cleared-field.stalx");

    let section_id;
    {
        let state = state_with_project("OpenUndoBugs");
        let created = state.execute(Command::CreateSection {
            name: Some("Sump".to_string()),
            surveyor: Some("Ferreira".to_string()),
            date: None,
            color: "#3388FF".to_string(),
            direction: None,
            shots: vec![straight_shot(0.0, 5.0)],
        });
        assert!(created.success, "CreateSection failed: {:?}", created.error);
        section_id = created.section_id.expect("CreateSection returned no id");

        // The edit under test: CLEAR the surveyor. `Some(None)`, not `None`.
        let cleared = state.execute(Command::UpdateSectionMetadata {
            section_id,
            updates: SectionUpdates {
                surveyor: Some(None),
                ..Default::default()
            },
        });
        assert!(cleared.success, "clearing failed: {:?}", cleared.error);
        assert_eq!(
            surveyor_of(&state, section_id),
            CLEARED,
            "setup: the clear did not take effect before the save"
        );

        let saved = state.save_project(Some(save_path.clone()));
        assert!(saved.success, "save failed: {:?}", saved.error);
    }

    let reopened = AppState::new().unwrap();
    let opened = reopened.execute(Command::OpenProject { path: save_path });
    assert!(opened.success, "reopen failed: {:?}", opened.error);

    let undone = reopened.undo();
    assert!(
        undone.success,
        "undo after reopen failed: {:?}",
        undone.error
    );
    assert_eq!(
        surveyor_of(&reopened, section_id),
        "Ferreira",
        "setup: the undo did not put the surveyor back, so this test is measuring finding 5's \
         UNDO half rather than its command half"
    );

    let redone = reopened.redo();
    assert!(
        redone.success,
        "redo after reopen failed: {:?}",
        redone.error
    );

    assert_eq!(
        surveyor_of(&reopened, section_id),
        CLEARED,
        "the redo of a reloaded 'clear the surveyor' left the surveyor in place: the persisted \
         command's `Some(None)` came back as `None`, which `update_section` reads as 'leave it \
         alone'"
    );
}

/// The same defect, one step earlier and visible without touching the project:
/// the history dropdown stopped saying the entry cleared anything.
///
/// `derive_preview_data` builds the `UpdateSectionMetadata` rows from
/// `updates.surveyor` (commands/history.rs), so an entry whose `Some(None)` had
/// decayed to `None` rendered no Surveyor row at all - and when the cleared field
/// was the only thing the edit changed, `preview_data` became `null` and the
/// hover was empty. The entry still UNDID correctly here; it just no longer said
/// what it did.
///
/// FIXED with the redo half above (the same `double_option` round trip), and
/// kept because it is the cheaper tripwire: it fails on a preview string without
/// needing an undo, a redo or a project to compare, so a serde regression on
/// `SectionUpdates` is caught here first.
#[test]
fn the_history_preview_still_reports_a_cleared_field() {
    let _config = isolated_config();
    let temp = TempDir::new().unwrap();
    let save_path: PathBuf = temp.path().join("cleared-preview.stalx");

    let before;
    {
        let state = state_with_project("OpenUndoBugs");
        let created = state.execute(Command::CreateSection {
            name: Some("Sump".to_string()),
            surveyor: Some("Ferreira".to_string()),
            date: None,
            color: "#3388FF".to_string(),
            direction: None,
            shots: vec![straight_shot(0.0, 5.0)],
        });
        assert!(created.success, "CreateSection failed: {:?}", created.error);
        let section_id = created.section_id.expect("CreateSection returned no id");

        let cleared = state.execute(Command::UpdateSectionMetadata {
            section_id,
            updates: SectionUpdates {
                surveyor: Some(None),
                ..Default::default()
            },
        });
        assert!(cleared.success, "clearing failed: {:?}", cleared.error);
        before = preview_fields(&state);
        assert!(
            before.contains(&"Surveyor".to_string()),
            "setup: the preview did not list the Surveyor row before the save, it listed {before:?}"
        );

        let saved = state.save_project(Some(save_path.clone()));
        assert!(saved.success, "save failed: {:?}", saved.error);
    }

    let reopened = AppState::new().unwrap();
    let opened = reopened.execute(Command::OpenProject { path: save_path });
    assert!(opened.success, "reopen failed: {:?}", opened.error);

    assert_eq!(
        preview_fields(&reopened),
        before,
        "the reloaded entry's history preview no longer lists the field it cleared"
    );
}

/// The newest undo entry's preview rows, by field label.
fn preview_fields(state: &AppState) -> Vec<String> {
    state
        .get_undo_stack()
        .first()
        .and_then(|item| item.preview_data.as_ref())
        .map(|preview| {
            preview
                .changes
                .iter()
                .map(|change| change.field.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// A section's surveyor, rendered so that a CLEARED field and a missing section
/// cannot read as the same thing.
fn surveyor_of(state: &AppState, section_id: SectionId) -> String {
    let project = state.get_project_state().expect("no project open");
    let section = project
        .sections
        .iter()
        .find(|section| section.id == section_id)
        .expect("section missing");
    section
        .surveyor
        .clone()
        .unwrap_or_else(|| CLEARED.to_string())
}

/// What [`surveyor_of`] renders for a field that has been cleared.
const CLEARED: &str = "<cleared>";

// ===========================================================================
// Finding 23 (FIXED), in the history dropdown: an f64 that serde_json wrote and
// its own parser could not read back
// ===========================================================================

/// `format_property_tests.rs` records finding 23 as "a save/load cycle is not a
/// fixpoint", with the gzipped `undo_stack` / `redo_stack` JSON blobs as one of
/// the two places the drift lived. This is that drift with its mechanism named
/// and a user-visible consequence attached.
///
/// `serde_json` writes an `f64` through ryu's shortest round-tripping form and
/// used to read it back with a parser that is not correctly rounded past 16
/// significant digits: `13.565000000000001` is written for the bits
/// `0x402b2147ae147ae2`, `str::parse::<f64>` returns exactly those bits, and the
/// default `serde_json::from_str` returned `0x402b2147ae147ae1` - one ULP low.
///
/// One ULP is invisible until it crosses a rendering boundary. The history
/// preview renders a shot length at two decimals, and this value sits exactly on
/// one: `13.57` before the save, `13.56` after it, on an entry nobody touched.
///
/// FIXED, and kept as the regression guard: the crate now depends on
/// `serde_json` with the `float_roundtrip` feature (src-tauri/Cargo.toml, where
/// the requirement is written up as REQUIRED rather than a preference), which
/// switches the parser to the correctly-rounded one - so every persisted undo
/// command carrying a 17-digit float reads back bit for bit. The other half of
/// the same finding, the loader's re-solve perturbing an already-right persisted
/// offset, was closed by `Project::OFFSET_DUST_M` (see bug 2 above).
///
/// Found by `fuzz_undo_tests.rs`'s entry-content oracle
/// (`finding-23-history-float-dust-moves-a-displayed-value`).
#[test]
fn a_reloaded_entry_displays_the_length_it_was_given() {
    let _config = isolated_config();
    let temp = TempDir::new().unwrap();
    let save_path: PathBuf = temp.path().join("ulp.stalx");

    // The bits ryu renders as `13.565000000000001`, one ULP above the f64
    // nearest to 13.565 - which is what serde_json's parser returns for it.
    let length = f64::from_bits(0x402b_2147_ae14_7ae2);
    let before;
    {
        let state = state_with_project("OpenUndoBugs");
        let section_id = create_section(&state, "Traverse", vec![straight_shot(0.0, 5.0)]);
        let shot_id = shot_ids(&state, section_id)[0];
        let updated = state.execute(Command::UpdateShot {
            section_id,
            shot_id,
            updates: ShotUpdates {
                length: Some(length),
                ..Default::default()
            },
        });
        assert!(updated.success, "UpdateShot failed: {:?}", updated.error);
        before = preview_values(&state);
        assert_eq!(
            before,
            vec!["13.57".to_string()],
            "setup: the preview did not render the length as expected before the save"
        );

        let saved = state.save_project(Some(save_path.clone()));
        assert!(saved.success, "save failed: {:?}", saved.error);
    }

    let reopened = AppState::new().unwrap();
    let opened = reopened.execute(Command::OpenProject { path: save_path });
    assert!(opened.success, "reopen failed: {:?}", opened.error);

    assert_eq!(
        preview_values(&reopened),
        before,
        "the reloaded entry displays a different length than the one it was created with; the \
         value moved by one ULP on the way through serde_json's own parser"
    );
}

/// The newest undo entry's preview rows, by rendered new value.
fn preview_values(state: &AppState) -> Vec<String> {
    state
        .get_undo_stack()
        .first()
        .and_then(|item| item.preview_data.as_ref())
        .map(|preview| {
            preview
                .changes
                .iter()
                .map(|change| change.new_value.clone())
                .collect()
        })
        .unwrap_or_default()
}

// ===========================================================================
// The orphan-closure observation (FIXED, both halves): a closure orphaned by
// one command, converted by an unrelated undo and by the next load
// ===========================================================================

/// HALF ONE, the FORWARD half. `RemoveZeroDatum` un-anchors one side of a
/// closure that bridges two otherwise unconnected groups. That changes no
/// topology, so the closure was already a sole bridge - what changes is that the
/// conversion's double-anchor guard stops refusing it, because there is no
/// longer a second GPS anchor to fuse into one rigid component. The command was
/// classified `can_orphan_closures: false` and converted nothing, while
/// `redrive_offsets_after_load` converts unconditionally, so from this move on
/// memory said "1 closure, 0 merges" and the next open of the same file said
/// "0 closures, 1 merge" (#413, the orphan-closure observation; the same
/// disagreement finding 10 fixed for the delete entry points, one command
/// further out).
///
/// FIXED by classifying `RemoveZeroDatum` as `can_orphan_closures` (the
/// `REMOVE_DATUM` profile in core/state/commands.rs), so `execute()`'s epilogue
/// converts at the instant the closure becomes convertible - exactly what the
/// loader would do, in session. `SetZeroDatum` needs no such change: ADDING an
/// anchor can only move the guard the other way, and a refused orphan is one the
/// loader leaves alone too.
///
/// The conversion is not a thing `ZeroDatumInfo` can invert, so the command's
/// undo capture moved to a `ProjectSnapshotPair` - the assertions below are as
/// much about that as about the conversion: undo has to put the closure AND the
/// datum back, and redo has to land on the converted state again.
#[test]
fn removing_a_zero_datum_converts_the_closure_it_makes_convertible() {
    let _config = isolated_config();
    let (state, _a, b) = detached_anchored_closure();
    let before = closure_and_merge_counts(&state);
    assert_eq!(
        before,
        (1, 0),
        "setup: while both groups are anchored the closure must survive as a closure"
    );
    let datums_before = datum_stations(&state);

    let removed = state.execute(Command::RemoveZeroDatum {
        station_ref: StationRef::new(b, 0),
    });
    assert!(
        removed.success,
        "RemoveZeroDatum failed: {:?}",
        removed.error
    );
    assert_eq!(
        closure_and_merge_counts(&state),
        (0, 1),
        "removing the second anchor made the sole-bridge closure convertible, so the command \
         must convert it in session - the next load will"
    );

    assert!(state.undo().success, "undo of RemoveZeroDatum failed");
    assert_eq!(
        closure_and_merge_counts(&state),
        before,
        "the undo has to put the CLOSURE back, not just the datum"
    );
    assert_eq!(
        datum_stations(&state),
        datums_before,
        "the undo has to put the datum back, at the index it was removed from (#413, finding 17)"
    );

    assert!(state.redo().success, "redo of RemoveZeroDatum failed");
    assert_eq!(
        closure_and_merge_counts(&state),
        (0, 1),
        "the redo has to land on the converted state the forward command produced"
    );
}

/// HALF ONE's consequence, stated the way a user meets it: what the session
/// holds after the removal is what the file gives back. This is the 14-move
/// fuzzer repro's essence - its `save/close/reopen` is where the divergence was
/// measured - and it was RED before the classification above, `(1, 0)` in memory
/// reopening as `(0, 1)`.
#[test]
fn removing_a_zero_datum_leaves_nothing_for_the_next_load_to_reclassify() {
    let _config = isolated_config();
    let temp = TempDir::new().unwrap();
    let save_path: PathBuf = temp.path().join("converged-orphan.stalx");

    let (before, before_offsets) = {
        let (state, _a, b) = detached_anchored_closure();
        let removed = state.execute(Command::RemoveZeroDatum {
            station_ref: StationRef::new(b, 0),
        });
        assert!(
            removed.success,
            "RemoveZeroDatum failed: {:?}",
            removed.error
        );
        let counts = closure_and_merge_counts(&state);
        let offsets = all_section_offsets(&state);
        let saved = state.save_project(Some(save_path.clone()));
        assert!(saved.success, "save failed: {:?}", saved.error);
        (counts, offsets)
    };

    let reopened = AppState::new().unwrap();
    let opened = reopened.execute(Command::OpenProject { path: save_path });
    assert!(opened.success, "reopen failed: {:?}", opened.error);

    assert_eq!(
        closure_and_merge_counts(&reopened),
        before,
        "save + reopen reclassified a loop closure as a station merge: the session that removed \
         the zero datum did not run the conversion the loader runs"
    );
    // Geometry converges too, not only topology. The removal here takes the
    // SECOND datum, so it changes no frame - but the closure->merge conversion it
    // triggers does move offsets, and the loader must reproduce them rather than
    // re-solve to something else (#469, item 1's convergence argument, which rests
    // on the loader being a no-op on what the session already settled).
    assert_eq!(
        all_section_offsets(&reopened),
        before_offsets,
        "save + reopen did not reproduce the section offsets the session settled on"
    );
}

/// HALF TWO, the UNDO half. `Project::convert_orphan_closures_to_merges` is a
/// WHOLE-PROJECT sweep, and both shot-removal paths ran it - including when they
/// run as the inverse of `CreateShot` (`remove_created_shot`). With a closure
/// left orphaned two moves earlier by an unrelated command, the undo of the shot
/// converted it and landed on a state the history never recorded: seeds 20 and
/// 35 of `COMBINED=1 SEEDS=40 ITERATIONS=300` failed on `undo x# did not restore
/// the state recorded at depth # | at loop_closures.length` with no save and no
/// reload involved.
///
/// FIXED by scoping the two removal paths to the orphans they THEMSELVES create
/// (`Project::convert_closures_orphaned_since`, models/project/offsets.rs): the
/// caller snapshots the already-orphaned ids before mutating and only closures
/// that are orphans afterwards and were not orphans before convert. The forward
/// user delete is unchanged (`execute()`'s epilogue still sweeps the project),
/// and the load-time and Repair sweeps still convert everything they find, which
/// is what finding 10's load-boundary fix rests on.
///
/// What this test can still pin, now that HALF ONE has converged the forward
/// path: the undo lands on the RECORDED state exactly. It cannot reach the
/// original red - a project holding a pre-existing convertible orphan is no
/// longer constructible through the command API, which IS half one's invariant -
/// so the scoping's own red/green guard is at the model level, where the state
/// can be built directly:
/// `removing_a_shot_converts_the_closure_it_orphans_and_not_an_older_one` and
/// its `remove_appended_first_shot` sibling in models/project/tests/orphan_closure_tests.rs.
///
/// The undo of `CreateShot` picks its inverse by shape, so both are pinned: this
/// one is the append into an EMPTY section (`remove_appended_first_shot`).
#[test]
fn undo_of_an_appended_first_shot_restores_the_recorded_closure_state() {
    let _config = isolated_config();
    let state = converged_orphan_state();
    let empty = create_section(&state, "Elsewhere", vec![]);
    let recorded = closure_and_merge_counts(&state);

    let created = state.execute(Command::CreateShot {
        section_id: empty,
        shot: straight_shot(180.0, 4.0),
        after_shot_id: None,
        before_shot_id: None,
    });
    assert!(created.success, "CreateShot failed: {:?}", created.error);
    assert!(state.undo().success, "undo of CreateShot failed");

    assert_eq!(
        closure_and_merge_counts(&state),
        recorded,
        "the undo of a shot creation moved an entry between loop_closures and station_merges: \
         the recorded state had {recorded:?} (closures, merges)"
    );
}

/// The other shape of the same undo: an append into a section that already has
/// a shot, whose inverse is `remove_shot_with_rewire`. Same requirement - the
/// undo puts the shot back and touches nothing else.
#[test]
fn undo_of_an_appended_shot_restores_the_recorded_closure_state() {
    let _config = isolated_config();
    let state = converged_orphan_state();
    let elsewhere = create_section(&state, "Elsewhere", vec![straight_shot(270.0, 6.0)]);
    let first = shot_ids(&state, elsewhere)[0];
    let recorded = closure_and_merge_counts(&state);

    let created = state.execute(Command::CreateShot {
        section_id: elsewhere,
        shot: straight_shot(180.0, 4.0),
        after_shot_id: Some(first),
        before_shot_id: None,
    });
    assert!(created.success, "CreateShot failed: {:?}", created.error);
    assert!(state.undo().success, "undo of CreateShot failed");

    assert_eq!(
        closure_and_merge_counts(&state),
        recorded,
        "the undo of a shot creation moved an entry between loop_closures and station_merges: \
         the recorded state had {recorded:?} (closures, merges)"
    );
}

/// The undo arm a stack persisted BEFORE the capture change still lands on, kept
/// working and kept honest about what it can do.
///
/// A legacy entry carries `ZeroDatumInfo`, so its undo restores the datum and the
/// offsets and cannot put back a closure that was converted - that is the defect
/// the pair fixed, and an old entry does not retroactively acquire the fix. What
/// it must still do is what it always did, and finding 17 is the part that is
/// easy to lose: the datum goes back at the INDEX it was removed from, not
/// appended, because `zero_datums` is persisted in order, indexed by the
/// frontend, and walked in order by `resolve_offsets` when it picks the datum
/// that anchors a connected group. The removal here takes the FIRST of the two
/// datums, so an append would be visible.
///
/// Taking the FIRST datum also changes the FRAME, so the forward command now
/// renormalises the whole survey (#469, item 1). A legacy entry's narrow
/// `previous_offsets` cannot hand a whole-project translation back - it restores
/// the merge group it recorded and nothing more, which is the same
/// degrade-gracefully rule as the closure. Both halves are asserted: the index,
/// and that the offsets it DID record came back verbatim.
#[test]
fn a_legacy_remove_zero_datum_entry_still_restores_the_datum_at_its_index() {
    let _config = isolated_config();
    let (state, a, _b) = detached_anchored_closure();
    let station = StationRef::new(a, 0);
    let datums_before = datum_stations(&state);

    // Exactly what the pre-pair capture recorded for this command.
    let (legacy, recorded_offsets) = {
        let project = state.get_project_state().expect("no project open");
        let (position, gps) = project
            .zero_datums
            .iter()
            .enumerate()
            .find(|(_, zd)| project.resolve_key(&zd.station_ref) == Some(station))
            .map(|(position, zd)| (position, zd.gps))
            .expect("setup: the datum being removed must exist");
        assert_eq!(position, 0, "setup: the removal must take the FIRST datum");
        let previous_offsets = project.capture_connected_section_offsets(a);
        (
            crate::core::UndoData::ZeroDatumInfo {
                station_ref: station,
                previous_gps: Some(gps),
                position: Some(position),
                previous_offsets: previous_offsets.clone(),
            },
            previous_offsets,
        )
    };

    let removed = state.execute(Command::RemoveZeroDatum {
        station_ref: station,
    });
    assert!(
        removed.success,
        "RemoveZeroDatum failed: {:?}",
        removed.error
    );

    // Swap the live pair for the shape a persisted stack still carries.
    {
        let mut stack = state.undo_stack.lock();
        stack.clear();
        stack.push_with_data(
            Command::RemoveZeroDatum {
                station_ref: station,
            },
            legacy,
        );
    }

    assert!(state.undo().success, "undo of the legacy entry failed");
    assert_eq!(
        datum_stations(&state),
        datums_before,
        "the legacy arm put the datum back in the wrong place: it re-inserts at the recorded \
         index rather than appending (#413, finding 17)"
    );

    // What it recorded, it restored - values and key presence both.
    let now = all_section_offsets(&state);
    for (section_id, recorded) in &recorded_offsets {
        assert_eq!(
            now.get(section_id).copied(),
            *recorded,
            "the legacy arm did not restore the offset it recorded for section {section_id}"
        );
    }
}

/// Two sections with no path between them, each with its own GPS anchor, and a
/// loop closure bridging them - which `create_loop_closure` allows only because
/// both are anchored. The closure is a sole bridge from the moment it exists and
/// survives as a closure only because converting it would put two anchors in one
/// rigid merge component. Returns the state and both section ids - the tests
/// take an anchor off one or the other.
fn detached_anchored_closure() -> (AppState, SectionId, SectionId) {
    let state = state_with_project("OpenUndoBugs");
    let a = create_section(&state, "A", vec![straight_shot(0.0, 5.0)]);
    let b = create_section(&state, "B", vec![straight_shot(90.0, 5.0)]);

    for (section, latitude) in [(a, 10.0), (b, 11.0)] {
        let datum = state.execute(Command::SetZeroDatum {
            station_ref: StationRef::new(section, 0),
            coordinates: GpsCoordinates::new(latitude, 20.0, 0.0).unwrap(),
        });
        assert!(datum.success, "SetZeroDatum failed: {:?}", datum.error);
    }

    let closure = state.execute(Command::CreateLoopClosure {
        station_a: StationRef::new(a, 1),
        station_b: StationRef::new(b, 1),
        closure_strategy: ClosureStrategy::default(),
    });
    assert!(
        closure.success,
        "CreateLoopClosure failed: {:?}",
        closure.error
    );
    (state, a, b)
}

/// [`detached_anchored_closure`] with the second anchor removed - the fuzzer's
/// first two moves - so the closure has been converted and the project is
/// converged with what a load would produce. Asserted here so a test that stops
/// reproducing the shape fails instead of passing vacuously.
fn converged_orphan_state() -> AppState {
    let (state, _a, b) = detached_anchored_closure();
    let removed = state.execute(Command::RemoveZeroDatum {
        station_ref: StationRef::new(b, 0),
    });
    assert!(
        removed.success,
        "RemoveZeroDatum failed: {:?}",
        removed.error
    );
    assert_eq!(
        closure_and_merge_counts(&state),
        (0, 1),
        "setup: the datum removal must have converted the closure it made convertible"
    );
    state
}

/// `(loop_closures, station_merges)` - the pair that moved by one in each
/// direction across the failing undo, and across the failing reload.
fn closure_and_merge_counts(state: &AppState) -> (usize, usize) {
    let project = state.get_project_state().expect("no project open");
    (project.loop_closures.len(), project.station_merges.len())
}

/// The stations carrying a zero datum, IN ORDER - the order is what finding 17
/// is about, so a restore that appends instead of re-inserting has to fail here.
fn datum_stations(state: &AppState) -> Vec<StationRef> {
    let project = state.get_project_state().expect("no project open");
    project
        .zero_datums
        .iter()
        .filter_map(|zd| project.resolve_key(&zd.station_ref))
        .collect()
}

/// #557: a PERSISTED undo entry reaches `restore_section` with a file-supplied
/// section id, and `Section::restore_with_identity` -> `Section::new` computes
/// `id * FEATURE_ID_STRIDE` unchecked. Live undo cannot mint an over-bound id
/// (#556), so the guard exists purely for a crafted or corrupted undo table -
/// the entry is refused, never constructed. Not `#[ignore]`d: written with its
/// fix, per the module rule it guards the mechanism from day one.
#[test]
fn restoring_a_section_with_an_over_bound_id_is_refused() {
    let state = state_with_project("Overbound Restore");

    let hostile = crate::models::MAX_SECTION_ID + 1;
    let result = state.restore_section(
        hostile,
        crate::models::SectionData::default(),
        &crate::models::SectionIdentity::default(),
    );
    assert!(!result.success, "an over-bound restore must be refused");
    let err = result.error.unwrap_or_default();
    assert!(
        err.contains(&hostile.to_string())
            && err.contains(&crate::models::MAX_SECTION_ID.to_string()),
        "the refusal must name the id and the bound: {err}"
    );
    // The refusal must leave the project untouched.
    let project = state.get_project_state().unwrap();
    assert!(
        project.sections.is_empty(),
        "no section may be constructed from an over-bound id"
    );
}

/// #557 review finding (all three reviewers, independently): the
/// `ProjectSnapshotPair` replay arm - the one CURRENT builds write - rebuilds
/// sections through serde, never `Section::new`, so neither the open-refusal
/// nor the `restore_section` bound sees an over-bound id arriving through a
/// crafted undo table. `restore_snapshot_into` is the chokepoint both
/// snapshot arms share; it must refuse and leave the live project untouched.
#[test]
fn a_snapshot_restore_with_an_over_bound_section_id_is_refused() {
    let mut hostile = crate::models::Project::new("Hostile".to_string(), None);
    hostile.add_section(crate::models::SectionData::default());
    hostile.sections[0].id = crate::models::MAX_SECTION_ID + 1;
    let json = serde_json::to_string(&hostile).unwrap();

    let mut slot = super::project_slot::ProjectSlot::new(Some(crate::models::Project::new(
        "Live".to_string(),
        None,
    )));
    assert!(
        !AppState::restore_snapshot_into(&mut slot, &json),
        "an over-bound snapshot must be refused"
    );
    let live = slot.into_project().unwrap();
    assert_eq!(live.name, "Live", "the live project must be untouched");
    assert!(
        live.sections.is_empty(),
        "no hostile section may be installed"
    );
}

/// PR #575 Codex finding: `next_shot_id` is serde-carried too, so a snapshot
/// with a LEGAL section id but a hostile counter (u32::MAX here) used to pass
/// the id-only guard - and the next `add_shot` (`next_shot_id += 1`) panics in
/// debug or wraps into another section's id space in release. The window check
/// refuses it; a legitimate serialize -> restore round trip must still pass.
#[test]
fn a_snapshot_restore_with_a_hostile_shot_counter_is_refused() {
    let mut hostile = crate::models::Project::new("Hostile Counter".to_string(), None);
    hostile.add_section(crate::models::SectionData::default());
    assert!(
        hostile.sections[0].id <= crate::models::MAX_SECTION_ID,
        "precondition: the id itself is legal"
    );
    hostile.sections[0].next_shot_id = u32::MAX;
    let json = serde_json::to_string(&hostile).unwrap();

    let mut slot = super::project_slot::ProjectSlot::new(Some(crate::models::Project::new(
        "Live".to_string(),
        None,
    )));
    assert!(
        !AppState::restore_snapshot_into(&mut slot, &json),
        "a hostile shot counter must be refused"
    );
    assert!(
        slot.into_project().unwrap().sections.is_empty(),
        "nothing may be installed"
    );

    // The guard must not over-refuse: a real project with shots serializes and
    // restores cleanly.
    let mut legit = crate::models::Project::new("Legit".to_string(), None);
    legit.add_section(crate::models::SectionData {
        shots: vec![ShotData {
            length: 5.0,
            azimuth_in: 90.0,
            ..Default::default()
        }],
        ..Default::default()
    });
    let json = serde_json::to_string(&legit).unwrap();
    let mut slot = super::project_slot::ProjectSlot::new(Some(crate::models::Project::new(
        "Live".to_string(),
        None,
    )));
    assert!(
        AppState::restore_snapshot_into(&mut slot, &json),
        "a legitimate snapshot must still restore"
    );
    assert_eq!(slot.into_project().unwrap().sections.len(), 1);
}
