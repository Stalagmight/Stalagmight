//! Command wrapper tests focused on critical/edge behaviors.

use super::{
    cache, config, export, history, import, project, raw_data, section, shot, station, validation,
};
use crate::core::{AppState, Command, CommandResult};
use crate::models::{
    ClosureStrategy, Coordinates, GpsCoordinates, Project, SectionData, SectionUpdates, ShotData,
    ShotKind, ShotUpdates, StationRef, SurveyDirection,
};
use crate::persistence::Database;
use crate::test_helpers::TestDb;
use chrono::NaiveDate;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::{env, fs};
use tauri::test::{mock_app, MockRuntime};
use tauri::{Manager, State};
use tempfile::{NamedTempFile, TempDir};

fn app_with_state() -> tauri::App<MockRuntime> {
    let app = mock_app();
    app.manage(AppState::new().expect("app state"));
    app
}

fn app_with_custom_state(state: AppState) -> tauri::App<MockRuntime> {
    let app = mock_app();
    app.manage(state);
    app
}

fn state(app: &tauri::App<MockRuntime>) -> State<'_, AppState> {
    app.state::<AppState>()
}

fn with_config_lock<T>(f: impl FnOnce() -> T) -> T {
    // Redirect the config path to a fresh temp dir for the duration, so a test
    // neither reads nor pollutes the real ~/.config/stalagmight/config.toml.
    // Resolution is per-thread now (see `test_helpers`), so this no longer takes
    // a process-global lock and no longer serializes with anything; the name is
    // kept because dozens of tests call it.
    crate::test_helpers::with_isolated_config(f)
}

fn with_restored_config<T>(app: &tauri::App<MockRuntime>, f: impl FnOnce() -> T) -> T {
    let original = config::get_config(state(app));
    let result = f();
    config::update_config_impl(state(app).inner(), original).expect("restore config");
    result
}

struct EnvVarGuard {
    key: String,
    original: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &str, value: &str) -> Self {
        let original = env::var(key).ok();
        env::set_var(key, value);
        Self {
            key: key.to_string(),
            original,
        }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(value) = &self.original {
            env::set_var(&self.key, value);
        } else {
            env::remove_var(&self.key);
        }
    }
}

fn assert_success(result: &CommandResult) {
    assert!(
        result.success,
        "expected success, got error: {:?}",
        result.error
    );
    assert!(result.error.is_none());
}

fn assert_failure_contains(result: &CommandResult, needle: &str) {
    assert!(!result.success, "expected failure");
    let err = result.error.as_deref().unwrap_or("<none>");
    assert!(
        err.contains(needle),
        "expected error containing '{needle}', got '{err}'"
    );
}

fn shot_data(length: f64) -> ShotData {
    ShotData {
        azimuth_in: 0.0,
        azimuth_out: 5.0,
        length,
        depth_in: 0.0,
        depth_out: 0.0,
        ..Default::default()
    }
}

fn shot_data_with_depth(length: f64, depth_in: f64, depth_out: f64) -> ShotData {
    ShotData {
        azimuth_in: 10.0,
        azimuth_out: 15.0,
        length,
        depth_in,
        depth_out,
        ..Default::default()
    }
}

fn create_project(app: &tauri::App<MockRuntime>) {
    let result = state(app).execute(Command::CreateProject {
        name: "Test Cave".to_string(),
        location: Some("Test Location".to_string()),
    });
    assert_success(&result);
}

/// Test read-back helper; replaces the retired get_section IPC command (#275).
fn get_section(
    state: State<'_, AppState>,
    section_id: crate::models::SectionId,
) -> Option<crate::models::Section> {
    let project = state.get_project_state()?;
    project.get_section(section_id).cloned()
}

/// Test helper; replaces the retired open_project IPC command (#275). Calls
/// the internal Command::OpenProject path the command wrapped, preserving
/// exact opening semantics (recent-projects update, undo/redo restore, etc).
fn open_project(state: State<'_, AppState>, path: PathBuf) -> CommandResult {
    state.execute(Command::OpenProject { path })
}

fn set_project_for_tests(app: &tauri::App<MockRuntime>, project: Project) {
    state(app).inner().set_project_for_tests(project);
}

fn create_section_with_shots(app: &tauri::App<MockRuntime>, shots: Vec<ShotData>) -> u32 {
    let result = section::create_section(
        state(app),
        Some("Section A".to_string()),
        Some("Surveyor".to_string()),
        None,
        "#4ecdc4".to_string(),
        Some(SurveyDirection::In),
        shots,
    );
    assert_success(&result);
    result.section_id.expect("section id")
}

fn project_with_section(shots: Vec<ShotData>) -> Project {
    let mut project = Project::new("Test Cave".to_string(), None);
    project.add_section(SectionData {
        name: Some("Section A".to_string()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_string(),
        direction: None,
        quality: 1.0,
        shots,
    });
    project
}

fn save_project_to_temp(app: &tauri::App<MockRuntime>, dir: &TempDir) -> PathBuf {
    let path = dir.path().join("project");
    let result = project::save_project(state(app), Some(path.clone()), None);
    assert_success(&result);
    path.with_extension("stalx")
}

// === Config commands ===

#[test]
fn test_get_config_defaults_general_auto_save() {
    // Isolate from the real config file: this asserts DEFAULTS, which only
    // holds when no user config (e.g. auto_save = false) is readable.
    with_config_lock(|| {
        let app = app_with_state();
        let cfg = config::get_config(state(&app));
        assert!(cfg.general.auto_save);
        assert_eq!(cfg.general.auto_save_interval_seconds, 30);
    });
}

#[test]
fn test_get_config_defaults_panels() {
    // Test the actual defaults, not user config which may differ
    let cfg = crate::config::Config::default();
    assert_eq!(cfg.panels.sections_panel_width, 250);
    assert_eq!(cfg.panels.detail_panel_width, 300);
    assert_eq!(cfg.panels.data_table_height, 200);
    assert!(cfg.panels.data_table_station_column_order.is_empty());
    assert_eq!(
        cfg.panels.data_table_station_hidden_columns,
        vec!["_x", "_y", "_lat", "_long", "name"]
    );
    assert_eq!(cfg.panels.data_table_station_columns_seen, vec!["name"]);
    // The new-section dialog opens on what the old hand-rolled table showed.
    assert!(cfg.panels.new_section_column_order.is_empty());
    assert_eq!(
        cfg.panels.new_section_hidden_columns,
        vec![
            "azimuth_in",
            "azimuth_out",
            "_pitch_avg",
            "pitch_in",
            "pitch_out",
            "left",
            "right",
            "up",
            "down"
        ]
    );
    assert!(cfg.panels.new_section_column_widths.is_empty());
    // The edit-section grid edits an existing survey, so it opens on everything
    // except the five columns added to it after the fact - hiding those is what
    // keeps its four host dialogs looking as they did before they existed.
    assert!(cfg.panels.edit_section_column_order.is_empty());
    assert_eq!(
        cfg.panels.edit_section_hidden_columns,
        vec![
            "azimuth_in",
            "azimuth_out",
            "_pitch_avg",
            "pitch_in",
            "pitch_out"
        ]
    );
    assert!(cfg.panels.edit_section_column_widths.is_empty());
}

#[test]
fn test_update_data_table_view_roundtrip() {
    with_config_lock(|| {
        let app = app_with_state();
        config::update_data_table_view_impl(
            state(&app).inner(),
            Some(vec!["length".to_string(), "_azimuth_avg".to_string()]),
            Some(vec![
                "left".to_string(),
                "right".to_string(),
                "up".to_string(),
                "down".to_string(),
            ]),
            Some("inout".to_string()),
            None,
            Some(vec!["_zeroDatum".to_string(), "_x".to_string()]),
            Some(vec!["_connections".to_string()]),
            Some(BTreeMap::from([
                ("length".to_string(), 180),
                ("_azimuth_avg".to_string(), 95),
            ])),
            Some(BTreeMap::from([("_zeroDatum".to_string(), 140)])),
        )
        .expect("update data-table view");
        let cfg = config::get_config(state(&app));
        assert_eq!(
            cfg.panels.data_table_shot_column_order,
            vec!["length", "_azimuth_avg"]
        );
        assert_eq!(
            cfg.panels.data_table_shot_hidden_columns,
            vec!["left", "right", "up", "down"]
        );
        assert_eq!(cfg.panels.data_table_azimuth_mode, "inout");
        // untouched field keeps its default
        assert_eq!(cfg.panels.data_table_inclination_mode, "off");
        assert_eq!(
            cfg.panels.data_table_station_column_order,
            vec!["_zeroDatum", "_x"]
        );
        assert_eq!(
            cfg.panels.data_table_station_hidden_columns,
            vec!["_connections"]
        );
        assert_eq!(
            cfg.panels.data_table_shot_column_widths,
            BTreeMap::from([
                ("length".to_string(), 180),
                ("_azimuth_avg".to_string(), 95)
            ])
        );
        assert_eq!(
            cfg.panels.data_table_station_column_widths,
            BTreeMap::from([("_zeroDatum".to_string(), 140)])
        );
        // invalid mode is rejected, config untouched
        let err = config::update_data_table_view_impl(
            state(&app).inner(),
            None,
            None,
            Some("sideways".to_string()),
            None,
            None,
            None,
            None,
            None,
        );
        assert!(err.is_err());
        assert_eq!(
            config::get_config(state(&app))
                .panels
                .data_table_azimuth_mode,
            "inout"
        );
    });
}

#[test]
fn test_update_new_section_view_roundtrip() {
    with_config_lock(|| {
        let app = app_with_state();
        config::update_new_section_view_impl(
            state(&app).inner(),
            Some(vec!["length".to_string(), "_azimuth_avg".to_string()]),
            Some(vec!["comment".to_string()]),
            Some(BTreeMap::from([("length".to_string(), 180)])),
        )
        .expect("update new-section view");
        let cfg = config::get_config(state(&app));
        assert_eq!(
            cfg.panels.new_section_column_order,
            vec!["length", "_azimuth_avg"]
        );
        assert_eq!(cfg.panels.new_section_hidden_columns, vec!["comment"]);
        assert_eq!(
            cfg.panels.new_section_column_widths,
            BTreeMap::from([("length".to_string(), 180)])
        );

        // The dialog's layout is its OWN: writing it must not disturb the data
        // table's, which the two commands keep in separate fields.
        assert!(cfg.panels.data_table_shot_column_order.is_empty());
        assert_eq!(
            cfg.panels.data_table_shot_hidden_columns,
            vec!["section_id", "shot_id"]
        );

        // A partial update leaves the fields it omits alone.
        config::update_new_section_view_impl(
            state(&app).inner(),
            None,
            Some(vec!["left".to_string()]),
            None,
        )
        .expect("update new-section view");
        let cfg = config::get_config(state(&app));
        assert_eq!(cfg.panels.new_section_hidden_columns, vec!["left"]);
        assert_eq!(
            cfg.panels.new_section_column_order,
            vec!["length", "_azimuth_avg"],
            "an omitted field keeps its stored value"
        );
    });
}

#[test]
fn test_update_edit_section_view_roundtrip() {
    with_config_lock(|| {
        let app = app_with_state();
        config::update_edit_section_view_impl(
            state(&app).inner(),
            Some(vec!["length".to_string()]),
            Some(vec!["comment".to_string()]),
            Some(BTreeMap::from([("length".to_string(), 190)])),
        )
        .expect("update edit-section view");
        let cfg = config::get_config(state(&app));
        assert_eq!(cfg.panels.edit_section_column_order, vec!["length"]);
        assert_eq!(cfg.panels.edit_section_hidden_columns, vec!["comment"]);
        assert_eq!(
            cfg.panels.edit_section_column_widths,
            BTreeMap::from([("length".to_string(), 190)])
        );

        // Three grids, three disjoint field sets: writing one leaves the others.
        assert!(cfg.panels.new_section_column_order.is_empty());
        assert!(cfg.panels.data_table_shot_column_order.is_empty());

        // Degenerate widths are clamped on the way in, as everywhere else.
        config::update_edit_section_view_impl(
            state(&app).inner(),
            None,
            None,
            Some(BTreeMap::from([
                ("collapsed".to_string(), 0),
                ("runaway".to_string(), 100_000),
            ])),
        )
        .expect("update edit-section view");
        assert_eq!(
            config::get_config(state(&app))
                .panels
                .edit_section_column_widths,
            BTreeMap::from([("collapsed".to_string(), 40), ("runaway".to_string(), 2000),])
        );
    });
}

#[test]
fn test_update_new_section_view_clamps_degenerate_widths() {
    with_config_lock(|| {
        let app = app_with_state();
        config::update_new_section_view_impl(
            state(&app).inner(),
            None,
            None,
            Some(BTreeMap::from([
                ("collapsed".to_string(), 0),
                ("runaway".to_string(), 100_000),
                ("sane".to_string(), 150),
            ])),
        )
        .expect("update new-section view");
        assert_eq!(
            config::get_config(state(&app))
                .panels
                .new_section_column_widths,
            BTreeMap::from([
                ("collapsed".to_string(), 40),
                ("runaway".to_string(), 2000),
                ("sane".to_string(), 150),
            ]),
            "degenerate widths clamp to the renderable range, sane ones pass through"
        );
    });
}

#[test]
fn test_update_data_table_view_clamps_degenerate_widths() {
    with_config_lock(|| {
        let app = app_with_state();
        // A 0-px column renders invisible AND un-resizable (no border left to
        // grab), and a 100000-px one pushes every later column off-screen. Both
        // must be repaired before they reach disk.
        config::update_data_table_view_impl(
            state(&app).inner(),
            None,
            None,
            None,
            None,
            None,
            None,
            Some(BTreeMap::from([
                ("collapsed".to_string(), 0),
                ("runaway".to_string(), 100_000),
                ("sane".to_string(), 150),
            ])),
            Some(BTreeMap::from([("also_collapsed".to_string(), 3)])),
        )
        .expect("update data-table view");
        let cfg = config::get_config(state(&app));
        assert_eq!(
            cfg.panels.data_table_shot_column_widths,
            BTreeMap::from([
                ("collapsed".to_string(), 40),
                ("runaway".to_string(), 2000),
                ("sane".to_string(), 150),
            ]),
            "degenerate widths clamp to the renderable range, sane ones pass through"
        );
        assert_eq!(
            cfg.panels.data_table_station_column_widths,
            BTreeMap::from([("also_collapsed".to_string(), 40)])
        );
    });
}

/// The column-key LISTS (order + hidden, shot + station) get the same ceiling as
/// the width maps beside them. They used to be written straight through from the
/// renderer with no cap at all: `config.toml` is re-parsed at every launch, so an
/// oversized list is not a one-off cost but a permanent one, and nothing on the
/// load path pruned it either.
///
/// Both doors are checked here because the fix lives in `PanelsConfig::normalize`
/// precisely so it covers both: the partial IPC command AND the whole-`Config`
/// `update_config` (which is also the load path's normalizer).
#[test]
fn test_column_name_lists_are_bounded_on_both_write_paths() {
    // Mirrors MAX_COLUMN_WIDTH_ENTRIES / MAX_COLUMN_KEY_LEN in
    // src-tauri/src/config/column_widths.rs (private to that module).
    const MAX_ENTRIES: usize = 256;
    const OVERSIZE_KEY_LEN: usize = 129;

    // 300 distinct keys, plus the three shapes that must not survive: an empty
    // key, a key too long to name a real grid column, and a duplicate.
    let hostile_order = || {
        let mut names: Vec<String> = (0..300).map(|i| format!("col{i}")).collect();
        names.push(String::new());
        names.push("z".repeat(OVERSIZE_KEY_LEN));
        names.push("col0".to_string());
        names
    };
    let hostile_hidden = || {
        vec![
            "left".to_string(),
            String::new(),
            "left".to_string(),
            "z".repeat(OVERSIZE_KEY_LEN),
        ]
    };
    let assert_bounded = |names: &[String], field: &str| {
        assert!(
            names.len() <= MAX_ENTRIES,
            "{field} kept {} entries, past the {MAX_ENTRIES} cap",
            names.len()
        );
        assert!(
            names
                .iter()
                .all(|n| !n.is_empty() && n.len() < OVERSIZE_KEY_LEN),
            "{field} kept an empty or oversize key: {names:?}"
        );
        let mut sorted = names.to_vec();
        sorted.sort();
        let before = sorted.len();
        sorted.dedup();
        assert_eq!(before, sorted.len(), "{field} kept a duplicate key");
    };

    with_config_lock(|| {
        let app = app_with_state();

        // Door 1: the partial command the frontend actually calls.
        config::update_data_table_view_impl(
            state(&app).inner(),
            Some(hostile_order()),
            Some(hostile_hidden()),
            None,
            None,
            Some(hostile_order()),
            Some(hostile_hidden()),
            None,
            None,
        )
        .expect("update data-table view");
        let panels = config::get_config(state(&app)).panels;
        assert_bounded(&panels.data_table_shot_column_order, "shot_column_order");
        assert_bounded(
            &panels.data_table_shot_hidden_columns,
            "shot_hidden_columns",
        );
        assert_bounded(
            &panels.data_table_station_column_order,
            "station_column_order",
        );
        assert_bounded(
            &panels.data_table_station_hidden_columns,
            "station_hidden_columns",
        );
        // Survivors keep their ORDER - these lists ARE the user's column order,
        // so pruning must never reshuffle what is left.
        assert_eq!(
            panels.data_table_shot_column_order.len(),
            MAX_ENTRIES,
            "the 300 valid keys should truncate to exactly the cap"
        );
        assert_eq!(panels.data_table_shot_column_order[0], "col0");
        assert_eq!(
            panels.data_table_shot_column_order[MAX_ENTRIES - 1],
            "col255"
        );
        assert_eq!(panels.data_table_shot_hidden_columns, vec!["left"]);

        // Same door, the new-section dialog's own lists.
        config::update_new_section_view_impl(
            state(&app).inner(),
            Some(hostile_order()),
            Some(hostile_hidden()),
            None,
        )
        .expect("update new-section view");
        let panels = config::get_config(state(&app)).panels;
        assert_bounded(&panels.new_section_column_order, "new_section_column_order");
        assert_bounded(
            &panels.new_section_hidden_columns,
            "new_section_hidden_columns",
        );

        config::update_edit_section_view_impl(
            state(&app).inner(),
            Some(hostile_order()),
            Some(hostile_hidden()),
            None,
        )
        .expect("update edit-section view");
        let panels = config::get_config(state(&app)).panels;
        assert_bounded(
            &panels.edit_section_column_order,
            "edit_section_column_order",
        );
        assert_bounded(
            &panels.edit_section_hidden_columns,
            "edit_section_hidden_columns",
        );

        // Door 2: the whole-`Config` write, which does no per-field validation of
        // its own and so relies entirely on the normalize pass.
        with_restored_config(&app, || {
            let mut cfg = config::get_config(state(&app));
            cfg.panels.data_table_shot_column_order = hostile_order();
            cfg.panels.data_table_shot_hidden_columns = hostile_hidden();
            cfg.panels.data_table_station_column_order = hostile_order();
            cfg.panels.data_table_station_hidden_columns = hostile_hidden();
            cfg.panels.new_section_column_order = hostile_order();
            cfg.panels.new_section_hidden_columns = hostile_hidden();
            cfg.panels.edit_section_column_order = hostile_order();
            cfg.panels.edit_section_hidden_columns = hostile_hidden();
            config::update_config_impl(state(&app).inner(), cfg).expect("update config");

            let panels = config::get_config(state(&app)).panels;
            assert_bounded(&panels.data_table_shot_column_order, "shot_column_order");
            assert_bounded(
                &panels.data_table_shot_hidden_columns,
                "shot_hidden_columns",
            );
            assert_bounded(
                &panels.data_table_station_column_order,
                "station_column_order",
            );
            assert_bounded(
                &panels.data_table_station_hidden_columns,
                "station_hidden_columns",
            );
            assert_bounded(&panels.new_section_column_order, "new_section_column_order");
            assert_bounded(
                &panels.new_section_hidden_columns,
                "new_section_hidden_columns",
            );
            assert_bounded(
                &panels.edit_section_column_order,
                "edit_section_column_order",
            );
            assert_bounded(
                &panels.edit_section_hidden_columns,
                "edit_section_hidden_columns",
            );
        });
    });
}

/// An angle display mode the PARTIAL command rejects must not sail through the
/// whole-`Config` door. `update_data_table_view_impl` returns `Err` for
/// "sideways", but `update_config` writes a whole `Config` and validated neither
/// mode string - so the same value was refused at one door and persisted cleanly
/// through the other, and the load path never repaired it either.
///
/// The whole-payload door cannot REJECT (it has one field's worth of nothing to
/// report against a 20-block config), so the contract here is repair-to-default,
/// which is also what the frontend already renders for an unknown mode.
#[test]
fn test_update_config_repairs_invalid_angle_modes() {
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            // Precondition: this is exactly the value the partial door refuses.
            assert!(
                config::update_data_table_view_impl(
                    state(&app).inner(),
                    None,
                    None,
                    Some("sideways".to_string()),
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .is_err(),
                "precondition: the partial command rejects 'sideways'"
            );

            let mut cfg = config::get_config(state(&app));
            cfg.panels.data_table_azimuth_mode = "sideways".to_string();
            cfg.panels.data_table_inclination_mode = String::new();
            config::update_config_impl(state(&app).inner(), cfg).expect("update config");

            let panels = config::get_config(state(&app)).panels;
            assert_eq!(
                panels.data_table_azimuth_mode, "avg",
                "an invalid azimuth mode must fall back to its documented default"
            );
            assert_eq!(
                panels.data_table_inclination_mode, "off",
                "an invalid inclination mode must fall back to its documented default"
            );

            // The repair must be a repair, not a reset: a VALID non-default mode
            // still has to survive the same door.
            let mut cfg = config::get_config(state(&app));
            cfg.panels.data_table_azimuth_mode = "inout".to_string();
            cfg.panels.data_table_inclination_mode = "avg".to_string();
            config::update_config_impl(state(&app).inner(), cfg).expect("update config");

            let panels = config::get_config(state(&app)).panels;
            assert_eq!(panels.data_table_azimuth_mode, "inout");
            assert_eq!(panels.data_table_inclination_mode, "avg");
        });
    });
}

/// `update_gas_config` is the partial door for the `[gas]` block, and both halves
/// of what makes it correct are asserted here:
///
/// (a) going partial did not skip normalization - `modify_config` still ends in
///     `normalize_except_shortcuts`, so a negative swim speed (which would later
///     divide into `edge_gas` and poison the whole reachability field with +Inf
///     or NaN) is repaired on the way in, exactly as the whole-`Config` door does;
/// (b) the partial semantics that are the entire point of the command - an
///     unrelated `[panels]` value written just before must SURVIVE. The whole-
///     payload write this replaced carried a stale snapshot of every other field
///     and silently reverted whatever landed inside its debounce window.
#[test]
fn test_update_gas_config_normalizes_and_leaves_other_sections_alone() {
    with_config_lock(|| {
        let app = app_with_state();
        // A value inside the 100..500 clamp, so anything other than 321 coming
        // back means the write was lost rather than clamped.
        config::update_panel_sizes_impl(state(&app).inner(), None, None, Some(321), None)
            .expect("update panel sizes");

        let mut gas = config::get_config(state(&app)).gas;
        gas.swim_speed_m_per_min = -1.0;
        // A valid change alongside it, so the test cannot pass by the command
        // being a no-op.
        gas.include_reference_legs = true;
        config::update_gas_config_impl(state(&app).inner(), gas).expect("update gas config");

        let cfg = config::get_config(state(&app));
        assert_eq!(
            cfg.gas.swim_speed_m_per_min, 10.0,
            "a non-positive swim speed must be repaired to the default, not persisted"
        );
        assert!(
            cfg.gas.include_reference_legs,
            "the valid part of the gas payload must land"
        );
        assert_eq!(
            cfg.panels.data_table_height, 321,
            "a partial gas write must not revert an unrelated [panels] value"
        );
    });
}

#[test]
fn test_get_config_defaults_validation() {
    // Same isolation as above: defaults must not depend on the real config.
    with_config_lock(|| {
        let app = app_with_state();
        let cfg = config::get_config(state(&app));
        assert_eq!(cfg.validation.warn_shot_length_min, 0.3);
        assert_eq!(cfg.validation.warn_shot_length_max, 50.0);
    });
}

