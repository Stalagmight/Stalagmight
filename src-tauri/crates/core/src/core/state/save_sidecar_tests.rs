//! [#576 A3, B15] What a save owes the side tables it cannot read or write.
//!
//! The save builds a FRESH `.stalx.tmp` and renames it over the destination, so a
//! table it fails to carry is gone from the file. The policy (maintainer decision
//! A3-D2, "stop unless cosmetic"):
//!
//! - ignored warnings, the SpeleoDB link and the undo-chain markers STOP the save:
//!   nothing is written, the file on disk is untouched;
//! - display settings are cosmetic: the save goes ahead and says so in a warning;
//! - undo history keeps drop-and-warn, and the warning is in the result.
//!
//! And B15-D1: per-project settings the open could not read refuse every save, so
//! the file keeps its stored values.
//!
//! Write failures have no organic trigger (the temp file is fresh), so they are
//! injected through the test-only `save_sidecars::fault` hook. Read failures are
//! organic: a column the reader needs is renamed in the open file.
//!
//! All data is synthetic, built in code. No fixture is read or written.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::save_sidecars::{fault, SaveMode, SaveRequest, Sidecar, SidecarPolicy};
use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::{CommandErrorCode, Project, SectionData, ShotData, ShotUpdates, SpeleoLink};
use crate::persistence::{Database, ResolvedWarning, SAVE_PROJECT_TABLES};
use crate::test_fixtures::state_with_null_sink;

const DISPLAY_KEY: &str = "show_grid";
const IGNORED_CODE: &str = "LONG_SHOT";
const CHAIN_SHA: &str = "older-tip";
const WATERMARK: &str = "2026-08-01T00:00:00+00:00";

fn fixture_link() -> SpeleoLink {
    SpeleoLink {
        endpoint: "http://10.10.0.220:8000/".to_string(),
        project_id: "11111111-1111-1111-1111-111111111111".to_string(),
        project_name: "CaveA".to_string(),
        base_commit_sha: Some("abc123".to_string()),
        has_unpushed_edits: false,
        last_edit_at: None,
        last_verify_at: None,
        undo_mode: None,
    }
}

/// An open session over a file that carries every side table a save has to carry.
struct Fixture {
    state: AppState,
    path: PathBuf,
    _dir: TempDir,
}

/// One section of two shots, a stored display setting and an ignored warning on
/// the first shot; when `linked`, also the SpeleoDB link and both undo-chain
/// markers. Opened through `open_project`, then edited once (so the save has an
/// edit to land and an undo entry to carry).
fn fixture(linked: bool) -> Fixture {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("CaveA.stalx");
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
    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    // Read back for the ids and UUIDs the file actually holds.
    let stored = db.load_project().unwrap();
    let section = &stored.sections[0];
    db.save_resolved_warning(&ResolvedWarning {
        section_id: section.id,
        shot_id: section.shots[0].id,
        warning_code: IGNORED_CODE.to_string(),
        resolved_at: chrono::Utc::now(),
        resolved_comment: Some("checked".to_string()),
        shot_uuid: section.shots[0].uuid.clone(),
    })
    .unwrap();
    db.save_display_setting(DISPLAY_KEY, "true").unwrap();
    if linked {
        db.save_speleo_link(Some(&fixture_link())).unwrap();
        db.save_speleo_undo_chain(Some(CHAIN_SHA)).unwrap();
        db.save_speleo_undo_watermark(Some(WATERMARK)).unwrap();
    }
    drop(db);

    let state = AppState::new().unwrap();
    let opened = state.open_project(path.clone());
    assert!(opened.success, "open failed: {:?}", opened.error);
    // A linked open is read-only until the lock is taken; take it.
    state.set_edit_enabled(true);
    let live = state.get_project_state().unwrap();
    let edited = state.execute(Command::UpdateShot {
        section_id: live.sections[0].id,
        shot_id: live.sections[0].shots[1].id,
        updates: ShotUpdates {
            length: Some(9.0),
            ..Default::default()
        },
    });
    assert!(edited.success, "edit failed: {:?}", edited.error);
    if linked {
        // A forward edit drops the chain pointer (the walk no longer describes the
        // copy); an Unwind after it is what leaves one in place at save time.
        state.speleodb_set_unwound_from(Some(CHAIN_SHA));
    }
    Fixture {
        state,
        path,
        _dir: dir,
    }
}

