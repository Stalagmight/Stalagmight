use anyhow::{Context, Result};
use serde::{Deserialize, Deserializer, Serialize};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use ts_rs::TS;

use super::panels::PanelsConfig;
use super::webkit::WebkitMemoryConfig;
use crate::models::config::{
    ClosureAdjustmentConfig, GasConfig, PlacementSuggestionConfig, ValidationConfig,
};
use crate::speleodb::config::SpeleoDbConfig;

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct Config {
    pub general: GeneralConfig,
    pub units: UnitsConfig,
    pub display: DisplayConfig,
    pub section_colors: SectionColorsConfig,
    pub map: MapConfig,
    pub validation: ValidationConfig,
    #[serde(default)]
    pub closure_adjustment: ClosureAdjustmentConfig,
    pub export: ExportConfig,
    pub logging: LoggingConfig,
    pub shortcuts: ShortcutsConfig,
    #[serde(default)]
    pub panels: PanelsConfig,
    #[serde(default)]
    pub cache: CacheConfig,
    #[serde(default)]
    pub debug: DebugConfig,
    /// Linux-only WebKit memory-pressure tuning. Backend-only: `#[ts(skip)]`
    /// keeps it out of the generated frontend types, and `#[serde(skip_serializing)]`
    /// keeps it out of both `get_config`'s payload and the generated
    /// `defaultConfig.ts` (which is typed as the frontend `Config`, so an emitted
    /// `webkit_memory` block would fail `tsc`). Because `skip_serializing` also
    /// drops it from `toml::to_string_pretty`, saving would otherwise wipe the
    /// `[webkit_memory]` section from disk; `Config::save_to_path` therefore
    /// re-appends the table (when non-default) after serializing, so a hand-tuned
    /// knob in `config.toml` survives a save. The re-appended value is the one
    /// `AppState::update_config` preserves in memory across the session, so saving
    /// any setting can never reset the knob to defaults.
    #[serde(default, skip_serializing)]
    #[ts(skip)]
    pub webkit_memory: WebkitMemoryConfig,
    #[serde(default)]
    pub global_view: GlobalViewConfig,
    #[serde(default)]
    pub appearance: AppearanceConfig,
    #[serde(default)]
    pub import: ImportConfig,
    #[serde(default)]
    pub placement_suggestion: PlacementSuggestionConfig,
    #[serde(default)]
    pub syntactic_highlighting: SyntacticHighlightingConfig,
    #[serde(default)]
    pub gas: GasConfig,
    #[serde(default)]
    pub speleodb: SpeleoDbConfig,

    // Legacy fields for backwards compatibility (deserialize only, never serialize)
    // Uses alias to match old TOML key name while keeping internal field name distinct
    #[serde(default, skip_serializing, alias = "snap")]
    #[ts(skip)]
    pub(super) legacy_snap: Option<LegacySnapConfig>,
}

/// Legacy snap config for migration from old [snap] section
#[derive(Debug, Clone, Serialize, Deserialize, Default, TS)]
pub(super) struct LegacySnapConfig {
    #[serde(default)]
    enabled_merge: Option<bool>,
    #[serde(default)]
    enabled_measure: Option<bool>,
    /// Old threshold was u32, but accept f64 for forward compatibility
    #[serde(default)]
    threshold_pixels: Option<f64>,
}

/// Station visual and interaction configuration
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct StationConfig {
    /// Visual radius in pixels (1.0-50.0)
    #[serde(default = "default_station_radius")]
    pub radius: f64,
    /// Clickable area radius in pixels (1.0-50.0, >= radius)
    #[serde(default = "default_station_hit_area")]
    pub hit_area: f64,
    /// Radius when hovered (1.0-50.0, typically > radius)
    #[serde(default = "default_station_hover_radius")]
    pub hover_radius: f64,
    /// Radius when selected (1.0-50.0, typically > radius)
    #[serde(default = "default_station_selection_radius")]
    pub selection_radius: f64,
}

impl StationConfig {
    pub fn normalize(&mut self) {
        if !self.radius.is_finite() {
            self.radius = default_station_radius();
        }
        if !self.hit_area.is_finite() {
            self.hit_area = default_station_hit_area();
        }
        if !self.hover_radius.is_finite() {
            self.hover_radius = default_station_hover_radius();
        }
        if !self.selection_radius.is_finite() {
            self.selection_radius = default_station_selection_radius();
        }
        self.radius = self.radius.clamp(1.0, 50.0);
        self.hit_area = self.hit_area.clamp(1.0, 50.0).max(self.radius);
        self.hover_radius = self.hover_radius.clamp(1.0, 50.0);
        self.selection_radius = self.selection_radius.clamp(1.0, 50.0);
    }
}

impl Default for StationConfig {
    fn default() -> Self {
        Self {
            radius: default_station_radius(),
            hit_area: default_station_hit_area(),
            hover_radius: default_station_hover_radius(),
            selection_radius: default_station_selection_radius(),
        }
    }
}

fn default_station_radius() -> f64 {
    4.0
}

fn default_station_hit_area() -> f64 {
    10.0
}

fn default_station_hover_radius() -> f64 {
    6.0
}

fn default_station_selection_radius() -> f64 {
    5.0
}

/// Shot visual and interaction configuration
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ShotConfig {
    /// Visual line width in pixels (0.5-20.0)
    #[serde(default = "default_shot_width")]
    pub width: f64,
    /// Clickable area width in pixels (0.5-20.0, >= width)
    #[serde(default = "default_shot_hit_area")]
    pub hit_area: f64,
    /// Width when hovered (0.5-20.0, typically > width)
    #[serde(default = "default_shot_hover_width")]
    pub hover_width: f64,
    /// Width when selected (0.5-20.0, typically > width)
    #[serde(default = "default_shot_selection_width")]
    pub selection_width: f64,
}

impl ShotConfig {
    pub fn normalize(&mut self) {
        if !self.width.is_finite() {
            self.width = default_shot_width();
        }
        if !self.hit_area.is_finite() {
            self.hit_area = default_shot_hit_area();
        }
        if !self.hover_width.is_finite() {
            self.hover_width = default_shot_hover_width();
        }
        if !self.selection_width.is_finite() {
            self.selection_width = default_shot_selection_width();
        }
        self.width = self.width.clamp(0.5, 20.0);
        self.hit_area = self.hit_area.clamp(0.5, 20.0).max(self.width);
        self.hover_width = self.hover_width.clamp(0.5, 20.0);
        self.selection_width = self.selection_width.clamp(0.5, 20.0);
    }
}

impl Default for ShotConfig {
    fn default() -> Self {
        Self {
            width: default_shot_width(),
            hit_area: default_shot_hit_area(),
            hover_width: default_shot_hover_width(),
            selection_width: default_shot_selection_width(),
        }
    }
}

fn default_shot_width() -> f64 {
    2.0
}

fn default_shot_hit_area() -> f64 {
    8.0
}