/// The `[general]` collections are writable through the whole-`Config` door and
/// nothing pruned them: `onboarding_completed_flows` was deduped but uncapped,
/// `locale` was checked only for emptiness, and `recent_projects` was not touched
/// at all while its own `recent_projects_max` was unclamped. `config.toml` is
/// re-parsed at every launch, so one oversized write is re-read forever.
///
/// The same unbounded-growth shape `normalize_column_names` was written to close,
/// asserted at the same door, and against the PERSISTED value rather than the
/// in-memory one - the caps only matter if they reach disk.
#[test]
fn test_update_config_caps_general_collections() {
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            let mut cfg = config::get_config(state(&app));
            cfg.general.onboarding_completed_flows =
                (0..100_000).map(|i| format!("flow-{i}")).collect();
            cfg.general.locale = "e".repeat(10_000);
            // Large, but still inside i64 and therefore still serializable to
            // TOML. `usize::MAX` would also fail this door - it overflows TOML's
            // integer and the save errors out before any of these assertions are
            // reached - which is a different (and quieter) consequence of the
            // same missing clamp, covered by the unit test in `config::tests`.
            cfg.general.recent_projects_max = 1_000_000;
            cfg.general.recent_projects = (0..100_000)
                .map(|i| PathBuf::from(format!("/caves/{i}.stalx")))
                .collect();
            config::update_config_impl(state(&app).inner(), cfg).expect("update config");

            let general = config::get_config(state(&app)).general;
            assert!(
                general.onboarding_completed_flows.len() <= 256,
                "100k flow ids reached the config: {}",
                general.onboarding_completed_flows.len()
            );
            assert_eq!(
                general.locale, "system",
                "an unusable locale tag must be repaired to its default before it is persisted"
            );
            assert!(
                general.recent_projects_max <= 256,
                "recent_projects_max reached the config unclamped: {}",
                general.recent_projects_max
            );
            assert_eq!(
                general.recent_projects.len(),
                general.recent_projects_max,
                "the recents list must obey the clamped maximum"
            );
        });
    });
}

#[test]
fn test_update_config_roundtrip_auto_save() {
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            let mut cfg = config::get_config(state(&app));
            cfg.general.auto_save = false;
            config::update_config_impl(state(&app).inner(), cfg).expect("update config");
            let updated = config::get_config(state(&app));
            assert!(!updated.general.auto_save);
        });
    });
}

#[test]
fn test_update_config_roundtrip_recent_projects() {
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            let mut cfg = config::get_config(state(&app));
            cfg.general.recent_projects = vec![PathBuf::from("/tmp/example.stalx")];
            config::update_config_impl(state(&app).inner(), cfg).expect("update config");
            let updated = config::get_config(state(&app));
            assert_eq!(updated.general.recent_projects.len(), 1);
            assert!(updated
                .general
                .recent_projects
                .first()
                .unwrap()
                .to_string_lossy()
                .contains("example.stalx"));
        });
    });
}

#[test]
fn test_update_panel_sizes_sections_only() {
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            let before = config::get_config(state(&app));
            config::update_panel_sizes(state(&app), Some(333), None, None, None).unwrap();
            let after = config::get_config(state(&app));
            assert_eq!(after.panels.sections_panel_width, 333);
            assert_eq!(
                after.panels.detail_panel_width,
                before.panels.detail_panel_width
            );
            assert_eq!(
                after.panels.data_table_height,
                before.panels.data_table_height
            );
        });
    });
}

#[test]
fn test_update_panel_sizes_detail_only() {
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            let before = config::get_config(state(&app));
            config::update_panel_sizes(state(&app), None, Some(444), None, None).unwrap();
            let after = config::get_config(state(&app));
            assert_eq!(after.panels.detail_panel_width, 444);
            assert_eq!(
                after.panels.sections_panel_width,
                before.panels.sections_panel_width
            );
            assert_eq!(
                after.panels.data_table_height,
                before.panels.data_table_height
            );
        });
    });
}

#[test]
fn test_update_panel_sizes_data_only() {
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            let before = config::get_config(state(&app));
            // IN-RANGE on purpose (`data_table_height` is clamped to 100..500).
            // This test is about the PARTIAL semantics - only the field passed is
            // touched - so it must not also be straddling the clamp. It used to
            // pass 555 and assert 555 came back, which quietly encoded the old
            // "the write path normalizes nothing" behaviour as the spec: the value
            // reached disk out of range and was repaired on the NEXT load instead.
            // Clamping on the way in is covered by its own test below.
            config::update_panel_sizes(state(&app), None, None, Some(400), None).unwrap();
            let after = config::get_config(state(&app));
            assert_eq!(after.panels.data_table_height, 400);
            assert_eq!(
                after.panels.sections_panel_width,
                before.panels.sections_panel_width
            );
            assert_eq!(
                after.panels.detail_panel_width,
                before.panels.detail_panel_width
            );
        });
    });
}

#[test]
fn test_update_panel_sizes_all() {
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            // All four IN RANGE (150..400, 200..500, 100..500, 280..900) - see
            // the note in `test_update_panel_sizes_data_only`. The old 111 for a
            // field with a 150 floor made this assert that an out-of-range value
            // survives.
            config::update_panel_sizes(state(&app), Some(200), Some(222), Some(333), Some(600))
                .unwrap();
            let after = config::get_config(state(&app));
            assert_eq!(after.panels.sections_panel_width, 200);
            assert_eq!(after.panels.detail_panel_width, 222);
            assert_eq!(after.panels.data_table_height, 333);
            assert_eq!(after.panels.wizard_map_panel_width, 600);
        });
    });
}

/// The behaviour the two tests above USED to contradict: a partial panel-size
/// write is repaired on the way in, not left for the next load to fix.
///
/// Worth its own test rather than folding it into those: they are about the
/// partial semantics (only the field you pass moves), and a fixture that
/// straddles the clamp cannot show both things at once - which is how it went
/// wrong the first time.
#[test]
fn test_update_panel_sizes_clamps_out_of_range_values() {
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            // Below the 150 floor, above the 500 ceiling.
            config::update_panel_sizes(state(&app), Some(1), None, Some(9999), Some(9999)).unwrap();
            let after = config::get_config(state(&app));
            assert_eq!(after.panels.sections_panel_width, 150);
            assert_eq!(after.panels.data_table_height, 500);
            // The wizard map column clamps to 280..900, not 100..500.
            assert_eq!(after.panels.wizard_map_panel_width, 900);
        });
    });
}

#[test]
fn test_update_panel_sizes_none_keeps_values() {
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            let before = config::get_config(state(&app));
            config::update_panel_sizes(state(&app), None, None, None, None).unwrap();
            let after = config::get_config(state(&app));
            assert_eq!(
                after.panels.sections_panel_width,
                before.panels.sections_panel_width
            );
            assert_eq!(
                after.panels.detail_panel_width,
                before.panels.detail_panel_width
            );
            assert_eq!(
                after.panels.data_table_height,
                before.panels.data_table_height
            );
        });
    });
}

#[test]
fn test_update_panel_layout_partial() {
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            let before = config::get_config(state(&app));
            // Update only the sections panel: close it + switch to hover.
            config::update_panel_layout(
                state(&app),
                Some(false),
                Some("hover".to_string()),
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let after = config::get_config(state(&app));
            assert!(!after.panels.sections_panel_open);
            assert_eq!(after.panels.sections_panel_mode, "hover");
            // Other panels untouched.
            assert_eq!(
                after.panels.detail_panel_open,
                before.panels.detail_panel_open
            );
            assert_eq!(after.panels.data_table_mode, before.panels.data_table_mode);
        });
    });
}

#[test]
fn test_update_panel_layout_rejects_invalid_mode() {
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            let result = config::update_panel_layout(
                state(&app),
                None,
                Some("nope".to_string()),
                None,
                None,
                None,
                None,
            );
            assert!(result.is_err());
            // A rejected mode must not have mutated config.
            let after = config::get_config(state(&app));
            assert_eq!(after.panels.sections_panel_mode, "pin");
        });
    });
}

#[test]
fn test_save_display_setting_then_get() {
    let app = app_with_state();
    config::save_display_setting(state(&app), "k1".to_string(), "v1".to_string()).unwrap();
    let value = state(&app).get_display_setting("k1").unwrap();
    assert_eq!(value, Some("v1".to_string()));
}

#[test]
fn test_save_display_setting_overwrites() {
    let app = app_with_state();
    config::save_display_setting(state(&app), "k2".to_string(), "v1".to_string()).unwrap();
    config::save_display_setting(state(&app), "k2".to_string(), "v2".to_string()).unwrap();
    let value = state(&app).get_display_setting("k2").unwrap();
    assert_eq!(value, Some("v2".to_string()));
}

#[test]
fn test_get_display_setting_missing_returns_none() {
    let app = app_with_state();
    let value = state(&app).get_display_setting("missing").unwrap();
    assert_eq!(value, None);
}

#[test]
fn test_load_display_settings_empty_default() {
    let app = app_with_state();
    let values = config::load_display_settings(state(&app)).unwrap();
    assert!(values.is_empty());
}

#[test]
fn test_load_display_settings_returns_all() {
    let app = app_with_state();
    config::save_display_setting(state(&app), "k1".to_string(), "v1".to_string()).unwrap();
    config::save_display_setting(state(&app), "k2".to_string(), "v2".to_string()).unwrap();
    let values = config::load_display_settings(state(&app)).unwrap();
    assert_eq!(values.len(), 2);
    assert_eq!(values.get("k1").cloned(), Some("v1".to_string()));
    assert_eq!(values.get("k2").cloned(), Some("v2".to_string()));
}

// === History commands ===

#[test]
fn test_can_undo_false_initial() {
    let app = app_with_state();
    assert!(!history::can_undo(state(&app)));
}

#[test]
fn test_can_redo_false_initial() {
    let app = app_with_state();
    assert!(!history::can_redo(state(&app)));
}

#[test]
fn test_undo_fails_when_empty() {
    let app = app_with_state();
    let result = history::undo(state(&app));
    assert_failure_contains(&result, "Nothing to undo");
}

#[test]
fn test_redo_fails_when_empty() {
    let app = app_with_state();
    let result = history::redo(state(&app));
    assert_failure_contains(&result, "Nothing to redo");
}

#[test]
fn test_can_undo_true_after_create_section() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    assert!(history::can_undo(state(&app)));
}

#[test]
fn test_undo_removes_section() {
    let app = app_with_state();
    create_project(&app);
    let section_id = create_section_with_shots(&app, vec![shot_data(1.0)]);
    let undo_result = history::undo(state(&app));
    assert_success(&undo_result);
    let section = get_section(state(&app), section_id);
    assert!(section.is_none());
}

#[test]
fn test_can_redo_true_after_undo() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let _ = history::undo(state(&app));
    assert!(history::can_redo(state(&app)));
}

#[test]
fn test_redo_restores_section() {
    let app = app_with_state();
    create_project(&app);
    let _section_id = create_section_with_shots(&app, vec![shot_data(1.0)]);
    let _ = history::undo(state(&app));
    let redo_result = history::redo(state(&app));
    assert_success(&redo_result);
    let project_state = project::get_project_state(state(&app)).expect("project state");
    assert_eq!(project_state.project.sections.len(), 1);
}

#[test]
fn test_undo_to_fails_when_index_out_of_bounds() {
    let app = app_with_state();
    create_project(&app);
    // Create one section - only 1 item in undo stack
    create_section_with_shots(&app, vec![shot_data(1.0)]);

    // Index 0 should work (1 item), but index 1 should fail
    let result = history::undo_to(1, state(&app));
    assert!(!result.success);
    assert!(result.error.unwrap().contains("Invalid undo index"));
}

#[test]
fn test_undo_to_index_0_undoes_one_operation() {
    let app = app_with_state();
    create_project(&app);
    let section_id = create_section_with_shots(&app, vec![shot_data(1.0)]);

    // Undo to index 0 should undo just the most recent action
    let result = history::undo_to(0, state(&app));
    assert_success(&result);

    // Section should be removed
    let section = get_section(state(&app), section_id);
    assert!(section.is_none());
}

#[test]
fn test_undo_to_index_2_undoes_three_operations() {
    let app = app_with_state();
    create_project(&app);

    // Create 3 sections (3 undo entries)
    let _section_1 = create_section_with_shots(&app, vec![shot_data(1.0)]);
    let _section_2 = create_section_with_shots(&app, vec![shot_data(2.0)]);
    let _section_3 = create_section_with_shots(&app, vec![shot_data(3.0)]);

    // Verify we have 3 sections
    let project_state = project::get_project_state(state(&app)).expect("project state");
    assert_eq!(project_state.project.sections.len(), 3);

    // Undo to index 2 should undo all 3 sections (indices 0, 1, 2)
    let result = history::undo_to(2, state(&app));
    assert_success(&result);

    // All sections should be removed
    let project_state = project::get_project_state(state(&app)).expect("project state");
    assert_eq!(project_state.project.sections.len(), 0);
}

#[test]
fn test_redo_to_fails_when_index_out_of_bounds() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    // Undo to create a redo entry
    let _ = history::undo(state(&app));

    // Index 0 should work (1 item), but index 1 should fail
    let result = history::redo_to(1, state(&app));
    assert!(!result.success);
    assert!(result.error.unwrap().contains("Invalid redo index"));
}

#[test]
fn test_redo_to_index_0_redoes_one_operation() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let _ = history::undo(state(&app));

    // Verify section is gone
    let project_state = project::get_project_state(state(&app)).expect("project state");
    assert_eq!(project_state.project.sections.len(), 0);

    // Redo to index 0 should redo just the most recent undone action
    let result = history::redo_to(0, state(&app));
    assert_success(&result);

    // Section should be restored
    let project_state = project::get_project_state(state(&app)).expect("project state");
    assert_eq!(project_state.project.sections.len(), 1);
}

#[test]
fn test_redo_to_index_2_redoes_three_operations() {
    let app = app_with_state();
    create_project(&app);

    // Create 3 sections
    let _section_1 = create_section_with_shots(&app, vec![shot_data(1.0)]);
    let _section_2 = create_section_with_shots(&app, vec![shot_data(2.0)]);
    let _section_3 = create_section_with_shots(&app, vec![shot_data(3.0)]);

    // Undo all 3
    let _ = history::undo_to(2, state(&app));

    // Verify all sections are gone
    let project_state = project::get_project_state(state(&app)).expect("project state");
    assert_eq!(project_state.project.sections.len(), 0);

    // Redo to index 2 should redo all 3 sections (indices 0, 1, 2)
    let result = history::redo_to(2, state(&app));
    assert_success(&result);

    // All sections should be restored
    let project_state = project::get_project_state(state(&app)).expect("project state");
    assert_eq!(project_state.project.sections.len(), 3);
}

#[test]
fn test_undo_to_fails_when_stack_empty() {
    let app = app_with_state();
    // No project or actions
    let result = history::undo_to(0, state(&app));
    assert!(!result.success);
    assert!(result.error.unwrap().contains("Invalid undo index"));
}

#[test]
fn test_redo_to_fails_when_stack_empty() {
    let app = app_with_state();
    // No project or actions, so no redo available
    let result = history::redo_to(0, state(&app));
    assert!(!result.success);
    assert!(result.error.unwrap().contains("Invalid redo index"));
}

// === Project commands ===

#[test]
fn test_get_project_state_none_when_no_project() {
    let app = app_with_state();
    assert!(project::get_project_state(state(&app)).is_none());
}

/// The depth stats the frontend gets are the ABSOLUTE station range, matching
/// the station z on the very same payload - not the per-section delta walk that
/// reported 0..3 for a survey that never leaves 5..8 m (#481).
#[test]
fn test_get_project_state_depth_stats_match_station_z() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(
        &app,
        vec![
            shot_data_with_depth(10.0, 5.0, 6.0),
            shot_data_with_depth(10.0, 6.0, 8.0),
        ],
    );

    let project_state = project::get_project_state(state(&app)).expect("project state");
    let depths: Vec<f64> = project_state
        .stations
        .iter()
        .map(|s| s.coordinates.z)
        .collect();
    assert_eq!(depths, vec![5.0, 6.0, 8.0]);
    assert_eq!(project_state.stats.min_depth, 5.0);
    assert_eq!(project_state.stats.max_depth, 8.0);
}

#[test]
fn test_close_project_clears_state() {
    let app = app_with_state();
    create_project(&app);
    assert_success(&project::close_project_impl(
        state(&app).inner(),
        project::CloseLock::Release,
    ));
    assert!(project::get_project_state(state(&app)).is_none());
}

#[test]
fn test_save_project_fails_when_no_project() {
    let app = app_with_state();
    let result = project::save_project(state(&app), Some(PathBuf::from("/tmp/nope.stalx")), None);
    assert_failure_contains(&result, "No project open");
}

#[test]
fn test_open_project_fails_when_missing() {
    let app = app_with_state();
    let result = open_project(state(&app), PathBuf::from("/tmp/missing.stalx"));
    assert_failure_contains(&result, "File not found");
}

#[test]
fn test_save_project_creates_stalx_extension() {
    with_config_lock(|| {
        let app = app_with_state();
        create_project(&app);
        let dir = TempDir::new().unwrap();
        let raw_path = dir.path().join("project_data");
        let result = project::save_project(state(&app), Some(raw_path), None);
        assert_success(&result);
        let saved_path = dir.path().join("project_data.stalx");
        assert!(saved_path.exists());
    });
}

#[test]
fn test_open_project_loads_saved_project() {
    with_config_lock(|| {
        let app = app_with_state();
        create_project(&app);
        let dir = TempDir::new().unwrap();
        let saved_path = save_project_to_temp(&app, &dir);

        let app2 = app_with_state();
        let result = open_project(state(&app2), saved_path);
        assert_success(&result);
        let project_state = project::get_project_state(state(&app2)).expect("project state");
        assert_eq!(project_state.project.name, "Test Cave");
    });
}

#[test]
fn test_project_lifecycle_persists_undo_and_state() {
    with_config_lock(|| {
        let app = app_with_state();
        create_project(&app);
        let section_a = create_section_with_shots(&app, vec![shot_data(10.0), shot_data(5.0)]);
        let section_b = create_section_with_shots(&app, vec![shot_data(7.0)]);

        let merge_result = station::merge_stations(
            state(&app),
            StationRef::new(section_a, 0),
            StationRef::new(section_b, 0),
        );
        assert_success(&merge_result);

        let gps = GpsCoordinates::new(10.0, 20.0, 0.0).unwrap();
        let datum_result = station::set_zero_datum(state(&app), StationRef::new(section_a, 0), gps);
        assert_success(&datum_result);

        let updates = ShotUpdates {
            length: Some(12.5),
            comment: Some(Some("recalc".to_string())),
            ..Default::default()
        };
        let update_result = shot::update_shot(state(&app), section_a, 0, updates);
        assert_success(&update_result);

        let undo_before = history::get_undo_stack(state(&app));
        assert!(!undo_before.is_empty());
        assert_eq!(undo_before[0].command_type, "update_shot");

        let dir = TempDir::new().unwrap();
        let saved_path = save_project_to_temp(&app, &dir);

        // Wait for background undo persistence thread to complete
        std::thread::sleep(std::time::Duration::from_millis(200));

        assert_success(&project::close_project_impl(
            state(&app).inner(),
            project::CloseLock::Release,
        ));
        let open_result = open_project(state(&app), saved_path);
        assert_success(&open_result);

        let project_state = project::get_project_state(state(&app)).expect("project state");
        assert_eq!(project_state.project.sections.len(), 2);
        assert_eq!(project_state.project.station_merges.len(), 1);
        assert_eq!(project_state.project.zero_datums.len(), 1);

        let section = project_state
            .project
            .get_section(section_a)
            .expect("section a");
        assert!((section.shots[0].length - 12.5).abs() < f64::EPSILON);
        assert_eq!(section.shots[0].comment.as_deref(), Some("recalc"));

        let undo_after = history::get_undo_stack(state(&app));
        assert_eq!(undo_after.len(), undo_before.len());
        assert_eq!(undo_after[0].command_type, "update_shot");
    });
}

#[test]
fn test_get_project_state_emits_positional_wire_refs() {
    with_config_lock(|| {
        let app = app_with_state();
        create_project(&app);
        let section_a = create_section_with_shots(&app, vec![shot_data(10.0), shot_data(5.0)]);
        let section_b = create_section_with_shots(&app, vec![shot_data(7.0)]);
        // Merge two AfterShot stations across sections so the wire form must
        // resolve a non-trivial positional index.
        assert_success(&station::merge_stations(
            state(&app),
            StationRef::new(section_a, 1),
            StationRef::new(section_b, 1),
        ));

        let project_state = project::get_project_state(state(&app)).expect("project state");

        // The IPC payload (ProjectState) must carry the positional StationRef
        // wire shape: `station_index` present, no StationKey `anchor`.
        let ipc = serde_json::to_value(&project_state).unwrap();
        let merge = &ipc["project"]["station_merges"][0];
        assert!(
            merge["station_a"]["station_index"].is_number(),
            "wire merge endpoint must expose station_index, got: {merge}"
        );
        assert!(
            merge["station_a"].get("anchor").is_none(),
            "wire merge endpoint must NOT expose the StationKey anchor, got: {merge}"
        );

        // Regression guard: the raw Project serde (the undo snapshot path) must
        // STILL emit the lossless StationKey form. The two encodings diverge by
        // design - only the IPC view is positional.
        let raw = serde_json::to_string(&project_state.project).unwrap();
        assert!(
            raw.contains("anchor"),
            "raw Project serde must keep StationKey anchors for undo snapshots"
        );
    });
}

#[test]
#[cfg(target_os = "linux")]
fn test_open_project_updates_recent_projects_when_config_save_fails() {
    use std::os::unix::fs::PermissionsExt;

    // Skip when running as root - file permission restrictions don't apply to UID 0
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("Skipping: test requires non-root (chmod 555 has no effect as root)");
        return;
    }

    // Resolve the config path for REAL rather than into this thread's own temp
    // dir: this test drives config_path() itself by pointing XDG_CONFIG_HOME at
    // a read-only dir to force a save failure. The env is process-wide, so the
    // guard also serializes against the other real-resolution tests.
    let _config_guard = crate::test_helpers::real_config_resolution();
    {
        let readonly_dir = TempDir::new().unwrap();
        let mut perms = fs::metadata(readonly_dir.path()).unwrap().permissions();
        perms.set_mode(0o555);
        fs::set_permissions(readonly_dir.path(), perms).unwrap();

        let _env_guard = EnvVarGuard::set("XDG_CONFIG_HOME", readonly_dir.path().to_str().unwrap());

        let app = app_with_state();
        let project_dir = TempDir::new().unwrap();
        let project_path = project_dir.path().join("config-fail.stalx");

        let mut project = Project::new("Config Fail".to_string(), None);
        project.add_section(SectionData {
            shots: vec![shot_data(1.0)],
            ..Default::default()
        });
        let mut db = Database::open(&project_path).unwrap();
        db.save_project(&project).unwrap();
        drop(db);

        let result = open_project(state(&app), project_path.clone());
        assert_success(&result);

        let cfg = config::get_config(state(&app));
        assert_eq!(cfg.general.recent_projects.first(), Some(&project_path));

        let config_path = crate::config::Config::config_path().unwrap();
        assert!(
            !config_path.exists(),
            "config file should not be written when config dir is read-only"
        );

        // Reset permissions so TempDir can clean up.
        let mut perms = fs::metadata(readonly_dir.path()).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(readonly_dir.path(), perms).unwrap();
    }
}

/// macOS sibling of the Linux test above. On macOS the `directories` crate ignores
/// XDG_CONFIG_HOME, deriving paths from $HOME instead. We override HOME to a
/// read-only temp dir so config.save() fails, then verify in-memory state is
/// still updated.
#[test]
#[cfg(target_os = "macos")]
fn test_open_project_updates_recent_projects_when_config_save_fails_macos() {
    use std::os::unix::fs::PermissionsExt;

    if unsafe { libc::geteuid() } == 0 {
        eprintln!("Skipping: test requires non-root (chmod 555 has no effect as root)");
        return;
    }

    // Real resolution (not this thread's temp dir): drives config_path() via HOME.
    let _config_guard = crate::test_helpers::real_config_resolution();
    {
        let readonly_dir = TempDir::new().unwrap();
        let mut perms = fs::metadata(readonly_dir.path()).unwrap().permissions();
        perms.set_mode(0o555);
        fs::set_permissions(readonly_dir.path(), perms).unwrap();

        // On macOS, directories crate uses ~/Library/Application Support/...
        // Setting HOME to a read-only dir makes create_dir_all fail for config save.
        let _env_guard = EnvVarGuard::set("HOME", readonly_dir.path().to_str().unwrap());

        let app = app_with_state();
        let project_dir = TempDir::new().unwrap();
        let project_path = project_dir.path().join("config-fail.stalx");

        let mut project = Project::new("Config Fail".to_string(), None);
        project.add_section(SectionData {
            shots: vec![shot_data(1.0)],
            ..Default::default()
        });
        let mut db = Database::open(&project_path).unwrap();
        db.save_project(&project).unwrap();
        drop(db);

        let result = open_project(state(&app), project_path.clone());
        assert_success(&result);

        let cfg = config::get_config(state(&app));
        assert_eq!(cfg.general.recent_projects.first(), Some(&project_path));

        let config_path = crate::config::Config::config_path().unwrap();
        assert!(
            !config_path.exists(),
            "config file should not be written when HOME points to a read-only dir"
        );

        // Reset permissions so TempDir can clean up.
        let mut perms = fs::metadata(readonly_dir.path()).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(readonly_dir.path(), perms).unwrap();
    }
}

