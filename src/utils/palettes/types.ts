/**
 * Colour-scale types for the map's depth / distance colouring and the image
 * export's depth / distance modes.
 *
 * A colour scale is a palette (what colours) plus a Levels mapping (which
 * values land where on it). The palette choice persists in config.toml
 * (`[map_palette]`); the Levels mapping is session-only. Neither is ever
 * written to the .stalx or sent to SpeleoDB. See docs/color-palettes.md.
 */

/** What a colour scale colours by. */
export type ColorScaleKind = 'depth' | 'distance';

/** The palette id every kind defaults to: the active theme's own ramp. */
export const THEME_PALETTE_ID = 'theme';

/** A palette selection as persisted in `[map_palette]`. */
export interface PaletteChoice {
  readonly id: string;
  readonly reversed: boolean;
}

export const DEFAULT_PALETTE_CHOICE: PaletteChoice = { id: THEME_PALETTE_ID, reversed: false };

/**
 * A Photoshop-Levels-style mapping from a value to a palette position:
 * values at or below `min` take the palette's start, at or above `max` its
 * end, and `gamma` bends the middle (`t' = t^(1/gamma)`, so the midpoint
 * handle sits at `0.5^gamma`). gamma 1 is linear.
 */
export interface LevelsScale {
  readonly min: number;
  readonly max: number;
  readonly gamma: number;
}

/**
 * Palette data as generated / authored. `stops` data is flat
 * `pos, r, g, b` quadruples (pos 0..1 ascending, channels 0..255);
 * `discrete` data is flat `r, g, b` triples drawn as equal hard bands.
 */
export interface GeneratedPalette {
  readonly id: string;
  readonly name: string;
  readonly group: string;
  readonly kind: 'stops' | 'discrete';
  readonly data: readonly number[];
}

/** A palette in the registry. `theme` palettes resolve their colours from
 *  the active theme and kind at use time, so they carry no data. */
export interface Palette {
  readonly id: string;
  readonly name: string;
  readonly group: string;
  readonly kind: 'stops' | 'discrete' | 'theme';
  readonly data: readonly number[];
}