/// What the file at `path` holds of each side table, read through a fresh handle.
struct OnDisk {
    ignores: usize,
    display_settings: usize,
    link: Option<SpeleoLink>,
    chain: Option<String>,
    watermark: Option<String>,
    second_shot_length: f64,
}

fn on_disk(path: &Path) -> OnDisk {
    let db = Database::open_for_metadata(path).unwrap();
    OnDisk {
        ignores: db.load_resolved_warnings_verbatim().unwrap().len(),
        display_settings: db.load_display_settings().unwrap().len(),
        link: db.load_speleo_link().unwrap(),
        chain: db.load_speleo_undo_chain().unwrap(),
        watermark: db.load_speleo_undo_watermark().unwrap(),
        second_shot_length: db.load_project().unwrap().sections[0].shots[1].length,
    }
}

fn describe(label: &str, result: &CommandResult, path: &Path) {
    let codes: Vec<&str> = result.warnings.iter().map(|w| w.code.as_str()).collect();
    println!(
        "{label}: save success={} error={:?}",
        result.success, result.error
    );
    println!("{label}: save warnings={codes:?}");
    match Database::open_for_metadata(path) {
        Ok(db) => println!(
            "{label}: file now holds ignores={:?} display_settings={:?} link={:?} chain={:?} \
             watermark={:?}",
            db.load_resolved_warnings_verbatim().map(|v| v.len()),
            db.load_display_settings().map(|m| m.len()),
            db.load_speleo_link().map(|l| l.is_some()),
            db.load_speleo_undo_chain(),
            db.load_speleo_undo_watermark(),
        ),
        Err(e) => println!("{label}: file could not be reopened: {e}"),
    }
}

/// A refused save: not a success, the destination byte-for-byte as it was, no
/// temp file left behind, and the session still on its file with its handle.
fn assert_refused_and_untouched(fx: &Fixture, before: &[u8], result: &CommandResult) {
    assert!(
        !result.success,
        "the save must be refused, it reported success with warnings {:?}",
        result.warnings
    );
    assert!(
        std::fs::read(&fx.path).unwrap() == before,
        "a refused save must leave the file on disk untouched"
    );
    assert!(
        !fx.path.with_extension("stalx.tmp").exists(),
        "a refused save must not leave its temp file behind"
    );
    assert_eq!(
        fx.state.open_project_path().as_deref(),
        Some(fx.path.as_path())
    );
    assert!(
        fx.state.database.lock().is_some(),
        "the session keeps its database handle"
    );
}

fn has_warning(result: &CommandResult, code: &str) -> bool {
    result.warnings.iter().any(|w| w.code == code)
}

// --- Stop: the ignored warnings -------------------------------------------------

/// NF-AN2-3: the ignore list's WRITE into the temp file was fail-open. The read
/// already refused (#517), but a failed write only logged, and the file was
/// renamed into place without a single ignore.
#[test]
fn a_failed_ignores_write_refuses_the_save_and_leaves_the_file_untouched() {
    let fx = fixture(false);
    let before = std::fs::read(&fx.path).unwrap();
    let _fault = fault::fail_writes_to("resolved_warnings");

    let result = fx.state.save_project(None);

    describe("ignores write fails", &result, &fx.path);
    assert_refused_and_untouched(&fx, &before, &result);
    assert_eq!(on_disk(&fx.path).ignores, 1, "the file keeps its ignore");
}

// --- Stop: the SpeleoDB link ----------------------------------------------------

/// The link is the project's identity. Renamed into place without it, a managed
/// project silently becomes a plain local file (B35(b)).
#[test]
fn a_failed_link_write_refuses_the_save_and_leaves_the_file_untouched() {
    let fx = fixture(true);
    let before = std::fs::read(&fx.path).unwrap();
    let _fault = fault::fail_writes_to("speleo_link");

    let result = fx.state.save_project(None);

    describe("link write fails", &result, &fx.path);
    assert_refused_and_untouched(&fx, &before, &result);
    assert!(on_disk(&fx.path).link.is_some(), "the file keeps its link");
}

// --- Stop: the undo-chain markers -----------------------------------------------