#[test]
fn test_open_project_updates_recent_projects_order_and_truncates() {
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            let mut cfg = config::get_config(state(&app));
            cfg.general.recent_projects_max = 2;
            cfg.general.recent_projects = Vec::new();
            config::update_config_impl(state(&app).inner(), cfg).expect("update config");

            let dir = TempDir::new().unwrap();
            let path_a = dir.path().join("a.stalx");
            let path_b = dir.path().join("b.stalx");
            let path_c = dir.path().join("c.stalx");

            for (path, name) in [(&path_a, "A"), (&path_b, "B"), (&path_c, "C")] {
                let mut project = Project::new(name.to_string(), None);
                project.add_section(SectionData {
                    shots: vec![shot_data(1.0)],
                    ..Default::default()
                });
                let mut db = Database::open(path).unwrap();
                db.save_project(&project).unwrap();
            }

            assert_success(&open_project(state(&app), path_a));
            assert_success(&open_project(state(&app), path_b.clone()));
            assert_success(&open_project(state(&app), path_c.clone()));

            let cfg = config::get_config(state(&app));
            assert_eq!(
                cfg.general.recent_projects,
                vec![path_c.clone(), path_b.clone()]
            );

            assert_success(&open_project(state(&app), path_b.clone()));
            let cfg = config::get_config(state(&app));
            assert_eq!(cfg.general.recent_projects, vec![path_b, path_c]);
        });
    });
}

#[test]
fn test_save_project_persists_sections() {
    with_config_lock(|| {
        let app = app_with_state();
        create_project(&app);
        create_section_with_shots(&app, vec![shot_data(1.0)]);
        let dir = TempDir::new().unwrap();
        let saved_path = save_project_to_temp(&app, &dir);

        let app2 = app_with_state();
        let _ = open_project(state(&app2), saved_path);
        let project_state = project::get_project_state(state(&app2)).expect("project state");
        assert_eq!(project_state.project.sections.len(), 1);
    });
}

#[test]
fn test_save_project_fails_when_output_is_directory() {
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            create_project(&app);
            let dir = TempDir::new().unwrap();
            let blocked = dir.path().join("blocked.stalx");
            fs::create_dir_all(&blocked).unwrap();

            let result = project::save_project(state(&app), Some(blocked.clone()), None);
            assert_failure_contains(&result, "Failed to finalize save");

            let temp_path = blocked.with_extension("stalx.tmp");
            assert!(!temp_path.exists());

            let project_state = state(&app).inner().get_project_state().unwrap();
            assert!(project_state.file_path.is_none());
        });
    });
}

// #92: importing into a never-saved (deferred) project must durably persist it -
// write the .stalx and add it to Recent Projects - at the `import_sections`
// command every import path funnels through, so the fix survives changes to the
// import UI. Guards the backend chokepoint (PR #104).
#[test]
fn test_import_sections_persists_deferred_new_project() {
    with_config_lock(|| {
        let app = app_with_state();
        let dir = TempDir::new().unwrap();
        let project_path = dir.path().join("new-import.stalx");

        // A deferred project: its save path is known but it has never been written
        // (the AppState database handle stays None).
        let mut project = Project::new("New Import".to_string(), None);
        project.file_path = Some(project_path.clone());
        state(&app).inner().set_project_for_tests(project);
        state(&app)
            .inner()
            .set_initial_save_deferred_for_tests(true);
        assert!(
            state(&app).initial_save_deferred(),
            "precondition: project must be deferred (never saved)"
        );
        assert!(
            !project_path.exists(),
            "precondition: .stalx must not exist yet"
        );

        // Import one origin-placed section through the command.
        let sections = vec![crate::models::ImportSectionConfig {
            reverse: false,
            section_index: 0,
            name: Some("Imported".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![shot_data(5.0)],
            station_comments: Vec::new(),
            station_names: Vec::new(),
            placement: crate::models::ImportPlacement::Origin,
            azimuth_reference: None,
        }];
        let result = import::import_sections(
            state(&app),
            sections,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert_success(&result);

        // The command resolved the deferred save: file on disk + in Recent Projects.
        assert!(
            project_path.exists(),
            "import must write the deferred project to disk"
        );
        let cfg = config::get_config(state(&app));
        assert_eq!(cfg.general.recent_projects.first(), Some(&project_path));
        assert!(
            !state(&app).initial_save_deferred(),
            "project should no longer be deferred after the import-triggered save"
        );

        let receipt = result
            .import_receipt
            .as_ref()
            .expect("import must attach an import receipt");
        assert_eq!(receipt.sections_imported, 1);
        assert_eq!(receipt.shot_count, 1);
    });
}

/// #674: `add_project_file` is an import door too - into a never-saved project
/// it resolves the deferred save, and its result carries exactly the added
/// section ids (the orphan-placement input) plus a receipt.
#[test]
fn test_add_project_file_reports_added_sections_and_persists_deferred_project() {
    with_config_lock(|| {
        let app = app_with_state();
        let dir = TempDir::new().unwrap();
        let project_path = dir.path().join("new-add.stalx");
        let mut project = Project::new("New Add".to_string(), None);
        project.file_path = Some(project_path.clone());
        state(&app).inner().set_project_for_tests(project);
        state(&app)
            .inner()
            .set_initial_save_deferred_for_tests(true);

        let mut source = Project::new("Source".to_string(), None);
        source.add_section(SectionData {
            name: Some("Added".to_string()),
            shots: vec![shot_data(5.0), shot_data(6.0)],
            ..Default::default()
        });
        let source_path = dir.path().join("source.stalx");
        Database::open(&source_path)
            .unwrap()
            .save_project(&source)
            .unwrap();

        // Through the async command itself, so the blocking-thread handoff is
        // what is tested; that thread resolves its own temp config dir.
        let result = tauri::async_runtime::block_on(import::add_project_file(
            app.handle().clone(),
            source_path.to_string_lossy().into_owned(),
        ));
        assert_success(&result);
        assert_eq!(result.section_ids, vec![0], "exactly the added section");
        let receipt = result.import_receipt.as_ref().expect("a receipt");
        assert_eq!(receipt.source_format, "Stalagmight");
        assert_eq!(receipt.file_name, "source.stalx");
        assert_eq!(receipt.sections_imported, 1);
        assert_eq!(receipt.shot_count, 2);
        assert!(project_path.exists(), "the deferred project was saved");
        assert!(!state(&app).initial_save_deferred());
    });
}

/// #674: `suggest_orphan_placements` runs on a blocking thread; driven through
/// the async command, a newly imported floating section that re-surveys an
/// anchored one is offered that section as its target.
#[test]
fn test_suggest_orphan_placements_command_offers_the_anchored_host() {
    with_config_lock(|| {
        let app = app_with_state();
        let leg = || -> Vec<ShotData> {
            [(45.0, 10.0), (90.0, 8.0), (135.0, 6.0), (180.0, 4.0)]
                .iter()
                .map(|&(azimuth, length)| ShotData {
                    azimuth_in: azimuth,
                    azimuth_out: azimuth,
                    length,
                    depth_in: 2.0,
                    depth_out: 2.5,
                    ..Default::default()
                })
                .collect()
        };
        let mut project = Project::new("Host".to_string(), None);
        let host = project
            .add_section(SectionData {
                shots: leg(),
                ..Default::default()
            })
            .id;
        project
            .store_zero_datum_without_repositioning(
                StationRef::new(host, 0),
                GpsCoordinates::new(20.5, -87.3, 0.0).unwrap(),
            )
            .unwrap();
        let added = project
            .add_section(SectionData {
                shots: leg(),
                ..Default::default()
            })
            .id;
        state(&app).inner().set_project_for_tests(project);

        let suggestions = tauri::async_runtime::block_on(import::suggest_orphan_placements(
            app.handle().clone(),
            vec![added],
        ))
        .unwrap();

        assert!(
            suggestions
                .iter()
                .any(|s| s.orphan_section_id == added && s.target_section_id == host),
            "the added section must be offered the anchored host: {suggestions:?}"
        );
    });
}

/// #391: `import_sections`' additive provenance params (used by the mNemo DMP
/// wizard) decorate the base receipt when passed, and leave it at its
/// (empty-provenance) defaults when omitted - the shape every other caller
/// still uses.
#[test]
fn test_import_sections_provenance_params_decorate_receipt() {
    with_config_lock(|| {
        let app = app_with_state();
        state(&app).execute(Command::CreateProject {
            name: "Test".to_string(),
            location: None,
        });

        let sections = vec![crate::models::ImportSectionConfig {
            reverse: false,
            section_index: 0,
            name: Some("Imported".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![shot_data(5.0)],
            station_comments: Vec::new(),
            station_names: Vec::new(),
            placement: crate::models::ImportPlacement::Origin,
            azimuth_reference: None,
        }];

        let result = import::import_sections(
            state(&app),
            sections,
            None,
            None,
            None,
            None,
            Some("mNemo DMP".to_string()),
            Some("dive.dmp".to_string()),
            Some(1),
        );
        assert_success(&result);
        let receipt = result
            .import_receipt
            .as_ref()
            .expect("import must attach an import receipt");
        assert_eq!(receipt.source_format, "mNemo DMP");
        assert_eq!(receipt.file_name, "dive.dmp");
        assert_eq!(receipt.sections_found, 1);
    });
}

/// PR #529 Codex review: `import_sections` (the mNemo DMP wizard's actual IPC
/// entry point) never calls `decorate_receipt` - it only overwrites
/// source_format/file_name/sections_found directly (see above and
/// `import::import_sections`). A `zero_datum_created` sanitized ONLY in
/// `decorate_receipt` would therefore reach the frontend as a raw,
/// file-controlled `ImportSectionConfig::name` through this exact command.
/// Drives the real command with a hostile name (a bidi override + 10k chars)
/// and asserts the receipt field arrives clean - proof this route, not just
/// `decorate_receipt`'s callers, is sanitized.
#[test]
fn test_import_sections_sanitizes_hostile_zero_datum_section_name() {
    with_config_lock(|| {
        let app = app_with_state();
        state(&app).execute(Command::CreateProject {
            name: "Test".to_string(),
            location: None,
        });

        let hostile_name = format!("A\u{202e}B{}", "x".repeat(10_000));
        let sections = vec![crate::models::ImportSectionConfig {
            reverse: false,
            section_index: 0,
            name: Some(hostile_name),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![shot_data(5.0)],
            station_comments: Vec::new(),
            station_names: Vec::new(),
            placement: crate::models::ImportPlacement::ZeroDatum {
                coordinates: GpsCoordinates::new(10.0, 20.0, 0.0).unwrap(),
                station_index: 0,
            },
            azimuth_reference: None,
        }];

        let result = import::import_sections(
            state(&app),
            sections,
            None,
            None,
            None,
            None,
            Some("mNemo DMP".to_string()),
            Some("dive.dmp".to_string()),
            Some(1),
        );
        assert_success(&result);

        let receipt = result
            .import_receipt
            .as_ref()
            .expect("import must attach an import receipt");
        let name = receipt
            .zero_datum_created
            .as_ref()
            .expect("field must still be Some after sanitizing");
        assert!(
            !name.contains('\u{202e}'),
            "bidi override must be stripped: {name:?}"
        );
        assert!(
            name.chars().count() <= 120,
            "must clamp to MAX_FILE_NAME_LEN chars, got {}",
            name.chars().count()
        );
    });
}

#[test]
fn test_import_sections_without_provenance_params_leaves_receipt_defaults() {
    with_config_lock(|| {
        let app = app_with_state();
        state(&app).execute(Command::CreateProject {
            name: "Test".to_string(),
            location: None,
        });

        let sections = vec![crate::models::ImportSectionConfig {
            reverse: false,
            section_index: 0,
            name: Some("Imported".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![shot_data(5.0)],
            station_comments: Vec::new(),
            station_names: Vec::new(),
            placement: crate::models::ImportPlacement::Origin,
            azimuth_reference: None,
        }];

        let result = import::import_sections(
            state(&app),
            sections,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        assert_success(&result);
        let receipt = result
            .import_receipt
            .as_ref()
            .expect("import must attach an import receipt");
        assert_eq!(receipt.sections_imported, 1);
        assert_eq!(receipt.source_format, "");
        assert_eq!(receipt.file_name, "");
        assert_eq!(receipt.sections_found, 0);
    });
}

/// #391: `execute_gpx_import` decorates the import receipt with source format
/// "GPX", no unit conversion, and `sections_found` counting the tracks/routes/
/// waypoints actually selected for import.
#[test]
fn test_execute_gpx_import_decorates_receipt() {
    with_config_lock(|| {
        let app = app_with_state();
        state(&app).execute(Command::CreateProject {
            name: "Test".to_string(),
            location: None,
        });

        let gpx_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="test" xmlns="http://www.topografix.com/GPX/1/1">
  <trk>
    <name>Surface Path</name>
    <trkseg>
      <trkpt lat="45.5" lon="-90.5"><ele>350.2</ele></trkpt>
      <trkpt lat="45.5005" lon="-90.4995"><ele>351.0</ele></trkpt>
      <trkpt lat="45.501" lon="-90.499"><ele>348.0</ele></trkpt>
    </trkseg>
  </trk>
</gpx>"#;
        let mut file = NamedTempFile::with_suffix(".gpx").unwrap();
        use std::io::Write;
        file.write_all(gpx_xml.as_bytes()).unwrap();

        let result = import::execute_gpx_import(
            state(&app),
            file.path().to_path_buf(),
            None,
            false,
            true,
            false,
            Vec::new(),
            false,
        );
        assert_success(&result);
        let receipt = result
            .import_receipt
            .as_ref()
            .expect("import must attach an import receipt");
        assert_eq!(receipt.source_format, "GPX");
        assert_eq!(receipt.sections_found, 1);
        assert_eq!(receipt.unit_conversion, None);
    });
}

/// #411: an import whose sections all vanished before they reached the command
/// (every shot excluded, or every section malformed) lands NOTHING. It must
/// still return a receipt - a zero one - because `decorate_receipt` hangs the
/// converter's "why" on that receipt and had nothing to hang it on when the
/// command returned a bare success. A silent, successful, empty import is the
/// exact failure this closes.
#[test]
fn test_import_sections_with_nothing_to_import_still_reports_a_receipt() {
    with_config_lock(|| {
        let app = app_with_state();
        state(&app).execute(Command::CreateProject {
            name: "Test".to_string(),
            location: None,
        });

        let result = import::import_sections(
            state(&app),
            Vec::new(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );

        assert_success(&result);
        let receipt = result
            .import_receipt
            .as_ref()
            .expect("an import that landed nothing must still report a receipt");
        assert_eq!(receipt.sections_imported, 0);
        assert_eq!(receipt.shot_count, 0);
        assert_eq!(receipt.closures_requested, 0);
        assert!(receipt.warnings.is_empty());
    });
}

// #92 / #112: the adversarial twin of the success test above. When the deferred
// save *fails* (here: the save's parent directory is missing, so the temp
// `.stalx.tmp` can't be created), `finalize_deferred_import` must return a PARTIAL
// success -
// keep the import's `success` (so the frontend refreshes + marks dirty, keeping
// the in-memory import under the close-guard), attach a warning, and never
// phantom-add the never-written project to Recent Projects. A hard failure (or a
// silent "saved") would put us back in the #92 data-loss class.
#[test]
fn test_import_sections_deferred_save_failure_warns_no_phantom() {
    with_config_lock(|| {
        let app = app_with_state();
        let dir = TempDir::new().unwrap();
        // Force the deferred save to fail: point it at a path whose PARENT directory
        // doesn't exist, so the atomic save can't create the temp `.stalx.tmp`
        // (`Database::open` returns "Failed to create save file"). NB a directory
        // squatting the *exact* reserved path does NOT fail - the first implicit save
        // auto-renames around a collision (`next_available_project_path`) and
        // succeeds; and a read-only dir wouldn't fail under the root-in-container CI
        // runner. A missing parent fails the create regardless of uid. (This drives the
        // GENERIC save-failure branch; the permission-denied humanization in
        // `finalize_deferred_import` needs `dir_present && EACCES`, which isn't
        // root-safe to induce here - that sub-branch is covered by the
        // permission-denied twin test below.)
        let project_path = dir.path().join("missing-subdir").join("new-import.stalx");

        let mut project = Project::new("New Import".to_string(), None);
        project.file_path = Some(project_path.clone());
        state(&app).inner().set_project_for_tests(project);
        state(&app)
            .inner()
            .set_initial_save_deferred_for_tests(true);
        assert!(
            state(&app).initial_save_deferred(),
            "precondition: project must be deferred (never saved)"
        );

        // Import one origin-placed section through the command - same shape as the
        // success test, only the save target differs.
        let sections = vec![crate::models::ImportSectionConfig {
            reverse: false,
            section_index: 0,
            name: Some("Imported".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![shot_data(5.0)],
            station_comments: Vec::new(),
            station_names: Vec::new(),
            placement: crate::models::ImportPlacement::Origin,
            azimuth_reference: None,
        }];
        let result = import::import_sections(
            state(&app),
            sections,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );

        // Partial success: the import itself still succeeds, but a warning flags the
        // failed save instead of reporting a phantom "saved".
        assert_success(&result);
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.message.contains("could not save")),
            "a failed deferred save must surface a warning, not a phantom save: {:?}",
            result.warnings
        );

        // No phantom: the never-written project must NOT appear in Recent Projects,
        // and it must remain deferred (still unsaved) so the app keeps prompting.
        let cfg = config::get_config(state(&app));
        assert!(
            !cfg.general.recent_projects.contains(&project_path),
            "a project whose save failed must not be added to Recent Projects"
        );
        assert!(
            state(&app).initial_save_deferred(),
            "a failed deferred save must leave the project deferred (still unsaved)"
        );

        // ...and the just-imported section must still be in memory: the partial-
        // success contract keeps the import under the close-guard so it isn't lost.
        // A regression that returns success+warning while dropping the in-memory
        // import would be the #92 data-loss class on the failure path - assert the
        // CommandResult flag (above) is NOT enough on its own.
        let project_state = state(&app).inner().get_project_state().unwrap();
        assert_eq!(
            project_state.sections.len(),
            1,
            "the imported section must survive in memory after a failed deferred save"
        );

        // The import receipt is orthogonal to the deferred-save-failure warning: the
        // receipt still reports the import went through, and its warnings list stays
        // empty (the deferred-save failure is a "SYSTEM" CommandResult warning, never
        // an ImportReceipt warning - see ImportReceipt's doc comment).
        assert!(
            result.import_receipt.is_some(),
            "a failed deferred save must not suppress the import receipt"
        );
        assert_eq!(
            result
                .warnings
                .iter()
                .filter(|w| w.code == "SYSTEM")
                .count(),
            1,
            "exactly one SYSTEM warning for the deferred-save failure: {:?}",
            result.warnings
        );
    });
}

// #92 / #112: the permission-denied face of the failure path - the macOS "folder
// access revoked" scenario the deferred-save warning was built for. When the save
// fails with a permission-classified error, `finalize_deferred_import` must HUMANIZE
// it ("permission denied (check folder access ...)") and must NOT leak the internal
// `FS_PERMISSION_DENIED` marker into the user-facing warning (import.rs:143-152).
//
// A real EACCES is the only route to that branch since #429: SQLite's generic
// "unable to open database file" is no longer read as a denial on its own (a
// directory squatting the temp path produces it too, and that is not a
// permission problem). So this is unix-only, and skips itself when the mode bits
// don't bite (container-root holds CAP_DAC_OVERRIDE) rather than false-failing -
// the same idiom as `save_into_unwritable_dir_reports_permission_code`. The
// partial-success invariants (no phantom, in-memory survival, still-deferred) are
// already covered by the generic-failure twin above; this test focuses on the
// message mapping.
#[cfg(unix)]
#[test]
fn test_import_sections_deferred_save_permission_denied_humanizes_no_marker_leak() {
    use std::os::unix::fs::PermissionsExt;
    with_config_lock(|| {
        let app = app_with_state();
        let dir = TempDir::new().unwrap();
        let locked = dir.path().join("locked");
        fs::create_dir_all(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        // Restore perms first so TempDir cleanup can remove the directory.
        if fs::File::create(locked.join(".probe")).is_ok() {
            let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o755));
            return;
        }
        let project_path = locked.join("new-import.stalx");

        let mut project = Project::new("New Import".to_string(), None);
        project.file_path = Some(project_path.clone());
        state(&app).inner().set_project_for_tests(project);
        state(&app)
            .inner()
            .set_initial_save_deferred_for_tests(true);
        assert!(
            state(&app).initial_save_deferred(),
            "precondition: project must be deferred (never saved)"
        );

        let sections = vec![crate::models::ImportSectionConfig {
            reverse: false,
            section_index: 0,
            name: Some("Imported".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![shot_data(5.0)],
            station_comments: Vec::new(),
            station_names: Vec::new(),
            placement: crate::models::ImportPlacement::Origin,
            azimuth_reference: None,
        }];
        let result = import::import_sections(
            state(&app),
            sections,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );

        let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o755));

        // Partial success carrying a HUMANIZED permission warning...
        assert_success(&result);
        assert!(
            result
                .warnings
                .iter()
                .any(|w| w.message.contains("permission denied (check folder access")),
            "a permission-classified save failure must be humanized: {:?}",
            result.warnings
        );
        // ...and the internal marker must never reach the user-facing message.
        assert!(
            result.warnings.iter().all(|w| !w
                .message
                .contains(crate::core::loading::PERMISSION_DENIED_CODE)),
            "the internal FS_PERMISSION_DENIED marker must be stripped, not leaked: {:?}",
            result.warnings
        );
    });
}

// #429: the scenario the test above used to run on - a DIRECTORY squatting the
// atomic-save temp path (`<stem>.stalx.tmp`), which makes `Database::open`
// report SQLite's generic "unable to open database file" while the destination
// folder exists and is perfectly writable. That is not a denial, and calling it
// one is what put Windows users in front of macOS folder-access instructions.
// The deferred-import warning must therefore carry the plain failure, with no
// marker of any kind in it.
#[test]
fn test_import_sections_deferred_save_generic_open_failure_is_not_a_denial() {
    with_config_lock(|| {
        let app = app_with_state();
        let dir = TempDir::new().unwrap();
        let project_path = dir.path().join("new-import.stalx");
        // The reserved `.stalx` path itself stays absent (so no collision
        // auto-rename); only the temp path is occupied.
        fs::create_dir_all(dir.path().join("new-import.stalx.tmp")).unwrap();

        let mut project = Project::new("New Import".to_string(), None);
        project.file_path = Some(project_path.clone());
        state(&app).inner().set_project_for_tests(project);
        state(&app)
            .inner()
            .set_initial_save_deferred_for_tests(true);

        let sections = vec![crate::models::ImportSectionConfig {
            reverse: false,
            section_index: 0,
            name: Some("Imported".to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![shot_data(5.0)],
            station_comments: Vec::new(),
            station_names: Vec::new(),
            placement: crate::models::ImportPlacement::Origin,
            azimuth_reference: None,
        }];
        let result = import::import_sections(
            state(&app),
            sections,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        );

        assert_success(&result);
        assert!(
            !result.warnings.is_empty(),
            "the deferred save failed, so the import must say it is unsaved"
        );
        assert!(
            result
                .warnings
                .iter()
                .all(|w| !w.message.contains("permission denied")),
            "a writable destination folder is denying nothing: {:?}",
            result.warnings
        );
        for marker in [
            crate::core::loading::PERMISSION_DENIED_CODE,
            crate::core::loading::PATH_NOT_FOUND_CODE,
            crate::core::loading::PATH_TOO_LONG_CODE,
        ] {
            assert!(
                result.warnings.iter().all(|w| !w.message.contains(marker)),
                "no internal marker may reach the user: {marker} in {:?}",
                result.warnings
            );
        }
    });
}

// #104 review: "deferred" is tracked by an explicit flag, not inferred from the DB
// handle or file existence (both ambiguous - dropped handles after a save, and
// reserved-path collisions). A persisted project must never read as deferred; a
// freshly-created deferred one must.
#[test]
fn test_initial_save_deferred_uses_explicit_flag() {
    let app = app_with_state();
    state(&app)
        .inner()
        .set_project_for_tests(Project::new("P".to_string(), None));

    // Persisted (flag cleared) - not deferred, even with no DB handle.
    state(&app)
        .inner()
        .set_initial_save_deferred_for_tests(false);
    assert!(!state(&app).initial_save_deferred());

    // Deferred new project (flag set) - deferred.
    state(&app)
        .inner()
        .set_initial_save_deferred_for_tests(true);
    assert!(state(&app).initial_save_deferred());
}

#[test]
fn test_list_recent_projects_includes_saved() {
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            create_project(&app);
            let dir = TempDir::new().unwrap();
            let saved_path = save_project_to_temp(&app, &dir);
            let recent = project::list_recent_projects(state(&app));
            assert!(recent.iter().any(|p| p.file_path == saved_path));
        });
    });
}

