//! Tests for #674: the append shared by "Merge projects" and "Add .stalx to
//! current project". Every project here is synthetic, built in code and written
//! to a temp dir - no fixture file is read or written into the tree.

use std::path::{Path, PathBuf};

use chrono::{TimeZone, Utc};
use tempfile::TempDir;

use super::fuzz_undo_tests::{canonical_snapshot, describe_diffs, diff_snapshots};
use super::project_add::{set_add_window_hook, AddWindow};
use super::AppState;
use crate::core::{AddedContent, Command, CommandResult, UndoData, EDIT_DISABLED_CODE};
use crate::models::{
    gps_to_local, AzimuthReference, ClosureStrategy, CorrectionSource, GpsCoordinates, LoopClosure,
    Project, SectionData, SectionId, ShotData, ShotKind, SpeleoLink, StationMerge, StationRef,
    SurveyDirection, ZeroDatum, FEATURE_ID_STRIDE, MAX_SECTIONS_IN_ID_SPACE, MAX_SECTION_ID,
};
use crate::persistence::{Database, ResolvedWarning};

/// A shot with a FIXED uuid, so every output that carries shot identity is
/// deterministic across runs (a minted uuid would make the parity digest
/// below differ on every run).
fn shot(uuid: &str, azimuth: f64, length: f64, depth: f64) -> ShotData {
    ShotData {
        uuid: Some(uuid.to_string()),
        azimuth_in: azimuth,
        azimuth_out: (azimuth + 180.0) % 360.0,
        length,
        depth_in: depth,
        depth_out: depth + 0.5,
        ..Default::default()
    }
}

fn section(name: &str, shots: Vec<ShotData>) -> SectionData {
    SectionData {
        name: Some(name.to_string()),
        surveyor: Some("user1".to_string()),
        date: None,
        color: "#4ecdc4".to_string(),
        direction: Some(SurveyDirection::In),
        quality: 1.0,
        shots,
    }
}

fn fixed_time(seconds: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(1_700_000_000 + seconds, 0)
        .single()
        .unwrap_or_default()
}

fn closure(a: StationRef, b: StationRef, strategy: ClosureStrategy) -> LoopClosure {
    LoopClosure {
        id: 0,
        station_a: a.into(),
        station_b: b.into(),
        path_length: 12.5,
        misclosure_distance: 0.25,
        error_percentage: 2.0,
        horizontal_misclosure: 0.2,
        vertical_misclosure: 0.15,
        strategy,
        is_aligned: false,
    }
}

fn save_to(dir: &Path, file: &str, project: &Project) -> PathBuf {
    let path = dir.join(file);
    let mut db = Database::open(&path).unwrap();
    db.save_project(project).unwrap();
    path
}

/// Source A: a datum, an intra-project merge, a legacy id-0 closure, a project
/// default north reference AND a section that declares its own, a station
/// offset, comments, an entrance elevation and a location.
fn source_alpha() -> Project {
    let mut p = Project::new("Alpha".to_string(), Some("Yucatan".to_string()));
    p.surface_elevation = Some(12.0);
    p.default_azimuth_reference = Some(AzimuthReference::magnetic(
        -4.0,
        CorrectionSource::FileDeclared,
    ));
    let a0 = p
        .add_section(section(
            "A0",
            vec![
                shot("a0-0", 10.0, 5.0, 3.0),
                shot("a0-1", 40.0, 6.5, 3.5),
                shot("a0-2", 95.0, 4.25, 4.0),
            ],
        ))
        .id;
    let a1 = p
        .add_section(section(
            "A1",
            vec![shot("a1-0", 200.0, 7.0, 4.5), shot("a1-1", 230.0, 3.0, 5.0)],
        ))
        .id;
    let a2 = p
        .add_section(section("A2", vec![shot("a2-0", 300.0, 2.0, 6.0)]))
        .id;
    if let Some(s) = p.get_section_mut(a2) {
        s.azimuth_reference = Some(AzimuthReference::magnetic(10.0, CorrectionSource::Manual));
    }
    p.record_station_merge(StationMerge {
        station_a: StationRef::new(a0, 3).into(),
        station_b: StationRef::new(a1, 0).into(),
        merged_at: fixed_time(1),
    });
    p.section_offsets.insert(a1, (5.0, -2.0));
    p.section_offsets.insert(a2, (-30.0, 12.0));
    p.station_offsets
        .insert(StationRef::new(a0, 1).into(), (0.5, -0.5, 1.0));
    p.station_comments
        .insert(StationRef::new(a0, 2).into(), "squeeze".to_string());
    p.station_comments
        .insert(StationRef::new(a1, 0).into(), "junction".to_string());
    p.store_zero_datum_without_repositioning(
        StationRef::new(a0, 0),
        GpsCoordinates::new(20.5, -87.3, 0.0).unwrap(),
    )
    .unwrap();
    p.loop_closures.push(closure(
        StationRef::new(a0, 1),
        StationRef::new(a0, 3),
        ClosureStrategy::Batch,
    ));
    p
}

/// Source A plus a station name (#676). Separate from [`source_alpha`] so the
/// merge parity digest above stays pinned to the pre-extraction loop's input.
fn source_alpha_named() -> Project {
    let mut p = source_alpha();
    let a0 = p.sections[0].id;
    p.station_names
        .insert(StationRef::new(a0, 1).into(), "Gate".to_string());
    p
}

/// Source B: a second datum, a cross-section closure, a DISAGREEING entrance
/// elevation (the first source's must win, with a warning) and a new location.
fn source_bravo() -> Project {
    let mut p = Project::new("Bravo".to_string(), Some("Quintana Roo".to_string()));
    p.surface_elevation = Some(15.0);
    let b0 = p
        .add_section(section(
            "B0",
            vec![shot("b0-0", 120.0, 8.0, 2.0), shot("b0-1", 150.0, 9.0, 2.5)],
        ))
        .id;
    let b1 = p
        .add_section(section(
            "B1",
            vec![shot("b1-0", 60.0, 4.0, 3.0), shot("b1-1", 20.0, 5.5, 3.25)],
        ))
        .id;
    p.record_station_merge(StationMerge {
        station_a: StationRef::new(b0, 2).into(),
        station_b: StationRef::new(b1, 0).into(),
        merged_at: fixed_time(2),
    });
    p.station_comments
        .insert(StationRef::new(b1, 1).into(), "sump".to_string());
    p.store_zero_datum_without_repositioning(
        StationRef::new(b0, 0),
        GpsCoordinates::new(20.6, -87.4, 1.5).unwrap(),
    )
    .unwrap();
    p.loop_closures.push(closure(
        StationRef::new(b0, 0),
        StationRef::new(b1, 2),
        ClosureStrategy::Batch,
    ));
    p
}

/// Source C: no datum (an unanchored island), a location the merge already
/// holds (so it is NOT appended twice), and a displaced section.
fn source_charlie() -> Project {
    let mut p = Project::new("Charlie".to_string(), Some("Yucatan".to_string()));
    let c0 = p
        .add_section(section("C0", vec![shot("c0-0", 270.0, 11.0, 7.0)]))
        .id;
    p.section_offsets.insert(c0, (100.0, 50.0));
    p
}

/// FNV-1a, 64 bit. Hand-rolled on purpose: `DefaultHasher`'s algorithm is not
/// stable across toolchains, and a digest pinned in a test has to be.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Round to 9 significant digits, so a last-bit difference in a re-solved
/// offset (a summation order the cascade does not promise) cannot move the
/// digest while any real change still does.
fn canonical_number(value: f64) -> serde_json::Value {
    if !value.is_finite() || value == 0.0 {
        return serde_json::Value::from(value);
    }
    let rounded = format!("{value:.8e}").parse::<f64>().unwrap_or(value);
    serde_json::Value::from(rounded)
}

fn canonicalize(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), canonicalize(v)))
                .collect(),
        ),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(canonicalize).collect())
        }
        serde_json::Value::Number(n) => n.as_f64().map_or_else(|| value.clone(), canonical_number),
        other => other.clone(),
    }
}

/// The merge's observable output as one comparable string: the project as
/// PERSISTED (reloaded raw, no load-time heal) plus the result's warnings.
///
/// Dropped, each because it is not the merge's doing: `created_at` /
/// `modified_at` (wall clock), `file_path` (the temp dir), and `validation`
/// (seeded from the host's global config, which is not the merge's input).
/// `station_offsets` / `station_comments` serialize from a `HashMap`, so they
/// are sorted.
fn canonical_merge_output(merged: &Project, result: &CommandResult) -> String {
    let mut value = serde_json::to_value(merged).unwrap();
    if let Some(map) = value.as_object_mut() {
        for key in ["created_at", "modified_at", "file_path", "validation"] {
            map.remove(key);
        }
        for key in ["station_offsets", "station_comments"] {
            if let Some(serde_json::Value::Array(items)) = map.get_mut(key) {
                items.sort_by_key(ToString::to_string);
            }
        }
    }
    let warnings: Vec<String> = result
        .warnings
        .iter()
        .map(|w| format!("{}: {}", w.code, w.message))
        .collect();
    format!(
        "{}\nsuccess={} warnings={warnings:?}",
        canonicalize(&value),
        result.success
    )
}

