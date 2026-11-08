//! [#576 W30-6, Q-W30-3-D1] A station comment row past the end of its section
//! (an orphan) never reaches a station.
//!
//! The open's re-anchor (`Project::reanchor_station_refs`) joins the keys that
//! resolve, but a key that does not resolve - a row past the end of its
//! section - used to stay the positional `Index` key it was read as. An edit
//! that gave the section that station (a shot added or prepended) brought the
//! key back, now naming a station that had, or later got, a key of its own:
//! the orphan's text landed on a station it was never written for, and the
//! save joined it into that station's row. The open now drops such a row and
//! lists it on its receipt (`load_repairs.station_comments`), so no later edit
//! can bring it back.
//!
//! The commands that still meet a legacy `Index` key - it can come back from
//! undo history a build before Q-W30-3-D1 persisted - are tested with that key
//! built in memory ([`seed_legacy_comment_rows`]).
//!
//! All data is synthetic: a file this build saved, then rows added with plain
//! SQL. No fixture is read or written.

use std::path::{Path, PathBuf};

use rusqlite::OptionalExtension;
use tempfile::TempDir;

use super::AppState;
use crate::core::Command;
use crate::models::{Project, SectionData, ShotData, ShotId, StationKey, StationRef};
use crate::persistence::Database;
use crate::test_fixtures::state_with_null_sink;

/// A file this build saved - one section, two shots, stations 0 to 2 - with
/// `rows` (`(station_index, comment)`) then written into its `station_comments`
/// table, as a file from another build can hold them. Returns the path and the
/// section's id.
pub(super) fn file_with_comment_rows(dir: &Path, rows: &[(u32, &str)]) -> (PathBuf, u32) {
    let mut project = Project::new("CaveA".to_string(), None);
    project.add_section(SectionData {
        name: Some("SectionA".to_string()),
        color: "#123456".to_string(),
        shots: vec![
            ShotData {
                length: 5.0,
                ..Default::default()
            },
            ShotData {
                length: 7.0,
                ..Default::default()
            },
        ],
        ..Default::default()
    });
    project.ensure_shot_uuids();
    let path = dir.join("CaveA.stalx");
    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    drop(db);
    let conn = rusqlite::Connection::open(&path).unwrap();
    let sid: u32 = conn
        .query_row("SELECT id FROM sections LIMIT 1", [], |r| r.get(0))
        .unwrap();
    for (index, comment) in rows {
        conn.execute(
            "INSERT INTO station_comments (section_id, station_index, comment) VALUES (?1, ?2, ?3)",
            rusqlite::params![sid, index, comment],
        )
        .unwrap();
    }
    (path, sid)
}

/// [#576 Q-W30-3-D1] Put `rows` (`(station_index, comment)`) under positional
/// `Index` keys of section `sid` in the open project, as an open before
/// Q-W30-3-D1 left a row past the end of its section. An open now drops such a
/// row, so the tests of the commands that still meet a legacy key (from undo
/// history an older build persisted) build it here, right after the open.
pub(super) fn seed_legacy_comment_rows(state: &AppState, sid: u32, rows: &[(u32, &str)]) {
    let mut guard = state.project.lock();
    let project = guard.as_mut().unwrap();
    for (index, comment) in rows {
        project
            .station_comments
            .insert(StationKey::positional(sid, *index), (*comment).to_string());
    }
}

/// The comment the open project shows on `sr` (the `comment` the station
/// payload carries to the map and the station panel). `None` with no project
/// open too: each test also asserts the save wrote a comment, so that cannot
/// pass as an agreement.
pub(super) fn shown_comment(state: &AppState, sr: StationRef) -> Option<String> {
    state
        .get_project_state()?
        .compute_station_coordinates()
        .into_iter()
        .find(|s| s.station_ref == sr)
        .and_then(|s| s.comment)
}

/// The comment the file at `path` holds for `sr`.
pub(super) fn written_comment(path: &Path, sr: StationRef) -> Option<String> {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.query_row(
        "SELECT comment FROM station_comments WHERE section_id = ?1 AND station_index = ?2",
        rusqlite::params![sr.section_id, sr.station_index],
        |r| r.get(0),
    )
    .optional()
    .unwrap()
}

