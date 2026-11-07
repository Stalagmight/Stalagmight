//! Project validation and surface-elevation rows, and resolved warnings keyed by shot uuid.
//!
//! Lines 556-871 of the former single-file `persistence/sqlite/tests.rs` (#576 AFK-D3), moved
//! verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;

#[test]
fn load_project_validation_returns_none_when_table_absent() {
    // A legacy .stalx predating `project_settings`, opened read-only (which does
    // NOT migrate), must not error: load_project_validation returns Ok(None) so
    // get_survey_geometry_for_file's preview load still works (Codex). Simulate the
    // missing table by dropping it, then confirm no error is propagated.
    let db = Database::in_memory().unwrap();
    db.conn
        .execute("DROP TABLE IF EXISTS project_settings", [])
        .unwrap();
    let result = db.load_project_validation();
    assert!(
        result.is_ok(),
        "a missing project_settings table must not error"
    );
    assert!(
        result.unwrap().is_none(),
        "absent validation resolves to None"
    );
}

/// #431 persistence: `Some` and `None` are distinct states, not `Some(0.0)`
/// vs. an absent row read the same way.
#[test]
fn surface_elevation_some_and_none_round_trip_distinctly() {
    let temp = TempDir::new().unwrap();

    let with_elevation_path = temp.path().join("with_elevation.stalx");
    let mut db = Database::open(&with_elevation_path).unwrap();
    let mut project = Project::new("Has Elevation".to_string(), None);
    project.surface_elevation = Some(123.5);
    db.save_project(&project).unwrap();
    let loaded = db.load_project().unwrap();
    assert_eq!(loaded.surface_elevation, Some(123.5));

    let without_elevation_path = temp.path().join("without_elevation.stalx");
    let mut db2 = Database::open(&without_elevation_path).unwrap();
    let project = Project::new("No Elevation".to_string(), None);
    assert_eq!(project.surface_elevation, None, "precondition");
    db2.save_project(&project).unwrap();
    let loaded = db2.load_project().unwrap();
    assert_eq!(loaded.surface_elevation, None);
}

/// #431: a corrupted or hand-edited `.stalx` (untrusted, same as a `.tml`)
/// can carry a `surface_elevation` value Rust's `f64::parse` accepts but the
/// rest of the pipeline cannot: `"NaN"` defeats the no-clobber comparison
/// (every NaN compare is false) and would land verbatim in a TML export; a
/// plain non-numeric string is the ordinary "unparseable" case. Both must
/// read as `None`, not propagate a bad value or error the load. Red before
/// the `.filter(f64::is_finite)` guard on `load_project_surface_elevation`.
#[test]
fn stored_non_finite_or_unparseable_elevation_reads_as_none() {
    for raw in ["NaN", "not-a-number"] {
        let db = Database::in_memory().unwrap();
        db.conn
            .execute(
                "INSERT INTO project_settings (key, value) VALUES ('surface_elevation', ?1)",
                [raw],
            )
            .unwrap();
        let result = db.load_project_surface_elevation();
        assert!(result.is_ok(), "{raw:?} must not error the load");
        assert_eq!(
            result.unwrap(),
            None,
            "{raw:?} must read as unknown, not a value"
        );
    }
}

/// #431, the case that proves no migration is needed: a `.stalx` written
/// before this feature existed has no `surface_elevation` row (simulated by
/// deleting it after an ordinary save). Opening it must read `None`, not
/// error, and a subsequent save must succeed.
#[test]
fn a_pre_431_project_opens_with_unknown_elevation_and_saves_cleanly() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("legacy.stalx");
    let mut db = Database::open(&path).unwrap();
    db.save_project(&Project::new("Legacy".to_string(), None))
        .unwrap();
    db.conn
        .execute(
            "DELETE FROM project_settings WHERE key = 'surface_elevation'",
            [],
        )
        .unwrap();

    let loaded = db.load_project().unwrap();
    assert_eq!(
        loaded.surface_elevation, None,
        "a pre-#431 project must read as unknown"
    );

    db.save_project(&loaded).unwrap();
    let reloaded = db.load_project().unwrap();
    assert_eq!(reloaded.surface_elevation, None);
}

