//! [#517] A save must not turn "I could not read the ignore list" into "there
//! are no ignores", and write-protected media must not cost the user their
//! work.
//!
//! In 0.6.0 every save re-read `resolved_warnings` off the live SQLite handle,
//! so a table the current SELECT could not name (a forged stamp over pre-v8
//! tables), hostile cell values, or a read that failed outright decided what
//! the saved file held. The station-graph core reads a legacy file once,
//! through the frozen 0.6.0 open (with #517's lenient reader) and the
//! conversion, and the ignores are in memory from then on: no save reads them
//! back from disk. What #517 pinned is pinned here on that route:
//!
//! * a forged current stamp over pre-v8 tables keeps the ignore;
//! * hostile values never make a legacy project unopenable or unsaveable (a
//!   coerced row is carried inert, as 0.6.0 carried it);
//! * the #516 route: a write-protected legacy file opens, and Save As to a
//!   writable place carries its ignores, keyed by uuid;
//! * on write-protected media a save is refused and changes nothing, a
//!   local-only durable save (a display toggle) keeps the change in memory, and
//!   Save As elsewhere writes all of it.
//!
//! The display half of the forged-stamp case (the warning shown as ignored) is
//! the warnings package's (`compute_project_warnings`).
//!
//! All data here is synthetic: a project built in memory, saved by this build,
//! then hand-narrowed. No fixture is read or written.

use std::path::Path;

use tempfile::TempDir;

use super::AppState;
use crate::core::Command;
use crate::models::{LineData, Project, ShotData};
use crate::persistence::stalx_v2::{self, Extras, IgnoredWarning, Mode};
use crate::persistence::Database;

const IGNORED_CODE: &str = "SHORT_SHOT";
const RESOLVED_AT: &str = "2026-01-01T00:00:00+00:00";

/// The pre-v8 `resolved_warnings` shape: every current column except
/// `shot_uuid`.
const PRE_V8_COLUMNS: &str = "section_id INTEGER NOT NULL, \
     shot_id INTEGER NOT NULL, \
     warning_code TEXT NOT NULL, \
     resolved_at TEXT NOT NULL, \
     resolved_comment TEXT, \
     UNIQUE(section_id, shot_id, warning_code)";

/// The hostile-values shape: `shot_id` and `warning_code` are declared with NO
/// type, so the cells keep whatever the file put in them (a declared INTEGER
/// column would convert a numeric-looking TEXT on insert).
const UNTYPED_ID_COLUMNS: &str = "section_id INTEGER NOT NULL, \
     shot_id, \
     warning_code, \
     resolved_at, \
     resolved_comment";

/// What the seeded file holds, as this build wrote it.
struct Seeded {
    line_id: u32,
    shot_id: u32,
    shot_uuid: String,
}

/// Two short shots that the v12 pitch-flip migration cannot vote on (no
/// pitch, no depth change); the first is the one the fixtures ignore.
fn synthetic_project() -> Project {
    let unflippable = |length: f64| ShotData {
        length,
        pitch_in: None,
        pitch_out: None,
        depth_in: 10.0,
        depth_out: 10.0,
        ..Default::default()
    };
    let mut project = Project::new("Forged".to_string(), None);
    project.add_line(LineData {
        color: "#abcdef".to_string(),
        shots: vec![unflippable(0.1), unflippable(0.2)],
        ..Default::default()
    });
    project
}

/// Write a legacy `.stalx` this build's SQLite writer considers current.
fn seed_current_file(path: &Path) -> Seeded {
    let mut db = Database::open(path).unwrap();
    db.save_project(&synthetic_project()).unwrap();
    let saved = db.load_project().unwrap();
    Seeded {
        line_id: saved.lines[0].id,
        shot_id: saved.lines[0].shots[0].id,
        shot_uuid: saved.lines[0].shots[0].uuid.clone().unwrap(),
    }
}

/// Replace `resolved_warnings` with `ddl` holding `values_rows`, WITHOUT
/// touching the stamp the writer wrote: the file claims the whole migration
/// chain ran while the table underneath is whatever `ddl` says.
fn forge_ignore_table(path: &Path, ddl: &str, columns: &str, values_rows: &[String]) {
    let conn = rusqlite::Connection::open(path).unwrap();
    let stamped: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    let inserts = values_rows
        .iter()
        .map(|v| format!("INSERT INTO resolved_warnings ({columns}) VALUES ({v});"))
        .collect::<Vec<_>>()
        .join("\n");
    conn.execute_batch(&format!(
        "DROP TABLE resolved_warnings;
         CREATE TABLE resolved_warnings (
            id INTEGER PRIMARY KEY,
            {ddl},
            FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
         );
         {inserts}"
    ))
    .unwrap();
    assert!(stamped >= 8, "the stamp must be past the v8 migration");
    let has_uuid_column: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('resolved_warnings') WHERE name='shot_uuid'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(has_uuid_column, 0, "the forged table must be pre-v8");
}

