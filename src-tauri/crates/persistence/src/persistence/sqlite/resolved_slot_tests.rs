//! [#576 FZ-PERSIST-1] An Ignore keyed by POSITION never deletes another shot's
//! uuid-keyed ignore that sits in the same slot.
//!
//! The open file is written with the session's shot ids, which drift from the
//! file's positions: a save re-keys every ignore to its shot's saved position
//! while the session keeps its ids, so after "delete a shot, save, undo the
//! delete" a uuid-keyed ignore of shot B can sit at the slot `(section, id)`
//! that the session gives to the restored shot A. A's uuid is not on disk (the
//! save dropped it), so Ignore on A is keyed by position - and its pre-insert
//! dedup deleted every row in that slot, B's included: B's warning came back,
//! with nothing said. Found by the round-trip fuzzer (seed 3526).
//!
//! The dedup now clears only position-keyed rows there (as the uuid branch
//! already did), so the guarded upsert sees B's row and refuses the insert
//! instead, keeping B's ignore - the outcome `save_resolved_warning` documents
//! for a slot held by a different shot.

use std::collections::HashMap;

use chrono::Utc;

use super::{Database, ResolvedWarning};
use crate::models::{Project, SectionData};

fn ignore(shot_uuid: Option<&str>, comment: &str) -> ResolvedWarning {
    ResolvedWarning {
        section_id: 0,
        shot_id: 1,
        warning_code: "AZIMUTH_DEVIATION".to_owned(),
        resolved_at: Utc::now(),
        resolved_comment: Some(comment.to_owned()),
        shot_uuid: shot_uuid.map(str::to_owned),
    }
}

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

#[test]
fn a_position_keyed_ignore_never_deletes_another_shots_ignore_in_its_slot() {
    let db = database_with_one_section();
    // Shot B's ignore, keyed by B's uuid, at slot (0, 1).
    db.save_resolved_warning(&ignore(Some("uuid-b"), "B's"))
        .unwrap();

    // Ignore on shot A, whose uuid is not on disk, at the same slot.
    let refusal = db
        .save_resolved_warning(&ignore(None, "A's"))
        .err()
        .map(|e| e.to_string());

    let rows = db.load_resolved_warnings().unwrap();
    assert!(
        rows.iter().any(|r| r.shot_uuid.as_deref() == Some("uuid-b")
            && r.resolved_comment.as_deref() == Some("B's")),
        "shot B's ignore was deleted by an ignore on another shot: {rows:?}"
    );
    assert!(
        refusal.is_some(),
        "the slot is held by another shot's ignore, so this one is refused, not recorded \
         over it: {rows:?}"
    );
    // [#576 FZ-PERSIST-1-D1] A save is what frees the slot (it writes A's uuid and
    // re-keys the ignores), so that is what the refusal asks for - not a reload.
    assert!(
        refusal
            .as_deref()
            .is_some_and(|m| m.ends_with("save the project and try again")),
        "the refusal says what to do: {refusal:?}"
    );
}

#[test]
fn a_position_keyed_ignore_still_replaces_the_position_keyed_one_in_its_slot() {
    let db = database_with_one_section();
    db.save_resolved_warning(&ignore(None, "first")).unwrap();
    db.save_resolved_warning(&ignore(None, "second")).unwrap();

    let rows = db.load_resolved_warnings().unwrap();
    assert_eq!(rows.len(), 1, "one ignore per slot: {rows:?}");
    assert_eq!(
        rows.first().and_then(|r| r.resolved_comment.as_deref()),
        Some("second")
    );
}

/// [#576 FZ-PERSIST-2] The re-key a section reverse uses to keep position-keyed
/// ignores on their shots: moves swap slots, leave a uuid-keyed row whose uuid
/// names no shot of the section (an ignore of a deleted shot) where it is, and
/// are all-or-nothing.
#[test]
fn position_keyed_ignores_move_with_their_shots_all_or_nothing() {
    let db = database_with_one_section();
    let at = |shot_id: u32, code: &str, shot_uuid: Option<&str>, comment: &str| ResolvedWarning {
        shot_id,
        warning_code: code.to_owned(),
        ..ignore(shot_uuid, comment)
    };
    let slots = |db: &Database| {
        let mut rows: Vec<(u32, String, String)> = db
            .load_resolved_warnings()
            .unwrap()
            .into_iter()
            .map(|r| {
                (
                    r.shot_id,
                    r.warning_code,
                    r.resolved_comment.unwrap_or_default(),
                )
            })
            .collect();
        rows.sort();
        rows
    };
    let row =
        |shot_id: u32, code: &str, comment: &str| (shot_id, code.to_owned(), comment.to_owned());
    db.save_resolved_warning(&at(1, "A", None, "shot one"))
        .unwrap();
    db.save_resolved_warning(&at(2, "A", None, "shot two"))
        .unwrap();
    db.save_resolved_warning(&at(1, "B", Some("uuid-kept"), "kept"))
        .unwrap();

    // The shots of the section: none carries "uuid-kept".
    let shots = HashMap::from([
        ((0, 1), "uuid-1".to_owned()),
        ((0, 2), "uuid-2".to_owned()),
        ((0, 3), "uuid-3".to_owned()),
    ]);

    // A swap of slots 1 and 2: the position-keyed rows trade places, the
    // uuid-keyed one stays.
    let moved = db
        .rekey_position_resolved_warnings(&[(0, 1, 2), (0, 2, 1)], &shots)
        .unwrap();
    assert_eq!(moved, 2);
    assert_eq!(
        slots(&db),
        vec![
            row(1, "A", "shot two"),
            row(1, "B", "kept"),
            row(2, "A", "shot one")
        ],
        "a swap trades the position-keyed rows and leaves the uuid-keyed one"
    );

    // A move onto a slot whose ignore with the same code stays put fails whole.
    db.save_resolved_warning(&at(3, "B", None, "shot three"))
        .unwrap();
    let before = slots(&db);
    assert!(db
        .rekey_position_resolved_warnings(&[(0, 3, 1)], &shots)
        .is_err());
    assert_eq!(
        slots(&db),
        before,
        "a move that cannot be made moves nothing"
    );
}