/// The genuine "table doesn't exist at all" shape the doc comment on
/// `load_project_surface_elevation` (read.rs) claims to cover - stronger
/// than the DELETE above, which only proves the missing-KEY case (and the
/// writer already deletes the key for `None`, so that DELETE can even be a
/// no-op). Mirrors `load_project_validation_returns_none_when_table_absent`.
#[test]
fn load_project_surface_elevation_returns_none_when_table_absent() {
    let db = Database::in_memory().unwrap();
    db.conn
        .execute("DROP TABLE IF EXISTS project_settings", [])
        .unwrap();
    let result = db.load_project_surface_elevation();
    assert!(
        result.is_ok(),
        "a missing project_settings table must not error"
    );
    assert_eq!(
        result.unwrap(),
        None,
        "absent surface_elevation resolves to None"
    );
}

/// #431: `project_settings` is a shared key/value table, so writing
/// `validation` and `surface_elevation` in the same save must not let either
/// key clobber the other.
#[test]
fn validation_and_surface_elevation_survive_the_same_save() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("both_settings.stalx");
    let mut db = Database::open(&path).unwrap();

    let mut project = Project::new("Both Settings".to_string(), None);
    project.validation.warn_shot_length_min = 99.0;
    project.surface_elevation = Some(-12.25);
    db.save_project(&project).unwrap();

    let loaded = db.load_project().unwrap();
    assert_eq!(loaded.validation.warn_shot_length_min, 99.0);
    assert_eq!(loaded.surface_elevation, Some(-12.25));
}