fn default_shot_hover_width() -> f64 {
    4.0
}

fn default_shot_selection_width() -> f64 {
    3.0
}

/// Snap-to-station configuration for precision tools
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct MapSnapConfig {
    /// Snap threshold in pixels (1.0-50.0)
    #[serde(default = "default_snap_threshold")]
    pub threshold: f64,
    /// Enable snap for merge tool
    #[serde(default = "default_true")]
    pub enabled_merge: bool,
    /// Enable snap for measure tool
    #[serde(default = "default_true")]
    pub enabled_measure: bool,
}

impl MapSnapConfig {
    pub fn normalize(&mut self) {
        if !self.threshold.is_finite() {
            self.threshold = default_snap_threshold();
        }
        self.threshold = self.threshold.clamp(1.0, 50.0);
    }
}

impl Default for MapSnapConfig {
    fn default() -> Self {
        Self {
            threshold: default_snap_threshold(),
            enabled_merge: true,
            enabled_measure: true,
        }
    }
}

fn default_snap_threshold() -> f64 {
    10.0
}

pub(super) fn default_true() -> bool {
    true
}

/// Animation configuration for map focus transitions
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct MapAnimationConfig {
    /// Whether animations are enabled
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Animation duration in milliseconds
    #[serde(default = "default_animation_duration")]
    pub duration: u32,
    /// Zoom level for single-point focus
    #[serde(default = "default_point_zoom")]
    pub point_zoom: u8,
}

impl Default for MapAnimationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            duration: default_animation_duration(),
            point_zoom: default_point_zoom(),
        }
    }
}

fn default_animation_duration() -> u32 {
    300
}

fn default_point_zoom() -> u8 {
    18
}

/// Debug configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default, TS)]
pub struct DebugConfig {
    #[serde(default)]
    pub map: MapDebugConfig,
    /// Show the on-screen memory probe overlay (Ctrl+Shift+M toggles).
    /// Polls /proc for backend + WebKit child RSS and shows in-process
    /// counters for the JS heap retention sinks (geometryData, MapLibre
    /// feature counts, etc).
    #[serde(default)]
    pub memory_probe: bool,
}

/// Map-specific debug options
#[derive(Debug, Clone, Serialize, Deserialize, Default, TS)]
pub struct MapDebugConfig {
    /// Show hitbox visualization for debugging
    #[serde(default)]
    pub show_hitboxes: bool,
    /// Log hit detection events to console
    #[serde(default)]
    pub log_hit_detection: bool,
}

/// Per-provider cache override
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ProviderCacheConfig {
    /// Retention TTL for this provider's tiles, in days. Falls back to the
    /// global `tile_ttl_days` when unset.
    ///
    /// This bounds how long a tile stays on disk. It does NOT extend how long
    /// one may be served as current: that is additionally capped by the
    /// provider's own `Cache-Control: max-age` (see `crate::cache::tiles`), so
    /// raising this only lengthens how long a stored copy remains available to
    /// answer a request the network could not - never staler "fresh" tiles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_days: Option<u32>,
}

/// HTTP cache configuration for map tiles.
///
/// Imagery is never stored for offline use, or for any purpose of our own. It is
/// cached strictly under the provider's own `Cache-Control`: stored only where
/// the response grants it, for no longer than it grants, and revalidated with
/// the provider's own validator after that.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct CacheConfig {
    /// Whether the tile cache may be used at all. Whether any individual tile
    /// is stored remains the provider's decision, via its `Cache-Control`.
    #[serde(default = "default_tile_cache_enabled")]
    pub tile_cache_enabled: bool,
    /// Maximum cache size in megabytes
    #[serde(default = "default_tile_cache_max_mb")]
    pub tile_cache_max_mb: u32,
    /// Time-to-live for cached tiles in days
    #[serde(default = "default_tile_ttl_days")]
    pub tile_ttl_days: u32,
    /// Per-provider cache overrides (providers not listed default to enabled)
    #[serde(default)]
    pub providers: HashMap<String, ProviderCacheConfig>,
}

impl CacheConfig {
    /// Per-provider retention TTLs in days, for every provider that overrides
    /// the global one. Providers absent from the map use `tile_ttl_days`.
    pub fn provider_ttl_days(&self) -> HashMap<String, u32> {
        self.providers
            .iter()
            .filter_map(|(name, cfg)| cfg.ttl_days.map(|days| (name.clone(), days)))
            .collect()
    }
}

fn default_tile_cache_enabled() -> bool {
    true
}

fn default_tile_cache_max_mb() -> u32 {
    500
}

fn default_tile_ttl_days() -> u32 {
    30
}

/// Global view configuration for multi-system map rendering
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct GlobalViewConfig {
    /// FPS threshold below which auto-throttle activates
    #[serde(default = "default_fps_threshold")]
    pub fps_threshold: u32,
    /// GeoJSON simplification tolerance at normal FPS
    #[serde(default = "default_tolerance")]
    pub tolerance: f64,
    /// GeoJSON simplification tolerance when throttled (higher = more simplification)
    #[serde(default = "default_throttle_tolerance")]
    pub throttle_tolerance: f64,
}

impl GlobalViewConfig {
    /// Normalize values to valid ranges
    pub fn normalize(&mut self) {
        if !self.tolerance.is_finite() {
            self.tolerance = default_tolerance();
        }
        if !self.throttle_tolerance.is_finite() {
            self.throttle_tolerance = default_throttle_tolerance();
        }
        // Ensure tolerance values are positive
        self.tolerance = self.tolerance.max(0.0);
        self.throttle_tolerance = self.throttle_tolerance.max(0.0);
        // Ensure throttle_tolerance >= tolerance
        if self.throttle_tolerance < self.tolerance {
            self.throttle_tolerance = self.tolerance;
        }
    }
}

impl Default for GlobalViewConfig {
    fn default() -> Self {
        Self {
            fps_threshold: default_fps_threshold(),
            tolerance: default_tolerance(),
            throttle_tolerance: default_throttle_tolerance(),
        }
    }
}

/// Theme mode for the application.
/// Uses `#[serde(other)]` on Unknown variant so that invalid/unrecognized
/// theme values in TOML don't prevent config loading; they are coerced to
/// Dark during normalize().
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, TS)]
#[serde(rename_all = "lowercase")]
#[ts(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
    Auto,
    /// Catch-all for unrecognized theme values; normalized to Dark
    #[serde(other, rename = "unknown")]
    Unknown,
}

/// Color theme (palette) for a given mode.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ColorTheme {
    Dark,
    Light,
    Cave,
    Dracula,
    OneDark,
    SolarizedLight,
    CatppuccinLatte,
    Nord,
    CatppuccinMocha,
    KaleidoEink,
}