#[test]
fn test_remove_from_recent_projects_absent_is_idempotent_success() {
    // "Remove from recent" means "ensure this path is absent", so removing a path
    // that was never in the list is a NO-OP SUCCESS, not a failure. Returning a
    // failure used to strand a zombie "missing" row that the frontend (whose config
    // copy can lag the trimmed backend list) could never clear, because it only
    // refreshes the recents on success.
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            let missing = PathBuf::from("/tmp/does-not-exist.stalx");
            let result = project::remove_from_recent_projects(state(&app), missing);
            assert_success(&result);
        });
    });
}

#[test]
fn test_remove_from_recent_projects_success() {
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            create_project(&app);
            let dir = TempDir::new().unwrap();
            let saved_path = save_project_to_temp(&app, &dir);
            let remove_result =
                project::remove_from_recent_projects(state(&app), saved_path.clone());
            assert_success(&remove_result);
            let recent = project::list_recent_projects(state(&app));
            assert!(!recent.iter().any(|p| p.file_path == saved_path));
        });
    });
}

// In test builds `send_to_trash` is a deterministic `fs::remove_file` (see
// recent_projects.rs), so these assert the file is actually gone without touching
// the developer's real Trash or needing a trash dir on a headless CI runner.

#[test]
fn test_delete_project_removes_file_and_recent() {
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            create_project(&app);
            let dir = TempDir::new().unwrap();
            let saved_path = save_project_to_temp(&app, &dir);
            assert!(saved_path.exists(), "precondition: .stalx written to disk");

            let result = project::delete_project_impl(state(&app).inner(), saved_path.clone());
            assert_success(&result);
            assert!(
                !saved_path.exists(),
                "project file should be deleted from disk"
            );
            let recent = project::list_recent_projects(state(&app));
            assert!(!recent.iter().any(|p| p.file_path == saved_path));
        });
    });
}

#[test]
fn test_delete_project_tolerates_missing_file() {
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            // Nothing was ever written here: deleting a path with no file on disk
            // should still succeed (there is simply nothing to remove).
            let dir = TempDir::new().unwrap();
            let missing = dir.path().join("never-saved.stalx");
            let result = project::delete_project_impl(state(&app).inner(), missing.clone());
            assert_success(&result);
            assert!(!missing.exists());
        });
    });
}

#[test]
fn test_delete_project_leaves_a_same_stem_tml_alone() {
    // Deleting a project trashes its `.stalx` and nothing else. A same-stem
    // `.tml` beside it is an ordinary file as far as this app is concerned -
    // very likely one the user imported from or exported to - so it must
    // survive. (It used to be trashed too, when it was a collaboration
    // sibling this app owned.)
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            create_project(&app);
            let dir = TempDir::new().unwrap();
            let saved_path = save_project_to_temp(&app, &dir);
            assert!(
                project::list_recent_projects(state(&app))
                    .iter()
                    .any(|p| p.file_path == saved_path),
                "precondition: saved project is listed in recents"
            );
            let tml = saved_path.with_extension("tml");
            fs::write(&tml, b"not ours").unwrap();

            let result = project::delete_project_impl(state(&app).inner(), saved_path.clone());

            assert!(result.success, "the .stalx delete succeeded");
            assert!(!saved_path.exists(), "project .stalx should be trashed");
            assert!(
                tml.exists(),
                "an unrelated same-stem .tml must be left alone"
            );
            assert!(
                !project::list_recent_projects(state(&app))
                    .iter()
                    .any(|p| p.file_path == saved_path),
                "recents must be pruned"
            );
        });
    });
}

/// Once the `.stalx` has been trashed the delete HAS happened, so a later problem
/// must not be reported as a failure.
///
/// The frontend refreshes its recents list only on `success`, so a failure returned
/// after the file is already gone leaves the deleted project rendered as a permanent
/// "File not found" zombie row that no amount of clicking can remove. The contract
/// is therefore all three of: `success: true`, a `delete_project_persist_failed`
/// warning naming what went wrong, and the recents entry dropped anyway.
///
/// The surviving post-trash failure is the recents persist (the `.tml` sibling this
/// used to fail on belonged to the removed collab mode, and is now an unrelated file
/// the delete never touches - see the same-stem test above).
#[test]
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn test_delete_project_partial_failure_still_prunes_recents() {
    use std::os::unix::fs::PermissionsExt;

    // Skip when running as root - file permission restrictions don't apply to UID 0
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("Skipping: test requires non-root (chmod 555 has no effect as root)");
        return;
    }

    // Same technique as test_open_project_updates_recent_projects_when_config_save_fails:
    // resolve `config_path()` for real rather than into this thread's temp dir, and point
    // the platform's config root at a read-only directory so every `config.save()` fails
    // deterministically (`save_to_path` cannot create its parent there). Linux resolves
    // the root from XDG_CONFIG_HOME; the `directories` crate ignores that on macOS and
    // derives from HOME instead.
    let _config_guard = crate::test_helpers::real_config_resolution();
    let readonly_dir = TempDir::new().unwrap();
    let mut perms = fs::metadata(readonly_dir.path()).unwrap().permissions();
    perms.set_mode(0o555);
    fs::set_permissions(readonly_dir.path(), perms).unwrap();
    #[cfg(target_os = "linux")]
    let _env_guard = EnvVarGuard::set("XDG_CONFIG_HOME", readonly_dir.path().to_str().unwrap());
    #[cfg(target_os = "macos")]
    let _env_guard = EnvVarGuard::set("HOME", readonly_dir.path().to_str().unwrap());

    let app = app_with_state();
    create_project(&app);
    let project_dir = TempDir::new().unwrap();
    let saved_path = save_project_to_temp(&app, &project_dir);
    // The save could not persist the config either - that is the point - but it DID
    // update the in-memory recents, so there is a real entry for the delete to prune.
    assert!(
        config::get_config(state(&app))
            .general
            .recent_projects
            .contains(&saved_path),
        "precondition: the saved project is in recents"
    );
    assert!(
        !crate::config::Config::config_path().unwrap().exists(),
        "precondition: the config root is genuinely unwritable"
    );

    let result = project::delete_project_impl(state(&app).inner(), saved_path.clone());

    // Restore permissions BEFORE the assertions: a panicking assert would otherwise
    // unwind past this and leave TempDir unable to clean up.
    let mut perms = fs::metadata(readonly_dir.path()).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(readonly_dir.path(), perms).unwrap();

    assert!(
        result.success,
        "the .stalx was trashed, so the delete succeeded overall: {:?}",
        result.error
    );
    assert!(!saved_path.exists(), "project .stalx should be trashed");
    assert!(
        result
            .warnings
            .iter()
            .any(|w| w.code == "delete_project_persist_failed"),
        "the un-persisted recents list must be surfaced as a warning, got {:?}",
        result.warnings
    );
    assert!(
        !config::get_config(state(&app))
            .general
            .recent_projects
            .contains(&saved_path),
        "recents must be pruned even though the persist failed - otherwise the \
         deleted project stays on screen as an unremovable zombie row"
    );
}

/// Deleting the `.stalx` backing the OPEN project tears the whole SESSION down, not
/// just the project slot.
///
/// It used to only do `*self.project.lock() = None`, which left behind a live SQLite
/// handle on a file that no longer exists, plus the entire undo/redo stack - so an
/// undo could replay against a deleted project. `delete_project` now routes through
/// the same `close_project` teardown a real close uses.
///
/// Observed through the surface the frontend itself has: `get_project_state` for the
/// project slot, the raw-data reader for the database handle (the only command that
/// reports `self.database == None` distinctly - see
/// `test_raw_data_commands_fail_without_db`), and `can_undo`/`can_redo` +
/// `get_undo_stack`/`get_redo_stack` for the two stacks.
#[test]
fn test_delete_project_of_open_project_tears_down_the_session() {
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            create_project(&app);
            let dir = TempDir::new().unwrap();
            let saved_path = save_project_to_temp(&app, &dir);

            // Two undoable commands and then one undo, so BOTH stacks are genuinely
            // non-empty going in - asserting "empty afterwards" on a stack that was
            // already empty would prove nothing.
            create_section_with_shots(&app, vec![shot_data(1.0)]);
            create_section_with_shots(&app, vec![shot_data(2.0)]);
            assert_success(&history::undo(state(&app)));

            assert_eq!(
                project::get_project_state(state(&app))
                    .expect("precondition: a project is open")
                    .project
                    .file_path
                    .as_deref(),
                Some(saved_path.as_path()),
                "precondition: the path being deleted IS the open project"
            );
            assert!(
                raw_data::get_raw_table_names_impl(state(&app).inner()).is_ok(),
                "precondition: the project database handle is live"
            );
            assert!(
                history::can_undo(state(&app)),
                "precondition: the undo stack is non-empty"
            );
            assert!(
                history::can_redo(state(&app)),
                "precondition: the redo stack is non-empty"
            );

            let result = project::delete_project_impl(state(&app).inner(), saved_path.clone());

            assert_success(&result);
            assert!(!saved_path.exists(), "project .stalx should be trashed");

            assert!(
                project::get_project_state(state(&app)).is_none(),
                "the open project must be dropped when its file is deleted"
            );
            let db_err = raw_data::get_raw_table_names_impl(state(&app).inner()).expect_err(
                "the database handle LEAKED: still open on a file that was just deleted",
            );
            assert!(
                db_err.contains("No project database"),
                "expected the no-database error, got: {db_err}"
            );
            assert!(
                !history::can_undo(state(&app)),
                "the undo stack LEAKED ({} entries): an undo must never be able to \
                 replay against a deleted project",
                history::get_undo_stack(state(&app)).len()
            );
            assert!(
                history::get_undo_stack(state(&app)).is_empty(),
                "the undo stack LEAKED: {:?}",
                history::get_undo_stack(state(&app))
            );
            assert!(
                !history::can_redo(state(&app)),
                "the redo stack LEAKED ({} entries): a redo must never be able to \
                 replay against a deleted project",
                history::get_redo_stack(state(&app)).len()
            );
            assert!(
                history::get_redo_stack(state(&app)).is_empty(),
                "the redo stack LEAKED: {:?}",
                history::get_redo_stack(state(&app))
            );
        });
    });
}

/// The teardown runs BEFORE the file is handed to the OS delete, not after.
///
/// On Windows the ordering is what makes the delete possible at all (SQLite holds the
/// file without `FILE_SHARE_DELETE`, so both the Recycle-Bin move and the
/// `remove_file` fallback fail with "Access is denied" while the handle is alive), but
/// "the delete would have failed" is not observable on Unix. The other side of the
/// same coin is: force the removal to FAIL and the session must be torn down anyway -
/// which can only happen if the teardown ran FIRST, because the trash step returns
/// early on failure, so a teardown placed after it would never be reached. That is
/// exactly the trade-off `delete_project`'s own comment documents ("the project is now
/// closed and its undo history dropped even though the file survives").
///
/// Unix-only for the same reason as the partial-failure test above: making the removal
/// fail deterministically means taking write permission off the parent directory
/// (`fs::remove_file` needs it), and that is a POSIX mode bit.
#[test]
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn test_delete_project_tears_down_before_removing_the_file() {
    use std::os::unix::fs::PermissionsExt;

    // Skip when running as root - file permission restrictions don't apply to UID 0
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("Skipping: test requires non-root (chmod 555 has no effect as root)");
        return;
    }

    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            create_project(&app);
            let dir = TempDir::new().unwrap();
            let saved_path = save_project_to_temp(&app, &dir);
            create_section_with_shots(&app, vec![shot_data(1.0)]);
            assert!(
                history::can_undo(state(&app)),
                "precondition: there is real undo history to tear down"
            );

            // Drop write permission on the project's parent directory so the
            // `fs::remove_file` behind `send_to_trash` fails with EACCES.
            let mut perms = fs::metadata(dir.path()).unwrap().permissions();
            perms.set_mode(0o555);
            fs::set_permissions(dir.path(), perms).unwrap();

            let result = project::delete_project_impl(state(&app).inner(), saved_path.clone());

            // Restore permissions BEFORE the assertions: a panicking assert would
            // otherwise unwind past this and leave TempDir unable to clean up.
            let mut perms = fs::metadata(dir.path()).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(dir.path(), perms).unwrap();

            assert_failure_contains(&result, "Failed to delete");
            assert!(
                saved_path.exists(),
                "precondition: the removal really did fail, so the file survived"
            );

            // The removal never happened, yet the session is gone - so the teardown
            // must have run before it.
            assert!(
                project::get_project_state(state(&app)).is_none(),
                "the teardown must precede the file removal: the project is still open, \
                 so the close can only have been sequenced AFTER the (failed) delete"
            );
            assert!(
                raw_data::get_raw_table_names_impl(state(&app).inner()).is_err(),
                "the teardown must precede the file removal: the database handle is \
                 still live, so the close can only have been sequenced AFTER the \
                 (failed) delete"
            );
            assert!(
                !history::can_undo(state(&app)),
                "the teardown must precede the file removal: the undo stack still has \
                 {} entries",
                history::get_undo_stack(state(&app)).len()
            );

            // The documented cost of closing first: recents is left untouched on this
            // branch, so the user can reopen the project that survived.
            assert!(
                project::list_recent_projects(state(&app))
                    .iter()
                    .any(|p| p.file_path == saved_path),
                "a failed delete must leave the recents entry so the surviving project \
                 stays re-openable"
            );

            // ...and the FAILURE result still reports the teardown. This is the branch
            // the flag exists for: the frontend cannot derive it from `success` here,
            // and a frontend that tried kept rendering a project the backend had
            // already dropped.
            assert_eq!(
                result.session_closed,
                Some(true),
                "a failed delete that already tore the session down must still report it"
            );
        });
    });
}

/// The teardown is REPORTED to the caller (`CommandResult::session_closed`), and only
/// when it actually happened.
///
/// The frontend clears its own copy of the session off this bit. It cannot derive it:
/// not from `success` (see the pre-removal-teardown test above, where the teardown
/// happened and the result is a failure) and not from the path it asked to delete
/// (`same_file` canonicalizes - see the alias test below). The negative half matters
/// just as much: an absent flag is what tells the frontend the delete hit some OTHER
/// file, and acting on it anyway would throw a live project's undo history away.
#[test]
fn test_delete_project_reports_whether_it_closed_the_session() {
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            let dir = TempDir::new().unwrap();

            // A project that is NOT open: save it, then close the session.
            create_project(&app);
            let other_path = save_project_to_temp(&app, &dir);
            assert_success(&project::close_project_impl(
                state(&app).inner(),
                project::CloseLock::Release,
            ));

            // ...and the one that IS open, in its own directory so the two saves
            // don't collide on `project.stalx`.
            let open_dir = TempDir::new().unwrap();
            create_project(&app);
            let open_path = save_project_to_temp(&app, &open_dir);
            create_section_with_shots(&app, vec![shot_data(1.0)]);

            let other = project::delete_project_impl(state(&app).inner(), other_path.clone());
            assert_success(&other);
            assert!(!other_path.exists(), "the unrelated .stalx was deleted");
            assert_eq!(
                other.session_closed, None,
                "deleting an unrelated project must not report - or perform - a close"
            );
            assert!(
                project::get_project_state(state(&app)).is_some(),
                "the OPEN project must survive an unrelated delete"
            );
            assert!(
                raw_data::get_raw_table_names_impl(state(&app).inner()).is_ok(),
                "the open project's database handle must survive an unrelated delete"
            );
            assert!(
                history::can_undo(state(&app)),
                "the open project's undo stack must survive an unrelated delete"
            );

            let own = project::delete_project_impl(state(&app).inner(), open_path.clone());
            assert_success(&own);
            assert_eq!(
                own.session_closed,
                Some(true),
                "deleting the OPEN project's own .stalx must report the close"
            );
            assert!(
                project::get_project_state(state(&app)).is_none(),
                "precondition for the flag: the session really was torn down"
            );
        });
    });
}

/// A SECOND SPELLING of the open project's path still tears the session down, and
/// still reports it.
///
/// The match is `same_file`, which canonicalizes, so a `..` segment (and on Windows a
/// case or short-name variant) resolves to the same file. A plain `==` was survivable
/// while the check only kept an autosave from resurrecting the file, but the Windows
/// delete DEPENDS on it firing: SQLite holds the file without `FILE_SHARE_DELETE`, so
/// missing the close means the trash fails with "Access is denied".
///
/// The alias needs a `..` rather than a `.`: `Path`'s `PartialEq` compares
/// `components()`, which already normalizes `CurDir` away, so `<dir>/./x` and
/// `<dir>/x` are equal even to `==` and would prove nothing. `ParentDir` is NOT
/// normalized away, so only canonicalization resolves it.
#[test]
fn test_delete_project_matches_an_alias_of_the_open_path() {
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            create_project(&app);
            let dir = TempDir::new().unwrap();
            let saved_path = save_project_to_temp(&app, &dir);
            create_section_with_shots(&app, vec![shot_data(1.0)]);
            assert!(
                history::can_undo(state(&app)),
                "precondition: there is real undo history to tear down"
            );

            // Same file, different spelling: <dir>/sub/../project.stalx. The open
            // project's `file_path` is the plain <dir>/project.stalx, and `sub` has
            // to really exist for `canonicalize` to resolve the `..` through it.
            fs::create_dir(dir.path().join("sub")).unwrap();
            let alias = dir.path().join("sub").join("..").join("project.stalx");
            assert_ne!(
                alias, saved_path,
                "precondition: the alias is not literally the open project's path"
            );

            let result = project::delete_project_impl(state(&app).inner(), alias);

            assert_success(&result);
            assert_eq!(
                result.session_closed,
                Some(true),
                "an alias of the open project's path must still close the session"
            );
            assert!(!saved_path.exists(), "the aliased .stalx should be trashed");
            assert!(
                project::get_project_state(state(&app)).is_none(),
                "the open project must be dropped when an alias of its path is deleted"
            );
            assert!(
                raw_data::get_raw_table_names_impl(state(&app).inner()).is_err(),
                "the database handle LEAKED: still open on a file that was just deleted"
            );
            assert!(
                !history::can_undo(state(&app)),
                "the undo stack LEAKED ({} entries)",
                history::get_undo_stack(state(&app)).len()
            );
            // The recents prune matches on file identity too, so the entry stored
            // under the plain path goes with it - see
            // `test_delete_project_by_an_alias_prunes_the_recents_entry` for the
            // focused version of that (this test is about the live SQLite handle,
            // which is what the Windows delete depends on).
            assert!(
                !config::get_config(state(&app))
                    .general
                    .recent_projects
                    .contains(&saved_path),
                "an aliased delete must prune the recents row too"
            );
        });
    });
}

/// Deleting a project through an ALIAS of the path stored in recents still prunes the
/// recents row.
///
/// The prune used to compare raw paths with `!=`, so a `..` segment (or a symlink, or a
/// Windows case variant) trashed the `.stalx` and left the row behind - the "File not
/// found" zombie row `delete_project`'s own doc promises to avoid, and a uniquely sticky
/// one: the row's file no longer exists, so a second delete cannot clear it either.
///
/// Two things make the assertion real:
///
/// - The alias uses a `..` segment, NOT a `.`: `Path`'s `PartialEq` compares
///   `components()`, which already normalizes `CurDir` away, so `<dir>/./x` and
///   `<dir>/x` are equal even to `==` and the test would prove nothing. `ParentDir` is
///   not normalized away, so only canonicalization resolves it. (Same trap as
///   `test_delete_project_matches_an_alias_of_the_open_path`.)
/// - It is asserted against `config.general.recent_projects` rather than
///   `list_recent_projects`, which reads each entry's metadata off disk and SKIPS the
///   ones it cannot open. A deleted project's file is gone either way, so that view
///   reports it as absent whether or not the row was actually pruned.
///
/// The project is CLOSED first so the delete's own session teardown is out of the
/// picture: this is about the prune alone. A second, unrelated project is left in
/// recents (and open) to pin the other half of the contract - matching on file identity
/// must not over-prune, and an alias of one project is not a delete of another.
#[test]
fn test_delete_project_by_an_alias_prunes_the_recents_entry() {
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            // The project to delete by alias, then closed.
            create_project(&app);
            let dir = TempDir::new().unwrap();
            let saved_path = save_project_to_temp(&app, &dir);
            assert_success(&project::close_project_impl(
                state(&app).inner(),
                project::CloseLock::Release,
            ));

            // An unrelated bystander, in its own directory so the two saves don't
            // collide on `project.stalx`. Left OPEN as well.
            let other_dir = TempDir::new().unwrap();
            create_project(&app);
            let other_path = save_project_to_temp(&app, &other_dir);

            let recents_before = config::get_config(state(&app)).general.recent_projects;
            assert!(
                recents_before.contains(&saved_path) && recents_before.contains(&other_path),
                "precondition: both projects are in recents under their plain paths, \
                 got {recents_before:?}"
            );

            // Same file, different spelling: <dir>/sub/../project.stalx. `sub` has to
            // really exist for `canonicalize` to resolve the `..` through it.
            fs::create_dir(dir.path().join("sub")).unwrap();
            let alias = dir.path().join("sub").join("..").join("project.stalx");
            assert_ne!(
                alias, saved_path,
                "precondition: the alias is not literally the stored recents path"
            );

            let result = project::delete_project_impl(state(&app).inner(), alias);

            assert_success(&result);
            assert!(!saved_path.exists(), "the aliased .stalx should be trashed");
            let recents_after = config::get_config(state(&app)).general.recent_projects;
            assert!(
                !recents_after.contains(&saved_path),
                "an aliased delete must prune the recents row, or it becomes a \
                 permanent 'File not found' zombie: {recents_after:?}"
            );
            assert!(
                recents_after.contains(&other_path),
                "and must not over-prune: an unrelated project's row has to survive: \
                 {recents_after:?}"
            );
            assert_eq!(
                result.session_closed, None,
                "the deleted project was already closed and the OPEN one is a \
                 different file, so no session was torn down"
            );
            assert!(
                project::get_project_state(state(&app)).is_some(),
                "the unrelated open project must survive"
            );
            assert!(other_path.exists(), "and so must its .stalx");
        });
    });
}

/// A DANGLING symlink is still an entry on disk, so deleting it unlinks it.
///
/// The removal is guarded on "is there anything at this path", and `Path::exists`
/// answers that by FOLLOWING the link - so for a broken link it says `false`, the
/// guard skipped the removal, and the recents row was pruned while the dead link
/// stayed on disk forever. `symlink_metadata` is the question that actually matches
/// what `send_to_trash` operates on.
///
/// Unix-only for the same reason as the tests below: creating a symlink on Windows
/// needs Developer Mode or `SeCreateSymbolicLinkPrivilege`.
#[test]
#[cfg(unix)]
fn test_delete_project_unlinks_a_dangling_symlink() {
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            let dir = TempDir::new().unwrap();
            let link = dir.path().join("dangling.stalx");
            std::os::unix::fs::symlink(dir.path().join("gone.stalx"), &link).unwrap();
            assert!(
                !link.exists(),
                "precondition: `exists()` follows the link and reports false, which is \
                 exactly what made the old guard skip the removal"
            );
            assert!(
                fs::symlink_metadata(&link).is_ok(),
                "precondition: but there IS an entry at that path to unlink"
            );

            let result = project::delete_project_impl(state(&app).inner(), link.clone());

            assert_success(&result);
            assert!(
                fs::symlink_metadata(&link).is_err(),
                "the dead link the user named must be gone, not left behind under a \
                 `success: true`"
            );
        });
    });
}

