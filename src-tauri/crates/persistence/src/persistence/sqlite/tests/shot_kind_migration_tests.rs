//! Migration v13 (is_reference to shot kind) and unrecognised shot kinds.
//!
//! Lines 6899-7379 of the former single-file `persistence/sqlite/tests.rs` (#576 AFK-D3), moved
//! verbatim; shared helpers live in the module root (`tests.rs`). `descending_section` (former
//! lines 6855-6883) moved to `pitch_migration_tests.rs`, its only user. `section_pitches` (former
//! lines 6884-6898) moved to `pitch_migration_tests.rs`, its only user.

use super::*;

/// Opening a pre-v13 project converts the boolean `is_reference` column into the
/// 3-way `kind`, seeds every virtual shot as Jump (#27 q2), and then lets the
/// one-shot backfill flag re-decide via the chain rule (#27 q16).
#[test]
fn migrate_v13_converts_is_reference_to_kind_and_flags_backfill() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("legacy_v12.stalx");

    let mut project = Project::new("Legacy".to_string(), None);
    // R V R  -> bounded, so the backfill should settle on Jump.
    // R V V  -> one-sided, so the backfill should settle on Reference.
    project.add_section(SectionData {
        name: Some("Bounded".to_string()),
        shots: vec![
            straight_shot(0.0, 10.0).with_kind(ShotKind::Normal),
            straight_shot(0.0, 10.0).with_kind(ShotKind::Reference),
            straight_shot(0.0, 10.0).with_kind(ShotKind::Normal),
        ],
        ..Default::default()
    });
    project.add_section(SectionData {
        name: Some("Dangling".to_string()),
        shots: vec![
            straight_shot(0.0, 10.0).with_kind(ShotKind::Normal),
            straight_shot(0.0, 10.0).with_kind(ShotKind::Reference),
            straight_shot(0.0, 10.0).with_kind(ShotKind::Reference),
        ],
        ..Default::default()
    });

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        // Reshape the table back to the v12 form: a boolean is_reference where
        // every virtual shot reads 1, which is what a real pre-#27 file holds.
        db.conn
            .execute_batch(
                "ALTER TABLE shots ADD COLUMN is_reference INTEGER NOT NULL DEFAULT 0;
                 UPDATE shots SET is_reference = CASE WHEN kind != 0 THEN 1 ELSE 0 END;
                 ALTER TABLE shots DROP COLUMN kind;",
            )
            .unwrap();
        db.set_user_version(12).unwrap();
    }

    // open_existing runs ensure_schema_version → the v12→v13 migration.
    let db = Database::open_existing(&path).unwrap();
    assert_eq!(db.get_user_version().unwrap(), SCHEMA_VERSION);
    assert!(
        db.conn.prepare("SELECT kind FROM shots LIMIT 0").is_ok(),
        "kind column must exist after migration"
    );
    assert!(
        db.conn
            .prepare("SELECT is_reference FROM shots LIMIT 0")
            .is_err(),
        "the boolean column must be gone"
    );

    // The migration itself only seeds Jump (discriminant 1) per q2.
    let seeded: Vec<i64> = db
        .conn
        .prepare("SELECT kind FROM shots ORDER BY section_id, sequence_order")
        .unwrap()
        .query_map([], |r| r.get::<_, i64>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(seeded, vec![0, 1, 0, 0, 1, 1]);

    // And it hands the real verdict to the one-shot backfill on next load.
    assert!(
        db.is_kind_classification_pending(),
        "migration must flag the chain-rule backfill"
    );
}