/// The chain pointer and the inherited-history watermark decide which undo rows
/// are the user's own (#303). They used to be dropped together with the whole
/// undo history on a failed write; the policy is now to stop.
#[test]
fn a_failed_undo_chain_write_refuses_the_save_and_leaves_the_file_untouched() {
    for table in ["speleo_undo_chain", "speleo_undo_watermark"] {
        let fx = fixture(true);
        let before = std::fs::read(&fx.path).unwrap();
        let _fault = fault::fail_writes_to(table);

        let result = fx.state.save_project(None);

        describe(&format!("{table} write fails"), &result, &fx.path);
        assert_refused_and_untouched(&fx, &before, &result);
        let disk = on_disk(&fx.path);
        assert_eq!(disk.chain.as_deref(), Some(CHAIN_SHA));
        assert_eq!(disk.watermark.as_deref(), Some(WATERMARK));
        assert!(
            fx.state.undo_stack.lock().can_undo(),
            "a refused save leaves the live undo history alone"
        );
    }
}

/// The read side of the same rule: a marker table that cannot be read refuses
/// the save instead of saving with no undo history.
#[test]
fn unreadable_undo_chain_markers_refuse_the_save_and_leave_the_file_untouched() {
    let fx = fixture(true);
    rusqlite::Connection::open(&fx.path)
        .unwrap()
        .execute_batch(
            "ALTER TABLE speleo_undo_watermark RENAME COLUMN inherited_through TO broken_value;",
        )
        .unwrap();
    let before = std::fs::read(&fx.path).unwrap();

    let result = fx.state.save_project(None);

    describe("markers unreadable", &result, &fx.path);
    assert_refused_and_untouched(&fx, &before, &result);
    let error = result.error.as_deref().unwrap_or_default();
    assert!(
        result.code == Some(CommandErrorCode::UndoMarkersUnreadable)
            && error.starts_with("Can't save: "),
        "the refusal carries its code (and no marker), got: {error}"
    );
    assert!(
        fx.state.undo_stack.lock().can_undo(),
        "a refused save leaves the live undo history alone"
    );
}

// --- Continue, and say so: display settings --------------------------------------

/// Display settings are per-machine view state: not worth refusing a save over,
/// but the user is told (the warning reaches the frontend in the result).
#[test]
fn a_failed_display_settings_write_saves_with_a_warning() {
    let fx = fixture(false);
    let _fault = fault::fail_writes_to("display_settings");

    let result = fx.state.save_project(None);

    describe("display settings write fails", &result, &fx.path);
    assert!(result.success, "the save goes ahead: {:?}", result.error);
    assert!(
        has_warning(&result, "DISPLAY_SETTINGS_NOT_SAVED"),
        "the lost display settings must be reported, got {:?}",
        result.warnings
    );
    let disk = on_disk(&fx.path);
    assert!(
        (disk.second_shot_length - 9.0).abs() < 1e-9,
        "the edit landed"
    );
    assert_eq!(disk.ignores, 1, "everything else was carried");
}

/// The read side: an unreadable display-settings table used to fall back to the
/// (empty) in-memory cache silently.
#[test]
fn unreadable_display_settings_save_with_a_warning() {
    let fx = fixture(false);
    rusqlite::Connection::open(&fx.path)
        .unwrap()
        .execute_batch("ALTER TABLE display_settings RENAME COLUMN value TO broken_value;")
        .unwrap();

    let result = fx.state.save_project(None);

    describe("display settings unreadable", &result, &fx.path);
    assert!(result.success, "the save goes ahead: {:?}", result.error);
    assert!(
        has_warning(&result, "DISPLAY_SETTINGS_NOT_SAVED"),
        "the lost display settings must be reported, got {:?}",
        result.warnings
    );
    assert!((on_disk(&fx.path).second_shot_length - 9.0).abs() < 1e-9);
}

// --- Drop and warn: undo history -------------------------------------------------

/// Undo history keeps drop-and-warn: the survey saves, the history is removed
/// from the file, and the result says so.
#[test]
fn a_failed_history_write_saves_without_history_and_says_so() {
    let fx = fixture(true);
    let _fault = fault::fail_writes_to("undo_stack");

    let result = fx.state.save_project(None);

    describe("history write fails", &result, &fx.path);
    assert!(result.success, "the save goes ahead: {:?}", result.error);
    assert!(has_warning(&result, "UNDO_HISTORY_DROPPED"));
    let disk = on_disk(&fx.path);
    assert!(disk.link.is_some(), "the link was carried");
    assert_eq!(disk.ignores, 1, "the ignores were carried");
    assert!((disk.second_shot_length - 9.0).abs() < 1e-9);
}

// --- B15: stored project settings the open could not read ------------------------