/// The digest of [`canonical_merge_output`] for the three sources above,
/// recorded on the merge loop as it stood BEFORE #674 extracted it into the
/// shared append. If this fires, merge output changed: diff the printed
/// canonical form against the previous one, and only re-pin if the change is
/// intended. Re-pinned once when #676 added `station_names` to the project
/// (an empty list in this canonical form); master's pre-extraction merge was
/// run against the new value and produced it.
const MERGE_PARITY_DIGEST: u64 = 9_039_697_118_154_861_914;

/// #674 parity: "Merge projects" must produce exactly what it produced before
/// its per-source loop became the append that "Add .stalx" shares. Rich on
/// purpose - two datums, merges, a legacy id-0 closure, both north-reference
/// routes, offsets, comments, a disagreeing elevation and a repeated location -
/// so every branch of the loop contributes to the digest.
#[test]
fn merge_output_is_unchanged_by_the_append_extraction() {
    let temp = TempDir::new().unwrap();
    let sources = vec![
        save_to(temp.path(), "alpha.stalx", &source_alpha()),
        save_to(temp.path(), "bravo.stalx", &source_bravo()),
        save_to(temp.path(), "charlie.stalx", &source_charlie()),
    ];
    let state = AppState::new().unwrap();
    let output = temp.path().join("merged.stalx");
    let result = state.execute(Command::MergeProjects {
        source_paths: sources,
        output_path: output.clone(),
        output_name: "Parity".to_string(),
        archive_originals: false,
    });
    assert!(result.success, "merge failed: {:?}", result.error);

    let merged = Database::open_existing(&output)
        .unwrap()
        .load_project()
        .unwrap();
    assert_eq!(merged.sections.len(), 6, "every source section is merged");
    let canonical = canonical_merge_output(&merged, &result);
    assert_eq!(
        fnv1a64(canonical.as_bytes()),
        MERGE_PARITY_DIGEST,
        "merge output changed; canonical form:\n{canonical}"
    );
    // Not vacuous: the pinned form really carries what the loop copies.
    for needle in [
        "SURFACE_ELEVATION_MISMATCH",
        "Yucatan, Quintana Roo",
        "squeeze",
        "junction",
        "sump",
    ] {
        assert!(canonical.contains(needle), "canonical form lacks {needle}");
    }
    // The two datums survive with their GPS, the closures with fresh ids.
    assert_eq!(merged.zero_datums.len(), 2);
    assert_eq!(merged.loop_closures.len(), 2);
    assert!(merged.loop_closures.iter().all(|c| c.id != 0));
    assert_eq!(merged.station_merges.len(), 2);
}

// ===========================================================================
// Add .stalx to current project
// ===========================================================================

const TARGET_FRAME: (f64, f64) = (20.55, -87.35);

/// The OPEN project every add test adds into: georeferenced, with one earlier
/// undoable edit on its stack, and saved THROUGH THE APP so the save-time heals
/// have already happened (a later save/reload then changes nothing of its own).
fn open_target(dir: &Path, georeferenced: bool) -> (AppState, PathBuf) {
    let mut target = Project::new("Target".to_string(), Some("Mexico".to_string()));
    let t0 = target
        .add_section(section(
            "T0",
            vec![shot("t0-0", 0.0, 10.0, 1.0), shot("t0-1", 90.0, 8.0, 1.5)],
        ))
        .id;
    if georeferenced {
        target
            .store_zero_datum_without_repositioning(
                StationRef::new(t0, 0),
                GpsCoordinates::new(TARGET_FRAME.0, TARGET_FRAME.1, 0.0).unwrap(),
            )
            .unwrap();
    }
    let path = save_to(dir, "target.stalx", &target);
    let state = AppState::new().unwrap();
    assert!(state.open_project(path.clone()).success, "open the target");
    let created = state.execute(Command::CreateSection {
        name: Some("T1".to_string()),
        surveyor: None,
        date: None,
        color: "#ff0000".to_string(),
        direction: None,
        shots: vec![shot("ignored-by-create", 180.0, 6.0, 2.0)],
    });
    assert!(created.success, "an earlier edit for the stack");
    save(&state);
    (state, path)
}

fn save(state: &AppState) {
    let saved = state.execute(Command::SaveProject { path: None });
    assert!(saved.success, "save failed: {:?}", saved.error);
}

fn reopen(state: &AppState, path: &Path) {
    assert!(state.execute(Command::CloseProject).success);
    assert!(state.open_project(path.to_path_buf()).success, "reopen");
}

fn add(state: &AppState, path: &Path) -> CommandResult {
    state.execute(Command::add_project_file(path.to_path_buf()))
}

fn assert_same_project(state: &AppState, expected: &str, id_agnostic: bool, what: &str) {
    let diffs = diff_snapshots(expected, &canonical_snapshot(state, id_agnostic));
    assert!(diffs.is_empty(), "{what}:\n{}", describe_diffs(&diffs));
}

fn undo_depth(state: &AppState) -> usize {
    state.get_undo_stack().len()
}

fn receipt_warnings(result: &CommandResult) -> Vec<String> {
    result
        .import_receipt
        .as_ref()
        .expect("an add always carries a receipt")
        .warnings
        .clone()
}

/// INJECTION (tried): dropping the `location` restore from
/// `AddedContent::remove_from` fails the exactness assertion below on
/// `location` ("Mexico, Yucatan" vs "Mexico"); dropping the `station_merges`
/// retain fails it on the dangling merge; dropping the `station_names` retain
/// fails it on the name left on a removed section.
#[test]
fn add_then_undo_restores_the_project_exactly_and_keeps_older_history() {
    let temp = TempDir::new().unwrap();
    let (state, _) = open_target(temp.path(), true);
    let alpha = save_to(temp.path(), "alpha.stalx", &source_alpha_named());
    let before = canonical_snapshot(&state, false);
    let depth = undo_depth(&state);

    let result = add(&state, &alpha);
    assert!(result.success, "add failed: {:?}", result.error);
    assert_eq!(result.section_ids.len(), 3, "exactly the added sections");
    assert_eq!(undo_depth(&state), depth + 1, "the add is ONE undo step");
    assert_ne!(canonical_snapshot(&state, false), before);

    // The step records the add, not the project.
    {
        let stack = state.undo_stack.lock().unwrap();
        let Some(UndoData::AddedProjectContent(content)) = stack.peek_undo_data() else {
            panic!("the add must record AddedProjectContent");
        };
        let mut recorded: Vec<SectionId> = content.sections.iter().map(|s| s.id).collect();
        recorded.sort_unstable();
        let mut created = result.section_ids;
        created.sort_unstable();
        assert_eq!(
            recorded, created,
            "only the add's own sections are recorded"
        );
        assert_eq!(content.zero_datums.len(), 1);
        assert_eq!(content.station_merges.len(), 1);
        assert_eq!(content.loop_closures.len(), 1);
        assert_eq!(content.station_comments.len(), 2);
        assert_eq!(content.station_names.len(), 1);
        assert!(
            content.baseline.is_none(),
            "the stub baseline never persists"
        );
    }

    assert!(state.undo().success);
    assert_same_project(
        &state,
        &before,
        false,
        "undo of an add must restore the project",
    );
    assert_eq!(undo_depth(&state), depth, "the earlier history is intact");
    assert!(
        state.undo().success,
        "the edit made before the add still undoes"
    );
}

/// INJECTION (tried): letting the redo fall through to the catch-all, which
/// re-runs the add, mints fresh section and closure ids and fails below;
/// dropping the `station_names` reinstall in `install_into` fails it on the
/// missing name.
#[test]
fn redo_of_an_add_reproduces_the_same_ids() {
    let temp = TempDir::new().unwrap();
    let (state, _) = open_target(temp.path(), true);
    let alpha = save_to(temp.path(), "alpha.stalx", &source_alpha_named());

    let result = add(&state, &alpha);
    assert!(result.success);
    let after = canonical_snapshot(&state, false);
    let closure_ids: Vec<u64> = state
        .get_project_state()
        .unwrap()
        .loop_closures
        .iter()
        .map(|c| c.id)
        .collect();

    assert!(state.undo().success);
    assert!(state.redo().success);
    // Not id-agnostic: section ids, shot ids, uuids and closure ids all match.
    assert_same_project(
        &state,
        &after,
        false,
        "redo must reinstall the same content",
    );
    let project = state.get_project_state().unwrap();
    for id in &result.section_ids {
        assert!(project.get_section(*id).is_some(), "section {id} is back");
    }
    let redone: Vec<u64> = project.loop_closures.iter().map(|c| c.id).collect();
    assert_eq!(redone, closure_ids);
}

