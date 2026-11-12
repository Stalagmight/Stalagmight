/**
 * Unit tests for config types and defaultConfig.
 * Tests that TypeScript config structure matches expected values.
 */

import { describe, it, expect } from 'vitest';
import {
  defaultConfig,
  lightDisplayDefaults,
  lightSectionColorDefaults,
  getThemedDisplayDefaults,
  getThemedSectionColorDefaults,
  mapCanvasFor,
  type Config,
  type StationConfig,
  type ShotConfig,
  type MapSnapConfig,
  type DebugConfig,
  type MapDebugConfig,
  type AppearanceConfig,
  type ThemeMode,
} from '../config';
import { defaultConfig as generatedDefaultConfig } from '../ipc/defaultConfig';
import { themeTokens } from '../../theme/tokens';

describe('defaultConfig single-sourcing (#275)', () => {
  it('re-exports the generated Config::default() unmodified', () => {
    // Tripwire: config.ts must re-export src/types/ipc/defaultConfig.ts as-is,
    // not merge/override it. Catches a future partial regen or a reintroduced
    // hand-written override drifting from Config::default() in the Rust config.
    expect(defaultConfig).toEqual(generatedDefaultConfig);
  });
});

describe('defaultConfig structure', () => {
  describe('top-level structure', () => {
    it('has all required top-level keys', () => {
      expect(defaultConfig).toHaveProperty('general');
      expect(defaultConfig).toHaveProperty('units');
      expect(defaultConfig).toHaveProperty('display');
      expect(defaultConfig).toHaveProperty('section_colors');
      expect(defaultConfig).toHaveProperty('map');
      expect(defaultConfig).toHaveProperty('validation');
      expect(defaultConfig).toHaveProperty('export');
      expect(defaultConfig).toHaveProperty('logging');
      expect(defaultConfig).toHaveProperty('shortcuts');
      expect(defaultConfig).toHaveProperty('panels');
      expect(defaultConfig).toHaveProperty('cache');
      expect(defaultConfig).toHaveProperty('debug');
      expect(defaultConfig).toHaveProperty('appearance');
      expect(defaultConfig).toHaveProperty('gas');
    });
  });

  describe('map.station defaults', () => {
    it('has correct station default values', () => {
      expect(defaultConfig.map.station.radius).toBe(4.0);
      expect(defaultConfig.map.station.hit_area).toBe(10.0);
    });

    it('station is a valid StationConfig', () => {
      const station: StationConfig = defaultConfig.map.station;
      expect(typeof station.radius).toBe('number');
      expect(typeof station.hit_area).toBe('number');
    });
  });

  describe('map.shot defaults', () => {
    it('has correct shot default values', () => {
      expect(defaultConfig.map.shot.width).toBe(2.0);
      expect(defaultConfig.map.shot.hit_area).toBe(8.0);
    });

    it('shot is a valid ShotConfig', () => {
      const shot: ShotConfig = defaultConfig.map.shot;
      expect(typeof shot.width).toBe('number');
      expect(typeof shot.hit_area).toBe('number');
    });
  });

  describe('map.snap defaults', () => {
    it('has correct snap default values', () => {
      expect(defaultConfig.map.snap.threshold).toBe(10.0);
      expect(defaultConfig.map.snap.enabled_merge).toBe(true);
      expect(defaultConfig.map.snap.enabled_measure).toBe(true);
    });

    it('snap is a valid MapSnapConfig', () => {
      const snap: MapSnapConfig = defaultConfig.map.snap;
      expect(typeof snap.threshold).toBe('number');
      expect(typeof snap.enabled_merge).toBe('boolean');
      expect(typeof snap.enabled_measure).toBe('boolean');
    });
  });

  describe('debug defaults', () => {
    it('has correct debug.map default values', () => {
      expect(defaultConfig.debug.map.show_hitboxes).toBe(false);
      expect(defaultConfig.debug.map.log_hit_detection).toBe(false);
    });

    it('debug is a valid DebugConfig', () => {
      const debug: DebugConfig = defaultConfig.debug;
      expect(debug).toHaveProperty('map');
    });

    it('debug.map is a valid MapDebugConfig', () => {
      const mapDebug: MapDebugConfig = defaultConfig.debug.map;
      expect(typeof mapDebug.show_hitboxes).toBe('boolean');
      expect(typeof mapDebug.log_hit_detection).toBe('boolean');
    });
  });

  describe('map structure', () => {
    it('has all map nested configs', () => {
      expect(defaultConfig.map).toHaveProperty('station');
      expect(defaultConfig.map).toHaveProperty('shot');
      expect(defaultConfig.map).toHaveProperty('snap');
      expect(defaultConfig.map).toHaveProperty('controls');
      expect(defaultConfig.map).toHaveProperty('satellite_gps_origin');
    });

    it('has correct zoom defaults', () => {
      expect(defaultConfig.map.zoom_default).toBe(15);
      expect(defaultConfig.map.zoom_min).toBe(0);
      expect(defaultConfig.map.zoom_max).toBe(22);
      expect(defaultConfig.map.zoom_step).toBe(1.0);
    });

    it('has correct satellite defaults', () => {
      expect(defaultConfig.map.satellite_provider).toBe('esri');
      expect(defaultConfig.map.satellite_enabled).toBe(true);
    });
  });

  describe('general config', () => {
    it('has correct general defaults', () => {
      expect(defaultConfig.general.auto_save).toBe(true);
      expect(defaultConfig.general.auto_save_interval_seconds).toBe(30);
      expect(defaultConfig.general.recent_projects_max).toBe(20);
      expect(defaultConfig.general.undo_history_limit).toBe(100);
      expect(defaultConfig.general.undo_persist_limit).toBe(20);
      expect(defaultConfig.general.locale).toBe('system');
      expect(defaultConfig.general.onboarding_enabled).toBe(true);
      expect(defaultConfig.general.onboarding_completed_flows).toEqual([]);
      expect(defaultConfig.general.onboarding_first_launch_done).toBe(false);
    });
  });

  describe('units config', () => {
    it('has correct units defaults', () => {
      expect(defaultConfig.units.length).toBe('meters');
      expect(defaultConfig.units.depth).toBe('meters');
      expect(defaultConfig.units.angle).toBe('degrees');
      expect(defaultConfig.units.decimal_precision).toEqual({ length: 1, coordinates: 2, depth: 1, azimuth: 1, inclination: 1, gps: 6 });
    });
  });

  describe('validation config', () => {
    it('has correct validation defaults', () => {
      expect(defaultConfig.validation.warn_shot_length_min).toBe(0.3);
      expect(defaultConfig.validation.warn_shot_length_max).toBe(50.0);
      expect(defaultConfig.validation.warn_depth_change_per_shot_max).toBe(10.0);
      expect(defaultConfig.validation.warn_azimuth_deviation_max).toBe(30.0);
      expect(defaultConfig.validation.azimuth_diff_threshold).toBe(20.0);
      expect(defaultConfig.validation.pitch_diff_threshold).toBe(10.0);
      expect(defaultConfig.validation.inclination_depth_diff_threshold).toBe(15.0);
    });
  });

  describe('panels config', () => {
    it('has correct panel defaults', () => {
      expect(defaultConfig.panels.sections_panel_width).toBe(250);
      expect(defaultConfig.panels.detail_panel_width).toBe(300);
      expect(defaultConfig.panels.data_table_height).toBe(200);
    });
  });

  describe('cache config', () => {
    it('has correct cache defaults', () => {
      expect(defaultConfig.cache.tile_cache_enabled).toBe(true);
      expect(defaultConfig.cache.tile_cache_max_mb).toBe(500);
      expect(defaultConfig.cache.tile_ttl_days).toBe(30);
    });
  });

  describe('appearance config', () => {
    it('has correct appearance defaults', () => {
      expect(defaultConfig.appearance.theme).toBe('dark');
    });

    it('appearance is a valid AppearanceConfig', () => {
      const appearance: AppearanceConfig = defaultConfig.appearance;
      expect(typeof appearance.theme).toBe('string');
    });

    it('theme is a valid ThemeMode', () => {
      const theme: ThemeMode = defaultConfig.appearance.theme;
      expect(['dark', 'light', 'auto']).toContain(theme);
    });
  });

  describe('gas config', () => {
    // Pins defaultConfig.gas, which is now generated from Config::default()
    // (src-tauri/src/config/gas.rs) via `pnpm gen:ipc-types` - see
    // src/types/ipc/defaultConfig.ts. Fails loudly if a regen changes these
    // values unintentionally.
    it('has correct gas defaults', () => {
      expect(defaultConfig.gas.units).toBe('metric');
      expect(defaultConfig.gas.default_reserve).toBe('all_thirds');
      expect(defaultConfig.gas.swim_speed_m_per_min).toBe(10);
      expect(defaultConfig.gas.survey_speed_m_per_min).toBeNull();
      expect(defaultConfig.gas.survey_rmv_multiplier).toBe(1);
      expect(defaultConfig.gas.stressed_rmv_multiplier).toBe(1.5);
      expect(defaultConfig.gas.jump_rmv_multiplier).toBe(1.5);
      expect(defaultConfig.gas.conservative_tank_gas).toBe(true);
      expect(defaultConfig.gas.dpv).toBeNull();
      expect(defaultConfig.gas.reserve_profile).toEqual({
        stage_reserve_bar: 30,
        all_thirds_safety_margin_bar: 0,
        recalc_hold_stage2_bar: 100,
        recalc_hold_stage1_bar: 70,
      });
    });
  });
});

