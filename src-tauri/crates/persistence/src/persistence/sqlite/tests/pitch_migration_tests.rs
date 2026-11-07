//! Migrations v12 (the pitch-convention flip), v14 (azimuth normalisation) and v15.
//!
//! Lines 7623-8558 of the former single-file `persistence/sqlite/tests.rs` (#576 AFK-D3), moved
//! verbatim; shared helpers live in the module root (`tests.rs`). `descending_section` (former
//! lines 6855-6883, in `shot_kind_migration_tests.rs`'s range) leads this file: only it uses that
//! helper. `section_pitches` (former lines 6884-6898, in `shot_kind_migration_tests.rs`'s range)
//! leads this file: only it uses that helper.

use super::*;

/// Build a section of shots that all descend at ~30°, with `sign` applied to the
/// recorded pitch: `-1.0` reproduces the pre-#296 DMP storage (+up), `1.0` the
/// already-correct Walls/Compass storage (+down).
fn descending_section(name: &str, sign: f64, count: usize) -> SectionData {
    // length 10 with dz 5 is a 30° descent, well clear of the migration's
    // near-horizontal and short-shot floors.
    let shots = (0..count)
        .map(|i| ShotData {
            azimuth_in: 90.0,
            azimuth_out: 90.0,
            length: 10.0,
            depth_in: 5.0 * i as f64,
            depth_out: 5.0 * (i + 1) as f64,
            pitch_in: Some(sign * 30.0),
            pitch_out: Some(sign * 30.0),
            ..Default::default()
        })
        .collect();
    SectionData {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#ff5733".to_string(),
        direction: Some(SurveyDirection::In),
        quality: 1.0,
        shots,
    }
}

fn section_pitches(db: &Database, section_name: &str) -> Vec<Option<f64>> {
    let mut stmt = db
        .conn
        .prepare(
            "SELECT s.pitch_in FROM shots s
             JOIN sections sec ON sec.id = s.section_id
             WHERE sec.name = ?1 ORDER BY s.sequence_order",
        )
        .unwrap();
    stmt.query_map([section_name], |r| r.get::<_, Option<f64>>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

/// Opening a pre-v12 project normalises only the sections whose pitch is stored
/// in the old +up convention, and leaves everything else exactly as it was (#296).
///
/// The verdict has to be per section because nothing records which importer
/// produced a section, and a project can hold a DMP section and a Compass section
/// side by side, and only the DMP one may be touched.
#[test]
fn migrate_v12_flips_only_the_sections_stored_in_the_old_convention() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("legacy_v11.stalx");

    let mut project = Project::new("Mixed".to_string(), None);
    // Pre-#296 DMP: descending shots recorded negative. Must be flipped.
    project.add_section(descending_section("FromDmp", -1.0, 4));
    // Walls/Compass: already +down. Must be left alone.
    project.add_section(descending_section("FromCompass", 1.0, 4));
    // Ariane never populates inclination, so there is no signal to act on.
    project.add_section(descending_section("FromAriane", 0.0, 4));
    // Too few usable shots to call a section-wide verdict, even though each
    // individual shot looks reversed.
    project.add_section(descending_section("TooShort", -1.0, 2));

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        db.set_user_version(11).unwrap();
    }

    // open_existing runs ensure_schema_version → the v11→v12 migration.
    let db = Database::open_existing(&path).unwrap();
    assert_eq!(db.get_user_version().unwrap(), SCHEMA_VERSION);

    assert_eq!(
        section_pitches(&db, "FromDmp"),
        vec![Some(30.0); 4],
        "a section stored +up is negated into +down"
    );
    assert_eq!(
        section_pitches(&db, "FromCompass"),
        vec![Some(30.0); 4],
        "a section already +down must not be double-flipped"
    );
    assert_eq!(
        section_pitches(&db, "FromAriane"),
        vec![Some(0.0); 4],
        "all-zero pitch carries no signal and is left untouched"
    );
    assert_eq!(
        section_pitches(&db, "TooShort"),
        vec![Some(-30.0); 2],
        "too few usable shots is not a verdict; left alone rather than guessed"
    );
}

