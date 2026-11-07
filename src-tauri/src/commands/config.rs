use anyhow::anyhow;
use tauri::{AppHandle, Runtime};

use std::collections::BTreeMap;

use crate::commands::pool;
use crate::config::{
    normalize_column_widths, Config, ConfigPatch, ConfigSaveReport, GasConfig,
    DATA_TABLE_ANGLE_MODES,
};
use crate::core::CommandError;

/// Get the current configuration.
///
/// [#576 NF-W7E-5] A `Result` like every other read: a body that crashed on the
/// pool answers `INTERNAL_ERROR` instead of re-raising, which left the call
/// unresolved (the frontend's `loadConfig` catches the failure and keeps the
/// settings it has).
#[tauri::command]
pub async fn get_config<R: Runtime>(app: AppHandle<R>) -> Result<Config, CommandError> {
    pool::run(app, |state| Ok(state.get_config())).await
}

/// Update the configuration.
///
/// The [`ConfigSaveReport`] is the whole answer, not decoration: a save that wrote
/// every setting can still have failed to evict the API key from the OS keyring it
/// was just told to stop using, and `Ok(())` reported that as unqualified success.
///
/// [#576 NF-W4M-1] Why this whole-config write stays beside [`patch_config`]: every
/// frontend writer sends only what it changed through `patch_config` (D27-32(c)), so a
/// write can no longer revert what the backend recorded since the writer's snapshot.
/// That diff needs a snapshot to diff against, and the one caller left is the store's
/// `updateConfig` when it has NONE (the config has not loaded): then the given config
/// is all there is to send. It is the same write the load path's normalizer runs
/// (`AppState::update_config`), so it carries no separate behaviour to keep in step.
#[tauri::command]
pub async fn update_config<R: Runtime>(
    app: AppHandle<R>,
    config: Config,
) -> Result<ConfigSaveReport, String> {
    pool::run(app, move |state| {
        state.update_config(config).map_err(|e| e.to_string())
    })
    .await
}

/// [#576 D27-32(c)] Apply only the given settings onto the live config, then
/// normalize and save it. The Settings dialog sends the settings the user changed
/// (and the other config writers the one each owns), so a save can no longer revert
/// what the backend recorded since the dialog's snapshot was taken. Refused whole
/// when a path names no setting or a value does not fit it.
#[tauri::command]
pub async fn patch_config<R: Runtime>(
    app: AppHandle<R>,
    patches: Vec<ConfigPatch>,
) -> Result<ConfigSaveReport, String> {
    pool::run(app, move |state| {
        state.patch_config(&patches).map_err(|e| e.to_string())
    })
    .await
}

/// Reload configuration from disk without persisting any changes
/// This re-reads the config file and updates the in-memory state
#[tauri::command]
pub async fn reload_config<R: Runtime>(app: AppHandle<R>) -> Result<Config, String> {
    pool::run(app, |state| {
        state.reload_config().map_err(|e| e.to_string())
    })
    .await
}

/// Update panel sizes in configuration (efficient partial update)
#[tauri::command]
pub async fn update_panel_sizes<R: Runtime>(
    app: AppHandle<R>,
    sections_panel_width: Option<u32>,
    detail_panel_width: Option<u32>,
    data_table_height: Option<u32>,
    wizard_map_panel_width: Option<u32>,
) -> Result<(), String> {
    pool::run(app, move |state| {
        // One critical section, same reason as `update_data_table_view`: this is
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
    })
    .await
}