/// #27: a `kind` byte this build cannot read must be REPORTED, not just quietly
/// substituted. `from_u8` fails it closed to `Reference` so it cannot invent a
/// swimmable leg, but the user is then owed an explanation and a choice.
#[test]
fn unrecognised_shot_kinds_are_reported_with_their_raw_value() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("bad_kind.stalx");

    let mut project = Project::new("Corrupt".to_string(), None);
    project.add_section(SectionData {
        name: Some("Main".to_string()),
        shots: vec![
            straight_shot(0.0, 10.0).with_kind(ShotKind::Normal),
            straight_shot(0.0, 10.0).with_kind(ShotKind::Jump),
            straight_shot(0.0, 10.0).with_kind(ShotKind::Reference),
        ],
        ..Default::default()
    });

    let db = {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        db
    };
    // Nothing wrong yet.
    assert!(db.unrecognised_shot_kinds().unwrap().is_empty());

    // Corrupt the middle shot's discriminant. There is no CHECK constraint, so
    // this is exactly what a bad write or a newer writer would leave behind.
    db.conn
        .execute_batch("UPDATE shots SET kind = 7 WHERE sequence_order = 1;")
        .unwrap();

    let found = db.unrecognised_shot_kinds().unwrap();
    assert_eq!(found.len(), 1, "exactly the corrupt row is reported");
    let hit = &found[0];
    assert_eq!(
        hit.raw_value, "7",
        "the raw value must survive for the dialog"
    );
    assert_eq!(hit.shot_number, 2, "1-based position for display");
    assert_eq!(hit.section_name.as_deref(), Some("Main"));
    let section_id = project.sections[0].id;
    assert_eq!(hit.section_id, section_id);
    assert_eq!(
        hit.shot_id,
        section_id * crate::models::FEATURE_ID_STRIDE + 1,
        "shot_id must be the in-memory id set_shot_kind expects, not the rowid"
    );

    // And the loader itself fails the shot closed rather than making it swimmable.
    let loaded = db.load_project().unwrap();
    assert_eq!(
        loaded.sections[0].shots[1].kind,
        ShotKind::Reference,
        "an unreadable kind must not load as a swimmable leg"
    );

    // Values OUTSIDE the u8 range have to fail closed too. The column is a
    // plain INTEGER, so -1 and 256 are both storable, and the u8 conversion on
    // the way to `from_u8` is a second place the fail-closed rule can be lost.
    // Includes values outside i32: SQLite stores INTEGER as i64, and reading the
    // column as i32 would ERROR rather than fall back, aborting the whole load.
    for bad in [
        -1_i64,
        256,
        i64::from(i32::MAX),
        2_147_483_648,
        i64::MIN,
        i64::MAX,
    ] {
        db.conn
            .execute("UPDATE shots SET kind = ?1 WHERE sequence_order = 1", [bad])
            .unwrap();
        let reported = db.unrecognised_shot_kinds().unwrap();
        assert_eq!(reported.len(), 1, "kind {bad} must still be reported");
        assert_eq!(reported[0].raw_value, bad.to_string());
        assert_eq!(
            db.load_project().unwrap().sections[0].shots[1].kind,
            ShotKind::Reference,
            "kind {bad} must load as Reference, not Normal"
        );
    }

    // The column has INTEGER *affinity*, not an INTEGER constraint - the table
    // is not STRICT - so a corrupt or hand-edited row can hold any storage
    // class. These must load, not error out the whole project.
    for bad in ["'not-a-number'", "x'DEADBEEF'", "3.5"] {
        db.conn
            .execute_batch(&format!(
                "UPDATE shots SET kind = {bad} WHERE sequence_order = 1;"
            ))
            .unwrap();
        assert_eq!(
            db.load_project().unwrap().sections[0].shots[1].kind,
            ShotKind::Reference,
            "kind {bad} must load as Reference rather than failing the load"
        );
        assert_eq!(
            db.unrecognised_shot_kinds().unwrap().len(),
            1,
            "kind {bad} must still reach the resolution prompt"
        );
    }
}

/// #27: the fail-closed substitution has to reach DISK, or it is session-only.
///
/// Nothing marks the project dirty (the user edited nothing), so without an
/// explicit repair the invalid discriminant survives every open and the prompt
/// reappears forever - while the dialog claims the choice is saved with the
/// project.
#[test]
fn unrecognised_shot_kinds_are_repaired_on_disk() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("repair.stalx");

    let mut project = Project::new("Corrupt".to_string(), None);
    project.add_section(SectionData {
        shots: vec![
            straight_shot(0.0, 10.0).with_kind(ShotKind::Normal),
            straight_shot(0.0, 10.0).with_kind(ShotKind::Jump),
        ],
        ..Default::default()
    });

    let db = {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        db
    };
    db.conn
        .execute_batch("UPDATE shots SET kind = 9 WHERE sequence_order = 1;")
        .unwrap();

    assert_eq!(db.repair_unrecognised_shot_kinds().unwrap(), 1);
    assert!(
        db.unrecognised_shot_kinds().unwrap().is_empty(),
        "nothing is left to prompt about once the row is repaired"
    );
    let kinds: Vec<i64> = db
        .conn
        .prepare("SELECT kind FROM shots ORDER BY sequence_order")
        .unwrap()
        .query_map([], |r| r.get::<_, i64>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(
        kinds,
        vec![0, 2],
        "the unreadable row is now Reference on disk"
    );

    // Idempotent, and it never touches a row it can read.
    assert_eq!(db.repair_unrecognised_shot_kinds().unwrap(), 0);
}