/// The migration must not invent a pitch where none was recorded, and a shot
/// whose recorded pitch genuinely contradicts its depth change must stay
/// contradictory: that disagreement is a real survey blunder and validation
/// exists to surface it (#296).
#[test]
fn migrate_v12_preserves_null_pitch_and_does_not_launder_a_blunder() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("legacy_nulls.stalx");

    let mut project = Project::new("Nulls".to_string(), None);
    let mut section = descending_section("FromDmp", -1.0, 6);
    // One shot never had an inclination recorded.
    section.shots[1].pitch_in = None;
    section.shots[1].pitch_out = None;
    // One shot is a genuine blunder: it disagrees with the rest of its own
    // section about which way it points. The section verdict flips it along
    // with everything else, so it comes out still disagreeing with its depth
    // change, which is exactly right.
    section.shots[5].pitch_in = Some(30.0);
    section.shots[5].pitch_out = Some(30.0);
    project.add_section(section);

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        db.set_user_version(11).unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    let pitches = section_pitches(&db, "FromDmp");
    assert_eq!(pitches[0], Some(30.0), "ordinary shot normalised");
    assert_eq!(pitches[1], None, "an unrecorded pitch stays unrecorded");
    assert_eq!(pitches[2], Some(30.0), "ordinary shot normalised");
    assert_eq!(
        pitches[5],
        Some(-30.0),
        "the outlier inherits the section verdict and stays wrong, so validation \
         still flags it instead of the migration silently agreeing with it"
    );
}

/// The standard legs of a parsed DMP section as `SectionData`, for the v12
/// migration test below. The parser crate's own `to_section_data` was removed with
/// #576 B24 (the import path converts in the wizard). This keeps its shot mapping
/// (`to_shot_data`: wrapped azimuths, pitches, LRUD, marker, temperature) and its
/// name, date and quality, but NOT its section fields the test does not read: the
/// direction is `None` (the removed fn mapped the dump's In/Out) and the color is
/// empty (it took the caller's). The v12 test reads only the shots' pitches.
fn flat_dmp_section_data(section: &stalagmight_parsers::parsers::dmp::DmpSection) -> SectionData {
    use stalagmight_parsers::parsers::dmp::DmpShotType;
    SectionData {
        name: Some(section.name.clone()),
        surveyor: None,
        date: Some(section.date.date()),
        color: String::new(),
        direction: None,
        quality: 1.0,
        shots: section
            .shots
            .iter()
            .filter(|s| s.shot_type == DmpShotType::Standard)
            .map(|s| ShotData {
                uuid: None,
                azimuth_in: crate::models::normalize_azimuth(s.heading_in),
                azimuth_out: crate::models::normalize_azimuth(s.heading_out),
                length: s.length,
                depth_in: s.depth_in,
                depth_out: s.depth_out,
                pitch_in: Some(s.pitch_in),
                pitch_out: Some(s.pitch_out),
                left: Some(s.left),
                right: Some(s.right),
                up: Some(s.up),
                down: Some(s.down),
                comment: s.comment.clone(),
                kind: ShotKind::Normal,
                marker_index: Some(u32::from(s.marker_index)),
                temperature: Some(s.temperature),
            })
            .collect(),
    }
}

/// Real device bytes with no vertical signal must be left exactly as they are.
///
/// `test_v5.dmp` records no depth change on any shot, so the geometry cannot say
/// which convention its inclinations are in. The migration has to decline rather
/// than guess: silently negating a section it cannot reason about would corrupt
/// it, and unlike a wrong warning that is not something the user can see.
#[test]
fn migrate_v12_leaves_a_section_with_no_vertical_signal_untouched() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../e2e/fixtures/test_v5.dmp");
    assert!(fixture.exists(), "Fixture missing: {}", fixture.display());
    let dmp = stalagmight_parsers::parsers::dmp::parse_dmp(&fixture).unwrap();

    let mut project = Project::new("Flat".to_string(), None);
    let mut stored: Vec<Option<f64>> = Vec::new();
    for section in &dmp.sections {
        let mut data = flat_dmp_section_data(section);
        if data.shots.len() < 4 {
            continue;
        }
        // Precondition: this fixture really is flat, so there is nothing to infer
        // from. If a future fixture gains depth changes this assert fires rather
        // than the test quietly stopping to cover anything.
        for shot in &data.shots {
            assert!(
                (shot.depth_out - shot.depth_in).abs() <= 0.05,
                "fixture gained a depth change; this test no longer covers the \
                 no-signal path"
            );
        }
        data.name = Some(format!("S{}", stored.len()));
        stored.extend(data.shots.iter().map(|s| s.pitch_in));
        project.add_section(data);
    }
    assert!(!stored.is_empty(), "fixture yielded no usable section");
    assert!(
        stored.iter().any(|p| p.is_some_and(|v| v.abs() > 1.0)),
        "fixture has no recorded inclination, so there would be nothing to leave alone"
    );

    let temp = TempDir::new().unwrap();
    let path = temp.path().join("flat.stalx");
    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        db.set_user_version(11).unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    assert_eq!(db.get_user_version().unwrap(), SCHEMA_VERSION);
    let after: Vec<Option<f64>> = db
        .load_project()
        .unwrap()
        .sections
        .iter()
        .flat_map(|s| s.shots.iter().map(|sh| sh.pitch_in))
        .collect();
    assert_eq!(
        after, stored,
        "with no depth change to compare against, every pitch must survive verbatim"
    );
}

