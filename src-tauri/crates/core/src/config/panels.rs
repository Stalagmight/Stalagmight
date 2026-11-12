//! The `[panels]` config table: panel sizes, reveal state, and the persisted
//! column layout for each of the app's four shot grids (data table, stations
//! table, new-section dialog, edit-section dialog).
//!
//! Split out of `types.rs` because the four grids' order/hidden/width triples
//! pushed that file past its size budget (#157). Everything here is reachable
//! through `config::*` exactly as before - `mod.rs` re-exports this module the
//! same way it re-exports `types`.

use super::column_widths::{
    lenient_column_widths, normalize_column_names, normalize_column_widths,
};
use super::types::default_true;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use ts_rs::TS;

/// Panel sizes configuration (persisted across sessions)
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct PanelsConfig {
    /// Sections panel width in pixels
    pub sections_panel_width: u32,
    /// Detail panel width in pixels
    pub detail_panel_width: u32,
    /// Data table height in pixels
    pub data_table_height: u32,
    /// Width in pixels of the import wizards' map column (#404).
    ///
    /// ONE value for every wizard that shows one - the mNemo wizard's two steps
    /// and the Ariane import dialog - so the map does not change size as the
    /// user moves between them or between formats. It is the panel's only
    /// persisted state: unlike the three above it has no open/hover mode, since
    /// the map column is always shown while its step is.
    ///
    /// `serde(default)` rather than a required field, so a config written by an
    /// older build loads unchanged and picks up the default here.
    #[serde(default = "default_wizard_map_panel_width")]
    pub wizard_map_panel_width: u32,
    /// Shots-table column order (grid column keys; empty = declaration order)
    #[serde(default)]
    pub data_table_shot_column_order: Vec<String>,
    /// Shots-table hidden column keys (the raw id columns and the survey
    /// credit columns start hidden)
    #[serde(default = "default_data_table_hidden_columns")]
    pub data_table_shot_hidden_columns: Vec<String>,
    /// The [`SHOT_COLUMNS_HIDDEN_ON_ARRIVAL`] this config has already been
    /// given their hidden default for: the shots-table twin of
    /// `data_table_station_columns_seen`. The hidden list is authoritative once
    /// persisted (an empty one means "show all"), so a column added after a
    /// user's list was written would otherwise show up for them; load hides
    /// each such column ONCE and records it here, so a user who later shows it
    /// keeps it shown. Missing in configs written before it existed.
    #[serde(default)]
    pub data_table_shot_columns_seen: Vec<String>,
    /// Shots-table column widths in pixels, keyed by grid column key. A missing
    /// key means "no override", so the column keeps its declared width. BTreeMap
    /// rather than HashMap so the serialized TOML key order is stable: this is
    /// rewritten on every column resize, and a nondeterministic order would
    /// reshuffle the config file on each save.
    ///
    /// Deserialized via [`lenient_column_widths`] rather than the plain derive:
    /// `#[serde(default)]` only covers an ABSENT table, so a table that is
    /// present with one unusable value would fail the whole config parse, and
    /// `Config::load().unwrap_or_default()` then silently resets every OTHER
    /// setting because one column width was typo'd.
    ///
    /// Deliberately NOT `skip_serializing_if` empty, tempting though it is to keep
    /// two empty tables out of a default config file: the generated
    /// `defaultConfig.ts` is produced by serializing `Config::default()`, and
    /// skipping the field there drops a key the generated `PanelsConfig` type
    /// still requires.
    #[serde(default, deserialize_with = "lenient_column_widths")]
    pub data_table_shot_column_widths: BTreeMap<String, u32>,
    /// Shots-table azimuth display: "avg" | "inout" | "off"
    #[serde(default = "default_azimuth_mode")]
    pub data_table_azimuth_mode: String,
    /// Shots-table inclination display: "avg" | "inout" | "off"
    #[serde(default = "default_inclination_mode")]
    pub data_table_inclination_mode: String,
    /// Stations-table column order (grid column keys; empty = declaration order)
    #[serde(default)]
    pub data_table_station_column_order: Vec<String>,
    /// Stations-table hidden column keys (both coordinate pairs and the station
    /// name start hidden)
    #[serde(default = "default_station_hidden_columns")]
    pub data_table_station_hidden_columns: Vec<String>,
    /// The [`STATION_COLUMNS_HIDDEN_ON_ARRIVAL`] this config has already been
    /// given their hidden default for. The hidden list is authoritative once
    /// persisted (an empty one means "show all"), so a column added after a
    /// user's list was written would otherwise show up for them; load hides
    /// each such column ONCE and records it here, so a user who later shows it
    /// keeps it shown. Missing in configs written before it existed.
    #[serde(default)]
    pub data_table_station_columns_seen: Vec<String>,
    /// Stations-table column widths in pixels, keyed by grid column key.
    /// See `data_table_shot_column_widths` for the BTreeMap and the tolerant
    /// deserializer.
    #[serde(default, deserialize_with = "lenient_column_widths")]
    pub data_table_station_column_widths: BTreeMap<String, u32>,
    /// New-section dialog grid: column order (grid column keys; empty =
    /// declaration order). Kept separate from the data table's own layout: the
    /// two grids share column definitions but not a column SET (the dialog omits
    /// the warning/id columns), so one shared list would let a key hidden in one
    /// view silently govern the other.
    #[serde(default)]
    pub new_section_column_order: Vec<String>,
    /// New-section dialog grid: hidden column keys (the in/out legs, pitch and
    /// LRUD start hidden, matching what the old hand-rolled table showed).
    #[serde(default = "default_new_section_hidden_columns")]
    pub new_section_hidden_columns: Vec<String>,
    /// New-section dialog grid: column widths in pixels, keyed by grid column
    /// key. See `data_table_shot_column_widths` for the BTreeMap and the
    /// tolerant deserializer.
    #[serde(default, deserialize_with = "lenient_column_widths")]
    pub new_section_column_widths: BTreeMap<String, u32>,
    /// Edit-section dialog's shot grid: column order (grid column keys; empty =
    /// declaration order). Its own set again, for the same reason as
    /// `new_section_*`: the grids share column definitions but not a column SET.
    #[serde(default)]
    pub edit_section_column_order: Vec<String>,
    /// Edit-section dialog's shot grid: hidden column keys.
    #[serde(default = "default_edit_section_hidden_columns")]
    pub edit_section_hidden_columns: Vec<String>,
    /// Edit-section dialog's shot grid: column widths in pixels, keyed by grid
    /// column key. See `data_table_shot_column_widths` for the BTreeMap and the
    /// tolerant deserializer.
    #[serde(default, deserialize_with = "lenient_column_widths")]
    pub edit_section_column_widths: BTreeMap<String, u32>,
    /// The IMPORT-time shot grids (the mNemo wizard, both of its steps, and
    /// zero-datum config): column order. Its own set because those dialogs edit
    /// shots that are not in the project yet - reshaping columns while placing an
    /// import is a different job from reviewing a saved section, and sharing one
    /// set meant either overwrote the other's layout.
    #[serde(default)]
    pub import_shots_column_order: Vec<String>,
    /// Import-time shot grids: hidden column keys.
    #[serde(default = "default_edit_section_hidden_columns")]
    pub import_shots_hidden_columns: Vec<String>,
    /// Import-time shot grids: column widths in pixels, keyed by grid column key.
    #[serde(default, deserialize_with = "lenient_column_widths")]
    pub import_shots_column_widths: BTreeMap<String, u32>,
    /// Sections panel open (pinned) at startup
    #[serde(default = "default_true")]
    pub sections_panel_open: bool,
    /// Sections panel reveal mode: "pin" | "hover"
    #[serde(default = "default_panel_mode")]
    pub sections_panel_mode: String,
    /// Detail panel open (pinned) at startup
    #[serde(default = "default_true")]
    pub detail_panel_open: bool,
    /// Detail panel reveal mode: "pin" | "hover"
    #[serde(default = "default_panel_mode")]
    pub detail_panel_mode: String,
    /// Data table open (pinned) at startup
    #[serde(default = "default_true")]
    pub data_table_open: bool,
    /// Data table reveal mode: "pin" | "hover"
    #[serde(default = "default_panel_mode")]
    pub data_table_mode: String,
}

