use super::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ========================================================================
// Default Values Tests
// ========================================================================

#[test]
fn test_station_config_defaults() {
    let config = StationConfig::default();
    assert_eq!(config.radius, 4.0);
    assert_eq!(config.hit_area, 10.0);
}

#[test]
fn test_shot_config_defaults() {
    let config = ShotConfig::default();
    assert_eq!(config.width, 2.0);
    assert_eq!(config.hit_area, 8.0);
}

#[test]
fn test_map_snap_config_defaults() {
    let config = MapSnapConfig::default();
    assert_eq!(config.threshold, 10.0);
    assert!(config.enabled_merge);
    assert!(config.enabled_measure);
}

#[test]
fn test_map_controls_config_defaults() {
    let config = MapControlsConfig::default();
    assert_eq!(config.pan_speed, 1.0);
    // #429: zoom_speed is zoom LEVELS per wheel notch, and 0.55 is what the old
    // 1.0 multiplier delivered through MapLibre's sigmoid.
    assert_eq!(config.zoom_speed, 0.55);
    assert!(!config.invert_scroll);
    // #409: the Pan/Zoom toolbar group is hidden unless the user opts in.
    assert!(!config.show_navigation_tools);
}

#[test]
fn test_map_config_defaults() {
    let config = MapConfig::default();
    assert_eq!(config.station.radius, 4.0);
    assert_eq!(config.station.hit_area, 10.0);
    assert_eq!(config.shot.width, 2.0);
    assert_eq!(config.shot.hit_area, 8.0);
    assert_eq!(config.snap.threshold, 10.0);
    assert_eq!(config.zoom_step, 1.0);
}

#[test]
fn test_debug_config_defaults() {
    let config = DebugConfig::default();
    assert!(!config.map.show_hitboxes);
    assert!(!config.map.log_hit_detection);
}

// ========================================================================
// Station Config Validation Tests
// ========================================================================

#[test]
fn test_station_validation_accepts_minimum_bound() {
    let mut config = StationConfig {
        radius: 1.0,
        hit_area: 1.0,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.radius, 1.0);
    assert_eq!(config.hit_area, 1.0);
}

#[test]
fn test_station_validation_accepts_mid_range() {
    let mut config = StationConfig {
        radius: 25.0,
        hit_area: 25.0,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.radius, 25.0);
    assert_eq!(config.hit_area, 25.0);
}

#[test]
fn test_station_validation_accepts_maximum_bound() {
    let mut config = StationConfig {
        radius: 50.0,
        hit_area: 50.0,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.radius, 50.0);
    assert_eq!(config.hit_area, 50.0);
}

#[test]
fn test_station_validation_rejects_zero() {
    let mut config = StationConfig {
        radius: 0.0,
        hit_area: 0.0,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.radius, 1.0); // clamped to min
    assert_eq!(config.hit_area, 1.0); // clamped to min
}

#[test]
fn test_gas_config_normalize_clamps_invalid_speeds_and_multipliers() {
    let mut config = GasConfig {
        swim_speed_m_per_min: 0.0,          // invalid -> default 10
        survey_speed_m_per_min: Some(-1.0), // invalid -> None (symmetric)
        survey_rmv_multiplier: f64::NAN,    // invalid -> default 1.0
        stressed_rmv_multiplier: 0.0,       // invalid -> default 1.5
        jump_rmv_multiplier: 0.5,           // below 1.0 (would cut gas) -> default 1.5
        dpv: Some(DpvRig {
            primary: dpv_unit(-5.0, 90.0),
            backup: dpv_unit(50.0, 60.0),
            range_reserve_fraction: 0.2,
        }),
        ..GasConfig::default()
    };
    config.normalize();
    assert_eq!(config.swim_speed_m_per_min, 10.0);
    assert_eq!(config.survey_speed_m_per_min, None);
    assert_eq!(config.survey_rmv_multiplier, 1.0);
    assert_eq!(config.stressed_rmv_multiplier, 1.5);
    assert_eq!(config.jump_rmv_multiplier, 1.5); // clamped up to the default: a jump is never less work
    assert_eq!(config.dpv, None); // bad DPV speed drops the whole rig
}

fn dpv_unit(speed: f64, burn: f64) -> DpvUnit {
    DpvUnit {
        speed_m_per_min: speed,
        burn_minutes: burn,
    }
}

#[test]
fn test_gas_config_normalize_drops_the_whole_rig_when_either_unit_is_invalid() {
    // Repairing one unit is not an option: the cap is `min(range)` over the pair, so inventing a
    // backup would silently SET the penetration limit from a number nobody entered. Dropping the
    // rig removes the cap instead - which is safe only because `resolve_segments` then also forces
    // every span off the scooter, so nothing is flown on a unit that does not exist.
    for (primary, backup) in [
        (dpv_unit(55.0, 0.0), dpv_unit(50.0, 60.0)), // zero burn
        (dpv_unit(55.0, 90.0), dpv_unit(f64::NAN, 60.0)), // NaN speed
        (dpv_unit(55.0, 90.0), dpv_unit(50.0, -1.0)), // negative burn
        (dpv_unit(f64::INFINITY, 90.0), dpv_unit(50.0, 60.0)),
    ] {
        let mut config = GasConfig {
            dpv: Some(DpvRig {
                primary,
                backup,
                range_reserve_fraction: 0.2,
            }),
            ..GasConfig::default()
        };
        config.normalize();
        assert_eq!(config.dpv, None);
    }
}

#[test]
fn test_gas_config_normalize_guards_the_dpv_range_reserve() {
    let rig = |fraction| GasConfig {
        dpv: Some(DpvRig {
            primary: dpv_unit(55.0, 90.0),
            backup: dpv_unit(50.0, 60.0),
            range_reserve_fraction: fraction,
        }),
        ..GasConfig::default()
    };
    // A NEGATIVE reserve would credit MORE range than the units carry (over-stating reach, the
    // unsafe direction), so it resets to the default rather than being clamped to 0.
    for bad in [-0.1, f64::NAN] {
        let mut config = rig(bad);
        config.normalize();
        assert_eq!(
            config.dpv.unwrap().range_reserve_fraction,
            default_dpv_range_reserve_fraction()
        );
    }
    // At/above 1.0 the cap would be zero and every scootered station would read unreachable with
    // no explanation. That is useless rather than dangerous, so it caps instead of resetting.
    let mut config = rig(2.0);
    config.normalize();
    assert_eq!(config.dpv.unwrap().range_reserve_fraction, 0.9);
    // A legitimate value is left alone.
    let mut config = rig(0.35);
    config.normalize();
    assert_eq!(config.dpv.unwrap().range_reserve_fraction, 0.35);
    // The unclamped default is 0.1 - confirmed literally, not just by round-tripping through the
    // same function the resets above already call.
    assert_eq!(default_dpv_range_reserve_fraction(), 0.1);
}

#[test]
fn test_gas_config_normalize_clamps_dpv_range_reserve_at_the_cap_boundary() {
    // ⚠ CROSS-LANGUAGE PIN. The Settings > Gas panel previews the DPV usable range before any
    // plan exists (`GasSettings.tsx`), so it duplicates this clamp in TypeScript rather than
    // reading it off a computed field - a duplicated clamp drifts silently (the #233
    // gauge-divergence class), so both sides pin the exact same boundary and the twin is named
    // in each so a change to either breaks a test that says where its counterpart lives.
    //
    // Twin: `the usable-range preview clamps a reserve above the cap down to 0.9` in
    // `src/components/Dialogs/settings/GasSettings.test.tsx`.
    let mut config = GasConfig {
        dpv: Some(DpvRig {
            primary: dpv_unit(55.0, 90.0),
            backup: dpv_unit(50.0, 60.0),
            range_reserve_fraction: 0.95,
        }),
        ..GasConfig::default()
    };
    config.normalize();
    assert_eq!(config.dpv.unwrap().range_reserve_fraction, 0.9);
}

#[test]
fn test_gas_config_normalize_floors_survey_and_stressed_rmv_at_one() {
    // #241 (P1): survey/stressed RMV multipliers are work-factors relative to the baseline
    // swim RMV, so a positive-but-below-1.0 value (which the old `<= 0.0` guard let through)
    // under-states in-leg gas and over-states reach -- the unsafe direction. They must floor
    // at 1.0 like the jump multiplier. A value >= 1.0 is a legitimate advanced setting and is
    // preserved (covered by test_gas_config_normalize_preserves_valid_values).
    let mut config = GasConfig {
        survey_rmv_multiplier: 0.5,   // positive but < 1.0 -> reset to default 1.0
        stressed_rmv_multiplier: 0.8, // positive but < 1.0 -> reset to default 1.5
        ..GasConfig::default()
    };
    config.normalize();
    assert_eq!(
        config.survey_rmv_multiplier, 1.0,
        "survey RMV below 1.0 must floor to the 1.0 default"
    );
    assert_eq!(
        config.stressed_rmv_multiplier, 1.5,
        "stressed RMV below 1.0 must floor to the 1.5 default"
    );
}

#[test]
fn test_gas_config_normalize_preserves_rmv_multipliers_at_exactly_one() {
    // The floor is a STRICT `< 1.0`, so exactly 1.0 is a legitimate value and must be
    // preserved, not reset to a default. This matters most for stressed (default 1.5):
    // a regression to `<= 1.0` would silently bump a user-set 1.0 up to 1.5.
    let mut config = GasConfig {
        survey_rmv_multiplier: 1.0,
        stressed_rmv_multiplier: 1.0,
        jump_rmv_multiplier: 1.0,
        ..GasConfig::default()
    };
    config.normalize();
    assert_eq!(
        config.survey_rmv_multiplier, 1.0,
        "survey RMV of exactly 1.0 must be preserved"
    );
    assert_eq!(
        config.stressed_rmv_multiplier, 1.0,
        "stressed RMV of exactly 1.0 must be preserved"
    );
    assert_eq!(
        config.jump_rmv_multiplier, 1.0,
        "jump RMV of exactly 1.0 must be preserved"
    );
}

#[test]
fn test_gas_config_normalize_preserves_valid_values() {
    let mut config = GasConfig {
        swim_speed_m_per_min: 12.0,
        survey_speed_m_per_min: Some(4.0),
        survey_rmv_multiplier: 1.2,
        stressed_rmv_multiplier: 2.0,
        jump_rmv_multiplier: 2.0, // a valid non-default value: must be preserved, not reset
        ..GasConfig::default()
    };
    config.normalize();
    assert_eq!(config.swim_speed_m_per_min, 12.0);
    assert_eq!(config.survey_speed_m_per_min, Some(4.0));
    assert_eq!(config.survey_rmv_multiplier, 1.2);
    assert_eq!(config.stressed_rmv_multiplier, 2.0);
    assert_eq!(config.jump_rmv_multiplier, 2.0);
}

#[test]
fn test_gas_config_normalize_drops_backwards_survey_speed() {
    // Survey faster than swim is backwards → dropped to symmetric (None).
    let mut backwards = GasConfig {
        swim_speed_m_per_min: 10.0,
        survey_speed_m_per_min: Some(15.0),
        ..GasConfig::default()
    };
    backwards.normalize();
    assert_eq!(backwards.survey_speed_m_per_min, None);

    // Survey slower than (or equal to) swim is kept.
    let mut ok = GasConfig {
        swim_speed_m_per_min: 10.0,
        survey_speed_m_per_min: Some(4.0),
        ..GasConfig::default()
    };
    ok.normalize();
    assert_eq!(ok.survey_speed_m_per_min, Some(4.0));
}

#[test]
fn test_gas_config_normalize_clamps_reserve_profile() {
    // A NEGATIVE stage reserve is the unsafe direction (it drops the stage lower → more in-gas),
    // so it resets to the default; non-finite recalc holds reset too; valid values preserved.
    let mut config = GasConfig {
        reserve_profile: ReserveProfile {
            stage_reserve_bar: -5.0,
            all_thirds_safety_margin_bar: -3.0, // negative → adds gas (unsafe) → reset to 0
            recalc_hold_stage2_bar: f64::NAN,
            recalc_hold_stage1_bar: 80.0, // valid → kept
        },
        ..GasConfig::default()
    };
    config.normalize();
    assert_eq!(config.reserve_profile.stage_reserve_bar, 30.0);
    assert_eq!(config.reserve_profile.all_thirds_safety_margin_bar, 0.0);
    assert_eq!(config.reserve_profile.recalc_hold_stage2_bar, 100.0);
    assert_eq!(config.reserve_profile.recalc_hold_stage1_bar, 80.0);
}

#[test]
fn test_gas_config_caps_oversized_stage_reserve() {
    // A LARGE stage reserve only shrinks a stage's usable in-gas - the SAFE direction - so it is
    // merely capped at the max (resetting it to the smaller default would be *less* conservative),
    // unlike the negative case which resets. A plausible value is preserved.
    let mut config = GasConfig {
        reserve_profile: ReserveProfile {
            stage_reserve_bar: 9999.0,
            ..ReserveProfile::default()
        },
        ..GasConfig::default()
    };
    config.normalize();
    assert_eq!(config.reserve_profile.stage_reserve_bar, 100.0);

    let mut ok = GasConfig {
        reserve_profile: ReserveProfile {
            stage_reserve_bar: 40.0,
            ..ReserveProfile::default()
        },
        ..GasConfig::default()
    };
    ok.normalize();
    assert_eq!(ok.reserve_profile.stage_reserve_bar, 40.0);
}

#[test]
fn test_gas_config_deserializes_with_reserve_profile_missing() {
    // Migration tolerance: an old config whose [gas] block predates reserve_profile (or omits
    // some members) must still load, defaulting the missing pieces - not error.
    let missing: GasConfig =
        toml::from_str("units = \"metric\"\n").expect("gas config without reserve_profile loads");
    assert_eq!(missing.reserve_profile, ReserveProfile::default());

    // An old config carrying the RENAMED `turn_buffer_bar` key loads fine - the unknown key is
    // ignored and stage_reserve_bar defaults (30). The new key is honoured when present.
    let legacy: GasConfig =
        toml::from_str("units = \"metric\"\n[reserve_profile]\nturn_buffer_bar = 20.0\n")
            .expect("legacy reserve_profile with the old key loads");
    assert_eq!(legacy.reserve_profile.stage_reserve_bar, 30.0);

    let partial: GasConfig =
        toml::from_str("units = \"metric\"\n[reserve_profile]\nstage_reserve_bar = 40.0\n")
            .expect("partial reserve_profile loads");
    assert_eq!(partial.reserve_profile.stage_reserve_bar, 40.0);
    assert_eq!(partial.reserve_profile.recalc_hold_stage2_bar, 100.0); // missing member defaults
}

#[test]
fn test_gas_config_dpv_old_shape_degrades_to_none_without_losing_the_rest_of_the_config() {
    // #317 replaced the old `DpvProfile { speed_m_per_min, burn_minutes: Option<f64>,
    // range_m: Option<f64> }` shape with `DpvRig`. A config written by a previous build still
    // carries a `[dpv]` table in the OLD shape. Erroring on that one field would fail the WHOLE
    // parse, and `Config::load().unwrap_or_default()` then silently resets EVERY setting in the
    // file - not just the DPV block. The `dpv` field must instead degrade to `None`, and every
    // other, unrelated setting in the same file (here a customized swim speed) must survive.
    let toml_str = r#"
units = "metric"
swim_speed_m_per_min = 7.5

[dpv]
speed_m_per_min = 45.0
burn_minutes = 60.0
range_m = 2700.0
"#;
    let config: GasConfig =
        toml::from_str(toml_str).expect("an old-shape dpv table must not fail the whole parse");
    assert_eq!(config.dpv, None);
    assert_eq!(
        config.swim_speed_m_per_min, 7.5,
        "an unrelated customized setting in the same file must survive the dpv shape change"
    );
}