/// Deleting a SYMLINK to the open project removes the link and NOTHING else: the
/// referent's `.stalx`, its session and its recents row all survive.
///
/// The removal never follows that last hop: the deterministic `cfg(test)`
/// `fs::remove_file` this test drives removes the LINK. `trash::delete` and its
/// `fs::remove_file` fallback are believed to behave the same, but no test config
/// can reach that real arm BY DESIGN (see `send_to_trash`'s doc comment in
/// `core::state::recent_projects`), so that belief is unverified from tests.
/// Applying referent identity here would report a destruction that had not happened:
/// the link vanished, and with it the user's open session and the recents row for a
/// project whose file was still sitting on disk, all under a `success: true`. Identity
/// is therefore EXACT-PATH when the final component is a symlink.
///
/// The `canonicalize` precondition is what makes this non-vacuous: it proves the link
/// really does resolve to the open project, i.e. that `same_file` WOULD have matched it
/// and torn the session down.
///
/// Unix-only because creating a symlink on Windows needs either Developer Mode or
/// `SeCreateSymbolicLinkPrivilege`; the production rule itself is platform-neutral
/// (`symlink_metadata` reports Windows symlinks the same way).
#[test]
#[cfg(unix)]
fn test_delete_project_of_a_symlink_to_the_open_project_spares_the_referent() {
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            create_project(&app);
            let dir = TempDir::new().unwrap();
            let saved_path = save_project_to_temp(&app, &dir);
            create_section_with_shots(&app, vec![shot_data(1.0)]);
            assert!(
                history::can_undo(state(&app)),
                "precondition: there is real undo history that must NOT be torn down"
            );

            let link = dir.path().join("link.stalx");
            std::os::unix::fs::symlink(&saved_path, &link).unwrap();
            assert_eq!(
                link.canonicalize().unwrap(),
                saved_path.canonicalize().unwrap(),
                "precondition: the link resolves to the open project, so referent \
                 identity WOULD have matched it - this is what stops the test being \
                 vacuous"
            );

            let result = project::delete_project_impl(state(&app).inner(), link.clone());

            assert_success(&result);
            assert!(
                fs::symlink_metadata(&link).is_err(),
                "the link the user named must be gone"
            );
            assert!(
                saved_path.exists(),
                "the referent was never named, so it must still be on disk - deleting a \
                 link must never destroy the project behind it"
            );

            // The session is still valid: its file is right there.
            assert_eq!(
                result.session_closed, None,
                "the referent's session is still backed by a real file, so nothing may \
                 be torn down - and the frontend must not be told it was"
            );
            assert_eq!(
                project::get_project_state(state(&app))
                    .expect("the open project must survive the deletion of a link to it")
                    .project
                    .file_path
                    .as_deref(),
                Some(saved_path.as_path()),
                "the open project must still be the referent"
            );
            assert!(
                raw_data::get_raw_table_names_impl(state(&app).inner()).is_ok(),
                "the database handle must survive: its file was never deleted"
            );
            assert!(
                history::can_undo(state(&app)),
                "the undo history must survive: nothing was destroyed that it could \
                 replay against"
            );
            assert!(
                config::get_config(state(&app))
                    .general
                    .recent_projects
                    .contains(&saved_path),
                "the referent's recents row must survive - pruning it would cost the \
                 user their way back to a project that is still on disk"
            );
        });
    });
}

/// ...but a symlink that IS the path the project was OPENED THROUGH still tears the
/// session down.
///
/// The other half of the exact-path rule, and the reason it is exact-path rather than
/// "a symlink never closes anything": once the link is gone, a session whose
/// `file_path` points at it is backed by nothing, so every later autosave, save and
/// undo persist would target a path that no longer exists. The teardown is what keeps
/// that from happening.
///
/// The recents assertions are also what make this test non-vacuous: under referent
/// identity the prune took the REFERENT's row too, stranding the user with no route
/// back to a project that was never deleted.
#[test]
#[cfg(unix)]
fn test_delete_project_of_the_symlink_it_was_opened_through_closes_the_session() {
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            // Save under the real name, close, then reopen THROUGH the link so the
            // session's own `file_path` is the link rather than the referent.
            create_project(&app);
            let dir = TempDir::new().unwrap();
            let real_path = save_project_to_temp(&app, &dir);
            assert_success(&project::close_project_impl(
                state(&app).inner(),
                project::CloseLock::Release,
            ));

            let link = dir.path().join("link.stalx");
            std::os::unix::fs::symlink(&real_path, &link).unwrap();
            assert_success(&open_project(state(&app), link.clone()));
            create_section_with_shots(&app, vec![shot_data(1.0)]);

            assert_eq!(
                project::get_project_state(state(&app))
                    .expect("precondition: a project is open")
                    .project
                    .file_path
                    .as_deref(),
                Some(link.as_path()),
                "precondition: the session was opened through the link, so its \
                 file_path IS the link"
            );
            let recents_before = config::get_config(state(&app)).general.recent_projects;
            assert!(
                recents_before.contains(&link) && recents_before.contains(&real_path),
                "precondition: both spellings have their own recents row (the save \
                 stored one, the open through the link the other), got \
                 {recents_before:?}"
            );

            let result = project::delete_project_impl(state(&app).inner(), link.clone());

            assert_success(&result);
            assert!(
                fs::symlink_metadata(&link).is_err(),
                "the link must be gone"
            );
            assert!(
                real_path.exists(),
                "the referent was not named, so it survives even here"
            );

            assert_eq!(
                result.session_closed,
                Some(true),
                "the session's own file_path was the link we just removed, so it MUST \
                 be torn down - leaving it open points every later save at a path that \
                 no longer exists"
            );
            assert!(
                project::get_project_state(state(&app)).is_none(),
                "precondition for the flag: the session really was torn down"
            );
            assert!(
                raw_data::get_raw_table_names_impl(state(&app).inner()).is_err(),
                "the database handle LEAKED: still open through a link that is gone"
            );
            assert!(
                !history::can_undo(state(&app)),
                "the undo stack LEAKED ({} entries)",
                history::get_undo_stack(state(&app)).len()
            );

            let recents_after = config::get_config(state(&app)).general.recent_projects;
            assert!(
                !recents_after.contains(&link),
                "the link's own row must be pruned - its file is gone, so it would be a \
                 'File not found' zombie: {recents_after:?}"
            );
            assert!(
                recents_after.contains(&real_path),
                "and the referent's row must NOT be: its .stalx is still on disk and \
                 that row is now the user's only route to it: {recents_after:?}"
            );
        });
    });
}

/// A symlink in a PARENT directory is not the same thing at all: the final component is
/// a real file, the delete really does destroy it, and referent identity still applies.
///
/// This is the regression guard for the stat that draws the line.
/// `symlink_metadata` resolves symlinks in the parent components but not the last one,
/// which is exactly the distinction the rule needs - swap it for anything that follows
/// the whole path (`metadata`) and a link to the open project is treated as the project
/// itself again; swap it for anything that follows none of it and THIS case loses the
/// teardown that the Windows delete depends on (SQLite holds the file without
/// `FILE_SHARE_DELETE`, so a missed close means "Access is denied").
#[test]
#[cfg(unix)]
fn test_delete_project_through_a_symlinked_parent_dir_matches_by_identity() {
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            create_project(&app);
            let dir = TempDir::new().unwrap();
            let real_dir = dir.path().join("realdir");
            fs::create_dir(&real_dir).unwrap();
            assert_success(&project::save_project(
                state(&app),
                Some(real_dir.join("project")),
                None,
            ));
            let saved_path = real_dir.join("project.stalx");
            assert!(
                saved_path.exists(),
                "precondition: the .stalx really is under the real directory"
            );
            create_section_with_shots(&app, vec![shot_data(1.0)]);
            assert!(
                history::can_undo(state(&app)),
                "precondition: there is real undo history to tear down"
            );

            // <dir>/linkdir -> <dir>/realdir, so <dir>/linkdir/project.stalx is a
            // second spelling of the open project's path - NOT a link to it.
            let link_dir = dir.path().join("linkdir");
            std::os::unix::fs::symlink(&real_dir, &link_dir).unwrap();
            let through_link = link_dir.join("project.stalx");
            assert!(
                !fs::symlink_metadata(&through_link)
                    .unwrap()
                    .file_type()
                    .is_symlink(),
                "precondition: only the PARENT is a link - the final component must \
                 stat as an ordinary file, or this test is exercising the other rule"
            );

            let result = project::delete_project_impl(state(&app).inner(), through_link);

            assert_success(&result);
            assert!(
                !saved_path.exists(),
                "the delete really did destroy the file under the real path, so \
                 referent identity is the correct reading here"
            );
            assert_eq!(
                result.session_closed,
                Some(true),
                "a second spelling of the open project's path must still close the \
                 session: on Windows the trash is denied outright while SQLite's handle \
                 is alive"
            );
            assert!(
                project::get_project_state(state(&app)).is_none(),
                "the open project must be dropped when its file is deleted through a \
                 symlinked parent directory"
            );
            assert!(
                raw_data::get_raw_table_names_impl(state(&app).inner()).is_err(),
                "the database handle LEAKED: still open on a file that was just deleted"
            );
            assert!(
                !history::can_undo(state(&app)),
                "the undo stack LEAKED ({} entries)",
                history::get_undo_stack(state(&app)).len()
            );
            assert!(
                !config::get_config(state(&app))
                    .general
                    .recent_projects
                    .contains(&saved_path),
                "the recents row is stored under the real path and its file is gone, so \
                 it must be pruned or it becomes a 'File not found' zombie"
            );
        });
    });
}

#[test]
fn test_save_project_fails_when_directory_missing() {
    with_config_lock(|| {
        let app = app_with_state();
        create_project(&app);
        let dir = TempDir::new().unwrap();
        let missing_dir = dir.path().join("missing");
        let path = missing_dir.join("project");

        let result = project::save_project(state(&app), Some(path), None);
        // #429: refused up front, naming the destination as the problem, rather
        // than relaying SQLite's "Failed to open database" from an attempt that
        // never had a folder to write into.
        assert_failure_contains(
            &result,
            "the folder no longer exists or the drive is not connected",
        );
        assert_failure_contains(&result, crate::core::loading::PATH_NOT_FOUND_CODE);
        assert!(!missing_dir.exists());
    });
}

#[test]
fn test_merge_projects_fails_when_source_missing() {
    let app = app_with_state();
    let project = project_with_section(vec![shot_data(1.0)]);
    let (db_a, _database_a) = TestDb::with_project(&project).unwrap();
    let missing_path = db_a.path().with_file_name("missing.stalx");
    let output_dir = TempDir::new().unwrap();
    let output_path = output_dir.path().join("merged.stalx");

    let result = project::merge_projects(
        state(&app),
        vec![db_a.path().to_path_buf(), missing_path],
        output_path,
        "Merged".to_string(),
        false,
    );

    assert_failure_contains(&result, "Source file not found");
}

// === Section commands ===

#[test]
fn test_create_section_fails_without_project() {
    let app = app_with_state();
    let result = section::create_section(
        state(&app),
        Some("Section".to_string()),
        None,
        None,
        "#4ecdc4".to_string(),
        None,
        vec![shot_data(1.0)],
    );
    assert_failure_contains(&result, "No project open");
}

#[test]
fn test_create_section_success_returns_id() {
    let app = app_with_state();
    create_project(&app);
    let section_id = create_section_with_shots(&app, vec![shot_data(1.0)]);
    assert_eq!(section_id, 0);
}

#[test]
fn test_create_section_defaults_direction() {
    let app = app_with_state();
    create_project(&app);
    let result = section::create_section(
        state(&app),
        Some("Section".to_string()),
        None,
        None,
        "#4ecdc4".to_string(),
        None,
        vec![shot_data(1.0)],
    );
    assert_success(&result);
    let section = get_section(state(&app), result.section_id.unwrap()).unwrap();
    assert_eq!(section.direction, SurveyDirection::In);
}

#[test]
fn test_get_section_returns_none_missing() {
    let app = app_with_state();
    create_project(&app);
    assert!(get_section(state(&app), 999).is_none());
}

#[test]
fn test_get_section_returns_section() {
    let app = app_with_state();
    create_project(&app);
    let id = create_section_with_shots(&app, vec![shot_data(1.0)]);
    let section = get_section(state(&app), id).unwrap();
    assert_eq!(section.id, id);
}

#[test]
fn test_update_section_fails_missing() {
    let app = app_with_state();
    create_project(&app);
    let result = section::update_section(state(&app), 99, SectionUpdates::default());
    assert_failure_contains(&result, "not found");
}

#[test]
fn test_update_section_updates_name() {
    let app = app_with_state();
    create_project(&app);
    let id = create_section_with_shots(&app, vec![shot_data(1.0)]);
    let updates = SectionUpdates {
        name: Some(Some("Updated".to_string())),
        ..Default::default()
    };
    let result = section::update_section(state(&app), id, updates);
    assert_success(&result);
    let section = get_section(state(&app), id).unwrap();
    assert_eq!(section.name, Some("Updated".to_string()));
}

#[test]
fn test_update_section_updates_color() {
    let app = app_with_state();
    create_project(&app);
    let id = create_section_with_shots(&app, vec![shot_data(1.0)]);
    let updates = SectionUpdates {
        color: Some("#123456".to_string()),
        ..Default::default()
    };
    let result = section::update_section(state(&app), id, updates);
    assert_success(&result);
    let section = get_section(state(&app), id).unwrap();
    assert_eq!(section.color, "#123456".to_string());
}

#[test]
fn test_delete_section_fails_missing() {
    let app = app_with_state();
    create_project(&app);
    let result = section::delete_section(state(&app), 22);
    assert_failure_contains(&result, "not found");
}

#[test]
fn test_delete_section_removes_section() {
    let app = app_with_state();
    create_project(&app);
    let id = create_section_with_shots(&app, vec![shot_data(1.0)]);
    let result = section::delete_section(state(&app), id);
    assert_success(&result);
    assert!(get_section(state(&app), id).is_none());
}

#[test]
fn test_move_section_fails_missing() {
    let app = app_with_state();
    create_project(&app);
    let result = section::move_section(state(&app), 22, Coordinates::new(1.0, 1.0, 0.0));
    assert_failure_contains(&result, "Section not found");
}

#[test]
fn test_move_section_success() {
    let app = app_with_state();
    create_project(&app);
    let id = create_section_with_shots(&app, vec![shot_data(1.0)]);
    let result = section::move_section(state(&app), id, Coordinates::new(2.0, -3.0, 0.0));
    assert_success(&result);
}

#[test]
fn test_move_segment_moves_connected_sections() {
    let app = app_with_state();
    create_project(&app);
    let section_a = create_section_with_shots(&app, vec![shot_data(1.0)]);
    let section_b = create_section_with_shots(&app, vec![shot_data(1.0)]);

    assert_success(&station::merge_stations(
        state(&app),
        StationRef::new(section_a, 1),
        StationRef::new(section_b, 0),
    ));

    let before = project::get_project_state(state(&app)).expect("project state");
    let offset_a_before = before
        .project
        .section_offsets
        .get(&section_a)
        .copied()
        .unwrap_or((0.0, 0.0));
    let offset_b_before = before
        .project
        .section_offsets
        .get(&section_b)
        .copied()
        .unwrap_or((0.0, 0.0));

    let delta = Coordinates::new(3.0, -2.0, 0.0);
    assert_success(&section::move_segment(state(&app), section_a, delta));

    let after = project::get_project_state(state(&app)).expect("project state");
    let offset_a_after = after
        .project
        .section_offsets
        .get(&section_a)
        .copied()
        .unwrap_or((0.0, 0.0));
    let offset_b_after = after
        .project
        .section_offsets
        .get(&section_b)
        .copied()
        .unwrap_or((0.0, 0.0));

    assert!((offset_a_after.0 - (offset_a_before.0 + delta.x)).abs() < 1e-6);
    assert!((offset_a_after.1 - (offset_a_before.1 + delta.y)).abs() < 1e-6);
    assert!((offset_b_after.0 - (offset_b_before.0 + delta.x)).abs() < 1e-6);
    assert!((offset_b_after.1 - (offset_b_before.1 + delta.y)).abs() < 1e-6);
}

#[test]
fn test_create_section_with_validation_warnings() {
    let app = app_with_state();
    create_project(&app);
    let result = section::create_section(
        state(&app),
        Some("Section".to_string()),
        None,
        None,
        "#4ecdc4".to_string(),
        None,
        vec![shot_data(0.05)],
    );
    assert!(result.success);
    assert!(!result.warnings.is_empty());
}

// === Shot commands ===

#[test]
fn test_create_shot_fails_without_project() {
    let app = app_with_state();
    let result = shot::create_shot(state(&app), 0, shot_data(1.0), None, None);
    assert_failure_contains(&result, "No project open");
}

#[test]
fn test_create_shot_fails_invalid_section() {
    let app = app_with_state();
    create_project(&app);
    let result = shot::create_shot(state(&app), 99, shot_data(1.0), None, None);
    assert_failure_contains(&result, "Section 99 not found");
}

#[test]
fn test_create_shot_success_adds_shot() {
    let app = app_with_state();
    create_project(&app);
    let id = create_section_with_shots(&app, vec![]);
    let result = shot::create_shot(state(&app), id, shot_data(1.0), None, None);
    assert_success(&result);
    let section = get_section(state(&app), id).unwrap();
    assert_eq!(section.shots.len(), 1);
}

#[test]
fn test_create_shot_after_shot_id_inserts() {
    let app = app_with_state();
    create_project(&app);
    let id = create_section_with_shots(&app, vec![shot_data(1.0), shot_data(2.0)]);
    let result = shot::create_shot(state(&app), id, shot_data(3.0), Some(0), None);
    assert_success(&result);
    let section = get_section(state(&app), id).unwrap();
    assert_eq!(section.shots.len(), 3);
    assert_eq!(section.shots[1].length, 3.0);
}

#[test]
fn test_create_shot_returns_warning_for_short() {
    let app = app_with_state();
    create_project(&app);
    let id = create_section_with_shots(&app, vec![]);
    let result = shot::create_shot(state(&app), id, shot_data(0.05), None, None);
    assert!(result.success);
    assert!(!result.warnings.is_empty());
    assert!(
        result.shot_id.is_some(),
        "shot_id must be returned even when warnings are present"
    );
}

#[test]
fn test_create_shot_rejects_both_after_and_before() {
    let app = app_with_state();
    create_project(&app);
    let id = create_section_with_shots(&app, vec![shot_data(1.0), shot_data(2.0)]);
    let result = shot::create_shot(state(&app), id, shot_data(3.0), Some(0), Some(1));
    assert_failure_contains(&result, "Cannot specify both");
}

#[test]
fn test_create_shot_before_inserts_at_position() {
    let app = app_with_state();
    create_project(&app);
    let id = create_section_with_shots(&app, vec![shot_data(1.0), shot_data(2.0)]);
    // Insert before the first shot (position 0)
    let result = shot::create_shot(state(&app), id, shot_data(5.0), None, Some(0));
    assert_success(&result);
    assert!(
        result.shot_id.is_some(),
        "shot_id must be returned on success"
    );
    let section = get_section(state(&app), id).unwrap();
    assert_eq!(section.shots.len(), 3);
    assert_eq!(
        section.shots[0].length, 5.0,
        "New shot should be at position 0 (prepended)"
    );
    assert_eq!(section.shots[1].length, 1.0);
    assert_eq!(section.shots[2].length, 2.0);
}

#[test]
fn test_create_shot_wrapper_strips_a_client_named_uuid() {
    // Identity is not the caller's to name (#423): `create_shot_impl` clears
    // `ShotData::uuid` before building the command. The executor clears its own
    // copy too, but `execute` records the command AS RECEIVED into the undo
    // stack, which every save persists - so the persisted-history assertion
    // below is the one only the wrapper's clear can hold.
    const SPOOFED: &str = "ffffffff-0000-4000-8000-000000000423";
    let app = app_with_state();
    create_project(&app);
    let id = create_section_with_shots(&app, vec![shot_data(1.0)]);
    let result = shot::create_shot_impl(
        state(&app).inner(),
        id,
        ShotData {
            uuid: Some(SPOOFED.to_string()),
            ..shot_data(2.0)
        },
        None,
        None,
    );
    assert_success(&result);
    let section = get_section(state(&app), id).unwrap();
    let created = section.shots.last().unwrap();
    assert_ne!(
        created.uuid.as_deref(),
        Some(SPOOFED),
        "the wrapper must clear a client-named identity before execution"
    );
    assert!(created.uuid.is_some(), "and the shot still gets a mint");

    let dir = TempDir::new().unwrap();
    let path = save_project_to_temp(&app, &dir);
    let rows = Database::open_existing(&path)
        .unwrap()
        .load_undo_stack()
        .unwrap();
    assert!(!rows.is_empty(), "the save must persist the undo history");
    assert!(
        rows.iter()
            .all(|(_, command_json, _)| !command_json.contains(SPOOFED)),
        "a client-named uuid must not reach the recorded (persisted) undo command"
    );
}

#[test]
fn test_update_shot_fails_missing() {
    let app = app_with_state();
    create_project(&app);
    let result = shot::update_shot(state(&app), 0, 999, ShotUpdates::default());
    assert_failure_contains(&result, "not found");
}

#[test]
fn test_update_shot_updates_length() {
    let app = app_with_state();
    create_project(&app);
    let id = create_section_with_shots(&app, vec![shot_data(1.0)]);
    let updates = ShotUpdates {
        length: Some(12.0),
        ..Default::default()
    };
    let result = shot::update_shot(state(&app), id, 0, updates);
    assert_success(&result);
    let section = get_section(state(&app), id).unwrap();
    assert_eq!(section.shots[0].length, 12.0);
}

#[test]
fn test_update_shot_updates_comment() {
    let app = app_with_state();
    create_project(&app);
    let id = create_section_with_shots(&app, vec![shot_data(1.0)]);
    let updates = ShotUpdates {
        comment: Some(Some("note".to_string())),
        ..Default::default()
    };
    let result = shot::update_shot(state(&app), id, 0, updates);
    assert_success(&result);
    let section = get_section(state(&app), id).unwrap();
    assert_eq!(section.shots[0].comment, Some("note".to_string()));
}

#[test]
fn test_delete_shot_fails_missing() {
    let app = app_with_state();
    create_project(&app);
    let result = shot::delete_shot(state(&app), 0, 1);
    assert_failure_contains(&result, "not found");
}

#[test]
fn test_delete_shot_removes_shot() {
    let app = app_with_state();
    create_project(&app);
    let id = create_section_with_shots(&app, vec![shot_data(1.0)]);
    let result = shot::delete_shot(state(&app), id, 0);
    assert_success(&result);
    let section = get_section(state(&app), id).unwrap();
    assert!(section.shots.is_empty());
}

#[test]
fn test_delete_shot_and_split_creates_new_section() {
    let app = app_with_state();
    create_project(&app);
    let id = create_section_with_shots(&app, vec![shot_data(1.0), shot_data(2.0), shot_data(3.0)]);
    let section_state = get_section(state(&app), id).unwrap();
    let shot_id = section_state.shots[1].id;

    let result = shot::delete_shot_and_split(
        state(&app),
        id,
        shot_id,
        "Section A".to_string(),
        "Section B".to_string(),
    );
    assert_success(&result);
    let new_id = result.section_id.expect("new section id");

    let section_a = get_section(state(&app), id).unwrap();
    let section_b = get_section(state(&app), new_id).unwrap();
    assert_eq!(section_a.name, Some("Section A".to_string()));
    assert_eq!(section_b.name, Some("Section B".to_string()));
    assert_eq!(section_a.shots.len(), 1);
    assert_eq!(section_b.shots.len(), 1);
}

#[test]
fn test_delete_last_shot_and_section_removes_section() {
    let app = app_with_state();
    create_project(&app);
    let id = create_section_with_shots(&app, vec![shot_data(1.0)]);
    let section_state = get_section(state(&app), id).unwrap();
    let shot_id = section_state.shots[0].id;

    let result = shot::delete_last_shot_and_section(state(&app), id, shot_id);
    assert_success(&result);
    assert!(get_section(state(&app), id).is_none());
}

#[test]
fn test_set_shot_kind_fails_missing() {
    let app = app_with_state();
    create_project(&app);
    let result = shot::set_shot_kind(state(&app), 0, 999, ShotKind::Jump);
    assert_failure_contains(&result, "not found");
}

#[test]
fn test_set_shot_kind_sets_jump() {
    let app = app_with_state();
    create_project(&app);
    let id = create_section_with_shots(&app, vec![shot_data(1.0)]);
    let result = shot::set_shot_kind(state(&app), id, 0, ShotKind::Jump);
    assert_success(&result);
    let section = get_section(state(&app), id).unwrap();
    assert_eq!(section.shots[0].kind, ShotKind::Jump);
}

#[test]
fn test_set_shot_kind_sets_reference() {
    let app = app_with_state();
    create_project(&app);
    let id = create_section_with_shots(&app, vec![shot_data(1.0)]);
    let result = shot::set_shot_kind(state(&app), id, 0, ShotKind::Reference);
    assert_success(&result);
    let section = get_section(state(&app), id).unwrap();
    assert_eq!(section.shots[0].kind, ShotKind::Reference);
}

#[test]
fn test_set_shot_kind_back_to_normal() {
    let app = app_with_state();
    create_project(&app);
    let id = create_section_with_shots(&app, vec![shot_data(1.0)]);
    let _ = shot::set_shot_kind(state(&app), id, 0, ShotKind::Jump);
    let result = shot::set_shot_kind(state(&app), id, 0, ShotKind::Normal);
    assert_success(&result);
    let section = get_section(state(&app), id).unwrap();
    assert_eq!(section.shots[0].kind, ShotKind::Normal);
}

