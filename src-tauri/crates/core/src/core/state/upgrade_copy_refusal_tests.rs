//! [#576 B6-D2-FAIL] Opening an older project whose pre-upgrade copy cannot be
//! written is refused, and the user is told why.
//!
//! `ensure_schema_version` keeps one copy of the file before it upgrades it
//! (B6-D2). When that copy cannot be written, the decided behaviour is to refuse
//! the open and leave the file untouched, not to upgrade without a way back.
//! This pins what the load path hands the frontend for that refusal: the
//! `PERMISSION` code (the folder refused the copy), whose dialog shows the
//! message, and the message names the copy.
//!
//! All data is synthetic: a project built in memory, saved by this build, then
//! re-stamped as an older schema. No fixture is read or written.

use std::path::Path;

use tempfile::TempDir;

use crate::models::{Project, SectionData, ShotData};
use crate::persistence::Database;
use crate::test_fixtures::state_with_null_sink;

fn user_version(path: &Path) -> i64 {
    rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap()
}

#[cfg(unix)]
#[test]
fn an_older_project_whose_copy_cannot_be_written_is_refused_with_the_reason() {
    use std::os::unix::fs::PermissionsExt as _;

    let temp = TempDir::new().unwrap();
    let folder = temp.path().join("CaveA");
    std::fs::create_dir(&folder).unwrap();
    let path = folder.join("CaveA.stalx");
    let mut project = Project::new("CaveA".to_string(), None);
    project.add_section(SectionData {
        name: Some("SectionA".to_string()),
        color: "#123456".to_string(),
        shots: vec![ShotData {
            length: 5.0,
            ..Default::default()
        }],
        ..Default::default()
    });
    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    drop(db);
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("UPDATE project SET schema_version = 17; PRAGMA user_version = 17;")
        .unwrap();
    let original = std::fs::read(&path).unwrap();

    let state = state_with_null_sink();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
    let premise = std::fs::File::create(folder.join("premise-probe"));
    let loaded = premise
        .is_err()
        .then(|| state.do_load_project_full(path.clone()).err());
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        premise.is_err(),
        "premise: a read-only folder refuses new files (run the tests as a non-root user)"
    );

    let error = loaded.unwrap().unwrap();
    println!(
        "load -> code {}: {} (details {:?})",
        error.code, error.message, error.details
    );
    assert_eq!(
        error.code,
        crate::models::CommandErrorCode::FsPermissionDenied,
        "{error:?}"
    );
    assert!(
        error
            .message
            .contains("Could not keep a copy of this project before upgrading it"),
        "the dialog shows this message, so it must name the copy: {error:?}"
    );
    assert_eq!(
        error.details.as_deref(),
        Some(path.display().to_string().as_str())
    );
    let unchanged = std::fs::read(&path).unwrap() == original;
    println!(
        "file unchanged: {unchanged}; user_version: {}",
        user_version(&path)
    );
    assert!(
        unchanged,
        "a refused upgrade must leave the file byte-identical"
    );
    assert_eq!(user_version(&path), 17);
}