describe('Config type safety', () => {
  it('defaultConfig satisfies Config type', () => {
    // This test verifies that defaultConfig is a valid Config at compile time
    const config: Config = defaultConfig;
    expect(config).toBeDefined();
  });

  it('allows creating custom config with modified values', () => {
    // Verify Config type allows overriding values
    const customConfig: Config = {
      ...defaultConfig,
      map: {
        ...defaultConfig.map,
        station: {
          radius: 8.0,
          hit_area: 16.0,
          hover_radius: 10.0,
          selection_radius: 9.0,
        },
        shot: {
          width: 4.0,
          hit_area: 12.0,
          hover_width: 6.0,
          selection_width: 5.0,
        },
      },
    };

    expect(customConfig.map.station.radius).toBe(8.0);
    expect(customConfig.map.station.hit_area).toBe(16.0);
    expect(customConfig.map.shot.width).toBe(4.0);
    expect(customConfig.map.shot.hit_area).toBe(12.0);
  });

  it('allows modifying debug config', () => {
    const customConfig: Config = {
      ...defaultConfig,
      debug: {
        map: {
          show_hitboxes: true,
          log_hit_detection: true,
        },
        memory_probe: false,
      },
    };

    expect(customConfig.debug.map.show_hitboxes).toBe(true);
    expect(customConfig.debug.map.log_hit_detection).toBe(true);
  });
});

