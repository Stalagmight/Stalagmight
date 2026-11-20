//! "Merge projects" (#674) on the station-graph model: N sources into a NEW
//! version 2 file, the open project never touched.
//!
//! Ported: the 0.6.0 merge edge cases of `tests.rs` ("Coverage gap tests:
//! merge_projects edge cases": an existing output, an extension-less output
//! over an existing `.stalx`, a missing source, combined locations, no
//! archive, preserved merges, which are junctions now), and the merge cases
//! of `project_add_tests.rs` (placement by GPS, add and merge placing alike,
//! a datum-less first source, an untrustworthy source, a source an open would
//! rewrite left untouched, the WAL read and archived with its file, the hot
//! journal rolled back, the long source label). Retired: the digest of the
//! 0.6.0 output (it pinned the old model's bytes); replaced by "the output
//! is exactly the appended sources". New: the E5 refusal, the elevation
//! warning, fewer than two sources, the open project untouched.
//!
//! Every project is synthetic (the kit in `project_add_tests.rs`); the
//! legacy cases copy the tracked e2e fixture into a temp dir.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::project_add_tests::{
    add, assert_unchanged, assert_valid, at, fixed_time, georeferenced, legacy_copy,
    legacy_with_hot_journal, legacy_with_wal, lengths, line_named, meta, open, open_target,
    read_v2, shot, sidecar, snapshot, source_alpha, source_bravo, source_charlie, write_v2, xy,
};
use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::survey::{Project, StartAt};
use crate::models::MAX_STATION_ID;

fn merge(sources: Vec<PathBuf>, output: &Path, archive: bool) -> CommandResult {
    AppState::new().unwrap().execute(Command::MergeProjects {
        source_paths: sources,
        output_path: output.to_path_buf(),
        output_name: "Merged".to_string(),
        archive_originals: archive,
    })
}

/// Merge `sources` into `dir/merged.stalx` and return the decoded output.
fn merge_files(dir: &Path, sources: Vec<PathBuf>) -> Project {
    let output = dir.join("merged.stalx");
    let result = merge(sources, &output, false);
    assert!(result.success, "merge failed: {:?}", result.error);
    read_v2(&output).project
}

/// Two one-line datum-less sources `a.stalx` and `b.stalx`.
fn two_sources(dir: &Path) -> (PathBuf, PathBuf) {
    let one = |name: &str, uuid: &str, created: i64| {
        let mut p = Project::new_at(name, None, fixed_time(created));
        let line = p
            .add_line(meta(name), StartAt::New, vec![shot(uuid, 0.0, 2.0, 1.0)])
            .unwrap();
        let start = at(&p, line, 0);
        p.set_placement(start, 0.0, 0.0).unwrap();
        p
    };
    (
        write_v2(dir, "a.stalx", &one("A", "a-0", 20)),
        write_v2(dir, "b.stalx", &one("B", "b-0", 21)),
    )
}

// ===========================================================================
// Ported: the 0.6.0 edge cases
// ===========================================================================

#[test]
fn test_merge_projects_output_already_exists() {
    let temp = TempDir::new().unwrap();
    let (a, b) = two_sources(temp.path());
    let output = temp.path().join("output.stalx");
    std::fs::write(&output, b"existing").unwrap();

    let result = merge(vec![a, b], &output, false);
    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("already exists"));
    assert_eq!(std::fs::read(&output).unwrap(), b"existing");
}

/// An extension-less output path is normalized to `.stalx` BEFORE the
/// existence check, so a merge cannot silently overwrite an existing
/// `output.stalx` (#169).
#[test]
fn test_merge_projects_extension_less_output_detects_existing_stalx() {
    let temp = TempDir::new().unwrap();
    let (a, b) = two_sources(temp.path());
    let existing = temp.path().join("output.stalx");
    std::fs::write(&existing, b"existing").unwrap();

    let result = merge(vec![a, b], &temp.path().join("output"), false);
    assert!(!result.success, "the existing output.stalx is detected");
    assert!(result.error.unwrap_or_default().contains("already exists"));
    assert_eq!(std::fs::read(&existing).unwrap(), b"existing");
}

#[test]
fn test_merge_projects_source_missing() {
    let temp = TempDir::new().unwrap();
    let (a, _) = two_sources(temp.path());
    let output = temp.path().join("output.stalx");

    let result = merge(vec![a, temp.path().join("missing.stalx")], &output, false);
    assert!(!result.success);
    assert!(result.error.unwrap_or_default().contains("not found"));
    assert!(!output.exists());
}