/// Default panel reveal mode (pin: click to open, stays open).
fn default_panel_mode() -> String {
    "pin".to_string()
}

/// Default import-wizard map column width. Mirrors
/// `WIZARD_MAP_PANEL_DEFAULT_WIDTH` in
/// src/components/SingleSystemView/layoutConstants.ts.
fn default_wizard_map_panel_width() -> u32 {
    480
}

impl PanelsConfig {
    /// Clamp persisted panel sizes to the same bounds the UI enforces while
    /// dragging, and repair any out-of-range reveal-mode string. The frontend
    /// clamps panel sizes before persisting them (savePanelSizes), but the load
    /// path did not - so a hand-edited, older, or otherwise degenerate config
    /// could load e.g. a 0-pixel width that the frontend then renders as a
    /// collapsed, invisible panel. Bounds mirror PANEL_SIZE_BOUNDS in
    /// src/components/SingleSystemView/layoutConstants.ts. The mirror is
    /// enforced by src/test/__tests__/configBoundsMirror.test.ts, which parses
    /// these clamp calls and compares against the TS constants.
    pub fn normalize(&mut self) {
        self.sections_panel_width = self.sections_panel_width.clamp(150, 400);
        self.detail_panel_width = self.detail_panel_width.clamp(200, 500);
        self.data_table_height = self.data_table_height.clamp(100, 500);
        self.wizard_map_panel_width = self.wizard_map_panel_width.clamp(280, 900);
        normalize_panel_mode(&mut self.sections_panel_mode);
        normalize_panel_mode(&mut self.detail_panel_mode);
        normalize_panel_mode(&mut self.data_table_mode);
        normalize_column_widths(&mut self.data_table_shot_column_widths);
        normalize_column_widths(&mut self.data_table_station_column_widths);
        normalize_column_widths(&mut self.new_section_column_widths);
        normalize_column_widths(&mut self.edit_section_column_widths);
        normalize_column_widths(&mut self.import_shots_column_widths);
        // The eight column-key LISTS need the same ceiling as the width maps
        // beside them, and for the same reason - see `normalize_column_names`.
        // Doing it here rather than in `update_data_table_view_impl` is what
        // makes it hold on BOTH doors: the partial IPC command and the
        // whole-`Config` `update_config` both end in `normalize_except_shortcuts`,
        // and `Config::load` calls it too, so an already-bloated file on disk is
        // pruned on the next launch instead of being carried forever.
        normalize_column_names(&mut self.data_table_shot_column_order);
        normalize_column_names(&mut self.data_table_shot_hidden_columns);
        normalize_column_names(&mut self.data_table_station_column_order);
        normalize_column_names(&mut self.data_table_station_hidden_columns);
        normalize_column_names(&mut self.new_section_column_order);
        normalize_column_names(&mut self.new_section_hidden_columns);
        normalize_column_names(&mut self.edit_section_column_order);
        normalize_column_names(&mut self.edit_section_hidden_columns);
        normalize_column_names(&mut self.import_shots_column_order);
        normalize_column_names(&mut self.import_shots_hidden_columns);
        // Same both-doors argument for the angle display modes. The PARTIAL
        // command validates them and rejects an unknown string, but
        // `update_config` takes a whole `Config` and used to write one through
        // untouched - so `"sideways"` was refused at one door and persisted
        // cleanly through the other, and the load path never repaired it either.
        // Repair rather than reject here: `normalize` has no way to report an
        // error, and the read side (`DataTable`) treats an unknown mode as its
        // own default anyway, so falling back matches what the UI already shows.
        normalize_angle_mode(&mut self.data_table_azimuth_mode, default_azimuth_mode);
        normalize_angle_mode(
            &mut self.data_table_inclination_mode,
            default_inclination_mode,
        );
    }
}