#[test]
fn save_resolved_warning_never_clobbers_a_different_shots_uuid_row() {
    // The legacy positional UNIQUE must not let one shot's ignore delete another
    // shot's UUID-keyed ignore sharing the (section, shot_id) slot - which can
    // happen transiently when in-memory ids diverge from saved positions after a
    // save (issue #57 / Codex). The conditional upsert preserves the existing row
    // and reports the collision rather than silently dropping either ignore.
    let db = Database::in_memory().unwrap();
    db.conn
        .execute(
            "INSERT INTO sections (id, color, created_at) VALUES (0, '#abcdef', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
    let mk = |uuid: &str| ResolvedWarning {
        section_id: 0,
        shot_id: 5,
        warning_code: "SHORT_SHOT".to_string(),
        resolved_at: Utc::now(),
        resolved_comment: None,
        shot_uuid: Some(uuid.to_string()),
    };
    db.save_resolved_warning(&mk("aaa")).unwrap();
    // A second shot's ignore landing on the same stale position is refused, not silent.
    assert!(
        db.save_resolved_warning(&mk("bbb")).is_err(),
        "a colliding different-shot ignore must be reported, not silently applied"
    );
    let rows = db.load_resolved_warnings().unwrap();
    assert_eq!(rows.len(), 1, "the first shot's ignore must be preserved");
    assert_eq!(rows[0].shot_uuid.as_deref(), Some("aaa"));
}

#[test]
fn bulk_upsert_keeps_uuid_row_over_colliding_legacy_null_row() {
    // On the save carry-over a legacy NULL-uuid row can collide with a remapped
    // UUID row at the same position; the UUID row must win regardless of insert
    // order, never downgrading the survivor to position-keying (issue #57 / Codex).
    let uuid_row = ResolvedWarning {
        section_id: 0,
        shot_id: 7,
        warning_code: "SHORT_SHOT".to_string(),
        resolved_at: Utc::now(),
        resolved_comment: None,
        shot_uuid: Some("ccc".to_string()),
    };
    let null_row = ResolvedWarning {
        shot_uuid: None,
        ..uuid_row.clone()
    };
    for batch in [
        vec![null_row.clone(), uuid_row.clone()],
        vec![uuid_row.clone(), null_row.clone()],
    ] {
        let db = Database::in_memory().unwrap();
        db.conn
            .execute(
                "INSERT INTO sections (id, color, created_at) VALUES (0, '#abcdef', '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        db.save_resolved_warnings_bulk(&batch).unwrap();
        let rows = db.load_resolved_warnings().unwrap();
        assert_eq!(rows.len(), 1, "the colliding rows collapse to one");
        assert_eq!(
            rows[0].shot_uuid.as_deref(),
            Some("ccc"),
            "the UUID row wins"
        );
    }
}

#[test]
fn shot_uuid_is_persisted_reflects_the_shots_table() {
    // A new ignore is UUID-keyed only when the shot's UUID is already on disk; a
    // pre-v6 shot's in-memory-minted UUID isn't, so its ignore stays position-keyed
    // to survive the next open (issue #57 / Codex).
    let db = Database::in_memory().unwrap();
    db.conn
        .execute(
            "INSERT INTO sections (id, color, created_at) VALUES (0, '#abcdef', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
    db.conn
        .execute(
            "INSERT INTO shots (id, section_id, sequence_order, azimuth_in, azimuth_out, length, depth_in, depth_out, uuid) \
             VALUES (1, 0, 0, 0, 0, 1, 0, 0, 'persisted-uuid')",
            [],
        )
        .unwrap();
    db.conn
        .execute(
            "INSERT INTO shots (id, section_id, sequence_order, azimuth_in, azimuth_out, length, depth_in, depth_out, uuid) \
             VALUES (2, 0, 1, 0, 0, 1, 0, 0, NULL)",
            [],
        )
        .unwrap();
    assert!(db.shot_uuid_is_persisted(0, "persisted-uuid").unwrap());
    assert!(!db.shot_uuid_is_persisted(0, "transient-uuid").unwrap());
}

/// The check is SECTION-SCOPED, because `ensure_unique_shot_uuids` only de-dups within a
/// section - two sections can legitimately hold the same shot UUID.
///
/// Unscoped, the query answers "persisted" for section 1 because section 0's copy is on disk,
/// and the caller then keys an ignore by a UUID that is still in-memory-only for the section it
/// belongs to. The next open mints a different one and the suppression is orphaned - exactly
/// what this guard exists to prevent (issue #57).
#[test]
fn shot_uuid_is_persisted_does_not_answer_for_another_section() {
    let db = Database::in_memory().unwrap();
    for section_id in [0, 1] {
        db.conn
            .execute(
                "INSERT INTO sections (id, color, created_at) VALUES (?1, '#abcdef', '2026-01-01T00:00:00Z')",
                [section_id],
            )
            .unwrap();
    }
    // The SHARED uuid is on disk for section 0 only; section 1's copy is still in memory.
    db.conn
        .execute(
            "INSERT INTO shots (id, section_id, sequence_order, azimuth_in, azimuth_out, length, depth_in, depth_out, uuid) \
             VALUES (1, 0, 0, 0, 0, 1, 0, 0, 'shared-uuid')",
            [],
        )
        .unwrap();

    assert!(db.shot_uuid_is_persisted(0, "shared-uuid").unwrap());
    assert!(
        !db.shot_uuid_is_persisted(1, "shared-uuid").unwrap(),
        "another section's copy must not count as persisted for this one"
    );
}

#[test]
fn save_resolved_warning_collision_preserves_the_prior_ignore() {
    // A refused collision (the requested position holds a DIFFERENT shot's ignore)
    // must roll back the dedup-delete too, so this shot's OWN prior ignore is not
    // dropped on the way to refusing (issue #57 / Codex).
    let db = Database::in_memory().unwrap();
    db.conn
        .execute(
            "INSERT INTO sections (id, color, created_at) VALUES (0, '#abcdef', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
    let row = |shot_id: u32, uuid: &str| ResolvedWarning {
        section_id: 0,
        shot_id,
        warning_code: "SHORT_SHOT".to_string(),
        resolved_at: Utc::now(),
        resolved_comment: None,
        shot_uuid: Some(uuid.to_string()),
    };
    // Shot A's ignore at position 3, shot B's at position 5.
    db.save_resolved_warning(&row(3, "aaa")).unwrap();
    db.save_resolved_warning(&row(5, "bbb")).unwrap();
    // Re-resolve A claiming position 5 (where B sits): refused, and A's old row stays.
    assert!(db.save_resolved_warning(&row(5, "aaa")).is_err());
    let mut rows = db.load_resolved_warnings().unwrap();
    rows.sort_by_key(|r| r.shot_id);
    assert_eq!(
        rows.len(),
        2,
        "both shots' ignores survive the refused collision"
    );
    assert_eq!(rows[0].shot_id, 3);
    assert_eq!(rows[0].shot_uuid.as_deref(), Some("aaa"));
    assert_eq!(rows[1].shot_id, 5);
    assert_eq!(rows[1].shot_uuid.as_deref(), Some("bbb"));
}