/// A project whose stored validation thresholds, entrance elevation and north
/// reference are all non-default, with the settings table then broken so the
/// open cannot read any of them.
fn file_with_unreadable_settings(dir: &Path) -> PathBuf {
    let path = dir.join("CaveB.stalx");
    let mut project = Project::new("CaveB".to_string(), None);
    project.add_section(SectionData {
        name: Some("SectionB".to_string()),
        color: "#654321".to_string(),
        shots: vec![ShotData {
            length: 4.0,
            ..Default::default()
        }],
        ..Default::default()
    });
    project.validation.warn_shot_length_max = 77.0;
    project.surface_elevation = Some(123.0);
    project.default_azimuth_reference = Some(crate::models::AzimuthReference::magnetic(
        4.5,
        crate::models::CorrectionSource::FileDeclared,
    ));
    let mut db = Database::open(&path).unwrap();
    db.save_project(&project).unwrap();
    drop(db);
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("ALTER TABLE project_settings RENAME COLUMN value TO broken_value;")
        .unwrap();
    path
}

/// The stored values, read back through a copy whose column is put right again.
fn stored_settings(dir: &Path, path: &Path) -> Vec<(String, String)> {
    let copy = dir.join("readback.stalx");
    std::fs::copy(path, &copy).unwrap();
    let conn = rusqlite::Connection::open(&copy).unwrap();
    // Still broken unless a save replaced the table with a fresh one.
    let _ = conn.execute_batch("ALTER TABLE project_settings RENAME COLUMN broken_value TO value;");
    let mut stmt = conn
        .prepare("SELECT key, value FROM project_settings ORDER BY key")
        .unwrap();
    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    rows
}

/// B15: a settings read that failed used to read as "not set", so the next save
/// wrote the defaults over the stored values (and DELETEd the elevation and the
/// north reference). Now the project opens and edits as usual, and every save -
/// in place and Save As - is refused, so the file keeps its values.
#[test]
fn unreadable_project_settings_are_never_saved_over() {
    let dir = TempDir::new().unwrap();
    let path = file_with_unreadable_settings(dir.path());
    let stored_before = stored_settings(dir.path(), &path);
    println!("stored before the save: {stored_before:?}");

    let state = AppState::new().unwrap();
    let opened = state.open_project(path.clone());
    assert!(
        opened.success,
        "the project still opens: {:?}",
        opened.error
    );
    let live = state.get_project_state().unwrap();
    let edited = state.execute(Command::UpdateShot {
        section_id: live.sections[0].id,
        shot_id: live.sections[0].shots[0].id,
        updates: ShotUpdates {
            length: Some(6.0),
            ..Default::default()
        },
    });
    assert!(edited.success, "editing works: {:?}", edited.error);
    let before = std::fs::read(&path).unwrap();

    let in_place = state.save_project(None);
    let save_as = state.save_project(Some(dir.path().join("elsewhere.stalx")));

    println!(
        "save in place: success={} error={:?}",
        in_place.success, in_place.error
    );
    println!(
        "save as:       success={} error={:?}",
        save_as.success, save_as.error
    );
    println!(
        "stored after the save:  {:?}",
        stored_settings(dir.path(), &path)
    );
    assert!(!in_place.success, "a save in place must be refused");
    assert!(!save_as.success, "a Save As must be refused too");
    for refused in [&in_place, &save_as] {
        let error = refused.error.as_deref().unwrap_or_default();
        assert!(
            refused.code == Some(CommandErrorCode::ProjectSettingsUnreadable)
                && error.starts_with("Can't save: "),
            "the refusal carries its code (and no marker), got: {error}"
        );
    }
    assert!(
        std::fs::read(&path).unwrap() == before,
        "the file on disk must be untouched"
    );
    assert!(!dir.path().join("elsewhere.stalx").exists());
    assert_eq!(stored_settings(dir.path(), &path), stored_before);
}

const ALL_THREE: [&str; 3] = [
    "validation",
    "surface_elevation",
    "default_azimuth_reference",
];