/// Appearance configuration
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct AppearanceConfig {
    #[serde(default)]
    pub theme: ThemeMode,
    /// Color palette used when day (light) mode is active
    #[serde(default = "default_day_color_theme")]
    pub day_color_theme: ColorTheme,
    /// Color palette used when night (dark) mode is active
    #[serde(default = "default_night_color_theme")]
    pub night_color_theme: ColorTheme,
}

fn default_day_color_theme() -> ColorTheme {
    ColorTheme::Light
}

fn default_night_color_theme() -> ColorTheme {
    ColorTheme::Cave
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            theme: ThemeMode::default(),
            day_color_theme: default_day_color_theme(),
            night_color_theme: default_night_color_theme(),
        }
    }
}

impl AppearanceConfig {
    /// Normalize appearance values: invalid/unknown theme resets to "dark"
    pub fn normalize(&mut self) {
        if self.theme == ThemeMode::Unknown {
            tracing::warn!("Unknown theme value in config, normalizing to \"dark\"");
            self.theme = ThemeMode::Dark;
        }
    }
}

/// Syntactic-highlighting level. Controls how far the color system reaches.
/// - `Off`: monochrome toolbar, no entity coloring anywhere.
/// - `ToolbarOnly`: color the toolbar icons only; rest of the app unchanged.
/// - `Full`: toolbar + entity blobs in tables and detail-panel links.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum SyntacticHighlightLevel {
    Off,
    ToolbarOnly,
    /// Toolbar coloring plus action icons in menus (e.g. context menus).
    Icons,
    /// Opt-out default: full highlighting is on for new users.
    #[default]
    Full,
    /// Catch-all for unrecognized values; normalized to Full.
    #[serde(other, rename = "unknown")]
    Unknown,
}

/// Syntactic-highlighting color preset. `Default` follows the spec palette;
/// the others are colorblind-safe (CVD) palettes tuned per deficiency.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum SyntacticHighlightPreset {
    #[default]
    Default,
    Deuteranopia,
    Protanopia,
    Tritanopia,
    /// Catch-all for unrecognized values; normalized to Default.
    #[serde(other, rename = "unknown")]
    Unknown,
}

/// Syntactic-highlighting configuration.
///
/// `overrides` is a sparse map of `preset id -> (role id -> #rrggbb)`. The base
/// palette for each preset lives in the frontend; only user-customized colors are
/// stored here, so the resolved color for a role is `overrides[preset][role]`
/// falling back to the built-in preset value.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SyntacticHighlightingConfig {
    #[serde(default)]
    pub level: SyntacticHighlightLevel,
    #[serde(default)]
    pub preset: SyntacticHighlightPreset,
    #[serde(default)]
    pub overrides: HashMap<String, HashMap<String, String>>,
}

impl Default for SyntacticHighlightingConfig {
    fn default() -> Self {
        Self {
            level: SyntacticHighlightLevel::default(),
            preset: SyntacticHighlightPreset::default(),
            overrides: HashMap::new(),
        }
    }
}

impl SyntacticHighlightingConfig {
    /// Normalize: coerce unknown enum values to defaults and drop any override
    /// entries whose value is not a valid `#rrggbb` hex color.
    pub fn normalize(&mut self) {
        if self.level == SyntacticHighlightLevel::Unknown {
            self.level = SyntacticHighlightLevel::Full;
        }
        if self.preset == SyntacticHighlightPreset::Unknown {
            self.preset = SyntacticHighlightPreset::Default;
        }
        for roles in self.overrides.values_mut() {
            roles.retain(|_role, color| is_valid_hex_color(color));
        }
        self.overrides.retain(|_preset, roles| !roles.is_empty());
    }
}

/// Whether a string is a `#rrggbb` hex color (case-insensitive).
fn is_valid_hex_color(s: &str) -> bool {
    let bytes = s.as_bytes();
    bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(|b| b.is_ascii_hexdigit())
}

/// How overlap shots are handled during import connection
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../bindings/")]
pub enum OverlapMode {
    #[default]
    ImportData,
    ExistingData,
    Interpolate,
    KeepBoth,
}

/// Import-related configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct ImportConfig {
    #[serde(default)]
    pub overlap_mode: OverlapMode,
}

fn default_fps_threshold() -> u32 {
    30
}

fn default_tolerance() -> f64 {
    0.5
}

fn default_throttle_tolerance() -> f64 {
    1.0
}

/// Shortcut feedback preference for toast notifications
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ShortcutFeedback {
    /// No toasts for any shortcuts
    None,
    /// Toast for view/panel toggles only (default)
    #[default]
    NonObvious,
    /// Toast for all shortcut actions
    All,
}

/// Which release track the in-app update check follows.
///
/// The `Default` is a property of the BUILD, not of the user: CI compiles a
/// release candidate with `STALAGMIGHT_CHANNEL=beta` set, so a candidate install
/// follows the beta manifest out of the box while an ordinary stable build -
/// which has no such variable at compile time - follows the stable one.
/// `option_env!` is read when this crate is compiled, so flipping the variable
/// on an installed app does nothing; the user's own choice lives in the config
/// field and overrides the build default from the first launch onwards.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum UpdateTrack {
    /// Final releases only.
    Stable,
    /// Release candidates of the next version, then its final release (#650).
    ///
    /// Read from `weekly` too: 0.5.0 and 0.5.1 saved that for the track this
    /// replaces, and a config that stopped loading would lose every other
    /// setting with it. Always written back as `beta`.
    #[serde(alias = "weekly")]
    Beta,
}