/// The step is persisted with the project and still undoes, then redoes, after
/// a save and a reopen - where the command no longer even names the file.
/// INJECTION (tried): the catch-all redo fails here too - after a reload the
/// command's path is empty, so a re-run cannot even find the file.
#[test]
fn add_undo_and_redo_survive_save_and_reload() {
    let temp = TempDir::new().unwrap();
    let (state, path) = open_target(temp.path(), true);
    let alpha = save_to(temp.path(), "alpha.stalx", &source_alpha_named());
    let before = canonical_snapshot(&state, true);

    assert!(add(&state, &alpha).success);
    save(&state);
    let after = canonical_snapshot(&state, true);
    assert!(after.contains("\"Gate\""), "the station name was added");
    // The source is gone: neither undo nor redo may need it.
    std::fs::remove_file(&alpha).unwrap();

    reopen(&state, &path);
    assert_same_project(&state, &after, true, "reload changes nothing");
    assert!(state.undo().success, "undo after reload");
    assert_same_project(&state, &before, true, "undo after reload restores");
    assert!(state.redo().success, "redo after reload");
    assert_same_project(&state, &after, true, "redo after reload reinstalls");
}

#[test]
fn read_only_gate_refuses_an_add_before_reading_the_file() {
    let temp = TempDir::new().unwrap();
    // A SpeleoDB project opened without its lock is the read-only state the
    // backend gate enforces; a local project is always editable.
    let target = save_to(temp.path(), "remote.stalx", &source_bravo());
    Database::open_existing(&target)
        .unwrap()
        .save_speleo_link(Some(&linked("Remote Cave")))
        .unwrap();
    let state = AppState::new().unwrap();
    assert!(state.open_project(target).success, "open the linked target");
    let alpha = save_to(temp.path(), "alpha.stalx", &source_alpha());
    let before = canonical_snapshot(&state, false);
    let depth = undo_depth(&state);

    let refused = add(&state, &alpha);
    assert!(!refused.success);
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|e| e.starts_with(EDIT_DISABLED_CODE)),
        "{:?}",
        refused.error
    );
    assert_same_project(&state, &before, false, "a refused add changes nothing");
    assert_eq!(undo_depth(&state), depth);
}

/// The source with ignorable warnings and their ignores: one keyed by uuid, one
/// legacy row keyed by position only.
fn source_with_ignores(dir: &Path) -> PathBuf {
    let mut p = Project::new("Ignored".to_string(), None);
    let s = p
        .add_section(section(
            "I0",
            vec![
                shot("i0-0", 10.0, 4.0, 1.0),
                shot("i0-1", 20.0, 250.0, 1.0),
                shot("i0-2", 30.0, 300.0, 1.0),
            ],
        ))
        .id;
    let path = save_to(dir, "ignored.stalx", &p);
    let db = Database::open_existing(&path).unwrap();
    for (seq, uuid) in [(1, Some("i0-1")), (2, None)] {
        db.save_resolved_warning(&ResolvedWarning {
            section_id: s,
            shot_id: s * FEATURE_ID_STRIDE + seq,
            warning_code: "LONG_SHOT".to_string(),
            resolved_at: fixed_time(3),
            resolved_comment: Some("measured twice".to_string()),
            shot_uuid: uuid.map(str::to_string),
        })
        .unwrap();
    }
    path
}

fn long_shot_verdicts(state: &AppState, section_ids: &[SectionId]) -> Vec<bool> {
    let project = state.get_project_state().unwrap();
    state
        .compute_project_warnings(&project, None)
        .into_iter()
        .filter(|w| w.code == "LONG_SHOT" && section_ids.contains(&w.section_id))
        .map(|w| w.is_resolved)
        .collect()
}

/// INJECTION (tried): skipping `add_pending_ignores` in `add_project_file`
/// fails the first assertion; dropping the `pending_ignores()` extend from the
/// save's carry-over fails the post-reload one.
#[test]
fn ignored_warnings_survive_the_add_a_save_and_a_reload() {
    let temp = TempDir::new().unwrap();
    let (state, path) = open_target(temp.path(), true);
    let source = source_with_ignores(temp.path());

    let result = add(&state, &source);
    assert!(result.success, "{:?}", result.error);
    let ids = result.section_ids;
    assert_eq!(
        long_shot_verdicts(&state, &ids),
        vec![true, true],
        "both carried ignores are honored before any save"
    );

    save(&state);
    reopen(&state, &path);
    assert_eq!(
        long_shot_verdicts(&state, &ids),
        vec![true, true],
        "the save wrote them into the open file"
    );

    assert!(state.undo().success);
    assert!(long_shot_verdicts(&state, &ids).is_empty());
    assert!(state.redo().success);
    assert_eq!(long_shot_verdicts(&state, &ids), vec![true, true]);
}

fn linked(project_name: &str) -> SpeleoLink {
    SpeleoLink {
        endpoint: "http://speleodb.invalid/".to_string(),
        project_id: "22222222-2222-2222-2222-222222222222".to_string(),
        project_name: project_name.to_string(),
        base_commit_sha: Some("def456".to_string()),
        has_unpushed_edits: false,
        last_edit_at: None,
        last_verify_at: None,
        undo_mode: None,
    }
}

/// INJECTION (tried): dropping the `source.link` branch in `add_receipt` fails
/// the receipt assertion. The byte comparison guards the private copy; it can
/// only fire on an older-schema source (which an open would migrate), so it is
/// a tripwire here rather than a proven guard.
#[test]
fn linked_source_copies_data_but_never_the_link() {
    let temp = TempDir::new().unwrap();
    let (state, path) = open_target(temp.path(), true);
    let source = save_to(temp.path(), "charlie.stalx", &source_charlie());
    Database::open_existing(&source)
        .unwrap()
        .save_speleo_link(Some(&linked("Upstream Survey")))
        .unwrap();
    let source_bytes = std::fs::read(&source).unwrap();

    let result = add(&state, &source);
    assert!(
        result.success,
        "a linked source is not refused: {:?}",
        result.error
    );
    assert_eq!(result.section_ids.len(), 1, "its survey data was copied");
    assert!(
        receipt_warnings(&result).iter().any(|w| w
            == "'charlie.stalx' is linked to SpeleoDB project 'Upstream Survey'; its data \
                was copied, that project is unchanged"),
        "{:?}",
        receipt_warnings(&result)
    );
    assert_eq!(
        std::fs::read(&source).unwrap(),
        source_bytes,
        "the source file is untouched"
    );

    save(&state);
    let link = Database::open_existing(&path)
        .unwrap()
        .load_speleo_link()
        .unwrap();
    assert!(link.is_none(), "the open project did not acquire the link");
}

/// INJECTION (tried): disabling the `imported_file_hashes.contains(&identity)`
/// check fails the second-add assertion.
#[test]
fn adding_the_same_stalx_twice_adds_again_with_a_warning() {
    let temp = TempDir::new().unwrap();
    let (state, path) = open_target(temp.path(), true);
    let alpha = save_to(temp.path(), "alpha.stalx", &source_alpha());

    let first = add(&state, &alpha);
    assert!(first.success);
    assert!(
        !receipt_warnings(&first)
            .iter()
            .any(|w| w.contains("already added")),
        "the first add is not a duplicate"
    );

    // Across a save and a reopen: the identity rides the dedup set.
    save(&state);
    reopen(&state, &path);
    let second = add(&state, &alpha);
    assert!(second.success, "a duplicate is still added");
    assert_eq!(second.section_ids.len(), 3);
    assert!(
        receipt_warnings(&second).iter().any(|w| w
            == "'alpha.stalx' was already added to this project; its sections were added again"),
        "{:?}",
        receipt_warnings(&second)
    );

    // A copy of the open project itself is the same survey a second time.
    let copy = temp.path().join("target-copy.stalx");
    std::fs::copy(&path, &copy).unwrap();
    let third = add(&state, &copy);
    assert!(third.success);
    assert!(receipt_warnings(&third)
        .iter()
        .any(|w| w.contains("is a copy of the open project")));
}

