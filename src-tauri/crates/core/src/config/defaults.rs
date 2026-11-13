use super::{
    AngleUnit, AppearanceConfig, CacheConfig, ClosureAdjustmentConfig, Config, DebugConfig,
    DecimalPrecision, DisplayConfig, ExportConfig, GasConfig, GeneralConfig, GlobalViewConfig,
    GpsOrigin, ImportConfig, LengthUnit, LoggingConfig, MapAnimationConfig, MapConfig,
    MapControlsConfig, MapPaletteConfig, MapSnapConfig, PanelsConfig, PeopleConfig,
    PlacementSuggestionConfig, SectionColorsConfig, ShortcutFeedback, ShortcutsConfig, ShotConfig,
    SpeleoDbConfig, StationConfig, SyntacticHighlightingConfig, UnitsConfig, UpdateTrack,
    ValidationConfig, WebkitMemoryConfig,
};

#[expect(clippy::derivable_impls)]
impl Default for Config {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            units: UnitsConfig::default(),
            display: DisplayConfig::default(),
            section_colors: SectionColorsConfig::default(),
            map: MapConfig::default(),
            validation: ValidationConfig::default(),
            closure_adjustment: ClosureAdjustmentConfig::default(),
            export: ExportConfig::default(),
            logging: LoggingConfig::default(),
            shortcuts: ShortcutsConfig::default(),
            panels: PanelsConfig::default(),
            cache: CacheConfig::default(),
            debug: DebugConfig::default(),
            webkit_memory: WebkitMemoryConfig::default(),
            global_view: GlobalViewConfig::default(),
            appearance: AppearanceConfig::default(),
            import: ImportConfig::default(),
            placement_suggestion: PlacementSuggestionConfig::default(),
            syntactic_highlighting: SyntacticHighlightingConfig::default(),
            gas: GasConfig::default(),
            speleodb: SpeleoDbConfig::default(),
            map_palette: MapPaletteConfig::default(),
            people: PeopleConfig::default(),
            legacy_snap: None,
        }
    }
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            auto_save: true,
            auto_save_interval_seconds: 30,
            recent_projects_max: 20,
            recent_projects: Vec::new(),
            undo_history_limit: 100,
            undo_persist_limit: 20,
            shortcut_feedback: ShortcutFeedback::default(),
            locale: "system".to_string(),
            number_format: "locale".to_string(),
            date_format: "locale".to_string(),
            splash_screen: false,
            onboarding_enabled: true,
            onboarding_completed_flows: Vec::new(),
            onboarding_first_launch_done: false,
            telemetry_enabled: false,
            telemetry_opt_in_shown: false,
            update_track: UpdateTrack::default(),
            update_check_enabled: true,
        }
    }
}

impl Default for DecimalPrecision {
    fn default() -> Self {
        Self {
            length: 1,
            coordinates: 2,
            depth: 1,
            azimuth: 1,
            inclination: 1,
            gps: 6,
        }
    }
}

impl Default for UnitsConfig {
    fn default() -> Self {
        Self {
            length: LengthUnit::Meters,
            depth: LengthUnit::Meters,
            angle: AngleUnit::Degrees,
            decimal_precision: DecimalPrecision::default(),
        }
    }
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            grid_color: "#333366".to_string(),
            grid_spacing: 10.0,
            font_size: 12,
            font_color: "#f5f5f5".to_string(),
            measure_path_color: "#22d3ee".to_string(),
        }
    }
}

impl Default for SectionColorsConfig {
    fn default() -> Self {
        Self {
            palette: vec![
                "#4ecdc4".to_string(), // Teal
                "#ff6b6b".to_string(), // Coral
                "#95e1d3".to_string(), // Mint
                "#f38181".to_string(), // Salmon
                "#aa96da".to_string(), // Lavender
                "#fcbad3".to_string(), // Pink
                "#a8d8ea".to_string(), // Sky Blue
                "#ffcb77".to_string(), // Peach
            ],
        }
    }
}

impl Default for MapConfig {
    fn default() -> Self {
        Self {
            satellite_provider: "esri".to_string(),
            satellite_enabled: true,
            satellite_gps_origin: GpsOrigin {
                latitude: 24.55,
                longitude: -81.78,
            },
            zoom_default: 15,
            zoom_min: 0,
            zoom_max: 22,
            zoom_step: 1.0,
            controls: MapControlsConfig::default(),
            station: StationConfig::default(),
            shot: ShotConfig::default(),
            snap: MapSnapConfig::default(),
            animation: MapAnimationConfig::default(),
            highlight_warnings_on_tab_switch: true,
        }
    }
}

impl Default for MapControlsConfig {
    fn default() -> Self {
        Self {
            pan_speed: 1.0,
            // Zoom LEVELS per 100 px wheel notch, not a multiplier (#429). 0.55
            // is what the old 1.0 multiplier actually delivered through
            // MapLibre's sigmoid, so the default gesture is unchanged.
            zoom_speed: 0.55,
            invert_scroll: false,
            show_navigation_tools: false,
        }
    }
}

