//! Stored shot ranks, the per-section id window and the kind-classification flag.
//!
//! Lines 7380-7622 of the former single-file `persistence/sqlite/tests.rs` (#576 AFK-D3), moved
//! verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;

// === Shot ids on load: dense, inside the section's window (B35(f), A18(d), #576) ===

/// A project with sections `0..shot_counts.len()`, the i-th holding
/// `shot_counts[i]` shots, saved to an in-memory database.
fn saved_project_with_shots(shot_counts: &[usize]) -> Database {
    let mut project = Project::new("CaveA".to_string(), None);
    for (i, count) in shot_counts.iter().enumerate() {
        project.add_section(SectionData {
            name: Some(format!("StationA{i}")),
            shots: (0..*count)
                .map(|n| ShotData {
                    length: 1.0 + n as f64,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        });
    }
    let mut db = Database::in_memory().unwrap();
    db.save_project(&project).unwrap();
    db
}

/// Rewrite `section_id`'s stored ranks, in their current order, to `ranks` - the
/// shape a foreign or hand-edited file can have. Every row is parked on a
/// negative rank first so `UNIQUE(section_id, sequence_order)` never sees two
/// rows on one rank part-way through.
fn set_sequence_orders(db: &Database, section_id: u32, ranks: &[i64]) {
    let rowids: Vec<i64> = db
        .conn
        .prepare("SELECT rowid FROM shots WHERE section_id = ?1 ORDER BY sequence_order")
        .unwrap()
        .query_map([section_id], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(rowids.len(), ranks.len(), "one rank per stored shot");
    for (i, rowid) in rowids.iter().enumerate() {
        db.conn
            .execute(
                "UPDATE shots SET sequence_order = ?1 WHERE rowid = ?2",
                rusqlite::params![-1_000_000 - i as i64, rowid],
            )
            .unwrap();
    }
    for (rowid, rank) in rowids.iter().zip(ranks) {
        db.conn
            .execute(
                "UPDATE shots SET sequence_order = ?1 WHERE rowid = ?2",
                rusqlite::params![rank, rowid],
            )
            .unwrap();
    }
}

fn shot_ids(project: &Project, section_id: u32) -> Vec<u32> {
    project
        .get_section(section_id)
        .unwrap()
        .shots
        .iter()
        .map(|s| s.id)
        .collect()
}

/// B35(f): a gap in the stored ranks must not let the next shot duplicate an id.
///
/// Ids used to come from the stored rank (`section_id * STRIDE + rank`) while
/// the counter was `base + shots.len()`, so ranks 0, 2, 3 loaded as ids
/// 100000, 100002, 100003 with the counter on 100003 - and the next shot added
/// took 100003 a second time, so id-keyed edits, deletes and undo hit whichever
/// came first. The app always writes ranks 0..n; only a foreign or hand-edited
/// file has a gap.
#[test]
fn a_gap_in_the_stored_ranks_cannot_make_the_next_shot_duplicate_an_id() {
    let db = saved_project_with_shots(&[0, 3]);
    set_sequence_orders(&db, 1, &[0, 2, 3]);

    let mut loaded = db.load_project().unwrap();
    let loaded_ids = shot_ids(&loaded, 1);
    println!(
        "loaded ids={loaded_ids:?} next_shot_id={}",
        loaded.get_section(1).unwrap().next_shot_id
    );
    let new_id = loaded
        .add_shot(
            1,
            ShotData {
                length: 9.0,
                ..Default::default()
            },
        )
        .unwrap()
        .id;
    let ids = shot_ids(&loaded, 1);
    println!("after add_shot new_id={new_id} ids={ids:?}");

    assert_eq!(
        loaded_ids,
        vec![100_000, 100_001, 100_002],
        "ids follow the stored order, densely"
    );
    assert_eq!(
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        ids.len(),
        "two shots share an id: {ids:?}"
    );
}

/// A18(d): a stored rank outside `[0, STRIDE)` cannot push an id out of the
/// section's window. Rank 250000 used to land in section 3's window and rank -5,
/// cast `as u32`, on `u32::MAX`.
#[test]
fn a_stored_rank_outside_the_window_still_loads_inside_it() {
    let db = saved_project_with_shots(&[0, 3]);
    set_sequence_orders(&db, 1, &[-5, 7, 250_000]);

    let loaded = db.load_project().unwrap();
    let ids = shot_ids(&loaded, 1);
    println!("ranks [-5, 7, 250000] loaded as ids={ids:?}");

    assert_eq!(ids, vec![100_000, 100_001, 100_002]);
}

/// A18(d): a section with more shots than its id window can number is refused
/// with a clear message, like a section id past the encode bound (#557).
///
/// The app caps a section at `FEATURE_ID_STRIDE - 1` shots. A file holding more
/// numbered the extra shots into the NEXT section's window: here section 0's
/// 100001st shot took id 100000, which is section 1's first shot.
#[test]
fn a_section_with_more_shots_than_its_id_window_refuses_to_open() {
    let db = saved_project_with_shots(&[1, 1]);
    db.conn
        .execute(
            &format!(
                "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {stride}) \
                 INSERT INTO shots (section_id, sequence_order, azimuth_in, azimuth_out, length, \
                 depth_in, depth_out) SELECT 0, i, 0.0, 180.0, 1.0, 10.0, 10.0 FROM n",
                stride = crate::models::FEATURE_ID_STRIDE
            ),
            [],
        )
        .unwrap();

    let outcome = db.load_project();
    match &outcome {
        Ok(project) => println!(
            "opened: section 0 holds {} shots, its last id {} - section 1's first id {}",
            project.sections[0].shots.len(),
            project.sections[0].shots.last().unwrap().id,
            project.sections[1].shots[0].id
        ),
        Err(e) => println!("refused: {e}"),
    }

    let error = outcome.err().map(|e| e.to_string()).unwrap_or_default();
    assert!(
        error.contains("cannot be opened") && error.contains("section 0"),
        "the open must be refused and name the section, got: {error:?}"
    );
}

/// The #27 unreadable-kind prompt names each shot by the id the LOADER gives it
/// and by its 1-based position, so a rank gap must not shift either: the
/// frontend addresses the shot by that id, and the user reads the number.
#[test]
fn an_unrecognised_kind_is_reported_under_the_loaded_id_and_position() {
    let db = saved_project_with_shots(&[3]);
    set_sequence_orders(&db, 0, &[0, 2, 3]);
    db.conn
        .execute(
            "UPDATE shots SET kind = 9 WHERE section_id = 0 AND sequence_order = 2",
            [],
        )
        .unwrap();

    let loaded = db.load_project().unwrap();
    let found = db.unrecognised_shot_kinds().unwrap();
    println!(
        "loaded ids={:?}; reported shot_id={:?} shot_number={:?}",
        shot_ids(&loaded, 0),
        found.iter().map(|f| f.shot_id).collect::<Vec<_>>(),
        found.iter().map(|f| f.shot_number).collect::<Vec<_>>()
    );

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].shot_id, loaded.sections[0].shots[1].id);
    assert_eq!(found[0].shot_number, 2, "it is the section's second shot");
}

/// The #27 kind backfill addresses a shot by its position in the loaded
/// section - what `project_load` hands it - so across a rank gap the verdict
/// must still land on that shot's row, not on whichever row holds the same
/// RANK (here: none).
#[test]
fn backfilled_shot_kinds_land_on_the_loaded_position_across_a_rank_gap() {
    let db = saved_project_with_shots(&[3]);
    set_sequence_orders(&db, 0, &[0, 2, 3]);
    let loaded = db.load_project().unwrap();
    let position = loaded.sections[0]
        .shots
        .iter()
        .position(|s| s.id == loaded.sections[0].shots[1].id)
        .unwrap() as u32;

    db.persist_backfilled_shot_kinds(&[(0, position, ShotKind::Reference)])
        .unwrap();

    let reloaded = db.load_project().unwrap();
    let kinds: Vec<ShotKind> = reloaded.sections[0].shots.iter().map(|s| s.kind).collect();
    println!("kinds after backfilling position {position}: {kinds:?}");
    assert_eq!(
        kinds,
        vec![ShotKind::Normal, ShotKind::Reference, ShotKind::Normal]
    );
}

/// The backfill flag is one-shot: once cleared it stays cleared, because
/// classification is import-time only (#27 q14) and must never silently
/// re-flip a kind after the user has edited it.
#[test]
fn pending_kind_classification_flag_clears_once() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("flag.stalx");
    let db = Database::open(&path).unwrap();

    assert!(!db.is_kind_classification_pending());
    db.conn
        .execute_batch(
            "INSERT OR REPLACE INTO project_settings (key, value)
             VALUES ('pending_kind_classification', '1');",
        )
        .unwrap();
    assert!(db.is_kind_classification_pending());

    db.clear_pending_kind_classification().unwrap();
    assert!(!db.is_kind_classification_pending());
    // Idempotent: clearing an already-clear flag is not an error.
    db.clear_pending_kind_classification().unwrap();
    assert!(!db.is_kind_classification_pending());
}