fn open(path: &Path) -> AppState {
    let state = AppState::new().unwrap();
    let opened = state.execute(Command::OpenProject {
        path: path.to_path_buf(),
    });
    assert!(opened.success, "open failed: {:?}", opened.error);
    state
}

fn ignores_in_memory(state: &AppState) -> Vec<IgnoredWarning> {
    state.with_file(|file| file.ignored_warnings().to_vec())
}

fn ignores_on_disk(path: &Path) -> Vec<IgnoredWarning> {
    stalx_v2::decode(&std::fs::read(path).unwrap(), &Default::default())
        .unwrap()
        .extras
        .ignored_warnings
}

/// A file name no other test shares (the upgrade backups folder is shared).
fn unique(dir: &Path, what: &str) -> std::path::PathBuf {
    dir.join(format!("{what}-{}.stalx", uuid::Uuid::new_v4()))
}

fn forget_backups_of(path: &Path) {
    let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
    if let Some(folder) = super::project_io::upgrade_backup_dir() {
        for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
            if entry.file_name().to_string_lossy().starts_with(&stem) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// Make `path` read-only, or say why it cannot be (root ignores mode bits,
/// and CI's check job runs as root): `false` means skip, not pass.
#[cfg(unix)]
fn make_read_only(path: &Path, mode: u32, test: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    let probe = if path.is_dir() {
        let probe = path.join(".write-probe");
        let wrote = std::fs::write(&probe, b"x").is_ok();
        let _ = std::fs::remove_file(probe);
        wrote
    } else {
        std::fs::OpenOptions::new().write(true).open(path).is_ok()
    };
    if probe {
        let writable = if path.is_dir() { 0o755 } else { 0o644 };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(writable)).unwrap();
        eprintln!(
            "skipping {test}: this user can write a read-only path (root?), so there is no \
             write-protected medium to test"
        );
        return false;
    }
    true
}

/// THE forged-stamp regression guard: the ignore is read on open and written
/// by the save that upgrades the file, naming the same shot.
#[test]
fn a_forged_current_stamp_over_pre_v8_tables_does_not_erase_the_ignore_list() {
    crate::app_dirs::init_for_test();
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let path = unique(temp.path(), "forged");
    let seeded = seed_current_file(&path);
    forge_ignore_table(
        &path,
        PRE_V8_COLUMNS,
        "section_id, shot_id, warning_code, resolved_at",
        &[format!(
            "{}, {}, '{IGNORED_CODE}', '{RESOLVED_AT}'",
            seeded.line_id, seeded.shot_id
        )],
    );

    let state = open(&path);
    let live = ignores_in_memory(&state);
    assert_eq!(live.len(), 1, "the open must keep the ignore");
    assert_eq!(live[0].shot, seeded.shot_uuid);
    assert_eq!(live[0].code, IGNORED_CODE);

    let saved = state.execute(Command::SaveProject { path: None });
    forget_backups_of(&path);
    assert!(saved.success, "save failed: {:?}", saved.error);
    assert_eq!(ignores_on_disk(&path), live);
}

/// SQLite columns are untyped, so a hand-written file can put a BLOB in any of
/// them. One poisoned row must not make the project unopenable or unsaveable:
/// the well-formed row is carried, and so is a row the reader could key to a
/// shot (its coerced code inert); one with no readable shot is not.
#[test]
fn hostile_ignore_values_never_make_a_legacy_project_unsaveable() {
    crate::app_dirs::init_for_test();
    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let path = unique(temp.path(), "hostile");
    let seeded = seed_current_file(&path);
    forge_ignore_table(
        &path,
        UNTYPED_ID_COLUMNS,
        "section_id, shot_id, warning_code, resolved_at, resolved_comment",
        &[
            format!(
                "{}, {}, '{IGNORED_CODE}', '{RESOLVED_AT}', NULL",
                seeded.line_id, seeded.shot_id
            ),
            format!(
                "{}, X'DEADBEEF', '{IGNORED_CODE}', '{RESOLVED_AT}', NULL",
                seeded.line_id
            ),
            format!(
                "{}, {}, X'00FF', '{RESOLVED_AT}', X'01'",
                seeded.line_id, seeded.shot_id
            ),
        ],
    );

    let state = open(&path);
    let live = ignores_in_memory(&state);
    assert!(
        live.iter()
            .any(|w| w.shot == seeded.shot_uuid && w.code == IGNORED_CODE),
        "the readable row is kept: {live:?}"
    );

    let saved = state.execute(Command::SaveProject { path: None });
    forget_backups_of(&path);
    assert!(
        saved.success,
        "a poisoned row must not refuse the save: {:?}",
        saved.error
    );
    // The file keeps them sorted by shot then code (the canonical order);
    // a coerced row is carried inert, as 0.6.0 carried it.
    let mut expected = live;
    expected.sort_by(|a, b| (&a.shot, &a.code).cmp(&(&b.shot, &b.code)));
    assert_eq!(ignores_on_disk(&path), expected);
}

/// The #516 route, end to end: an UNSTAMPED pre-v8 file on write-protected
/// media opens (the frozen reader migrates a private copy; nothing is written
/// to the original), and Save As to a writable place carries its ignore,
/// keyed by uuid, as version 2.
#[test]
#[cfg(unix)]
fn an_unwritable_unstamped_pre_v8_file_keeps_its_ignores_through_a_save_as() {
    use std::os::unix::fs::PermissionsExt;

    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("readonly.stalx");
    let seeded = seed_current_file(&source);
    forge_ignore_table(
        &source,
        PRE_V8_COLUMNS,
        "section_id, shot_id, warning_code, resolved_at",
        &[format!(
            "{}, {}, '{IGNORED_CODE}', '{RESOLVED_AT}'",
            seeded.line_id, seeded.shot_id
        )],
    );
    rusqlite::Connection::open(&source)
        .unwrap()
        .execute_batch("PRAGMA user_version = 0;")
        .unwrap();
    let before = std::fs::read(&source).unwrap();
    if !make_read_only(
        &source,
        0o444,
        "an_unwritable_unstamped_pre_v8_file_keeps_its_ignores_through_a_save_as",
    ) {
        return;
    }

    let state = open(&source);
    let dest = temp.path().join("copy.stalx");
    let saved = state.execute(Command::SaveProject {
        path: Some(dest.clone()),
    });
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o644)).unwrap();

    assert!(
        saved.success,
        "Save As off write-protected media must work: {:?}",
        saved.error
    );
    assert_eq!(
        std::fs::read(&source).unwrap(),
        before,
        "nothing was written to the source"
    );
    let rows = ignores_on_disk(&dest);
    assert_eq!(
        rows.len(),
        1,
        "the ignore must ride along to the writable copy"
    );
    assert_eq!(rows[0].code, IGNORED_CODE);
    assert_eq!(rows[0].shot, seeded.shot_uuid, "keyed by the shot's uuid");
}