describe('Config value consistency', () => {
  it('station hit_area >= station radius in defaults', () => {
    expect(defaultConfig.map.station.hit_area).toBeGreaterThanOrEqual(
      defaultConfig.map.station.radius
    );
  });

  it('shot hit_area >= shot width in defaults', () => {
    expect(defaultConfig.map.shot.hit_area).toBeGreaterThanOrEqual(
      defaultConfig.map.shot.width
    );
  });

  it('station values are within valid bounds (1-50)', () => {
    expect(defaultConfig.map.station.radius).toBeGreaterThanOrEqual(1);
    expect(defaultConfig.map.station.radius).toBeLessThanOrEqual(50);
    expect(defaultConfig.map.station.hit_area).toBeGreaterThanOrEqual(1);
    expect(defaultConfig.map.station.hit_area).toBeLessThanOrEqual(50);
  });

  it('shot values are within valid bounds (0.5-20)', () => {
    expect(defaultConfig.map.shot.width).toBeGreaterThanOrEqual(0.5);
    expect(defaultConfig.map.shot.width).toBeLessThanOrEqual(20);
    expect(defaultConfig.map.shot.hit_area).toBeGreaterThanOrEqual(0.5);
    expect(defaultConfig.map.shot.hit_area).toBeLessThanOrEqual(20);
  });

  it('snap threshold is within valid bounds (1-50)', () => {
    expect(defaultConfig.map.snap.threshold).toBeGreaterThanOrEqual(1);
    expect(defaultConfig.map.snap.threshold).toBeLessThanOrEqual(50);
  });
});