#[test]
fn test_gas_config_dpv_current_shape_round_trips() {
    let toml_str = r#"
units = "metric"

[dpv]
range_reserve_fraction = 0.2

[dpv.primary]
speed_m_per_min = 45.0
burn_minutes = 60.0

[dpv.backup]
speed_m_per_min = 30.0
burn_minutes = 40.0
"#;
    let config: GasConfig = toml::from_str(toml_str).expect("a current-shape dpv table must parse");
    let rig = config
        .dpv
        .expect("a well-formed dpv table must deserialize to Some");
    assert_eq!(rig.primary.speed_m_per_min, 45.0);
    assert_eq!(rig.primary.burn_minutes, 60.0);
    assert_eq!(rig.backup.speed_m_per_min, 30.0);
    assert_eq!(rig.backup.burn_minutes, 40.0);
    assert_eq!(rig.range_reserve_fraction, 0.2);
}

#[test]
fn test_gas_config_dpv_missing_defaults_to_none() {
    let config: GasConfig =
        toml::from_str("units = \"metric\"\n").expect("gas config without a dpv table loads");
    assert_eq!(config.dpv, None);
}

#[test]
fn test_station_validation_rejects_negative() {
    let mut config = StationConfig {
        radius: -1.0,
        hit_area: -1.0,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.radius, 1.0); // clamped to min
    assert_eq!(config.hit_area, 1.0); // clamped to min
}

#[test]
fn test_station_validation_rejects_above_max() {
    let mut config = StationConfig {
        radius: 51.0,
        hit_area: 51.0,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.radius, 50.0); // clamped to max
    assert_eq!(config.hit_area, 50.0); // clamped to max
}

#[test]
fn test_station_validation_rejects_large_values() {
    let mut config = StationConfig {
        radius: 100.0,
        hit_area: 100.0,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.radius, 50.0); // clamped to max
    assert_eq!(config.hit_area, 50.0); // clamped to max
}

#[test]
fn test_station_validation_handles_nan() {
    let mut config = StationConfig {
        radius: f64::NAN,
        hit_area: f64::NAN,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.radius, 4.0); // default
    assert_eq!(config.hit_area, 10.0); // default
}

#[test]
fn test_station_validation_handles_infinity() {
    let mut config = StationConfig {
        radius: f64::INFINITY,
        hit_area: f64::INFINITY,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.radius, 4.0); // default
    assert_eq!(config.hit_area, 10.0); // default
}

#[test]
fn test_station_validation_handles_neg_infinity() {
    let mut config = StationConfig {
        radius: f64::NEG_INFINITY,
        hit_area: f64::NEG_INFINITY,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.radius, 4.0); // default
    assert_eq!(config.hit_area, 10.0); // default
}

#[test]
fn test_station_hit_area_enforced_gte_radius() {
    let mut config = StationConfig {
        radius: 20.0,
        hit_area: 10.0,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.radius, 20.0);
    assert_eq!(config.hit_area, 20.0); // enforced >= radius
}

// ========================================================================
// Shot Config Validation Tests
// ========================================================================

#[test]
fn test_shot_validation_accepts_minimum_bound() {
    let mut config = ShotConfig {
        width: 0.5,
        hit_area: 0.5,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.width, 0.5);
    assert_eq!(config.hit_area, 0.5);
}

#[test]
fn test_shot_validation_accepts_mid_range() {
    let mut config = ShotConfig {
        width: 10.0,
        hit_area: 10.0,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.width, 10.0);
    assert_eq!(config.hit_area, 10.0);
}

#[test]
fn test_shot_validation_accepts_maximum_bound() {
    let mut config = ShotConfig {
        width: 20.0,
        hit_area: 20.0,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.width, 20.0);
    assert_eq!(config.hit_area, 20.0);
}

#[test]
fn test_shot_validation_rejects_zero() {
    let mut config = ShotConfig {
        width: 0.0,
        hit_area: 0.0,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.width, 0.5); // clamped to min
    assert_eq!(config.hit_area, 0.5); // clamped to min
}

#[test]
fn test_shot_validation_rejects_negative() {
    let mut config = ShotConfig {
        width: -1.0,
        hit_area: -1.0,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.width, 0.5); // clamped to min
    assert_eq!(config.hit_area, 0.5); // clamped to min
}

#[test]
fn test_shot_validation_rejects_above_max() {
    let mut config = ShotConfig {
        width: 21.0,
        hit_area: 21.0,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.width, 20.0); // clamped to max
    assert_eq!(config.hit_area, 20.0); // clamped to max
}

#[test]
fn test_shot_validation_rejects_large_values() {
    let mut config = ShotConfig {
        width: 100.0,
        hit_area: 100.0,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.width, 20.0); // clamped to max
    assert_eq!(config.hit_area, 20.0); // clamped to max
}

#[test]
fn test_shot_validation_handles_nan() {
    let mut config = ShotConfig {
        width: f64::NAN,
        hit_area: f64::NAN,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.width, 2.0); // default
    assert_eq!(config.hit_area, 8.0); // default
}

#[test]
fn test_shot_validation_handles_infinity() {
    let mut config = ShotConfig {
        width: f64::INFINITY,
        hit_area: f64::INFINITY,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.width, 2.0); // default
    assert_eq!(config.hit_area, 8.0); // default
}

#[test]
fn test_shot_hit_area_enforced_gte_width() {
    let mut config = ShotConfig {
        width: 15.0,
        hit_area: 5.0,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.width, 15.0);
    assert_eq!(config.hit_area, 15.0); // enforced >= width
}

// ========================================================================
// Snap Config Validation Tests
// ========================================================================

#[test]
fn test_snap_validation_accepts_minimum_bound() {
    let mut config = MapSnapConfig {
        threshold: 1.0,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.threshold, 1.0);
}

#[test]
fn test_snap_validation_accepts_mid_range() {
    let mut config = MapSnapConfig {
        threshold: 25.0,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.threshold, 25.0);
}

#[test]
fn test_snap_validation_accepts_maximum_bound() {
    let mut config = MapSnapConfig {
        threshold: 50.0,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.threshold, 50.0);
}

#[test]
fn test_snap_validation_rejects_zero() {
    let mut config = MapSnapConfig {
        threshold: 0.0,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.threshold, 1.0); // clamped to min
}

#[test]
fn test_snap_validation_rejects_negative() {
    let mut config = MapSnapConfig {
        threshold: -1.0,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.threshold, 1.0); // clamped to min
}

#[test]
fn test_snap_validation_rejects_above_max() {
    let mut config = MapSnapConfig {
        threshold: 51.0,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.threshold, 50.0); // clamped to max
}

#[test]
fn test_snap_validation_handles_nan() {
    let mut config = MapSnapConfig {
        threshold: f64::NAN,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.threshold, 10.0); // default
}

#[test]
fn test_snap_validation_handles_infinity() {
    let mut config = MapSnapConfig {
        threshold: f64::INFINITY,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.threshold, 10.0); // default
}

// ========================================================================
// Config Parsing Tests
// ========================================================================

#[test]
fn test_config_parsing_complete() {
    let toml_str = r##"
[general]
auto_save = true
auto_save_interval_seconds = 30
recent_projects_max = 20

[units]
length = "meters"
depth = "meters"
angle = "degrees"
decimal_precision = 1

[display]
default_background = "#1a1a2e"
grid_color = "#333366"
font_size = 12
font_color = "#f5f5f5"

[display.depth_gradient]
enabled = false
shallow_color = "#00bfff"
deep_color = "#ff4444"
shallow_depth = 0.0
deep_depth = 50.0

[section_colors]
palette = ["#4ecdc4"]

[map]
satellite_provider = "openstreetmap"
satellite_enabled = false
zoom_default = 15
zoom_min = 0
zoom_max = 22
zoom_step = 1.5

[map.controls]
pan_speed = 1.0
zoom_speed = 1.0
invert_scroll = false
show_navigation_tools = true

[map.station]
radius = 6.0
hit_area = 12.0

[map.shot]
width = 3.0
hit_area = 10.0

[map.snap]
threshold = 15.0
enabled_draw = true
enabled_merge = false
enabled_measure = true

[validation]
warn_shot_length_min = 0.1
warn_shot_length_max = 50.0
warn_depth_change_per_shot_max = 10.0
warn_azimuth_deviation_max = 30.0

[export]
default_image_format = "png"
default_dpi = 300
default_include_satellite = true
default_include_grid = false
default_include_labels = true

[logging]
level = "info"
max_directory_size_mb = 128

[shortcuts]
select = "S"
pan = "P"
zoom = "Z"
draw = "D"
delete = "X"
merge = "M"
measure = "R"
move_station = "T"
undo = "Ctrl+Z"
redo = "Ctrl+Y"
save = "Ctrl+S"

[debug.map]
show_hitboxes = true
log_hit_detection = true
"##;

    let config: Config = toml::from_str(toml_str).expect("Complete config should parse");

    // Verify map section values
    assert_eq!(config.map.station.radius, 6.0);
    assert_eq!(config.map.station.hit_area, 12.0);
    assert_eq!(config.map.shot.width, 3.0);
    assert_eq!(config.map.shot.hit_area, 10.0);
    assert_eq!(config.map.snap.threshold, 15.0);
    assert!(!config.map.snap.enabled_merge);
    assert!(config.map.snap.enabled_measure);
    assert_eq!(config.map.zoom_step, 1.5);
    assert!(config.map.controls.show_navigation_tools);

    // Verify debug values
    assert!(config.debug.map.show_hitboxes);
    assert!(config.debug.map.log_hit_detection);
}

#[test]
fn test_config_parsing_partial_uses_defaults() {
    // Config with only required sections, missing optional nested configs
    let toml_str = r##"
[general]
auto_save = true
auto_save_interval_seconds = 30
recent_projects_max = 20

[units]
length = "meters"
depth = "meters"
angle = "degrees"
decimal_precision = 1

[display]
default_background = "#1a1a2e"
grid_color = "#333366"
font_size = 12
font_color = "#f5f5f5"

[display.depth_gradient]
enabled = false
shallow_color = "#00bfff"
deep_color = "#ff4444"
shallow_depth = 0.0
deep_depth = 50.0

[section_colors]
palette = ["#4ecdc4"]

[map]
satellite_provider = "openstreetmap"
satellite_enabled = false
zoom_default = 15
zoom_min = 0
zoom_max = 22

[map.controls]
pan_speed = 1.0
zoom_speed = 1.0
invert_scroll = false

[validation]
warn_shot_length_min = 0.1
warn_shot_length_max = 50.0
warn_depth_change_per_shot_max = 10.0
warn_azimuth_deviation_max = 30.0

[export]
default_image_format = "png"
default_dpi = 300
default_include_satellite = true
default_include_grid = false
default_include_labels = true

[logging]
level = "info"
max_directory_size_mb = 128

[shortcuts]
select = "S"
pan = "P"
zoom = "Z"
draw = "D"
delete = "X"
merge = "M"
measure = "R"
move_station = "T"
undo = "Ctrl+Z"
redo = "Ctrl+Y"
save = "Ctrl+S"
"##;

    let config: Config = toml::from_str(toml_str).expect("Partial config should parse");

    // Missing sections should use defaults
    assert_eq!(config.map.station.radius, 4.0); // default
    assert_eq!(config.map.station.hit_area, 10.0); // default
    assert_eq!(config.map.shot.width, 2.0); // default
    assert_eq!(config.map.shot.hit_area, 8.0); // default
    assert_eq!(config.map.snap.threshold, 10.0); // default
    assert!(config.map.snap.enabled_merge); // default
    assert!(config.map.snap.enabled_measure); // default
    assert_eq!(config.map.zoom_step, 1.0); // default

    // A [map.controls] table written before #409 has no show_navigation_tools key:
    // it must still parse, with the tools hidden.
    assert!(!config.map.controls.show_navigation_tools); // default

    // Debug section defaults
    assert!(!config.debug.map.show_hitboxes);
    assert!(!config.debug.map.log_hit_detection);
}

#[test]
fn test_config_parsing_partial_station_uses_field_defaults() {
    // Config with partial station config (only radius, missing hit_area)
    let toml_str = r##"
[general]
auto_save = true
auto_save_interval_seconds = 30
recent_projects_max = 20

[units]
length = "meters"
depth = "meters"
angle = "degrees"
decimal_precision = 1

[display]
default_background = "#1a1a2e"
grid_color = "#333366"
font_size = 12
font_color = "#f5f5f5"

[display.depth_gradient]
enabled = false
shallow_color = "#00bfff"
deep_color = "#ff4444"
shallow_depth = 0.0
deep_depth = 50.0

[section_colors]
palette = ["#4ecdc4"]

[map]
satellite_provider = "openstreetmap"
satellite_enabled = false
zoom_default = 15
zoom_min = 0
zoom_max = 22

[map.controls]
pan_speed = 1.0
zoom_speed = 1.0
invert_scroll = false

[map.station]
radius = 8.0

[validation]
warn_shot_length_min = 0.1
warn_shot_length_max = 50.0
warn_depth_change_per_shot_max = 10.0
warn_azimuth_deviation_max = 30.0

[export]
default_image_format = "png"
default_dpi = 300
default_include_satellite = true
default_include_grid = false
default_include_labels = true

[logging]
level = "info"
max_directory_size_mb = 128

[shortcuts]
select = "S"
pan = "P"
zoom = "Z"
draw = "D"
delete = "X"
merge = "M"
measure = "R"
move_station = "T"
undo = "Ctrl+Z"
redo = "Ctrl+Y"
save = "Ctrl+S"
"##;

    let config: Config =
        toml::from_str(toml_str).expect("Config with partial station should parse");

    // Explicit value
    assert_eq!(config.map.station.radius, 8.0);
    // Missing field uses default
    assert_eq!(config.map.station.hit_area, 10.0);
}

#[test]
fn test_map_controls_show_navigation_tools_opt_in_parses_and_round_trips() {
    // #409: the setting is what puts the Pan/Zoom tools back, so an explicit
    // `true` has to survive both directions - reading the user's file, and
    // rewriting it when any other setting is saved.
    let controls: MapControlsConfig = toml::from_str(
        "pan_speed = 1.0\nzoom_speed = 1.0\ninvert_scroll = false\nshow_navigation_tools = true\n",
    )
    .expect("a [map.controls] table with show_navigation_tools should parse");
    assert!(controls.show_navigation_tools);

    let mut config = Config::default();
    config.map.controls.show_navigation_tools = true;
    let serialized = toml::to_string_pretty(&config).expect("Config should serialize");
    let deserialized: Config = toml::from_str(&serialized).expect("Serialized config should parse");
    assert!(deserialized.map.controls.show_navigation_tools);
}

// ========================================================================
// Config Serialization Tests
// ========================================================================