/// A version 2 project in a write-protected folder: a save is refused with
/// the permission marker and changes nothing (the edit stays unsaved); a
/// display toggle cannot be made durable but stays in memory; Save As to a
/// writable place writes the edit and the toggle.
#[test]
#[cfg(unix)]
fn on_write_protected_media_saves_are_refused_and_nothing_is_lost() {
    use std::os::unix::fs::PermissionsExt;

    let _cfg = crate::test_helpers::isolated_config();
    let temp = TempDir::new().unwrap();
    let media = temp.path().join("media");
    std::fs::create_dir(&media).unwrap();
    let path = media.join("cave.stalx");
    let mut project = crate::models::survey::Project::new("Media", None);
    let line = project
        .add_line(
            crate::models::survey::LineMeta::default(),
            crate::models::survey::StartAt::New,
            vec![ShotData {
                length: 3.0,
                ..ShotData::default()
            }],
        )
        .unwrap();
    let start = project.line(line).unwrap().start;
    project.set_placement(start, 0.0, 0.0).unwrap();
    std::fs::write(
        &path,
        stalx_v2::encode(&project, &Extras::default(), Mode::Normal).unwrap(),
    )
    .unwrap();
    let before = std::fs::read(&path).unwrap();
    let state = open(&path);
    let (line_id, shot_id) = {
        let p = state.get_project_state().unwrap();
        (p.lines()[0].id, p.lines()[0].shots[0].id)
    };
    let edit = state.execute(Command::UpdateShot {
        line_id,
        shot_id,
        updates: crate::models::ShotUpdates {
            length: Some(3.5),
            ..Default::default()
        },
    });
    assert!(edit.success, "{:?}", edit.error);
    if !make_read_only(
        &media,
        0o555,
        "on_write_protected_media_saves_are_refused_and_nothing_is_lost",
    ) {
        return;
    }

    let saved = state.execute(Command::SaveProject { path: None });
    let toggled = state.save_display_setting("showGrid", "true");
    std::fs::set_permissions(&media, std::fs::Permissions::from_mode(0o755)).unwrap();

    let error = saved.error.unwrap_or_default();
    assert!(!saved.success);
    assert!(
        error.starts_with(crate::core::loading::PERMISSION_DENIED_CODE),
        "the refusal routes to the permission remediation: {error}"
    );
    assert!(toggled.is_err(), "the toggle could not be made durable");
    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "nothing changed on disk"
    );
    assert_eq!(
        std::fs::read_dir(&media).unwrap().count(),
        1,
        "no temp left behind"
    );
    assert!(state.has_unsaved_changes());
    assert_eq!(
        state.get_display_setting("showGrid").unwrap().as_deref(),
        Some("true"),
        "the toggle stays in memory"
    );

    let dest = temp.path().join("copy.stalx");
    let saved = state.execute(Command::SaveProject {
        path: Some(dest.clone()),
    });
    assert!(saved.success, "{:?}", saved.error);
    let copy = stalx_v2::decode(&std::fs::read(&dest).unwrap(), &Default::default()).unwrap();
    assert_eq!(copy.project.lines()[0].shots[0].length, 3.5);
    assert_eq!(
        copy.extras
            .local
            .display
            .get("showGrid")
            .map(String::as_str),
        Some("true")
    );
}