/// Write a pre-v12 project to `path` and stamp it v11, so the next
/// `open_existing` runs the pitch migration over it.
fn seed_v11_project(path: &Path, project: &Project) {
    let mut db = Database::open(path).unwrap();
    db.save_project(project).unwrap();
    db.set_user_version(11).unwrap();
}

/// `MIN_VOTING_SHOTS` is 3 and the comparison is `>=`, so a section with exactly
/// three usable shots is decided rather than skipped. The existing coverage uses
/// 2 (declined) and 4 (flipped), so a `>=` slipping to `>` would go unnoticed.
#[test]
fn migrate_v12_decides_a_section_at_exactly_the_minimum_vote_count() {
    let temp = TempDir::new().unwrap();

    for (count, expected, why) in [
        (
            2usize,
            -30.0,
            "two usable shots is below the minimum, so leave it",
        ),
        (
            3usize,
            30.0,
            "three usable shots is the minimum and must decide",
        ),
    ] {
        let path = temp.path().join(format!("votes_{count}.stalx"));
        let mut project = Project::new("Votes".to_string(), None);
        project.add_section(descending_section("FromDmp", -1.0, count));
        seed_v11_project(&path, &project);

        let db = Database::open_existing(&path).unwrap();
        assert_eq!(
            section_pitches(&db, "FromDmp"),
            vec![Some(expected); count],
            "{why}"
        );
    }
}

/// The version stamp is written outside the migration's transaction, so a crash
/// between the two re-runs the migration against already-flipped rows on the
/// next open. The verdict is re-derived from the geometry each time, so a second
/// pass must decline. Asserted explicitly because a future change to the
/// criterion could silently make the migration flip back and forth.
#[test]
fn migrate_v12_is_idempotent_when_run_twice() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("twice.stalx");

    let mut project = Project::new("Twice".to_string(), None);
    project.add_section(descending_section("FromDmp", -1.0, 4));
    seed_v11_project(&path, &project);

    let db = Database::open_existing(&path).unwrap();
    let after_first = section_pitches(&db, "FromDmp");
    assert_eq!(after_first, vec![Some(30.0); 4], "first pass flips");

    // Simulate the interrupted case: rows are already flipped, but the stamp
    // never landed, so the migration runs over them again.
    Database::migrate_pitch_to_down_convention(&db.conn).unwrap();
    assert_eq!(
        section_pitches(&db, "FromDmp"),
        after_first,
        "a second pass over already-flipped rows must not flip them back"
    );
}

/// The flip must not commit on its own.
///
/// `ensure_schema_version` wraps the flip and `invalidate_pre_pitch_flip_state`
/// in one transaction, because the verdict is re-derived from the rows
/// themselves: a flip that committed alone would look already-correct on the
/// next open, so the migration would decline, the invalidation would never run,
/// and v12 would be stamped over permanently stale undo history, warning
/// dismissals.
///
/// Rolling back the caller's transaction must therefore undo the flip. If this
/// fails, the migration has taken a transaction of its own again and that
/// window is back.
#[test]
fn migrate_v12_flip_is_governed_by_the_callers_transaction() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("rollback.stalx");

    let mut project = Project::new("Rollback".to_string(), None);
    project.add_section(descending_section("FromDmp", -1.0, 4));

    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    assert_eq!(section_pitches(&db, "FromDmp"), vec![Some(-30.0); 4]);

    let tx = db.conn.unchecked_transaction().unwrap();
    assert!(
        !Database::migrate_pitch_to_down_convention(&tx)
            .unwrap()
            .is_empty(),
        "precondition: this section is the kind the migration flips"
    );
    assert_eq!(
        section_pitches(&db, "FromDmp"),
        vec![Some(30.0); 4],
        "precondition: the flip is visible inside the open transaction"
    );
    tx.rollback().unwrap();

    assert_eq!(
        section_pitches(&db, "FromDmp"),
        vec![Some(-30.0); 4],
        "rolling back the caller's transaction must undo the flip, so the flip \
         and the invalidation cannot be separated by a crash"
    );
}