#[test]
fn test_merge_projects_combines_locations() {
    let temp = TempDir::new().unwrap();
    let merged = merge_files(
        temp.path(),
        vec![
            write_v2(temp.path(), "alpha.stalx", &source_alpha()),
            write_v2(temp.path(), "bravo.stalx", &source_bravo()),
            // Charlie's location is Alpha's: not appended twice.
            write_v2(temp.path(), "charlie.stalx", &source_charlie()),
        ],
    );
    assert_eq!(merged.location.as_deref(), Some("Yucatan, Quintana Roo"));
}

#[test]
fn test_merge_projects_no_archive() {
    let temp = TempDir::new().unwrap();
    let (a, b) = two_sources(temp.path());
    let result = merge(
        vec![a.clone(), b.clone()],
        &temp.path().join("merged.stalx"),
        false,
    );
    assert!(result.success, "{:?}", result.error);
    assert!(a.exists(), "source A should still exist");
    assert!(b.exists(), "source B should still exist");
}

/// A merge inside one source (a junction on the station graph) is kept, on
/// the source's own lines.
#[test]
fn test_merge_projects_preserves_station_merges() {
    let temp = TempDir::new().unwrap();
    let mut a = Project::new_at("A", None, fixed_time(30));
    let a1 = a
        .add_line(
            meta("A1"),
            StartAt::New,
            vec![shot("a1-0", 0.0, 1.0, 0.0), shot("a1-1", 10.0, 1.5, 0.0)],
        )
        .unwrap();
    let a2 = a
        .add_line(
            meta("A2"),
            StartAt::Station(at(&a, a1, 1)),
            vec![shot("a2-0", 90.0, 2.0, 0.0)],
        )
        .unwrap();
    let start = at(&a, a1, 0);
    a.set_placement(start, 0.0, 0.0).unwrap();
    let mut b = Project::new_at("B", None, fixed_time(31));
    let b1 = b
        .add_line(meta("B1"), StartAt::New, vec![shot("b1-0", 0.0, 3.0, 0.0)])
        .unwrap();
    let start = at(&b, b1, 0);
    b.set_placement(start, 0.0, 0.0).unwrap();
    let _ = a2;

    let merged = merge_files(
        temp.path(),
        vec![
            write_v2(temp.path(), "a.stalx", &a),
            write_v2(temp.path(), "b.stalx", &b),
        ],
    );
    let junctions: Vec<_> = merged
        .stations
        .keys()
        .filter(|s| merged.slots_of(**s).len() > 1)
        .collect();
    assert_eq!(junctions.len(), 1, "the junction is preserved");
    let lines: Vec<u32> = merged
        .slots_of(*junctions[0])
        .iter()
        .map(|slot| slot.line_id)
        .collect();
    assert_eq!(
        lines,
        vec![line_named(&merged, "A1"), line_named(&merged, "A2")],
        "on the source's own two lines"
    );
}

// ===========================================================================
// The output
// ===========================================================================

/// The output is exactly "new project + append each source": the same
/// survey a merge in memory gives, as a canonical version 2 file with no
/// history, the global validation thresholds and the app's generator.
#[test]
fn a_merge_writes_exactly_the_appended_sources() {
    let temp = TempDir::new().unwrap();
    let sources = [source_alpha(), source_bravo(), source_charlie()];
    let paths: Vec<PathBuf> = sources
        .iter()
        .zip(["alpha.stalx", "bravo.stalx", "charlie.stalx"])
        .map(|(p, file)| write_v2(temp.path(), file, p))
        .collect();
    let output = temp.path().join("merged.stalx");
    let state = AppState::new().unwrap();
    let result = state.execute(Command::MergeProjects {
        source_paths: paths,
        output_path: output.clone(),
        output_name: "Merged".to_string(),
        archive_originals: false,
    });
    assert!(result.success, "{:?}", result.error);

    let decoded = read_v2(&output);
    let written = decoded.project;
    assert_valid(&written);
    let mut expected = Project::new_at("Merged", None, written.created_at);
    expected.modified_at = written.modified_at;
    expected.settings.validation = state.get_config().validation;
    for source in &sources {
        expected.append_project(source).unwrap();
    }
    let canonical = |p: &Project| {
        crate::persistence::stalx_v2::encode(
            p,
            &crate::persistence::stalx_v2::Extras::default(),
            crate::persistence::stalx_v2::Mode::Normal,
        )
        .unwrap()
    };
    assert_eq!(canonical(&written), canonical(&expected));
    assert!(
        decoded.extras.history.is_empty(),
        "a new file has no history"
    );
    assert!(decoded.extras.ignored_warnings.is_empty());
    assert!(decoded.extras.speleodb.is_none());
    assert!(
        written.imported_file_hashes.is_empty(),
        "as 0.6.0: the output carries no import hashes"
    );
    assert_eq!(written.lines().len(), 6);
}