#[test]
fn test_set_shot_kind_undo_restores_previous_kind() {
    // The old boolean toggle inverted itself; a 3-way kind must carry the
    // captured previous value (#27 q12). Jump -> Reference must undo to Jump,
    // NOT to Normal.
    let app = app_with_state();
    create_project(&app);
    let id = create_section_with_shots(&app, vec![shot_data(1.0)]);
    assert_success(&shot::set_shot_kind(state(&app), id, 0, ShotKind::Jump));
    assert_success(&shot::set_shot_kind(
        state(&app),
        id,
        0,
        ShotKind::Reference,
    ));

    assert_success(&history::undo_impl(state(&app).inner()));

    let section = get_section(state(&app), id).unwrap();
    assert_eq!(
        section.shots[0].kind,
        ShotKind::Jump,
        "undo must restore the captured previous kind, not toggle to Normal"
    );
}

// === Station commands ===

#[test]
fn test_merge_stations_fails_without_project() {
    let app = app_with_state();
    let result = station::merge_stations(state(&app), StationRef::new(0, 0), StationRef::new(1, 0));
    assert_failure_contains(&result, "No project open");
}

#[test]
fn test_merge_stations_fails_same_section() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let result = station::merge_stations(state(&app), StationRef::new(0, 0), StationRef::new(0, 1));
    assert_failure_contains(&result, "same section");
}

#[test]
fn test_merge_stations_success_between_sections() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let result = station::merge_stations(state(&app), StationRef::new(0, 1), StationRef::new(1, 0));
    assert_success(&result);
}

#[test]
fn test_unmerge_stations_noop_success() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let result = station::unmerge_stations(state(&app), StationRef::new(0, 0));
    assert_success(&result);
}

#[test]
fn test_unmerge_stations_removes_merge() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let _ = station::merge_stations(state(&app), StationRef::new(0, 1), StationRef::new(1, 0));
    let result = station::unmerge_stations(state(&app), StationRef::new(0, 1));
    assert_success(&result);
}

#[test]
fn test_set_zero_datum_success() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let gps = GpsCoordinates {
        latitude: 1.0,
        longitude: 2.0,
        depth_reference: 3.0,
    };
    let result = station::set_zero_datum(state(&app), StationRef::new(0, 0), gps);
    assert_success(&result);
}

#[test]
fn test_set_zero_datum_fails_invalid_station() {
    let app = app_with_state();
    create_project(&app);
    let gps = GpsCoordinates {
        latitude: 1.0,
        longitude: 2.0,
        depth_reference: 3.0,
    };
    let result = station::set_zero_datum(state(&app), StationRef::new(0, 99), gps);
    assert_failure_contains(&result, "does not exist");
}

#[test]
fn test_remove_zero_datum_fails_when_not_zero() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let result = station::remove_zero_datum(state(&app), StationRef::new(0, 0));
    assert_failure_contains(&result, "not a zero datum");
}

#[test]
fn test_remove_zero_datum_success() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let gps = GpsCoordinates {
        latitude: 1.0,
        longitude: 2.0,
        depth_reference: 3.0,
    };
    let _ = station::set_zero_datum(state(&app), StationRef::new(0, 0), gps);
    let result = station::remove_zero_datum(state(&app), StationRef::new(0, 0));
    assert_success(&result);
}

#[test]
fn test_move_station_fails_without_project() {
    let app = app_with_state();
    let result = station::move_station(
        state(&app),
        StationRef::new(0, 0),
        Coordinates::new(1.0, 1.0, 0.0),
    );
    assert_failure_contains(&result, "No project open");
}

#[test]
fn test_move_station_fails_invalid_section() {
    let app = app_with_state();
    create_project(&app);
    let result = station::move_station(
        state(&app),
        StationRef::new(99, 0),
        Coordinates::new(1.0, 1.0, 0.0),
    );
    assert_failure_contains(&result, "Section not found");
}

#[test]
fn test_move_station_fails_invalid_index() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let result = station::move_station(
        state(&app),
        StationRef::new(0, 5),
        Coordinates::new(1.0, 1.0, 0.0),
    );
    assert_failure_contains(&result, "Invalid station index");
}

#[test]
fn test_move_station_success() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let result = station::move_station(
        state(&app),
        StationRef::new(0, 0),
        Coordinates::new(1.0, 1.0, 0.0),
    );
    assert_success(&result);
}

// === Import commands ===

#[test]
fn test_import_dmp_missing_file_err() {
    let missing = PathBuf::from("/tmp/does-not-exist.dmp");
    let result = import::import_dmp(missing);
    assert!(result.is_err());
}

#[test]
fn test_calculate_file_hash_empty() {
    let file = NamedTempFile::new().unwrap();
    let hash = import::calculate_file_hash(file.path().to_path_buf()).unwrap();
    assert_eq!(
        hash,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn test_calculate_file_hash_known_abc() {
    let file = NamedTempFile::new().unwrap();
    fs::write(file.path(), b"abc").unwrap();
    let hash = import::calculate_file_hash(file.path().to_path_buf()).unwrap();
    assert_eq!(
        hash,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn test_calculate_file_hash_known_linebreak() {
    let file = NamedTempFile::new().unwrap();
    fs::write(file.path(), b"line1\n").unwrap();
    let hash = import::calculate_file_hash(file.path().to_path_buf()).unwrap();
    assert_eq!(
        hash,
        "cd205f1f8b8ab1bf7da554fd3460b5d377c587eb7fa4f394c3f403af3a787a1b"
    );
}

#[test]
fn test_check_duplicate_import_false_no_project() {
    let app = app_with_state();
    let result = import::check_duplicate_import(state(&app), "hash1".to_string()).unwrap();
    assert!(!result);
}

#[test]
fn test_track_imported_file_fails_no_project() {
    let app = app_with_state();
    let result = import::track_imported_file(state(&app), "hash1".to_string());
    assert_failure_contains(&result, "No project open");
}

#[test]
fn test_track_imported_file_then_check_true() {
    let app = app_with_state();
    create_project(&app);
    let _ = import::track_imported_file(state(&app), "hash1".to_string());
    let result = import::check_duplicate_import(state(&app), "hash1".to_string()).unwrap();
    assert!(result);
}

#[test]
fn test_track_imported_file_idempotent() {
    let app = app_with_state();
    create_project(&app);
    let _ = import::track_imported_file(state(&app), "hash1".to_string());
    let _ = import::track_imported_file(state(&app), "hash1".to_string());
    let project_state = project::get_project_state(state(&app)).unwrap();
    assert_eq!(project_state.project.imported_file_hashes.len(), 1);
}

#[test]
fn test_check_duplicate_import_false_before_track() {
    let app = app_with_state();
    create_project(&app);
    let result = import::check_duplicate_import(state(&app), "hash2".to_string()).unwrap();
    assert!(!result);
}

// === Export commands ===

#[test]
fn test_export_csv_fails_no_project() {
    let app = app_with_state();
    let result = export::export_csv(state(&app), PathBuf::from("/tmp/out.csv"));
    assert!(result.is_err());
}

#[test]
fn test_export_json_fails_no_project() {
    let app = app_with_state();
    let result = export::export_json(state(&app), PathBuf::from("/tmp/out.json"));
    assert!(result.is_err());
}

#[test]
fn test_export_image_fails_no_project() {
    let app = app_with_state();
    let options = crate::export::ExportImageOptions::default();
    let result =
        export::export_image_impl(state(&app).inner(), PathBuf::from("/tmp/out.png"), options);
    assert!(result.is_err());
}

#[test]
fn test_export_csv_fails_when_directory_missing() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let dir = TempDir::new().unwrap();
    let missing_dir = dir.path().join("missing");
    let path = missing_dir.join("out.csv");
    let result = export::export_csv(state(&app), path.clone());
    assert!(result.is_err());
    assert!(!path.exists());
}

#[test]
fn test_export_json_fails_when_directory_missing() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let dir = TempDir::new().unwrap();
    let missing_dir = dir.path().join("missing");
    let path = missing_dir.join("out.json");
    let result = export::export_json(state(&app), path.clone());
    assert!(result.is_err());
    assert!(!path.exists());
}

#[test]
fn test_export_image_fails_when_directory_missing() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let dir = TempDir::new().unwrap();
    let missing_dir = dir.path().join("missing");
    let path = missing_dir.join("out.svg");
    let mut options = crate::export::ExportImageOptions::default();
    options.format = crate::export::ImageFormat::Svg;
    let result = export::export_image_impl(state(&app).inner(), path.clone(), options);
    assert!(result.is_err());
    assert!(!path.exists());
}

#[test]
fn test_export_image_refuses_to_overwrite_open_project_file() {
    // Exporting an image over the open project's own `.stalx` would corrupt the
    // live database (image export does a raw file write). The guard must reject
    // that path before any write, leaving the project file intact.
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let dir = TempDir::new().unwrap();
    let saved_path = save_project_to_temp(&app, &dir);

    let mut options = crate::export::ExportImageOptions::default();
    options.format = crate::export::ImageFormat::Svg;
    let result = export::export_image_impl(state(&app).inner(), saved_path.clone(), options);
    assert!(
        result.is_err(),
        "must refuse to export an image over the open project file"
    );
    assert!(
        result.unwrap_err().contains("open project"),
        "error must explain the clobber guard"
    );
    // The project database on disk is untouched (still present, not an SVG).
    assert!(saved_path.exists());
}

#[test]
fn test_export_image_refuses_to_overwrite_the_plnx_sibling() {
    // `<project>.plnx` holds EVERY saved dive plan for the open project. The
    // text exports and the dive-profile export have always refused it via
    // `is_project_owned_file`; the image export used the narrower
    // `is_open_project_file`, so a PNG/SVG/PDF aimed at that path was written
    // and took the whole plan store with it. The path is not reachable through
    // the save dialog (the frontend forces the extension), but the IPC boundary
    // takes an arbitrary path from the webview - which is the boundary the
    // guard exists for, and which the export fuzzer drives.
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let dir = TempDir::new().unwrap();
    let saved_path = save_project_to_temp(&app, &dir);
    let plnx = saved_path.with_extension("plnx");
    // A project with no saved plan has no sibling to protect, so the guard has
    // nothing to canonicalize - the refusal only applies to a `.plnx` that is
    // really there, exactly as for the `.stalx`.
    fs::write(&plnx, b"dive plans").unwrap();

    let mut options = crate::export::ExportImageOptions::default();
    options.format = crate::export::ImageFormat::Svg;
    let result = export::export_image_impl(state(&app).inner(), plnx.clone(), options);
    assert!(
        result.is_err(),
        "an image export must refuse the open project's .plnx sibling"
    );
    assert!(
        result.unwrap_err().contains("open project"),
        "error must explain the clobber guard"
    );
    assert_eq!(
        fs::read(&plnx).unwrap(),
        b"dive plans",
        "the plan store must be byte-for-byte untouched"
    );
}

#[test]
fn test_text_exports_refuse_to_overwrite_the_plnx_sibling() {
    // The control for the test above: the text exports were already correct, so
    // a regression that narrowed THEIR guard would otherwise go unnoticed.
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let dir = TempDir::new().unwrap();
    let saved_path = save_project_to_temp(&app, &dir);
    let plnx = saved_path.with_extension("plnx");
    fs::write(&plnx, b"dive plans").unwrap();

    for res in [
        export::export_csv(state(&app), plnx.clone()),
        export::export_json(state(&app), plnx.clone()),
        export::export_tml(state(&app), plnx.clone()).map(|_| ()),
    ] {
        assert!(res.is_err(), "text export must refuse the .plnx sibling");
        assert!(
            res.unwrap_err().contains("open project"),
            "error must explain the clobber guard"
        );
    }
    assert_eq!(fs::read(&plnx).unwrap(), b"dive plans");
}

#[test]
fn test_text_exports_refuse_to_overwrite_open_project_file() {
    // CSV/JSON/TML exports do raw file writes too; exporting over the open
    // project's own `.stalx` would corrupt the live database. The guard must
    // reject before any write, leaving the DB bytes byte-for-byte intact.
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let dir = TempDir::new().unwrap();
    let saved_path = save_project_to_temp(&app, &dir);
    let before = fs::read(&saved_path).unwrap();

    for res in [
        export::export_csv(state(&app), saved_path.clone()),
        export::export_json(state(&app), saved_path.clone()),
        export::export_tml(state(&app), saved_path.clone()).map(|_| ()),
    ] {
        assert!(
            res.is_err(),
            "text export must refuse the open project path"
        );
        assert!(
            res.unwrap_err().contains("open project"),
            "error must explain the clobber guard"
        );
    }
    // Byte-for-byte identical: no export path touched the DB.
    assert_eq!(fs::read(&saved_path).unwrap(), before);
}

#[test]
fn test_export_image_rejects_zero_dpi() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("out.png");
    let mut options = crate::export::ExportImageOptions::default();
    options.format = crate::export::ImageFormat::Png;
    options.dpi = 0;
    let result = export::export_image_impl(state(&app).inner(), path.clone(), options);
    assert!(result.is_err());
    assert!(!path.exists());
}

#[test]
fn test_export_json_overwrites_existing_file() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("out.json");
    fs::write(&path, "sentinel").unwrap();
    export::export_json(state(&app), path.clone()).unwrap();
    let contents = fs::read_to_string(&path).unwrap();
    assert!(contents.contains("\"project\""));
    assert!(!contents.contains("sentinel"));
}

#[test]
fn test_export_csv_success_creates_file() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("out.csv");
    export::export_csv(state(&app), path.clone()).unwrap();
    assert!(path.exists());
}

#[test]
fn test_export_json_success_creates_file() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("out.json");
    export::export_json(state(&app), path.clone()).unwrap();
    assert!(path.exists());
}

#[test]
fn test_export_image_success_creates_file() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(1.0)]);
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("out.svg");
    let mut options = crate::export::ExportImageOptions::default();
    options.format = crate::export::ImageFormat::Svg;
    export::export_image_impl(state(&app).inner(), path.clone(), options).unwrap();
    assert!(path.exists());
}

#[test]
fn test_export_roundtrip_includes_reference_and_comment() {
    let app = app_with_state();
    create_project(&app);
    let shot = ShotData {
        azimuth_in: 12.0,
        azimuth_out: 15.0,
        length: 8.25,
        depth_in: 1.0,
        depth_out: 2.0,
        left: Some(0.3),
        right: Some(0.4),
        up: Some(0.5),
        down: Some(0.6),
        comment: Some("note,with comma".to_string()),
        ..Default::default()
    };
    let section_id = create_section_with_shots(&app, vec![shot]);
    assert_success(&shot::set_shot_kind(
        state(&app),
        section_id,
        0,
        ShotKind::Jump,
    ));

    let dir = TempDir::new().unwrap();
    let csv_path = dir.path().join("export.csv");
    let json_path = dir.path().join("export.json");

    export::export_csv(state(&app), csv_path.clone()).unwrap();
    export::export_json(state(&app), json_path.clone()).unwrap();

    let csv_contents = fs::read_to_string(&csv_path).unwrap();
    assert!(csv_contents.contains("section_id,section_name"));
    assert!(csv_contents.contains("\"note,with comma\""));
    // #27 replaced the boolean `is_reference` column with the 3-way `kind`,
    // so the export now names the kind instead of emitting "true".
    assert!(csv_contents.contains(",kind,comment"));
    assert!(csv_contents.contains("Jump"));

    let json_contents = fs::read_to_string(&json_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_contents).unwrap();
    let shot_json = &parsed["sections"][0]["shots"][0];
    assert_eq!(shot_json["kind"], "Jump");
    assert_eq!(shot_json["comment"], "note,with comma");
    assert!(shot_json["lrud"].is_array());
}

// === Cache commands ===

#[test]
fn test_get_cache_stats_empty_when_disabled() {
    let app = app_with_state();
    let cache = state(&app).tile_cache();
    let mut guard = cache.lock().unwrap();
    *guard = None;
    drop(guard);
    let stats = cache::get_cache_stats(state(&app)).unwrap();
    assert_eq!(stats.total_tiles, 0);
    assert_eq!(stats.total_bytes, 0);
    assert!(stats.providers.is_empty());
}

#[test]
fn test_get_cache_stats_counts_provider() {
    let app = app_with_state();
    let cache = state(&app).tile_cache();
    {
        let mut guard = cache.lock().unwrap();
        *guard = Some(crate::cache::TileCache::in_memory(1, 30).unwrap());
        let c = guard.as_mut().unwrap();
        c.put_granted("esri", 1, 2, 3, b"data", "image/png")
            .unwrap();
    }
    let stats = cache::get_cache_stats(state(&app)).unwrap();
    assert_eq!(stats.total_tiles, 1);
    assert_eq!(stats.providers.get("esri").unwrap().tiles, 1);
}

#[test]
fn test_clear_tile_cache_noop_when_disabled() {
    let app = app_with_state();
    let cache = state(&app).tile_cache();
    let mut guard = cache.lock().unwrap();
    *guard = None;
    drop(guard);
    let result = cache::clear_tile_cache(state(&app), None);
    assert!(result.is_ok());
}

#[test]
fn test_clear_tile_cache_all_clears() {
    let app = app_with_state();
    let cache = state(&app).tile_cache();
    {
        let mut guard = cache.lock().unwrap();
        *guard = Some(crate::cache::TileCache::in_memory(1, 30).unwrap());
        let c = guard.as_mut().unwrap();
        c.put_granted("esri", 1, 2, 3, b"data", "image/png")
            .unwrap();
    }
    cache::clear_tile_cache(state(&app), None).unwrap();
    let stats = cache::get_cache_stats(state(&app)).unwrap();
    assert_eq!(stats.total_tiles, 0);
}

#[test]
fn test_clear_tile_cache_provider_only_clears() {
    let app = app_with_state();
    let cache = state(&app).tile_cache();
    {
        let mut guard = cache.lock().unwrap();
        *guard = Some(crate::cache::TileCache::in_memory(1, 30).unwrap());
        let c = guard.as_mut().unwrap();
        c.put_granted("esri", 1, 2, 3, b"data", "image/png")
            .unwrap();
        c.put_granted("other", 1, 2, 3, b"data", "image/png")
            .unwrap();
    }
    cache::clear_tile_cache(state(&app), Some("esri".to_string())).unwrap();
    let stats = cache::get_cache_stats(state(&app)).unwrap();
    assert_eq!(stats.total_tiles, 1);
    assert!(stats.providers.contains_key("other"));
}

// A fresh tile with a server cache grant is returned before any tilemap or
// imagery request. This also exercises the command's default cache config.
#[test]
fn test_fetch_tile_cache_hit_returns_data() {
    let app = app_with_state();
    let cache = state(&app).tile_cache();
    {
        let mut guard = cache.lock().unwrap();
        *guard = Some(crate::cache::TileCache::in_memory(1, 30).unwrap());
        let c = guard.as_mut().unwrap();
        c.put_granted("esri", 1, 2, 3, b"data", "image/png")
            .unwrap();
    }
    // Through the impl, not the command: the command takes a concrete
    // `AppHandle` for the telemetry collector, and these run on `MockRuntime`.
    // `None` skips only the tracking call.
    let bytes = tauri::async_runtime::block_on(cache::fetch_tile_impl(
        None,
        state(&app).inner(),
        "esri".to_string(),
        1,
        2,
        3,
    ))
    .unwrap();
    assert_eq!(bytes, b"data");
}

#[test]
fn test_fetch_tile_unknown_provider_errors() {
    let app = app_with_state();
    let result = tauri::async_runtime::block_on(cache::fetch_tile_impl(
        None,
        state(&app).inner(),
        "unknown".to_string(),
        1,
        2,
        3,
    ));
    assert!(result.is_err());
}

// === Validation commands ===

#[test]
fn test_get_project_warnings_empty_when_no_project() {
    let app = app_with_state();
    let warnings = validation::get_project_warnings(state(&app));
    assert!(warnings.is_empty());
}

#[test]
fn test_get_project_warnings_detects_zero_length() {
    let app = app_with_state();
    let project = project_with_section(vec![shot_data(0.0)]);
    set_project_for_tests(&app, project);
    let warnings = validation::get_project_warnings(state(&app));
    assert!(warnings.iter().any(|w| w.code == "ZERO_LENGTH"));
}

#[test]
fn test_get_project_warnings_detects_negative_length() {
    let app = app_with_state();
    let project = project_with_section(vec![shot_data(-1.0)]);
    set_project_for_tests(&app, project);
    let warnings = validation::get_project_warnings(state(&app));
    assert!(warnings.iter().any(|w| w.code == "NEGATIVE_LENGTH"));
}

#[test]
fn test_get_project_warnings_detects_short_shot() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(0.05)]);
    let warnings = validation::get_project_warnings(state(&app));
    assert!(warnings.iter().any(|w| w.code == "SHORT_SHOT"));
}

#[test]
fn test_get_project_warnings_detects_long_shot() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data(100.0)]);
    let warnings = validation::get_project_warnings(state(&app));
    assert!(warnings.iter().any(|w| w.code == "LONG_SHOT"));
}

#[test]
fn test_get_project_warnings_orders_error_before_warning() {
    let app = app_with_state();
    let project = project_with_section(vec![shot_data(0.0), shot_data(0.05)]);
    set_project_for_tests(&app, project);
    let warnings = validation::get_project_warnings(state(&app));
    assert!(!warnings.is_empty());
    assert_eq!(warnings[0].severity, crate::core::WarningSeverity::Error);
}

#[test]
fn test_get_project_warnings_includes_shot_index() {
    let app = app_with_state();
    let project = project_with_section(vec![shot_data(0.0), shot_data(0.0)]);
    set_project_for_tests(&app, project);
    let warnings = validation::get_project_warnings(state(&app));
    assert!(warnings.iter().any(|w| w.shot_index == 0));
    assert!(warnings.iter().any(|w| w.shot_index == 1));
}

#[test]
fn test_get_project_warnings_multiple_sections_sorted_by_section_id() {
    let app = app_with_state();
    let mut project = Project::new("Test Cave".to_string(), None);
    project.add_section(SectionData {
        shots: vec![shot_data(0.0)],
        ..Default::default()
    });
    project.add_section(SectionData {
        shots: vec![shot_data(0.0)],
        ..Default::default()
    });
    set_project_for_tests(&app, project);
    let warnings = validation::get_project_warnings(state(&app));
    assert!(warnings.len() >= 2);
    assert!(warnings[0].section_id <= warnings[1].section_id);
}

#[test]
fn test_get_project_warnings_detects_large_depth_change() {
    let app = app_with_state();
    create_project(&app);
    create_section_with_shots(&app, vec![shot_data_with_depth(5.0, 0.0, 20.0)]);
    let warnings = validation::get_project_warnings(state(&app));
    assert!(warnings.iter().any(|w| w.code == "LARGE_DEPTH_CHANGE"));
}

#[test]
fn test_resolve_warning_fails_without_db() {
    let app = app_with_state();
    let result = validation::resolve_warning(state(&app), 0, 0, "SHORT_SHOT".to_string(), None);
    assert!(result.is_err());
}

#[test]
fn test_unresolve_warning_fails_without_db() {
    let app = app_with_state();
    let result = validation::unresolve_warning(state(&app), 0, 0, "SHORT_SHOT".to_string());
    assert!(result.is_err());
}

#[test]
fn test_resolve_warning_marks_resolved_in_list() {
    let project = project_with_section(vec![shot_data(0.1)]);
    let (app_state, _db) = crate::test_helpers::app_state_with_project_db(project).unwrap();
    let app = app_with_custom_state(app_state);

    let warnings = validation::get_project_warnings(state(&app));
    let warning = warnings
        .iter()
        .find(|w| w.code == "SHORT_SHOT")
        .expect("short shot warning");
    assert!(!warning.is_resolved);

    validation::resolve_warning(
        state(&app),
        warning.section_id,
        warning.shot_id,
        warning.code.clone(),
        Some("ack".to_string()),
    )
    .unwrap();

    let warnings_after = validation::get_project_warnings(state(&app));
    let warning_after = warnings_after
        .iter()
        .find(|w| w.code == "SHORT_SHOT")
        .expect("short shot warning");
    assert!(warning_after.is_resolved);

    // Issue #57 (Item 8): a fresh ignore is always UUID-keyed (never a legacy
    // position-only row). The persisted row must carry the shot's UUID.
    let persisted = state(&app).inner().load_resolved_warnings().unwrap();
    let row = persisted
        .iter()
        .find(|r| r.warning_code == "SHORT_SHOT")
        .expect("persisted resolved row");
    assert!(
        row.shot_uuid.is_some(),
        "a fresh ignore must store a shot UUID"
    );
}