/// A leg steeper than vertical is a survey blunder, and validation skips the
/// inclination cross-check for it. It must not vote in the migration either, or
/// a single typo can drag a section's verdict in whichever direction its sign
/// happens to point.
#[test]
fn migrate_v12_ignores_out_of_range_legs_when_voting() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("blunder.stalx");

    // The good shots are deliberately shallow, so each contributes little to the
    // as-is total and the blunder can dominate. Three correct +2 shots (implied
    // +2, so as-is 0 and flipped 4 apiece) plus one -91 leg whose geometry
    // implies +87 (as-is 178, flipped 4).
    //
    // Let the blunder vote and the totals are as-is 178 / flipped 16, which
    // clears the decisive ratio and flips all four shots, so a single typo
    // corrupts three correct ones. Exclude it and the totals are as-is 0 /
    // flipped 12, which declines. That gap is what this test exists to hold: it
    // fails if MAX_VOTING_PITCH_DEG is raised or the guard removed.
    let shallow = |i: usize| ShotData {
        azimuth_in: 90.0,
        azimuth_out: 90.0,
        length: 10.0,
        depth_in: 0.349 * i as f64,
        depth_out: 0.349 * (i + 1) as f64,
        pitch_in: Some(2.0),
        pitch_out: Some(2.0),
        ..Default::default()
    };
    let mut shots: Vec<ShotData> = (0..3).map(shallow).collect();
    shots.push(ShotData {
        azimuth_in: 90.0,
        azimuth_out: 90.0,
        length: 10.0,
        depth_in: 1.047,
        depth_out: 11.033,
        pitch_in: Some(-91.0),
        pitch_out: Some(-91.0),
        ..Default::default()
    });

    let mut section = descending_section("FromCompass", 1.0, 1);
    section.shots = shots;
    let mut project = Project::new("Blunder".to_string(), None);
    project.add_section(section);
    seed_v11_project(&path, &project);

    let db = Database::open_existing(&path).unwrap();
    assert_eq!(
        section_pitches(&db, "FromCompass"),
        vec![Some(2.0), Some(2.0), Some(2.0), Some(-91.0)],
        "an out-of-range leg must neither vote nor be rewritten, and must not \
         drag the correct shots around it"
    );
}

/// Everything outside the `shots` table that still holds a pre-flip pitch is
/// dropped when the migration flips a section, because the v12 stamp means
/// there is no second pass to catch it later.
///
/// Undo is the sharpest case: the payload carries absolute pitch values, so
/// replaying one entry writes a +up pitch straight back into a migrated file.
#[test]
fn migrate_v12_discards_state_that_still_holds_pre_flip_pitch() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("stale.stalx");

    let mut project = Project::new("Stale".to_string(), None);
    project.add_section(descending_section("FromDmp", -1.0, 4));

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        db.save_undo_stack(vec![(
            r#"{"UpdateShot":{"pitch_in":-30.0}}"#.to_string(),
            r#"{"PreviousValues":{"pitch_in":-30.0}}"#.to_string(),
        )])
        .unwrap();
        db.save_resolved_warnings_bulk(&[ResolvedWarning {
            section_id: 0,
            shot_id: 0,
            warning_code: "INCLINATION_DEPTH_MISMATCH".to_string(),
            resolved_at: Utc::now(),
            resolved_comment: None,
            shot_uuid: None,
        }])
        .unwrap();
        db.set_user_version(11).unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    assert_eq!(
        section_pitches(&db, "FromDmp"),
        vec![Some(30.0); 4],
        "precondition: the migration flipped this section"
    );

    assert!(
        db.load_undo_stack_with_timestamps().unwrap().is_empty(),
        "undo history carrying pre-flip pitch must be dropped, not replayable"
    );
    assert!(
        db.load_resolved_warnings().unwrap().is_empty(),
        "an inclination dismissal taken under the old convention must be cleared"
    );
}

