import { getMapThemeColors } from './themeColors';
import type { ExportImageOptions } from '../types/commands';
import type { ExportColorScale, MapPaletteConfig } from '../types/ipc.generated';
import { resolveChoice } from './palettes';
import {
  DEFAULT_PALETTE_CHOICE,
  THEME_PALETTE_ID,
  type ColorScaleKind,
  type PaletteKind,
  type LevelsScale,
  type PaletteChoice,
} from './palettes/types';

/** The two image-export options that pick what a station's label says. */
export type StationLabelOption = 'show_station_labels' | 'show_station_names';

/**
 * Tick or untick one of the station-label options. Station IDs and station
 * names are one choice, like the Display menu's pair: ticking one unticks the
 * other, and unticking one leaves the other alone (no station labels). The
 * backend draws the IDs if both still arrive set. Returns a new object.
 */
export function setStationLabelOption(
  options: ExportImageOptions,
  key: StationLabelOption,
  on: boolean,
): ExportImageOptions {
  const partner: StationLabelOption = key === 'show_station_labels' ? 'show_station_names' : 'show_station_labels';
  return { ...options, [key]: on, ...(on ? { [partner]: false } : {}) };
}

/**
 * Stamp the live theme's depth-gradient endpoints onto an export options object.
 *
 * The backend cannot resolve these itself: #332 made every theme declare its own
 * `--cc-depth-shallow` / `--cc-depth-deep`, so the constants in `image.rs` match
 * the dark theme alone (#360). Anything that renders survey geometry through
 * `ExportImageOptions` has to send what the document currently resolves.
 *
 * BOTH commands that take these options must go through here - `export_image`
 * writes the file, `preview_svg_export` draws the dialog's WYSIWYG preview, and
 * the preview's whole promise is that it cannot drift from the file. `format`
 * (not the returned markup) is what decides whether colour is emitted, so a
 * preview of a PNG/PDF export *is* coloured and would otherwise show the dark
 * ramp beside a themed export.
 *
 * Read per call, never cached: the theme can change between two exports.
 * Returns a new object; the caller's own options are left untouched.
 *
 * This relies on `getComputedStyle().getPropertyValue()` handing back a custom
 * property's token as authored rather than normalising it - the Rust side takes
 * `#rrggbb` only, and falls back silently otherwise. Verified in Chromium
 * (returns `#a89060` for `--cc-depth-shallow`, not `rgb(...)`); jsdom agreeing
 * would not have been evidence for the shipped webview. `tokens.test.ts` pins
 * the other half, that every theme declares those two as hex in the first place.
 */
export function withThemeDepthRamp(options: ExportImageOptions): ExportImageOptions {
  const { depthShallow, depthDeep } = getMapThemeColors();
  return { ...options, depth_shallow: depthShallow, depth_deep: depthDeep };
}

/** What the export's colour scales are built from: the palette choice per kind
 *  (persisted `[map_palette]`, or the picker's live preview) and the export's
 *  own session Levels range per kind. */
export interface ExportScaleInputs {
  depth: { choice: PaletteChoice; scale: LevelsScale | null };
  distance: { choice: PaletteChoice; scale: LevelsScale | null };
}

/** Entries in the palette table sent to the exporter. */
export const EXPORT_PALETTE_ENTRIES = 256;

/** The export inputs from the persisted palettes and the colour-scale session
 *  state (a live picker preview wins for its kind, so the export preview
 *  follows the picker like the map does). */
export function exportScaleInputsFrom(
  mapPalette: Pick<MapPaletteConfig, 'depth' | 'distance'> | undefined,
  session: { exportScale: Readonly<Record<ColorScaleKind, LevelsScale | null>>; palettePreview: { kind: PaletteKind; choice: PaletteChoice } | null },
): ExportScaleInputs {
  const choice = (kind: ColorScaleKind): PaletteChoice =>
    session.palettePreview?.kind === kind
      ? session.palettePreview.choice
      : (mapPalette?.[kind] ?? DEFAULT_PALETTE_CHOICE);
  return {
    depth: { choice: choice('depth'), scale: session.exportScale.depth },
    distance: { choice: choice('distance'), scale: session.exportScale.distance },
  };
}

/** One kind's `ExportColorScale`, or undefined for the untouched default so the
 *  exporter keeps its original ramp and range byte for byte. */
function exportScaleFor(
  kind: ColorScaleKind,
  { choice, scale }: { choice: PaletteChoice; scale: LevelsScale | null },
): ExportColorScale | undefined {
  const defaultPalette = choice.id === THEME_PALETTE_ID && !choice.reversed;
  if (defaultPalette && scale === null) return undefined;
  const out: ExportColorScale = {};
  if (!defaultPalette) {
    const palette = resolveChoice(choice, kind, getMapThemeColors());
    const table: string[] = [];
    for (let i = 0; i < EXPORT_PALETTE_ENTRIES; i++) {
      table.push(palette.hex(i / (EXPORT_PALETTE_ENTRIES - 1)) ?? '#808080');
    }
    out.palette = table;
  }
  if (scale) out.levels = { min: scale.min, max: scale.max, gamma: scale.gamma };
  return out;
}

/**
 * {@link withThemeDepthRamp} plus the user's depth / distance colour scales.
 * Both image commands go through here (the export and its preview), for the
 * same can't-drift reason as the theme endpoints.
 */
export function withColorScales(options: ExportImageOptions, inputs: ExportScaleInputs): ExportImageOptions {
  const out: ExportImageOptions = withThemeDepthRamp(options);
  const depth = exportScaleFor('depth', inputs.depth);
  const distance = exportScaleFor('distance', inputs.distance);
  if (depth) out.depth_scale = depth;
  else delete out.depth_scale;
  if (distance) out.distance_scale = distance;
  else delete out.distance_scale;
  return out;
}