/// The accepted values for `data_table_azimuth_mode` / `data_table_inclination_mode`.
///
/// Shared with `commands::config::update_data_table_view_impl`, which rejects
/// anything outside it, so the validating door and the repairing door cannot
/// drift apart into two different notions of "valid mode".
pub const DATA_TABLE_ANGLE_MODES: [&str; 3] = ["avg", "inout", "off"];

/// Coerce a persisted angle display mode to a known value, falling back to the
/// documented default for that field (they differ: azimuth defaults to "avg",
/// inclination to "off", so the default arrives as a fn rather than a literal).
fn normalize_angle_mode(mode: &mut String, default: fn() -> String) {
    if !DATA_TABLE_ANGLE_MODES.contains(&mode.as_str()) {
        *mode = default();
    }
}

/// Coerce a persisted reveal-mode string to a known value, mirroring the
/// frontend's `asMode` (anything that isn't "hover" becomes the default "pin").
fn normalize_panel_mode(mode: &mut String) {
    if mode != "pin" && mode != "hover" {
        *mode = default_panel_mode();
    }
}

/// Shots-table columns that arrived after users already had a persisted
/// hidden list, and that should start hidden for them too (see
/// `PanelsConfig::data_table_shot_columns_seen`): the per-shot survey credit
/// (surveyors, explorers, survey date). A new column that should start hidden
/// goes here AND into `default_data_table_hidden_columns`.
pub const SHOT_COLUMNS_HIDDEN_ON_ARRIVAL: [&str; 3] = ["surveyors", "explorers", "survey_date"];