#[test]
fn test_config_serialization_roundtrip() {
    let original = Config::default();
    let serialized = toml::to_string_pretty(&original).expect("Config should serialize");
    let deserialized: Config = toml::from_str(&serialized).expect("Serialized config should parse");

    // Verify key values survived roundtrip
    assert_eq!(deserialized.map.station.radius, original.map.station.radius);
    assert_eq!(
        deserialized.map.station.hit_area,
        original.map.station.hit_area
    );
    assert_eq!(deserialized.map.shot.width, original.map.shot.width);
    assert_eq!(deserialized.map.shot.hit_area, original.map.shot.hit_area);
    assert_eq!(deserialized.map.snap.threshold, original.map.snap.threshold);
    assert_eq!(
        deserialized.debug.map.show_hitboxes,
        original.debug.map.show_hitboxes
    );
}

#[test]
fn test_config_serialization_preserves_custom_values() {
    let mut config = Config::default();
    config.map.station.radius = 8.0;
    config.map.station.hit_area = 16.0;
    config.map.shot.width = 4.0;
    config.map.shot.hit_area = 12.0;
    config.map.snap.threshold = 20.0;
    config.debug.map.show_hitboxes = true;

    let serialized = toml::to_string_pretty(&config).expect("Config should serialize");
    let deserialized: Config = toml::from_str(&serialized).expect("Serialized config should parse");

    assert_eq!(deserialized.map.station.radius, 8.0);
    assert_eq!(deserialized.map.station.hit_area, 16.0);
    assert_eq!(deserialized.map.shot.width, 4.0);
    assert_eq!(deserialized.map.shot.hit_area, 12.0);
    assert_eq!(deserialized.map.snap.threshold, 20.0);
    assert!(deserialized.debug.map.show_hitboxes);
}

#[test]
fn test_config_serialization_includes_map_section() {
    let config = Config::default();
    let serialized = toml::to_string_pretty(&config).expect("Config should serialize");

    assert!(serialized.contains("[map.station]"));
    assert!(serialized.contains("radius"));
    assert!(serialized.contains("hit_area"));
    assert!(serialized.contains("[map.shot]"));
    assert!(serialized.contains("width"));
    assert!(serialized.contains("[map.snap]"));
    assert!(serialized.contains("threshold"));
    assert!(serialized.contains("[debug.map]"));
    assert!(serialized.contains("show_hitboxes"));
}

// ========================================================================
// Legacy Migration Tests (existing)
// ========================================================================

#[test]
fn test_legacy_snap_migration() {
    let old_config_toml = r##"
[general]
auto_save = true
auto_save_interval_seconds = 30
recent_projects_max = 20

[units]
length = "meters"
depth = "meters"
angle = "degrees"
decimal_precision = 1

[display]
default_background = "#1a1a2e"
grid_color = "#333366"
font_size = 12
font_color = "#f5f5f5"

[display.depth_gradient]
enabled = false
shallow_color = "#00bfff"
deep_color = "#ff4444"
shallow_depth = 0.0
deep_depth = 50.0

[section_colors]
palette = ["#4ecdc4"]

[map]
satellite_provider = "openstreetmap"
satellite_enabled = false
zoom_default = 15
zoom_min = 0
zoom_max = 22

[map.controls]
pan_speed = 1.0
zoom_speed = 1.0
invert_scroll = false

[validation]
warn_shot_length_min = 0.1
warn_shot_length_max = 50.0
warn_depth_change_per_shot_max = 10.0
warn_azimuth_deviation_max = 30.0

[export]
default_image_format = "png"
default_dpi = 300
default_include_satellite = true
default_include_grid = false
default_include_labels = true

[logging]
level = "info"
max_directory_size_mb = 128

[shortcuts]
select = "S"
pan = "P"
zoom = "Z"
draw = "D"
delete = "X"
merge = "M"
measure = "R"
move_station = "T"
undo = "Ctrl+Z"
redo = "Ctrl+Y"
save = "Ctrl+S"

[snap]
enabled_draw = false
enabled_merge = true
enabled_measure = false
threshold_pixels = 25
"##;

    let mut config: Config =
        toml::from_str(old_config_toml).expect("Config with legacy [snap] should parse");

    // Before migration, map.snap has defaults
    assert_eq!(config.map.snap.threshold, 10.0);
    assert!(config.map.snap.enabled_merge);
    assert!(config.map.snap.enabled_measure);

    // legacy_snap should have the old values
    assert!(config.legacy_snap.is_some());

    // Run migration (simulating what Config::load does)
    config.migrate_legacy();
    config.normalize();

    // After migration, map.snap should have the legacy values
    assert_eq!(config.map.snap.threshold, 25.0);
    assert!(config.map.snap.enabled_merge);
    assert!(!config.map.snap.enabled_measure);

    // legacy_snap should be consumed (taken)
    assert!(config.legacy_snap.is_none());
}

#[test]
fn test_legacy_snap_not_migrated_when_new_config_present() {
    // If user has both old [snap] and new [map.snap], prefer new values
    let mixed_config_toml = r##"
[general]
auto_save = true
auto_save_interval_seconds = 30
recent_projects_max = 20

[units]
length = "meters"
depth = "meters"
angle = "degrees"
decimal_precision = 1

[display]
default_background = "#1a1a2e"
grid_color = "#333366"
font_size = 12
font_color = "#f5f5f5"

[display.depth_gradient]
enabled = false
shallow_color = "#00bfff"
deep_color = "#ff4444"
shallow_depth = 0.0
deep_depth = 50.0

[section_colors]
palette = ["#4ecdc4"]

[map]
satellite_provider = "openstreetmap"
satellite_enabled = false
zoom_default = 15
zoom_min = 0
zoom_max = 22

[map.controls]
pan_speed = 1.0
zoom_speed = 1.0
invert_scroll = false

[map.snap]
threshold = 30.0
enabled_draw = true
enabled_merge = false
enabled_measure = true

[validation]
warn_shot_length_min = 0.1
warn_shot_length_max = 50.0
warn_depth_change_per_shot_max = 10.0
warn_azimuth_deviation_max = 30.0

[export]
default_image_format = "png"
default_dpi = 300
default_include_satellite = true
default_include_grid = false
default_include_labels = true

[logging]
level = "info"
max_directory_size_mb = 128

[shortcuts]
select = "S"
pan = "P"
zoom = "Z"
draw = "D"
delete = "X"
merge = "M"
measure = "R"
move_station = "T"
undo = "Ctrl+Z"
redo = "Ctrl+Y"
save = "Ctrl+S"

[snap]
enabled_draw = false
enabled_merge = true
enabled_measure = false
threshold_pixels = 25
"##;

    let mut config: Config =
        toml::from_str(mixed_config_toml).expect("Config with both snap configs should parse");

    // map.snap has explicit values (not defaults)
    assert_eq!(config.map.snap.threshold, 30.0);
    assert!(!config.map.snap.enabled_merge);
    assert!(config.map.snap.enabled_measure);

    // Run migration
    config.migrate_legacy();
    config.normalize();

    // map.snap should NOT be overwritten by legacy values
    // (because map.snap was explicitly set, not defaults)
    assert_eq!(config.map.snap.threshold, 30.0);
    assert!(!config.map.snap.enabled_merge);
    assert!(config.map.snap.enabled_measure);
}

// ========================================================================
// Shortcut Deserializer Tests
// ========================================================================

#[test]
fn test_shortcut_deserialize_legacy_string() {
    // Test deserializing old format (single string)
    let toml_str = r#"
select = "S"
pan = "P"
"#;
    let config: ShortcutsConfig = toml::from_str(toml_str).expect("Legacy string should parse");
    assert_eq!(config.select, vec!["S".to_string()]);
    assert_eq!(config.pan, vec!["P".to_string()]);
}

#[test]
fn test_shortcut_deserialize_new_array() {
    // Test deserializing new format (array)
    let toml_str = r#"
select = ["S", "Shift+S"]
pan = ["P"]
"#;
    let config: ShortcutsConfig = toml::from_str(toml_str).expect("Array format should parse");
    assert_eq!(config.select, vec!["S".to_string(), "Shift+S".to_string()]);
    assert_eq!(config.pan, vec!["P".to_string()]);
}

#[test]
fn test_shortcut_deserialize_mixed_formats() {
    // Test deserializing mix of old and new formats
    let toml_str = r#"
select = "S"
pan = ["P", "Shift+P"]
zoom = "Z"
"#;
    let config: ShortcutsConfig = toml::from_str(toml_str).expect("Mixed format should parse");
    assert_eq!(config.select, vec!["S".to_string()]);
    assert_eq!(config.pan, vec!["P".to_string(), "Shift+P".to_string()]);
    assert_eq!(config.zoom, vec!["Z".to_string()]);
}

#[test]
fn test_shortcut_missing_fields_use_defaults() {
    // Test that missing fields get defaults via #[serde(default)]
    let toml_str = r#"
select = "S"
"#;
    let mut config: ShortcutsConfig =
        toml::from_str(toml_str).expect("Partial config should parse");
    config.normalize();

    // Explicit value
    assert_eq!(config.select, vec!["S".to_string()]);
    // Missing fields should have defaults after normalize
    assert_eq!(config.pan, vec!["P".to_string()]);
    assert_eq!(config.undo, vec!["Ctrl+Z".to_string()]);
    assert_eq!(
        config.zoom_in,
        vec!["Equal".to_string(), "Plus".to_string()]
    );
}

// ========================================================================
// Shortcut Normalization Tests
// ========================================================================

#[test]
fn test_normalize_shortcut_simple_key() {
    assert_eq!(normalize_shortcut("S"), Some("S".to_string()));
    assert_eq!(normalize_shortcut("s"), Some("S".to_string()));
    assert_eq!(normalize_shortcut("1"), Some("1".to_string()));
    assert_eq!(normalize_shortcut("Digit1"), Some("1".to_string()));
    assert_eq!(normalize_shortcut("Numpad1"), Some("1".to_string()));
}

#[test]
fn test_normalize_shortcut_with_modifiers() {
    assert_eq!(normalize_shortcut("Ctrl+S"), Some("Ctrl+S".to_string()));
    assert_eq!(normalize_shortcut("ctrl+s"), Some("Ctrl+S".to_string()));
    assert_eq!(normalize_shortcut("CTRL+S"), Some("Ctrl+S".to_string()));
    assert_eq!(
        normalize_shortcut("Ctrl+Digit1"),
        Some("Ctrl+1".to_string())
    );
    assert_eq!(
        normalize_shortcut("Ctrl+Numpad1"),
        Some("Ctrl+1".to_string())
    );
}

#[test]
fn test_normalize_shortcut_modifier_order() {
    // Modifiers should always be Ctrl+Alt+Shift+Key
    assert_eq!(
        normalize_shortcut("Shift+Ctrl+S"),
        Some("Ctrl+Shift+S".to_string())
    );
    assert_eq!(
        normalize_shortcut("Alt+Ctrl+S"),
        Some("Ctrl+Alt+S".to_string())
    );
    assert_eq!(
        normalize_shortcut("Shift+Alt+Ctrl+S"),
        Some("Ctrl+Alt+Shift+S".to_string())
    );
}

#[test]
fn test_normalize_shortcut_platform_aliases() {
    // Meta/Cmd/Command should normalize to Ctrl
    assert_eq!(normalize_shortcut("Meta+S"), Some("Ctrl+S".to_string()));
    assert_eq!(normalize_shortcut("Cmd+S"), Some("Ctrl+S".to_string()));
    assert_eq!(normalize_shortcut("Command+S"), Some("Ctrl+S".to_string()));
    assert_eq!(normalize_shortcut("Option+S"), Some("Alt+S".to_string()));
}

#[test]
fn test_normalize_shortcut_special_keys() {
    assert_eq!(normalize_shortcut("Space"), Some("Space".to_string()));
    assert_eq!(normalize_shortcut("space"), Some("Space".to_string()));
    assert_eq!(normalize_shortcut("Enter"), Some("Enter".to_string()));
    assert_eq!(normalize_shortcut("Return"), Some("Enter".to_string()));
    assert_eq!(normalize_shortcut("Escape"), Some("Escape".to_string()));
    assert_eq!(normalize_shortcut("Esc"), Some("Escape".to_string()));
    assert_eq!(normalize_shortcut("ArrowUp"), Some("ArrowUp".to_string()));
    assert_eq!(normalize_shortcut("Up"), Some("ArrowUp".to_string()));
    assert_eq!(
        normalize_shortcut("ArrowDown"),
        Some("ArrowDown".to_string())
    );
    assert_eq!(normalize_shortcut("Down"), Some("ArrowDown".to_string()));
}

#[test]
fn test_normalize_shortcut_plus_minus_equal() {
    assert_eq!(normalize_shortcut("+"), Some("Plus".to_string()));
    assert_eq!(normalize_shortcut("Plus"), Some("Plus".to_string()));
    assert_eq!(normalize_shortcut("-"), Some("Minus".to_string()));
    assert_eq!(normalize_shortcut("Minus"), Some("Minus".to_string()));
    assert_eq!(normalize_shortcut("="), Some("Equal".to_string()));
    assert_eq!(normalize_shortcut("Equal"), Some("Equal".to_string()));
}

#[test]
fn test_normalize_shortcut_function_keys() {
    assert_eq!(normalize_shortcut("F1"), Some("F1".to_string()));
    assert_eq!(normalize_shortcut("f1"), Some("F1".to_string()));
    assert_eq!(normalize_shortcut("F12"), Some("F12".to_string()));
}

#[test]
fn test_normalize_shortcut_invalid() {
    // Empty string
    assert_eq!(normalize_shortcut(""), None);
    // Only whitespace
    assert_eq!(normalize_shortcut("   "), None);
    // Multiple non-modifier keys
    assert_eq!(normalize_shortcut("A+B"), None);
    // Empty parts in middle
    assert_eq!(normalize_shortcut("Ctrl++S"), None);
}

#[test]
fn test_normalize_shortcut_trailing_plus() {
    // "Ctrl+" is valid and means Ctrl+Plus
    assert_eq!(normalize_shortcut("Ctrl+"), Some("Ctrl+Plus".to_string()));
    assert_eq!(normalize_shortcut("Shift+"), Some("Shift+Plus".to_string()));
    // "Ctrl++" is also valid and means Ctrl+Plus (common representation)
    assert_eq!(normalize_shortcut("Ctrl++"), Some("Ctrl+Plus".to_string()));
    assert_eq!(
        normalize_shortcut("Shift++"),
        Some("Shift+Plus".to_string())
    );
    // Multiple modifiers with trailing +
    assert_eq!(
        normalize_shortcut("Ctrl+Shift+"),
        Some("Ctrl+Shift+Plus".to_string())
    );
    assert_eq!(
        normalize_shortcut("Ctrl+Shift++"),
        Some("Ctrl+Shift+Plus".to_string())
    );
}

