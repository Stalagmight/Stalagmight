use anyhow::anyhow;
use tauri::{AppHandle, Manager, State};

use std::collections::BTreeMap;

use crate::config::{
    normalize_column_widths, Config, GasConfig, MapPaletteConfig, DATA_TABLE_ANGLE_MODES,
    DATA_WORKSPACE_TINT_MODES,
};
use crate::core::AppState;
use crate::models::Person;

/// Get the current configuration
#[tauri::command]
pub fn get_config(state: State<AppState>) -> Config {
    get_config_impl(state.inner())
}

pub(crate) fn get_config_impl(state: &AppState) -> Config {
    state.get_config()
}

/// Update the configuration.
///
/// Async, off the main thread: a save that changes the SpeleoDB instance ends the
/// session, which waits for any SpeleoDB lock operation in flight (an upload or a
/// download can run for minutes) and then hands the old project's lock back over
/// the network. As a synchronous command that froze the whole window. The partial
/// `update_*` commands below stay synchronous - they never wait on SpeleoDB (see
/// `AppState::modify_config`) - which keeps their in-order application on the
/// main thread.
#[tauri::command]
pub async fn update_config(app: AppHandle, config: Config) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        update_config_impl(app.state::<AppState>().inner(), config)
    })
    .await
    .map_err(|e| format!("settings save task failed: {e}"))?
}

pub(crate) fn update_config_impl(state: &AppState, config: Config) -> Result<(), String> {
    state.update_config(config).map_err(|e| e.to_string())
}