/// A disagreeing entrance elevation keeps the first source's and says so.
#[test]
fn a_disagreeing_entrance_elevation_is_reported() {
    let temp = TempDir::new().unwrap();
    let output = temp.path().join("merged.stalx");
    let result = merge(
        vec![
            write_v2(temp.path(), "alpha.stalx", &source_alpha()),
            write_v2(temp.path(), "bravo.stalx", &source_bravo()),
        ],
        &output,
        false,
    );
    assert!(result.success, "{:?}", result.error);
    assert_eq!(result.warnings.len(), 1);
    assert_eq!(result.warnings[0].code, "SURFACE_ELEVATION_MISMATCH");
    assert_eq!(
        result.warnings[0].message,
        "Source project's entrance elevation (15.00 m) differs from the merged project's \
         value (12.00 m); kept 12.00 m"
    );
    assert_eq!(
        read_v2(&output).project.settings.surface_elevation,
        Some(12.0)
    );
}

/// A merge never touches the open project.
#[test]
fn a_merge_leaves_the_open_project_alone() {
    let temp = TempDir::new().unwrap();
    let (state, _) = open_target(temp.path(), true);
    let before = snapshot(&state);
    let depth = state.get_undo_stack().len();
    let (a, b) = two_sources(temp.path());

    let result = state.execute(Command::MergeProjects {
        source_paths: vec![a, b],
        output_path: temp.path().join("merged.stalx"),
        output_name: "Merged".to_string(),
        archive_originals: false,
    });
    assert!(result.success, "{:?}", result.error);
    assert_unchanged(&state, &before, "the open project");
    assert_eq!(state.get_undo_stack().len(), depth);
    assert!(!state.has_unsaved_changes());
}

#[test]
fn a_merge_needs_two_sources() {
    let temp = TempDir::new().unwrap();
    let (a, _) = two_sources(temp.path());
    let output = temp.path().join("merged.stalx");
    for (sources, expected) in [
        (vec![], "No source projects provided"),
        (vec![a], "At least two projects are required for merge"),
    ] {
        let result = merge(sources, &output, false);
        assert_eq!(result.error.as_deref(), Some(expected));
        assert!(!output.exists());
    }
}

// ===========================================================================
// Placement
// ===========================================================================

#[test]
fn merging_two_georeferenced_projects_places_the_second_by_its_gps() {
    let temp = TempDir::new().unwrap();
    let merged = merge_files(
        temp.path(),
        vec![
            write_v2(temp.path(), "north.stalx", &georeferenced("N", "n", 20.6)),
            write_v2(temp.path(), "south.stalx", &georeferenced("S", "s", 20.5)),
        ],
    );
    let north = xy(&merged, at(&merged, line_named(&merged, "N"), 0));
    let south = xy(&merged, at(&merged, line_named(&merged, "S"), 0));
    assert!(north[0].abs() < 1e-9 && north[1].abs() < 1e-9, "{north:?}");
    let expected = crate::models::gps_to_local(
        &super::project_add_tests::gps(20.5, -87.3, 0.0),
        &super::project_add_tests::gps(20.6, -87.3, 0.0),
    );
    assert!(
        (south[0] - expected.0).abs() < 1e-6 && (south[1] - expected.1).abs() < 1e-6,
        "{south:?} vs {expected:?}"
    );
}