#[test]
fn test_normalize_shortcut_remaining_special_keys() {
    // Arrow keys (all directions)
    assert_eq!(
        normalize_shortcut("ArrowLeft"),
        Some("ArrowLeft".to_string())
    );
    assert_eq!(
        normalize_shortcut("arrowleft"),
        Some("ArrowLeft".to_string())
    );
    assert_eq!(normalize_shortcut("Left"), Some("ArrowLeft".to_string()));
    assert_eq!(
        normalize_shortcut("ArrowRight"),
        Some("ArrowRight".to_string())
    );
    assert_eq!(
        normalize_shortcut("arrowright"),
        Some("ArrowRight".to_string())
    );
    assert_eq!(normalize_shortcut("Right"), Some("ArrowRight".to_string()));

    // Backspace
    assert_eq!(
        normalize_shortcut("Backspace"),
        Some("Backspace".to_string())
    );
    assert_eq!(
        normalize_shortcut("backspace"),
        Some("Backspace".to_string())
    );
    assert_eq!(normalize_shortcut("Back"), Some("Backspace".to_string()));

    // Delete
    assert_eq!(normalize_shortcut("Delete"), Some("Delete".to_string()));
    assert_eq!(normalize_shortcut("delete"), Some("Delete".to_string()));
    assert_eq!(normalize_shortcut("Del"), Some("Delete".to_string()));

    // Tab
    assert_eq!(normalize_shortcut("Tab"), Some("Tab".to_string()));
    assert_eq!(normalize_shortcut("tab"), Some("Tab".to_string()));

    // Home/End
    assert_eq!(normalize_shortcut("Home"), Some("Home".to_string()));
    assert_eq!(normalize_shortcut("End"), Some("End".to_string()));

    // Page Up/Down
    assert_eq!(normalize_shortcut("PageUp"), Some("PageUp".to_string()));
    assert_eq!(normalize_shortcut("pgup"), Some("PageUp".to_string()));
    assert_eq!(normalize_shortcut("PageDown"), Some("PageDown".to_string()));
    assert_eq!(normalize_shortcut("pgdn"), Some("PageDown".to_string()));

    // Insert
    assert_eq!(normalize_shortcut("Insert"), Some("Insert".to_string()));
    assert_eq!(normalize_shortcut("Ins"), Some("Insert".to_string()));
}

#[test]
fn test_normalize_shortcut_special_keys_with_modifiers() {
    assert_eq!(
        normalize_shortcut("Ctrl+ArrowLeft"),
        Some("Ctrl+ArrowLeft".to_string())
    );
    assert_eq!(
        normalize_shortcut("Shift+Delete"),
        Some("Shift+Delete".to_string())
    );
    assert_eq!(
        normalize_shortcut("Ctrl+Backspace"),
        Some("Ctrl+Backspace".to_string())
    );
    assert_eq!(normalize_shortcut("Alt+Tab"), Some("Alt+Tab".to_string()));
    assert_eq!(
        normalize_shortcut("Ctrl+Home"),
        Some("Ctrl+Home".to_string())
    );
    assert_eq!(normalize_shortcut("Ctrl+End"), Some("Ctrl+End".to_string()));
}

#[test]
fn test_shortcut_config_normalize_truncates() {
    let mut config = ShortcutsConfig::default();
    config.select = vec!["A".to_string(), "B".to_string(), "C".to_string()];
    config.normalize();
    // Should truncate to 2
    assert_eq!(config.select.len(), 2);
    assert_eq!(config.select, vec!["A".to_string(), "B".to_string()]);
}

#[test]
fn test_shortcut_config_normalize_removes_empty() {
    let mut config = ShortcutsConfig::default();
    config.select = vec![String::new(), "S".to_string(), "  ".to_string()];
    config.normalize();
    // Should remove empty and only keep "S"
    assert_eq!(config.select, vec!["S".to_string()]);
}

#[test]
fn test_shortcut_config_normalize_empty_uses_default() {
    let mut config = ShortcutsConfig::default();
    config.select = vec![];
    config.normalize();
    // Should use default
    assert_eq!(config.select, vec!["S".to_string()]);
}

#[test]
fn test_shortcut_config_normalize_invalid_falls_back() {
    let mut config = ShortcutsConfig::default();
    config.select = vec!["A+B".to_string()]; // Invalid (two non-modifier keys)
    config.normalize();
    // Should fall back to default
    assert_eq!(config.select, vec!["S".to_string()]);
}

// ========================================================================
// Legacy Delete Migration Tests
// ========================================================================

// ========================================================================
// New Actions Tests
// ========================================================================

#[test]
fn test_shortcut_new_actions_defaults() {
    let config = ShortcutsConfig::default();

    // Verify new actions have defaults
    assert_eq!(config.toggle_station_labels, vec!["L".to_string()]);
    assert_eq!(config.toggle_shot_labels, vec!["Shift+L".to_string()]);
    assert_eq!(config.toggle_section_labels, vec!["Ctrl+L".to_string()]);
    assert_eq!(
        config.zoom_in,
        vec!["Equal".to_string(), "Plus".to_string()]
    );
    assert_eq!(config.zoom_out, vec!["Minus".to_string()]);
    assert_eq!(config.toggle_depth_gradient, vec!["Shift+D".to_string()]);
    assert_eq!(config.move_segment, vec!["V".to_string()]);
    assert_eq!(config.move_shot, vec!["G".to_string()]);
    assert_eq!(config.toggle_grid, vec!["Shift+G".to_string()]);
    assert_eq!(config.toggle_satellite, vec!["B".to_string()]);
    assert_eq!(config.fit_to_bounds, vec!["F".to_string()]);
    assert_eq!(config.toggle_sections_panel, vec!["Ctrl+1".to_string()]);
    assert_eq!(config.toggle_detail_panel, vec!["Ctrl+2".to_string()]);
    assert_eq!(config.toggle_data_table, vec!["Ctrl+3".to_string()]);
}

#[test]
fn test_shortcut_config_serialization_roundtrip() {
    let original = ShortcutsConfig::default();
    let serialized = toml::to_string_pretty(&original).expect("Should serialize");
    let deserialized: ShortcutsConfig = toml::from_str(&serialized).expect("Should deserialize");

    assert_eq!(deserialized.select, original.select);
    assert_eq!(deserialized.zoom_in, original.zoom_in);
    assert_eq!(
        deserialized.toggle_station_labels,
        original.toggle_station_labels
    );
}

// ========================================================================
// ShortcutFeedback Enum Tests
// ========================================================================

#[test]
fn test_shortcut_feedback_default() {
    // Default should be NonObvious
    let feedback = ShortcutFeedback::default();
    assert_eq!(feedback, ShortcutFeedback::NonObvious);
}

#[test]
fn test_shortcut_feedback_serialize_none() {
    // TOML requires key-value pairs, so use wrapper struct
    #[derive(Serialize)]
    struct Wrapper {
        value: ShortcutFeedback,
    }
    let w = Wrapper {
        value: ShortcutFeedback::None,
    };
    let serialized = toml::to_string(&w).expect("Should serialize");
    assert!(serialized.contains("value = \"none\""));
}

#[test]
fn test_shortcut_feedback_serialize_non_obvious() {
    #[derive(Serialize)]
    struct Wrapper {
        value: ShortcutFeedback,
    }
    let w = Wrapper {
        value: ShortcutFeedback::NonObvious,
    };
    let serialized = toml::to_string(&w).expect("Should serialize");
    assert!(serialized.contains("value = \"non_obvious\""));
}

#[test]
fn test_shortcut_feedback_serialize_all() {
    #[derive(Serialize)]
    struct Wrapper {
        value: ShortcutFeedback,
    }
    let w = Wrapper {
        value: ShortcutFeedback::All,
    };
    let serialized = toml::to_string(&w).expect("Should serialize");
    assert!(serialized.contains("value = \"all\""));
}

#[test]
fn test_shortcut_feedback_deserialize_none() {
    #[derive(Deserialize)]
    struct Wrapper {
        value: ShortcutFeedback,
    }
    let w: Wrapper = toml::from_str("value = \"none\"").expect("Should deserialize");
    assert_eq!(w.value, ShortcutFeedback::None);
}

#[test]
fn test_shortcut_feedback_deserialize_non_obvious() {
    #[derive(Deserialize)]
    struct Wrapper {
        value: ShortcutFeedback,
    }
    let w: Wrapper = toml::from_str("value = \"non_obvious\"").expect("Should deserialize");
    assert_eq!(w.value, ShortcutFeedback::NonObvious);
}

#[test]
fn test_shortcut_feedback_deserialize_all() {
    #[derive(Deserialize)]
    struct Wrapper {
        value: ShortcutFeedback,
    }
    let w: Wrapper = toml::from_str("value = \"all\"").expect("Should deserialize");
    assert_eq!(w.value, ShortcutFeedback::All);
}

#[test]
fn test_general_config_shortcut_feedback_default() {
    // When shortcut_feedback is missing from TOML, should use default
    let toml_str = r"
auto_save = true
auto_save_interval_seconds = 30
recent_projects_max = 20
";
    let config: GeneralConfig = toml::from_str(toml_str).expect("Should deserialize");
    assert_eq!(config.shortcut_feedback, ShortcutFeedback::NonObvious);
    assert_eq!(config.locale, "system");
}

#[test]
fn test_general_config_shortcut_feedback_explicit() {
    let toml_str = r#"
auto_save = true
auto_save_interval_seconds = 30
recent_projects_max = 20
shortcut_feedback = "all"
"#;
    let config: GeneralConfig = toml::from_str(toml_str).expect("Should deserialize");
    assert_eq!(config.shortcut_feedback, ShortcutFeedback::All);
}

#[test]
fn test_general_config_locale_explicit() {
    let toml_str = r#"
auto_save = true
auto_save_interval_seconds = 30
recent_projects_max = 20
locale = "en-US"
"#;
    let config: GeneralConfig = toml::from_str(toml_str).expect("Should deserialize");
    assert_eq!(config.locale, "en-US");
}

#[test]
fn test_general_config_shortcut_feedback_roundtrip() {
    let mut config = GeneralConfig::default();
    config.shortcut_feedback = ShortcutFeedback::None;

    let serialized = toml::to_string_pretty(&config).expect("Should serialize");
    let deserialized: GeneralConfig = toml::from_str(&serialized).expect("Should deserialize");

    assert_eq!(deserialized.shortcut_feedback, ShortcutFeedback::None);
}

// ========================================================================
// UpdateTrack / update-check Tests (#579)
// ========================================================================

#[test]
fn test_update_track_default_follows_the_build_channel() {
    // `option_env!` is read when this crate is COMPILED, so the answer here is
    // a property of how the test binary was built: an ordinary build has no
    // STALAGMIGHT_CHANNEL and must land on Stable, a candidate CI build compiles
    // the same expression with `beta` (or the legacy `weekly`) and must land on
    // Beta. Pinning both arms is what catches the default being flipped outright.
    let expected = match option_env!("STALAGMIGHT_CHANNEL") {
        Some("beta" | "weekly") => UpdateTrack::Beta,
        _ => UpdateTrack::Stable,
    };
    assert_eq!(UpdateTrack::default(), expected);
    assert_eq!(GeneralConfig::default().update_track, expected);
}

#[test]
fn test_update_track_serializes_snake_case() {
    #[derive(Serialize)]
    struct Wrapper {
        value: UpdateTrack,
    }
    for (value, expected) in [
        (UpdateTrack::Stable, "value = \"stable\""),
        (UpdateTrack::Beta, "value = \"beta\""),
    ] {
        let serialized = toml::to_string(&Wrapper { value }).expect("Should serialize");
        assert!(
            serialized.contains(expected),
            "expected {expected} in {serialized}"
        );
    }
}

#[test]
fn test_update_track_deserializes_snake_case() {
    #[derive(Deserialize)]
    struct Wrapper {
        value: UpdateTrack,
    }
    for (input, expected) in [
        ("value = \"stable\"", UpdateTrack::Stable),
        ("value = \"beta\"", UpdateTrack::Beta),
    ] {
        let w: Wrapper = toml::from_str(input).expect("Should deserialize");
        assert_eq!(w.value, expected);
    }
}

/// [#650] 0.5.0 and 0.5.1 wrote `weekly` for the track Beta replaces. Such a
/// config must still load - a rejected value fails the whole file, not just
/// this field - land on Beta, and be written back under the new name.
#[test]
fn test_update_track_reads_the_legacy_weekly_value_as_beta() {
    #[derive(Serialize, Deserialize)]
    struct Wrapper {
        value: UpdateTrack,
    }
    let w: Wrapper = toml::from_str("value = \"weekly\"").expect("Should deserialize");
    assert_eq!(w.value, UpdateTrack::Beta);
    let rewritten = toml::to_string(&w).expect("Should serialize");
    assert!(
        rewritten.contains("value = \"beta\""),
        "expected beta in {rewritten}"
    );

    // ...and in the shape 0.5.1 actually saved, next to the other settings.
    let saved_by_051 = r#"
auto_save = true
auto_save_interval_seconds = 30
recent_projects_max = 20
update_track = "weekly"
"#;
    let config: GeneralConfig = toml::from_str(saved_by_051).expect("Should deserialize");
    assert_eq!(config.update_track, UpdateTrack::Beta);
}

/// An unrecognised track is REJECTED, not silently coerced - the same contract
/// `ShortcutFeedback` and every other config enum has (no `#[serde(other)]`
/// arm). Pinned because a future `other` arm would turn a typo in the config
/// file into a silent switch of release track.
#[test]
fn test_update_track_rejects_an_unknown_value() {
    #[derive(Debug, Deserialize)]
    struct Wrapper {
        #[expect(dead_code)]
        value: UpdateTrack,
    }
    let err = toml::from_str::<Wrapper>("value = \"nightly\"").expect_err("Should reject");
    assert!(
        err.to_string().contains("nightly") || err.to_string().contains("unknown variant"),
        "unhelpful error: {err}"
    );
}

#[test]
fn test_general_config_update_fields_default_when_absent() {
    let toml_str = r"
auto_save = true
auto_save_interval_seconds = 30
recent_projects_max = 20
";
    let config: GeneralConfig = toml::from_str(toml_str).expect("Should deserialize");
    assert_eq!(config.update_track, UpdateTrack::default());
    // The check is opt-OUT: a config written before this field existed must
    // come back with it on, not off.
    assert!(config.update_check_enabled);
}

#[test]
fn test_general_config_update_fields_explicit() {
    let toml_str = r#"
auto_save = true
auto_save_interval_seconds = 30
recent_projects_max = 20
update_track = "beta"
update_check_enabled = false
"#;
    let config: GeneralConfig = toml::from_str(toml_str).expect("Should deserialize");
    assert_eq!(config.update_track, UpdateTrack::Beta);
    assert!(!config.update_check_enabled);
}

#[test]
fn test_general_config_update_fields_roundtrip() {
    let mut config = GeneralConfig::default();
    config.update_track = UpdateTrack::Beta;
    config.update_check_enabled = false;

    let serialized = toml::to_string_pretty(&config).expect("Should serialize");
    let deserialized: GeneralConfig = toml::from_str(&serialized).expect("Should deserialize");

    assert_eq!(deserialized.update_track, UpdateTrack::Beta);
    assert!(!deserialized.update_check_enabled);

    // ...and the other direction, so a stable/enabled pair is not just the
    // struct default leaking through a dropped field.
    let mut config = GeneralConfig::default();
    config.update_track = UpdateTrack::Stable;
    config.update_check_enabled = true;
    let serialized = toml::to_string_pretty(&config).expect("Should serialize");
    assert!(serialized.contains("update_track = \"stable\""));
    assert!(serialized.contains("update_check_enabled = true"));
}

// ========================================================================
// File I/O Tests
// ========================================================================