impl Default for UpdateTrack {
    fn default() -> Self {
        match option_env!("STALAGMIGHT_CHANNEL") {
            Some("beta" | "weekly") => Self::Beta,
            _ => Self::Stable,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct GeneralConfig {
    pub auto_save: bool,
    pub auto_save_interval_seconds: u32,
    pub recent_projects_max: usize,
    #[serde(default)]
    #[ts(type = "string[]")]
    pub recent_projects: Vec<PathBuf>,
    /// Maximum undo history entries to keep in memory (default: 100)
    #[serde(default = "default_undo_history_limit")]
    pub undo_history_limit: usize,
    /// Maximum undo entries to persist to project file (default: 20)
    #[serde(default = "default_undo_persist_limit")]
    pub undo_persist_limit: usize,
    /// Shortcut feedback preference: none, non_obvious (default), or all
    #[serde(default)]
    pub shortcut_feedback: ShortcutFeedback,
    /// Locale setting: 'system' or a specific locale tag (e.g., 'en-US')
    #[serde(default = "default_locale")]
    pub locale: String,
    /// Number format override: 'locale' (follow language setting) or a locale tag (e.g., 'de-DE')
    #[serde(default = "default_format_locale")]
    pub number_format: String,
    /// Date format override: 'locale' (follow language setting) or a locale tag (e.g., 'en-GB')
    #[serde(default = "default_format_locale")]
    pub date_format: String,
    /// Show splash screen with branding on startup
    #[serde(default)]
    pub splash_screen: bool,
    /// Global onboarding master switch
    #[serde(default = "default_onboarding_enabled")]
    pub onboarding_enabled: bool,
    /// Onboarding flow IDs already completed/skipped by the user
    #[serde(default)]
    pub onboarding_completed_flows: Vec<String>,
    /// Burned once the one-shot orientation tour (`workspace-basics`) is
    /// actually on screen. It gates that tour only - the contextual guides
    /// repeat-suppress per flow via `onboarding_completed_flows`.
    /// "Restart onboarding" in Settings clears it to re-arm.
    #[serde(default)]
    pub onboarding_first_launch_done: bool,
    /// Whether anonymous telemetry is enabled
    #[serde(default)]
    pub telemetry_enabled: bool,
    /// Whether the telemetry opt-in dialog has been shown
    #[serde(default)]
    pub telemetry_opt_in_shown: bool,
    /// Release track the update check follows: stable (default) or beta
    #[serde(default)]
    pub update_track: UpdateTrack,
    /// Whether the AUTOMATIC launch-time update check runs. On by default.
    ///
    /// Read by the frontend's launch hook only - `check_for_app_update` does
    /// not consult it, deliberately: the About dialog's "Check for updates"
    /// button is an explicit request by the user, and a backend that refused
    /// would break that button along with the background check this setting is
    /// actually about.
    #[serde(default = "default_update_check_enabled")]
    pub update_check_enabled: bool,
}

pub(super) fn default_locale() -> String {
    "system".to_string()
}

pub(super) fn default_format_locale() -> String {
    "locale".to_string()
}

fn default_onboarding_enabled() -> bool {
    true
}

fn default_update_check_enabled() -> bool {
    true
}

fn default_undo_history_limit() -> usize {
    100
}

fn default_undo_persist_limit() -> usize {
    20
}

/// Per-category decimal precision settings.
/// Each field controls the number of decimal places for a specific measurement category.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DecimalPrecision {
    /// Shot lengths, path lengths, misclosure distances, LRUD
    #[serde(default = "default_precision_length")]
    pub length: u8,
    /// Station X, Y positions
    #[serde(default = "default_precision_coordinates")]
    pub coordinates: u8,
    /// Z coordinate, depth values
    #[serde(default = "default_precision_depth")]
    pub depth: u8,
    /// Compass bearings / azimuths
    #[serde(default = "default_precision_azimuth")]
    pub azimuth: u8,
    /// Pitch / dip angles
    #[serde(default = "default_precision_inclination")]
    pub inclination: u8,
    /// Latitude, longitude
    #[serde(default = "default_precision_gps")]
    pub gps: u8,
}

impl DecimalPrecision {
    /// Clamp all fields to the valid range 0-6
    pub fn normalize(&mut self) {
        self.length = self.length.min(6);
        self.coordinates = self.coordinates.min(6);
        self.depth = self.depth.min(6);
        self.azimuth = self.azimuth.min(6);
        self.inclination = self.inclination.min(6);
        self.gps = self.gps.min(6);
    }
}

fn default_precision_length() -> u8 {
    1
}
fn default_precision_coordinates() -> u8 {
    2
}
fn default_precision_depth() -> u8 {
    1
}
fn default_precision_azimuth() -> u8 {
    1
}
fn default_precision_inclination() -> u8 {
    1
}
fn default_precision_gps() -> u8 {
    6
}

/// Custom deserializer for backward compatibility: accepts either a bare integer
/// (old `decimal_precision = 1` format) or the new struct format.
fn deserialize_decimal_precision<'de, D>(deserializer: D) -> Result<DecimalPrecision, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum DecimalPrecisionOrU8 {
        Struct(DecimalPrecision),
        Bare(u8),
    }