#[test]
fn test_resolve_warning_refuses_when_shot_missing() {
    // Issue #57 (Item 8): with a project open, resolving a warning for a shot
    // that does not exist must be refused (no stable identity to bind), and no
    // position-keyed row may be written.
    let project = project_with_section(vec![shot_data(0.1)]);
    let (app_state, _db) = crate::test_helpers::app_state_with_project_db(project).unwrap();
    let app = app_with_custom_state(app_state);

    let warning = validation::get_project_warnings(state(&app))
        .into_iter()
        .find(|w| w.code == "SHORT_SHOT")
        .expect("short shot warning");

    // A shot_id that does not exist in the section.
    let result = validation::resolve_warning(
        state(&app),
        warning.section_id,
        warning.shot_id + 9999,
        "SHORT_SHOT".to_string(),
        None,
    );
    assert!(result.is_err(), "resolving a vanished shot must be refused");
    assert!(
        state(&app)
            .inner()
            .load_resolved_warnings()
            .unwrap()
            .is_empty(),
        "no position-keyed row may be persisted for a missing shot"
    );
}

#[test]
fn test_unresolve_warning_clears_resolved_flag() {
    let project = project_with_section(vec![shot_data(0.1)]);
    let (app_state, _db) = crate::test_helpers::app_state_with_project_db(project).unwrap();
    let app = app_with_custom_state(app_state);

    let warning = validation::get_project_warnings(state(&app))
        .into_iter()
        .find(|w| w.code == "SHORT_SHOT")
        .expect("short shot warning");

    validation::resolve_warning(
        state(&app),
        warning.section_id,
        warning.shot_id,
        warning.code.clone(),
        None,
    )
    .unwrap();

    validation::unresolve_warning(
        state(&app),
        warning.section_id,
        warning.shot_id,
        warning.code,
    )
    .unwrap();

    let warnings_after = validation::get_project_warnings(state(&app));
    let warning_after = warnings_after
        .iter()
        .find(|w| w.code == "SHORT_SHOT")
        .expect("short shot warning");
    assert!(!warning_after.is_resolved);
}

#[test]
fn test_resolved_warning_follows_shot_uuid_after_id_change() {
    // Issue #57: an ignore is keyed by the shot's stable UUID, so it stays
    // bound to the same shot even after a structural edit renumbers shot_id
    // (e.g. inserting a shot earlier in the section).
    let mut project = project_with_section(vec![shot_data(0.1)]);
    project.ensure_shot_uuids();
    let (app_state, _db) = crate::test_helpers::app_state_with_project_db(project).unwrap();
    let app = app_with_custom_state(app_state);

    let warning = validation::get_project_warnings(state(&app))
        .into_iter()
        .find(|w| w.code == "SHORT_SHOT")
        .expect("short shot warning");
    let original_shot_id = warning.shot_id;
    let uuid = warning.shot_uuid.clone().expect("loaded shot has a uuid");

    validation::resolve_warning(
        state(&app),
        warning.section_id,
        warning.shot_id,
        warning.code.clone(),
        None,
    )
    .unwrap();
    assert!(validation::get_project_warnings(state(&app))
        .iter()
        .any(|w| w.code == "SHORT_SHOT" && w.is_resolved));

    // Drift the unstable shot_id (mimicking an insert/reload) but keep the UUID.
    let mut renumbered = state(&app).inner().get_project_state().unwrap();
    renumbered.sections[0].shots[0].id = original_shot_id + 1000;
    state(&app).inner().set_project_for_tests(renumbered);

    let after = validation::get_project_warnings(state(&app));
    let w = after
        .iter()
        .find(|w| w.code == "SHORT_SHOT")
        .expect("short shot warning");
    assert_eq!(w.shot_uuid.as_deref(), Some(uuid.as_str()));
    assert_ne!(w.shot_id, original_shot_id, "precondition: shot_id changed");
    assert!(
        w.is_resolved,
        "ignore must follow the shot UUID across an id change"
    );
}

#[test]
fn test_resolved_warning_legacy_position_fallback_matches_uuid_shot() {
    // Issue #57: a pre-v8 (or pre-v6) suppression row carries no shot_uuid and is
    // keyed by (section_id, shot_id) position. After load the shot has a freshly
    // minted in-memory UUID, but the legacy row must still suppress it - without
    // the position fallback a pre-v6 file's ignores would be silently lost.
    let mut project = project_with_section(vec![shot_data(0.1)]);
    project.ensure_shot_uuids();
    let (app_state, _db) = crate::test_helpers::app_state_with_project_db(project).unwrap();
    let app = app_with_custom_state(app_state);

    let warning = validation::get_project_warnings(state(&app))
        .into_iter()
        .find(|w| w.code == "SHORT_SHOT")
        .expect("short shot warning");
    assert!(
        warning.shot_uuid.is_some(),
        "loaded shot already carries a uuid"
    );
    assert!(!warning.is_resolved);

    // Insert a LEGACY suppression row with NO shot_uuid, keyed by position only.
    state(&app)
        .inner()
        .save_resolved_warning(&crate::persistence::ResolvedWarning {
            section_id: warning.section_id,
            shot_id: warning.shot_id,
            warning_code: warning.code.clone(),
            resolved_at: chrono::Utc::now(),
            resolved_comment: None,
            shot_uuid: None,
        })
        .unwrap();

    let after = validation::get_project_warnings(state(&app));
    let w = after
        .iter()
        .find(|w| w.code == "SHORT_SHOT")
        .expect("short shot warning");
    assert!(
        w.is_resolved,
        "a legacy position-keyed row must still suppress the now-UUID-bearing shot"
    );
}

#[test]
fn test_is_resolved_for_matches_uuid_and_legacy_position_branches() {
    // Issue #57 (Item 12) equivalence guard for the borrow-keyed is_resolved_for
    // refactor in compute_project_warnings: a UUID-keyed row suppresses via the
    // UUID branch, and a legacy NULL-uuid row suppresses via the position-fallback
    // branch. Both branches must light up `is_resolved` and only for their shot.
    let project = project_with_section(vec![shot_data(0.1), shot_data(0.1)]);
    let (app_state, _db) = crate::test_helpers::app_state_with_project_db(project).unwrap();
    let app = app_with_custom_state(app_state);

    let warnings = validation::get_project_warnings(state(&app));
    let mut zero: Vec<_> = warnings
        .into_iter()
        .filter(|w| w.code == "SHORT_SHOT")
        .collect();
    assert_eq!(zero.len(), 2, "expected a short-shot warning per shot");
    zero.sort_by_key(|w| w.shot_id);
    let shot_uuid_hit = zero[0].clone();
    let shot_pos_hit = zero[1].clone();
    let uuid = shot_uuid_hit
        .shot_uuid
        .clone()
        .expect("loaded shot has a uuid");

    // UUID branch: a row carrying the shot's UUID.
    state(&app)
        .inner()
        .save_resolved_warning(&crate::persistence::ResolvedWarning {
            section_id: shot_uuid_hit.section_id,
            shot_id: shot_uuid_hit.shot_id,
            warning_code: "SHORT_SHOT".to_string(),
            resolved_at: chrono::Utc::now(),
            resolved_comment: None,
            shot_uuid: Some(uuid),
        })
        .unwrap();
    // Position-fallback branch: a legacy NULL-uuid row keyed by (section, shot).
    state(&app)
        .inner()
        .save_resolved_warning(&crate::persistence::ResolvedWarning {
            section_id: shot_pos_hit.section_id,
            shot_id: shot_pos_hit.shot_id,
            warning_code: "SHORT_SHOT".to_string(),
            resolved_at: chrono::Utc::now(),
            resolved_comment: None,
            shot_uuid: None,
        })
        .unwrap();

    let after = validation::get_project_warnings(state(&app));
    let a = after
        .iter()
        .find(|w| w.code == "SHORT_SHOT" && w.shot_id == shot_uuid_hit.shot_id)
        .expect("uuid-keyed warning");
    let b = after
        .iter()
        .find(|w| w.code == "SHORT_SHOT" && w.shot_id == shot_pos_hit.shot_id)
        .expect("position-keyed warning");
    assert!(
        a.is_resolved,
        "UUID-keyed row must suppress via the UUID branch"
    );
    assert!(
        b.is_resolved,
        "NULL-uuid row must suppress via the position-fallback branch"
    );
}

#[test]
fn test_resolved_warning_survives_save() {
    // Issue #57 regression: resolved (ignored) warnings live in their own table,
    // not the Project model, so the save pipeline (temp file + atomic rename)
    // used to drop them. A save must carry them over so an ignore persists.
    let mut project = project_with_section(vec![shot_data(0.1)]);
    project.ensure_shot_uuids();
    let (app_state, _db) = crate::test_helpers::app_state_with_project_db(project).unwrap();
    let app = app_with_custom_state(app_state);

    let warning = validation::get_project_warnings(state(&app))
        .into_iter()
        .find(|w| w.code == "SHORT_SHOT")
        .expect("short shot warning");
    validation::resolve_warning(
        state(&app),
        warning.section_id,
        warning.shot_id,
        warning.code.clone(),
        None,
    )
    .unwrap();

    // Save rebuilds the .stalx and reopens the DB at the new file.
    let dir = tempfile::tempdir().unwrap();
    let _saved = save_project_to_temp(&app, &dir);

    let after = validation::get_project_warnings(state(&app));
    let w = after
        .iter()
        .find(|w| w.code == "SHORT_SHOT")
        .expect("short shot warning");
    assert!(
        w.is_resolved,
        "resolved/ignored warning must survive a save"
    );
}

#[test]
fn test_save_keeps_other_ignores_when_a_section_was_deleted() {
    // Regression (#57 review): a resolved_warnings row for a deleted section is
    // orphaned in the DB (delete_section is in-memory and never prunes the
    // table). The save carry-over must skip it instead of letting one FK-violating
    // row abort the whole transaction and silently drop every other ignore.
    let mut project = Project::new("Test Cave".to_string(), None);
    for (name, color) in [("A", "#4ecdc4"), ("B", "#ff6b6b")] {
        project.add_section(SectionData {
            name: Some(name.to_string()),
            surveyor: None,
            date: None,
            color: color.to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![shot_data(0.1)], // short-shot → SHORT_SHOT warning
        });
    }
    project.ensure_shot_uuids();
    let (app_state, _db) = crate::test_helpers::app_state_with_project_db(project).unwrap();
    let app = app_with_custom_state(app_state);

    // Ignore the SHORT_SHOT warning in each section.
    let zero: Vec<_> = validation::get_project_warnings(state(&app))
        .into_iter()
        .filter(|w| w.code == "SHORT_SHOT")
        .collect();
    assert_eq!(zero.len(), 2, "expected a short-shot warning per section");
    for w in &zero {
        validation::resolve_warning(state(&app), w.section_id, w.shot_id, w.code.clone(), None)
            .unwrap();
    }
    let surviving_section = zero[1].section_id;

    // Delete the first section - its resolved_warnings row is now orphaned.
    assert_success(&section::delete_section(state(&app), zero[0].section_id));

    // Save, then confirm the surviving section's ignore is still resolved.
    let dir = tempfile::tempdir().unwrap();
    let _ = save_project_to_temp(&app, &dir);

    let after = validation::get_project_warnings(state(&app));
    let surviving = after
        .iter()
        .find(|w| w.section_id == surviving_section && w.code == "SHORT_SHOT")
        .expect("surviving section's short-shot warning");
    assert!(
        surviving.is_resolved,
        "an ignore in a surviving section must not be lost because another section was deleted"
    );
}

#[test]
fn test_save_preserves_by_uuid_ignore_when_a_renumber_lands_another_shot_on_its_position() {
    // Issue #57 (Item 2, same-position collision): shot A's by-UUID ignore is
    // stored at A's old position. A renumber later moves A to a new position and
    // lands a DIFFERENT shot B on A's old position; B then gets ignored at that
    // stale position+code. On save, the carry-over re-derives each by-UUID row's
    // (section_id, shot_id) from the live shot's CURRENT position, so A's ignore
    // and B's ignore never collide on UNIQUE(section_id, shot_id, warning_code)
    // and both survive. Without the remap, INSERT OR REPLACE would overwrite A.
    let project = project_with_section(vec![shot_data(0.1), shot_data(0.1)]);
    let (app_state, _db) = crate::test_helpers::app_state_with_project_db(project).unwrap();
    let app = app_with_custom_state(app_state);

    // The two loaded shots, each with a stable UUID.
    let loaded = state(&app).inner().get_project_state().unwrap();
    let shot_a = loaded.sections[0].shots[0].clone();
    let shot_b = loaded.sections[0].shots[1].clone();
    let section_id = loaded.sections[0].id;
    let uuid_a = shot_a.uuid.clone().expect("shot A uuid");
    let uuid_b = shot_b.uuid.clone().expect("shot B uuid");
    assert_ne!(shot_a.id, shot_b.id);

    // Seed the stale state directly: A's by-UUID ignore lives at A's ORIGINAL
    // position (shot_a.id), and B's by-UUID ignore lives at B's position.
    for (uuid, shot_id) in [(&uuid_a, shot_a.id), (&uuid_b, shot_b.id)] {
        state(&app)
            .inner()
            .save_resolved_warning(&crate::persistence::ResolvedWarning {
                section_id,
                shot_id,
                warning_code: "SHORT_SHOT".to_string(),
                resolved_at: chrono::Utc::now(),
                resolved_comment: None,
                shot_uuid: Some(uuid.clone()),
            })
            .unwrap();
    }

    // Renumber: swap the shot order so A now sits at B's old position and B at
    // A's old position. The carry-over derives live positions from sequence
    // order, so A's stored shot_id (== shot_a.id, now occupied by B) is stale.
    let mut renumbered = loaded.clone();
    renumbered.sections[0].shots.swap(0, 1);
    state(&app).inner().set_project_for_tests(renumbered);

    // Save: rebuilds the .stalx and re-applies the carried-over ignores.
    let dir = tempfile::tempdir().unwrap();
    let _ = save_project_to_temp(&app, &dir);

    // Both by-UUID ignores must survive - neither was REPLACE-deleted.
    let after = state(&app).inner().load_resolved_warnings().unwrap();
    let surviving: std::collections::HashSet<_> = after
        .iter()
        .filter_map(|r| r.shot_uuid.as_deref())
        .collect();
    assert!(
        surviving.contains(uuid_a.as_str()),
        "shot A's by-UUID ignore must survive the renumber-onto-its-position"
    );
    assert!(
        surviving.contains(uuid_b.as_str()),
        "shot B's by-UUID ignore must survive"
    );
}

#[test]
fn test_save_drops_deleted_shots_ignore_without_clobbering_the_survivor() {
    // Issue #57 (review regression): when a shot is deleted, its by-UUID
    // resolved_warnings row is orphaned (delete_shot is in-memory only and never
    // prunes the table). The surviving successor shifts down into the deleted
    // shot's old position, so on save it re-derives to that same (section_id,
    // shot_id). If the dead row were carried over at its stale id, the two would
    // collide on UNIQUE(section_id, shot_id, warning_code) and INSERT OR REPLACE
    // would - depending on insert order - drop the SURVIVOR's still-valid ignore.
    // The carry-over must DROP the dead (UUID-not-live) row so the survivor's
    // ignore always persists and dead rows don't accumulate.
    let project = project_with_section(vec![shot_data(0.1), shot_data(0.1)]);
    let (app_state, _db) = crate::test_helpers::app_state_with_project_db(project).unwrap();
    let app = app_with_custom_state(app_state);

    // Ignore BOTH shots' SHORT_SHOT warnings (stored by UUID at positions 0 & 1).
    let zero: Vec<_> = validation::get_project_warnings(state(&app))
        .into_iter()
        .filter(|w| w.code == "SHORT_SHOT")
        .collect();
    assert_eq!(zero.len(), 2, "expected a short-shot warning per shot");
    for w in &zero {
        validation::resolve_warning(state(&app), w.section_id, w.shot_id, w.code.clone(), None)
            .unwrap();
    }

    let loaded = state(&app).inner().get_project_state().unwrap();
    let section_id = loaded.sections[0].id;
    let shot_a_id = loaded.sections[0].shots[0].id;
    let survivor_uuid = loaded.sections[0].shots[1]
        .uuid
        .clone()
        .expect("survivor uuid");

    // Delete the first shot. The successor shifts into its index/position, so the
    // orphaned by-UUID row for the deleted shot now shares the survivor's new
    // position. (No datums → not frozen → delete succeeds.)
    assert_success(&shot::delete_shot(state(&app), section_id, shot_a_id));

    let dir = tempfile::tempdir().unwrap();
    let _ = save_project_to_temp(&app, &dir);

    // The survivor's ignore must persist...
    let after = validation::get_project_warnings(state(&app));
    let survivor = after
        .iter()
        .find(|w| w.code == "SHORT_SHOT")
        .expect("survivor's short-shot warning");
    assert!(
        survivor.is_resolved,
        "the surviving shot's ignore must not be clobbered by the deleted shot's dead row"
    );
    // ...and the deleted shot's dead row must have been pruned (one row, keyed to
    // the survivor), so dead rows don't accumulate across saves.
    let rows = state(&app).inner().load_resolved_warnings().unwrap();
    assert_eq!(
        rows.len(),
        1,
        "the deleted shot's dead resolved row must be pruned"
    );
    assert_eq!(
        rows[0].shot_uuid.as_deref(),
        Some(survivor_uuid.as_str()),
        "the surviving row must be the survivor's by-UUID ignore"
    );
}

#[test]
fn test_project_validation_drives_warnings_and_persists() {
    // Per-project validation (#57): warnings use the project's own thresholds,
    // and those thresholds survive a save into the .stalx.
    let mut project = project_with_section(vec![shot_data(1.0)]);
    project.ensure_shot_uuids();
    let (app_state, _db) = crate::test_helpers::app_state_with_project_db(project).unwrap();
    let app = app_with_custom_state(app_state);

    // A 1.0 m shot is not short under the default 0.3 m minimum.
    assert!(validation::get_project_warnings(state(&app))
        .iter()
        .all(|w| w.code != "SHORT_SHOT"));

    // Raise the project's minimum length to 2.0 m via the settings command.
    let validation_cfg = crate::config::ValidationConfig {
        warn_shot_length_min: 2.0,
        ..crate::config::ValidationConfig::default()
    };
    assert_success(&project::update_project_settings(
        state(&app),
        "Renamed Cave".to_string(),
        validation_cfg,
    ));

    // Now it's flagged - warnings use the PROJECT's thresholds, not the global.
    assert!(validation::get_project_warnings(state(&app))
        .iter()
        .any(|w| w.code == "SHORT_SHOT"));

    // Save, then reopen the .stalx fresh: the thresholds (and name) persisted.
    let dir = tempfile::tempdir().unwrap();
    let saved = save_project_to_temp(&app, &dir);
    let reopened = Database::open(&saved).unwrap().load_project().unwrap();
    assert_eq!(reopened.validation.warn_shot_length_min, 2.0);
    assert_eq!(reopened.name, "Renamed Cave");
}

#[test]
fn test_update_project_settings_is_undoable() {
    // The settings change must be its own undo entry (#57 review). Otherwise
    // undoing an unrelated edit made earlier would silently revert the
    // name/validation back to whatever the edit's snapshot captured.
    let mut project = project_with_section(vec![shot_data(1.0)]);
    project.ensure_shot_uuids();
    let (app_state, _db) = crate::test_helpers::app_state_with_project_db(project).unwrap();
    let app = app_with_custom_state(app_state);

    let validation_cfg = crate::config::ValidationConfig {
        warn_shot_length_min: 2.0,
        ..crate::config::ValidationConfig::default()
    };
    assert_success(&project::update_project_settings(
        state(&app),
        "Renamed Cave".to_string(),
        validation_cfg,
    ));
    assert!(
        history::can_undo(state(&app)),
        "settings change must be undoable"
    );
    let applied = state(&app).get_project_state().unwrap();
    assert_eq!(applied.name, "Renamed Cave");
    assert_eq!(applied.validation.warn_shot_length_min, 2.0);

    // Undo reverts the settings change instead of leaving it stranded.
    assert_success(&history::undo(state(&app)));
    let reverted = state(&app).get_project_state().unwrap();
    assert_eq!(reverted.name, "Test Cave");
    assert_eq!(reverted.validation.warn_shot_length_min, 0.3);
}

#[test]
fn test_resolve_warning_refuses_to_ignore_over_anchoring() {
    // Over-anchoring is not ignorable (issue #57): the resolve_warning command
    // must refuse to persist a suppression for it.
    let project = project_with_section(vec![shot_data(0.0)]);
    let (app_state, _db) = crate::test_helpers::app_state_with_project_db(project).unwrap();
    let app = app_with_custom_state(app_state);

    validation::resolve_warning(
        state(&app),
        0,
        0,
        crate::models::MULTIPLE_DATUMS_PER_MERGE_COMPONENT.to_string(),
        None,
    )
    .unwrap();

    assert!(
        state(&app)
            .inner()
            .load_resolved_warnings()
            .unwrap()
            .is_empty(),
        "over-anchoring must never be recorded as an ignored warning"
    );
}

#[test]
fn test_resolve_warning_refuses_error_severity_codes() {
    // Issue #57 (Item 10/14): errors are not ignorable - one predicate
    // (warning_is_ignorable) governs every code. Resolving an error-severity
    // warning must persist nothing and leave it unresolved; warnings stay ignorable.
    use crate::models::{warning_is_ignorable, MULTIPLE_DATUMS_PER_MERGE_COMPONENT};
    assert!(!warning_is_ignorable("ZERO_LENGTH"));
    assert!(!warning_is_ignorable("NEGATIVE_LRUD"));
    assert!(!warning_is_ignorable("INVALID_PITCH"));
    assert!(!warning_is_ignorable("LENGTH_BELOW_DEPTH_CHANGE"));
    assert!(!warning_is_ignorable(MULTIPLE_DATUMS_PER_MERGE_COMPONENT));
    assert!(warning_is_ignorable("SHORT_SHOT"));
    assert!(warning_is_ignorable("AZIMUTH_DEVIATION"));
    assert!(warning_is_ignorable("INCLINATION_DEPTH_MISMATCH"));

    let project = project_with_section(vec![shot_data(0.0)]); // ZERO_LENGTH (error)
    let (app_state, _db) = crate::test_helpers::app_state_with_project_db(project).unwrap();
    let app = app_with_custom_state(app_state);

    let warning = validation::get_project_warnings(state(&app))
        .into_iter()
        .find(|w| w.code == "ZERO_LENGTH")
        .expect("zero length warning");
    validation::resolve_warning(
        state(&app),
        warning.section_id,
        warning.shot_id,
        warning.code.clone(),
        None,
    )
    .unwrap();
    assert!(
        state(&app)
            .inner()
            .load_resolved_warnings()
            .unwrap()
            .is_empty(),
        "an error-severity warning must never be persisted as ignored"
    );
    let after = validation::get_project_warnings(state(&app));
    assert!(
        !after
            .iter()
            .find(|w| w.code == "ZERO_LENGTH")
            .unwrap()
            .is_resolved,
        "an error stays unresolved even after a resolve attempt"
    );
}

#[test]
fn test_cross_section_duplicate_uuid_ignore_does_not_cross_suppress() {
    // A file saved before shot UUIDs were project-wide unique can hold one id in
    // two sections, and an undo of an integrity Repair can put such a state back
    // in memory. Ignoring a warning on one shot must NOT suppress the same code
    // on the other section's shot (issue #57 / Codex) - suppression is keyed by
    // (section_id, uuid, code), not the UUID alone.
    let mut project = Project::new("Dup UUID".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot_data(0.1)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot_data(0.1)],
            ..Default::default()
        })
        .id;
    // Force the same (cross-section) UUID on both shots - a legal duplicate.
    for section in &mut project.sections {
        section.shots[0].uuid = Some("shared-uuid".to_string());
    }
    let (app_state, _db) = crate::test_helpers::app_state_with_project_db(project).unwrap();
    let app = app_with_custom_state(app_state);

    let warnings = validation::get_project_warnings(state(&app));
    let a_warning = warnings
        .iter()
        .find(|w| w.section_id == a && w.code == "SHORT_SHOT")
        .expect("section A short-shot warning");
    assert!(warnings
        .iter()
        .any(|w| w.section_id == b && w.code == "SHORT_SHOT"));

    validation::resolve_warning(
        state(&app),
        a_warning.section_id,
        a_warning.shot_id,
        "SHORT_SHOT".to_string(),
        None,
    )
    .unwrap();

    let after = validation::get_project_warnings(state(&app));
    assert!(
        after
            .iter()
            .find(|w| w.section_id == a && w.code == "SHORT_SHOT")
            .unwrap()
            .is_resolved,
        "section A's ignore is recorded"
    );
    assert!(
        !after
            .iter()
            .find(|w| w.section_id == b && w.code == "SHORT_SHOT")
            .unwrap()
            .is_resolved,
        "section B's shot shares the UUID but a different section - it must NOT be suppressed"
    );

    // Now ignore B too, then un-ignore A: B's suppression must SURVIVE, because the
    // delete is also keyed by (section_id, uuid) - un-ignoring A's shared-UUID shot
    // must not clear B's row (issue #57 / Codex round 7).
    let b_warning = after
        .iter()
        .find(|w| w.section_id == b && w.code == "SHORT_SHOT")
        .unwrap()
        .clone();
    validation::resolve_warning(
        state(&app),
        b_warning.section_id,
        b_warning.shot_id,
        "SHORT_SHOT".to_string(),
        None,
    )
    .unwrap();
    validation::unresolve_warning(
        state(&app),
        a_warning.section_id,
        a_warning.shot_id,
        "SHORT_SHOT".to_string(),
    )
    .unwrap();
    let cleared = validation::get_project_warnings(state(&app));
    assert!(
        !cleared
            .iter()
            .find(|w| w.section_id == a && w.code == "SHORT_SHOT")
            .unwrap()
            .is_resolved,
        "section A was un-ignored"
    );
    assert!(
        cleared
            .iter()
            .find(|w| w.section_id == b && w.code == "SHORT_SHOT")
            .unwrap()
            .is_resolved,
        "section B's ignore (same UUID, different section) must survive A's un-ignore"
    );
}