impl Default for ExportConfig {
    fn default() -> Self {
        Self {
            default_image_format: "png".to_string(),
            default_dpi: 300,
            default_include_satellite: true,
            default_include_grid: false,
            default_include_labels: true,
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            max_directory_size_mb: 128,
        }
    }
}

impl Default for ShortcutsConfig {
    fn default() -> Self {
        Self {
            // Tools
            select: vec!["S".to_string()],
            pan: vec!["P".to_string()],
            zoom: vec!["Z".to_string()],
            merge: vec!["M".to_string()],
            measure: vec!["R".to_string()],
            move_station: vec!["T".to_string()],
            move_segment: vec!["V".to_string()],
            move_shot: vec!["G".to_string()],
            comment_tool: vec!["C".to_string()],
            move_comment_tool: vec!["Shift+C".to_string()],

            // Edit
            undo: vec!["Ctrl+Z".to_string()],
            redo: vec!["Ctrl+Y".to_string(), "Ctrl+Shift+Z".to_string()],

            // View
            toggle_depth_gradient: vec!["Shift+D".to_string()],
            toggle_grid: vec!["Shift+G".to_string()],
            toggle_satellite: vec!["B".to_string()],
            toggle_station_labels: vec!["L".to_string()],
            toggle_shot_labels: vec!["Shift+L".to_string()],
            toggle_section_labels: vec!["Ctrl+L".to_string()],
            fit_to_bounds: vec!["F".to_string()],
            zoom_in: vec!["Equal".to_string(), "Plus".to_string()],
            zoom_out: vec!["Minus".to_string()],

            // Panels
            toggle_sections_panel: vec!["Ctrl+1".to_string()],
            toggle_detail_panel: vec!["Ctrl+2".to_string()],
            toggle_data_table: vec!["Ctrl+3".to_string()],

            // File
            save: vec!["Ctrl+S".to_string()],
            new_file: vec!["Ctrl+N".to_string()],
            open_file: vec!["Ctrl+O".to_string()],
            save_as: vec!["Ctrl+Shift+S".to_string()],
            import_dmp: vec!["Ctrl+I".to_string()],
            export: vec!["Ctrl+E".to_string()],
            quit: vec!["Ctrl+Q".to_string()],
        }
    }
}

impl Default for PanelsConfig {
    fn default() -> Self {
        Self {
            sections_panel_width: 250,
            detail_panel_width: 300,
            data_table_height: 200,
            wizard_map_panel_width: 480,
            data_table_shot_column_order: Vec::new(),
            data_table_shot_hidden_columns: super::panels::default_data_table_hidden_columns(),
            data_table_shot_columns_seen: super::panels::default_shot_columns_seen(),
            data_table_shot_column_widths: std::collections::BTreeMap::new(),
            data_table_azimuth_mode: "avg".to_string(),
            data_table_inclination_mode: "off".to_string(),
            data_table_station_column_order: Vec::new(),
            data_table_station_hidden_columns: super::panels::default_station_hidden_columns(),
            data_table_station_columns_seen: super::panels::default_station_columns_seen(),
            data_table_station_column_widths: std::collections::BTreeMap::new(),
            new_section_column_order: Vec::new(),
            new_section_hidden_columns: vec![
                "azimuth_in".to_string(),
                "azimuth_out".to_string(),
                "_pitch_avg".to_string(),
                "pitch_in".to_string(),
                "pitch_out".to_string(),
                "left".to_string(),
                "right".to_string(),
                "up".to_string(),
                "down".to_string(),
            ],
            new_section_column_widths: std::collections::BTreeMap::new(),
            edit_section_column_order: Vec::new(),
            edit_section_hidden_columns: vec![
                "azimuth_in".to_string(),
                "azimuth_out".to_string(),
                "_pitch_avg".to_string(),
                "pitch_in".to_string(),
                "pitch_out".to_string(),
            ],
            edit_section_column_widths: std::collections::BTreeMap::new(),
            import_shots_column_order: Vec::new(),
            import_shots_hidden_columns: super::panels::default_edit_section_hidden_columns(),
            import_shots_column_widths: std::collections::BTreeMap::new(),
            sections_panel_open: true,
            sections_panel_mode: "pin".to_string(),
            detail_panel_open: true,
            detail_panel_mode: "pin".to_string(),
            data_table_open: true,
            data_table_mode: "pin".to_string(),
        }
    }
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            tile_cache_enabled: true,
            tile_cache_max_mb: 500, // per SPEC.md
            tile_ttl_days: 30,
            // No per-provider overrides by default. The single on/off switch is
            // `tile_cache_enabled`; whether any given tile is actually written
            // is decided per response by its `Cache-Control` grant, not by
            // config (see `crate::cache::tiles`).
            providers: std::collections::HashMap::new(),
        }
    }
}
