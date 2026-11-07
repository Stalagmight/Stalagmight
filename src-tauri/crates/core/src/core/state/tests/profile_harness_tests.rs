//! The `#[ignore]`d profiling harnesses for TML import and .stalx load.
//!
//! Lines 23302-23767 of the former single-file `core/state/tests.rs` (#576 E1),
//! moved verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;
use crate::test_fixtures::{shot, state_holding, state_with_project};

// ============================================================================
// Performance profiler (not a unit test - a measurement harness)
// ============================================================================
//
// Run with:
//   STALAGMIGHT_PROFILE_TML=/path/to/large-project.tml \
//     cargo test --release --manifest-path src-tauri/Cargo.toml -p stalagmight-core --lib \
//     state::tests::profile_tml_import_full -- --nocapture --ignored
//
// The harness drives parse_tml → convert_tml_to_import_configs →
// execute_import_sections end-to-end on an empty in-memory project,
// installs a real tracing-subscriber so the timing markers added to
// commands.rs land on stdout, and prints wall-clock for each phase.

#[test]
#[ignore = "profiling harness - run explicitly with STALAGMIGHT_PROFILE_TML=<path>"]
fn profile_tml_import_full() {
    use crate::models::Project;
    use crate::parsers::tml as tml_parser;
    use crate::persistence::Database;
    use std::time::Instant;

    let path = crate::models::test_support::require_env_file("STALAGMIGHT_PROFILE_TML");

    // Use the test framework's tracing-subscriber init guard so we
    // don't double-init when the test runs alongside others.
    let _ = tracing_subscriber::fmt()
        .with_target(false)
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();

    let t0 = Instant::now();
    let tml = tml_parser::parse_tml(std::path::Path::new(&path)).expect("parse_tml");
    let t1 = Instant::now();
    let (sections, closures, _warnings) =
        tml_parser::convert_tml_to_import_configs(&tml, false, false).expect("convert");
    let t2 = Instant::now();

    let project = Project::new("profile".to_string(), None);
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("profile.stalx");
    let mut db = Database::open(&db_path).unwrap();
    db.save_project(&project).unwrap();
    let state = state_holding(project);
    state.set_database_for_tests(db);
    let t3 = Instant::now();

    let total_shots: usize = sections.iter().map(|s| s.shots.len()).sum();

    eprintln!("\n┌─ profile_tml_import_full ─────────────────────────────");
    eprintln!("│ file:     {}", path.display());
    eprintln!("│ sections: {}", sections.len());
    eprintln!("│ shots:    {}", total_shots);
    eprintln!("│ closures: {}", closures.len());
    eprintln!("├─ phase timings ───────────────────────────────────────");
    eprintln!(
        "│ parse_tml:                   {:>9.2} ms",
        (t1 - t0).as_secs_f64() * 1000.0
    );
    eprintln!(
        "│ convert_tml_to_import_configs:{:>9.2} ms",
        (t2 - t1).as_secs_f64() * 1000.0
    );
    eprintln!(
        "│ AppState setup (excluded):   {:>9.2} ms",
        (t3 - t2).as_secs_f64() * 1000.0
    );

    let t_exec_start = Instant::now();
    let result = state.execute_import_sections(
        sections,
        closures,
        None,
        Vec::new(),
        Vec::new(),
        VirtualShotMode::Auto,
    );
    let exec_elapsed = t_exec_start.elapsed();
    eprintln!(
        "│ execute_import_sections:     {:>9.2} ms",
        exec_elapsed.as_secs_f64() * 1000.0
    );
    eprintln!("├─ totals ──────────────────────────────────────────────");
    eprintln!(
        "│ end-to-end (parse+convert+execute): {:>9.2} ms",
        ((t1 - t0) + (t2 - t1) + exec_elapsed).as_secs_f64() * 1000.0
    );
    eprintln!("└───────────────────────────────────────────────────────");

    assert!(
        result.success,
        "execute_import_sections failed: {:?}",
        result.error
    );
}

