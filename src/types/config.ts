import { themeTokens } from '../theme/tokens';
import type { DisplayConfig, SectionColorsConfig } from './ipc.generated';
import { defaultConfig } from './ipc/defaultConfig';

export { defaultConfig };

export type {
  LengthUnit,
  AngleUnit,
  DecimalPrecision,
  ShortcutFeedback,
  ThemeMode,
  ColorTheme,
  AppearanceConfig,
  ImportConfig,
  OverlapMode,
  Config,
  GeneralConfig,
  UnitsConfig,
  DisplayConfig,
  SectionColorsConfig,
  GpsOrigin,
  StationConfig,
  ShotConfig,
  MapSnapConfig,
  MapAnimationConfig,
  DebugConfig,
  MapDebugConfig,
  MapConfig,
  MapControlsConfig,
  ValidationConfig,
  ClosureAdjustmentConfig,
  PlacementSuggestionConfig,
  ExportConfig,
  LoggingConfig,
  ShortcutsConfig,
  PanelsConfig,
  CacheConfig,
  GlobalViewConfig,
  SyntacticHighlightingConfig,
  SyntacticHighlightLevel,
  SyntacticHighlightPreset,
} from './ipc.generated';

/**
 * Light-theme display defaults. Used when "Reset to defaults" is triggered
 * while a light-CANVAS theme is active. Colors are chosen for visibility on
 * the light map canvas (`--cc-map-background`), which is what they are drawn on.
 */
export const lightDisplayDefaults: DisplayConfig = {
  grid_color: '#c1a07a',
  grid_spacing: 10,
  font_size: 12,
  font_color: themeTokens.light['--cc-text-primary'],
  // #357 item 17: the dark default is `#22d3ee`, a bright cyan that measures
  // 1.69:1 on light's `#faf7f2` - the ruler's measured path was all but invisible
  // even after resetting Display settings, because these defaults specified the
  // same cyan. So light needs its own dark colour. Dark green, hue 130, 4.86:1
  // on `#faf7f2`, pinned by "light default measure color clears the floor" in
  // __tests__/config.test.ts. It clears the selection highlight (`#3a8a82`), the
  // hover/zero-datum gold (`#b8860b`), the snap brick (`#c0392b`) and both ends
  // of the distance/gas overlay ramp (`#f4d03f` -> `#7d3c98`).
  //
  // KNOWN COLLISION, deliberately NOT fixed here. This green was chosen when the
  // light depth ramp ran navy 211 -> brick 7 the LONG way round, through indigo
  // and magenta, which left hue 130 free. That ramp is gone: light now runs cyan
  // 175 -> red 5, descending THROUGH green and yellow, so hue 130 is no longer
  // free - it is mid-ramp. Measured separation from the nearest sample on the
  // light ramp is 15.4 dE2000, so with Depth colouring on, the ruler is drawn in
  // close to the same colour as mid-depth passage.
  //
  // Left alone because every colour on this branch is visually QA'd before it
  // ships and this one has not been. If it is ever revisited, indigo near hue
  // 224 (e.g. `#1e3a8a`) measured 9.69:1 on the canvas and 43.9 dE2000 from the
  // ramp, while still clearing every overlay above (nearest: the gas deep end,
  // 18.1) - but that is a proposal, not a decision.
  measure_path_color: '#157e27',
};

/**
 * Light-theme section color palette. Darker/more saturated colors that are
 * visually distinct on light backgrounds. Every entry must clear 3:1 against the
 * light canvas (`#faf7f2`); the guard in `__tests__/config.test.ts` measures it.
 */
export const lightSectionColorDefaults: SectionColorsConfig = {
  palette: [
    '#1a8a7d',
    '#c0392b',
    '#2e7d6e',
    '#b94a4a',
    '#6b4fa0',
    '#c4587a',
    '#2f7d9e',
    // #357 item 9: was `#c7873a`, which measured 2.83:1 on `#faf7f2` - the one
    // entry of this "light" palette that was itself below the floor. Same hue,
    // darker: 4.14:1.
    '#a86a1e',
  ],
};

/**
 * Themes whose MAP CANVAS is light, so map-drawn defaults must be dark ink.
 *
 * Keyed on `--cc-map-background`, NOT on the chrome: `solarized_light` and
 * `catppuccin_latte` are chrome-only light themes that deliberately leave the
 * map-visualization tokens unset (see `mapVizTokenNames`), so their canvas is
 * `:root`'s dark `#1a1a2e` and the DARK defaults are the correct ones for them.
 * Treating them as light handed them `#6b4fa0` at 2.64:1 on that dark canvas.
 * `mapCanvasFor` below derives the same split from the tokens, and the test
 * "light-canvas themes are exactly the ones with a light map background" pins
 * this set against it so the two cannot drift.
 */
const LIGHT_CANVAS_THEMES = new Set(['light', 'kaleido_eink']);

/**
 * The map canvas a theme actually paints on. Chrome-only themes inherit `:root`'s
 * dark canvas because they declare no `--cc-map-background` of their own.
 */
export function mapCanvasFor(resolvedTheme: string): string {
  const tokens = (themeTokens as Record<string, Record<string, string> | undefined>)[resolvedTheme];
  return tokens?.['--cc-map-background'] ?? themeTokens.dark['--cc-map-background'];
}

/**
 * Get theme-appropriate display defaults.
 * Returns light-theme defaults for light-CANVAS themes, dark defaults otherwise.
 */
export function getThemedDisplayDefaults(resolvedTheme: string): DisplayConfig {
  if (LIGHT_CANVAS_THEMES.has(resolvedTheme)) {
    return { ...lightDisplayDefaults };
  }
  return { ...defaultConfig.display };
}

/**
 * Get theme-appropriate section color defaults.
 * Returns light-theme palette for light-CANVAS themes, dark palette otherwise.
 */
export function getThemedSectionColorDefaults(resolvedTheme: string): SectionColorsConfig {
  if (LIGHT_CANVAS_THEMES.has(resolvedTheme)) {
    return { palette: [...lightSectionColorDefaults.palette] };
  }
  return { palette: [...defaultConfig.section_colors.palette] };
}