#[test]
fn an_add_is_refused_with_nothing_changed() {
    let temp = TempDir::new().unwrap();
    let alpha = save_to(temp.path(), "alpha.stalx", &source_alpha());

    let nothing_open = AppState::new().unwrap();
    let refused = add(&nothing_open, &alpha);
    assert!(!refused.success);
    assert!(nothing_open.get_project_state().is_none());

    let (state, path) = open_target(temp.path(), true);
    let before = canonical_snapshot(&state, false);
    let depth = undo_depth(&state);
    let garbage = temp.path().join("garbage.stalx");
    std::fs::write(&garbage, b"this is not a sqlite database").unwrap();
    for (what, target) in [
        ("the open project's own file", path),
        ("a file that is not a project", garbage),
        (
            "a file that does not exist",
            temp.path().join("missing.stalx"),
        ),
    ] {
        let refused = add(&state, &target);
        assert!(!refused.success, "{what} must be refused");
        assert!(refused.import_receipt.is_none());
        assert_same_project(&state, &before, false, what);
        assert_eq!(undo_depth(&state), depth, "{what} pushed nothing");
    }
}

/// The section-id budget (#555) is checked before a single section is copied.
/// INJECTION (not tried): without the check, `Section::new`'s unchecked
/// `id * FEATURE_ID_STRIDE` panics in debug on the second copied section.
#[test]
fn an_add_past_the_section_id_budget_is_refused() {
    let temp = TempDir::new().unwrap();
    let (state, _) = open_target(temp.path(), true);
    let alpha = save_to(temp.path(), "alpha.stalx", &source_alpha());
    if let Some(project) = state.project.lock().unwrap().as_mut() {
        project.next_section_id = MAX_SECTION_ID;
    }
    let before = canonical_snapshot(&state, false);

    let refused = add(&state, &alpha);
    assert!(!refused.success);
    assert!(refused
        .error
        .as_deref()
        .is_some_and(|e| e.starts_with("Add refused")));
    assert_same_project(&state, &before, false, "nothing was appended");
}

/// The orphan-placement signal is `CommandResult::section_ids` holding exactly
/// the added sections; the dialog's own queries then find the unanchored ones.
#[test]
fn added_sections_are_reported_for_orphan_placement() {
    let temp = TempDir::new().unwrap();
    let (state, _) = open_target(temp.path(), true);
    let charlie = save_to(temp.path(), "charlie.stalx", &source_charlie());
    let alpha = save_to(temp.path(), "alpha.stalx", &source_alpha());

    let island = add(&state, &charlie);
    assert!(island.success);
    let project = state.get_project_state().unwrap();
    assert_eq!(project.count_unanchored_sections(&island.section_ids), 1);

    let anchored = add(&state, &alpha);
    let project = state.get_project_state().unwrap();
    // A0 + A1 hang off Alpha's datum; A2 is a free island inside Alpha.
    assert_eq!(project.count_unanchored_sections(&anchored.section_ids), 1);
}

/// The leg both sides of the overlap test below share: distinct bearings and
/// lengths so the matcher has a real shape to align.
const OVERLAP_LEG: [(f64, f64); 4] = [(45.0, 10.0), (90.0, 8.0), (135.0, 6.0), (180.0, 4.0)];

fn overlap_leg(prefix: &str) -> Vec<ShotData> {
    OVERLAP_LEG
        .iter()
        .enumerate()
        .map(|(i, &(azimuth, length))| shot(&format!("{prefix}-{i}"), azimuth, length, 2.0))
        .collect()
}

/// #674 end to end: a datum-less `.stalx` that re-surveys a passage of the
/// open, anchored project is added through the real command, and the
/// suggester - asked exactly as the frontend asks, with the add's returned ids
/// and the live config - offers a placement of the added section onto the
/// host. Before the fix it returned nothing: its targets came from the add
/// alone, and a datum-less add has no anchored section to offer.
#[test]
fn a_datum_less_add_overlapping_the_host_gets_a_placement_suggestion() {
    let temp = TempDir::new().unwrap();
    let mut host = Project::new("Host".to_string(), None);
    let h0 = host.add_section(section("H0", overlap_leg("h0"))).id;
    host.store_zero_datum_without_repositioning(
        StationRef::new(h0, 0),
        GpsCoordinates::new(TARGET_FRAME.0, TARGET_FRAME.1, 0.0).unwrap(),
    )
    .unwrap();
    let host_path = save_to(temp.path(), "host.stalx", &host);
    let state = AppState::new().unwrap();
    assert!(state.open_project(host_path).success, "open the host");
    let mut source = Project::new("Resurvey".to_string(), None);
    source.add_section(section("R0", overlap_leg("r0")));
    let source_path = save_to(temp.path(), "resurvey.stalx", &source);

    let result = add(&state, &source_path);
    assert!(result.success, "add failed: {:?}", result.error);
    assert_eq!(result.section_ids.len(), 1);
    let added = result.section_ids[0];
    let project = state.get_project_state().unwrap();
    let host_id = project.sections[0].id;
    let suggestions = project.suggest_orphan_placements(
        &result.section_ids,
        &state.get_config().placement_suggestion,
    );

    assert!(
        suggestions
            .iter()
            .any(|s| s.orphan_section_id == added && s.target_section_id == host_id),
        "the added section must be offered a placement onto the host: {suggestions:?}"
    );
}

/// A georeferenced source lands at its GPS position in the open project's
/// frame, rigidly, and nothing that was already there moves.
#[test]
fn a_georeferenced_source_is_placed_by_its_datum() {
    let temp = TempDir::new().unwrap();
    let (state, _) = open_target(temp.path(), true);
    let alpha = save_to(temp.path(), "alpha.stalx", &source_alpha());
    let before = state.get_project_state().unwrap();

    let result = add(&state, &alpha);
    assert!(result.success);
    let project = state.get_project_state().unwrap();
    let datum_station = StationRef::new(result.section_ids[0], 0);
    let at = project.compute_station_raw_coords(datum_station).unwrap();
    let expected = gps_to_local(
        &GpsCoordinates::new(20.5, -87.3, 0.0).unwrap(),
        &GpsCoordinates::new(TARGET_FRAME.0, TARGET_FRAME.1, 0.0).unwrap(),
    );
    assert!(
        (at.x - expected.0).abs() < 1e-6 && (at.y - expected.1).abs() < 1e-6,
        "datum station at ({}, {}), expected {expected:?}",
        at.x,
        at.y
    );
    for section in &before.sections {
        assert_eq!(
            project.section_offsets.get(&section.id),
            before.section_offsets.get(&section.id),
            "pre-existing section {} must not move",
            section.id
        );
    }
}

/// Into a project with NO frame, the source's datum becomes the frame (its
/// station on the origin) and the open project's free sections stay put.
#[test]
fn a_source_datum_frames_an_unreferenced_project_without_moving_it() {
    let temp = TempDir::new().unwrap();
    let (state, _) = open_target(temp.path(), false);
    let alpha = save_to(temp.path(), "alpha.stalx", &source_alpha());
    let before = state.get_project_state().unwrap();

    let result = add(&state, &alpha);
    assert!(result.success);
    let project = state.get_project_state().unwrap();
    let at = project
        .compute_station_raw_coords(StationRef::new(result.section_ids[0], 0))
        .unwrap();
    assert!(
        at.x.abs() < 1e-6 && at.y.abs() < 1e-6,
        "({}, {})",
        at.x,
        at.y
    );
    for section in &before.sections {
        assert_eq!(
            project.section_offsets.get(&section.id),
            before.section_offsets.get(&section.id)
        );
    }
}

/// Undo history rides to SpeleoDB in Full mode, so the command's serialized
/// form must not carry the local path - only the file's name.
#[test]
fn the_add_command_never_serializes_its_path() {
    let private = PathBuf::from("/home/user1/private/survey.stalx");
    let command = Command::add_project_file(private);
    let json = serde_json::to_string(&command).unwrap();
    assert!(!json.contains("/home/user1"), "{json}");
    assert!(json.contains("survey.stalx"));
    assert!(!AppState::is_batch_allowed(&command));
    assert_eq!(command.description(), "Add 'survey.stalx' to project");
}

/// Open `bravo` into `state` from inside `window` of the next add, and return
/// what Bravo looks like opened on its own.
fn swap_in_during(window: AddWindow, bravo: PathBuf) -> String {
    let expected = {
        let reference = AppState::new().unwrap();
        assert!(reference.open_project(bravo.clone()).success);
        canonical_snapshot(&reference, false)
    };
    set_add_window_hook(window, move |state| {
        assert!(
            state.open_project(bravo).success,
            "commit another project mid-add"
        );
    });
    expected
}