#[test]
fn test_over_anchoring_warning_is_never_suppressible() {
    // The authoritative invariant: even a stray resolved row (legacy, bulk, or
    // any other path) must NOT mark an over-anchoring warning resolved. It clears
    // only by removing a datum, after which the warning is not generated at all.
    use crate::models::{StationMerge, ZeroDatum};

    let mut project = Project::new("OverAnchored".to_string(), None);
    let a = project
        .add_section(SectionData {
            shots: vec![shot_data(10.0)],
            ..Default::default()
        })
        .id;
    let b = project
        .add_section(SectionData {
            shots: vec![shot_data(10.0)],
            ..Default::default()
        })
        .id;
    project.station_merges.push(StationMerge {
        station_a: StationRef::new(a, 1).into(),
        station_b: StationRef::new(b, 0).into(),
        merged_at: chrono::Utc::now(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(a, 0).into(),
        gps: GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
    });
    project.zero_datums.push(ZeroDatum {
        station_ref: StationRef::new(b, 0).into(),
        gps: GpsCoordinates::new(45.0, -90.0, 0.0).unwrap(),
    });
    project.ensure_shot_uuids();

    let (app_state, _db) = crate::test_helpers::app_state_with_project_db(project).unwrap();
    let app = app_with_custom_state(app_state);

    let datum_warning = validation::get_project_warnings(state(&app))
        .into_iter()
        .find(|w| w.code == crate::models::MULTIPLE_DATUMS_PER_MERGE_COMPONENT)
        .expect("over-anchored merge component must raise a warning");
    assert!(!datum_warning.is_resolved, "freshly raised, not resolved");

    // Force a stray resolved row keyed exactly as the warning (as a bulk save or
    // a legacy file could). Compute must still refuse to mark it resolved.
    state(&app)
        .inner()
        .save_resolved_warning(&crate::persistence::ResolvedWarning {
            section_id: datum_warning.section_id,
            shot_id: datum_warning.shot_id,
            warning_code: datum_warning.code.clone(),
            resolved_at: chrono::Utc::now(),
            resolved_comment: None,
            shot_uuid: datum_warning.shot_uuid.clone(),
        })
        .unwrap();

    let after = validation::get_project_warnings(state(&app));
    let w = after
        .iter()
        .find(|w| w.code == crate::models::MULTIPLE_DATUMS_PER_MERGE_COMPONENT)
        .expect("over-anchoring warning still present");
    assert!(
        !w.is_resolved,
        "over-anchoring must never be suppressed by a resolved row"
    );
}

#[test]
fn test_get_project_warnings_detects_invalid_azimuth() {
    let app = app_with_state();
    create_project(&app);
    let mut shot = shot_data(1.0);
    shot.azimuth_in = -1.0;
    create_section_with_shots(&app, vec![shot]);
    let warnings = validation::get_project_warnings(state(&app));
    assert!(warnings.iter().any(|w| w.code == "INVALID_AZIMUTH"));
}

#[test]
fn test_get_project_warnings_detects_negative_lrud() {
    let app = app_with_state();
    create_project(&app);
    let mut shot = shot_data(1.0);
    shot.left = Some(-1.0);
    create_section_with_shots(&app, vec![shot]);
    let warnings = validation::get_project_warnings(state(&app));
    assert!(warnings.iter().any(|w| w.code == "NEGATIVE_LRUD"));
}

#[test]
fn test_get_project_warnings_detects_azimuth_deviation() {
    let app = app_with_state();
    create_project(&app);
    let mut shot = shot_data(5.0);
    shot.azimuth_in = 0.0;
    shot.azimuth_out = 45.0; // 45° difference exceeds 10° threshold
    create_section_with_shots(&app, vec![shot]);
    let warnings = validation::get_project_warnings(state(&app));
    assert!(warnings.iter().any(|w| w.code == "AZIMUTH_DEVIATION"));
}

// === Misc command coverage ===

#[test]
fn test_create_section_with_date_roundtrip() {
    let app = app_with_state();
    create_project(&app);
    let date = NaiveDate::from_ymd_opt(2025, 1, 1).unwrap();
    let result = section::create_section(
        state(&app),
        Some("Dated".to_string()),
        None,
        Some(date),
        "#4ecdc4".to_string(),
        Some(SurveyDirection::Out),
        vec![shot_data(1.0)],
    );
    assert_success(&result);
    let section = get_section(state(&app), result.section_id.unwrap()).unwrap();
    assert_eq!(section.date, Some(date));
    assert_eq!(section.direction, SurveyDirection::Out);
}

#[test]
fn test_create_section_and_merge_fails_without_project() {
    let app = app_with_state();
    let result = section::create_section_and_merge(
        state(&app),
        Some("Imported".to_string()),
        None,
        None,
        "#4ecdc4".to_string(),
        Some(SurveyDirection::In),
        vec![shot_data(1.0)],
        0,
        StationRef::new(0, 0),
        ClosureStrategy::Batch,
    );
    assert_failure_contains(&result, "No project open");
}

#[test]
fn test_create_section_and_merge_success() {
    let app = app_with_state();
    create_project(&app);
    let existing_section_id = create_section_with_shots(&app, vec![shot_data(1.0)]);

    let result = section::create_section_and_merge(
        state(&app),
        Some("Imported".to_string()),
        None,
        None,
        "#4ecdc4".to_string(),
        Some(SurveyDirection::In),
        vec![shot_data(2.0)],
        0,
        StationRef::new(existing_section_id, 0),
        ClosureStrategy::Batch,
    );

    assert_success(&result);
    let new_section_id = result.section_id.expect("new section id");
    let project_state = project::get_project_state(state(&app)).expect("project state");
    assert_eq!(project_state.project.sections.len(), 2);
    assert_eq!(project_state.project.station_merges.len(), 1);

    let merge = &project_state.project.station_merges[0];
    let merged_sections = [merge.station_a.section_id, merge.station_b.section_id];
    assert!(merged_sections.contains(&existing_section_id));
    assert!(merged_sections.contains(&new_section_id));
}

#[test]
fn test_merge_projects_requires_sources() {
    let app = app_with_state();
    let output_dir = TempDir::new().unwrap();
    let output_path = output_dir.path().join("merged.stalx");

    let result = project::merge_projects(
        state(&app),
        Vec::new(),
        output_path,
        "Merged".to_string(),
        false,
    );

    assert_failure_contains(&result, "No source projects provided");
}

#[test]
fn test_merge_projects_requires_two_sources() {
    let app = app_with_state();
    let project = project_with_section(vec![shot_data(1.0)]);
    let (db, _database) = TestDb::with_project(&project).unwrap();
    let output_dir = TempDir::new().unwrap();
    let output_path = output_dir.path().join("merged.stalx");

    let result = project::merge_projects(
        state(&app),
        vec![db.path().to_path_buf()],
        output_path,
        "Merged".to_string(),
        false,
    );

    assert_failure_contains(&result, "At least two projects are required");
}

#[test]
fn test_merge_projects_fails_when_output_exists() {
    let app = app_with_state();
    let project = project_with_section(vec![shot_data(1.0)]);
    let (db_a, _database_a) = TestDb::with_project(&project).unwrap();
    let (db_b, _database_b) = TestDb::with_project(&project).unwrap();
    let output_dir = TempDir::new().unwrap();
    let output_path = output_dir.path().join("merged.stalx");
    fs::write(&output_path, b"exists").unwrap();

    let result = project::merge_projects(
        state(&app),
        vec![db_a.path().to_path_buf(), db_b.path().to_path_buf()],
        output_path,
        "Merged".to_string(),
        false,
    );

    assert_failure_contains(&result, "Output file already exists");
}

#[test]
fn test_merge_projects_success_creates_output() {
    let app = app_with_state();
    let project_a = project_with_section(vec![shot_data(1.0)]);
    let project_b = project_with_section(vec![shot_data(2.0)]);
    let (db_a, _database_a) = TestDb::with_project(&project_a).unwrap();
    let (db_b, _database_b) = TestDb::with_project(&project_b).unwrap();
    let output_dir = TempDir::new().unwrap();
    let output_path = output_dir.path().join("merged.stalx");

    let result = project::merge_projects(
        state(&app),
        vec![db_a.path().to_path_buf(), db_b.path().to_path_buf()],
        output_path.clone(),
        "Merged".to_string(),
        false,
    );

    assert_success(&result);
    assert!(output_path.exists());
}

#[test]
fn test_update_config_persists_and_reload_restores() {
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            let mut cfg = config::get_config(state(&app));
            let original_auto_save = cfg.general.auto_save;
            cfg.general.auto_save = !original_auto_save;
            config::update_config_impl(state(&app).inner(), cfg).expect("update config");

            let updated = config::get_config(state(&app));
            assert_eq!(updated.general.auto_save, !original_auto_save);

            let (reloaded, auth_changed) = state(&app).reload_config().expect("reload config");
            assert!(
                !auth_changed,
                "ordinary preference reload preserves the session"
            );
            assert_eq!(reloaded.general.auto_save, !original_auto_save);
        });
    });
}

#[test]
fn test_reload_config_syncs_undo_history_limit() {
    with_config_lock(|| {
        let app = app_with_state();
        with_restored_config(&app, || {
            let mut cfg = config::get_config(state(&app));
            cfg.general.undo_history_limit = 42;
            config::update_config_impl(state(&app).inner(), cfg).expect("update config");

            let (reloaded, auth_changed) = state(&app).reload_config().expect("reload config");
            assert!(
                !auth_changed,
                "ordinary preference reload preserves the session"
            );
            assert_eq!(reloaded.general.undo_history_limit, 42);

            // Verify the in-memory config also reflects the reloaded limit
            let current = config::get_config(state(&app));
            assert_eq!(current.general.undo_history_limit, 42);
        });
    });
}

// === get_segment_geometry ===

#[test]
fn test_get_segment_geometry_no_project_errors() {
    let app = app_with_state();
    let result = section::get_segment_geometry_impl(state(&app).inner(), 0);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("No project open"));
}

#[test]
fn test_get_segment_geometry_invalid_section_errors() {
    let app = app_with_state();
    create_project(&app);
    let result = section::get_segment_geometry_impl(state(&app).inner(), 999);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not found"));
}

#[test]
fn test_get_segment_geometry_single_section() {
    let app = app_with_state();
    create_project(&app);
    let id = create_section_with_shots(&app, vec![shot_data(5.0), shot_data(3.0)]);

    let geom = section::get_segment_geometry_impl(state(&app).inner(), id).unwrap();
    assert_eq!(geom.sections.len(), 1);
    assert_eq!(geom.sections[0].section_id, id);
    // 2 shots → 2 shot lines (station 0→1, station 1→2)
    assert_eq!(geom.sections[0].shot_lines.len(), 2);
    // First station is the anchor, so first shot line starts at (0, 0)
    let first_line = &geom.sections[0].shot_lines[0];
    assert!(
        (first_line[0][0]).abs() < 1e-6,
        "First station X should be 0 (anchor)"
    );
    assert!(
        (first_line[0][1]).abs() < 1e-6,
        "First station Y should be 0 (anchor)"
    );
}

#[test]
fn test_get_segment_geometry_connected_sections() {
    let app = app_with_state();
    create_project(&app);
    let id_a = create_section_with_shots(&app, vec![shot_data(5.0)]);
    let id_b = create_section_with_shots(&app, vec![shot_data(3.0)]);

    // Merge section A station 1 with section B station 0
    assert_success(&station::merge_stations(
        state(&app),
        StationRef::new(id_a, 1),
        StationRef::new(id_b, 0),
    ));

    // Query segment from section A
    let geom = section::get_segment_geometry_impl(state(&app).inner(), id_a).unwrap();
    assert_eq!(
        geom.sections.len(),
        2,
        "Both connected sections should be in the segment"
    );

    // Both sections should have shot lines
    let section_ids: Vec<u32> = geom.sections.iter().map(|s| s.section_id).collect();
    assert!(section_ids.contains(&id_a));
    assert!(section_ids.contains(&id_b));

    // Section A should have 1 shot line, section B should have 1 shot line
    for s in &geom.sections {
        assert_eq!(
            s.shot_lines.len(),
            1,
            "Each section has 1 shot → 1 shot line"
        );
    }

    // Coordinates should be relative to section A station 0 (the anchor)
    let sec_a = geom.sections.iter().find(|s| s.section_id == id_a).unwrap();
    assert!(
        (sec_a.shot_lines[0][0][0]).abs() < 1e-6,
        "Section A starts at anchor origin"
    );
    assert!(
        (sec_a.shot_lines[0][0][1]).abs() < 1e-6,
        "Section A starts at anchor origin"
    );
}

#[test]
fn test_get_segment_geometry_unconnected_section_excluded() {
    let app = app_with_state();
    create_project(&app);
    let id_a = create_section_with_shots(&app, vec![shot_data(5.0)]);
    let _id_b = create_section_with_shots(&app, vec![shot_data(3.0)]);

    // No merge - sections are disconnected
    let geom = section::get_segment_geometry_impl(state(&app).inner(), id_a).unwrap();
    assert_eq!(
        geom.sections.len(),
        1,
        "Only the queried section should be returned"
    );
    assert_eq!(geom.sections[0].section_id, id_a);
}

#[test]
fn test_get_segment_geometry_queried_from_either_end() {
    let app = app_with_state();
    create_project(&app);
    let id_a = create_section_with_shots(&app, vec![shot_data(5.0)]);
    let id_b = create_section_with_shots(&app, vec![shot_data(3.0)]);

    assert_success(&station::merge_stations(
        state(&app),
        StationRef::new(id_a, 1),
        StationRef::new(id_b, 0),
    ));

    // Query from section B - should still include both sections
    let geom = section::get_segment_geometry_impl(state(&app).inner(), id_b).unwrap();
    assert_eq!(geom.sections.len(), 2);

    // Coordinates should now be relative to section B station 0
    let sec_b = geom.sections.iter().find(|s| s.section_id == id_b).unwrap();
    assert!(
        (sec_b.shot_lines[0][0][0]).abs() < 1e-6,
        "Section B starts at anchor origin"
    );
    assert!(
        (sec_b.shot_lines[0][0][1]).abs() < 1e-6,
        "Section B starts at anchor origin"
    );
}

// === Raw data commands ===

#[test]
fn test_get_raw_table_names_lists_project_tables() {
    let project = project_with_section(vec![shot_data(5.0)]);
    let (app_state, _db) = crate::test_helpers::app_state_with_project_db(project).unwrap();
    let app = app_with_custom_state(app_state);

    let names = raw_data::get_raw_table_names_impl(state(&app).inner()).unwrap();
    for expected in ["project", "sections", "shots"] {
        assert!(
            names.iter().any(|n| n == expected),
            "missing table {expected}"
        );
    }
    assert!(!names.iter().any(|n| n.starts_with("sqlite_")));
}

#[test]
fn test_get_raw_table_row_shape_and_total() {
    let project = project_with_section(vec![shot_data(5.0), shot_data(3.0), shot_data(2.0)]);
    let (app_state, _db) = crate::test_helpers::app_state_with_project_db(project).unwrap();
    let app = app_with_custom_state(app_state);

    let data =
        raw_data::get_raw_table_impl(state(&app).inner(), "shots".to_string(), 100, 0).unwrap();
    assert!(!data.columns.is_empty());
    assert_eq!(data.total_rows, 3);
    assert_eq!(data.rows.len(), 3);
    for row in &data.rows {
        assert_eq!(row.len(), data.columns.len());
    }
}

#[test]
fn test_get_raw_table_offset_paging() {
    let project = project_with_section(vec![shot_data(5.0), shot_data(3.0), shot_data(2.0)]);
    let (app_state, _db) = crate::test_helpers::app_state_with_project_db(project).unwrap();
    let app = app_with_custom_state(app_state);

    let first =
        raw_data::get_raw_table_impl(state(&app).inner(), "shots".to_string(), 2, 0).unwrap();
    let second =
        raw_data::get_raw_table_impl(state(&app).inner(), "shots".to_string(), 2, 2).unwrap();
    assert_eq!(first.rows.len(), 2);
    assert_eq!(second.rows.len(), 1);
    assert_eq!(first.total_rows, 3);
    assert_eq!(second.total_rows, 3);
    assert!(!second.rows.contains(&first.rows[0]));
    assert!(!second.rows.contains(&first.rows[1]));
}

#[test]
fn test_get_raw_table_rejects_injection_names() {
    let project = project_with_section(vec![shot_data(5.0)]);
    let (app_state, _db) = crate::test_helpers::app_state_with_project_db(project).unwrap();
    let app = app_with_custom_state(app_state);

    for name in ["shots; DROP TABLE shots", "sqlite_master"] {
        let err = raw_data::get_raw_table_impl(state(&app).inner(), name.to_string(), 100, 0)
            .unwrap_err();
        assert!(
            err.contains("Unknown table"),
            "expected rejection for {name:?}, got: {err}"
        );
    }
}

#[test]
fn test_raw_data_commands_fail_without_db() {
    let app = app_with_state();

    let err = raw_data::get_raw_table_names_impl(state(&app).inner()).unwrap_err();
    assert!(err.contains("save the project"), "got: {err}");

    let err =
        raw_data::get_raw_table_impl(state(&app).inner(), "shots".to_string(), 100, 0).unwrap_err();
    assert!(err.contains("save the project"), "got: {err}");
}

// ========================================================================
// `AppState::config_write_lock` tests (#357 item 14: `modify_config` now
// holds one lock across the whole read-modify-write for the partial-update
// commands, closing the gap between a separate `get_config()` and
// `update_config()`).
// ========================================================================

#[test]
fn test_concurrent_partial_config_updates_do_not_lose_writes() {
    // Guard F: the lost-update race. `update_panel_sizes_impl` and
    // `update_data_table_view_impl` write DIFFERENT `[panels]` fields but used
    // to run get-then-update as two separate critical sections; a write from
    // one landing in the gap of the other's read-modify-write was silently
    // reverted by the other's stale snapshot. With the shared
    // `config_write_lock`, every read-modify-write is fully serialized, so
    // the LAST write either thread makes to its OWN field is deterministically
    // never lost - repeated over many trials so a reintroduced race is caught
    // with overwhelming probability rather than by luck.
    // Each iteration below is a REAL config.toml save (temp-file + rename), and
    // this test runs alongside thousands of others in the full suite; under
    // that load a `rename` can occasionally return a transient OS error
    // unrelated to the lock this test exists to cover (observed in practice,
    // sandboxed and unsandboxed alike). Retrying a few times on error keeps
    // the test honest about what it is proving - the FINAL VALUES after every
    // write eventually lands, not "zero I/O errors ever occur under load".
    fn retrying(mut op: impl FnMut() -> Result<(), String>) {
        for attempt in 0..5 {
            match op() {
                Ok(()) => return,
                Err(e) if attempt < 4 => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    eprintln!("retrying a transient config save failure: {e}");
                }
                Err(e) => {
                    Err::<(), String>(e).expect("config save kept failing after retries");
                }
            }
        }
    }

    with_config_lock(|| {
        // Kept modest (not e.g. 500x20): each iteration is a REAL config.toml
        // save, and this test already runs alongside thousands of others in
        // the full suite - a large volume here risks tripping over transient
        // OS-level file limits under that load rather than the race being
        // tested. `TRIALS` repeats the whole race rather than growing `ITERS`,
        // since it is the surface area at the TAIL of each thread's run (the
        // last write racing the other thread's last write) that catches a
        // reintroduced lost update, not raw iteration count.
        const ITERS: u32 = 25;
        const TRIALS: u32 = 12;
        // The race is between two SPAWNED threads writing one config file, and
        // config resolution is per-thread - without the handoff each worker
        // would save into a temp dir of its own and no write could ever be lost
        // to the other's, so this whole test would pass VACUOUSLY. Each worker
        // therefore asserts, once, that it really is on this file.
        let config = crate::test_helpers::ConfigHandoff::capture();
        let raced_file = crate::config::Config::config_path().expect("config path");
        // Trial-DISTINCT values, and that is the whole reason the trials count.
        // Every trial builds a fresh `AppState`, which re-loads the config the
        // previous trial left in the shared isolated directory - so with a value
        // series that does not move between trials, trial N > 1 opens already
        // holding the exact numbers it asserts at the end. Every trial but the
        // first then passes without a single write landing, and the lost-update
        // window each trial opens with is blind by construction because the stale
        // value IS the expected one. Seeding from the trial index makes the
        // starting state wrong for the assertion until this trial's own writes
        // land. Both series stay inside their normalizer's clamp
        // (`sections_panel_width` 150..400, column widths 40..2000), so what comes
        // back is what went in.
        for trial in 0..TRIALS {
            let shared = std::sync::Arc::new(AppState::new().expect("app state"));

            let a = std::sync::Arc::clone(&shared);
            let config_a = config.clone();
            let file_a = raced_file.clone();
            let handle_a = std::thread::spawn(move || {
                let _config = config_a.adopt();
                assert_eq!(
                    crate::config::Config::config_path().expect("config path"),
                    file_a,
                    "worker A must write the file worker B writes, or no update can be lost"
                );
                for i in 0..ITERS {
                    let width = 150 + trial * 10 + (i % 10);
                    retrying(|| {
                        config::update_panel_sizes_impl(a.as_ref(), Some(width), None, None, None)
                    });
                }
            });
            let b = std::sync::Arc::clone(&shared);
            let config_b = config.clone();
            let file_b = raced_file.clone();
            let handle_b = std::thread::spawn(move || {
                let _config = config_b.adopt();
                assert_eq!(
                    crate::config::Config::config_path().expect("config path"),
                    file_b,
                    "worker B must write the file worker A writes, or no update can be lost"
                );
                for i in 0..ITERS {
                    let widths =
                        BTreeMap::from([("length".to_string(), 100 + trial * 10 + (i % 10))]);
                    retrying(|| {
                        config::update_data_table_view_impl(
                            b.as_ref(),
                            None,
                            None,
                            None,
                            None,
                            None,
                            None,
                            Some(widths.clone()),
                            None,
                        )
                    });
                }
            });
            handle_a.join().expect("thread A panicked");
            handle_b.join().expect("thread B panicked");

            let last_width_a = 150 + trial * 10 + ((ITERS - 1) % 10);
            let last_width_b = 100 + trial * 10 + ((ITERS - 1) % 10);
            let cfg = shared.get_config();
            assert_eq!(
                cfg.panels.sections_panel_width, last_width_a,
                "thread A's last write to sections_panel_width was lost to a concurrent partial update"
            );
            assert_eq!(
                cfg.panels.data_table_shot_column_widths.get("length"),
                Some(&last_width_b),
                "thread B's last write to data_table_shot_column_widths was lost to a concurrent partial update"
            );
        }
    });
}

#[test]
fn test_update_data_table_view_rejects_the_whole_payload_on_invalid_mode() {
    // Guard G: `modify_config` returning `Err` from the mutate closure must
    // write NOTHING - not just leave the invalid field unchanged. Set a valid
    // field (column_order) in the SAME call as an invalid azimuth_mode and
    // confirm neither one lands.
    with_config_lock(|| {
        let app = app_with_state();
        let before = config::get_config(state(&app))
            .panels
            .data_table_shot_column_order;
        assert!(before.is_empty(), "test assumes the default is empty");

        let err = config::update_data_table_view_impl(
            state(&app).inner(),
            Some(vec!["length".to_string()]),
            None,
            Some("sideways".to_string()),
            None,
            None,
            None,
            None,
            None,
        );
        assert!(err.is_err(), "an invalid azimuth mode must be rejected");

        let after = config::get_config(state(&app));
        assert!(
            after.panels.data_table_shot_column_order.is_empty(),
            "a valid field set in the SAME rejected call must not land either: {:?}",
            after.panels.data_table_shot_column_order
        );
        assert_eq!(
            after.panels.data_table_azimuth_mode, "avg",
            "the rejected mode itself must be left at its previous value"
        );
    });
}