/// The add and the merge share the copy: a georeferenced source lands on the
/// same spot both ways.
#[test]
fn merge_and_add_place_a_georeferenced_source_identically() {
    let temp = TempDir::new().unwrap();
    let base = write_v2(
        temp.path(),
        "base.stalx",
        &georeferenced("Base", "base", 20.6),
    );
    let other = write_v2(
        temp.path(),
        "other.stalx",
        &georeferenced("Other", "o", 20.5),
    );
    let merged = merge_files(temp.path(), vec![base.clone(), other.clone()]);
    let state = open(&base);
    assert!(add(&state, &other).success);
    let added = state.get_project_state().unwrap();

    let stations = |p: &Project| -> Vec<[f64; 2]> {
        let line = line_named(p, "Other");
        (0..3).map(|i| xy(p, at(p, line, i))).collect()
    };
    assert_eq!(stations(&merged), stations(&added));
}

/// A datum-less first source stays where it is, and the first datum of a
/// later source becomes the merged project's frame (its station on the
/// origin).
#[test]
fn a_datum_less_first_source_stays_put_and_the_next_datum_frames_the_merge() {
    let temp = TempDir::new().unwrap();
    let merged = merge_files(
        temp.path(),
        vec![
            write_v2(temp.path(), "charlie.stalx", &source_charlie()),
            write_v2(temp.path(), "geo.stalx", &georeferenced("G", "g", 20.5)),
        ],
    );
    assert_eq!(
        xy(&merged, at(&merged, line_named(&merged, "C0"), 0)),
        [100.0, 50.0]
    );
    let frame = xy(&merged, at(&merged, line_named(&merged, "G"), 0));
    assert!(frame[0].abs() < 1e-9 && frame[1].abs() < 1e-9, "{frame:?}");
}

// ===========================================================================
// Refusals: nothing written
// ===========================================================================

#[test]
fn merge_refuses_an_untrustworthy_source_and_writes_nothing() {
    let temp = TempDir::new().unwrap();
    let mut polar = georeferenced("P", "p", 20.5);
    polar.datums[0].gps.latitude = 95.0;
    let output = temp.path().join("merged.stalx");
    let polar_path = write_v2(temp.path(), "polar.stalx", &polar);
    let result = merge(
        vec![
            write_v2(temp.path(), "charlie.stalx", &source_charlie()),
            polar_path.clone(),
        ],
        &output,
        false,
    );
    assert!(!result.success);
    let label = super::project_merge::merge_source_label(&polar_path);
    assert!(
        result
            .error
            .as_deref()
            .is_some_and(|e| e.starts_with(&format!(
                "'{label}' was not merged: a zero datum has invalid GPS coordinates"
            ))),
        "{:?}",
        result.error
    );
    assert!(!output.exists(), "no output file was written");
}

/// E5: every source keeps its Station IDs, offset past the ones before it;
/// when they do not fit, the merge is refused whole.
#[test]
fn a_merge_past_the_station_id_space_is_refused() {
    let temp = TempDir::new().unwrap();
    let mut crowded = source_charlie();
    crowded.counters.next_station_id = MAX_STATION_ID - 1;
    let output = temp.path().join("merged.stalx");
    let result = merge(
        vec![
            write_v2(temp.path(), "crowded.stalx", &crowded),
            write_v2(temp.path(), "alpha.stalx", &source_alpha()),
        ],
        &output,
        false,
    );
    assert!(!result.success);
    assert!(
        result.error.as_deref().is_some_and(|e| e
            .starts_with("Merge refused: the 2 source projects use")
            && e.ends_with("No output file was written.")),
        "{:?}",
        result.error
    );
    assert!(!output.exists());
}

/// A merge refusal names the source by its whole path, cut from the front
/// when too long so the file name, which says which source failed, survives.
#[test]
fn a_long_merge_source_path_keeps_its_file_name() {
    let short = Path::new("/caves/north/cave.stalx");
    assert_eq!(
        super::project_merge::merge_source_label(short),
        "/caves/north/cave.stalx"
    );
    let long = PathBuf::from(format!("/{}/cave.stalx", "d".repeat(900)));
    let label = super::project_merge::merge_source_label(&long);
    assert!(
        label.starts_with("...") && label.ends_with("/cave.stalx"),
        "{label}"
    );
    assert!(label.chars().count() <= 480, "{}", label.chars().count());
}

// ===========================================================================
// Legacy sources
// ===========================================================================

