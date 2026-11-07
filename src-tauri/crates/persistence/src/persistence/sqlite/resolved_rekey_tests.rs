//! [#576 RVF-d-3] A section reverse moves the position-keyed ignores to their
//! shots' new ids, and nothing in their way fails the move: not the ignore of a
//! deleted shot on a landing slot, and not a live shot's ignore that can land
//! neither on its shot's id nor back on its own slot. Synthetic rows only.

use std::collections::{HashMap, HashSet};

use chrono::Utc;

use super::{Database, ResolvedWarning};
use crate::models::{Project, SectionData, FEATURE_ID_STRIDE};

const CODE: &str = "MISSING_LRUD";

fn database_with_one_section() -> Database {
    let mut db = Database::in_memory().unwrap();
    let mut project = Project::new("CaveA".to_owned(), None);
    project.add_section(SectionData {
        color: "#000".to_owned(),
        ..Default::default()
    });
    db.save_project(&project).unwrap();
    db
}

fn ignore_at(db: &Database, shot_id: u32, shot_uuid: Option<&str>, comment: &str) {
    db.save_resolved_warning(&ResolvedWarning {
        section_id: 0,
        shot_id,
        warning_code: CODE.to_owned(),
        resolved_at: Utc::now(),
        resolved_comment: Some(comment.to_owned()),
        shot_uuid: shot_uuid.map(str::to_owned),
    })
    .unwrap();
}

/// `(slot, uuid, comment)` of every row, sorted.
fn slots(db: &Database) -> Vec<(u32, Option<String>, String)> {
    let mut rows: Vec<_> = db
        .load_resolved_warnings()
        .unwrap()
        .into_iter()
        .map(|r| {
            (
                r.shot_id,
                r.shot_uuid,
                r.resolved_comment.unwrap_or_default(),
            )
        })
        .collect();
    rows.sort();
    rows
}

fn row(slot: u32, uuid: Option<&str>, comment: &str) -> (u32, Option<String>, String) {
    (slot, uuid.map(str::to_owned), comment.to_owned())
}

/// The highest id of section 0's window, where an ignore in the way goes.
const TOP: u32 = FEATURE_ID_STRIDE - 1;

/// Shot N (not saved, id 3) is ignored by position; the deleted shot B's
/// ignore with the same code sits on id 1, where a save re-keyed it while the
/// session kept A on id 1. The reverse swaps N and A: N's row must land on 1.
/// B's row moves out of the way, to a slot no shot holds and never a negative
/// one, and stays there for an undo of the delete.
#[test]
fn a_deleted_shots_ignore_moves_out_of_the_way_of_a_landing_one() {
    let db = database_with_one_section();
    ignore_at(&db, 1, Some("uuid-b"), "B's");
    ignore_at(&db, 3, None, "N's");
    let reversed = HashMap::from([((0, 1), "uuid-n".to_owned()), ((0, 3), "uuid-a".to_owned())]);

    let moved =
        db.rekey_position_resolved_warnings(&[(0, 1, 3), (0, 3, 1)], &reversed, &HashSet::new());
    println!("rekey -> {moved:?}; rows {:?}", slots(&db));
    assert_eq!(moved.unwrap(), 1, "N's row lands on N's id");
    assert_eq!(
        slots(&db),
        vec![row(1, None, "N's"), row(TOP, Some("uuid-b"), "B's")]
    );

    // And back: N's row follows N again, B's stays out of the way.
    let back = HashMap::from([((0, 1), "uuid-a".to_owned()), ((0, 3), "uuid-n".to_owned())]);
    db.rekey_position_resolved_warnings(&[(0, 1, 3), (0, 3, 1)], &back, &HashSet::new())
        .unwrap();
    assert_eq!(
        slots(&db),
        vec![row(3, None, "N's"), row(TOP, Some("uuid-b"), "B's")]
    );
}