/// One in-range blunder must not outvote the section majority.
///
/// The verdict counts votes rather than summing angular errors, because summing
/// let a single shot decide by sheer magnitude. Here three correct shallow
/// shots disagree by 4 degrees each if flipped (12 total) while one blunder
/// recorded at -89 against +89 implied geometry contributes 178 to the as-is
/// score and 0 to the flipped score. Summed, the ratio flips all four, which
/// corrupts the three correct shots and launders the blunder into agreement
/// with its own depth change. Counted, it is one vote against three.
#[test]
fn migrate_v12_does_not_let_one_blunder_outvote_the_section() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("outvoted.stalx");

    let shallow = |i: usize| ShotData {
        azimuth_in: 90.0,
        azimuth_out: 90.0,
        length: 10.0,
        depth_in: 0.349 * i as f64,
        depth_out: 0.349 * (i + 1) as f64,
        pitch_in: Some(2.0),
        pitch_out: Some(2.0),
        ..Default::default()
    };
    let mut shots: Vec<ShotData> = (0..3).map(shallow).collect();
    // Near-vertical descent (implied about +89) recorded as -89: in range, so
    // it votes, but it is one vote.
    shots.push(ShotData {
        azimuth_in: 90.0,
        azimuth_out: 90.0,
        length: 10.0,
        depth_in: 1.047,
        depth_out: 11.046,
        pitch_in: Some(-89.0),
        pitch_out: Some(-89.0),
        ..Default::default()
    });

    let mut section = descending_section("FromCompass", 1.0, 1);
    section.shots = shots;
    let mut project = Project::new("Outvoted".to_string(), None);
    project.add_section(section);
    seed_v11_project(&path, &project);

    let db = Database::open_existing(&path).unwrap();
    assert_eq!(
        section_pitches(&db, "FromCompass"),
        vec![Some(2.0), Some(2.0), Some(2.0), Some(-89.0)],
        "three correct shots must outvote one blunder, and the blunder must stay \
         wrong so validation keeps flagging it"
    );
}

/// The other side of the margin: a real DMP section carrying one genuine
/// blunder must still be converted. The blunder inherits the section verdict
/// and stays flagged, which is what the cross-check is for.
#[test]
fn migrate_v12_still_flips_a_section_that_contains_one_blunder() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("blunder_in_dmp.stalx");

    // Six shots stored +up (pre-#296 DMP), plus one whose recorded pitch agrees
    // with its depth change and so votes the other way.
    let mut section = descending_section("FromDmp", -1.0, 6);
    section.shots.push(ShotData {
        azimuth_in: 90.0,
        azimuth_out: 90.0,
        length: 10.0,
        depth_in: 30.0,
        depth_out: 35.0,
        pitch_in: Some(30.0),
        pitch_out: Some(30.0),
        ..Default::default()
    });
    let mut project = Project::new("BlunderInDmp".to_string(), None);
    project.add_section(section);
    seed_v11_project(&path, &project);

    let db = Database::open_existing(&path).unwrap();
    let pitches = section_pitches(&db, "FromDmp");
    assert_eq!(
        pitches[..6].to_vec(),
        vec![Some(30.0); 6],
        "six votes to flip must carry the section past one dissenter"
    );
    assert_eq!(
        pitches[6],
        Some(-30.0),
        "the dissenting shot inherits the section verdict and is left contradicting \
         its own depth change, so validation still flags it"
    );
}

/// A shot with one impossible leg must not vote, even when the other leg drags
/// the average back inside the range.
///
/// The TML sign convention changed on both sides of the boundary at once, so the
/// vote guard has to reject per leg the way `Validator::validate_shot` does.
/// Averaging first and testing the mean lets `pitch_in = -91` ride in on a
/// `pitch_out = 2`.
#[test]
fn migrate_v12_rejects_a_shot_with_one_out_of_range_leg() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("half_blunder.stalx");

    // Same shape as the out-of-range test: three shallow, correct +2 shots that
    // contribute nothing as-is, plus one steep shot whose legs average to a
    // plausible -44.5 but whose pitch_in is impossible.
    let shallow = |i: usize| ShotData {
        azimuth_in: 90.0,
        azimuth_out: 90.0,
        length: 10.0,
        depth_in: 0.349 * i as f64,
        depth_out: 0.349 * (i + 1) as f64,
        pitch_in: Some(2.0),
        pitch_out: Some(2.0),
        ..Default::default()
    };
    let mut shots: Vec<ShotData> = (0..3).map(shallow).collect();
    shots.push(ShotData {
        azimuth_in: 90.0,
        azimuth_out: 90.0,
        length: 10.0,
        depth_in: 1.047,
        depth_out: 11.033,
        pitch_in: Some(-91.0),
        pitch_out: Some(2.0),
        ..Default::default()
    });

    let mut section = descending_section("FromCompass", 1.0, 1);
    section.shots = shots;
    let mut project = Project::new("HalfBlunder".to_string(), None);
    project.add_section(section);
    seed_v11_project(&path, &project);

    let db = Database::open_existing(&path).unwrap();
    assert_eq!(
        section_pitches(&db, "FromCompass"),
        vec![Some(2.0), Some(2.0), Some(2.0), Some(-91.0)],
        "a shot with one out-of-range leg must not vote, so the correct shots \
         around it stay put"
    );
}

