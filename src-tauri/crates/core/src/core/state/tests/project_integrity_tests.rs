//! Save-time integrity: dangling refs, repair, non-finite sanitizing, bad undo payloads.
//!
//! Lines 1701-2913 of the former single-file `core/state/tests.rs` (#576 E1),
//! moved verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;
use crate::test_fixtures::{shot, state_with_project};

#[test]
fn save_blocks_on_dangling_refs() {
    // A save with unresolvable cross-references is refused outright (no file
    // written), so corruption never reaches disk. The user is told it didn't run
    // and restores integrity via Repair, after which saves resume.
    //
    // This covers autosave too, and there is nothing extra to assert for it: the
    // backend save has no manual-vs-auto distinction. `Command::SaveProject`
    // carries only a path; the frontend's `isAutosave` reaches the backend for
    // the log line alone (`commands::project::save_project`) and never touches
    // the pipeline, so the debounced autosave reaches exactly the code exercised
    // below.
    let temp_dir = TempDir::new().unwrap();
    let state = AppState::new().unwrap();

    let mut project = Project::new("Dangle".to_string(), None);
    let a = project
        .add_section(SectionData {
            name: Some("Main".to_string()),
            shots: vec![shot(1.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    // Dangling station offset: anchors a shot UUID that doesn't exist.
    let dangling = StationKey::new(a, StationAnchor::AfterShot("ghost".to_string()));
    project.station_offsets.insert(dangling, (1.0, 2.0, 3.0));
    assert!(project.has_unresolvable_cross_references());
    state.set_project_for_tests(project);

    let stalx = temp_dir.path().join("dangle.stalx");

    let result = state.execute(Command::SaveProject {
        path: Some(stalx.clone()),
    });
    assert!(!result.success, "save must block on dangling refs");
    assert!(!stalx.exists(), "a blocked save must not write a file");
}

#[test]
fn repair_open_project_integrity_is_undoable_and_redoable() {
    // Repair drops dangling refs / re-cascades offsets; it must record an undo
    // entry so the user can revert it. Undo restores the pre-repair (corrupt)
    // state, redo re-applies the fix - both via the snapshot pair.
    let state = AppState::new().unwrap();

    let mut project = Project::new("Repairable".to_string(), None);
    let a = project
        .add_section(SectionData {
            name: Some("Main".to_string()),
            shots: vec![shot(1.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    // Dangling station offset → unresolvable cross-reference (corrupt project).
    let dangling = StationKey::new(a, StationAnchor::AfterShot("ghost".to_string()));
    project.station_offsets.insert(dangling, (1.0, 2.0, 3.0));
    assert!(project.has_unresolvable_cross_references());
    state.set_project_for_tests(project);

    // Repair fixes it AND records exactly one undo entry.
    let result = state.repair_open_project_integrity().unwrap();
    assert!(result.fixed >= 1, "repair should report at least one fix");
    assert!(
        !state
            .get_project_state()
            .unwrap()
            .has_unresolvable_cross_references(),
        "repair drops the dangling ref"
    );
    assert_eq!(
        state.get_undo_stack().len(),
        1,
        "repair records one undo entry"
    );

    // Undo restores the pre-repair (corrupt) state.
    assert!(state.undo().success, "repair must be undoable");
    assert!(
        state
            .get_project_state()
            .unwrap()
            .has_unresolvable_cross_references(),
        "undo brings the dangling ref back"
    );

    // Redo re-applies the repair.
    assert!(state.redo().success, "repair must be redoable");
    assert!(
        !state
            .get_project_state()
            .unwrap()
            .has_unresolvable_cross_references(),
        "redo re-drops the dangling ref"
    );
}

#[test]
fn redo_batch_preserves_double_anchored_bridge_closure() {
    // A single user Batch can strand a sole-bridge closure: two otherwise
    // unconnected sections joined only by a freshly-created closure. Normally the
    // post-command epilogue converts that closure into a merge (#8 - closures are
    // only for true loops), and execute_batch captures its redo snapshot AFTER that
    // conversion so undo→redo preserves it.
    //
    // BUT here both sections are zero-datum-anchored (the only way to create a
    // closure between two disconnected sections). Converting the bridge to a rigid
    // merge would put two GPS anchors in one merge component and tear it - the
    // reported bug - so the conversion guard now LEAVES it as a closure. This test
    // pins that corrected behaviour AND that the batch snapshot still captures the
    // post-epilogue state across undo/redo: the closure (not a merge) is preserved,
    // and the two datums stay in separate merge components.
    let state = AppState::new().unwrap();

    let mut project = Project::new("Batch".to_string(), None);
    let a = project
        .add_section(SectionData {
            name: Some("A".to_string()),
            shots: vec![shot(1.0), shot(2.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            name: Some("B".to_string()),
            shots: vec![shot(3.0), shot(4.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    state.set_project_for_tests(project);

    // One atomic batch: give each section a datum (so the otherwise-unconnected
    // pair is allowed a closure), then close A1↔B1. With no other path between them
    // the closure is a sole bridge and the epilogue converts it to a merge.
    let result = state.execute(Command::Batch {
        commands: vec![
            Command::SetZeroDatum {
                station_ref: StationRef::new(a, 0),
                coordinates: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
            },
            Command::SetZeroDatum {
                station_ref: StationRef::new(b, 0),
                coordinates: GpsCoordinates::new(46.0, -74.0, 110.0).unwrap(),
            },
            Command::CreateLoopClosure {
                station_a: StationRef::new(a, 1),
                station_b: StationRef::new(b, 1),
                closure_strategy: ClosureStrategy::default(),
            },
        ],
    });
    assert!(result.success, "batch should succeed: {:?}", result.error);

    let after_execute = state.get_project_state().unwrap();
    assert_eq!(
        after_execute.loop_closures.len(),
        1,
        "a sole-bridge closure between two anchored groups must be LEFT as a closure, not merged"
    );
    assert_eq!(
        after_execute.station_merges.len(),
        0,
        "no merge: converting it would double-anchor one merge component"
    );

    // Undo returns to the pre-batch topology (no datums, no closure, no merge).
    assert!(state.undo().success);
    let after_undo = state.get_project_state().unwrap();
    assert_eq!(after_undo.loop_closures.len(), 0);
    assert_eq!(after_undo.station_merges.len(), 0);

    // Redo must restore the post-epilogue topology: the closure is preserved and it
    // is still NOT a merge.
    assert!(state.redo().success);
    let after_redo = state.get_project_state().unwrap();
    assert_eq!(
        after_redo.loop_closures.len(),
        1,
        "redo must preserve the unconverted bridge closure"
    );
    assert_eq!(
        after_redo.station_merges.len(),
        0,
        "redo must not create the over-constraining merge"
    );
}

#[test]
fn update_shot_reconciles_datum_less_merge() {
    // Editing a shot upstream of a merge in a group with NO zero datum used to
    // leave the merge drifted: UpdateShot was misclassified INERT (skipping the
    // reconcile epilogue) and the bespoke realign_after_shot_edit gave up without
    // a datum. Both are fixed - UpdateShot is geometry-affecting so the reconcile
    // runs, and the realign anchors an unanchored group on the edited section
    // (#413 finding 20) - so this heals in the same command by either route.
    let state = AppState::new().unwrap();
    let mut project = Project::new("NoDatum".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(5.0), shot(5.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(5.0)],
            ..Default::default()
        })
        .id;
    // Merge A:2 (after both shots) ↔ B:0, with NO zero datum anywhere.
    project
        .merge_stations(StationRef::new(a, 2), StationRef::new(b, 0))
        .unwrap();
    project.resolve_offsets();
    assert!(project.check_merge_alignment().is_empty(), "starts aligned");
    state.set_project_for_tests(project);

    // Edit shot 0 in A (upstream of the merge station) - a big length change moves
    // A:2 well beyond the 5 cm reporting tolerance.
    let shot0 = state
        .get_project_state()
        .unwrap()
        .get_section(a)
        .unwrap()
        .shots[0]
        .id;
    let updates = ShotUpdates {
        length: Some(9.0),
        ..Default::default()
    };
    let result = state.execute(Command::UpdateShot {
        section_id: a,
        shot_id: shot0,
        updates,
    });
    assert!(result.success, "{:?}", result.error);

    assert!(
        state
            .get_project_state()
            .unwrap()
            .check_merge_alignment()
            .is_empty(),
        "a shot edit must reconcile the datum-less merge, not leave it drifted"
    );
}

#[test]
fn metadata_only_update_shot_does_not_heal_drift() {
    // The converse of update_shot_reconciles_datum_less_merge: a comment/LRUD/
    // reference-only UpdateShot touches no geometry, so it must NOT run the
    // reconcile epilogue. Reconciling would silently re-align an already-drifted
    // project as a side effect of a metadata edit whose undo only restores the
    // metadata - the asymmetric-undo hazard that keeps SetStationComment INERT.
    let state = AppState::new().unwrap();
    let mut project = Project::new("Drift".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(5.0), shot(5.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(5.0)],
            ..Default::default()
        })
        .id;
    project
        .merge_stations(StationRef::new(a, 2), StationRef::new(b, 0))
        .unwrap();
    project.resolve_offsets();
    assert!(project.check_merge_alignment().is_empty(), "starts aligned");

    // Introduce drift WITHOUT reconciling: lengthen a shot upstream of the merge
    // directly in the model, so A:2 moves out of alignment with B:0.
    let shot0 = project.get_section(a).unwrap().shots[0].id;
    project.get_section_mut(a).unwrap().shots[0].length = 9.0;
    assert!(
        !project.check_merge_alignment().is_empty(),
        "drift is present before the metadata edit"
    );
    state.set_project_for_tests(project);

    // A comment-only edit must leave the drift untouched.
    let updates = ShotUpdates {
        comment: Some(Some("note".to_string())),
        ..Default::default()
    };
    let result = state.execute(Command::UpdateShot {
        section_id: a,
        shot_id: shot0,
        updates,
    });
    assert!(result.success, "{:?}", result.error);
    assert!(
        !state
            .get_project_state()
            .unwrap()
            .check_merge_alignment()
            .is_empty(),
        "a metadata-only shot edit must not reconcile (heal) a drifted merge"
    );

    // Sanity-check the gate isn't simply always-off: a geometry edit DOES heal it.
    let updates = ShotUpdates {
        length: Some(7.0),
        ..Default::default()
    };
    assert!(
        state
            .execute(Command::UpdateShot {
                section_id: a,
                shot_id: shot0,
                updates
            })
            .success
    );
    assert!(
        state
            .get_project_state()
            .unwrap()
            .check_merge_alignment()
            .is_empty(),
        "a geometry edit must reconcile the drift"
    );
}

#[test]
fn metadata_only_update_shot_batch_does_not_heal_drift() {
    // execute_batch gates its reconcile epilogue the same way execute() does: a
    // batch of metadata-only UpdateShots (a multi-cell comment paste) must not
    // heal a drifted merge as a side effect.
    let state = AppState::new().unwrap();
    let mut project = Project::new("DriftBatch".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(5.0), shot(5.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(5.0)],
            ..Default::default()
        })
        .id;
    project
        .merge_stations(StationRef::new(a, 2), StationRef::new(b, 0))
        .unwrap();
    project.resolve_offsets();
    assert!(project.check_merge_alignment().is_empty(), "starts aligned");

    let shot0 = project.get_section(a).unwrap().shots[0].id;
    let shot1 = project.get_section(a).unwrap().shots[1].id;
    project.get_section_mut(a).unwrap().shots[0].length = 9.0;
    assert!(
        !project.check_merge_alignment().is_empty(),
        "drift present before the batch"
    );
    state.set_project_for_tests(project);

    // Multi-cell comment paste → one batch of two comment-only UpdateShots.
    let mk = |shot_id| Command::UpdateShot {
        section_id: a,
        shot_id,
        updates: ShotUpdates {
            comment: Some(Some("note".to_string())),
            ..Default::default()
        },
    };
    let result = state.execute(Command::Batch {
        commands: vec![mk(shot0), mk(shot1)],
    });
    assert!(result.success, "{:?}", result.error);
    assert!(
        !state
            .get_project_state()
            .unwrap()
            .check_merge_alignment()
            .is_empty(),
        "a metadata-only batch must not reconcile (heal) a drifted merge"
    );
}

#[test]
fn repair_noop_on_clean_project_reports_unchanged() {
    // A repair that finds nothing to fix must not report `changed` (which the
    // frontend dirties on) or push a spurious undo entry. This depends on
    // resolve_offsets() no longer touch()ing an already-consistent project - if it
    // did, the snapshot's modified_at would differ every time and `changed` would
    // be permanently true.
    let state = AppState::new().unwrap();
    let mut project = Project::new("Clean".to_string(), None);
    project.add_section(SectionData {
        shots: vec![shot(1.0)],
        ..Default::default()
    });
    project.ensure_shot_uuids();
    project.resolve_offsets();
    state.set_project_for_tests(project);

    let result = state.repair_open_project_integrity().unwrap();
    assert_eq!(result.fixed, 0);
    assert!(!result.changed, "a no-op repair must not report changed");
    assert_eq!(
        state.get_undo_stack().len(),
        0,
        "a no-op repair adds no undo entry"
    );
}

#[test]
fn save_remints_duplicate_shot_uuids_before_persisting() {
    // A within-section duplicate shot UUID is Critical (resolve_key snaps
    // AfterShot(uuid) to the FIRST match), but the unresolvable-cross-reference save
    // gate doesn't catch it (the ref still resolves, just to the wrong shot). The
    // save path re-mints the duplicate first so unique UUIDs reach disk - lossless,
    // so it auto-heals rather than blocking.
    let temp_dir = TempDir::new().unwrap();
    let state = AppState::new().unwrap();
    let mut project = Project::new("Dup".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(1.0), shot(2.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    // Force a within-section duplicate.
    let dup = project.get_section(a).unwrap().shots[0]
        .uuid
        .clone()
        .unwrap();
    project.get_section_mut(a).unwrap().shots[1].uuid = Some(dup.clone());
    assert!(
        project.has_critical_integrity_violations(),
        "dup UUID is Critical"
    );
    state.set_project_for_tests(project);

    let result = state.save_project(Some(temp_dir.path().join("dup")));
    assert!(
        result.success,
        "save auto-heals dup UUIDs rather than blocking: {:?}",
        result.error
    );

    // The persisted (and in-memory) project now has unique UUIDs, no Critical.
    let saved = state.get_project_state().unwrap();
    let uuids: Vec<_> = saved
        .get_section(a)
        .unwrap()
        .shots
        .iter()
        .filter_map(|s| s.uuid.clone())
        .collect();
    let mut uniq = uuids.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(
        uuids.len(),
        uniq.len(),
        "duplicate shot UUID re-minted before persist"
    );
    assert!(!saved.has_critical_integrity_violations());
}

/// `project` serialized as a PRE-ANCHOR build wrote it: every cross-reference
/// a positional `{section_id, station_index}` (#576 W52-1). This build writes
/// `{section_id, anchor}`, so a test of the legacy restore has to build the old
/// shape itself; one written with an `Index` anchor is a snapshot this build
/// could have taken. Only `Index` and `SectionStart` anchors have a position
/// without resolving them.
fn pre_anchor_snapshot(project: &Project) -> String {
    fn to_positional(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                if let Some(anchor) = map.remove("anchor") {
                    let index = match &anchor {
                        serde_json::Value::String(start) if start == "SectionStart" => 0,
                        serde_json::Value::Object(index) => index["Index"].as_u64().unwrap(),
                        other => unreachable!("anchor {other} has no position"),
                    };
                    map.insert("station_index".to_string(), index.into());
                }
                map.values_mut().for_each(to_positional);
            }
            serde_json::Value::Array(items) => items.iter_mut().for_each(to_positional),
            _ => {}
        }
    }
    let mut value = serde_json::to_value(project).unwrap();
    to_positional(&mut value);
    let snapshot = value.to_string();
    assert!(
        snapshot.contains("\"station_index\":"),
        "the snapshot holds a positional key: {snapshot}"
    );
    snapshot
}

/// [#576 W52-1] `restore_snapshot_into` tells a pre-anchor snapshot by the
/// positional `"station_index":` key, so no snapshot this build writes may
/// hold one: it would be re-anchored, and a key the snapshot recorded (a
/// comment row past the end, still `Index`) would move onto another shot.
/// Every cross-reference collection is filled, an `Index` key among them, and
/// a comment carries the marker's text.
#[test]
fn a_snapshot_this_build_writes_never_holds_the_positional_key_shape() {
    use crate::models::{GpsCoordinates, LoopClosure, StationMerge, ZeroDatum};
    let mut project = Project::new("CaveA".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(1.0), shot(2.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot(3.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    let key = |section, index| {
        project
            .key_for_ref(StationRef::new(section, index))
            .unwrap()
    };
    let (a1, a2, b0, b1) = (key(a, 1), key(a, 2), key(b, 0), key(b, 1));
    project.station_merges.push(StationMerge {
        station_a: a2,
        station_b: b0,
        merged_at: chrono::Utc::now(),
    });
    project.loop_closures.push(LoopClosure {
        id: 1,
        station_a: a1.clone(),
        station_b: b1.clone(),
        path_length: 1.0,
        misclosure_distance: 0.1,
        error_percentage: 10.0,
        horizontal_misclosure: 0.1,
        vertical_misclosure: 0.0,
        strategy: ClosureStrategy::default(),
        is_aligned: false,
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: b1.clone(),
        gps: GpsCoordinates {
            latitude: 1.0,
            longitude: 2.0,
            depth_reference: 0.0,
        },
    });
    project.station_offsets.insert(a1, (1.0, 2.0, 3.0));
    project
        .station_comments
        .insert(b1, "\"station_index\": 7".to_string());
    project.station_comments.insert(
        StationKey::new(a, StationAnchor::Index(9)),
        "comment past the end".to_string(),
    );
    project.set_section_offset(b, (5.0, 6.0));

    let snapshot = AppState::serialize_project_snapshot(&project).unwrap();
    println!("snapshot: {snapshot}");
    assert!(
        !snapshot.contains("\"station_index\":"),
        "a snapshot this build writes holds the pre-anchor key shape"
    );
}

#[test]
fn restore_snapshot_canonicalizes_legacy_index_anchors() {
    // A snapshot persisted by a PRE-ANCHOR build can carry positional Index
    // anchors (undo/redo history is persisted and reloaded across builds, and the
    // back-compat StationKey deserializer accepts the legacy shape). Restoring it
    // must canonicalize to AfterShot - otherwise a later shot insert/delete won't
    // shift the ref and the merge/datum/comment silently points at the wrong station.
    let mut project = Project::new("Legacy".to_string(), None);
    let s = project
        .add_section(SectionData {
            shots: vec![shot(1.0), shot(2.0), shot(3.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    // A station offset and a comment stored in the legacy positional form.
    project
        .station_offsets
        .insert(StationKey::new(s, StationAnchor::Index(2)), (1.0, 2.0, 3.0));
    project.station_comments.insert(
        StationKey::new(s, StationAnchor::Index(2)),
        "comment on station 2".to_string(),
    );
    let snapshot = pre_anchor_snapshot(&project);

    let mut slot = project_slot::ProjectSlot::default();
    assert!(AppState::restore_snapshot_into(&mut slot, &snapshot));
    let restored = slot.into_project().unwrap();

    // The re-anchor keeps the comment, on the station it named.
    let station_2 = restored.key_for_ref(StationRef::new(s, 2)).unwrap();
    assert_eq!(
        restored
            .station_comments
            .get(&station_2)
            .map(String::as_str),
        Some("comment on station 2"),
        "the canonicalized comment stays on station 2: {:?}",
        restored.station_comments
    );
    // No Index anchor survives into the live project after restore.
    let mut saw_index = false;
    restored.for_each_station_ref(|_label, key| {
        if matches!(key.anchor, StationAnchor::Index(_)) {
            saw_index = true;
        }
    });
    assert!(
        !saw_index,
        "restore_snapshot_into must canonicalize legacy Index anchors"
    );
}

#[test]
fn repair_serializes_under_command_lock() {
    // Repair must hold command_lock for its whole duration so a concurrent command
    // can't slip a mutation between repair's before-snapshot and its own mutation
    // (which would make undo-of-repair drop that command). Verify by holding the
    // lock and asserting repair blocks until it's released.
    use std::sync::mpsc;
    use std::time::Duration;
    let state = Arc::new(AppState::new().unwrap());
    let mut project = Project::new("LockRepair".to_string(), None);
    project.add_section(SectionData {
        shots: vec![shot(1.0)],
        ..Default::default()
    });
    project.ensure_shot_uuids();
    state.set_project_for_tests(project);

    let guard = state.command_lock.lock();
    let state2 = Arc::clone(&state);
    let (tx, rx) = mpsc::channel();
    let handle = std::thread::spawn(move || {
        let _ = tx.send(state2.repair_open_project_integrity().unwrap());
    });
    // While we hold command_lock, repair must block (no result arrives).
    assert!(
        rx.recv_timeout(Duration::from_millis(150)).is_err(),
        "repair ran without acquiring command_lock"
    );
    drop(guard);
    // Once released, repair completes.
    let result = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("repair completes after command_lock is released");
    handle.join().unwrap();
    assert_eq!(result.fixed, 0);
}

#[test]
fn save_drops_orphan_section_offset() {
    // A section_offsets entry for a section that no longer exists has a dangling
    // SQLite FK; persisting it aborts the WHOLE save with a generic error. It must
    // be dropped on save instead (the cross-reference gate doesn't see section_offsets).
    let temp_dir = TempDir::new().unwrap();
    let state = AppState::new().unwrap();
    let mut project = Project::new("Orphan".to_string(), None);
    project.add_section(SectionData {
        shots: vec![shot(1.0)],
        ..Default::default()
    });
    project.ensure_shot_uuids();
    project.set_raw_section_offset(99999, (5.0, 6.0)); // no such section
    state.set_project_for_tests(project);

    let result = state.save_project(Some(temp_dir.path().join("orphan")));
    assert!(
        result.success,
        "orphan section_offset must not abort the save: {:?}",
        result.error
    );
    let saved = state.get_project_state().unwrap();
    assert!(
        saved
            .section_offsets()
            .keys()
            .all(|sid| saved.get_section(*sid).is_some()),
        "orphan section_offsets must be dropped on save"
    );
}

/// [#469] A `.stalx` written before the dust gate widened can carry a
/// `section_offsets` value that was REAL to the old 1e-12 gate and is dust to
/// `Project::OFFSET_DUST_M` (1e-6). Opening drops the key and moves that section
/// by the value it held - the deliberate on-disk consequence of widening the
/// gate, pinned here rather than left to be discovered.
///
/// Three claims, because each is a separate way this could go wrong: the key is
/// GONE after the open (`normalize_section_offsets` at the load boundary, see
/// `Project::redrive_offsets_after_load`), the geometry moved by less than a
/// micrometre rather than by anything a survey could notice, and a save/reopen
/// CONVERGES - the row does not come back on the next round trip.
#[test]
fn opening_drops_a_legacy_sub_gate_section_offset() {
    // 5e-7 m, half the gate: the old bound preserved it, the current writer
    // cannot produce it (`Project::write_section_offset` removes the key
    // instead), so only a legacy FILE can hold one.
    const LEGACY_OFFSET: (f64, f64) = (5e-7, -5e-7);

    crate::test_helpers::with_isolated_config(|| {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("legacy-dust.stalx");

        let section_id = {
            let mut project = Project::new("LegacyDust".to_string(), None);
            let section_id = project
                .add_section(SectionData {
                    shots: vec![shot(10.0)],
                    ..Default::default()
                })
                .id;
            project.ensure_shot_uuids();
            // Straight into the map: routing through the writer would normalize it
            // away, which is the point - this state is unrepresentable in memory.
            project.set_raw_section_offset(section_id, LEGACY_OFFSET);

            // The drop is `normalize_section_offsets`' doing. Asked directly, it
            // claims exactly this one row.
            let mut probe = project.clone();
            assert_eq!(
                probe.normalize_section_offsets(),
                1,
                "the sub-gate row is what the load-boundary normalize drops"
            );

            let mut db = Database::open(&path).unwrap();
            db.save_project(&project).unwrap();
            section_id
        };

        // Precondition: the value really is on disk. The save does not normalize -
        // if it ever starts to, this test is measuring nothing.
        assert_eq!(
            Database::open_existing(&path)
                .unwrap()
                .load_project()
                .unwrap()
                .section_offsets()
                .get(&section_id)
                .copied(),
            Some(LEGACY_OFFSET),
            "precondition: the legacy file carries the sub-gate offset"
        );

        let state = AppState::new().unwrap();
        assert!(
            state.open_project(path.clone()).success,
            "open must succeed"
        );
        let opened = state.get_project_state().unwrap();
        assert!(
            !opened.section_offsets().contains_key(&section_id),
            "the load boundary must drop a sub-gate offset: {:?}",
            opened.section_offsets().get(&section_id)
        );

        // ...and the geometry it moved by is the value that went: under a
        // micrometre, four decades under anything the app displays.
        let station = StationRef::new(section_id, 0);
        let moved = opened.compute_station_raw_coords(station).unwrap();
        let (dx, dy) = (moved.x - LEGACY_OFFSET.0, moved.y - LEGACY_OFFSET.1);
        assert!(
            dx.abs() < Project::OFFSET_DUST_M && dy.abs() < Project::OFFSET_DUST_M,
            "dropping the key moved station 0 by ({dx}, {dy}), which is not sub-gate"
        );

        // Convergence: saving what the open produced and reopening it leaves the
        // key absent. A boundary that dropped and a writer that re-minted would
        // make every round trip a fresh edit.
        assert!(state.save_project(Some(path.clone())).success);
        let reopened_state = AppState::new().unwrap();
        assert!(reopened_state.open_project(path).success);
        let reopened = reopened_state.get_project_state().unwrap();
        assert!(
            !reopened.section_offsets().contains_key(&section_id),
            "a sub-gate offset must not resurrect across save/reopen: {:?}",
            reopened.section_offsets().get(&section_id)
        );
        let again = reopened.compute_station_raw_coords(station).unwrap();
        assert_eq!(
            (again.x.to_bits(), again.y.to_bits()),
            (moved.x.to_bits(), moved.y.to_bits()),
            "the second round trip must not move the geometry again - bit-exact, \
             because a converged round trip has nothing left to round off"
        );
    });
}

#[test]
fn save_sanitizes_non_finite_geometry() {
    // A non-finite shot value would abort the save (NaN → NULL in a NOT NULL
    // column) or persist garbage (inf); it must be sanitized on save instead.
    let temp_dir = TempDir::new().unwrap();
    let state = AppState::new().unwrap();
    let mut project = Project::new("NonFinite".to_string(), None);
    project.add_section(SectionData {
        shots: vec![shot(1.0)],
        ..Default::default()
    });
    project.ensure_shot_uuids();
    project.sections[0].shots[0].length = f64::NAN;
    assert!(!project.sections[0].shots[0].all_finite());
    state.set_project_for_tests(project);

    let result = state.save_project(Some(temp_dir.path().join("nonfinite")));
    assert!(
        result.success,
        "non-finite geometry must be sanitized, not abort the save: {:?}",
        result.error
    );
    let saved = state.get_project_state().unwrap();
    assert!(
        saved.sections[0].shots[0].all_finite(),
        "non-finite geometry sanitized on save"
    );
}

#[test]
fn save_sanitizes_non_finite_section_offset() {
    // section_offsets is the one geometry field with no finite guard. A non-finite
    // offset (corrupt/hand-edited DB, or an inf round-trip) would abort the save
    // (NaN → NULL) or persist and poison the component's world coords. It must be
    // sanitized on save, mirroring the shot sanitize and the load-boundary guard.
    //
    // And sanitized THE WAY the map spells (0, 0): by dropping the key. A fully
    // non-finite offset sanitizes to no displacement at all, and this map says
    // that with an absent key - writing a zero row instead persisted an inert
    // entry no other route to the same project has (#413 finding 2).
    let temp_dir = TempDir::new().unwrap();
    let state = AppState::new().unwrap();
    let mut project = Project::new("NonFiniteOffset".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(1.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    project.set_raw_section_offset(a, (f64::NAN, f64::INFINITY));
    state.set_project_for_tests(project);

    let result = state.save_project(Some(temp_dir.path().join("nf_offset")));
    assert!(
        result.success,
        "a non-finite section offset must be sanitized, not abort the save: {:?}",
        result.error
    );
    let saved = state.get_project_state().unwrap();
    let off = saved.section_offset(a);
    assert!(
        off.0.is_finite() && off.1.is_finite(),
        "section offset sanitized on save"
    );
    assert!(
        !saved.section_offsets().contains_key(&a),
        "a wholly non-finite offset sanitizes to no displacement, so the KEY must go - \
         a (0, 0) row is an inert entry the next save serializes again: {:?}",
        saved.section_offsets().get(&a)
    );

    // A half-non-finite offset keeps the component that IS a position: only the
    // poisoned axis is neutralized, so the key stays with the finite value.
    let mut project = state.get_project_state().unwrap();
    project.set_raw_section_offset(a, (7.5, f64::NAN));
    state.set_project_for_tests(project);
    let result = state.save_project(Some(temp_dir.path().join("nf_offset_half")));
    assert!(result.success, "half-non-finite: {:?}", result.error);
    let saved = state.get_project_state().unwrap();
    assert_eq!(
        saved.section_offsets().get(&a).copied(),
        Some((7.5, 0.0)),
        "only the non-finite component is neutralized; the section is still displaced"
    );
}

#[test]
fn blocked_save_does_not_mutate_non_finite_geometry() {
    // A save refused for a dangling cross-reference must NOT first run the
    // destructive non-finite sanitize on the live project - that mutation has no
    // undo entry, so a refused save would silently lose the user's shot values.
    let temp_dir = TempDir::new().unwrap();
    let state = AppState::new().unwrap();
    let mut project = Project::new("BlockedNoMutate".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(1.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    // A dangling station offset → unresolvable cross-reference (save-blocking).
    project.station_offsets.insert(
        StationKey::new(a, StationAnchor::AfterShot("ghost".to_string())),
        (1.0, 2.0, 3.0),
    );
    // ...and a non-finite shot the (now later) sanitize would otherwise destroy.
    project.sections[0].shots[0].length = f64::NAN;
    state.set_project_for_tests(project);

    let result = state.save_project(Some(temp_dir.path().join("blocked")));
    assert!(
        !result.success,
        "save must be blocked by the dangling cross-reference"
    );
    // The gate runs BEFORE the destructive sanitize, so the live shot is untouched.
    let unsaved = state.get_project_state().unwrap();
    assert!(
        !unsaved.sections[0].shots[0].all_finite(),
        "a refused save must not destructively sanitize the live project"
    );
}

#[test]
fn undo_of_duplicate_uuid_repair_restores_the_duplicates() {
    // The pre-repair state of a duplicate_shot_uuid repair is a CURRENT snapshot
    // that intentionally holds duplicate UUIDs. Undo must restore it EXACTLY -
    // restore_snapshot_into must not re-dedup a non-legacy (no Index anchor)
    // snapshot, or the repair can't be reverted.
    let state = AppState::new().unwrap();
    let mut project = Project::new("DupRepairUndo".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(1.0), shot(2.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    // A within-section duplicate UUID: a current corrupt state (AfterShot anchors).
    let dup = project.get_section(a).unwrap().shots[0]
        .uuid
        .clone()
        .unwrap();
    project.get_section_mut(a).unwrap().shots[1].uuid = Some(dup.clone());
    assert!(project.has_critical_integrity_violations());
    state.set_project_for_tests(project);

    // Repair re-mints the duplicate (now unique) and records the dup state as undo `before`.
    let r = state.repair_open_project_integrity().unwrap();
    assert!(r.changed, "repair changed the project");
    assert!(
        !state
            .get_project_state()
            .unwrap()
            .has_critical_integrity_violations(),
        "repair deduped the UUIDs"
    );

    // Undo must restore the duplicate-UUID state EXACTLY (and its warning).
    assert!(state.undo().success);
    let after_undo = state.get_project_state().unwrap();
    let uuids: Vec<_> = after_undo
        .get_section(a)
        .unwrap()
        .shots
        .iter()
        .filter_map(|s| s.uuid.clone())
        .collect();
    assert_eq!(
        uuids[0], uuids[1],
        "undo restored the duplicate UUID exactly"
    );
    assert!(
        after_undo.has_critical_integrity_violations(),
        "the duplicate_shot_uuid warning is back after undo"
    );
}

#[test]
fn restore_snapshot_mints_null_uuids_before_reanchor() {
    // A legacy (pre-v6) snapshot can carry shots with uuid:null AND positional Index
    // anchors. restore must MINT the missing UUIDs before re-anchoring -
    // ensure_unique only dedups EXISTING UUIDs - or key_for_ref can't canonicalize
    // the Index anchor and it stays live (the gap S4 closes in the P1 fix).
    let mut project = Project::new("LegacyNullUuid".to_string(), None);
    let s = project
        .add_section(SectionData {
            shots: vec![shot(1.0), shot(2.0), shot(3.0)],
            ..Default::default()
        })
        .id;
    // Force legacy null UUIDs (pre-v6 files load with NULL uuid columns).
    for shot in &mut project.get_section_mut(s).unwrap().shots {
        shot.uuid = None;
    }
    // A station offset stored in the legacy positional (Index) form.
    project
        .station_offsets
        .insert(StationKey::new(s, StationAnchor::Index(2)), (1.0, 2.0, 3.0));
    let snapshot = pre_anchor_snapshot(&project);

    let mut slot = project_slot::ProjectSlot::default();
    assert!(AppState::restore_snapshot_into(&mut slot, &snapshot));
    let restored = slot.into_project().unwrap();

    // No Index anchor survives: the null UUIDs were minted, so reanchor could
    // canonicalize Index → AfterShot.
    let mut saw_index = false;
    restored.for_each_station_ref(|_label, key| {
        if matches!(key.anchor, StationAnchor::Index(_)) {
            saw_index = true;
        }
    });
    assert!(
        !saw_index,
        "restore must mint null UUIDs so Index anchors canonicalize"
    );
}

#[test]
fn finalize_structural_edit_ignores_pre_existing_critical() {
    // A project opened ALREADY critical (e.g. a dangling cross-reference) must not
    // have an otherwise-clean import rolled back: finalize signals a
    // rollback only for NEWLY introduced corruption, matching execute()'s guard.
    let state = AppState::new().unwrap();
    let mut project = Project::new("AlreadyCritical".to_string(), None);
    project.add_section(SectionData {
        shots: vec![shot(1.0)],
        ..Default::default()
    });
    project.ensure_shot_uuids();
    // Inject a non-finite shot value → non_finite_geometry (a Critical violation).
    project.sections[0].shots[0].length = f64::NAN;
    assert!(project.has_critical_integrity_violations());
    state.set_project_for_tests(project);

    // pre_had_critical = true: do NOT signal rollback (the corruption pre-existed).
    assert!(
        !state.finalize_structural_edit(true),
        "must not block an otherwise-clean edit on an already-critical project"
    );
    // pre_had_critical = false (clean before): DOES signal rollback (corruption present).
    assert!(
        state.finalize_structural_edit(false),
        "must block when this edit introduced the critical violation"
    );
}

#[test]
fn redo_legacy_prepend_createshot_reanchors_section_start() {
    // A CreateShot undo entry persisted by a PRE-anchor build has after_shot_id /
    // before_shot_id == None even for a prepend, so redo used to hit a raw insert
    // that skipped the SectionStart re-anchor - leaving a station-0 datum pointing at
    // the newly-prepended station instead of the original first station. Redo now
    // routes a legacy (anchor-less) entry through the insert helper by position.
    let state = AppState::new().unwrap();
    let mut project = Project::new("LegacyRedo".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot(1.0), shot(2.0)],
            ..Default::default()
        })
        .id;
    project.ensure_shot_uuids();
    // Zero datum on station 0 → stored as a SectionStart anchor.
    project
        .set_zero_datum(
            StationRef::new(a, 0),
            GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
        )
        .unwrap();
    state.set_project_for_tests(project);

    // A LEGACY redo entry: prepend (position 0) with NO anchor fields, as a
    // pre-anchor build would have persisted it.
    let entry = UndoEntry {
        replayed_at: None,
        command_bytes: None,
        command: Command::CreateShot {
            section_id: a,
            shot: shot(5.0),
            after_shot_id: None,
            before_shot_id: None,
        },
        undo_data: UndoData::CreatedShotSnapshot {
            section_id: a,
            shot_id: 999,
            shot_data: shot(5.0),
            position: 0,
            station_shift_from: 0,
            after_shot_id: None,
            before_shot_id: None,
            after_shot_uuid: None,
            before_shot_uuid: None,
            uuid: Some("prep-uuid".to_string()),
            previous_offsets: HashMap::new(),
            legacy_comment_rows: Vec::new(),
        },
        timestamp: None,
    };
    state.undo_stack.lock().push_redo(entry);
    assert!(state.redo().success, "legacy prepend redo");

    let restored = state.get_project_state().unwrap();
    assert_eq!(
        restored.get_section(a).unwrap().shots.len(),
        3,
        "prepend re-applied"
    );
    assert_eq!(
        restored.get_section(a).unwrap().shots[0].uuid.as_deref(),
        Some("prep-uuid"),
    );
    // The station-0 datum must still resolve to the ORIGINAL first station (now
    // index 1), not the freshly-prepended station (index 0).
    let datum_key = restored.zero_datums[0].station_ref.clone();
    let resolved = restored
        .resolve_key(&datum_key)
        .expect("datum still resolves");
    assert_eq!(
        resolved.station_index, 1,
        "SectionStart datum re-anchored to the original first station after a legacy prepend redo"
    );
}

#[test]
fn test_open_project_fails_on_invalid_file() {
    let temp_dir = TempDir::new().unwrap();
    let invalid_path = temp_dir.path().join("invalid.stalx");
    std::fs::write(&invalid_path, b"not a database").unwrap();

    let state = AppState::new().unwrap();
    let result = state.open_project(invalid_path);
    assert!(!result.success);
    let err = result.error.unwrap_or_default();
    let err_lower = err.to_lowercase();
    assert!(
        err_lower.contains("database") || err_lower.contains("corrupt"),
        "Expected database/corruption error, got: {err}"
    );
}

#[test]
fn test_save_project_persists_display_settings_cache() {
    let temp_dir = TempDir::new().unwrap();
    let state = state_with_project("Cache Save");

    state.save_display_setting("showStations", "false").unwrap();

    let save_path = temp_dir.path().join("cache-save");
    let result = state.save_project(Some(save_path.clone()));
    assert!(result.success);

    let saved_path = save_path.with_extension("stalx");
    let db = Database::open_readonly(&saved_path).unwrap();
    let settings = db.load_display_settings().unwrap();
    assert_eq!(settings.get("showStations"), Some(&"false".to_string()));

    let cache = state.display_settings_cache.lock();
    assert!(cache.is_empty());
}

#[test]
fn test_undo_fails_with_invalid_previous_values_shot() {
    let state = AppState::new().unwrap();
    let mut stack = state.undo_stack.lock();
    stack.push_with_data(
        Command::UpdateShot {
            section_id: 1,
            shot_id: 1,
            updates: ShotUpdates::default(),
        },
        UndoData::PreviousValues {
            section_id: 1,
            shot_id: Some(1),
            shot_uuid: None,
            values_json: "not-json".to_string(),
            previous_offsets: HashMap::new(),
        },
    );
    drop(stack);

    let result = state.undo();
    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some("Failed to parse undo data"));
}

#[test]
fn test_undo_fails_with_invalid_previous_values_section() {
    let state = AppState::new().unwrap();
    let mut stack = state.undo_stack.lock();
    stack.push_with_data(
        Command::UpdateSectionMetadata {
            section_id: 1,
            updates: SectionUpdates::default(),
        },
        UndoData::PreviousValues {
            section_id: 1,
            shot_id: None,
            shot_uuid: None,
            values_json: "not-json".to_string(),
            previous_offsets: HashMap::new(),
        },
    );
    drop(stack);

    let result = state.undo();
    assert!(!result.success);
    assert_eq!(result.error.as_deref(), Some("Failed to parse undo data"));
}

#[test]
fn test_undo_fails_with_invalid_project_snapshot() {
    let state = AppState::new().unwrap();
    let mut stack = state.undo_stack.lock();
    stack.push_with_data(
        Command::Batch {
            commands: Vec::new(),
        },
        UndoData::ProjectSnapshot("not-json".to_string()),
    );
    drop(stack);

    let result = state.undo();
    assert!(!result.success);
    assert_eq!(
        result.error.as_deref(),
        Some("Failed to parse project snapshot")
    );
}

#[test]
fn test_redo_fails_with_invalid_project_snapshot_pair() {
    let state = AppState::new().unwrap();
    let mut stack = state.undo_stack.lock();
    let entry = UndoEntry {
        replayed_at: None,
        command_bytes: None,
        command: Command::CloseProject,
        undo_data: UndoData::ProjectSnapshotPair {
            before: "{}".to_string(),
            after: "not-json".to_string(),
        },
        timestamp: None,
    };
    stack.push_redo(entry);
    drop(stack);

    let result = state.redo();
    assert!(!result.success);
    assert_eq!(
        result.error.as_deref(),
        Some("Failed to parse project snapshot")
    );
}

#[test]
fn test_redo_fails_for_command_not_allowed_in_undo_context() {
    let state = AppState::new().unwrap();
    let mut stack = state.undo_stack.lock();
    let entry = UndoEntry {
        replayed_at: None,
        command_bytes: None,
        command: Command::MergeProjects {
            source_paths: Vec::new(),
            output_path: PathBuf::from("out.stalx"),
            output_name: "Merged".to_string(),
            archive_originals: false,
        },
        undo_data: UndoData::None,
        timestamp: None,
    };
    stack.push_redo(entry);
    drop(stack);

    let result = state.redo();
    assert!(!result.success);
    assert_eq!(
        result.error.as_deref(),
        Some("Command cannot be executed in undo context")
    );
}

#[test]
fn test_create_project_without_location() {
    let state = AppState::new().unwrap();

    let result = state.execute(Command::CreateProject {
        name: "Unnamed Cave".to_string(),
        location: None,
    });

    assert!(result.success);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.name, "Unnamed Cave");
    assert!(project.location.is_none());
}

#[test]
fn test_close_project() {
    // Create a project first
    let state = state_with_project("To Close");

    // Close it
    let result = state.execute(Command::CloseProject);
    assert!(result.success);

    // Project should be None
    assert!(state.get_project_state().is_none());
}