/// A live shot's ignore whose own slot a position-keyed row takes, and whose
/// shot's id holds a row that does not move (the deleted B's, same code), lands
/// on a free slot instead of failing the whole move. Ids 1, 2 and 3: N (not
/// saved) moves from 3 to 1, S sits on 2, B's ignore sits on 2, S's on 1.
#[test]
fn a_live_shots_ignore_with_nowhere_to_land_takes_a_free_slot() {
    let db = database_with_one_section();
    ignore_at(&db, 1, Some("uuid-s"), "S's");
    ignore_at(&db, 2, Some("uuid-b"), "B's");
    ignore_at(&db, 3, None, "N's");
    let reversed = HashMap::from([
        ((0, 1), "uuid-n".to_owned()),
        ((0, 2), "uuid-s".to_owned()),
        ((0, 3), "uuid-a".to_owned()),
    ]);

    let moved =
        db.rekey_position_resolved_warnings(&[(0, 1, 3), (0, 3, 1)], &reversed, &HashSet::new());
    println!("rekey -> {moved:?}; rows {:?}", slots(&db));
    assert_eq!(moved.unwrap(), 1, "only N's row lands on its shot's id");
    assert_eq!(
        slots(&db),
        vec![
            row(1, None, "N's"),
            row(2, Some("uuid-b"), "B's"),
            row(TOP, Some("uuid-s"), "S's")
        ]
    );
}

/// Two rows with one code in the way in one section take two free slots, from
/// the top down, skipping a slot a shot of the section holds. Ids 1 to 4, all
/// shots but N and M saved: N (id 4) and M (id 3) are ignored by position and
/// the reverse moves them to 1 and 2, where the deleted B's and C's ignores sit.
#[test]
fn free_slots_skip_the_ids_shots_hold() {
    let db = database_with_one_section();
    ignore_at(&db, 1, Some("uuid-b"), "B's");
    ignore_at(&db, 2, Some("uuid-c"), "C's");
    ignore_at(&db, 3, None, "M's");
    ignore_at(&db, 4, None, "N's");
    // A shot of the section holds the top id.
    let reversed = HashMap::from([
        ((0, 1), "uuid-n".to_owned()),
        ((0, 2), "uuid-m".to_owned()),
        ((0, 3), "uuid-e".to_owned()),
        ((0, 4), "uuid-d".to_owned()),
        ((0, TOP), "uuid-t".to_owned()),
    ]);
    let moves = [(0, 1, 4), (0, 2, 3), (0, 3, 2), (0, 4, 1)];
    let moved = db.rekey_position_resolved_warnings(&moves, &reversed, &HashSet::new());
    println!("rekey -> {moved:?}; rows {:?}", slots(&db));
    assert_eq!(moved.unwrap(), 2);
    let mut rows = slots(&db);
    let parked: Vec<u32> = rows.drain(2..).map(|(slot, ..)| slot).collect();
    assert_eq!(rows, vec![row(1, None, "N's"), row(2, None, "M's")]);
    let mut parked_sorted = parked.clone();
    parked_sorted.sort_unstable();
    assert_eq!(
        parked_sorted,
        vec![TOP - 2, TOP - 1],
        "below the held top id: {parked:?}"
    );
}

/// [#576 PR #594 review] A free slot is never the id of a shot of the section,
/// with a uuid or without one. The shots N (id 3, ignored by position), T (the
/// top id, no uuid) and A (id 1) are reversed: N and A swap, T stays in the
/// middle. The deleted B's ignore on id 1 moves out of the way of N's, to the
/// slot below T's id: T is in no uuid map, so its id comes in `shot_ids`.
#[test]
fn a_free_slot_is_never_the_id_of_a_shot_without_a_uuid() {
    let db = database_with_one_section();
    ignore_at(&db, 1, Some("uuid-b"), "B's");
    ignore_at(&db, 3, None, "N's");
    let reversed = HashMap::from([((0, 1), "uuid-n".to_owned()), ((0, 3), "uuid-a".to_owned())]);
    let shot_ids = HashSet::from([(0, 1), (0, TOP), (0, 3)]);

    let moved = db.rekey_position_resolved_warnings(&[(0, 1, 3), (0, 3, 1)], &reversed, &shot_ids);
    println!("rekey -> {moved:?}; rows {:?}", slots(&db));
    assert_eq!(moved.unwrap(), 1, "N's row lands on N's id");
    assert_eq!(
        slots(&db),
        vec![row(1, None, "N's"), row(TOP - 1, Some("uuid-b"), "B's")],
        "B's row is not on T's id ({TOP})"
    );
}