/// v14 -> v15 removes the `collab_state` table AND the persisted undo history.
///
/// Dropping the table is the visible half. The undo drop is the load-bearing one:
/// `Command` is an internally tagged enum with no catch-all, so an entry stored as
/// `{"type":"apply_collab_changes"}` no longer deserializes - and the loader SKIPS a
/// bad entry rather than truncating, keeping the entries on both sides of it. A v14
/// file reaches this migration with its stacks intact (`UNDO_FORMAT_SAFE_VERSION` is
/// 7), so without the drop a user who had applied collaborator changes would undo
/// straight through the hole and replay stale entries over the applied edits.
#[test]
fn migrate_v15_drops_collab_state_and_the_undo_history() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("v14.stalx");

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("V14".to_string(), None))
            .unwrap();
        // A v14 file as it would exist in the wild: the sidecar table, plus an undo
        // entry naming a command this build can no longer deserialize, plus one it
        // still can (so the test would catch a drop that only removed the bad row).
        db.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS collab_state (
                     id INTEGER PRIMARY KEY CHECK (id = 1),
                     state_json TEXT NOT NULL
                 );
                 INSERT INTO collab_state (id, state_json) VALUES (1, '{\"enabled\":true}');",
            )
            .unwrap();
        db.save_undo_stack(vec![
            (
                r#"{"type":"apply_collab_changes"}"#.to_string(),
                "null".to_string(),
            ),
            (
                r#"{"type":"update_project_settings"}"#.to_string(),
                "null".to_string(),
            ),
        ])
        .unwrap();
        // The REDO stack too: `drop_legacy_undo_history` loops both, and a redo entry
        // is just as unsafe to replay - a v14 user who undid past an
        // `apply_collab_changes` reaches this migration with the apply sitting in
        // redo. Asserting only `undo_stack` would let a helper that stopped looping
        // both tables through unnoticed.
        db.save_redo_stack_with_timestamps(vec![(
            r#"{"type":"apply_collab_changes"}"#.to_string(),
            "null".to_string(),
            None,
        )])
        .unwrap();
        db.set_user_version(14).unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    assert_eq!(db.get_user_version().unwrap(), SCHEMA_VERSION);
    assert!(
        !db.table_exists("collab_state").unwrap(),
        "v15 must drop the collab sidecar table, not leave a dead one in every file"
    );
    for table in ["undo_stack", "redo_stack"] {
        let remaining: i64 = db
            .conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            remaining, 0,
            "the whole {table} must go: skipping only the undeserializable entry \
             would leave a hole that replays stale entries over the applied changes"
        );
    }
}