pub(super) fn shot_ids(state: &AppState) -> Vec<ShotId> {
    let project = state.get_project_state().unwrap();
    project
        .sections
        .first()
        .unwrap()
        .shots
        .iter()
        .map(|s| s.id)
        .collect()
}

/// Save the open project to `path`, then open it again in a new state: what the
/// save wrote for `sr`, and what the reopen shows there.
pub(super) fn save_and_reopen(
    state: &AppState,
    path: &Path,
    sr: StationRef,
) -> (Option<String>, Option<String>) {
    let saved = state.save_project(Some(path.to_path_buf()));
    assert!(saved.success, "the save goes through: {:?}", saved.error);
    let written = written_comment(path, sr);
    let reopened = state_with_null_sink()
        .do_load_project_full(path.to_path_buf())
        .unwrap()
        .stations
        .into_iter()
        .find(|s| s.station_ref == sr)
        .and_then(|s| s.comment);
    (written, reopened)
}

/// [#576 Q-W30-3-D1] The file holds a comment on station 2 (the end of the
/// line) and one on station 3, past the end. The open drops the orphan, lists
/// it on its receipt, and does not refuse the save over it. A shot prepended to
/// the section moves the end station to index 3, where the orphan's key used
/// to resolve: the end station shows, the save writes and the reopen shows its
/// own comment alone. (The orphan used to land there, joined into the row.)
#[test]
fn an_orphan_comment_is_dropped_on_open_and_a_prepend_never_brings_it_back() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let (path, sid) = file_with_comment_rows(
        temp.path(),
        &[(2, "comment at the end"), (3, "comment past the end")],
    );
    let state = state_with_null_sink();
    state.do_load_project_full(path).unwrap();
    assert!(
        !state.integrity_check().save_blocked,
        "the orphan is gone at the open, so nothing blocks the save"
    );

    let first = *shot_ids(&state).first().unwrap();
    let prepended = state.execute(Command::CreateShot {
        section_id: sid,
        shot: ShotData {
            length: 3.0,
            ..Default::default()
        },
        after_shot_id: None,
        before_shot_id: Some(first),
    });
    assert!(prepended.success, "prepend: {:?}", prepended.error);

    let end = StationRef::new(sid, 3);
    let shown = shown_comment(&state, end);
    let (written, reopened) = save_and_reopen(&state, &temp.path().join("saved.stalx"), end);
    println!("station {end:?} shows:   {shown:?}");
    println!("the save writes:        {written:?}");
    println!("the reopen shows:       {reopened:?}");
    assert_eq!(
        written.as_deref(),
        Some("comment at the end"),
        "the end station keeps its own comment, and the orphan is not joined in"
    );
    assert_eq!(
        shown, written,
        "the station shows what the save writes for it"
    );
    assert_eq!(reopened, written, "and what the reopen shows");
}

/// [#576 Q-W30-3-D1] The file holds one comment past the end of the section. A
/// shot appended to the section gives it a station at that position: the new
/// station shows no comment, the save writes none there and the reopen shows
/// none. (The orphan used to land on the appended station.)
#[test]
fn an_appended_station_never_gets_an_orphan_comment() {
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let (path, sid) = file_with_comment_rows(temp.path(), &[(3, "comment past the end")]);
    let state = state_with_null_sink();
    state.do_load_project_full(path).unwrap();

    let last = *shot_ids(&state).last().unwrap();
    let appended = state.execute(Command::CreateShot {
        section_id: sid,
        shot: ShotData {
            length: 3.0,
            ..Default::default()
        },
        after_shot_id: Some(last),
        before_shot_id: None,
    });
    assert!(appended.success, "append: {:?}", appended.error);
    let station = StationRef::new(sid, 3);

    let shown = shown_comment(&state, station);
    let (written, reopened) = save_and_reopen(&state, &temp.path().join("saved.stalx"), station);
    println!("station {station:?} shows:   {shown:?}");
    println!("the save writes:        {written:?}");
    println!("the reopen shows:       {reopened:?}");
    assert_eq!(shown, None, "the appended station shows no comment");
    assert_eq!(written, None, "the save writes no comment for it");
    assert_eq!(reopened, None, "and the reopen shows none");
}