#[test]
fn test_load_from_path_missing_returns_default() {
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let config = Config::load_from_path(&path).unwrap();
    assert_eq!(config.general.auto_save_interval_seconds, 30);
    assert_eq!(config.general.locale, "system");
    assert_eq!(config.validation.warn_shot_length_max, 50.0);
}

#[test]
fn test_save_to_path_and_load_roundtrip() {
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let mut config = Config::default();
    config.general.auto_save = false;
    config.general.undo_history_limit = 42;
    config.display.grid_spacing = 7.5;

    config.save_to_path(&path).unwrap();
    let loaded = Config::load_from_path(&path).unwrap();

    assert!(!loaded.general.auto_save);
    assert_eq!(loaded.general.undo_history_limit, 42);
    assert_eq!(loaded.display.grid_spacing, 7.5);
}

#[test]
fn test_column_widths_survive_a_save_load_roundtrip() {
    // The property #334 is actually about: a width set in one session is still
    // there in the next. That crosses a process boundary, so it has to be proven
    // against a real file rather than the in-memory config an IPC round trip
    // reads back.
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let mut config = Config::default();
    config.panels.data_table_shot_column_widths = std::collections::BTreeMap::from([
        ("length".to_string(), 210),
        ("comment".to_string(), 480),
    ]);
    config.panels.data_table_station_column_widths =
        std::collections::BTreeMap::from([("_zeroDatum".to_string(), 96)]);

    config.save_to_path(&path).unwrap();
    let loaded = Config::load_from_path(&path).unwrap();

    assert_eq!(loaded.panels.data_table_shot_column_widths["length"], 210);
    assert_eq!(loaded.panels.data_table_shot_column_widths["comment"], 480);
    assert_eq!(
        loaded.panels.data_table_station_column_widths["_zeroDatum"],
        96
    );
}

#[test]
fn test_non_default_webkit_memory_survives_save_load_roundtrip() {
    // Regression: `webkit_memory` is `#[serde(skip_serializing)]`, so
    // `toml::to_string_pretty` drops it. `save_to_path` must re-append the
    // `[webkit_memory]` table when it is non-default, otherwise a hand-tuned knob
    // is wiped from disk on the next save (which fires on every project
    // open/create/save). Prove the value survives a write -> load -> save -> load.
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");

    // Write a config.toml carrying a NON-default [webkit_memory].
    let mut config = Config::default();
    config.webkit_memory.memory_limit_mb = 1234;
    config.webkit_memory.jsc_force_ram_size_mb = 4096;
    config.save_to_path(&path).unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(
        raw.contains("[webkit_memory]"),
        "a non-default webkit_memory must be re-appended on save: {raw}"
    );

    // First load: the non-default value made it to disk and back.
    let loaded = Config::load_from_path(&path).unwrap();
    assert_eq!(loaded.webkit_memory.memory_limit_mb, 1234);
    assert_eq!(loaded.webkit_memory.jsc_force_ram_size_mb, 4096);

    // Round-trip again (save the loaded config, reload) to prove a plain save
    // does not silently reset the knob.
    loaded.save_to_path(&path).unwrap();
    let reloaded = Config::load_from_path(&path).unwrap();
    assert_eq!(
        reloaded.webkit_memory.memory_limit_mb, 1234,
        "non-default memory_limit_mb must survive a save/load round-trip"
    );
    assert_eq!(reloaded.webkit_memory.jsc_force_ram_size_mb, 4096);
}

#[test]
fn test_default_webkit_memory_emits_no_section() {
    // A DEFAULT webkit_memory must NOT emit a `[webkit_memory]` table (it stays
    // backend-only, out of the frontend-typed serialization) and still round-trips
    // to the default.
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let config = Config::default();
    config.save_to_path(&path).unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(
        !raw.contains("[webkit_memory]"),
        "a default webkit_memory must not be serialized: {raw}"
    );

    let reloaded = Config::load_from_path(&path).unwrap();
    assert_eq!(
        reloaded.webkit_memory,
        WebkitMemoryConfig::default(),
        "a config with no [webkit_memory] section loads the default"
    );
}

#[test]
fn test_load_from_path_invalid_toml_errors() {
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "not = [valid").unwrap();
    assert!(Config::load_from_path(&path).is_err());
}

// ========================================================================
// Appearance Config Tests
// ========================================================================

#[test]
fn test_appearance_config_defaults() {
    let config = AppearanceConfig::default();
    assert_eq!(config.theme, ThemeMode::Dark);
}

#[test]
fn test_theme_mode_deserialize_valid_values() {
    #[derive(Deserialize)]
    struct Wrapper {
        theme: ThemeMode,
    }
    let dark: Wrapper = toml::from_str("theme = \"dark\"").expect("dark should parse");
    assert_eq!(dark.theme, ThemeMode::Dark);
    let light: Wrapper = toml::from_str("theme = \"light\"").expect("light should parse");
    assert_eq!(light.theme, ThemeMode::Light);
    let auto: Wrapper = toml::from_str("theme = \"auto\"").expect("auto should parse");
    assert_eq!(auto.theme, ThemeMode::Auto);
}

#[test]
fn test_theme_mode_deserialize_unknown_value() {
    // Unknown theme values should deserialize as Unknown (not error)
    #[derive(Deserialize)]
    struct Wrapper {
        theme: ThemeMode,
    }
    let unknown: Wrapper =
        toml::from_str("theme = \"neon\"").expect("unknown theme should parse via #[serde(other)]");
    assert_eq!(unknown.theme, ThemeMode::Unknown);
}

#[test]
fn test_appearance_normalize_unknown_to_dark() {
    let mut config = AppearanceConfig {
        theme: ThemeMode::Unknown,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.theme, ThemeMode::Dark);
}

#[test]
fn test_appearance_normalize_valid_themes_unchanged() {
    let mut dark = AppearanceConfig {
        theme: ThemeMode::Dark,
        ..Default::default()
    };
    dark.normalize();
    assert_eq!(dark.theme, ThemeMode::Dark);

    let mut light = AppearanceConfig {
        theme: ThemeMode::Light,
        ..Default::default()
    };
    light.normalize();
    assert_eq!(light.theme, ThemeMode::Light);

    let mut auto = AppearanceConfig {
        theme: ThemeMode::Auto,
        ..Default::default()
    };
    auto.normalize();
    assert_eq!(auto.theme, ThemeMode::Auto);
}

#[test]
fn test_config_without_appearance_section_loads() {
    // Existing config without [appearance] should parse and default to dark
    let toml_str = r##"
[general]
auto_save = true
auto_save_interval_seconds = 30
recent_projects_max = 20

[units]
length = "meters"
depth = "meters"
angle = "degrees"
decimal_precision = 1

[display]
default_background = "#1a1a2e"
grid_color = "#333366"
font_size = 12
font_color = "#f5f5f5"

[display.depth_gradient]
enabled = false
shallow_color = "#00bfff"
deep_color = "#ff4444"
shallow_depth = 0.0
deep_depth = 50.0

[section_colors]
palette = ["#4ecdc4"]

[map]
satellite_provider = "esri"
satellite_enabled = false
zoom_default = 15
zoom_min = 0
zoom_max = 22

[map.controls]
pan_speed = 1.0
zoom_speed = 1.0
invert_scroll = false

[validation]
warn_shot_length_min = 0.1
warn_shot_length_max = 50.0
warn_depth_change_per_shot_max = 10.0
warn_azimuth_deviation_max = 30.0

[export]
default_image_format = "png"
default_dpi = 300
default_include_satellite = true
default_include_grid = false
default_include_labels = true

[logging]
level = "info"
max_directory_size_mb = 128

[shortcuts]
select = "S"
pan = "P"
zoom = "Z"
draw = "D"
delete = "X"
merge = "M"
measure = "R"
move_station = "T"
undo = "Ctrl+Z"
redo = "Ctrl+Y"
save = "Ctrl+S"
"##;

    let config: Config =
        toml::from_str(toml_str).expect("Config without [appearance] should parse");
    assert_eq!(config.appearance.theme, ThemeMode::Dark);
}

#[test]
fn test_config_with_invalid_theme_normalizes() {
    let toml_str = r##"
[general]
auto_save = true
auto_save_interval_seconds = 30
recent_projects_max = 20

[units]
length = "meters"
depth = "meters"
angle = "degrees"
decimal_precision = 1

[display]
default_background = "#1a1a2e"
grid_color = "#333366"
font_size = 12
font_color = "#f5f5f5"

[display.depth_gradient]
enabled = false
shallow_color = "#00bfff"
deep_color = "#ff4444"
shallow_depth = 0.0
deep_depth = 50.0

[section_colors]
palette = ["#4ecdc4"]

[map]
satellite_provider = "esri"
satellite_enabled = false
zoom_default = 15
zoom_min = 0
zoom_max = 22

[map.controls]
pan_speed = 1.0
zoom_speed = 1.0
invert_scroll = false

[validation]
warn_shot_length_min = 0.1
warn_shot_length_max = 50.0
warn_depth_change_per_shot_max = 10.0
warn_azimuth_deviation_max = 30.0

[export]
default_image_format = "png"
default_dpi = 300
default_include_satellite = true
default_include_grid = false
default_include_labels = true

[logging]
level = "info"
max_directory_size_mb = 128

[shortcuts]
select = "S"
pan = "P"
zoom = "Z"
draw = "D"
delete = "X"
merge = "M"
measure = "R"
move_station = "T"
undo = "Ctrl+Z"
redo = "Ctrl+Y"
save = "Ctrl+S"

[appearance]
theme = "neon"
"##;

    let mut config: Config =
        toml::from_str(toml_str).expect("Config with invalid theme should still parse");
    assert_eq!(config.appearance.theme, ThemeMode::Unknown);
    config.normalize();
    assert_eq!(config.appearance.theme, ThemeMode::Dark);
}

// ========================================================================
// Satellite Provider Normalization Tests
// ========================================================================

#[test]
fn test_map_config_normalize_unknown_provider_to_esri() {
    let mut config = MapConfig {
        satellite_provider: "unknown_provider".to_string(),
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.satellite_provider, "esri");
}

#[test]
fn test_map_config_normalize_valid_provider_unchanged() {
    let mut config = MapConfig {
        satellite_provider: "esri".to_string(),
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.satellite_provider, "esri");
}

#[test]
fn test_map_config_normalize_empty_provider_to_esri() {
    let mut config = MapConfig {
        satellite_provider: String::new(),
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.satellite_provider, "esri");
}

// ========================================================================
// DecimalPrecision Deserialization Tests (backward-compatibility)
// ========================================================================

#[test]
fn test_decimal_precision_deserialize_bare_u8() {
    // Old format: decimal_precision = 2  →  all fields set to 2
    let toml_str = r#"
length = "meters"
depth = "meters"
angle = "degrees"
decimal_precision = 2
"#;
    let config: UnitsConfig = toml::from_str(toml_str).expect("Bare u8 should parse");
    assert_eq!(config.decimal_precision.length, 2);
    assert_eq!(config.decimal_precision.coordinates, 2);
    assert_eq!(config.decimal_precision.depth, 2);
    assert_eq!(config.decimal_precision.azimuth, 2);
    assert_eq!(config.decimal_precision.inclination, 2);
    assert_eq!(config.decimal_precision.gps, 2);
}

#[test]
fn test_decimal_precision_deserialize_bare_u8_zero() {
    let toml_str = r#"
length = "meters"
depth = "meters"
angle = "degrees"
decimal_precision = 0
"#;
    let config: UnitsConfig = toml::from_str(toml_str).expect("Bare u8 zero should parse");
    assert_eq!(config.decimal_precision.length, 0);
    assert_eq!(config.decimal_precision.gps, 0);
}

#[test]
fn test_decimal_precision_deserialize_struct_format() {
    // New format: individual fields
    let toml_str = r#"
length = "meters"
depth = "meters"
angle = "degrees"

[decimal_precision]
length = 1
coordinates = 2
depth = 1
azimuth = 1
inclination = 1
gps = 6
"#;
    let config: UnitsConfig = toml::from_str(toml_str).expect("Struct format should parse");
    assert_eq!(config.decimal_precision.length, 1);
    assert_eq!(config.decimal_precision.coordinates, 2);
    assert_eq!(config.decimal_precision.depth, 1);
    assert_eq!(config.decimal_precision.gps, 6);
}

#[test]
fn test_decimal_precision_deserialize_struct_partial_uses_field_defaults() {
    // New struct format with only some fields → missing ones use serde defaults
    let toml_str = r#"
length = "meters"
depth = "meters"
angle = "degrees"

[decimal_precision]
length = 3
gps = 4
"#;
    let config: UnitsConfig = toml::from_str(toml_str).expect("Partial struct should parse");
    assert_eq!(config.decimal_precision.length, 3);
    assert_eq!(config.decimal_precision.coordinates, 2); // default
    assert_eq!(config.decimal_precision.depth, 1); // default
    assert_eq!(config.decimal_precision.azimuth, 1); // default
    assert_eq!(config.decimal_precision.inclination, 1); // default
    assert_eq!(config.decimal_precision.gps, 4);
}

#[test]
fn test_decimal_precision_roundtrip_struct_format() {
    let original = UnitsConfig {
        length: LengthUnit::Meters,
        depth: LengthUnit::Meters,
        angle: AngleUnit::Degrees,
        decimal_precision: DecimalPrecision {
            length: 3,
            coordinates: 4,
            depth: 2,
            azimuth: 1,
            inclination: 0,
            gps: 5,
        },
    };
    let serialized = toml::to_string_pretty(&original).expect("Should serialize");
    let deserialized: UnitsConfig = toml::from_str(&serialized).expect("Should deserialize");
    assert_eq!(deserialized.decimal_precision.length, 3);
    assert_eq!(deserialized.decimal_precision.coordinates, 4);
    assert_eq!(deserialized.decimal_precision.depth, 2);
    assert_eq!(deserialized.decimal_precision.azimuth, 1);
    assert_eq!(deserialized.decimal_precision.inclination, 0);
    assert_eq!(deserialized.decimal_precision.gps, 5);
}

// ========================================================================
// DecimalPrecision Normalize Tests
// ========================================================================

#[test]
fn test_decimal_precision_normalize_clamps_to_six() {
    let mut dp = DecimalPrecision {
        length: 10,
        coordinates: 7,
        depth: 255,
        azimuth: 8,
        inclination: 100,
        gps: 6,
    };
    dp.normalize();
    assert_eq!(dp.length, 6);
    assert_eq!(dp.coordinates, 6);
    assert_eq!(dp.depth, 6);
    assert_eq!(dp.azimuth, 6);
    assert_eq!(dp.inclination, 6);
    assert_eq!(dp.gps, 6); // already at max, unchanged
}

#[test]
fn test_decimal_precision_normalize_preserves_valid_values() {
    let mut dp = DecimalPrecision {
        length: 0,
        coordinates: 3,
        depth: 6,
        azimuth: 1,
        inclination: 2,
        gps: 5,
    };
    dp.normalize();
    assert_eq!(dp.length, 0);
    assert_eq!(dp.coordinates, 3);
    assert_eq!(dp.depth, 6);
    assert_eq!(dp.azimuth, 1);
    assert_eq!(dp.inclination, 2);
    assert_eq!(dp.gps, 5);
}