/// #24: opening a pre-v14 project rewrites every stored azimuth into `[0, 360)`.
///
/// Nothing changed meaning - `Shot::average_azimuth` used Rust's truncated `%`, which
/// returned the ANTIPODE of the circular mean for a pair more than 540 degrees apart, and
/// now wraps with `rem_euclid`. Normalising takes the ambiguity out of the FILE so the
/// same `.stalx` renders identically on either build.
///
/// Both fixtures run because this used to be gated on a "reinterpreted" verdict: the
/// first holds a shot whose walked bearing really did move, the second only readings no
/// build ever misread. The gate is gone, so the two cases must behave identically.
#[test]
fn migrate_v14_normalises_stored_azimuths() {
    let temp = TempDir::new().unwrap();

    let out_of_range = |azimuth_in: f64, azimuth_out: f64| ShotData {
        azimuth_in,
        azimuth_out,
        length: 10.0,
        depth_in: 0.0,
        depth_out: 0.0,
        ..Default::default()
    };

    for (file, shots, expected) in [
        (
            "legacy_v13_azimuth.stalx",
            vec![
                // 600 and 0 are 600 apart: the truncated wrap walked this 180 degrees off.
                out_of_range(600.0, 0.0),
                // Out of range but only 20 apart - normalised, never misread.
                out_of_range(370.0, 350.0),
                // Already canonical: must be left exactly alone.
                out_of_range(12.5, 13.5),
                // A hair WEST of north. `rem_euclid` alone maps both of these to
                // exactly 360.0, so the migration would write the very
                // `INVALID_AZIMUTH` its "rows go inert" justification promises it
                // clears - and `geo_key` wraps 360.0 back to 0.0, so the migrated
                // project and an untouched sibling would disagree on that shot.
                // `normalize_azimuth` clamps the escape.
                out_of_range(-1e-14, -2.8e-14),
            ],
            vec![(240.0, 0.0), (10.0, 350.0), (12.5, 13.5), (0.0, 0.0)],
        ),
        (
            "legacy_v13_benign.stalx",
            vec![out_of_range(370.0, 350.0)],
            vec![(10.0, 350.0)],
        ),
    ] {
        let path = temp.path().join(file);
        let mut project = Project::new("Legacy".to_string(), None);
        project.add_section(SectionData {
            name: Some("Wide".to_string()),
            shots,
            ..Default::default()
        });

        {
            let mut db = Database::open(&path).unwrap();
            db.save_project(&project).unwrap();
            db.set_user_version(13).unwrap();
        }

        let db = Database::open_existing(&path).unwrap();
        assert_eq!(db.get_user_version().unwrap(), SCHEMA_VERSION);

        let stored: Vec<(f64, f64)> = db
            .conn
            .prepare("SELECT azimuth_in, azimuth_out FROM shots ORDER BY sequence_order")
            .unwrap()
            .query_map([], |r| Ok((r.get::<_, f64>(0)?, r.get::<_, f64>(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            stored, expected,
            "{file}: every reading must land in [0, 360) and an in-range one must be untouched"
        );
    }
}

/// The "this shot was walked 180 degrees off" verdict must not fire on a pair the old
/// truncated wrap handled correctly.
///
/// `average_azimuth_of` wraps `azimuth_out - azimuth_in + 540.0`; truncated `%` and
/// `rem_euclid` agree whenever that expression is non-negative OR an exact multiple of
/// 360. The naive `azimuth_out - azimuth_in < -540.0` test drops the second clause, so
/// every exact `-360k` offset was reported as reinterpreted when nothing moved.
#[test]
fn the_antipode_verdict_ignores_exact_360_multiples() {
    // The pre-#24 wrap, verbatim, as the oracle.
    let old_mean = |azimuth_in: f64, azimuth_out: f64| {
        let diff = (azimuth_out - azimuth_in + 540.0) % 360.0 - 180.0;
        (azimuth_in + diff / 2.0 + 360.0) % 360.0
    };

    for (azimuth_in, azimuth_out, expected, why) in [
        (
            900.0,
            0.0,
            false,
            "-900 is an exact -360k offset: both wraps walk it to the same bearing",
        ),
        (
            1260.0,
            0.0,
            false,
            "-1260 likewise - the naive `< -540` test called both of these reinterpreted",
        ),
        (
            600.0,
            0.0,
            true,
            "-600 is the real thing: the old wrap returned the antipode",
        ),
        (
            370.0,
            350.0,
            false,
            "out of range but only 20 apart - never misread",
        ),
    ] {
        // Ground truth: did the walked bearing actually move?
        let moved = {
            let delta = (old_mean(azimuth_in, azimuth_out)
                - crate::models::average_azimuth_of(azimuth_in, azimuth_out))
            .abs();
            delta.min(360.0 - delta) > 1e-9
        };
        assert_eq!(
            moved, expected,
            "fixture is wrong for ({azimuth_in}, {azimuth_out}): {why}"
        );
        assert_eq!(
            Database::old_wrap_returned_the_antipode(azimuth_in, azimuth_out),
            expected,
            "({azimuth_in}, {azimuth_out}): {why}"
        );
    }
}

/// Normalising must not move the cave: the walked bearing of a normalised pair is the one
/// the raw readings always described. This is the whole justification for rewriting stored
/// data instead of leaving it - it is a no-op for geometry under this build, and it takes
/// the truncated-`%` trap away from every older build too.
#[test]
fn migrate_v14_normalisation_leaves_the_walked_bearing_alone() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("legacy_v13_geometry.stalx");

    let mut project = Project::new("Geometry".to_string(), None);
    project.add_section(SectionData {
        name: Some("Wide".to_string()),
        shots: vec![ShotData {
            azimuth_in: 600.0,
            azimuth_out: 0.0,
            length: 10.0,
            ..Default::default()
        }],
        ..Default::default()
    });
    let before = project.compute_station_coordinates();

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        db.set_user_version(13).unwrap();
    }

    let migrated = Database::open_existing(&path)
        .unwrap()
        .load_project()
        .unwrap();
    let after = migrated.compute_station_coordinates();
    assert_eq!(before.len(), after.len());
    for (a, b) in before.iter().zip(&after) {
        assert!(
            (a.coordinates.x - b.coordinates.x).abs() < 1e-9
                && (a.coordinates.y - b.coordinates.y).abs() < 1e-9,
            "normalisation moved a station: {:?} -> {:?}",
            a.coordinates,
            b.coordinates
        );
    }
}

/// Dismissals of a warning the convention change cannot affect must survive.
///
/// `LENGTH_BELOW_DEPTH_CHANGE` compares length against the depth change and
/// never reads pitch, so clearing it would make previously dismissed warnings
/// reappear for no reason. Dismissals on sections the migration did not flip
/// must survive too.
#[test]
fn migrate_v12_only_clears_dismissals_whose_meaning_changed() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("dismissals.stalx");

    let mut project = Project::new("Dismissals".to_string(), None);
    project.add_section(descending_section("FromDmp", -1.0, 4));
    project.add_section(descending_section("FromCompass", 1.0, 4));

    let flipped_id = project.sections[0].id;
    let untouched_id = project.sections[1].id;

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        let warn = |section_id: u32, code: &str| ResolvedWarning {
            section_id,
            shot_id: 0,
            warning_code: code.to_string(),
            resolved_at: Utc::now(),
            resolved_comment: None,
            shot_uuid: None,
        };
        db.save_resolved_warnings_bulk(&[
            warn(flipped_id, "INCLINATION_DEPTH_MISMATCH"),
            warn(flipped_id, "LENGTH_BELOW_DEPTH_CHANGE"),
            warn(untouched_id, "INCLINATION_DEPTH_MISMATCH"),
        ])
        .unwrap();
        db.set_user_version(11).unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    assert_eq!(
        section_pitches(&db, "FromDmp"),
        vec![Some(30.0); 4],
        "precondition: the DMP section was flipped"
    );

    let mut remaining: Vec<(u32, String)> = db
        .load_resolved_warnings()
        .unwrap()
        .into_iter()
        .map(|w| (w.section_id, w.warning_code))
        .collect();
    remaining.sort();

    let mut expected = vec![
        (flipped_id, "LENGTH_BELOW_DEPTH_CHANGE".to_string()),
        (untouched_id, "INCLINATION_DEPTH_MISMATCH".to_string()),
    ];
    expected.sort();

    assert_eq!(
        remaining, expected,
        "only the inclination dismissal on the flipped section should go: a \
         length warning never reads pitch, and an untouched section did not move"
    );
}