/// B15: the project still opens - through both load paths - and the user is told
/// WHICH settings could not be read: the production load hands the list to the
/// frontend on its result, and both paths commit it as the state the save
/// refusal reads. Closing the project clears it.
#[test]
fn the_open_reports_which_settings_could_not_be_read() {
    let dir = TempDir::new().unwrap();
    let path = file_with_unreadable_settings(dir.path());

    let state = state_with_null_sink();
    let loaded = state.do_load_project_full(path.clone()).unwrap();
    println!(
        "load_project_full: opened={} unreadable_project_settings={:?}",
        loaded.project.sections.len() == 1,
        loaded.unreadable_project_settings
    );
    assert_eq!(loaded.unreadable_project_settings, ALL_THREE);
    assert_eq!(state.unreadable_project_settings(), ALL_THREE);
    // The defaults are what the session holds meanwhile.
    assert_eq!(loaded.project.surface_elevation, None);
    assert_eq!(loaded.project.default_azimuth_reference, None);
    let refused = state.save_project(None);
    let error = refused.error.unwrap_or_default();
    println!("save: {error}");
    assert!(error.contains(
        "validation thresholds, entrance elevation and default north reference could not be read"
    ));

    assert!(state.close_project().success);
    assert!(
        state.unreadable_project_settings().is_empty(),
        "close clears it"
    );

    let legacy = AppState::new().unwrap();
    assert!(legacy.open_project(path).success);
    assert_eq!(legacy.unreadable_project_settings(), ALL_THREE);
}

/// The other half: "not set" is still "not set". A file whose settings rows are
/// simply absent - or whose table predates the feature - opens with nothing
/// reported and saves as before; only an UNREADABLE value refuses.
#[test]
fn absent_project_settings_are_not_unreadable_and_still_save() {
    let dir = TempDir::new().unwrap();
    for (name, breakage) in [
        ("rows.stalx", "DELETE FROM project_settings;"),
        ("table.stalx", "DROP TABLE project_settings;"),
    ] {
        let path = dir.path().join(name);
        let mut db = Database::open(&path).unwrap();
        db.save_project(&Project::new("CaveC".to_string(), None))
            .unwrap();
        drop(db);
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch(breakage)
            .unwrap();

        let state = AppState::new().unwrap();
        assert!(state.open_project(path.clone()).success, "{name} opens");
        assert!(
            state.unreadable_project_settings().is_empty(),
            "{name}: absent settings are not unreadable"
        );
        let saved = state.save_project(None);
        assert!(saved.success, "{name} still saves: {:?}", saved.error);
    }
}

// --- The registry itself ----------------------------------------------------------

/// THE coverage guard (#576 A3): every table a fresh schema creates is either
/// rebuilt from the `Project` by the core save or carried by the side-table
/// registry - exactly one of the two. A table in neither is emptied by every save,
/// silently, which is how the SpeleoDB link (#191) and the Unwind chain pointer
/// (#303) were lost before anyone listed them.
#[test]
fn every_schema_table_is_written_by_the_core_save_or_carried_by_the_registry() {
    let schema = Database::in_memory()
        .unwrap()
        .list_raw_table_names()
        .unwrap();
    let carried: Vec<&str> = Sidecar::ALL
        .iter()
        .flat_map(|sidecar| sidecar.tables().iter().copied())
        .collect();
    for table in &schema {
        let core = SAVE_PROJECT_TABLES.contains(&table.as_str());
        let side = carried.contains(&table.as_str());
        assert!(
            core || side,
            "`{table}` is created by the schema but neither written by Database::save_project \
             nor registered in save_sidecars::Sidecar: every save would empty it"
        );
        assert!(
            !(core && side),
            "`{table}` is both core-written and carried: the carry would overwrite the survey"
        );
    }
    for table in carried {
        assert!(
            schema.iter().any(|t| t == table),
            "the registry carries `{table}`, which the schema does not create"
        );
    }
}

/// The mode is resolved once, from four facts, instead of being inferred again
/// at each step (the inference from a missing database handle is what sent
/// autosaves to `<stem>_1.stalx`, B35(c)).
#[test]
fn the_save_mode_is_resolved_from_four_facts() {
    use SaveMode::{DetachedCopy, FirstSave, InPlace, SaveAs};
    // (deferred, implicit, linked, rewrites_open_file) -> mode
    let cases = [
        ((true, true, false, false), FirstSave),
        ((true, false, false, false), FirstSave),
        ((false, true, false, true), InPlace),
        ((false, false, false, true), InPlace),
        ((false, true, true, true), InPlace),
        ((false, false, false, false), SaveAs),
        ((false, false, true, false), DetachedCopy),
        // An explicit path naming the managed file resolves to the copy, which the
        // save then refuses (it would strip the link in place).
        ((false, false, true, true), DetachedCopy),
    ];
    for ((deferred, implicit, linked, rewrites), expected) in cases {
        let request = SaveRequest {
            destination: Path::new("CaveA.stalx"),
            deferred,
            implicit,
            rewrites_open_file: rewrites,
        };
        assert_eq!(
            request.mode(linked),
            expected,
            "deferred={deferred} implicit={implicit} linked={linked} rewrites={rewrites}"
        );
    }
}