/// A project load does not take `command_lock`, so another project can be
/// committed into the slot at any point the add holds no `project` guard:
/// after `execute` captured the undo stub, while the source file is read, and
/// after the append before the undo push. Each time the add must leave the
/// project that is open now untouched and push no undo entry - an add landing
/// in the successor, or a record measured on it, would be paired with the
/// first project's baseline, and its undo would delete the successor's own
/// sections.
/// INJECTION (tried): binding the forward step to the generation its first
/// lock reads instead of the stub's (the code before this test) appends Alpha
/// into Bravo in the `BeforeFirstLock` case - the push check then reports a
/// failure, but Bravo is already changed; dropping the re-check under the
/// second lock does the same in the `DuringRead` case; dropping the generation
/// check in `push_add_undo` reports success in the `BeforePush` case, with an
/// entry on Bravo's stack that records Bravo's own sections as added.
#[test]
fn an_add_is_refused_when_another_project_is_opened_mid_command() {
    for window in [
        AddWindow::BeforeFirstLock,
        AddWindow::DuringRead,
        AddWindow::BeforePush,
    ] {
        let temp = TempDir::new().unwrap();
        let (state, _) = open_target(temp.path(), true);
        let alpha = save_to(temp.path(), "alpha.stalx", &source_alpha());
        let bravo = save_to(temp.path(), "bravo.stalx", &source_bravo());
        let expected = swap_in_during(window, bravo);

        let refused = add(&state, &alpha);
        assert!(
            !refused.success,
            "{window:?}: the add must not land in Bravo"
        );
        assert!(
            refused
                .error
                .as_deref()
                .is_some_and(|e| e.contains("changed while 'alpha.stalx' was being added")),
            "{window:?}: {:?}",
            refused.error
        );
        assert_same_project(&state, &expected, false, "the new project is untouched");
        assert_eq!(
            undo_depth(&state),
            0,
            "{window:?}: no undo entry was pushed"
        );
    }
}

/// The source is read under `command_lock`, so it must be a regular file of
/// bounded size: a directory (or a FIFO/device) named `*.stalx` is refused
/// from its metadata alone, and so is a file past the SpeleoDB download
/// ceiling. The oversized file is sparse, so this costs no disk.
/// INJECTION (tried): dropping the `is_file` check fails the directory case
/// on its message (the read then fails as "Failed to read"); dropping the
/// metadata length check fails the oversized case on its message (the bounded
/// copy still refuses it, as "grew past", after copying the whole ceiling).
/// That bounded-copy refusal itself needs a file growing mid-read and is not
/// exercised here.
#[test]
fn a_non_regular_or_oversized_source_is_refused() {
    let temp = TempDir::new().unwrap();
    let (state, _) = open_target(temp.path(), true);
    let before = canonical_snapshot(&state, false);
    let depth = undo_depth(&state);

    let directory = temp.path().join("folder.stalx");
    std::fs::create_dir(&directory).unwrap();
    let oversized = temp.path().join("huge.stalx");
    std::fs::File::create(&oversized)
        .unwrap()
        .set_len(crate::speleodb::client::NATIVE_LIMIT + 1)
        .unwrap();

    for (path, expected) in [
        (&directory, "'folder.stalx' is not a project file"),
        (&oversized, "'huge.stalx' is larger than 512 MB"),
    ] {
        let refused = add(&state, path);
        assert!(!refused.success, "{expected}");
        assert!(
            refused
                .error
                .as_deref()
                .is_some_and(|e| e.starts_with(expected)),
            "{:?}",
            refused.error
        );
        assert_same_project(&state, &before, false, expected);
        assert_eq!(undo_depth(&state), depth);
    }
}

/// The file reader builds datum GPS without validation and reads offsets as
/// stored, so a hostile source can carry an infinite or out-of-range datum or a
/// non-finite offset. The add refuses it up front, naming the problem, with the
/// open project unchanged. (`9e999` is how SQL spells an infinite REAL.)
/// INJECTION (tried): skipping `refuse_non_finite_placement` fails every case
/// on its message - the two datum cases are then refused late, by the append's
/// own datum store, with no word of the damage; the station-offset case is
/// ADDED, carrying the infinity into the open project.
#[test]
fn a_source_with_non_finite_or_invalid_placement_is_refused() {
    let temp = TempDir::new().unwrap();
    let (state, _) = open_target(temp.path(), true);
    let before = canonical_snapshot(&state, false);
    let depth = undo_depth(&state);

    for (file, sql, expected) in [
        (
            "infinite-datum.stalx",
            "UPDATE zero_datums SET latitude = 9e999",
            "a zero datum has invalid GPS coordinates (Latitude must be finite",
        ),
        (
            "polar-datum.stalx",
            "UPDATE zero_datums SET latitude = 95.0",
            "a zero datum has invalid GPS coordinates (Latitude must be in [-90, 90]",
        ),
        (
            "infinite-offset.stalx",
            "UPDATE station_offsets SET offset_x = 9e999",
            "a station offset is not a finite number",
        ),
    ] {
        let path = save_to(temp.path(), file, &source_alpha());
        let changed = rusqlite::Connection::open(&path)
            .unwrap()
            .execute(sql, [])
            .unwrap();
        assert_eq!(changed, 1, "{file}: the corruption landed");

        let refused = add(&state, &path);
        assert!(!refused.success, "{file} must be refused");
        assert!(
            refused.error.as_deref().is_some_and(|e| e
                .starts_with(&format!("'{file}' was not added: "))
                && e.contains(expected)),
            "{file}: {:?}",
            refused.error
        );
        assert_same_project(&state, &before, false, file);
        assert_eq!(undo_depth(&state), depth, "{file} pushed nothing");
    }
}

/// The add's undo entry is persisted with the file, so redo reads it back like
/// any other input: a record no add could have produced is refused with the
/// project unchanged, and the id allocators saturate rather than overflow.
/// INJECTION (tried): dropping the duplicate-id check, or the `MAX_SECTION_ID`
/// check, makes that install succeed and fails the refusal assertion;
/// reverting to `max + 1` on the closure id panics on overflow in this debug
/// build.
#[test]
fn redo_refuses_a_record_no_add_could_have_produced() {
    let mut project = source_bravo();
    let unchanged = serde_json::to_value(&project).unwrap();
    let mut one = project.sections[0].clone();
    one.id = 100;

    let mut duplicate = AddedContent::default();
    duplicate.sections = vec![one.clone(), one.clone()];
    let mut past_id_space = AddedContent::default();
    let mut far = one.clone();
    far.id = MAX_SECTION_ID + 1;
    past_id_space.sections = vec![far];

    for (what, content, expected) in [
        ("a duplicated section", &duplicate, "names a section twice"),
        (
            "a section past the id space",
            &past_id_space,
            "past the largest section id",
        ),
    ] {
        let refused = content.install_into(&mut project);
        assert!(
            refused.as_ref().is_err_and(|e| e.contains(expected)),
            "{what}: {refused:?}"
        );
        assert_eq!(
            serde_json::to_value(&project).unwrap(),
            unchanged,
            "{what}: nothing changed"
        );
    }

    // At the edges of both id spaces: installed, allocators pinned at the top.
    let mut edge = AddedContent::default();
    let mut last = one;
    last.id = MAX_SECTION_ID;
    edge.sections = vec![last];
    let mut top = closure(
        StationRef::new(MAX_SECTION_ID, 0),
        StationRef::new(MAX_SECTION_ID, 1),
        ClosureStrategy::Batch,
    );
    top.id = u64::MAX;
    edge.loop_closures = vec![top];
    edge.install_into(&mut project).unwrap();
    assert_eq!(project.next_section_id, MAX_SECTION_ID + 1);
    assert_eq!(project.next_closure_id, u64::MAX);
}

