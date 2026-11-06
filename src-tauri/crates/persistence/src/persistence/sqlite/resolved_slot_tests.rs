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
/// ignores on their shots: moves swap slots, leave uuid-keyed rows alone, and
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

    // A swap of slots 1 and 2: the position-keyed rows trade places, the
    // uuid-keyed one stays.
    let moved = db
        .rekey_position_resolved_warnings(&[(0, 1, 2), (0, 2, 1)])
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
    assert!(db.rekey_position_resolved_warnings(&[(0, 3, 1)]).is_err());
    assert_eq!(
        slots(&db),
        before,
        "a move that cannot be made moves nothing"
    );
}