/// [#576 FZ-PERSIST-2] A uuid-keyed row of a reversed section moves to the id of
/// the shot its uuid names. The mirrored shot's ignore with the same code is
/// then out of the way of the position-keyed one: left on its slot, it failed
/// the whole move, and the caller pinned the position-keyed ignore onto the
/// mirrored shot instead.
#[test]
fn uuid_keyed_ignores_move_with_their_shots_out_of_the_way() {
    let db = database_with_one_section();
    let at = |shot_id: u32, shot_uuid: Option<&str>, comment: &str| ResolvedWarning {
        shot_id,
        warning_code: "MISSING_LRUD".to_owned(),
        ..ignore(shot_uuid, comment)
    };
    let slots = |db: &Database| {
        let mut rows: Vec<(u32, Option<String>, String)> = db
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
    };
    let row = |shot_id: u32, shot_uuid: Option<&str>, comment: &str| {
        (shot_id, shot_uuid.map(str::to_owned), comment.to_owned())
    };
    // Saved shot S (id 1) and a shot N not saved yet (id 2), both ignored for
    // one code: S's by its uuid, N's by position.
    db.save_resolved_warning(&at(1, Some("uuid-s"), "S's"))
        .unwrap();
    db.save_resolved_warning(&at(2, None, "N's")).unwrap();
    // The reverse swaps them: N is at id 1 now, S at id 2.
    let reversed = HashMap::from([((0, 1), "uuid-n".to_owned()), ((0, 2), "uuid-s".to_owned())]);
    let moved = db
        .rekey_position_resolved_warnings(&[(0, 1, 2), (0, 2, 1)], &reversed)
        .unwrap();
    assert_eq!(moved, 2);
    assert_eq!(
        slots(&db),
        vec![row(1, None, "N's"), row(2, Some("uuid-s"), "S's")],
        "both ignores follow their shots"
    );
    // And back.
    let back = HashMap::from([((0, 1), "uuid-s".to_owned()), ((0, 2), "uuid-n".to_owned())]);
    db.rekey_position_resolved_warnings(&[(0, 1, 2), (0, 2, 1)], &back)
        .unwrap();
    assert_eq!(
        slots(&db),
        vec![row(1, Some("uuid-s"), "S's"), row(2, None, "N's")]
    );

    // A second row of S for that code on another slot - what the old fallback
    // left: N's ignore pinned onto S's uuid. One of S's rows takes S's id, the
    // other stays on its own slot; neither fails the move.
    db.save_resolved_warning(&at(3, None, "pinned")).unwrap();
    db.backfill_resolved_warning_uuids(&HashMap::from([((0, 3), "uuid-s".to_owned())]))
        .unwrap();
    let moved = db
        .rekey_position_resolved_warnings(&[(0, 1, 2), (0, 2, 1)], &reversed)
        .unwrap();
    assert_eq!(moved, 2, "N's row and one of S's land on their shots' ids");
    let keys: Vec<(u32, Option<String>)> = slots(&db)
        .into_iter()
        .map(|(id, uuid, _)| (id, uuid))
        .collect();
    assert_eq!(
        keys,
        vec![
            (1, None),
            (2, Some("uuid-s".to_owned())),
            (3, Some("uuid-s".to_owned()))
        ]
    );
}

/// [#576 W27] (fault sweep seed 3121675) A reverse moves a position-keyed ignore
/// onto the slot a uuid-keyed ignore of the same code is leaving. The uuid-keyed
/// row used to stay in that slot, so the move failed whole, the position-keyed
/// row stayed at an id that now names the other shot, and the next save gave it
/// that shot's uuid - replacing that shot's own ignore and losing its own.
#[test]
fn a_reverse_moves_a_position_keyed_ignore_onto_the_slot_a_uuid_keyed_one_leaves() {
    let db = database_with_one_section();
    let at = |shot_id: u32, shot_uuid: Option<&str>, comment: &str| ResolvedWarning {
        shot_id,
        ..ignore(shot_uuid, comment)
    };
    // Shot B (on disk) holds id 1, shot A (not saved yet) id 3.
    db.save_resolved_warning(&at(1, Some("uuid-b"), "B's"))
        .unwrap();
    db.save_resolved_warning(&at(3, None, "A's")).unwrap();

    // The reverse puts A at id 1 and B at id 3.
    let after = HashMap::from([((0, 1), "uuid-a".to_owned()), ((0, 3), "uuid-b".to_owned())]);
    let moved = db.rekey_position_resolved_warnings(&[(0, 1, 3), (0, 3, 1)], &after);

    let mut rows: Vec<(u32, Option<String>, String)> = db
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
    assert!(moved.is_ok(), "the move failed: {moved:?}, rows {rows:?}");
    assert_eq!(
        rows,
        vec![
            (1, None, "A's".to_owned()),
            (3, Some("uuid-b".to_owned()), "B's".to_owned())
        ],
        "each ignore is at the id its shot holds after the reverse"
    );
}