pub(super) fn default_shot_columns_seen() -> Vec<String> {
    SHOT_COLUMNS_HIDDEN_ON_ARRIVAL
        .iter()
        .map(|key| (*key).to_string())
        .collect()
}

/// Stations-table columns that arrived after users already had a persisted
/// hidden list, and that should start hidden for them too (see
/// `PanelsConfig::data_table_station_columns_seen`). A new column that should
/// start hidden goes here AND into `default_station_hidden_columns`.
pub const STATION_COLUMNS_HIDDEN_ON_ARRIVAL: [&str; 1] = ["name"];

pub(super) fn default_station_hidden_columns() -> Vec<String> {
    vec![
        "_x".to_string(),
        "_y".to_string(),
        "_lat".to_string(),
        "_long".to_string(),
        "name".to_string(),
    ]
}

pub(super) fn default_station_columns_seen() -> Vec<String> {
    STATION_COLUMNS_HIDDEN_ON_ARRIVAL
        .iter()
        .map(|key| (*key).to_string())
        .collect()
}

impl PanelsConfig {
    /// Load-time migration: hide each [`STATION_COLUMNS_HIDDEN_ON_ARRIVAL`]
    /// column this config has not been given its default for yet, once.
    pub(super) fn hide_newly_arrived_station_columns(&mut self) {
        hide_newly_arrived(
            &STATION_COLUMNS_HIDDEN_ON_ARRIVAL,
            &mut self.data_table_station_hidden_columns,
            &mut self.data_table_station_columns_seen,
        );
    }

    /// Load-time migration: hide each [`SHOT_COLUMNS_HIDDEN_ON_ARRIVAL`]
    /// column this config has not been given its default for yet, once.
    pub(super) fn hide_newly_arrived_shot_columns(&mut self) {
        hide_newly_arrived(
            &SHOT_COLUMNS_HIDDEN_ON_ARRIVAL,
            &mut self.data_table_shot_hidden_columns,
            &mut self.data_table_shot_columns_seen,
        );
    }
}

/// Hide each of `arrived` that `seen` does not list yet (unless `hidden`
/// already does), and record it in `seen` so it is never hidden again.
fn hide_newly_arrived(arrived: &[&str], hidden: &mut Vec<String>, seen: &mut Vec<String>) {
    for key in arrived {
        if seen.iter().any(|k| k == key) {
            continue;
        }
        if !hidden.iter().any(|k| k == key) {
            hidden.push((*key).to_string());
        }
        seen.push((*key).to_string());
    }
}

pub(super) fn default_data_table_hidden_columns() -> Vec<String> {
    let mut hidden = vec!["section_id".to_string(), "shot_id".to_string()];
    hidden.extend(default_shot_columns_seen());
    hidden
}

/// The edit-section grid hides the azimuth legs and the whole inclination family
/// and shows everything else. It is editing a survey that already exists, so
/// there is no "start simple" case the way there is when entering a brand-new
/// section - but those five columns were added to the grid after the fact, and
/// hiding them is what keeps the four dialogs that render it looking exactly as
/// they did before. The averages are what they have always shown.
///
/// Mirrored by `DEFAULT_EDIT_SECTION_HIDDEN_COLUMNS` in ShotTableEditor.tsx.
pub(super) fn default_edit_section_hidden_columns() -> Vec<String> {
    vec![
        "azimuth_in".to_string(),
        "azimuth_out".to_string(),
        "_pitch_avg".to_string(),
        "pitch_in".to_string(),
        "pitch_out".to_string(),
    ]
}

fn default_new_section_hidden_columns() -> Vec<String> {
    vec![
        "azimuth_in".to_string(),
        "azimuth_out".to_string(),
        "_pitch_avg".to_string(),
        "pitch_in".to_string(),
        "pitch_out".to_string(),
        "left".to_string(),
        "right".to_string(),
        "up".to_string(),
        "down".to_string(),
    ]
}

fn default_azimuth_mode() -> String {
    "avg".to_string()
}

fn default_inclination_mode() -> String {
    "off".to_string()
}