/// Update the data-table view state (shot/station column order, visibility and
/// widths, angle display modes) in configuration (efficient partial update)
#[tauri::command]
#[expect(clippy::too_many_arguments)]
pub async fn update_data_table_view<R: Runtime>(
    app: AppHandle<R>,
    column_order: Option<Vec<String>>,
    hidden_columns: Option<Vec<String>>,
    azimuth_mode: Option<String>,
    inclination_mode: Option<String>,
    station_column_order: Option<Vec<String>>,
    station_hidden_columns: Option<Vec<String>>,
    column_widths: Option<BTreeMap<String, u32>>,
    station_column_widths: Option<BTreeMap<String, u32>>,
) -> Result<(), String> {
    pool::run(app, move |state| {
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
    })
    .await
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
pub async fn update_new_section_view<R: Runtime>(
    app: AppHandle<R>,
    column_order: Option<Vec<String>>,
    hidden_columns: Option<Vec<String>>,
    column_widths: Option<BTreeMap<String, u32>>,
) -> Result<(), String> {
    pool::run(app, move |state| {
        // Read-modify-write inside ONE critical section, same reason as
        // `update_data_table_view`: `modify_config` is what keeps a concurrent
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
                // `update_data_table_view`.
                if let Some(mut widths) = column_widths {
                    normalize_column_widths(&mut widths);
                    config.panels.new_section_column_widths = widths;
                }
                Ok(())
            })
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
    .await
}

/// Update the edit-section dialog's shot-grid view state (column order,
/// Update the IMPORT-time shot grids' view (the mNemo wizard and zero-datum
/// config) in configuration.
///
/// Its own command for the same reason the other three are: separate `panels`
/// field sets, and a partial write to one must never carry another's keys.
#[tauri::command]
pub async fn update_import_shots_view<R: Runtime>(
    app: AppHandle<R>,
    column_order: Option<Vec<String>>,
    hidden_columns: Option<Vec<String>>,
    column_widths: Option<BTreeMap<String, u32>>,
) -> Result<(), String> {
    pool::run(app, move |state| {
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
    })
    .await
}

/// Update the edit-section dialog's shot-grid view (column order, visibility and
/// widths) in configuration (efficient partial update).
///
/// Its own command for the same reason `update_new_section_view` is: three
/// separate grids, three disjoint `panels` field sets, and a partial write to
/// one must never carry another's keys.
#[tauri::command]
pub async fn update_edit_section_view<R: Runtime>(
    app: AppHandle<R>,
    column_order: Option<Vec<String>>,
    hidden_columns: Option<Vec<String>>,
    column_widths: Option<BTreeMap<String, u32>>,
) -> Result<(), String> {
    pool::run(app, move |state| {
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
    })
    .await
}

/// Update per-panel reveal layout (open state + hover/pin mode) in configuration
/// (efficient partial update). Only provided fields are written.
#[tauri::command]
pub async fn update_panel_layout<R: Runtime>(
    app: AppHandle<R>,
    sections_panel_open: Option<bool>,
    sections_panel_mode: Option<String>,
    detail_panel_open: Option<bool>,
    detail_panel_mode: Option<String>,
    data_table_open: Option<bool>,
    data_table_mode: Option<String>,
) -> Result<(), String> {
    pool::run(app, move |state| {
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
    })
    .await
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
pub async fn update_gas_config<R: Runtime>(
    app: AppHandle<R>,
    gas: GasConfig,
) -> Result<(), String> {
    pool::run(app, move |state| {
        // Normalization is not skipped by going partial: `modify_config` ends in the
        // same `normalize_except_shortcuts`, which runs `GasConfig::normalize`.
        state
            .modify_config(|config| {
                config.gas = gas;
                Ok(())
            })
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
    .await
}

/// Load all display settings from the current project database
#[tauri::command]
pub async fn load_display_settings<R: Runtime>(
    app: AppHandle<R>,
) -> Result<std::collections::HashMap<String, String>, String> {
    pool::run(app, |state| {
        state.load_display_settings().map_err(|e| e.to_string())
    })
    .await
}

/// Save a display setting to the current project database
///
/// [#576 RV6b-2] `made_for` names the project the setting was made for (the
/// frontend's open project - its `created_at` and, since NF-W8FE-3, the file it is
/// open from - `None` when it had none); a write for a project that is no longer
/// open is refused with `PROJECT_CHANGED` (`AppState::save_display_setting_for`).
#[tauri::command]
pub async fn save_display_setting<R: Runtime>(
    app: AppHandle<R>,
    key: String,
    value: String,
    made_for: Option<crate::core::state::DisplaySettingTarget>,
) -> Result<(), CommandError> {
    pool::run(app, move |state| {
        state.save_display_setting_for(made_for.as_ref(), &key, &value)
    })
    .await
}