/// A legacy source is read through a private copy, so the file (which an
/// open would migrate) is left byte for byte as it was, and the output is a
/// version 2 file.
#[test]
fn merge_leaves_a_legacy_source_untouched() {
    crate::app_dirs::init_for_test();
    let temp = TempDir::new().unwrap();
    let legacy = legacy_copy(temp.path(), "legacy.stalx");
    let before = std::fs::read(&legacy).unwrap();
    let opened = open(&legacy).get_project_state().unwrap();

    let merged = merge_files(
        temp.path(),
        vec![
            legacy.clone(),
            write_v2(temp.path(), "bravo.stalx", &source_bravo()),
        ],
    );
    assert_eq!(std::fs::read(&legacy).unwrap(), before, "source untouched");
    assert_eq!(merged.lines().len(), opened.lines().len() + 2);
    assert_valid(&merged);
}

#[test]
fn a_sources_committed_wal_pages_are_read_by_the_merge() {
    crate::app_dirs::init_for_test();
    let temp = TempDir::new().unwrap();
    let crashed = legacy_with_wal(temp.path());
    let (db, wal) = (
        std::fs::read(&crashed).unwrap(),
        std::fs::read(sidecar(&crashed, "-wal")).unwrap(),
    );
    let merged = merge_files(
        temp.path(),
        vec![
            crashed.clone(),
            write_v2(temp.path(), "charlie.stalx", &source_charlie()),
        ],
    );
    assert!(
        lengths(&merged).contains(&77.0),
        "the merge applies the WAL"
    );
    assert_eq!(std::fs::read(&crashed).unwrap(), db, "source untouched");
    assert_eq!(std::fs::read(sidecar(&crashed, "-wal")).unwrap(), wal);
}

#[test]
fn a_sources_hot_journal_is_rolled_back_by_the_merge() {
    crate::app_dirs::init_for_test();
    let temp = TempDir::new().unwrap();
    let crashed = legacy_with_hot_journal(temp.path());
    let (db, journal) = (
        std::fs::read(&crashed).unwrap(),
        std::fs::read(sidecar(&crashed, "-journal")).unwrap(),
    );
    let merged = merge_files(
        temp.path(),
        vec![
            crashed.clone(),
            write_v2(temp.path(), "charlie.stalx", &source_charlie()),
        ],
    );
    assert!(
        lengths(&merged).iter().all(|l| *l < 1000.0),
        "the merge rolls the journal back"
    );
    assert_eq!(std::fs::read(&crashed).unwrap(), db);
    assert_eq!(
        std::fs::read(sidecar(&crashed, "-journal")).unwrap(),
        journal
    );
}

/// "Archive original project files" moves a source's SQLite sidecars with
/// it: a `-wal` left behind would take its commits away from the archive.
#[test]
fn archiving_a_source_moves_its_wal_with_it() {
    crate::app_dirs::init_for_test();
    let temp = TempDir::new().unwrap();
    let crashed = legacy_with_wal(temp.path());
    let charlie = write_v2(temp.path(), "charlie.stalx", &source_charlie());
    let result = merge(
        vec![crashed.clone(), charlie.clone()],
        &temp.path().join("merged.stalx"),
        true,
    );
    assert!(result.success, "{:?}", result.error);

    let archived = temp.path().join("crashed.stalx.archived");
    assert!(
        !crashed.exists() && !sidecar(&crashed, "-wal").exists(),
        "nothing left behind"
    );
    assert!(
        sidecar(&archived, "-wal").is_file(),
        "the WAL moved with its database"
    );
    assert!(temp.path().join("charlie.stalx.archived").is_file());
    assert!(!charlie.exists());
    // The archive still reads with the WAL's commit.
    let restored = temp.path().join("restored.stalx");
    std::fs::copy(&archived, &restored).unwrap();
    std::fs::copy(sidecar(&archived, "-wal"), sidecar(&restored, "-wal")).unwrap();
    assert!(lengths(&open(&restored).get_project_state().unwrap()).contains(&77.0));
}

/// A second archive of the same name gets a timestamp instead of replacing
/// the first.
#[test]
fn an_existing_archive_is_never_replaced() {
    let temp = TempDir::new().unwrap();
    let (a, b) = two_sources(temp.path());
    let first = temp.path().join("a.stalx.archived");
    std::fs::write(&first, b"an older archive").unwrap();
    let result = merge(vec![a.clone(), b], &temp.path().join("merged.stalx"), true);
    assert!(result.success, "{:?}", result.error);
    assert_eq!(std::fs::read(&first).unwrap(), b"an older archive");
    assert!(!a.exists());
    let stamped = std::fs::read_dir(temp.path())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("a.stalx.archived.")
        })
        .count();
    assert_eq!(stamped, 1, "the new archive is timestamped");
}
