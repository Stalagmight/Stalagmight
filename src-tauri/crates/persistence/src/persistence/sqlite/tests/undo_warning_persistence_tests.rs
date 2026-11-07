//! Undo-stack and resolved-warning persistence.
//!
//! Lines 1776-2169 of the former single-file `persistence/sqlite/tests.rs` (#576 AFK-D3), moved
//! verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;

/// [#303] The writer writes every row it is given, renumbering `sequence` 0..n so
/// `ORDER BY sequence` reproduces stack order.
///
/// There used to be an incremental `sync_undo_stack` here, and a test asserting the
/// OPPOSITE of this - that a second call left pre-existing rows in place. It was
/// measuring the helper against a database that persists between calls, which is NOT how
/// saving works: `save_project` builds a fresh `.stalx.tmp` and renames it over the
/// target, so every save writes into a file with no undo tables. The "optimization" that
/// test blessed silently dropped the history whenever nothing had changed since the
/// previous save.
#[test]
fn undo_persist_save_writes_the_rows_it_is_given() {
    let db = Database::in_memory().unwrap();

    let row = |id: u64| {
        (
            format!(r#"{{"type":"Cmd{id}"}}"#),
            format!(r#"{{"n":{id}}}"#),
            Some("2026-07-01T00:00:00+00:00".to_owned()),
        )
    };

    db.save_undo_stack_with_timestamps((0..50).map(row).collect())
        .unwrap();

    let loaded = db.load_undo_stack_with_timestamps().unwrap();
    assert_eq!(loaded.len(), 50);
    assert_eq!(loaded[0].1, r#"{"type":"Cmd0"}"#);
    assert_eq!(loaded[49].1, r#"{"type":"Cmd49"}"#);

    // A second call with one more row rewrites the table and keeps the ordering by
    // `sequence`.
    db.save_undo_stack_with_timestamps((0..51).map(row).collect())
        .unwrap();
    let loaded = db.load_undo_stack_with_timestamps().unwrap();
    assert_eq!(loaded.len(), 51);
    assert_eq!(
        loaded.iter().map(|(seq, ..)| *seq).collect::<Vec<_>>(),
        (0..51).collect::<Vec<_>>()
    );
}

/// [#303] The delete half: a save is a full rewrite, so rows the stack no longer holds
/// leave the table - whether they fell off the FRONT (a byte-budget trim) or the BACK
/// (an undo popping the tail), and all of them when the stack is empty.
#[test]
fn undo_persist_save_leaves_no_stragglers() {
    let db = Database::in_memory().unwrap();
    let row = |id: u64| {
        (
            format!(r#"{{"type":"Cmd{id}"}}"#),
            format!(r#"{{"n":{id}}}"#),
            None,
        )
    };
    db.save_undo_stack_with_timestamps((0..5).map(row).collect())
        .unwrap();

    // Two evicted from the front, one popped off the back: only the middle two are
    // handed over, and the other three must not survive.
    db.save_undo_stack_with_timestamps((2..4).map(row).collect())
        .unwrap();

    let loaded = db.load_undo_stack_with_timestamps().unwrap();
    assert_eq!(
        loaded
            .iter()
            .map(|(_, cmd, ..)| cmd.clone())
            .collect::<Vec<_>>(),
        vec![r#"{"type":"Cmd2"}"#, r#"{"type":"Cmd3"}"#]
    );

    // An empty stack clears the table outright - the shape a Full-mode project's redo
    // save takes on every save (#303 F05).
    db.save_undo_stack_with_timestamps(vec![]).unwrap();
    assert!(db.load_undo_stack_with_timestamps().unwrap().is_empty());
}

#[test]
fn test_undo_stack_persistence() {
    let db = Database::in_memory().unwrap();

    // Save undo entries
    let entries = vec![
        (
            r#"{"type":"AddSection"}"#.to_string(),
            r#"{"section_id":0}"#.to_string(),
        ),
        (
            r#"{"type":"AddShot"}"#.to_string(),
            r#"{"shot_id":1}"#.to_string(),
        ),
        (
            r#"{"type":"UpdateShot"}"#.to_string(),
            r#"{"old_data":{}}"#.to_string(),
        ),
    ];
    db.save_undo_stack(entries).unwrap();

    // Load undo stack
    let loaded = db.load_undo_stack().unwrap();
    assert_eq!(loaded.len(), 3);
    assert_eq!(loaded[0].0, 0); // sequence
    assert_eq!(loaded[0].1, r#"{"type":"AddSection"}"#);
    assert_eq!(loaded[0].2, r#"{"section_id":0}"#);
    assert_eq!(loaded[1].0, 1);
    assert_eq!(loaded[2].0, 2);
}

#[test]
fn test_resolved_warnings_persistence() {
    let db = Database::in_memory().unwrap();

    // Save project first (to have tables)
    let mut project = Project::new("Test".to_string(), None);
    project.add_section(SectionData {
        color: "#000".to_string(),
        ..Default::default()
    });
    let mut db = db;
    db.save_project(&project).unwrap();

    // Save resolved warning
    let warning = ResolvedWarning {
        section_id: 0,
        shot_id: 1,
        warning_code: "AZIMUTH_DIFF".to_string(),
        resolved_at: Utc::now(),
        resolved_comment: Some("Verified in field".to_string()),
        shot_uuid: None,
    };
    db.save_resolved_warning(&warning).unwrap();

    // Load resolved warnings
    let loaded = db.load_resolved_warnings().unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].section_id, 0);
    assert_eq!(loaded[0].shot_id, 1);
    assert_eq!(loaded[0].warning_code, "AZIMUTH_DIFF");
    assert_eq!(
        loaded[0].resolved_comment,
        Some("Verified in field".to_string())
    );

    // Delete resolved warning
    db.delete_resolved_warning(0, 1, "AZIMUTH_DIFF", None)
        .unwrap();
    let loaded_after_delete = db.load_resolved_warnings().unwrap();
    assert!(loaded_after_delete.is_empty());
}

#[test]
fn test_resolved_warning_dedup_and_delete_by_uuid() {
    // Issue #57: suppression is keyed by the shot's stable UUID, not the
    // renumber-prone shot_id. Two saves with the same UUID but different
    // shot_id must collapse to a single row, and deletion by UUID must clear
    // it regardless of the shot_id supplied.
    let mut db = Database::in_memory().unwrap();
    let mut project = Project::new("Test".to_string(), None);
    project.add_section(SectionData {
        color: "#000".to_string(),
        ..Default::default()
    });
    db.save_project(&project).unwrap();

    let base = ResolvedWarning {
        section_id: 0,
        shot_id: 1,
        warning_code: "ZERO_LENGTH".to_string(),
        resolved_at: Utc::now(),
        resolved_comment: None,
        shot_uuid: Some("uuid-1".to_string()),
    };
    db.save_resolved_warning(&base).unwrap();
    // Same UUID, drifted shot_id (as an insert-before would produce on reload).
    db.save_resolved_warning(&ResolvedWarning {
        shot_id: 5,
        ..base.clone()
    })
    .unwrap();

    let loaded = db.load_resolved_warnings().unwrap();
    assert_eq!(loaded.len(), 1, "same UUID must dedup to a single row");
    assert_eq!(loaded[0].shot_uuid.as_deref(), Some("uuid-1"));
    assert_eq!(loaded[0].shot_id, 5, "latest position wins");

    // Deleting by UUID clears it even with an unrelated shot_id.
    db.delete_resolved_warning(0, 999, "ZERO_LENGTH", Some("uuid-1"))
        .unwrap();
    assert!(db.load_resolved_warnings().unwrap().is_empty());
}

#[test]
fn test_resolved_warnings_bulk_tolerates_duplicate_keys() {
    // Issue #57 regression guard: resolve_all can hand the bulk save two
    // warnings with the SAME (section_id, shot_id, warning_code) - e.g. a shot
    // with both azimuths out of range yields two INVALID_AZIMUTH rows, and an
    // over-anchored merge component can map two datums onto one shot. The bulk
    // save must not abort on the legacy UNIQUE constraint.
    let mut db = Database::in_memory().unwrap();
    let mut project = Project::new("Test".to_string(), None);
    project.add_section(SectionData {
        color: "#000".to_string(),
        ..Default::default()
    });
    db.save_project(&project).unwrap();

    let w = ResolvedWarning {
        section_id: 0,
        shot_id: 1,
        warning_code: "INVALID_AZIMUTH".to_string(),
        resolved_at: Utc::now(),
        resolved_comment: None,
        shot_uuid: Some("uuid-1".to_string()),
    };
    // Two identical-key rows in one batch must not raise a UNIQUE error.
    db.save_resolved_warnings_bulk(&[w.clone(), w.clone()])
        .unwrap();
    assert_eq!(
        db.load_resolved_warnings().unwrap().len(),
        1,
        "duplicate (section, shot, code) collapses to one row instead of crashing the batch"
    );
}

#[test]
fn test_resolved_warning_uuid_save_does_not_delete_other_shots_ignore() {
    // Issue #57 (Item 2): a UUID-keyed save must NOT delete a DIFFERENT shot's
    // valid by-UUID row that occupies a different position. Before the fix the
    // DELETE also matched `(section_id, shot_id)`, so saving uuid-Y could wipe
    // uuid-X if Y's params happened to target X's position.
    let mut db = Database::in_memory().unwrap();
    let mut project = Project::new("Test".to_string(), None);
    project.add_section(SectionData {
        color: "#000".to_string(),
        ..Default::default()
    });
    db.save_project(&project).unwrap();

    // Shot X ignored at position shot_id=1.
    db.save_resolved_warning(&ResolvedWarning {
        section_id: 0,
        shot_id: 1,
        warning_code: "ZERO_LENGTH".to_string(),
        resolved_at: Utc::now(),
        resolved_comment: None,
        shot_uuid: Some("uuid-X".to_string()),
    })
    .unwrap();
    // Shot Y ignored at a DISTINCT position shot_id=2 (its own current spot).
    db.save_resolved_warning(&ResolvedWarning {
        section_id: 0,
        shot_id: 2,
        warning_code: "ZERO_LENGTH".to_string(),
        resolved_at: Utc::now(),
        resolved_comment: None,
        shot_uuid: Some("uuid-Y".to_string()),
    })
    .unwrap();

    let loaded = db.load_resolved_warnings().unwrap();
    assert_eq!(loaded.len(), 2, "both distinct-UUID ignores must survive");
    let uuids: std::collections::HashSet<_> = loaded
        .iter()
        .filter_map(|r| r.shot_uuid.as_deref())
        .collect();
    assert!(
        uuids.contains("uuid-X"),
        "uuid-X's ignore must not be deleted"
    );
    assert!(uuids.contains("uuid-Y"), "uuid-Y's ignore must survive");
}

#[test]
fn test_unresolve_with_uuid_clears_same_shots_legacy_position_row_only() {
    // Issue #57 (review): un-ignore passes the LIVE shot's UUID. A legacy
    // NULL-uuid row suppressing that same shot (e.g. on a load path that didn't
    // run the backfill) must still be cleared - the delete's position branch is
    // restricted to `shot_uuid IS NULL`, so it clears the legacy row WITHOUT
    // touching a DIFFERENT shot's by-UUID row that occupies the same position.
    let mut db = Database::in_memory().unwrap();
    let mut project = Project::new("Test".to_string(), None);
    project.add_section(SectionData {
        color: "#000".to_string(),
        ..Default::default()
    });
    db.save_project(&project).unwrap();

    // Legacy NULL-uuid suppression of the shot at position (0, 1).
    db.save_resolved_warning(&ResolvedWarning {
        section_id: 0,
        shot_id: 1,
        warning_code: "ZERO_LENGTH".to_string(),
        resolved_at: Utc::now(),
        resolved_comment: None,
        shot_uuid: None,
    })
    .unwrap();
    // A DIFFERENT shot's by-UUID suppression at a distinct position (0, 2) - must
    // NOT be deleted when we un-ignore the shot at (0, 1). (UNIQUE(section_id,
    // shot_id, warning_code) forbids two rows sharing one position+code, so this
    // models a different shot, not a same-position collision.)
    db.save_resolved_warning(&ResolvedWarning {
        section_id: 0,
        shot_id: 2,
        warning_code: "ZERO_LENGTH".to_string(),
        resolved_at: Utc::now(),
        resolved_comment: None,
        shot_uuid: Some("uuid-other".to_string()),
    })
    .unwrap();

    // Un-ignore the live shot at (0, 1) with ITS uuid: the uuid branch matches
    // nothing (no "uuid-live" row), but the NULL-only position branch clears the
    // legacy row at (0, 1).
    db.delete_resolved_warning(0, 1, "ZERO_LENGTH", Some("uuid-live"))
        .unwrap();

    let rows = db.load_resolved_warnings().unwrap();
    assert!(
        !rows.iter().any(|r| r.shot_uuid.is_none()),
        "the legacy NULL-uuid row for the same shot/position must be cleared by un-ignore"
    );
    assert!(
        rows.iter()
            .any(|r| r.shot_uuid.as_deref() == Some("uuid-other")),
        "a different shot's by-UUID row at another position must NOT be deleted"
    );
}

#[test]
fn test_backfill_resolved_warning_uuids_upgrades_and_is_idempotent() {
    // Issue #57 (Item 4): legacy NULL-uuid resolved rows whose (section_id,
    // shot_id) still maps to a live shot are upgraded to UUID keying on load.
    // Idempotent, and a row with no live shot is left NULL (never mis-keyed).
    let mut db = Database::in_memory().unwrap();
    let mut project = Project::new("Test".to_string(), None);
    project.add_section(SectionData {
        color: "#000".to_string(),
        ..Default::default()
    });
    db.save_project(&project).unwrap();

    // Legacy row keyed by position (no UUID) that maps to a live shot.
    db.save_resolved_warning(&ResolvedWarning {
        section_id: 0,
        shot_id: 1,
        warning_code: "ZERO_LENGTH".to_string(),
        resolved_at: Utc::now(),
        resolved_comment: None,
        shot_uuid: None,
    })
    .unwrap();
    // Legacy row whose (section, shot) is NOT in the live map - must stay NULL.
    db.save_resolved_warning(&ResolvedWarning {
        section_id: 0,
        shot_id: 7,
        warning_code: "SHORT_SHOT".to_string(),
        resolved_at: Utc::now(),
        resolved_comment: None,
        shot_uuid: None,
    })
    .unwrap();

    let mut live: HashMap<(u32, u32), String> = HashMap::new();
    live.insert((0, 1), "uuid-live".to_string());

    let upgraded = db.backfill_resolved_warning_uuids(&live).unwrap();
    assert_eq!(upgraded, 1, "only the row with a live shot is upgraded");

    let loaded = db.load_resolved_warnings().unwrap();
    let mapped = loaded
        .iter()
        .find(|r| r.shot_id == 1 && r.warning_code == "ZERO_LENGTH")
        .expect("the (0,1) row should still exist");
    assert_eq!(
        mapped.shot_uuid.as_deref(),
        Some("uuid-live"),
        "the live-mapped legacy row gets the shot UUID"
    );
    let orphan = loaded
        .iter()
        .find(|r| r.shot_id == 7 && r.warning_code == "SHORT_SHOT")
        .expect("the orphan row should still exist");
    assert_eq!(
        orphan.shot_uuid, None,
        "the unmapped legacy row stays position-keyed"
    );

    // Idempotent: a second backfill upgrades nothing.
    let upgraded_again = db.backfill_resolved_warning_uuids(&live).unwrap();
    assert_eq!(upgraded_again, 0, "second backfill is a no-op");
}