/// #27: the pre-#27 loader read `is_reference` as `!= 0`, so ANY truthy value
/// meant virtual. A migration that only copied `= 1` would leave a noncanonical
/// `-1` or `2` on the new column's `Normal` default and then drop the evidence -
/// permanently converting a reference tie into a swimmable leg, with the
/// backfill unable to help because the shot no longer looks virtual.
#[test]
fn migrate_v13_carries_every_truthy_legacy_reference_value() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("noncanonical.stalx");

    let mut project = Project::new("Legacy".to_string(), None);
    project.add_section(SectionData {
        shots: vec![
            straight_shot(0.0, 10.0).with_kind(ShotKind::Normal),
            straight_shot(0.0, 10.0).with_kind(ShotKind::Normal),
            straight_shot(0.0, 10.0).with_kind(ShotKind::Normal),
            straight_shot(0.0, 10.0).with_kind(ShotKind::Normal),
        ],
        ..Default::default()
    });

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        // Rebuild the v12 shape by hand with a spread of truthy values.
        db.conn
            .execute_batch(
                "ALTER TABLE shots ADD COLUMN is_reference INTEGER NOT NULL DEFAULT 0;
                 UPDATE shots SET is_reference = 0  WHERE sequence_order = 0;
                 UPDATE shots SET is_reference = 1  WHERE sequence_order = 1;
                 UPDATE shots SET is_reference = -1 WHERE sequence_order = 2;
                 UPDATE shots SET is_reference = 2  WHERE sequence_order = 3;
                 ALTER TABLE shots DROP COLUMN kind;",
            )
            .unwrap();
        db.set_user_version(12).unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    let kinds: Vec<i64> = db
        .conn
        .prepare("SELECT kind FROM shots ORDER BY sequence_order")
        .unwrap()
        .query_map([], |r| r.get::<_, i64>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    // 0 stays Normal; every truthy value becomes the Jump seed (1) the backfill
    // then judges - none is silently left Normal.
    assert_eq!(kinds, vec![0, 1, 1, 1]);
}

/// #27: the split toggles have to stay independent ACROSS RESTARTS, not just on
/// the first open.
///
/// The load-time fallback (`showJumps` -> `showReferenceShots`) gets the first
/// open right, but it is a read-time default. `save_display_setting` writes only
/// the key it is handed, so without a materialised row a user who then changes
/// References alone would find Jumps following that new value on the next open -
/// a toggle moving on its own.
#[test]
fn migrate_v13_materialises_show_jumps_from_the_old_single_toggle() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("legacy_toggle.stalx");

    let mut project = Project::new("Legacy".to_string(), None);
    project.add_section(SectionData {
        shots: vec![straight_shot(0.0, 10.0).with_kind(ShotKind::Reference)],
        ..Default::default()
    });

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        // A pre-#27 file: one combined toggle, turned OFF, and no showJumps row.
        db.conn
            .execute_batch(
                "DELETE FROM display_settings WHERE key IN ('showJumps', 'showReferenceShots');
                 INSERT INTO display_settings (key, value) VALUES ('showReferenceShots', 'false');",
            )
            .unwrap();
        db.set_user_version(12).unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    assert_eq!(
        db.get_display_setting("showJumps").unwrap().as_deref(),
        Some("false"),
        "showJumps must be materialised from the old combined toggle, not left absent"
    );

    // Now the user changes ONLY references. Jumps must not follow.
    db.save_display_setting("showReferenceShots", "true")
        .unwrap();
    assert_eq!(
        db.get_display_setting("showJumps").unwrap().as_deref(),
        Some("false"),
        "a stored showJumps must not be moved by a change to showReferenceShots"
    );
}