/// Reload configuration from disk without persisting any changes
/// Returns the reloaded config and whether the SpeleoDB credential identity changed.
///
/// Async for the same reason as [`update_config`]: a reload that finds another
/// account or instance on disk ends the SpeleoDB session.
#[tauri::command]
pub async fn reload_config(app: AppHandle) -> Result<(Config, bool), String> {
    tauri::async_runtime::spawn_blocking(move || {
        app.state::<AppState>()
            .reload_config()
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("settings reload task failed: {e}"))?
}

/// Update panel sizes in configuration (efficient partial update)
#[tauri::command]
pub fn update_panel_sizes(
    state: State<AppState>,
    sections_panel_width: Option<u32>,
    detail_panel_width: Option<u32>,
    data_table_height: Option<u32>,
    wizard_map_panel_width: Option<u32>,
) -> Result<(), String> {
    update_panel_sizes_impl(
        state.inner(),
        sections_panel_width,
        detail_panel_width,
        data_table_height,
        wizard_map_panel_width,
    )
}

/// Update the data-table view state (shot/station column order, visibility and
/// widths, angle display modes) in configuration (efficient partial update)
#[tauri::command]
#[expect(clippy::too_many_arguments)]
pub fn update_data_table_view(
    state: State<AppState>,
    column_order: Option<Vec<String>>,
    hidden_columns: Option<Vec<String>>,
    azimuth_mode: Option<String>,
    inclination_mode: Option<String>,
    station_column_order: Option<Vec<String>>,
    station_hidden_columns: Option<Vec<String>>,
    column_widths: Option<BTreeMap<String, u32>>,
    station_column_widths: Option<BTreeMap<String, u32>>,
) -> Result<(), String> {
    update_data_table_view_impl(
        state.inner(),
        column_order,
        hidden_columns,
        azimuth_mode,
        inclination_mode,
        station_column_order,
        station_hidden_columns,
        column_widths,
        station_column_widths,
    )
}

#[expect(clippy::too_many_arguments)]
pub(crate) fn update_data_table_view_impl(
    state: &AppState,
    column_order: Option<Vec<String>>,
    hidden_columns: Option<Vec<String>>,
    azimuth_mode: Option<String>,
    inclination_mode: Option<String>,
    station_column_order: Option<Vec<String>>,
    station_hidden_columns: Option<Vec<String>>,
    column_widths: Option<BTreeMap<String, u32>>,
    station_column_widths: Option<BTreeMap<String, u32>>,
) -> Result<(), String> {
    // The allowed values live in `config::types` next to the fields and the
    // repair pass that coerces them, not as a literal here: this door REJECTS an
    // unknown mode while `PanelsConfig::normalize` REPAIRS one, and two copies of
    // the list would let those two doors disagree about what is valid.
    // Read-modify-write inside ONE critical section. Doing the read with
    // `get_config()` and the write with `update_config()` left a gap in which a
    // concurrent `update_panel_sizes` could land, and whichever of the two wrote
    // second silently reverted the other from memory and from disk. See
    // `AppState::config_write_lock`.
    state
        .modify_config(|config| {
            if let Some(order) = column_order {
                config.panels.data_table_shot_column_order = order;
            }
            if let Some(hidden) = hidden_columns {
                config.panels.data_table_shot_hidden_columns = hidden;
            }
            if let Some(mode) = azimuth_mode {
                if !DATA_TABLE_ANGLE_MODES.contains(&mode.as_str()) {
                    return Err(anyhow!("Invalid azimuth mode '{mode}'"));
                }
                config.panels.data_table_azimuth_mode = mode;
            }
            if let Some(mode) = inclination_mode {
                if !DATA_TABLE_ANGLE_MODES.contains(&mode.as_str()) {
                    return Err(anyhow!("Invalid inclination mode '{mode}'"));
                }
                config.panels.data_table_inclination_mode = mode;
            }
            if let Some(order) = station_column_order {
                config.panels.data_table_station_column_order = order;
            }
            if let Some(hidden) = station_hidden_columns {
                config.panels.data_table_station_hidden_columns = hidden;
            }
            // Clamp on the way in as well as on load, so a width that arrived
            // degenerate is repaired before it reaches disk rather than only on
            // the next read. `update_config` now normalizes the whole payload as
            // well (`normalize_except_shortcuts`), so this is belt-and-braces
            // rather than the only guard it used to be - kept because it states
            // the constraint at the field it applies to.
            if let Some(mut widths) = column_widths {
                normalize_column_widths(&mut widths);
                config.panels.data_table_shot_column_widths = widths;
            }
            if let Some(mut widths) = station_column_widths {
                normalize_column_widths(&mut widths);
                config.panels.data_table_station_column_widths = widths;
            }
            Ok(())
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Update the new-section dialog's grid view state (column order, visibility and
/// widths) in configuration (efficient partial update).
///
/// Deliberately its own command rather than three more parameters on
/// `update_data_table_view`, which already carries eight and an
/// `#[expect(clippy::too_many_arguments)]`. The two share nothing but shape: the
/// dialog's grid is short-lived and its save fires on unmount, so keeping the
/// doors separate also keeps one view's flush from rewriting the other's fields
/// with whatever the caller happened to have in hand.
#[tauri::command]
pub fn update_new_section_view(
    state: State<AppState>,
    column_order: Option<Vec<String>>,
    hidden_columns: Option<Vec<String>>,
    column_widths: Option<BTreeMap<String, u32>>,
) -> Result<(), String> {
    update_new_section_view_impl(state.inner(), column_order, hidden_columns, column_widths)
}

pub(crate) fn update_new_section_view_impl(
    state: &AppState,
    column_order: Option<Vec<String>>,
    hidden_columns: Option<Vec<String>>,
    column_widths: Option<BTreeMap<String, u32>>,
) -> Result<(), String> {
    // Read-modify-write inside ONE critical section, same reason as
    // `update_data_table_view_impl`: `modify_config` is what keeps a concurrent
    // panel-size or data-table-view write from clobbering this one.
    state
        .modify_config(|config| {
            if let Some(order) = column_order {
                config.panels.new_section_column_order = order;
            }
            if let Some(hidden) = hidden_columns {
                config.panels.new_section_hidden_columns = hidden;
            }
            // Clamp on the way in as well as on load - see the same call in
            // `update_data_table_view_impl`.
            if let Some(mut widths) = column_widths {
                normalize_column_widths(&mut widths);
                config.panels.new_section_column_widths = widths;
            }
            Ok(())
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Update the edit-section dialog's shot-grid view state (column order,
/// Update the IMPORT-time shot grids' view (the mNemo wizard and zero-datum
/// config) in configuration.
///
/// Its own command for the same reason the other three are: separate `panels`
/// field sets, and a partial write to one must never carry another's keys.
#[tauri::command]
pub fn update_import_shots_view(
    state: State<AppState>,
    column_order: Option<Vec<String>>,
    hidden_columns: Option<Vec<String>>,
    column_widths: Option<BTreeMap<String, u32>>,
) -> Result<(), String> {
    update_import_shots_view_impl(state.inner(), column_order, hidden_columns, column_widths)
}

pub(crate) fn update_import_shots_view_impl(
    state: &AppState,
    column_order: Option<Vec<String>>,
    hidden_columns: Option<Vec<String>>,
    column_widths: Option<BTreeMap<String, u32>>,
) -> Result<(), String> {
    state
        .modify_config(|config| {
            if let Some(order) = column_order {
                config.panels.import_shots_column_order = order;
            }
            if let Some(hidden) = hidden_columns {
                config.panels.import_shots_hidden_columns = hidden;
            }
            if let Some(mut widths) = column_widths {
                normalize_column_widths(&mut widths);
                config.panels.import_shots_column_widths = widths;
            }
            Ok(())
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Update the Data workspace's persisted view (column order, visibility and
/// widths, row tint, map pane open + width, section header rows) in
/// configuration (efficient partial update). Only the provided fields are
/// written.
///
/// Its own command for the same reason the grid-view commands are: a disjoint
/// `panels` field set (`data_workspace_*`), so a partial write here can never
/// carry another view's keys.
#[tauri::command]
#[expect(clippy::too_many_arguments)]
pub fn update_data_workspace_view(
    state: State<AppState>,
    column_order: Option<Vec<String>>,
    hidden_columns: Option<Vec<String>>,
    column_widths: Option<BTreeMap<String, u32>>,
    tint_mode: Option<String>,
    map_pane_open: Option<bool>,
    map_pane_width: Option<u32>,
    section_headers: Option<bool>,
) -> Result<(), String> {
    update_data_workspace_view_impl(
        state.inner(),
        column_order,
        hidden_columns,
        column_widths,
        tint_mode,
        map_pane_open,
        map_pane_width,
        section_headers,
    )
}

#[expect(clippy::too_many_arguments)]
pub(crate) fn update_data_workspace_view_impl(
    state: &AppState,
    column_order: Option<Vec<String>>,
    hidden_columns: Option<Vec<String>>,
    column_widths: Option<BTreeMap<String, u32>>,
    tint_mode: Option<String>,
    map_pane_open: Option<bool>,
    map_pane_width: Option<u32>,
    section_headers: Option<bool>,
) -> Result<(), String> {
    // Read-modify-write inside ONE critical section, same reason as
    // `update_data_table_view_impl`. `modify_config` ends in the whole-config
    // normalizer, which prunes the key lists, clamps the widths and the pane
    // width - the same repair the load path applies.
    state
        .modify_config(|config| {
            // Validated BEFORE anything is written, so a rejected call leaves the
            // stored view exactly as it was. This door rejects an unknown tint
            // while `PanelsConfig::normalize` repairs one; both read the one list.
            if let Some(mode) = &tint_mode {
                if !DATA_WORKSPACE_TINT_MODES.contains(&mode.as_str()) {
                    return Err(anyhow!("Invalid Data workspace tint mode '{mode}'"));
                }
            }
            let panels = &mut config.panels;
            if let Some(order) = column_order {
                panels.data_workspace_column_order = order;
            }
            if let Some(hidden) = hidden_columns {
                panels.data_workspace_hidden_columns = hidden;
            }
            if let Some(mut widths) = column_widths {
                normalize_column_widths(&mut widths);
                panels.data_workspace_column_widths = widths;
            }
            if let Some(mode) = tint_mode {
                panels.data_workspace_tint_mode = mode;
            }
            if let Some(open) = map_pane_open {
                panels.data_workspace_map_pane_open = open;
            }
            if let Some(width) = map_pane_width {
                panels.data_workspace_map_pane_width = width;
            }
            if let Some(on) = section_headers {
                panels.data_workspace_section_headers = on;
            }
            Ok(())
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Update the edit-section dialog's shot-grid view (column order, visibility and
/// widths) in configuration (efficient partial update).
///
/// Its own command for the same reason `update_new_section_view` is: three
/// separate grids, three disjoint `panels` field sets, and a partial write to
/// one must never carry another's keys.
#[tauri::command]
pub fn update_edit_section_view(
    state: State<AppState>,
    column_order: Option<Vec<String>>,
    hidden_columns: Option<Vec<String>>,
    column_widths: Option<BTreeMap<String, u32>>,
) -> Result<(), String> {
    update_edit_section_view_impl(state.inner(), column_order, hidden_columns, column_widths)
}

pub(crate) fn update_edit_section_view_impl(
    state: &AppState,
    column_order: Option<Vec<String>>,
    hidden_columns: Option<Vec<String>>,
    column_widths: Option<BTreeMap<String, u32>>,
) -> Result<(), String> {
    state
        .modify_config(|config| {
            if let Some(order) = column_order {
                config.panels.edit_section_column_order = order;
            }
            if let Some(hidden) = hidden_columns {
                config.panels.edit_section_hidden_columns = hidden;
            }
            if let Some(mut widths) = column_widths {
                normalize_column_widths(&mut widths);
                config.panels.edit_section_column_widths = widths;
            }
            Ok(())
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

pub(crate) fn update_panel_sizes_impl(
    state: &AppState,
    sections_panel_width: Option<u32>,
    detail_panel_width: Option<u32>,
    data_table_height: Option<u32>,
    wizard_map_panel_width: Option<u32>,
) -> Result<(), String> {
    // One critical section, same reason as `update_data_table_view_impl`: this is
    // the OTHER half of the pair that raced. `savePanelSizes` fires undebounced on
    // divider release while the data-table view save fires 500 ms after the last
    // change, and both live in `SingleSystemView`.
    state
        .modify_config(|config| {
            if let Some(w) = sections_panel_width {
                config.panels.sections_panel_width = w;
            }
            if let Some(w) = detail_panel_width {
                config.panels.detail_panel_width = w;
            }
            if let Some(h) = data_table_height {
                config.panels.data_table_height = h;
            }
            if let Some(w) = wizard_map_panel_width {
                config.panels.wizard_map_panel_width = w;
            }
            Ok(())
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Update per-panel reveal layout (open state + hover/pin mode) in configuration
/// (efficient partial update). Only provided fields are written.
#[tauri::command]
pub fn update_panel_layout(
    state: State<AppState>,
    sections_panel_open: Option<bool>,
    sections_panel_mode: Option<String>,
    detail_panel_open: Option<bool>,
    detail_panel_mode: Option<String>,
    data_table_open: Option<bool>,
    data_table_mode: Option<String>,
) -> Result<(), String> {
    update_panel_layout_impl(
        state.inner(),
        sections_panel_open,
        sections_panel_mode,
        detail_panel_open,
        detail_panel_mode,
        data_table_open,
        data_table_mode,
    )
}

pub(crate) fn update_panel_layout_impl(
    state: &AppState,
    sections_panel_open: Option<bool>,
    sections_panel_mode: Option<String>,
    detail_panel_open: Option<bool>,
    detail_panel_mode: Option<String>,
    data_table_open: Option<bool>,
    data_table_mode: Option<String>,
) -> Result<(), String> {
    const MODES: [&str; 2] = ["pin", "hover"];
    let validate = |m: &str| -> Result<(), anyhow::Error> {
        if MODES.contains(&m) {
            Ok(())
        } else {
            Err(anyhow!("Invalid panel mode '{m}'"))
        }
    };
    // The third command with the read-modify-write shape, and #357 item 14 named
    // only the first two. Its exposure is the same: the reveal-layout writes fire
    // on panel open/close and mode flips, i.e. exactly while the user is dragging
    // dividers and resizing columns.
    state
        .modify_config(|config| {
            if let Some(open) = sections_panel_open {
                config.panels.sections_panel_open = open;
            }
            if let Some(mode) = sections_panel_mode {
                validate(&mode)?;
                config.panels.sections_panel_mode = mode;
            }
            if let Some(open) = detail_panel_open {
                config.panels.detail_panel_open = open;
            }
            if let Some(mode) = detail_panel_mode {
                validate(&mode)?;
                config.panels.detail_panel_mode = mode;
            }
            if let Some(open) = data_table_open {
                config.panels.data_table_open = open;
            }
            if let Some(mode) = data_table_mode {
                validate(&mode)?;
                config.panels.data_table_mode = mode;
            }
            Ok(())
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Update the `[gas]` block in configuration (efficient partial update).
///
/// Exists so Plan mode's gas controls stop persisting through a whole-`Config`
/// write (#357 item 2). `gasConfigDisk.ts` built its payload by spreading
/// `useProjectStore.getState().config`, a snapshot refreshed only by `loadConfig()`
/// after a write completes - so any OTHER partial save that landed inside that
/// window was overwritten by the stale copy of its own fields. It was reachable
/// in Plan mode by resizing a data-table column (500 ms debounce) and nudging a
/// gas field inside that window (450 ms debounce): the widths were lost, and
/// silently, because the open table keeps rendering its own state and only
/// reverts on the next launch.
///
/// Serializing the two commands could not fix that - the payload was already
/// stale when it was built, so ordering only decides which stale copy wins. Not
/// sending the other fields at all is what fixes it.
#[tauri::command]
pub fn update_gas_config(state: State<AppState>, gas: GasConfig) -> Result<(), String> {
    update_gas_config_impl(state.inner(), gas)
}

pub(crate) fn update_gas_config_impl(state: &AppState, gas: GasConfig) -> Result<(), String> {
    // Normalization is not skipped by going partial: `modify_config` ends in the
    // same `normalize_except_shortcuts`, which runs `GasConfig::normalize`.
    state
        .modify_config(|config| {
            config.gas = gas;
            Ok(())
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Update the `[map_palette]` block: the depth / distance colour palettes and
/// the pinned palettes (efficient partial update).
///
/// The palette picker on the map legend and in the export panel writes here.
/// Partial for the same reason as [`update_gas_config`]: a whole-`Config`
/// write built from a store snapshot could silently revert another partial
/// save. Global config only - the palette never reaches the `.stalx`
/// (`save_display_setting`) or SpeleoDB.
#[tauri::command]
pub fn update_map_palette(state: State<AppState>, palette: MapPaletteConfig) -> Result<(), String> {
    update_map_palette_impl(state.inner(), palette)
}

pub(crate) fn update_map_palette_impl(
    state: &AppState,
    palette: MapPaletteConfig,
) -> Result<(), String> {
    // `modify_config` ends in `normalize_except_shortcuts`, which runs
    // `MapPaletteConfig::normalize` (bad ids -> theme, pins deduped and capped).
    state
        .modify_config(|config| {
            config.map_palette = palette;
            Ok(())
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Remember `people` as the most recently credited, for the surveyor and
/// explorer suggestions in every project (`[people].recent`, a partial write).
///
/// The given people move to the FRONT in order (`people[0]` becomes the most
/// recent); an entry already listed under the same identity is replaced, so the
/// new spelling wins; the list is then normalized and capped at
/// [`crate::config::MAX_RECENT_PEOPLE`]. Returns the stored list. An empty (or
/// all-invalid) input writes nothing and returns the current list. Partial for
/// the same reason as [`update_gas_config`]; global config only, never the
/// `.stalx` or SpeleoDB.
#[tauri::command]
pub fn remember_people(state: State<AppState>, people: Vec<Person>) -> Result<Vec<Person>, String> {
    remember_people_impl(state.inner(), &people)
}

pub(crate) fn remember_people_impl(
    state: &AppState,
    people: &[Person],
) -> Result<Vec<Person>, String> {
    if !people.iter().any(Person::is_valid) {
        return Ok(state.get_config().people.recent);
    }
    state
        .modify_config(|config| {
            config.people.remember(people);
            Ok(())
        })
        .map_err(|e| e.to_string())?;
    Ok(state.get_config().people.recent)
}

/// Load all display settings from the current project database
#[tauri::command]
pub fn load_display_settings(
    state: State<AppState>,
) -> Result<std::collections::HashMap<String, String>, String> {
    load_display_settings_impl(state.inner())
}

pub(crate) fn load_display_settings_impl(
    state: &AppState,
) -> Result<std::collections::HashMap<String, String>, String> {
    state.load_display_settings().map_err(|e| e.to_string())
}

/// Save a display setting to the current project database
#[tauri::command]
pub fn save_display_setting(
    state: State<AppState>,
    key: String,
    value: String,
) -> Result<(), String> {
    save_display_setting_impl(state.inner(), key, value)
}

pub(crate) fn save_display_setting_impl(
    state: &AppState,
    key: String,
    value: String,
) -> Result<(), String> {
    state
        .save_display_setting(&key, &value)
        .map_err(|e| e.to_string())
}