// ============================================================================
// Performance profiler for project load (not a unit test)
// ============================================================================
//
// Run with:
//   STALAGMIGHT_PROFILE_STALX=/path/to/project.stalx \
//     cargo test --release --manifest-path src-tauri/Cargo.toml -p stalagmight-core --lib \
//     state::tests::profile_stalx_load -- --nocapture --ignored

#[test]
#[ignore = "profiling harness - run explicitly with STALAGMIGHT_PROFILE_STALX=<path>"]
fn profile_stalx_load() {
    use crate::persistence::Database;
    use std::time::Instant;

    let path = crate::models::test_support::require_env_file("STALAGMIGHT_PROFILE_STALX");

    let _ = tracing_subscriber::fmt()
        .with_target(false)
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();

    // === Phase 0: SQLite open ===
    let t0 = Instant::now();
    let db = Database::open_existing(std::path::Path::new(&path)).expect("open_existing");
    let t1 = Instant::now();

    // === Phase 1: load_project (the bulk query loop) ===
    let mut project = db.load_project().expect("load_project");
    let t2 = Instant::now();

    let section_count = project.sections.len();
    let shot_count: usize = project.sections.iter().map(|s| s.shots.len()).sum();

    // === Phase 2: station coordinate computation ===
    let stations = project.compute_station_coordinates();
    let t3 = Instant::now();
    let station_count = stations.len();

    // === Phase 3: project stats + penetration ===
    let mut stats = project.stats();
    stats.penetration_distance = project.calculate_penetration(&stations);
    let t4 = Instant::now();

    // === Phase 4: warnings (via AppState because the impl is on AppState) ===
    project.file_path = Some(PathBuf::from(&path));
    let state = state_holding(project.clone());
    let warnings = state.compute_project_warnings(&project, Some(&db));
    let t5 = Instant::now();

    // === Phase 5: load_display_settings ===
    let _settings = db.load_display_settings().unwrap_or_default();
    let t6 = Instant::now();

    // === Phase 6: load_undo_stack_with_timestamps ===
    let _undo = db.load_undo_stack_with_timestamps().unwrap_or_default();
    let _redo = db.load_redo_stack_with_timestamps().unwrap_or_default();
    let t7 = Instant::now();

    // === Phase 7: binary envelope encoding ===
    let display_settings = crate::core::loading::DisplaySettings::from_stored(
        &db.load_display_settings().unwrap_or_default(),
    );
    let result = crate::core::loading::LoadProjectFullResult {
        project,
        stations,
        stats,
        warnings,
        display_settings,
        loading_id: 0,
        is_remote: false,
        newer_schema_version: None,
        speleo_link_unreadable: false,
        unreadable_project_settings: Vec::new(),
        load_repairs: crate::persistence::LoadRepairs::default(),
    };
    let bin = crate::core::load_binary::encode(result).expect("encode");
    let t8 = Instant::now();

    eprintln!("\n┌─ profile_stalx_load ──────────────────────────────────");
    eprintln!("│ file:      {}", path.display());
    eprintln!("│ sections:  {section_count}");
    eprintln!("│ shots:     {shot_count}");
    eprintln!("│ stations:  {station_count}");
    eprintln!(
        "│ binary envelope: {} bytes ({:.1} MB)",
        bin.len(),
        bin.len() as f64 / 1024.0 / 1024.0
    );
    eprintln!("├─ phase timings ───────────────────────────────────────");
    eprintln!(
        "│ Database::open_existing:        {:>9.2} ms",
        (t1 - t0).as_secs_f64() * 1000.0
    );
    eprintln!(
        "│ db.load_project (SQLite):       {:>9.2} ms",
        (t2 - t1).as_secs_f64() * 1000.0
    );
    eprintln!(
        "│ compute_station_coordinates:    {:>9.2} ms",
        (t3 - t2).as_secs_f64() * 1000.0
    );
    eprintln!(
        "│ project.stats + penetration:    {:>9.2} ms",
        (t4 - t3).as_secs_f64() * 1000.0
    );
    eprintln!(
        "│ compute_project_warnings:       {:>9.2} ms",
        (t5 - t4).as_secs_f64() * 1000.0
    );
    eprintln!(
        "│ load_display_settings:          {:>9.2} ms",
        (t6 - t5).as_secs_f64() * 1000.0
    );
    eprintln!(
        "│ load_undo+redo_stacks:          {:>9.2} ms",
        (t7 - t6).as_secs_f64() * 1000.0
    );
    eprintln!(
        "│ binary envelope encode:         {:>9.2} ms",
        (t8 - t7).as_secs_f64() * 1000.0
    );
    eprintln!("├─ totals ──────────────────────────────────────────────");
    eprintln!(
        "│ end-to-end:                     {:>9.2} ms",
        (t8 - t0).as_secs_f64() * 1000.0
    );
    eprintln!("└───────────────────────────────────────────────────────");
}