/// What each mode carries, and the policy each table is under (A3-D2).
#[test]
fn a_detached_copy_sheds_the_server_tables_and_every_policy_is_the_decided_one() {
    for sidecar in Sidecar::ALL {
        let server_state = matches!(sidecar, Sidecar::SpeleoLink | Sidecar::UndoMarkers);
        assert_eq!(sidecar.carried_in(SaveMode::DetachedCopy), !server_state);
        // [#576 W2-SAVE question 1] So does a Save As away from an unreadable link.
        assert_eq!(sidecar.carried_in(SaveMode::UnlinkedSaveAs), !server_state);
        for mode in [SaveMode::FirstSave, SaveMode::InPlace, SaveMode::SaveAs] {
            assert!(
                sidecar.carried_in(mode),
                "{sidecar:?} is carried in {mode:?}"
            );
        }
        let expected = match sidecar {
            Sidecar::ResolvedWarnings | Sidecar::SpeleoLink | Sidecar::UndoMarkers => "refuse",
            Sidecar::DisplaySettings => "warn",
            Sidecar::History => "drop-history",
        };
        let actual = match sidecar.policy() {
            SidecarPolicy::Refuse { .. } => "refuse",
            SidecarPolicy::WarnAndContinue { .. } => "warn",
            SidecarPolicy::DropHistory { .. } => "drop-history",
        };
        assert_eq!(actual, expected, "{sidecar:?}");
    }
}

/// The snapshot pass resolves the mode at the link's turn, so a table read before
/// it is read before the mode is known: it has to be one every mode carries.
#[test]
fn every_table_read_before_the_link_is_carried_in_every_mode() {
    let link_at = Sidecar::ALL
        .iter()
        .position(|s| *s == Sidecar::SpeleoLink)
        .unwrap();
    for sidecar in &Sidecar::ALL[..link_at] {
        for mode in [
            SaveMode::FirstSave,
            SaveMode::InPlace,
            SaveMode::SaveAs,
            SaveMode::DetachedCopy,
            SaveMode::UnlinkedSaveAs,
        ] {
            assert!(
                sidecar.carried_in(mode),
                "{sidecar:?} is not carried in {mode:?}"
            );
        }
    }
}

/// The positive half: a save in place of a linked project carries every side
/// table onto the fresh file, and reports nothing.
#[test]
fn a_linked_save_carries_every_side_table() {
    let fx = fixture(true);

    let result = fx.state.save_project(None);

    describe("linked save", &result, &fx.path);
    assert!(result.success, "save failed: {:?}", result.error);
    assert!(
        result.warnings.is_empty(),
        "nothing was lost: {:?}",
        result.warnings
    );
    let disk = on_disk(&fx.path);
    assert_eq!(disk.ignores, 1);
    assert_eq!(disk.display_settings, 1);
    assert!(disk.link.is_some(), "the link was carried");
    let link = disk.link.unwrap();
    assert!(link.has_unpushed_edits, "the dirty bit rides with the link");
    assert_eq!(disk.chain.as_deref(), Some(CHAIN_SHA));
    assert_eq!(disk.watermark.as_deref(), Some(WATERMARK));
    assert!((disk.second_shot_length - 9.0).abs() < 1e-9);
    let undo_rows = Database::open_for_metadata(&fx.path)
        .unwrap()
        .load_undo_stack_with_timestamps()
        .unwrap()
        .len();
    assert!(undo_rows > 0, "the undo history was carried");
}

/// [#576 B35(b), folded into the registry] The link-unreadable refusal now carries
/// its own marker, like the ignore list's.
#[test]
fn an_unreadable_link_refusal_carries_its_marker() {
    let fx = fixture(true);
    rusqlite::Connection::open(&fx.path)
        .unwrap()
        .execute_batch("ALTER TABLE speleo_link RENAME COLUMN project_name TO project_title;")
        .unwrap();
    let before = std::fs::read(&fx.path).unwrap();

    let result = fx.state.save_project(None);

    describe("link unreadable", &result, &fx.path);
    assert_refused_and_untouched(&fx, &before, &result);
    let error = result.error.as_deref().unwrap_or_default();
    assert!(
        result.code == Some(CommandErrorCode::SpeleodbLinkUnreadable)
            && error.starts_with("Can't save: "),
        "the refusal carries its code (and no marker), got: {error}"
    );
    assert!(
        error.contains("Use Save As"),
        "and names the way out: {error}"
    );
}