/// #27: a v12 file keeps its persisted undo history (`UNDO_FORMAT_SAFE_VERSION`
/// is 7), and those payloads carry the legacy `is_reference` boolean. A bare
/// `true` cannot say which virtual kind it meant, so `de_shot_kind` maps it to
/// `Jump` - replaying one after the backfill settled a tie on `Reference` would
/// make it swimmable again, with no classifier pass on the undo path to correct
/// it. The v13 migration drops both stacks, exactly as v11->v12 does after a
/// pitch flip.
#[test]
fn migrate_v13_drops_persisted_undo_history() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("legacy_v12_undo.stalx");

    let mut project = Project::new("Legacy".to_string(), None);
    project.add_section(SectionData {
        shots: vec![
            straight_shot(0.0, 10.0).with_kind(ShotKind::Normal),
            straight_shot(0.0, 10.0).with_kind(ShotKind::Reference),
        ],
        ..Default::default()
    });

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        // A persisted entry whose payload still speaks the pre-#27 boolean.
        db.conn
            .execute_batch(
                r#"INSERT INTO undo_stack (sequence, command_json, undo_data_json, executed_at)
                   VALUES (0,
                           '{"type":"set_shot_reference","section_id":0,"shot_id":1,"is_reference":true}',
                           '{}', '2026-01-01T00:00:00Z');"#,
            )
            .unwrap();
        // Back to the v12 shape so ensure_schema_version runs the v13 arm.
        db.conn
            .execute_batch(
                "ALTER TABLE shots ADD COLUMN is_reference INTEGER NOT NULL DEFAULT 0;
                 UPDATE shots SET is_reference = CASE WHEN kind != 0 THEN 1 ELSE 0 END;
                 ALTER TABLE shots DROP COLUMN kind;",
            )
            .unwrap();
        db.set_user_version(12).unwrap();
        let staged: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM undo_stack", [], |r| r.get(0))
            .unwrap();
        assert_eq!(staged, 1, "the fixture must actually stage an undo entry");
    }

    let db = Database::open_existing(&path).unwrap();
    for table in ["undo_stack", "redo_stack"] {
        let remaining: i64 = db
            .conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            remaining, 0,
            "{table} must be empty: a legacy is_reference payload cannot be replayed safely"
        );
    }
}

/// The verdicts the one-shot backfill computes have to land on the RIGHT rows
/// and survive a reopen.
///
/// Regression: the first cut wrote `UPDATE shots SET kind = ? WHERE id = ?`
/// using the in-memory `Shot::id`. That is a different ID space from the
/// `shots.id` rowid - `write.rs` never stores an id, and `read.rs` synthesises
/// one as `section_id * FEATURE_ID_STRIDE + sequence_order`. So section 0's
/// verdicts landed one row early and every later section matched nothing at
/// all, while the pending flag was cleared in the same transaction - making the
/// wrong kinds permanent. Two sections here is the point: a single-section
/// fixture passes even with the rowid bug.
#[test]
fn backfilled_shot_kinds_land_on_the_right_rows_and_survive_a_reopen() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("backfill.stalx");

    let mut project = Project::new("Backfill".to_string(), None);
    project.add_section(SectionData {
        name: Some("First".to_string()),
        shots: vec![
            straight_shot(0.0, 10.0).with_kind(ShotKind::Normal),
            straight_shot(0.0, 10.0).with_kind(ShotKind::Reference),
            straight_shot(0.0, 10.0).with_kind(ShotKind::Normal),
        ],
        ..Default::default()
    });
    project.add_section(SectionData {
        name: Some("Second".to_string()),
        shots: vec![
            straight_shot(0.0, 10.0).with_kind(ShotKind::Normal),
            straight_shot(0.0, 10.0).with_kind(ShotKind::Reference),
        ],
        ..Default::default()
    });

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
    }

    // Reload so the shots carry the ids the real backfill path sees, then feed
    // the verdicts through exactly as project_load does.
    let db = Database::open_existing(&path).unwrap();
    let loaded = db.load_project().unwrap();
    // Section 0's virtual shot -> Reference, section 1's -> Jump. Deliberately
    // different per section, and neither equal to what is on disk, so a write
    // that lands on the wrong row or no row cannot pass.
    let verdicts = vec![
        (loaded.sections[0].id, 1_u32, ShotKind::Reference),
        (loaded.sections[1].id, 1_u32, ShotKind::Jump),
    ];
    db.persist_backfilled_shot_kinds(&verdicts).unwrap();
    drop(db);

    let db = Database::open_existing(&path).unwrap();
    let on_disk: Vec<(u32, i64, i64)> = db
        .conn
        .prepare("SELECT section_id, sequence_order, kind FROM shots ORDER BY section_id, sequence_order")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();

    let s0 = loaded.sections[0].id;
    let s1 = loaded.sections[1].id;
    assert_eq!(
        on_disk,
        vec![
            (s0, 0, 0), // Normal, untouched
            (s0, 1, 2), // Reference - the verdict, on the row it was aimed at
            (s0, 2, 0), // Normal, untouched (the rowid bug shifted this one)
            (s1, 0, 0), // Normal, untouched
            (s1, 1, 1), // Jump - the rowid bug never reached this section at all
        ],
        "each verdict must land on its own (section_id, sequence_order) row"
    );
    assert!(
        !db.is_kind_classification_pending(),
        "the flag clears with the writes, in the same transaction"
    );
}