/// A source refused AFTER the append ran is taken out again by the undo's own
/// removal, leaving the project exactly as it was: through the append's error
/// (two datums on one connected survey, a real file) and through the
/// post-append integrity guard (an infinite shot length - no loadable file is
/// known to reach that guard, see `append_in_memory`, so the source is handed
/// over in memory). The target already holds an earlier add's closure, merge
/// and datum, so the `assign_closure_ids` re-measure and the
/// `reanchor_station_refs` pass that run before the integrity check meet
/// pre-existing state: on a settled project both are no-ops, and this pins it.
/// INJECTION (tried): skipping `roll_back` on the append-error branch fails the
/// file case's exactness assertion (the copied sections stay); skipping it on
/// the integrity branch fails the in-memory case's.
#[test]
fn a_source_refused_after_the_append_is_rolled_back_exactly() {
    let temp = TempDir::new().unwrap();
    let (state, _) = open_target(temp.path(), true);
    let alpha = save_to(temp.path(), "alpha.stalx", &source_alpha_named());
    assert!(
        add(&state, &alpha).success,
        "an earlier add to roll back onto"
    );
    let before = canonical_snapshot(&state, false);
    let depth = undo_depth(&state);

    let mut two_datums = source_bravo();
    let b0 = two_datums.sections[0].id;
    two_datums.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(b0, 2).into(),
        gps: GpsCoordinates::new(20.7, -87.5, 0.0).unwrap(),
    });
    let two_datums = save_to(temp.path(), "two-datums.stalx", &two_datums);
    let refused = add(&state, &two_datums);
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|e| e.starts_with("Failed to copy zero datum from source project Bravo")),
        "{:?}",
        refused.error
    );
    assert_same_project(&state, &before, false, "the append error rolled back");
    assert_eq!(undo_depth(&state), depth, "the refused add pushed nothing");

    let mut infinite = Project::new("Infinite".to_string(), None);
    infinite.add_section(section("X0", vec![shot("x0-0", 10.0, f64::INFINITY, 1.0)]));
    let blocked = {
        let mut guard = state.project.lock().unwrap();
        super::project_add::append_in_memory(guard.as_mut().unwrap(), infinite)
    };
    assert!(
        blocked
            .as_ref()
            .is_err_and(|e| e.starts_with("Blocked: the file's survey has integrity problems")),
        "{blocked:?}"
    );
    assert_same_project(&state, &before, false, "the integrity block rolled back");
}

/// The section-id budget's boundary: an add that uses the id space up to its
/// last id exactly is accepted, one more section is refused.
/// INJECTION (tried): `>=` for `>` in the budget check refuses the exact fit;
/// `> MAX_SECTIONS_IN_ID_SPACE + 1` accepts the one-over case (a section id
/// past `MAX_SECTION_ID`), and both fail the success assertion.
#[test]
fn the_section_id_budget_admits_an_exact_fit_and_refuses_one_more() {
    let temp = TempDir::new().unwrap();
    let alpha = save_to(temp.path(), "alpha.stalx", &source_alpha());
    let incoming = source_alpha().sections.len() as u32;

    for (spent, fits) in [
        (MAX_SECTIONS_IN_ID_SPACE as u32 - incoming, true),
        (MAX_SECTIONS_IN_ID_SPACE as u32 - incoming + 1, false),
    ] {
        let dir = TempDir::new().unwrap();
        let (state, _) = open_target(dir.path(), true);
        if let Some(project) = state.project.lock().unwrap().as_mut() {
            project.next_section_id = spent;
        }
        let before = canonical_snapshot(&state, false);

        let result = add(&state, &alpha);
        assert_eq!(result.success, fits, "spent {spent}: {:?}", result.error);
        if fits {
            assert_eq!(result.section_ids.iter().max(), Some(&MAX_SECTION_ID));
        } else {
            assert!(result
                .error
                .as_deref()
                .is_some_and(|e| e.starts_with("Add refused")));
            assert_same_project(&state, &before, false, "nothing was appended");
        }
    }
}

/// A pre-v13 source: opening it migrates the read copy, which seeds every
/// legacy reference shot as Jump and leaves the #27 Jump/Reference backfill
/// pending for the open path. The add finishes it on the source's own graph,
/// as an open would - otherwise a dangling tie stays a swimmable Jump in the
/// destination for good, since the pending flag dies with the read copy.
/// Synthesized the way the stalx reader's test does: a current file carrying
/// the migration's Jump seed and the pending flag. A dangling virtual shot
/// reaches real survey at one point only, so the backfill calls it Reference.
/// INJECTION (tried): dropping the `kind_classification_pending` branch in
/// `load_add_source` leaves the flagged copy's shot a Jump and fails the
/// Reference assertion.
#[test]
fn a_pending_kind_classification_in_the_source_is_finished_by_the_add() {
    let temp = TempDir::new().unwrap();
    let (state, _) = open_target(temp.path(), true);
    let mut legacy = Project::new("Legacy".to_string(), None);
    let mut data = section(
        "L0",
        vec![shot("l0-0", 10.0, 5.0, 2.0), shot("l0-1", 40.0, 3.0, 2.5)],
    );
    data.shots[1].kind = ShotKind::Jump;
    legacy.add_section(data);
    let unflagged = save_to(temp.path(), "unflagged.stalx", &legacy);
    let flagged = save_to(temp.path(), "flagged.stalx", &legacy);
    rusqlite::Connection::open(&flagged)
        .unwrap()
        .execute(
            "INSERT INTO project_settings (key, value) VALUES ('pending_kind_classification', '1')",
            [],
        )
        .unwrap();

    let kind_after_add = |path: &Path| {
        let result = add(&state, path);
        assert!(result.success, "{:?}", result.error);
        let project = state.get_project_state().unwrap();
        project.get_section(result.section_ids[0]).unwrap().shots[1].kind
    };
    assert_eq!(
        kind_after_add(&unflagged),
        ShotKind::Jump,
        "without the flag the stored kind is kept"
    );
    assert_eq!(
        kind_after_add(&flagged),
        ShotKind::Reference,
        "with the flag the add classifies the source like an open"
    );
    assert!(
        Database::open_existing(&flagged)
            .unwrap()
            .is_kind_classification_pending(),
        "the source file itself is untouched"
    );
}

/// A refused add leaves no trace at all: the removal the undo runs keeps the
/// id allocators advanced on purpose (a redo may need those ids again), but a
/// refused add has no redo, so it must give back the ids it spent - or an add
/// that fits the section-id budget exactly is refused after a failed one - and
/// the modification time it bumped. Through the append's error (a real file)
/// and the post-append integrity guard (in memory, with a closure so the
/// closure-id allocator moves too).
/// INJECTION (tried): dropping each restore from `roll_back` in turn fails an
/// allocator assertion - `next_section_id` on the append-error path (two
/// section ids spent, which would also refuse the exact fit), `next_closure_id`
/// on the integrity path (one closure id spent), `modified_at` on the
/// append-error path (bumped by the removal).
#[test]
fn a_refused_add_spends_no_ids_and_keeps_the_modification_time() {
    let temp = TempDir::new().unwrap();
    let (state, _) = open_target(temp.path(), true);
    let alpha = save_to(temp.path(), "alpha.stalx", &source_alpha());
    let incoming = source_alpha().sections.len() as u32;
    let allocators = || {
        let guard = state.project.lock().unwrap();
        let project = guard.as_ref().unwrap();
        (
            project.next_section_id,
            project.next_closure_id,
            project.modified_at,
        )
    };
    if let Some(project) = state.project.lock().unwrap().as_mut() {
        project.next_section_id = MAX_SECTIONS_IN_ID_SPACE as u32 - incoming;
        project.modified_at = fixed_time(0);
    }
    let before = allocators();

    let mut two_datums = source_bravo();
    let b0 = two_datums.sections[0].id;
    two_datums.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(b0, 2).into(),
        gps: GpsCoordinates::new(20.7, -87.5, 0.0).unwrap(),
    });
    let two_datums = save_to(temp.path(), "two-datums.stalx", &two_datums);
    assert!(
        !add(&state, &two_datums).success,
        "the append error refuses"
    );
    assert_eq!(allocators(), before, "the append error spent nothing");

    let mut infinite = Project::new("Infinite".to_string(), None);
    let x0 = infinite
        .add_section(section("X0", vec![shot("x0-0", 10.0, f64::INFINITY, 1.0)]))
        .id;
    infinite.loop_closures.push(closure(
        StationRef::new(x0, 0),
        StationRef::new(x0, 1),
        ClosureStrategy::Batch,
    ));
    let blocked = {
        let mut guard = state.project.lock().unwrap();
        super::project_add::append_in_memory(guard.as_mut().unwrap(), infinite)
    };
    assert!(blocked.is_err(), "the integrity guard refuses");
    assert_eq!(allocators(), before, "the integrity block spent nothing");

    let fits = add(&state, &alpha);
    assert!(fits.success, "the exact fit still fits: {:?}", fits.error);
    assert_eq!(fits.section_ids.iter().max(), Some(&MAX_SECTION_ID));
}