#[test]
fn test_decimal_precision_normalize_is_idempotent() {
    let mut dp = DecimalPrecision {
        length: 10,
        coordinates: 3,
        depth: 0,
        azimuth: 6,
        inclination: 255,
        gps: 4,
    };
    dp.normalize();
    let first = dp.clone();
    dp.normalize();
    assert_eq!(dp.length, first.length);
    assert_eq!(dp.coordinates, first.coordinates);
    assert_eq!(dp.depth, first.depth);
    assert_eq!(dp.azimuth, first.azimuth);
    assert_eq!(dp.inclination, first.inclination);
    assert_eq!(dp.gps, first.gps);
}

// ========================================================================
// GlobalViewConfig Normalize Tests
// ========================================================================

#[test]
fn test_global_view_normalize_nan_tolerance_resets_to_default() {
    let mut config = GlobalViewConfig {
        tolerance: f64::NAN,
        throttle_tolerance: f64::NAN,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.tolerance, 0.5); // default
    assert_eq!(config.throttle_tolerance, 1.0); // default
}

#[test]
fn test_global_view_normalize_infinity_resets_to_default() {
    let mut config = GlobalViewConfig {
        tolerance: f64::INFINITY,
        throttle_tolerance: f64::NEG_INFINITY,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.tolerance, 0.5);
    assert_eq!(config.throttle_tolerance, 1.0);
}

#[test]
fn test_global_view_normalize_negative_clamped_to_zero() {
    let mut config = GlobalViewConfig {
        tolerance: -1.0,
        throttle_tolerance: -5.0,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.tolerance, 0.0);
    assert_eq!(config.throttle_tolerance, 0.0);
}

#[test]
fn test_global_view_normalize_throttle_bumped_to_tolerance() {
    let mut config = GlobalViewConfig {
        tolerance: 2.0,
        throttle_tolerance: 0.5,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.tolerance, 2.0);
    assert_eq!(config.throttle_tolerance, 2.0); // bumped to match tolerance
}

#[test]
fn test_global_view_normalize_valid_values_unchanged() {
    let mut config = GlobalViewConfig {
        fps_threshold: 60,
        tolerance: 0.3,
        throttle_tolerance: 0.8,
    };
    config.normalize();
    assert_eq!(config.tolerance, 0.3);
    assert_eq!(config.throttle_tolerance, 0.8);
}

#[test]
fn test_global_view_normalize_throttle_larger_ok() {
    let mut config = GlobalViewConfig {
        tolerance: 1.0,
        throttle_tolerance: 5.0,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.tolerance, 1.0);
    assert_eq!(config.throttle_tolerance, 5.0); // larger is fine
}

// ========================================================================
// ClosureAdjustmentConfig Normalize Tests
// ========================================================================

#[test]
fn test_closure_adjustment_normalize_nan_resets_to_default() {
    let mut config = ClosureAdjustmentConfig {
        base_sigma_length_m: f64::NAN,
        base_sigma_azimuth_deg: f64::NAN,
        base_sigma_depth_m: f64::NAN,
    };
    config.normalize();
    assert_eq!(config.base_sigma_length_m, 0.05);
    assert_eq!(config.base_sigma_azimuth_deg, 1.0);
    assert_eq!(config.base_sigma_depth_m, 0.05);
}

#[test]
fn test_closure_adjustment_normalize_infinity_resets_to_default() {
    let mut config = ClosureAdjustmentConfig {
        base_sigma_length_m: f64::INFINITY,
        base_sigma_azimuth_deg: f64::NEG_INFINITY,
        base_sigma_depth_m: f64::INFINITY,
    };
    config.normalize();
    assert_eq!(config.base_sigma_length_m, 0.05);
    assert_eq!(config.base_sigma_azimuth_deg, 1.0);
    assert_eq!(config.base_sigma_depth_m, 0.05);
}

#[test]
fn test_closure_adjustment_normalize_zero_resets_to_default() {
    let mut config = ClosureAdjustmentConfig {
        base_sigma_length_m: 0.0,
        base_sigma_azimuth_deg: 0.0,
        base_sigma_depth_m: 0.0,
    };
    config.normalize();
    assert_eq!(config.base_sigma_length_m, 0.05);
    assert_eq!(config.base_sigma_azimuth_deg, 1.0);
    assert_eq!(config.base_sigma_depth_m, 0.05);
}

#[test]
fn test_closure_adjustment_normalize_negative_resets_to_default() {
    let mut config = ClosureAdjustmentConfig {
        base_sigma_length_m: -0.1,
        base_sigma_azimuth_deg: -5.0,
        base_sigma_depth_m: -1.0,
    };
    config.normalize();
    assert_eq!(config.base_sigma_length_m, 0.05);
    assert_eq!(config.base_sigma_azimuth_deg, 1.0);
    assert_eq!(config.base_sigma_depth_m, 0.05);
}

#[test]
fn test_closure_adjustment_normalize_valid_values_unchanged() {
    let mut config = ClosureAdjustmentConfig {
        base_sigma_length_m: 0.1,
        base_sigma_azimuth_deg: 2.5,
        base_sigma_depth_m: 0.01,
    };
    config.normalize();
    assert_eq!(config.base_sigma_length_m, 0.1);
    assert_eq!(config.base_sigma_azimuth_deg, 2.5);
    assert_eq!(config.base_sigma_depth_m, 0.01);
}

#[test]
fn test_closure_adjustment_normalize_very_small_positive_accepted() {
    let mut config = ClosureAdjustmentConfig {
        base_sigma_length_m: 0.00001,
        base_sigma_azimuth_deg: 0.001,
        base_sigma_depth_m: 0.00001,
    };
    config.normalize();
    assert_eq!(config.base_sigma_length_m, 0.00001);
    assert_eq!(config.base_sigma_azimuth_deg, 0.001);
    assert_eq!(config.base_sigma_depth_m, 0.00001);
}

// ========================================================================
// MapConfig Normalize Tests (zoom_step edge cases)
// ========================================================================

#[test]
fn test_map_config_normalize_zoom_step_nan_resets_to_default() {
    let mut config = MapConfig {
        zoom_step: f64::NAN,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.zoom_step, 1.0);
}

#[test]
fn test_map_config_normalize_zoom_step_infinity_resets_to_default() {
    let mut config = MapConfig {
        zoom_step: f64::INFINITY,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.zoom_step, 1.0);
}

#[test]
fn test_map_config_normalize_zoom_step_below_min_clamped() {
    let mut config = MapConfig {
        zoom_step: 0.05,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.zoom_step, 0.1);
}

#[test]
fn test_map_config_normalize_zoom_step_above_max_clamped() {
    let mut config = MapConfig {
        zoom_step: 6.0,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.zoom_step, 5.0);
}

#[test]
fn test_map_config_normalize_zoom_step_at_bounds_unchanged() {
    let mut min_config = MapConfig {
        zoom_step: 0.1,
        ..Default::default()
    };
    min_config.normalize();
    assert_eq!(min_config.zoom_step, 0.1);

    let mut max_config = MapConfig {
        zoom_step: 5.0,
        ..Default::default()
    };
    max_config.normalize();
    assert_eq!(max_config.zoom_step, 5.0);
}

// ========================================================================
// zoom_speed: pre-#429 multiplier -> levels per wheel notch
// ========================================================================

/// Write `stored` into a real config file and read back what the app would use.
fn zoom_speed_after_load(stored: f64) -> f64 {
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let mut config = Config::default();
    config.map.controls.zoom_speed = stored;
    // `save_to_path` deliberately does not normalize, so the legacy value
    // reaches the file exactly as an older build would have written it.
    config.save_to_path(&path).unwrap();

    Config::load_from_path(&path)
        .unwrap()
        .map
        .controls
        .zoom_speed
}

#[test]
fn test_legacy_zoom_speed_multiplier_is_migrated_on_load() {
    // #429: the setting used to be a 0.1-5.0 multiplier that MapLibre's sigmoid
    // turned into log2(2 / (1 + e^-speed)) levels per notch. Read as levels
    // instead, the old DEFAULT of 1.0 would clamp to 0.95 - a 73% speed-up for
    // users who never touched it - so load converts anything above the new
    // maximum back into the levels it actually delivered.
    assert_eq!(zoom_speed_after_load(1.0), 0.55);
    assert_eq!(zoom_speed_after_load(3.0), 0.93);
    // 5.0 solves to 0.99 levels, past the top of the new range.
    assert_eq!(zoom_speed_after_load(5.0), 0.95);
}

#[test]
fn test_migrated_zoom_speed_is_stable_across_a_second_load() {
    // The migrated value is inside the new range, so re-saving and re-loading
    // must not convert it again.
    assert_eq!(zoom_speed_after_load(zoom_speed_after_load(1.0)), 0.55);
}

#[test]
fn test_in_range_zoom_speed_is_left_alone_on_load() {
    // The two schemes overlap below the new maximum, so a value the settings
    // dialog could have produced is taken at face value.
    assert_eq!(zoom_speed_after_load(0.55), 0.55);
    assert_eq!(zoom_speed_after_load(0.3), 0.3);
    assert_eq!(zoom_speed_after_load(0.1), 0.1);
    assert_eq!(zoom_speed_after_load(0.95), 0.95);
}

#[test]
fn test_map_controls_normalize_clamps_zoom_speed_to_the_usable_range() {
    let mut config = MapControlsConfig {
        zoom_speed: 0.02,
        ..Default::default()
    };
    config.normalize();
    assert_eq!(config.zoom_speed, 0.1);
}

#[test]
fn test_map_controls_normalize_non_finite_resets_to_default() {
    for junk in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut config = MapControlsConfig {
            zoom_speed: junk,
            ..Default::default()
        };
        config.normalize();
        assert_eq!(
            config.zoom_speed,
            MapControlsConfig::default().zoom_speed,
            "{junk} should fall back to the shipped default"
        );
    }
}

#[test]
fn test_zoom_speed_write_path_clamps_rather_than_migrating() {
    // The Settings dialog's number input does not enforce its own max, so an
    // out-of-range figure typed there arrives over IPC. That is a NEW-scheme
    // value the user just entered, not a legacy multiplier: it has to come back
    // as the nearest value the range holds. Only file loads migrate.
    let mut config = Config::default();
    config.map.controls.zoom_speed = 1.2;
    config.normalize_except_shortcuts();
    assert_eq!(config.map.controls.zoom_speed, 0.95);
}

// ========================================================================
// Config.next_section_color Tests
// ========================================================================

#[test]
fn test_next_section_color_returns_first_unused() {
    let mut config = Config::default();
    config.section_colors.palette = vec![
        "#ff0000".to_string(),
        "#00ff00".to_string(),
        "#0000ff".to_string(),
    ];
    let used = vec!["#ff0000".to_string()];
    assert_eq!(config.next_section_color(&used), "#00ff00");
}

#[test]
fn test_next_section_color_skips_multiple_used() {
    let mut config = Config::default();
    config.section_colors.palette = vec![
        "#ff0000".to_string(),
        "#00ff00".to_string(),
        "#0000ff".to_string(),
    ];
    let used = vec!["#ff0000".to_string(), "#00ff00".to_string()];
    assert_eq!(config.next_section_color(&used), "#0000ff");
}

#[test]
fn test_next_section_color_all_used_wraps_to_first() {
    let mut config = Config::default();
    config.section_colors.palette = vec!["#ff0000".to_string(), "#00ff00".to_string()];
    let used = vec!["#ff0000".to_string(), "#00ff00".to_string()];
    // All used → returns first color
    assert_eq!(config.next_section_color(&used), "#ff0000");
}

#[test]
fn test_next_section_color_empty_palette_returns_hardcoded() {
    let mut config = Config::default();
    config.section_colors.palette = vec![];
    assert_eq!(config.next_section_color(&[]), "#4ecdc4");
}

#[test]
fn test_next_section_color_no_used_returns_first() {
    let mut config = Config::default();
    config.section_colors.palette = vec!["#aabbcc".to_string(), "#ddeeff".to_string()];
    assert_eq!(config.next_section_color(&[]), "#aabbcc");
}

#[test]
fn test_next_section_color_used_colors_not_in_palette_ignored() {
    let mut config = Config::default();
    config.section_colors.palette = vec!["#ff0000".to_string(), "#00ff00".to_string()];
    let used = vec!["#999999".to_string()]; // not in palette
    assert_eq!(config.next_section_color(&used), "#ff0000");
}

// ========================================================================
// Syntactic Highlighting Tests
// ========================================================================

#[test]
fn test_syntactic_highlighting_defaults_to_full() {
    let config = SyntacticHighlightingConfig::default();
    assert_eq!(config.level, SyntacticHighlightLevel::Full);
    assert_eq!(config.preset, SyntacticHighlightPreset::Default);
    assert!(config.overrides.is_empty());
    // And the default top-level config exposes the same.
    assert_eq!(
        Config::default().syntactic_highlighting.level,
        SyntacticHighlightLevel::Full
    );
}

#[test]
fn test_syntactic_highlighting_missing_section_loads_as_full() {
    // A config written before this feature existed has no
    // [syntactic_highlighting] section; it must load with the opt-out default.
    // Simulate by serializing a default config and stripping the section.
    let mut value = toml::Value::try_from(Config::default()).expect("config to toml value");
    value
        .as_table_mut()
        .expect("config is a table")
        .remove("syntactic_highlighting");
    let without_section = toml::to_string(&value).expect("re-serialize");
    assert!(!without_section.contains("syntactic_highlighting"));

    let config: Config = toml::from_str(&without_section).expect("legacy config should parse");
    assert_eq!(
        config.syntactic_highlighting.level,
        SyntacticHighlightLevel::Full
    );
    assert_eq!(
        config.syntactic_highlighting.preset,
        SyntacticHighlightPreset::Default
    );
}

#[test]
fn test_syntactic_highlighting_serde_round_trip() {
    let mut config = SyntacticHighlightingConfig::default();
    config.level = SyntacticHighlightLevel::ToolbarOnly;
    config.preset = SyntacticHighlightPreset::Deuteranopia;
    let mut roles = HashMap::new();
    roles.insert("station".to_string(), "#36b96a".to_string());
    config.overrides.insert("deuteranopia".to_string(), roles);

    let serialized = toml::to_string(&config).expect("serialize");
    let deserialized: SyntacticHighlightingConfig =
        toml::from_str(&serialized).expect("deserialize");
    assert_eq!(deserialized.level, SyntacticHighlightLevel::ToolbarOnly);
    assert_eq!(deserialized.preset, SyntacticHighlightPreset::Deuteranopia);
    assert_eq!(deserialized.overrides["deuteranopia"]["station"], "#36b96a");
}

#[test]
fn test_syntactic_highlighting_unknown_enums_normalize() {
    let toml = r#"
level = "made_up"
preset = "also_made_up"
"#;
    let mut config: SyntacticHighlightingConfig = toml::from_str(toml).expect("parse");
    config.normalize();
    assert_eq!(config.level, SyntacticHighlightLevel::Full);
    assert_eq!(config.preset, SyntacticHighlightPreset::Default);
}

#[test]
fn test_syntactic_highlighting_normalize_drops_bad_hex() {
    let mut config = SyntacticHighlightingConfig::default();
    let mut roles = HashMap::new();
    roles.insert("station".to_string(), "#36b96a".to_string()); // valid
    roles.insert("shot".to_string(), "0070dd".to_string()); // missing '#'
    roles.insert("segment".to_string(), "#xyz123".to_string()); // non-hex
    roles.insert("section".to_string(), "#A335EE".to_string()); // valid uppercase
    config.overrides.insert("default".to_string(), roles);

    config.normalize();

    let default_roles = &config.overrides["default"];
    assert!(default_roles.contains_key("station"));
    assert!(default_roles.contains_key("section"));
    assert!(!default_roles.contains_key("shot"));
    assert!(!default_roles.contains_key("segment"));
}