#[test]
fn redo_create_shot_preserves_uuid_all_insert_modes() {
    // Redo of CreateShot must restore the shot's ORIGINAL UUID (not mint a fresh
    // one) on every insert mode - append, insert_after, and prepend - because
    // cross-references and Ariane matching anchor on it. Each mode hits a
    // distinct redo branch (append: Shot::with_uuid; after/before: the forced
    // uuid threaded through insert_shot_*).
    let state = state_with_project("X");
    let sid = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#111111".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![shot(5.0), shot(5.0)],
        })
        .section_id
        .unwrap();

    let uuid_of = |s: &AppState, shot_id: u32| -> Option<String> {
        s.get_project_state()
            .unwrap()
            .get_section(sid)
            .unwrap()
            .get_shot(shot_id)
            .unwrap()
            .uuid
            .clone()
    };
    let first_id = state
        .get_project_state()
        .unwrap()
        .get_section(sid)
        .unwrap()
        .shots[0]
        .id;
    let last_id = {
        let p = state.get_project_state().unwrap();
        let shots = &p.get_section(sid).unwrap().shots;
        shots[shots.len() - 1].id
    };

    let cases = [
        // (after_shot_id, before_shot_id) → append / insert_after / prepend
        Command::CreateShot {
            section_id: sid,
            shot: shot(3.0),
            after_shot_id: None,
            before_shot_id: None,
        },
        Command::CreateShot {
            section_id: sid,
            shot: shot(3.0),
            after_shot_id: Some(last_id),
            before_shot_id: None,
        },
        Command::CreateShot {
            section_id: sid,
            shot: shot(3.0),
            after_shot_id: None,
            before_shot_id: Some(first_id),
        },
    ];
    for cmd in cases {
        let res = state.execute(cmd);
        assert!(res.success, "create shot succeeds");
        let new_id = res.shot_id.expect("create shot returns id");
        let before = uuid_of(&state, new_id);
        assert!(before.is_some(), "created shot has a uuid");

        assert!(state.undo().success, "undo create shot");
        assert!(state.redo().success, "redo create shot");

        assert_eq!(
            uuid_of(&state, new_id),
            before,
            "redo must preserve the shot UUID"
        );
    }
}

#[test]
fn repair_open_project_integrity_reports_fixed_and_remaining() {
    // Repair returns the genuine before→after delta over the FULL violation set,
    // so a repairable issue counts as `fixed` and a detect-only one (a drifted
    // aligned loop closure) counts as `remaining`.
    let state = state_with_project("X");
    let a = state
        .execute(Command::CreateSection {
            name: Some("A".to_string()),
            surveyor: None,
            date: None,
            color: "#111111".to_string(),
            direction: Some(SurveyDirection::In),
            shots: vec![shot(5.0)],
        })
        .section_id
        .unwrap();

    // Inject a dangling merge (repairable: repair drops it).
    {
        let mut guard = state.project.lock();
        let project = guard.as_mut().unwrap();
        let dangling = StationKey::new(a, StationAnchor::AfterShot("ghost".to_string()));
        project.station_merges.push(StationMerge {
            station_a: dangling,
            station_b: StationRef::new(a, 0).into(),
            merged_at: chrono::Utc::now(),
        });
    }

    let before = state.integrity_violation_messages().len();
    assert!(before >= 1, "seeded at least one violation");

    let result = state.repair_open_project_integrity().unwrap();
    assert_eq!(
        (result.fixed + result.remaining) as usize,
        before,
        "fixed + remaining accounts for every issue the banner showed"
    );
    assert!(result.fixed >= 1, "the dangling merge was repaired");
    assert_eq!(
        result.remaining as usize,
        state.integrity_violation_messages().len(),
        "remaining matches the true post-repair count"
    );
}