/// A datum-less source whose section was reversed and then extended: its
/// least-squares gauge is pinned by the reverse parity and the gauge anchor
/// pair, none of which `SectionData` carries. Both the add and "Merge
/// projects" (the same append) must copy them, or the copied component is
/// gauged on a different station and its adjusted geometry translates.
/// INJECTION (tried): dropping the parity/anchor copy from `append_project`
/// fails the add's assertion with parity false and both anchors `None`, and,
/// with the add's assertion skipped, the merge's the same way.
#[test]
fn add_and_merge_keep_each_sections_gauge_parity_and_anchors() {
    let temp = TempDir::new().unwrap();
    let mut reversed = Project::new("Reversed".to_string(), None);
    let r0 = reversed
        .add_section(section(
            "R0",
            vec![
                shot("r0-0", 10.0, 5.0, 2.0),
                shot("r0-1", 40.0, 6.0, 2.5),
                shot("r0-2", 70.0, 4.0, 3.0),
            ],
        ))
        .id;
    if let Some(s) = reversed.get_section_mut(r0) {
        s.reverse_parity = true;
        s.gauge_shot_uuid = Some("r0-1".to_string());
        s.gauge_neighbour_uuid = Some("r0-0".to_string());
    }
    let source = save_to(temp.path(), "reversed.stalx", &reversed);
    let gauge_of = |project: &Project, id: SectionId| {
        let s = project.get_section(id).unwrap();
        (
            s.reverse_parity,
            s.gauge_shot_uuid.clone(),
            s.gauge_neighbour_uuid.clone(),
        )
    };
    let expected = (true, Some("r0-1".to_string()), Some("r0-0".to_string()));

    let (state, _) = open_target(temp.path(), true);
    let added = add(&state, &source);
    assert!(added.success, "{:?}", added.error);
    let project = state.get_project_state().unwrap();
    assert_eq!(gauge_of(&project, added.section_ids[0]), expected, "add");

    let output = temp.path().join("merged.stalx");
    let merged = AppState::new().unwrap().execute(Command::MergeProjects {
        source_paths: vec![
            source,
            save_to(temp.path(), "charlie.stalx", &source_charlie()),
        ],
        output_path: output.clone(),
        output_name: "Merged".to_string(),
        archive_originals: false,
    });
    assert!(merged.success, "{:?}", merged.error);
    let merged = Database::open_existing(&output)
        .unwrap()
        .load_project()
        .unwrap();
    assert_eq!(gauge_of(&merged, merged.sections[0].id), expected, "merge");
}

// ===========================================================================
// Shot uuids stay unique across the project after a copy
// ===========================================================================
//
// The copy is the side that loses its undo history, so it is the side whose
// colliding shot uuids are re-minted: an add never touches the open project's
// own shots (its undo history names them), and a merge keeps the first
// occurrence in source order.

/// Every shot uuid in `project`, in section then shot order.
fn every_shot_uuid(project: &Project) -> Vec<String> {
    project
        .sections
        .iter()
        .flat_map(|s| s.shots.iter())
        .map(|shot| shot.uuid.clone().unwrap_or_default())
        .collect()
}

fn assert_no_repeated_shot_uuid(project: &Project, what: &str) {
    let all = every_shot_uuid(project);
    let distinct: std::collections::HashSet<&String> = all.iter().collect();
    assert_eq!(
        distinct.len(),
        all.len(),
        "{what}: a shot uuid appears twice: {all:?}"
    );
}

/// The uuids of the shots of `ids`, section by section.
fn uuids_of(project: &Project, ids: &[SectionId]) -> Vec<Vec<String>> {
    ids.iter()
        .map(|id| {
            project
                .get_section(*id)
                .unwrap()
                .shots
                .iter()
                .map(|shot| shot.uuid.clone().unwrap_or_default())
                .collect()
        })
        .collect()
}

/// Everything hanging off the sections `ids`, with each section named by its
/// POSITION in `ids` and each station by its resolved index: two copies of
/// one source describe the same survey exactly when these lists are equal,
/// whatever ids and uuids each copy was given. A reference that no longer
/// resolves shows up as `?`.
fn references_of(project: &Project, ids: &[SectionId]) -> Vec<String> {
    let at = |key: &crate::models::StationKey| -> Option<String> {
        let position = ids.iter().position(|id| *id == key.section_id)?;
        Some(match project.resolve_key(key) {
            Some(sr) => format!("{position}:{}", sr.station_index),
            None => format!("{position}:?"),
        })
    };
    let mut refs = Vec::new();
    for merge in project.station_merges.iter() {
        if let (Some(a), Some(b)) = (at(&merge.station_a), at(&merge.station_b)) {
            refs.push(format!("merge {a} {b}"));
        }
    }
    for closure in &project.loop_closures {
        if let (Some(a), Some(b)) = (at(&closure.station_a), at(&closure.station_b)) {
            refs.push(format!("closure {a} {b} {:?}", closure.strategy));
        }
    }
    for (key, text) in &project.station_comments {
        if let Some(a) = at(key) {
            refs.push(format!("comment {a} {text}"));
        }
    }
    for (key, name) in &project.station_names {
        if let Some(a) = at(key) {
            refs.push(format!("name {a} {name}"));
        }
    }
    for (key, (x, y, z)) in &project.station_offsets {
        if let Some(a) = at(key) {
            refs.push(format!("offset {a} {x} {y} {z}"));
        }
    }
    for datum in &project.zero_datums {
        if let Some(a) = at(&datum.station_ref) {
            refs.push(format!(
                "datum {a} {} {}",
                datum.gps.latitude, datum.gps.longitude
            ));
        }
    }
    for (position, id) in ids.iter().enumerate() {
        let s = project.get_section(*id).unwrap();
        let index_of = |uuid: &Option<String>| {
            uuid.as_ref().map(|u| {
                s.shots
                    .iter()
                    .position(|shot| shot.uuid.as_deref() == Some(u.as_str()))
                    .map_or_else(|| "?".to_string(), |i| i.to_string())
            })
        };
        refs.push(format!(
            "gauge {position} {} {:?} {:?}",
            s.reverse_parity,
            index_of(&s.gauge_shot_uuid),
            index_of(&s.gauge_neighbour_uuid)
        ));
    }
    refs.sort();
    refs
}

/// Adding the same project twice: the first copy keeps the source's ids (the
/// open project does not hold them), the second re-mints every one, and both
/// copies describe the same survey - every merge, closure, comment, name,
/// station offset, datum and gauge anchor of the second copy sits where the
/// first copy's does. The open project's own shots keep their ids.
/// INJECTION (tried): skipping `claim_unique_uuids` fails the uniqueness
/// assertion; copying the gauge anchors verbatim leaves the second copy's
/// gauge naming ids its section no longer has (`?`).
#[test]
fn adding_the_same_project_twice_remints_only_the_second_copy() {
    let temp = TempDir::new().unwrap();
    let (state, _) = open_target(temp.path(), true);
    let host_ids: Vec<SectionId> = state
        .get_project_state()
        .unwrap()
        .sections
        .iter()
        .map(|s| s.id)
        .collect();
    let host_before = uuids_of(&state.get_project_state().unwrap(), &host_ids);
    // A gauge anchor pair on one section, so the re-mint has one to follow.
    let mut gauged = source_alpha_named();
    if let Some(s) = gauged.sections.first_mut() {
        s.reverse_parity = true;
        s.gauge_shot_uuid = Some("a0-1".to_string());
        s.gauge_neighbour_uuid = Some("a0-0".to_string());
    }
    let alpha = save_to(temp.path(), "alpha.stalx", &gauged);

    let first = add(&state, &alpha);
    assert!(first.success, "{:?}", first.error);
    let second = add(&state, &alpha);
    assert!(second.success, "{:?}", second.error);

    let project = state.get_project_state().unwrap();
    assert_no_repeated_shot_uuid(&project, "after two adds");
    assert_eq!(
        uuids_of(&project, &host_ids),
        host_before,
        "the open project's own shots keep their ids"
    );
    let source_ids: Vec<Vec<String>> = source_alpha_named()
        .sections
        .iter()
        .map(|s| {
            s.shots
                .iter()
                .map(|shot| shot.uuid.clone().unwrap())
                .collect()
        })
        .collect();
    assert_eq!(
        uuids_of(&project, &first.section_ids),
        source_ids,
        "the first copy collides with nothing, so it keeps every id"
    );

    let first_refs = references_of(&project, &first.section_ids);
    assert_eq!(
        first_refs,
        references_of(&project, &second.section_ids),
        "the re-minted copy hangs everything off the same stations"
    );
    for kind in [
        "merge ", "closure ", "comment ", "name ", "offset ", "datum ", "gauge ",
    ] {
        assert!(
            first_refs.iter().any(|r| r.starts_with(kind)),
            "the source exercises {kind:?}: {first_refs:?}"
        );
    }
    assert!(
        !first_refs.iter().any(|r| r.contains('?')),
        "every reference resolves: {first_refs:?}"
    );
    assert!(!project.has_critical_integrity_violations());
}