#[test]
fn test_syntactic_highlighting_normalize_removes_empty_preset_maps() {
    let mut config = SyntacticHighlightingConfig::default();
    let mut roles = HashMap::new();
    roles.insert("station".to_string(), "notacolor".to_string()); // dropped
    config.overrides.insert("default".to_string(), roles);
    config.normalize();
    // The preset entry becomes empty after dropping the bad color, so it is removed.
    assert!(config.overrides.is_empty());
}

#[test]
fn test_panels_config_normalize_clamps_sizes() {
    // Out-of-range sizes (e.g. hand-edited or written by an older build) must be
    // clamped to the same bounds the UI enforces while dragging - a degenerate 0
    // would otherwise reload the panel collapsed/invisible.
    let mut p = PanelsConfig {
        sections_panel_width: 0,
        detail_panel_width: 99999,
        data_table_height: 1,
        ..PanelsConfig::default()
    };
    p.normalize();
    assert_eq!(p.sections_panel_width, 150); // clamped up to min
    assert_eq!(p.detail_panel_width, 500); // clamped down to max
    assert_eq!(p.data_table_height, 100); // clamped up to min
}

#[test]
fn test_unusable_column_width_degrades_per_entry_without_losing_the_rest_of_the_config() {
    // `#[serde(default)]` covers an ABSENT widths table, not a present one holding a
    // value that is not a width. Erroring on such a value would fail the WHOLE parse,
    // and `Config::load().unwrap_or_default()` then silently resets EVERY setting in
    // the file because one column width was hand-typo'd. The bad ENTRY must drop and
    // everything else - sibling widths and unrelated panel settings alike - survive.
    // Mirrors test_gas_config_dpv_old_shape_degrades_to_none_without_losing_the_rest_of_the_config.
    let toml_str = r#"
sections_panel_width = 320
detail_panel_width = 300
data_table_height = 200
data_table_azimuth_mode = "inout"

[data_table_shot_column_widths]
negative = -5
fractional = 210.5
textual = "wide"
absurd = 99999999999
good = 180
"#;
    let mut panels: PanelsConfig =
        toml::from_str(toml_str).expect("an unusable width must not fail the whole parse");
    assert_eq!(
        panels.data_table_shot_column_widths,
        std::collections::BTreeMap::from([
            ("absurd".to_string(), u32::MAX),
            ("good".to_string(), 180),
        ]),
        "negative, fractional and textual values are not widths at all and drop; an \
         integral one past u32::MAX saturates so normalize can clamp it"
    );
    // Saturate-then-clamp rather than drop, so there is no arbitrary cliff at
    // u32::MAX: 5000 and 99999999999 both mean "absurdly wide" and both land on
    // MAX_COLUMN_WIDTH. Dropping would instead revert the column to its declared
    // default, which is a different width than the user asked for.
    panels.normalize();
    assert_eq!(
        panels.data_table_shot_column_widths["absurd"], 2000,
        "the saturated width must reach the clamp, not be discarded before it"
    );
    assert_eq!(
        panels.sections_panel_width, 320,
        "an unrelated customized setting in the same table must survive a bad width"
    );
    assert_eq!(
        panels.data_table_azimuth_mode, "inout",
        "a sibling data-table setting must survive a bad width"
    );
}

#[test]
fn test_panels_config_normalize_clamps_column_widths() {
    // config.toml is hand-editable, so the load path has to repair widths the
    // grid cannot render: 0 is invisible and un-resizable (no border to grab),
    // and a huge value pushes every later column off-screen.
    let mut p = PanelsConfig {
        data_table_shot_column_widths: std::collections::BTreeMap::from([
            ("collapsed".to_string(), 0),
            ("runaway".to_string(), 100_000),
            ("sane".to_string(), 150),
        ]),
        data_table_station_column_widths: std::collections::BTreeMap::from([(
            "tiny".to_string(),
            7,
        )]),
        ..PanelsConfig::default()
    };
    p.normalize();
    assert_eq!(p.data_table_shot_column_widths["collapsed"], 40); // up to min
    assert_eq!(p.data_table_shot_column_widths["runaway"], 2000); // down to max
    assert_eq!(p.data_table_shot_column_widths["sane"], 150); // untouched
    assert_eq!(p.data_table_station_column_widths["tiny"], 40);
}

#[test]
fn test_panels_config_normalize_bounds_the_shape_of_the_width_maps() {
    // Clamping each VALUE bounds one entry; nothing bounded the map itself. That
    // matters because no path prunes a key that never named a real column and the
    // frontend re-sends the whole map on every later resize, so one oversized write
    // is re-persisted forever. Both grids declare well under 100 columns.
    let mut p = PanelsConfig {
        data_table_shot_column_widths: (0..300).map(|i| (format!("col_{i:04}"), 120)).collect(),
        data_table_station_column_widths: std::collections::BTreeMap::from([
            ("x".repeat(129), 120),      // one byte over the key cap
            ("x".repeat(128), 130),      // exactly at it, kept
            (String::new(), 140),        // no column has an empty key
            ("length".to_string(), 150), // ordinary, kept
        ]),
        ..PanelsConfig::default()
    };
    p.normalize();

    assert_eq!(p.data_table_shot_column_widths.len(), 256);
    // Truncation is by key order, so it is at least deterministic across runs.
    assert!(p.data_table_shot_column_widths.contains_key("col_0000"));
    assert!(!p.data_table_shot_column_widths.contains_key("col_0299"));

    let station = &p.data_table_station_column_widths;
    assert_eq!(station.len(), 2, "over-long and empty keys are dropped");
    assert_eq!(station["length"], 150);
    assert_eq!(station[&"x".repeat(128)], 130);
    assert!(!station.contains_key(&"x".repeat(129)));
    assert!(!station.contains_key(""));
}

#[test]
fn test_panels_config_normalize_repairs_invalid_mode() {
    let mut p = PanelsConfig {
        sections_panel_mode: "Hover".to_string(), // wrong case
        detail_panel_mode: "nonsense".to_string(),
        data_table_mode: "hover".to_string(), // valid, preserved
        ..PanelsConfig::default()
    };
    p.normalize();
    assert_eq!(p.sections_panel_mode, "pin");
    assert_eq!(p.detail_panel_mode, "pin");
    assert_eq!(p.data_table_mode, "hover");
}

#[test]
fn test_panels_config_normalize_preserves_in_range_values() {
    let mut p = PanelsConfig {
        sections_panel_width: 300,
        detail_panel_width: 250,
        data_table_height: 400,
        sections_panel_mode: "hover".to_string(),
        detail_panel_mode: "pin".to_string(),
        data_table_mode: "hover".to_string(),
        ..PanelsConfig::default()
    };
    p.normalize();
    assert_eq!(p.sections_panel_width, 300);
    assert_eq!(p.detail_panel_width, 250);
    assert_eq!(p.data_table_height, 400);
    assert_eq!(p.sections_panel_mode, "hover");
    assert_eq!(p.detail_panel_mode, "pin");
    assert_eq!(p.data_table_mode, "hover");
}

#[test]
fn test_panels_config_deserializes_legacy_table_without_reveal_fields() {
    // An older [panels] table that predates the *_open/*_mode fields must still
    // deserialize, filling the new fields from their serde defaults.
    let toml = r"
        sections_panel_width = 250
        detail_panel_width = 300
        data_table_height = 200
    ";
    let p: PanelsConfig = toml::from_str(toml).expect("legacy panels table should deserialize");
    assert!(p.sections_panel_open);
    assert!(p.detail_panel_open);
    // Open, same as a fresh install: the serde default and `Default` agree, so
    // "the default" is one thing rather than two that can drift apart.
    assert!(p.data_table_open);
    assert_eq!(p.sections_panel_mode, "pin");
    assert_eq!(p.detail_panel_mode, "pin");
    assert_eq!(p.data_table_mode, "pin");
    // Every config written before #334 also lacks the column-width tables, so
    // they must default to "no overrides" rather than failing the whole load and
    // dropping the user back to stock settings.
    assert!(p.data_table_shot_column_widths.is_empty());
    assert!(p.data_table_station_column_widths.is_empty());
}

// ========================================================================
// SpeleoDB connection settings
// ========================================================================

// ========================================================================
// Atomic save tests (#357 item 3: `write_atomically` = sibling temp file +
// `sync_all` + rename, replacing a bare `fs::write` that truncated the
// destination before writing a byte).
// ========================================================================

#[test]
fn test_save_to_path_round_trips_settings_across_sections() {
    // A SERIALIZATION guard, not an atomicity one - restoring a plain
    // `fs::write` leaves this green, because all it asks is that a normal save
    // survives a reload across several sections of the config. That is worth
    // pinning on its own (the temp-file+rename rewrite had to not disturb it),
    // but the atomicity claim is carried by
    // `test_save_to_path_leaves_destination_untouched_when_write_cannot_complete`,
    // which is the test that goes red if the destination is written in place.
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let mut config = Config::default();
    config.general.locale = "en-US".to_string();
    config.general.undo_history_limit = 77;
    config.panels.sections_panel_width = 300;
    config.placement_suggestion.azimuth_sigma = 2.5;
    config.display.grid_spacing = 3.25;

    config.save_to_path(&path).unwrap();
    let loaded = Config::load_from_path(&path).unwrap();

    assert_eq!(loaded.general.locale, "en-US");
    assert_eq!(loaded.general.undo_history_limit, 77);
    assert_eq!(loaded.panels.sections_panel_width, 300);
    assert_eq!(loaded.placement_suggestion.azimuth_sigma, 2.5);
    assert_eq!(loaded.display.grid_spacing, 3.25);
}

/// Backdate a file's mtime so the staleness margin in `sweep_stale_temp_files`
/// treats it as abandoned. `std::fs::FileTimes` keeps this dependency-free.
fn backdate(path: &std::path::Path, age: std::time::Duration) {
    let file = std::fs::File::options().write(true).open(path).unwrap();
    let when = std::time::SystemTime::now() - age;
    file.set_times(std::fs::FileTimes::new().set_modified(when))
        .unwrap();
}

/// Comfortably past `atomic_write::STALE_TEMP_AGE` (one hour).
const ABANDONED: std::time::Duration = std::time::Duration::from_secs(2 * 60 * 60);

/// Every entry in `dir` whose name marks it as a `write_atomically` temp file.
///
/// The assertion has to be made against the DIRECTORY, not against a computed
/// path: `write_atomically` names its temp file `config.toml.<pid>.<n>.tmp`, so
/// a test that recomputes one specific name (the old `path.with_extension(
/// "toml.tmp")`) asserts the absence of a path no implementation can ever
/// create, and passes even against one that leaks every temp file it opens.
fn temp_file_siblings(dir: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .filter(|name| name.ends_with(".tmp"))
        .collect()
}

#[test]
fn test_save_to_path_leaves_no_temp_file_sibling() {
    // Guard D: `write_atomically`'s sibling temp file must not survive a
    // successful save.
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let mut config = Config::default();
    config.general.undo_history_limit = 55;
    config.save_to_path(&path).unwrap();

    let leaked = temp_file_siblings(dir.path());
    assert!(
        leaked.is_empty(),
        "a successful save must not leave a temp file behind, found: {leaked:?}"
    );
    // Pin the other half: the save did land, so the emptiness above is not
    // "nothing was written at all".
    assert!(path.exists(), "the config itself must exist after a save");
}

#[test]
fn test_repeated_saves_do_not_accumulate_temp_files() {
    // The temp name is unique per write, so a leak does not overwrite the
    // previous leak - it piles up. One save cannot tell those apart; several can.
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");

    let mut config = Config::default();
    for limit in 1..=5 {
        config.general.undo_history_limit = limit;
        config.save_to_path(&path).unwrap();
    }

    let leaked = temp_file_siblings(dir.path());
    assert!(
        leaked.is_empty(),
        "5 saves must leave no temp files, found: {leaked:?}"
    );
}

#[test]
fn test_load_sweeps_an_abandoned_temp_file() {
    // A crash between `File::create` and the rename leaves a temp file nothing
    // else collects. Load is the sweeper.
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");
    Config::default().save_to_path(&path).unwrap();

    // Stand in for the crashed writer, and backdate it past the staleness
    // margin - a file younger than that belongs to a possibly-live writer.
    let abandoned = dir.path().join("config.toml.99999.0.tmp");
    std::fs::write(&abandoned, "partial").unwrap();
    backdate(&abandoned, ABANDONED);

    Config::load_from_path(&path).unwrap();

    assert!(
        !abandoned.exists(),
        "load must sweep an abandoned config temp file"
    );
    assert!(path.exists(), "the sweep must not touch the config itself");
}

#[test]
fn test_load_leaves_a_fresh_temp_file_alone() {
    // The inverse, and the one that matters for correctness: a temp file young
    // enough to belong to a writer that is still between `File::create` and its
    // rename must survive. Deleting it reintroduces the ENOENT-on-rename failure
    // the unique naming exists to prevent.
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");
    Config::default().save_to_path(&path).unwrap();

    let in_flight = dir.path().join("config.toml.12345.0.tmp");
    std::fs::write(&in_flight, "a live writer owns this").unwrap();

    Config::load_from_path(&path).unwrap();

    assert!(
        in_flight.exists(),
        "load must not delete a temp file a live writer may still rename"
    );
}

#[test]
fn test_load_sweep_spares_unrelated_files() {
    // The sweep is name-scoped. A project's own `.stalx.tmp` and any other
    // sibling must be untouched even when they are ancient.
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");
    Config::default().save_to_path(&path).unwrap();

    let spared = ["cave.stalx.tmp", "notes.txt", "config.toml.backup"];
    for name in spared {
        let file = dir.path().join(name);
        std::fs::write(&file, "keep me").unwrap();
        backdate(&file, ABANDONED);
    }

    Config::load_from_path(&path).unwrap();

    for name in spared {
        assert!(
            dir.path().join(name).exists(),
            "the sweep must not delete {name}"
        );
    }
}