/// The mirror of the test above: a project the migration did not need to touch
/// keeps its undo history. Dropping it unconditionally would cost every user
/// their undo stack on upgrade for no reason.
#[test]
fn migrate_v12_does_not_flip_an_untouched_project_but_v13_still_drops_its_undo() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("untouched.stalx");

    let mut project = Project::new("Untouched".to_string(), None);
    project.add_section(descending_section("FromCompass", 1.0, 4));

    {
        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        db.save_undo_stack(vec![(
            r#"{"UpdateShot":{"pitch_in":30.0}}"#.to_string(),
            r#"{"PreviousValues":{"pitch_in":30.0}}"#.to_string(),
        )])
        .unwrap();
        db.set_user_version(11).unwrap();
    }

    let db = Database::open_existing(&path).unwrap();
    assert_eq!(
        section_pitches(&db, "FromCompass"),
        vec![Some(30.0); 4],
        "precondition: nothing was flipped"
    );
    // The v11->v12 pitch migration itself still leaves history alone when it
    // flips nothing - `invalidate_pre_pitch_flip_state` is only called for a
    // non-empty flip set. But this file also crosses v12->v13, which drops the
    // stacks unconditionally: any pre-#27 payload can carry an `is_reference`
    // boolean, and a bare `true` cannot say which virtual kind it meant, so
    // replaying one could turn a Reference tie back into a swimmable Jump. That
    // supersedes the narrower v12 guarantee this test used to assert
    // end-to-end; there is no version between 12 and SCHEMA_VERSION to observe
    // it at any more.
    assert_eq!(
        db.load_undo_stack_with_timestamps().unwrap().len(),
        0,
        "crossing v13 drops persisted undo: legacy is_reference payloads are unsafe to replay"
    );
}