/// The re-minted ids are part of the add's recorded content: undo removes
/// them, redo puts the SAME ids back (it never re-reads the file, and a fresh
/// mint would differ), and both survive a save and a reload.
/// INJECTION (tried): skipping `claim_unique_uuids` fails the post-reload
/// uniqueness assertion.
#[test]
fn reminted_ids_come_back_identical_on_redo_and_after_a_reload() {
    let temp = TempDir::new().unwrap();
    let (state, path) = open_target(temp.path(), true);
    let alpha = save_to(temp.path(), "alpha.stalx", &source_alpha_named());
    assert!(add(&state, &alpha).success);
    let before = canonical_snapshot(&state, false);

    let second = add(&state, &alpha);
    assert!(second.success, "{:?}", second.error);
    let reminted = uuids_of(&state.get_project_state().unwrap(), &second.section_ids);

    assert!(state.undo().success);
    assert_same_project(&state, &before, false, "undo removes the second copy");
    assert!(state.redo().success);
    assert_eq!(
        uuids_of(&state.get_project_state().unwrap(), &second.section_ids),
        reminted,
        "redo reinstalls the same re-minted ids"
    );

    save(&state);
    std::fs::remove_file(&alpha).unwrap();
    reopen(&state, &path);
    let project = state.get_project_state().unwrap();
    assert_eq!(
        uuids_of(&project, &second.section_ids),
        reminted,
        "the reload keeps them"
    );
    assert_no_repeated_shot_uuid(&project, "after the reload");
    assert!(state.undo().success, "undo after the reload");
    assert!(state.redo().success, "redo after the reload");
    assert_eq!(
        uuids_of(&state.get_project_state().unwrap(), &second.section_ids),
        reminted,
        "and redo after the reload still reinstalls them"
    );
}

/// An ignored warning rides with its shot even when that shot's id had to be
/// re-minted: the second copy's ignores are honored before any save, after a
/// save and reload, and across undo/redo.
/// INJECTION (tried): dropping the `reminted_uuids` translation in
/// `remap_ignores` fails the first assertion with `[false, false]` - the
/// ignores are silently lost.
#[test]
fn ignores_follow_a_reminted_shot_through_save_and_reload() {
    let temp = TempDir::new().unwrap();
    let (state, path) = open_target(temp.path(), true);
    let source = source_with_ignores(temp.path());
    assert!(add(&state, &source).success);

    let second = add(&state, &source);
    assert!(second.success, "{:?}", second.error);
    let ids = second.section_ids;
    assert_no_repeated_shot_uuid(&state.get_project_state().unwrap(), "two adds");
    assert_eq!(
        long_shot_verdicts(&state, &ids),
        vec![true, true],
        "the re-minted copy's ignores are honored before any save"
    );

    save(&state);
    reopen(&state, &path);
    assert_eq!(long_shot_verdicts(&state, &ids), vec![true, true]);
    assert!(state.undo().success);
    assert!(long_shot_verdicts(&state, &ids).is_empty());
    assert!(state.redo().success);
    assert_eq!(long_shot_verdicts(&state, &ids), vec![true, true]);
}

/// A source may legitimately hold one id in two of its own sections (#423).
/// The copy keeps the first and re-mints the second, and a reference anchored
/// on the second section's shot still lands on that section's station.
/// INJECTION (tried): seeding the taken set from the target only (not the
/// copy's own earlier sections) leaves the repeat and fails the uniqueness
/// assertion.
#[test]
fn a_source_repeating_an_id_across_its_own_sections_is_made_unique() {
    let temp = TempDir::new().unwrap();
    let mut repeating = Project::new("Repeating".to_string(), None);
    let s0 = repeating
        .add_section(section(
            "S0",
            vec![shot("shared", 10.0, 5.0, 1.0), shot("s0-1", 20.0, 4.0, 1.5)],
        ))
        .id;
    let s1 = repeating
        .add_section(section(
            "S1",
            vec![
                shot("s1-0", 200.0, 3.0, 2.0),
                shot("shared", 210.0, 6.0, 2.5),
            ],
        ))
        .id;
    repeating
        .station_comments
        .insert(StationRef::new(s1, 2).into(), "end of S1".to_string());
    repeating.record_station_merge(StationMerge {
        station_a: StationRef::new(s0, 1).into(),
        station_b: StationRef::new(s1, 0).into(),
        merged_at: fixed_time(4),
    });
    let source = save_to(temp.path(), "repeating.stalx", &repeating);

    let (state, _) = open_target(temp.path(), true);
    let result = add(&state, &source);
    assert!(result.success, "{:?}", result.error);
    let project = state.get_project_state().unwrap();
    assert_no_repeated_shot_uuid(&project, "a self-repeating source");
    let copied = uuids_of(&project, &result.section_ids);
    assert_eq!(copied[0][0], "shared", "the first occurrence keeps the id");
    assert_ne!(copied[1][1], "shared", "the second is re-minted");
    let refs = references_of(&project, &result.section_ids);
    assert!(
        refs.contains(&"comment 1:2 end of S1".to_string()),
        "the comment stays on S1's last station: {refs:?}"
    );
    assert!(
        refs.contains(&"merge 0:1 1:0".to_string()),
        "the merge stays put: {refs:?}"
    );
}

/// Merging two copies of one project: the first source keeps its ids, the
/// second is re-minted, and both describe the same survey.
#[test]
fn merging_two_copies_keeps_the_first_ids_and_remints_the_second() {
    let temp = TempDir::new().unwrap();
    let alpha = save_to(temp.path(), "alpha.stalx", &source_alpha_named());
    let copy = temp.path().join("alpha-copy.stalx");
    std::fs::copy(&alpha, &copy).unwrap();
    let output = temp.path().join("merged.stalx");

    let merged = AppState::new().unwrap().execute(Command::MergeProjects {
        source_paths: vec![alpha, copy],
        output_path: output.clone(),
        output_name: "Merged".to_string(),
        archive_originals: false,
    });
    assert!(merged.success, "{:?}", merged.error);
    let mut merged = Database::open_existing(&output)
        .unwrap()
        .load_project()
        .unwrap();
    // A raw read, as `merge_output_is_unchanged_by_the_append_extraction` does;
    // the redrive an open runs re-anchors the stored keys first.
    merged.redrive_offsets_after_load();
    assert_no_repeated_shot_uuid(&merged, "the merged project");

    let per_copy = source_alpha_named().sections.len();
    let ids: Vec<SectionId> = merged.sections.iter().map(|s| s.id).collect();
    let (first, second) = ids.split_at(per_copy);
    let source_ids: Vec<Vec<String>> = source_alpha_named()
        .sections
        .iter()
        .map(|s| {
            s.shots
                .iter()
                .map(|shot| shot.uuid.clone().unwrap())
                .collect()
        })
        .collect();
    assert_eq!(
        uuids_of(&merged, first),
        source_ids,
        "first source keeps its ids"
    );
    assert_eq!(
        references_of(&merged, first),
        references_of(&merged, second),
        "the re-minted copy hangs everything off the same stations"
    );
}

/// Repeats the open project ALREADY holds are not the add's to fix: its undo
/// history names those shots. The add only guarantees it brings in none.
#[test]
fn an_add_leaves_repeats_the_open_project_already_held() {
    let temp = TempDir::new().unwrap();
    let mut host = Project::new("Host".to_string(), None);
    host.add_section(section("H0", vec![shot("dup", 0.0, 10.0, 1.0)]));
    host.add_section(section("H1", vec![shot("dup", 90.0, 8.0, 1.5)]));
    let host_path = save_to(temp.path(), "host.stalx", &host);
    let state = AppState::new().unwrap();
    assert!(state.open_project(host_path).success);
    let host_ids: Vec<SectionId> = state
        .get_project_state()
        .unwrap()
        .sections
        .iter()
        .map(|s| s.id)
        .collect();

    let mut incoming = Project::new("Incoming".to_string(), None);
    incoming.add_section(section(
        "I0",
        vec![shot("dup", 180.0, 6.0, 2.0), shot("i0-1", 200.0, 5.0, 2.0)],
    ));
    let incoming = save_to(temp.path(), "incoming.stalx", &incoming);
    let result = add(&state, &incoming);
    assert!(result.success, "{:?}", result.error);

    let project = state.get_project_state().unwrap();
    assert_eq!(
        uuids_of(&project, &host_ids),
        vec![vec!["dup".to_string()], vec!["dup".to_string()]],
        "the open project's own repeat is left alone"
    );
    let added = &uuids_of(&project, &result.section_ids)[0];
    assert_ne!(added[0], "dup", "the incoming shot does not add a third");
    assert_eq!(added[1], "i0-1", "a non-colliding id is kept");
}