#[test]
fn repair_command_demotes_a_double_anchoring_bridge_merge() {
    // End-to-end through the Repair action on the damage a past bug left in saved
    // files: a RIGID merge bridging two independently GPS-anchored merge
    // components. `resolve_offsets` cannot satisfy it (each half is pinned to its
    // own datum), so before this repair the project reported `merge_misaligned` +
    // `multiple_datums_per_merge_component` on every check and Repair could never
    // clear them. Fully synthetic: three sections built by command, welded by hand.
    let state = state_with_project("Damaged");
    let section = |name: &str| {
        state
            .execute(Command::CreateSection {
                name: Some(name.to_string()),
                surveyor: None,
                date: None,
                color: "#111111".to_string(),
                direction: Some(SurveyDirection::In),
                shots: vec![shot(10.0), shot(10.0)],
            })
            .section_id
            .unwrap()
    };
    let (a, b, c) = (section("A"), section("B"), section("C"));

    {
        let mut guard = state.project.lock();
        let project = guard.as_mut().unwrap();
        // The legitimate merge inside the first component, made through the real
        // API so it is correctly aligned.
        project
            .merge_stations(StationRef::new(a, 2), StationRef::new(b, 0))
            .unwrap();
        // One datum per component - individually legal, which is why the
        // interactive guards never fired on the way in.
        project.zero_datums.push(crate::models::ZeroDatum {
            station_ref: StationRef::new(a, 0).into(),
            gps: GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
        });
        project.zero_datums.push(crate::models::ZeroDatum {
            station_ref: StationRef::new(c, 0).into(),
            gps: GpsCoordinates::new(45.001, -90.0, 0.0).unwrap(),
        });
        // Park C 500 m away, then weld it to B with the merge the promotion bug
        // created out of what should have stayed a loop closure.
        project.set_section_offset(c, (500.0, 0.0));
        project.station_merges.push(StationMerge {
            station_a: StationRef::new(b, 2).into(),
            station_b: StationRef::new(c, 0).into(),
            merged_at: chrono::Utc::now(),
        });
    }

    let before = state.integrity_violation_messages().len();
    assert!(
        before >= 2,
        "seeded a torn merge AND an over-anchored merge component, got {before}"
    );

    let result = state.repair_open_project_integrity().unwrap();
    assert!(
        result.fixed >= 1,
        "the demotion is reported as a fix, got {result:?}"
    );
    assert!(result.changed, "repair mutated the project");
    assert_eq!(
        result.remaining,
        0,
        "both symptoms clear: {:?}",
        state.integrity_violation_messages()
    );

    let guard = state.project.lock();
    let project = guard.as_ref().unwrap();
    assert_eq!(
        project.station_merges.len(),
        1,
        "only the legitimate A-B merge survives"
    );
    assert!(
        project.are_stations_in_closure(StationRef::new(b, 2), StationRef::new(c, 0)),
        "the demoted pair is now a loop closure - and it is still one after \
         convert_orphan_closures_to_merges ran later in the SAME repair pass"
    );
    assert!(
        project.loop_closures.iter().all(|lc| !lc.is_aligned),
        "an aligned closure would re-impose the constraint the demotion removed"
    );
}