    match DecimalPrecisionOrU8::deserialize(deserializer)? {
        DecimalPrecisionOrU8::Struct(dp) => Ok(dp),
        DecimalPrecisionOrU8::Bare(v) => Ok(DecimalPrecision {
            length: v,
            coordinates: v,
            depth: v,
            azimuth: v,
            inclination: v,
            gps: v,
        }),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct UnitsConfig {
    pub length: LengthUnit,
    pub depth: LengthUnit,
    pub angle: AngleUnit,
    #[serde(deserialize_with = "deserialize_decimal_precision")]
    pub decimal_precision: DecimalPrecision,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "lowercase")]
#[ts(rename_all = "lowercase")]
pub enum LengthUnit {
    Meters,
    Feet,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[serde(rename_all = "lowercase")]
#[ts(rename_all = "lowercase")]
pub enum AngleUnit {
    Degrees,
    Gradians,
}

/// Map-drawing defaults.
///
/// `default_background` used to live here (#357 item 10). It was removed rather
/// than wired: it had no renderer consumer anywhere in `src` or `src-tauri`, and
/// wiring it back would have undone #332 / PR #361, which made the map canvas
/// theme-driven off `--cc-map-background`. A single global background hex cannot
/// be right for all ten themes at once - the shipped default was the dark
/// `#1a1a2e`, so honouring it would have repainted every light theme's canvas
/// dark, which is the exact bug #332 fixed.
///
/// Removal is load-path safe: neither `Config` nor `DisplayConfig` sets
/// `deny_unknown_fields`, so a `default_background = "..."` line left in an
/// existing `config.toml` is ignored. Seven `[display]` fixtures in `tests.rs`
/// still carry that line and still parse, which is the regression coverage.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DisplayConfig {
    pub grid_color: String,
    /// Grid spacing in meters (e.g., 1, 5, 10, 25, 50)
    #[serde(default = "default_grid_spacing")]
    pub grid_spacing: f64,
    pub font_size: u32,
    pub font_color: String,
    pub depth_gradient: DepthGradientConfig,
    /// Highlight color for the ruler's chosen along-cave-line path (#26), applied
    /// to both the path's shots and its stations. `#rrggbb`.
    #[serde(default = "default_measure_path_color")]
    pub measure_path_color: String,
}

fn default_grid_spacing() -> f64 {
    10.0
}

fn default_measure_path_color() -> String {
    "#22d3ee".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct DepthGradientConfig {
    pub enabled: bool,
    pub shallow_color: String,
    pub deep_color: String,
    pub shallow_depth: f64,
    pub deep_depth: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SectionColorsConfig {
    pub palette: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct GpsOrigin {
    pub latitude: f64,
    pub longitude: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct MapConfig {
    pub satellite_provider: String,
    pub satellite_enabled: bool,
    /// GPS origin for satellite tile alignment
    #[serde(default = "default_satellite_gps_origin")]
    pub satellite_gps_origin: GpsOrigin,
    pub zoom_default: u32,
    pub zoom_min: u32,
    pub zoom_max: u32,
    /// Zoom step for click-to-zoom (default: 1.0, range: 0.1-5.0)
    #[serde(default = "default_zoom_step")]
    pub zoom_step: f64,
    pub controls: MapControlsConfig,
    /// Station visual and interaction settings
    #[serde(default)]
    pub station: StationConfig,
    /// Shot visual and interaction settings
    #[serde(default)]
    pub shot: ShotConfig,
    /// Snap-to-station settings
    #[serde(default)]
    pub snap: MapSnapConfig,
    /// Animation settings for map focus transitions
    #[serde(default)]
    pub animation: MapAnimationConfig,
    /// When true, switching the detail panel to the Warnings tab auto-enables the
    /// map's Issue-severity coloring (restored on leaving the tab).
    #[serde(default = "default_true")]
    pub highlight_warnings_on_tab_switch: bool,
}

impl MapConfig {
    /// Normalize zoom_step and nested config values (station, shot, snap) to valid ranges
    /// Handles NaN/inf by falling back to defaults
    /// Normalizes unknown satellite_provider to "esri" with a logged warning
    pub fn normalize(&mut self) {
        // Normalize satellite_provider: unknown providers fall back to "esri"
        if crate::tile_providers::get_provider(&self.satellite_provider).is_none() {
            tracing::warn!(
                provider = %self.satellite_provider,
                "Unknown satellite_provider in config, normalizing to \"esri\""
            );
            self.satellite_provider = "esri".to_string();
        }

        if self.zoom_step.is_finite() {
            self.zoom_step = self.zoom_step.clamp(0.1, 5.0);
        } else {
            self.zoom_step = default_zoom_step();
        }
        self.controls.normalize();
        self.station.normalize();
        self.shot.normalize();
        self.snap.normalize();
    }
}

fn default_zoom_step() -> f64 {
    1.0
}

fn default_satellite_gps_origin() -> GpsOrigin {
    GpsOrigin {
        latitude: 24.55,
        longitude: -81.78,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct MapControlsConfig {
    pub pan_speed: f64,
    /// Zoom LEVELS moved by one 100 px wheel notch, in
    /// [`ZOOM_SPEED_LEVELS_MIN`]..=[`ZOOM_SPEED_LEVELS_MAX`] (#429). Files
    /// written before the remap hold the old multiplier instead; see
    /// `Config::migrate_legacy`.
    pub zoom_speed: f64,
    pub invert_scroll: bool,
    /// Show the Pan/Zoom toolbar group (#409). Off by default: the map pans on a
    /// plain drag and zooms on the wheel under every tool, so the two navigation
    /// tools are redundant. Kept as an opt-in accessibility fallback for users who
    /// cannot drag or scroll. `serde(default)` so config files written before the
    /// field existed keep parsing.
    #[serde(default)]
    pub show_navigation_tools: bool,
}

/// Slowest `zoom_speed` the sigmoid delivers usefully. Mirrors
/// `ZOOM_LEVELS_PER_NOTCH_MIN` in `src/utils/zoomRate.ts`.
pub const ZOOM_SPEED_LEVELS_MIN: f64 = 0.1;

/// Fastest `zoom_speed` the settings dialog offers, and the ceiling the
/// frontend clamps to. Mirrors `ZOOM_LEVELS_PER_NOTCH_MAX` in
/// `src/utils/zoomRate.ts`.
pub const ZOOM_SPEED_LEVELS_MAX: f64 = 0.95;

impl MapControlsConfig {
    /// Repair `zoom_speed` to the range the wheel-zoom solve can deliver.
    ///
    /// Runs on the IPC write path as well as on load, so it only ever CLAMPS -
    /// the settings dialog's number input does not enforce its own `max`, and
    /// an out-of-range figure typed there has to come back as the nearest one
    /// the range holds, not as some other number. Reading a stale multiplier as
    /// levels is a load-time migration instead (`Config::migrate_legacy`).
    pub fn normalize(&mut self) {
        if self.zoom_speed.is_finite() {
            self.zoom_speed = self
                .zoom_speed
                .clamp(ZOOM_SPEED_LEVELS_MIN, ZOOM_SPEED_LEVELS_MAX);
        } else {
            self.zoom_speed = default_zoom_speed();
        }
    }
}

/// The shipped `zoom_speed`. Pinned to `MapControlsConfig::default()` by
/// `test_map_controls_normalize_non_finite_resets_to_default`.
fn default_zoom_speed() -> f64 {
    0.55
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct ExportConfig {
    pub default_image_format: String,
    pub default_dpi: u32,
    pub default_include_satellite: bool,
    pub default_include_grid: bool,
    pub default_include_labels: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct LoggingConfig {
    pub level: String,
    pub max_directory_size_mb: u32,
}

/// Maximum number of bindings per action
const MAX_BINDINGS_PER_ACTION: usize = 2;

/// Keyboard shortcuts configuration
/// Supports multiple bindings per action (max 2) with backward-compatible deserialization
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct ShortcutsConfig {
    // Tools
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub select: Vec<String>,
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub pan: Vec<String>,
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub zoom: Vec<String>,
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub merge: Vec<String>,
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub measure: Vec<String>,
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub move_station: Vec<String>,
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub move_segment: Vec<String>,
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub move_shot: Vec<String>,
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub comment_tool: Vec<String>,
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub move_comment_tool: Vec<String>,

    // Edit
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub undo: Vec<String>,
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub redo: Vec<String>,

    // View
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub toggle_depth_gradient: Vec<String>,
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub toggle_grid: Vec<String>,
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub toggle_satellite: Vec<String>,
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub toggle_station_labels: Vec<String>,
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub toggle_shot_labels: Vec<String>,
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub toggle_section_labels: Vec<String>,
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub fit_to_bounds: Vec<String>,
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub zoom_in: Vec<String>,
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub zoom_out: Vec<String>,

    // Panels
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub toggle_sections_panel: Vec<String>,
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub toggle_detail_panel: Vec<String>,
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub toggle_data_table: Vec<String>,

    // File
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub save: Vec<String>,
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub new_file: Vec<String>,
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub open_file: Vec<String>,
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub save_as: Vec<String>,
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub import_dmp: Vec<String>,
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub export: Vec<String>,
    #[serde(deserialize_with = "deserialize_shortcut_vec")]
    pub quit: Vec<String>,
}

impl ShortcutsConfig {
    /// Normalize all shortcuts to canonical form
    /// - Sorts modifiers: Ctrl -> Alt -> Shift -> Key
    /// - Maps special keys to canonical names
    /// - Truncates to max 2 bindings per action
    /// - Fills missing fields from defaults
    /// - Logs warnings for invalid shortcuts
    pub fn normalize(&mut self) {
        let defaults = ShortcutsConfig::default();

        // Migrate shortcut reassignments from pre-move-segment era:
        // G was toggle_depth_gradient, now it's move_shot; toggle_depth_gradient moves to Shift+D
        if self.move_shot.is_empty() && self.toggle_depth_gradient == vec!["G".to_string()] {
            self.toggle_depth_gradient = vec!["Shift+D".to_string()];
            self.move_shot = vec!["G".to_string()];
            tracing::info!(
                "Migrated toggle_depth_gradient shortcut: G -> Shift+D (G now used by move_shot)"
            );
        }

        // Normalize each field
        normalize_shortcut_field(&mut self.select, &defaults.select, "select");
        normalize_shortcut_field(&mut self.pan, &defaults.pan, "pan");
        normalize_shortcut_field(&mut self.zoom, &defaults.zoom, "zoom");
        normalize_shortcut_field(&mut self.merge, &defaults.merge, "merge");
        normalize_shortcut_field(&mut self.measure, &defaults.measure, "measure");
        normalize_shortcut_field(
            &mut self.move_station,
            &defaults.move_station,
            "move_station",
        );
        normalize_shortcut_field(
            &mut self.move_segment,
            &defaults.move_segment,
            "move_segment",
        );
        normalize_shortcut_field(&mut self.move_shot, &defaults.move_shot, "move_shot");
        normalize_shortcut_field(
            &mut self.comment_tool,
            &defaults.comment_tool,
            "comment_tool",
        );
        normalize_shortcut_field(
            &mut self.move_comment_tool,
            &defaults.move_comment_tool,
            "move_comment_tool",
        );
        normalize_shortcut_field(&mut self.undo, &defaults.undo, "undo");
        normalize_shortcut_field(&mut self.redo, &defaults.redo, "redo");
        normalize_shortcut_field(
            &mut self.toggle_depth_gradient,
            &defaults.toggle_depth_gradient,
            "toggle_depth_gradient",
        );
        normalize_shortcut_field(&mut self.toggle_grid, &defaults.toggle_grid, "toggle_grid");
        normalize_shortcut_field(
            &mut self.toggle_satellite,
            &defaults.toggle_satellite,
            "toggle_satellite",
        );
        normalize_shortcut_field(
            &mut self.toggle_station_labels,
            &defaults.toggle_station_labels,
            "toggle_station_labels",
        );
        normalize_shortcut_field(
            &mut self.toggle_shot_labels,
            &defaults.toggle_shot_labels,
            "toggle_shot_labels",
        );
        normalize_shortcut_field(
            &mut self.toggle_section_labels,
            &defaults.toggle_section_labels,
            "toggle_section_labels",
        );
        normalize_shortcut_field(
            &mut self.fit_to_bounds,
            &defaults.fit_to_bounds,
            "fit_to_bounds",
        );
        normalize_shortcut_field(&mut self.zoom_in, &defaults.zoom_in, "zoom_in");
        normalize_shortcut_field(&mut self.zoom_out, &defaults.zoom_out, "zoom_out");
        normalize_shortcut_field(
            &mut self.toggle_sections_panel,
            &defaults.toggle_sections_panel,
            "toggle_sections_panel",
        );
        normalize_shortcut_field(
            &mut self.toggle_detail_panel,
            &defaults.toggle_detail_panel,
            "toggle_detail_panel",
        );
        normalize_shortcut_field(
            &mut self.toggle_data_table,
            &defaults.toggle_data_table,
            "toggle_data_table",
        );
        normalize_shortcut_field(&mut self.save, &defaults.save, "save");
        normalize_shortcut_field(&mut self.new_file, &defaults.new_file, "new_file");
        normalize_shortcut_field(&mut self.open_file, &defaults.open_file, "open_file");
        normalize_shortcut_field(&mut self.save_as, &defaults.save_as, "save_as");
        normalize_shortcut_field(&mut self.import_dmp, &defaults.import_dmp, "import_dmp");
        normalize_shortcut_field(&mut self.export, &defaults.export, "export");
        normalize_shortcut_field(&mut self.quit, &defaults.quit, "quit");
    }
}

/// Normalize a shortcut field:
/// - If empty, use defaults
/// - Remove empty strings
/// - Truncate to MAX_BINDINGS_PER_ACTION
/// - Normalize each shortcut string
fn normalize_shortcut_field(field: &mut Vec<String>, defaults: &[String], field_name: &str) {
    // Remove empty strings
    field.retain(|s| !s.trim().is_empty());

    // If empty after cleanup, use defaults
    if field.is_empty() {
        *field = defaults.to_vec();
        return;
    }

    // Truncate to max bindings
    if field.len() > MAX_BINDINGS_PER_ACTION {
        tracing::warn!(
            "Shortcut '{}' has {} bindings, truncating to {}",
            field_name,
            field.len(),
            MAX_BINDINGS_PER_ACTION
        );
        field.truncate(MAX_BINDINGS_PER_ACTION);
    }

    // Normalize each shortcut
    let mut normalized: Vec<String> = Vec::with_capacity(field.len());
    for shortcut in field.iter() {
        if let Some(n) = normalize_shortcut(shortcut) {
            normalized.push(n);
        } else {
            tracing::warn!(
                "Invalid shortcut '{}' for '{}', falling back to default",
                shortcut,
                field_name
            );
            *field = defaults.to_vec();
            return;
        }
    }
    *field = normalized;
}

/// Normalize a single shortcut string to canonical form
/// Returns None if the shortcut is invalid
///
/// Key normalization rules:
/// - Single letter keys are UPPERCASE (matches JavaScript KeyboardEvent.key)
/// - Special keys use PascalCase canonical names (Space, ArrowUp, etc.)
/// - Modifiers are sorted: Ctrl -> Alt -> Shift -> Key
pub(super) fn normalize_shortcut(shortcut: &str) -> Option<String> {
    let shortcut = shortcut.trim();
    if shortcut.is_empty() {
        return None;
    }

    // Special case: single "+" key
    if shortcut == "+" {
        return Some("Plus".to_string());
    }

    // Special case: single "-" key
    if shortcut == "-" {
        return Some("Minus".to_string());
    }

    // Special case: single "=" key
    if shortcut == "=" {
        return Some("Equal".to_string());
    }

    // Split by + to get modifiers and key
    // Handle "Ctrl++" (Ctrl+Plus) by counting consecutive + at end
    let parts: Vec<&str> = shortcut.split('+').map(str::trim).collect();
    if parts.is_empty() {
        return None;
    }

    // Count trailing empty parts (consecutive + at end means Plus key)
    let trailing_empty_count = parts.iter().rev().take_while(|p| p.is_empty()).count();

    // Check for empty parts in the middle (not trailing)
    // e.g., "Ctrl++S" has empty in middle -> invalid
    // e.g., "Ctrl++" has trailing empties -> valid (means Ctrl+Plus)
    let non_trailing_parts = &parts[..parts.len().saturating_sub(trailing_empty_count)];
    if non_trailing_parts.iter().any(|p| p.is_empty()) {
        return None;
    }

    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    let mut key: Option<String> = None;

    // Process non-empty parts
    for part in non_trailing_parts {
        let lower = part.to_lowercase();
        match lower.as_str() {
            "ctrl" | "control" | "meta" | "cmd" | "command" => ctrl = true,
            "alt" | "option" => alt = true,
            "shift" => shift = true,
            _ => {
                // This is the key (should be only one non-modifier)
                if key.is_some() {
                    // Multiple non-modifier keys is invalid
                    return None;
                }
                key = Some(normalize_key_name(part));
            }
        }
    }

    // Handle trailing empty parts as Plus key
    // "Ctrl++" -> trailing_empty_count=2, but we only need one Plus
    // "Ctrl+" -> trailing_empty_count=1
    if trailing_empty_count > 0 {
        if key.is_some() {
            // Already have a key, can't have Plus too
            return None;
        }
        key = Some("Plus".to_string());
    }

    // Must have exactly one key
    let key = key?;

    // Build canonical form: Ctrl+Alt+Shift+Key
    let mut result = String::new();
    if ctrl {
        result.push_str("Ctrl+");
    }
    if alt {
        result.push_str("Alt+");
    }
    if shift {
        result.push_str("Shift+");
    }
    result.push_str(&key);

    Some(result)
}

/// Map key names to canonical form (PascalCase for special keys, UPPERCASE for letters)
///
/// Canonical names match JavaScript KeyboardEvent.key values where possible.
/// Single letter keys are uppercase to match KeyboardEvent.key (e.g., "S" not "s").
fn normalize_key_name(key: &str) -> String {
    let lower = key.to_lowercase();
    if let Some(num) = lower
        .strip_prefix("digit")
        .or_else(|| lower.strip_prefix("numpad"))
    {
        if num.len() == 1 && num.chars().all(|c| c.is_ascii_digit()) {
            return num.to_string();
        }
    }
    match lower.as_str() {
        // Special keys - canonical names (PascalCase, matching KeyboardEvent.key)
        "space" | "spacebar" => "Space".to_string(),
        "enter" | "return" => "Enter".to_string(),
        "esc" | "escape" => "Escape".to_string(),
        "up" | "arrowup" => "ArrowUp".to_string(),
        "down" | "arrowdown" => "ArrowDown".to_string(),
        "left" | "arrowleft" => "ArrowLeft".to_string(),
        "right" | "arrowright" => "ArrowRight".to_string(),
        "+" | "plus" => "Plus".to_string(),
        "-" | "minus" => "Minus".to_string(),
        "=" | "equal" | "equals" => "Equal".to_string(),
        "backspace" | "back" => "Backspace".to_string(),
        "delete" | "del" => "Delete".to_string(),
        "tab" => "Tab".to_string(),
        "home" => "Home".to_string(),
        "end" => "End".to_string(),
        "pageup" | "pgup" => "PageUp".to_string(),
        "pagedown" | "pgdn" => "PageDown".to_string(),
        "insert" | "ins" => "Insert".to_string(),
        // Function keys (F1-F12)
        k if k.starts_with('f') && k.len() > 1 && k[1..].parse::<u8>().is_ok() => {
            format!("F{}", &k[1..])
        }
        // Single letter keys - UPPERCASE (matches KeyboardEvent.key)
        k if k.len() == 1 && k.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) => {
            k.to_uppercase()
        }
        // Numbers and other keys - preserve as-is
        _ => key.to_string(),
    }
}

/// Custom deserializer that accepts both single String and Vec<String>
/// for backward compatibility with legacy config format
fn deserialize_shortcut_vec<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize, TS)]
    #[serde(untagged)]
    enum StringOrVec {
        Single(String),
        Multiple(Vec<String>),
    }

    match StringOrVec::deserialize(deserializer)? {
        StringOrVec::Single(s) => Ok(vec![s]),
        StringOrVec::Multiple(v) => Ok(v),
    }
}

impl Config {
    /// Load config from file, falling back to defaults
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;
        Self::load_from_path(&config_path)
    }

    /// Save config to file
    pub fn save(&mut self) -> Result<()> {
        // Note: We intentionally do NOT call normalize() here.
        // Normalization fills in defaults for missing fields and should only
        // happen when LOADING from file, not when SAVING. This allows users
        // to explicitly set shortcuts to empty (no shortcut).
        let config_path = Self::config_path()?;
        self.save_to_path(&config_path)
    }

    /// Load config from a specific path, falling back to defaults
    pub fn load_from_path(path: &Path) -> Result<Self> {
        // Collect anything a previous run leaked before reading. This is the only
        // sweeper for these files (see `write_atomically`), and load is the one
        // moment we know the directory is being used and no write is in flight
        // from *this* call.
        if let Some(parent) = path.parent() {
            super::atomic_write::sweep_stale_temp_files(parent);
        }

        if path.exists() {
            let contents = std::fs::read_to_string(path).context("Failed to read config file")?;
            let mut document: toml::Value =
                toml::from_str(&contents).context("Failed to parse config file")?;
            let legacy = document
                .as_table_mut()
                .and_then(|table| table.remove("speleodb"));
            let mut config: Config = document.try_into().context("Failed to parse config file")?;
            super::speleodb_file::load_legacy(&mut config.speleodb, path, legacy);
            super::speleodb_file::load_or_preserve(&mut config.speleodb, path);
            config.migrate_legacy();
            config.normalize();
            tracing::info!(config_path = ?path, "Loaded config from file");
            Ok(config)
        } else {
            tracing::info!(config_path = ?path, "Using default config");
            let mut config = Self::default();
            super::speleodb_file::load_or_preserve(&mut config.speleodb, path);
            config.normalize();
            Ok(config)
        }
    }

    /// Save config to a specific path
    pub fn save_to_path(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("Failed to create config directory")?;
        }

        let mut document = toml::Value::try_from(self).context("Failed to serialize config")?;
        if let Some(table) = document.as_table_mut() {
            table.remove("speleodb");
        }
        let mut contents =
            toml::to_string_pretty(&document).context("Failed to serialize config")?;
        // `webkit_memory` is `#[serde(skip_serializing)]` (backend-only, kept out
        // of the frontend types and payload), so `to_string_pretty` above omits
        // it. Re-append it as a top-level `[webkit_memory]` table whenever it
        // differs from the default so a hand-tuned knob in `config.toml` survives
        // a save (which fires on every project open/create/save). A top-level TOML
        // table appended at the end is order-independent and valid; the value is
        // exactly what was loaded from disk (preserved across the session by
        // `AppState::update_config`), so this round-trips it faithfully.
        if self.webkit_memory != WebkitMemoryConfig::default() {
            #[derive(serde::Serialize)]
            struct WebkitMemoryDoc<'a> {
                webkit_memory: &'a WebkitMemoryConfig,
            }
            let section = toml::to_string_pretty(&WebkitMemoryDoc {
                webkit_memory: &self.webkit_memory,
            })
            .context("Failed to serialize webkit_memory config")?;
            if !contents.ends_with('\n') {
                contents.push('\n');
            }
            contents.push('\n');
            contents.push_str(&section);
        }
        super::speleodb_file::save_with_general(&self.speleodb, path, &contents)?;
        tracing::info!(config_path = ?path, "Saved config to file");
        Ok(())
    }

    /// Persist only SpeleoDB settings and the backend token.
    pub(crate) fn save_speleodb(&self) -> Result<()> {
        super::speleodb_file::save(&self.speleodb, &Self::config_path()?)
    }

    /// Get the config file path
    pub fn config_path() -> Result<PathBuf> {
        // Under test this resolves PER THREAD - a temp dir of the calling
        // thread's own unless a guard says otherwise - so no test reads or
        // pollutes the developer's real `~/.config/stalagmight/config.toml`, and
        // no test can land a write in another's file (#522). See
        // `crate::test_helpers` for the modes and the spawned-thread handoff.
        // `any(test, feature = "test-support")`, matching the helper's own gate:
        // the root crate's tests isolate their config through this hook too, and
        // for THEIR builds this crate is a dependency - `cfg(test)` here would
        // be off and their isolation would be ignored.
        #[cfg(any(test, feature = "test-support"))]
        if let Some(dir) = crate::test_helpers::current_config_dir() {
            return Ok(dir.join("config.toml"));
        }
        if let Some(dir) = crate::app_dirs::config_dir() {
            return Ok(dir.join("config.toml"));
        }
        // Fallback for pre-setup calls on desktop
        let dirs = directories::ProjectDirs::from("com", "stalagmight", "Stalagmight")
            .context("No home directory")?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    /// Get the next section color from the palette
    pub fn next_section_color(&self, used_colors: &[String]) -> String {
        for color in &self.section_colors.palette {
            if !used_colors.contains(color) {
                return color.clone();
            }
        }
        // If all colors used, start from beginning
        self.section_colors
            .palette
            .first()
            .cloned()
            .unwrap_or_else(|| "#4ecdc4".to_string())
    }

    /// Normalize all config values to valid ranges
    /// Handles NaN/inf by falling back to defaults
    pub fn normalize(&mut self) {
        self.shortcuts.normalize();
        self.normalize_except_shortcuts();
    }

    /// Everything [`Self::normalize`] repairs EXCEPT `[shortcuts]`, for the IPC
    /// write path (`AppState::update_config`).
    ///
    /// The split exists because normalization is doing two different jobs, and
    /// only one of them is right for an arriving payload. Clamping a NaN sigma or
    /// a 100000-pixel column is REPAIR: the value is unusable however it got here,
    /// and repairing it on the way in is strictly better than persisting it and
    /// repairing it on the next launch. `normalize_shortcut_field` is DEFAULTING:
    /// it reads an empty binding list as "this key is missing from the file" and
    /// re-seeds the stock binding.
    ///
    /// That reading is correct for a config file and wrong for a Settings payload,
    /// where an empty list is the user having deliberately cleared the binding -
    /// `ShortcutsSettings.tsx` produces exactly that when the last slot for an
    /// action is cleared. Running it on the write path would restore the default
    /// binding under them and make clearing a shortcut impossible, which is the
    /// same trap `Config::save`'s comment already warns about.
    ///
    /// Shortcuts that arrive malformed rather than empty are therefore left for
    /// the load path to canonicalize, exactly as before this split. The frontend
    /// already canonicalizes on entry (`normalizeShortcut`), so that window is the
    /// pre-existing one, not a new one.
    pub fn normalize_except_shortcuts(&mut self) {
        self.map.normalize();
        self.panels.normalize();
        self.global_view.normalize();
        self.appearance.normalize();
        // Normalize SpeleoDB polling settings.
        self.speleodb.normalize();
        self.closure_adjustment.normalize();
        self.placement_suggestion.normalize();
        self.syntactic_highlighting.normalize();
        self.gas.normalize();
        self.webkit_memory.normalize();
        self.units.decimal_precision.normalize();
        super::general::normalize_general(&mut self.general);
    }

    /// Migrate legacy config fields to new structure
    /// Called during load() before normalize()
    ///
    /// Note: Migration only occurs when map.snap has default values.
    /// If a user explicitly sets map.snap to defaults AND has a legacy [snap] section,
    /// the legacy values will override. This edge case is accepted as the user likely
    /// wants to migrate their old settings.
    pub(super) fn migrate_legacy(&mut self) {
        // Migrate old [snap] section to new map.snap
        if let Some(legacy) = self.legacy_snap.take() {
            // Only migrate if map.snap appears to be at defaults (not explicitly set)
            // Limitation: Cannot distinguish explicit defaults from missing section
            let snap_is_default = (self.map.snap.threshold - default_snap_threshold()).abs()
                < f64::EPSILON
                && self.map.snap.enabled_merge
                && self.map.snap.enabled_measure;

            if snap_is_default {
                if let Some(enabled_merge) = legacy.enabled_merge {
                    self.map.snap.enabled_merge = enabled_merge;
                }
                if let Some(enabled_measure) = legacy.enabled_measure {
                    self.map.snap.enabled_measure = enabled_measure;
                }
                if let Some(threshold_pixels) = legacy.threshold_pixels {
                    self.map.snap.threshold = threshold_pixels;
                }
                tracing::info!("Migrated legacy [snap] config to map.snap");
            }
        }

        self.map.controls.zoom_speed = migrate_zoom_speed(self.map.controls.zoom_speed);
    }
}

/// Read a `map.controls.zoom_speed` written under the pre-#429 scheme as the
/// zoom levels per wheel notch it actually delivered.
///
/// The old value was a multiplier in 0.1-5.0 handed to MapLibre as
/// `speed / 100`, and MapLibre's sigmoid turned that into
/// `log2(2 / (1 + e^-speed))` levels per 100 px notch - so that expression IS
/// the conversion, and the old default of 1.0 comes out at the 0.55 the new
/// default ships (the gesture is unchanged for anyone who never touched the
/// setting). Left alone, a stored 1.0 would instead be read as 1.0 levels and
/// clamped to 0.95: a 73% speed-up nobody asked for.
///
/// Only a value ABOVE the new maximum is unambiguously old-scheme, since that
/// is the whole range the settings dialog offers; the two schemes overlap
/// below it, and a value in range is taken at face value. The result is
/// rounded to the 2 decimals the dialog's 0.05 step works in, so a migrated
/// config reads as a number a user could have typed.
///
/// A non-finite value is neither scheme's; it is left for
/// [`MapControlsConfig::normalize`] to reset, along with anything the
/// conversion lands outside the range.
fn migrate_zoom_speed(stored: f64) -> f64 {
    if !stored.is_finite() || stored <= ZOOM_SPEED_LEVELS_MAX {
        return stored;
    }
    let levels = (2.0 / (1.0 + (-stored).exp())).log2();
    let migrated = (levels * 100.0).round() / 100.0;
    tracing::info!(
        stored,
        migrated,
        "Migrated legacy map.controls.zoom_speed to zoom levels per wheel notch"
    );
    migrated
}