describe('theme-aware display defaults', () => {
  describe('lightDisplayDefaults', () => {
    it('has light-appropriate grid color', () => {
      expect(lightDisplayDefaults.grid_color).toBe('#c1a07a');
    });

    it('has light-appropriate font color', () => {
      expect(lightDisplayDefaults.font_color).toBe('#1a120b');
    });

    it('uses correct field names matching DisplayConfig', () => {
      expect(lightDisplayDefaults).toHaveProperty('grid_color');
      expect(lightDisplayDefaults).toHaveProperty('font_color');
      expect(lightDisplayDefaults).toHaveProperty('grid_spacing');
      expect(lightDisplayDefaults).toHaveProperty('font_size');
    });

    it('no longer carries the dead depth_gradient section (#380)', () => {
      expect(lightDisplayDefaults).not.toHaveProperty('depth_gradient');
      expect(getThemedDisplayDefaults('dark')).not.toHaveProperty('depth_gradient');
    });
  });

  describe('lightSectionColorDefaults', () => {
    it('has 8 palette colors', () => {
      expect(lightSectionColorDefaults.palette).toHaveLength(8);
    });

    it('has all valid hex colors', () => {
      for (const color of lightSectionColorDefaults.palette) {
        expect(color).toMatch(/^#[0-9a-fA-F]{6}$/);
      }
    });

    it('has different colors from dark palette', () => {
      const darkPalette = defaultConfig.section_colors.palette;
      const lightPalette = lightSectionColorDefaults.palette;
      // At least some colors should differ between themes
      const hasDifference = lightPalette.some((c, i) => c !== darkPalette[i]);
      expect(hasDifference).toBe(true);
    });
  });

  describe('getThemedDisplayDefaults', () => {
    it('returns dark defaults for dark theme', () => {
      const result = getThemedDisplayDefaults('dark');
      expect(result.grid_color).toBe(defaultConfig.display.grid_color);
      expect(result.font_color).toBe(defaultConfig.display.font_color);
      expect(result.measure_path_color).toBe(defaultConfig.display.measure_path_color);
    });

    it('returns light defaults for light theme', () => {
      const result = getThemedDisplayDefaults('light');
      expect(result.grid_color).toBe(lightDisplayDefaults.grid_color);
      expect(result.font_color).toBe(lightDisplayDefaults.font_color);
      expect(result.measure_path_color).toBe(lightDisplayDefaults.measure_path_color);
    });

    it('returns DARK defaults for the chrome-only light themes (#357 item 9)', () => {
      // solarized_light and catppuccin_latte set no map-visualization tokens, so
      // their map canvas is :root's dark #1a1a2e. These defaults are all drawn on
      // that canvas, so the light (dark-ink) set would be the wrong one.
      for (const theme of ['solarized_light', 'catppuccin_latte']) {
        expect(mapCanvasFor(theme)).toBe(themeTokens.dark['--cc-map-background']);
        expect(getThemedDisplayDefaults(theme).grid_color).toBe(defaultConfig.display.grid_color);
      }
    });

    it('returns deep copies (not references)', () => {
      const result1 = getThemedDisplayDefaults('light');
      const result2 = getThemedDisplayDefaults('light');
      expect(result1).not.toBe(result2);
    });
  });

  describe('getThemedSectionColorDefaults', () => {
    it('returns dark palette for dark theme', () => {
      const result = getThemedSectionColorDefaults('dark');
      expect(result.palette).toEqual(defaultConfig.section_colors.palette);
    });

    it('returns light palette for light theme', () => {
      const result = getThemedSectionColorDefaults('light');
      expect(result.palette).toEqual(lightSectionColorDefaults.palette);
    });

    it('returns deep copies (not references)', () => {
      const result1 = getThemedSectionColorDefaults('dark');
      const result2 = getThemedSectionColorDefaults('dark');
      expect(result1.palette).not.toBe(result2.palette);
    });
  });
});

/**
 * #357 items 9 and 17. Both were the same defect: a "themed" default specified a
 * colour tuned for the wrong canvas, so the thing it draws was invisible. Neither
 * was caught by anything, because every existing assertion here compares hexes to
 * hexes. These measure instead.
 */
describe('themed defaults are legible on the canvas they are drawn on', () => {
  /** WCAG relative luminance. */
  function luminance(hex: string): number {
    const channel = (v: number) => {
      const c = v / 255;
      return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
    };
    const [r, g, b] = [1, 3, 5].map((i) => channel(parseInt(hex.slice(i, i + 2), 16)));
    return 0.2126 * r! + 0.7152 * g! + 0.0722 * b!;
  }

  function contrast(a: string, b: string): number {
    const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
    return (hi! + 0.05) / (lo! + 0.05);
  }

  /** WCAG's floor for a non-text graphical object, which is what a survey line is. */
  const FLOOR = 3;

  const ALL_THEMES = Object.keys(themeTokens);

  it('light-canvas themes are exactly the ones with a light map background', () => {
    // Pins the hand-written LIGHT_CANVAS_THEMES set in config.ts against the
    // tokens it is meant to describe, so adding a theme cannot silently hand it
    // the wrong palette. `light` and `kaleido_eink` override --cc-map-background;
    // everyone else, including the chrome-only light themes, inherits the dark one.
    for (const theme of ALL_THEMES) {
      const canvasIsLight = luminance(mapCanvasFor(theme)) > 0.5;
      const usesLightDefaults =
        getThemedSectionColorDefaults(theme).palette[0] === lightSectionColorDefaults.palette[0];
      expect(
        usesLightDefaults,
        `${theme}: canvas ${mapCanvasFor(theme)} is ${canvasIsLight ? 'light' : 'dark'} but it ` +
          `gets the ${usesLightDefaults ? 'light' : 'dark'} palette`
      ).toBe(canvasIsLight);
    }
  });

  // #357 item 9. A section keeps whatever colour it was created with - per-project
  // user data, never rewritten on a theme flip - so the only place this can be
  // fixed is what a NEW project seeds. Every colour that seed can hand out has to
  // be legible on the canvas of the theme it was seeded under.
  it.each(ALL_THEMES)('%s: every seedable section colour clears 3:1 on its canvas', (theme) => {
    const canvas = mapCanvasFor(theme);
    for (const color of getThemedSectionColorDefaults(theme).palette) {
      const ratio = contrast(color, canvas);
      expect(
        ratio,
        `${theme}: section colour ${color} on ${canvas} is ${ratio.toFixed(2)}:1, below ${FLOOR}:1`
      ).toBeGreaterThanOrEqual(FLOOR);
    }
  });

  // #357 item 17. The ruler's measured path is tinted with this, over the canvas.
  it.each(ALL_THEMES)('%s: the default measure-path colour clears 3:1 on its canvas', (theme) => {
    const canvas = mapCanvasFor(theme);
    const color = getThemedDisplayDefaults(theme).measure_path_color;
    const ratio = contrast(color, canvas);
    expect(
      ratio,
      `${theme}: measure path ${color} on ${canvas} is ${ratio.toFixed(2)}:1, below ${FLOOR}:1`
    ).toBeGreaterThanOrEqual(FLOOR);
  });

  it('would reject the values these two guards were added for', () => {
    // Proves neither guard is vacuous. Both of these shipped: `#22d3ee` was the
    // light theme's own measure-path default, and `#c7873a` was an entry of the
    // "light" section palette. Measured on the light canvas they were written for.
    const lightCanvas = themeTokens.light['--cc-map-background'];
    expect(contrast('#22d3ee', lightCanvas)).toBeLessThan(2);
    expect(contrast('#c7873a', lightCanvas)).toBeLessThan(FLOOR);
    // ...and the dark palette on the light canvas, which is what item 9 describes
    // a light-mode user actually seeing today for an already-created project.
    for (const color of defaultConfig.section_colors.palette) {
      expect(contrast(color, lightCanvas)).toBeLessThan(FLOOR);
    }
  });
});
