/**
 * The palette registry: the Theme default, matplotlib's colormaps (generated)
 * and the hand-authored gradient folders, plus lookup, search and resolution.
 * See docs/color-palettes.md.
 */
import { CURATED_PALETTES } from './catalog.curated';
import { GENERATED_PALETTES } from './catalog.generated';
import { resolvePalette, type ResolvedPalette, type ThemeRampEndpoints } from './sample';
import { THEME_PALETTE_ID, type Palette, type PaletteChoice, type PaletteKind } from './types';

export * from './types';
export * from './levels';
export * from './sample';
export * from './scale';

export const THEME_PALETTE: Palette = {
  id: THEME_PALETTE_ID,
  name: 'Theme',
  group: 'theme',
  kind: 'theme',
  data: [],
};

/** Group display order. `pinned` is virtual: the picker fills it from config. */
export const PALETTE_GROUPS = [
  'pinned',
  'theme',
  'mpl-perceptual',
  'mpl-sequential',
  'mpl-sequential-2',
  'mpl-diverging',
  'mpl-cyclic',
  'mpl-qualitative',
  'mpl-misc',
  'grad-basics',
  'grad-spectrum',
  'grad-blues',
  'grad-greens',
  'grad-reds',
  'grad-purples',
  'grad-earth',
  'grad-grays',
  'grad-metals',
] as const;

export type PaletteGroupId = (typeof PALETTE_GROUPS)[number];

export const ALL_PALETTES: readonly Palette[] = [THEME_PALETTE, ...GENERATED_PALETTES, ...CURATED_PALETTES];

const BY_ID = new Map<string, Palette>(ALL_PALETTES.map((p) => [p.id, p]));

/** The palette with this id, or undefined (an id from another build, say). */
export function findPalette(id: string): Palette | undefined {
  return BY_ID.get(id);
}

/** The palette with this id, falling back to Theme for an unknown id. */
export function getPalette(id: string): Palette {
  return BY_ID.get(id) ?? THEME_PALETTE;
}

/**
 * Split a search query into its text and whether it asked for the reversed
 * palette with matplotlib's `_r` suffix (`viridis_r`).
 */
export function parsePaletteQuery(query: string): { text: string; reversed: boolean } {
  const q = query.trim().toLowerCase();
  if (q.length > 2 && q.endsWith('_r')) return { text: q.slice(0, -2), reversed: true };
  return { text: q, reversed: false };
}

/** Whether a palette matches a search: every word must appear in its name,
 *  id or group label (case-insensitive). An empty query matches everything. */
export function paletteMatches(palette: Palette, text: string, groupLabel: string): boolean {
  if (!text) return true;
  const hay = `${palette.name} ${palette.id} ${groupLabel}`.toLowerCase();
  return text.split(/\s+/).every((word) => hay.includes(word));
}

// Resolution is memoised by (palette, direction, kind, theme endpoints): the
// renderer resolves on every colour sync, and parsing the theme endpoints each
// time would be wasted work. Tiny and bounded: one entry per combination seen.
const resolved = new Map<string, ResolvedPalette>();

export function resolveChoice(
  choice: PaletteChoice,
  kind: PaletteKind,
  endpoints: ThemeRampEndpoints,
): ResolvedPalette {
  const palette = getPalette(choice.id);
  const cacheKey =
    palette.kind === 'theme'
      ? `${kind}|${choice.reversed ? 1 : 0}|${endpoints.depthShallow}|${endpoints.depthDeep}`
      : `${palette.id}|${choice.reversed ? 1 : 0}`;
  let hit = resolved.get(cacheKey);
  if (!hit) {
    if (resolved.size > 256) resolved.clear();
    hit = resolvePalette(palette, choice.reversed, kind, endpoints);
    resolved.set(cacheKey, hit);
  }
  return hit;
}