#[test]
fn test_save_to_path_leaves_destination_untouched_when_write_cannot_complete() {
    // Guard E: simulate a write that cannot complete, and assert the destination
    // is left exactly as the previous successful save left it - the whole point
    // of writing to a sibling and renaming rather than truncating `config.toml`.
    //
    // The failure is forced by making the config DIRECTORY unwritable, so
    // `File::create` fails with EACCES before a byte of the destination is
    // touched. An earlier version pre-occupied `config.toml.tmp` with a directory
    // to get EISDIR, which was simpler but depended on the temp name being
    // predictable - and it is deliberately NOT: `write_atomically` makes it unique
    // per write (pid plus a counter) so two writers on one config directory cannot
    // clobber each other's temp file. Pinning the old fixed name here would have
    // quietly re-asserted the shared-resource bug as the spec.
    //
    // Unix-only: it turns on POSIX permission bits, and the Windows ACL equivalent
    // is not the same test. The behaviour under guard is platform-independent, so
    // covering it on one family is enough.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");

        let mut old = Config::default();
        old.general.undo_history_limit = 11;
        old.save_to_path(&path).unwrap();

        // r-xr-xr-x: readable, so the survivor can still be loaded below, but no
        // new file may be created in it.
        let original = std::fs::metadata(dir.path()).unwrap().permissions();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();

        // ...unless the mode bits are advisory for us, as they are for root. The
        // self-hosted CI runner is a container running as uid 0, so this test
        // passes on hosted runners and fails on that one - which is how it
        // reached master: the runner picker sent its own PR to hosted.
        //
        // Probe the property the test depends on - can a file be created here at
        // all - rather than the uid behind it, matching `repair.rs`. This is not
        // because `geteuid` is unreachable (`command_tests.rs` calls it under an
        // `#[expect(unsafe_code)]`); it is because uid is the wrong question. A
        // non-root process holding CAP_DAC_OVERRIDE also walks through 0o555, and
        // a `geteuid() == 0` check would miss it and fail exactly as this did.
        if std::fs::File::create(dir.path().join(".root-probe")).is_ok() {
            let _ = std::fs::remove_file(dir.path().join(".root-probe"));
            std::fs::set_permissions(dir.path(), original).unwrap();
            // Only visible under `--nocapture`/`--show-output`: libtest captures
            // the output of passing tests, and CI passes neither. So this is a
            // hand-run aid, NOT the thing that makes the skip observable in CI -
            // nothing there reports which tests self-disabled. The assertions
            // below therefore run only on hosted (non-root) runs. Making the
            // inducer root-safe instead is not available to this test: the temp
            // name is deliberately unpredictable (see above), so the squat trick
            // used elsewhere cannot apply, and a missing-parent-dir inducer would
            // stop the destination existing - which is the very thing under test.
            #[expect(
                clippy::print_stderr,
                reason = "a skipped permission test says so, or it is silent in a hand-run too"
            )]
            {
                eprintln!("Skipping: test requires non-root (chmod 555 has no effect as root)");
            }
            return;
        }

        let mut new = Config::default();
        new.general.undo_history_limit = 99;
        let result = new.save_to_path(&path);

        // Restore BEFORE asserting, so a failed assertion cannot leave the
        // TempDir undeletable and take unrelated tests down with it.
        std::fs::set_permissions(dir.path(), original).unwrap();

        assert!(
            result.is_err(),
            "a temp file that cannot be created must fail the save"
        );
        let survivor = Config::load_from_path(&path).unwrap();
        assert_eq!(
            survivor.general.undo_history_limit, 11,
            "the destination must be untouched by a save that could not complete"
        );
    }
}

// ========================================================================
// Temp-name unpredictability + collision recovery (#357 residue: `create_new`
// is fail-CLOSED on an existing name, and the old pid+counter name was fully
// predictable, so one occupied name - planted or leaked by a crash whose pid got
// reused inside the sweep's one-hour margin - failed every save of the session).
// ========================================================================

/// Split a temp-file name into the fields between the fixed `config.toml.`
/// prefix and the fixed `.tmp` suffix.
/// A name the sweep would not match yields no fields at all, so the caller's
/// assertion reports that rather than this helper failing on its own.
fn temp_name_fields(path: &std::path::Path) -> Vec<String> {
    let name = path.file_name().unwrap().to_str().unwrap();
    let Some(middle) = name
        .strip_prefix("config.toml.")
        .and_then(|rest| rest.strip_suffix(".tmp"))
    else {
        return Vec::new();
    };
    middle.split('.').map(str::to_owned).collect()
}

#[test]
fn test_temp_name_is_not_a_pure_function_of_pid_and_counter() {
    // The predictability half of the fix. The old name was
    // `config.toml.<pid>.<n>.tmp` with `n` starting at 0, so anything able to
    // write to the config directory could enumerate the names a given process
    // would use and pre-create them; against O_EXCL that is a denial of
    // persistence, and the sweep spares a file younger than an hour, so it does
    // not self-heal within a session.
    let path = std::path::Path::new("/nonexistent/config.toml");
    let first = atomic_write::temp_candidate(path);
    let second = atomic_write::temp_candidate(path);
    assert_ne!(first, second, "candidates must never repeat");

    let fields = temp_name_fields(&first);
    assert_eq!(
        fields.len(),
        3,
        "expected <pid>.<seq>.<nonce>, got {fields:?} - a two-field name is the \
         predictable pid+counter shape this test exists to rule out"
    );
    let other = temp_name_fields(&second);
    assert_eq!(other.len(), 3, "second candidate: got {other:?}");

    // The pid is the same process; the counter is guessable by construction. The
    // nonce is the part an outsider cannot compute, so it is the part that has to
    // be fresh on every candidate.
    assert_eq!(fields[0], other[0], "same process, so same pid field");
    assert_ne!(
        fields[2], other[2],
        "the unpredictable field must be redrawn per candidate"
    );
    assert!(
        fields[2].len() >= 32 && fields[2].chars().all(|c| c.is_ascii_hexdigit()),
        "expected a v4 UUID in simple form (32 hex chars), got {:?}",
        fields[2]
    );
}

#[test]
fn test_load_sweeps_a_temp_file_named_the_way_writes_name_them() {
    // Ties the sweep's `starts_with("config.toml.") && ends_with(".tmp")`
    // predicate to the name the writer actually produces, using the writer's own
    // generator rather than a hand-written copy. Without this, a change to the
    // name shape (this one added a nonce field) could silently stop being swept -
    // and the sweep is the only thing that collects these.
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");
    Config::default().save_to_path(&path).unwrap();

    let abandoned = atomic_write::temp_candidate(&path);
    std::fs::write(&abandoned, "partial").unwrap();
    backdate(&abandoned, ABANDONED);

    Config::load_from_path(&path).unwrap();

    assert!(
        !abandoned.exists(),
        "the sweep must still match the writer's temp-file names, missed {abandoned:?}"
    );
}

#[test]
fn test_write_retries_past_an_occupied_temp_name() {
    // The recoverability half. Forcing the collision needs the name source
    // injected, because the nonce is exactly what makes it unreachable from
    // outside - so this drives `write_atomically_with`, which is the same
    // function `write_atomically` is a one-line wrapper around.
    use tempfile::tempdir;
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.toml");
    Config::default().save_to_path(&path).unwrap();

    // Stands in for a leaked temp file from a crashed writer whose pid was
    // reused, or for a plant. Either way it is not ours to delete.
    let occupied = dir.path().join("config.toml.4242.0.abcdef.tmp");
    std::fs::write(&occupied, "not mine").unwrap();

    let mut handed_out = 0;
    let mut candidates = || {
        handed_out += 1;
        if handed_out == 1 {
            occupied.clone()
        } else {
            atomic_write::temp_candidate(&path)
        }
    };
    // Asserted as bytes rather than through `Config::load_from_path`: the
    // question here is whether the retried write reached the destination at all,
    // and a round-trip through the parser would answer a different one.
    let contents = "written by the retry\n";
    atomic_write::write_atomically_with(&path, contents, &mut candidates)
        .expect("an occupied temp name must cost a retry, not the save");

    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        contents,
        "the retried write must land on the destination"
    );
    assert_eq!(
        std::fs::read_to_string(&occupied).unwrap(),
        "not mine",
        "a temp file we did not create must be left alone - deleting a live \
         writer's temp file reintroduces the ENOENT-on-rename it exists to prevent"
    );
    // Everything except the pre-planted squatter must be gone.
    let leaked: Vec<String> = temp_file_siblings(dir.path())
        .into_iter()
        .filter(|name| !occupied.ends_with(name))
        .collect();
    assert!(
        leaked.is_empty(),
        "the retry must not leak its own temp files, found: {leaked:?}"
    );
}

// ========================================================================
// Config-file permissions (#357 residue: `fs::write` opened the EXISTING file
// and kept its mode; a freshly created temp file is `0o666 & ~umask`, and the
// rename carries that onto `config.toml`).
// ========================================================================

#[test]
fn test_save_preserves_the_config_files_own_mode() {
    // A user who tightened their config must keep it. Saves fire on every
    // project open, create and save, so the regression republished the file -
    // which names the SpeleoDB account and every recent project's absolute path
    // - at the process umask within seconds of a `chmod`.
    //
    // The modes under test are deliberately NOT 0600. The temp file is created
    // 0600, so a 0600 destination cannot distinguish inheritance from that
    // default: the test would pass with the inheritance deleted outright. Each
    // mode here differs from both 0600 and the umask default, so only carrying
    // the destination's own mode across produces it - and both directions are
    // covered, since a save that clamped every config to 0600 would be a
    // different bug wearing this one's fix.
    //
    // Unix-only: Windows has no mode bits to carry, and its ACLs are inherited
    // from the directory by old and new file alike.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        use tempfile::tempdir;
        for (limit, expected) in [(42usize, 0o640u32), (43, 0o664), (44, 0o600)] {
            let dir = tempdir().unwrap();
            let path = dir.path().join("config.toml");
            Config::default().save_to_path(&path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(expected)).unwrap();

            let mut config = Config::default();
            config.general.undo_history_limit = limit;
            config.save_to_path(&path).unwrap();

            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode, expected,
                "a save must replace the config without changing its mode \
                 (expected {expected:o}, got {mode:o})"
            );
            // The other half: the mode survived because the file was replaced
            // with its mode carried over, not because the save never happened.
            assert_eq!(
                Config::load_from_path(&path)
                    .unwrap()
                    .general
                    .undo_history_limit,
                limit,
                "the save itself must still land"
            );
        }
    }
}

#[test]
fn test_first_save_creates_a_private_config_file() {
    // With no destination to inherit from there is no user intent to preserve,
    // so the default is the private one rather than the umask's. Same file,
    // same sensitive contents - the only difference is that the user has not
    // had the chance to tighten it yet.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");

        Config::default().save_to_path(&path).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "a config created from scratch must not be group/world readable (got {mode:o})"
        );
    }
}

// ========================================================================
// `[general]` collection bounds (#357 residue: the same unbounded-growth shape
// `normalize_column_names` closed, still open on three more fields).
// ========================================================================

#[test]
fn test_normalize_caps_general_collections() {
    // The load-path half - `Config::load` re-parses whatever is on disk, so the
    // caps have to hold for a hand-edited file as well as for an IPC payload.
    let mut config = Config::default();
    config.general.onboarding_completed_flows = (0..100_000).map(|i| format!("flow-{i}")).collect();
    config
        .general
        .onboarding_completed_flows
        .push("x".repeat(5000));
    config.general.locale = "e".repeat(10_000);
    config.general.number_format = "n".repeat(10_000);
    config.general.date_format = "d".repeat(10_000);
    config.general.recent_projects_max = usize::MAX;
    config.general.recent_projects = (0..100_000)
        .map(|i| std::path::PathBuf::from(format!("/caves/{i}.stalx")))
        .collect();

    config.normalize();

    let general = &config.general;
    assert!(
        general.onboarding_completed_flows.len() <= 256,
        "flow ids uncapped: {}",
        general.onboarding_completed_flows.len()
    );
    assert!(
        general
            .onboarding_completed_flows
            .iter()
            .all(|flow| flow.len() <= 128),
        "an oversized flow id survived"
    );
    assert_eq!(
        general.locale, "system",
        "an unusable locale tag must fall back to its default"
    );
    assert_eq!(general.number_format, "locale");
    assert_eq!(general.date_format, "locale");
    assert!(
        general.recent_projects_max <= 256,
        "recent_projects_max uncapped: {}",
        general.recent_projects_max
    );
    assert_eq!(
        general.recent_projects.len(),
        general.recent_projects_max,
        "the recents list must obey its own cap"
    );

    // Repair, not reset: ordinary values go through untouched.
    let mut ok = Config::default();
    ok.general.locale = "en-US".to_string();
    ok.general.number_format = "de-DE".to_string();
    ok.general.onboarding_completed_flows = vec!["workspace-basics".to_string()];
    ok.general.recent_projects = vec![std::path::PathBuf::from("/caves/cenote.stalx")];
    ok.normalize();
    assert_eq!(ok.general.locale, "en-US");
    assert_eq!(ok.general.number_format, "de-DE");
    assert_eq!(
        ok.general.onboarding_completed_flows,
        vec!["workspace-basics".to_string()]
    );
    assert_eq!(ok.general.recent_projects.len(), 1);
}

// ========================================================================
// Station-name column: hidden by default, including for existing configs
// ========================================================================

/// Save `panels` as an older build would have (no normalize), load it back.
fn panels_after_load(panels: PanelsConfig) -> PanelsConfig {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let config = Config {
        panels,
        ..Default::default()
    };
    config.save_to_path(&path).unwrap();
    Config::load_from_path(&path).unwrap().panels
}

#[test]
fn test_station_name_column_starts_hidden() {
    let panels = Config::default().panels;
    assert!(panels
        .data_table_station_hidden_columns
        .contains(&"name".to_string()));
    assert_eq!(panels.data_table_station_columns_seen, vec!["name"]);
}

#[test]
fn test_station_name_column_is_hidden_once_for_a_config_from_before_it() {
    // A config persisted before the column existed: its own hidden list, no
    // `data_table_station_columns_seen` key at all.
    let before = PanelsConfig {
        data_table_station_hidden_columns: vec!["_x".to_string()],
        data_table_station_columns_seen: Vec::new(),
        ..Config::default().panels
    };
    let loaded = panels_after_load(before);
    assert_eq!(loaded.data_table_station_hidden_columns, vec!["_x", "name"]);
    assert_eq!(loaded.data_table_station_columns_seen, vec!["name"]);

    // The user then shows it: a later load must respect that, not re-hide it.
    let shown = PanelsConfig {
        data_table_station_hidden_columns: vec!["_x".to_string()],
        ..loaded
    };
    let reloaded = panels_after_load(shown);
    assert_eq!(reloaded.data_table_station_hidden_columns, vec!["_x"]);
    assert_eq!(reloaded.data_table_station_columns_seen, vec!["name"]);
}

#[test]
fn test_an_empty_hidden_list_from_before_the_name_column_still_hides_it() {
    // "Show all" was chosen before the column existed; it starts hidden anyway.
    let loaded = panels_after_load(PanelsConfig {
        data_table_station_hidden_columns: Vec::new(),
        data_table_station_columns_seen: Vec::new(),
        ..Config::default().panels
    });
    assert_eq!(loaded.data_table_station_hidden_columns, vec!["name"]);
}

#[test]
fn test_a_config_file_without_the_seen_key_gets_the_name_column_hidden() {
    // Exactly what a pre-change build wrote: no `data_table_station_columns_seen`
    // key, and a hidden list without "name".
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut document = toml::Value::try_from(Config::default()).unwrap();
    let panels = document
        .get_mut("panels")
        .and_then(toml::Value::as_table_mut)
        .unwrap();
    assert!(panels.remove("data_table_station_columns_seen").is_some());
    panels.insert(
        "data_table_station_hidden_columns".to_string(),
        toml::Value::Array(vec![toml::Value::String("_lat".to_string())]),
    );
    std::fs::write(&path, toml::to_string(&document).unwrap()).unwrap();

    let loaded = Config::load_from_path(&path).unwrap().panels;
    assert_eq!(
        loaded.data_table_station_hidden_columns,
        vec!["_lat", "name"]
    );
    assert_eq!(loaded.data_table_station_columns_seen, vec!["name"]);
}
