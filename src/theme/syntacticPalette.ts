/**
 * Syntactic-highlighting palette: the semantic color "roles" that teach users
 * what an element/tool acts on, the built-in presets (Default + colorblind-safe),
 * the tool→role mapping, and a resolver that layers user overrides on a preset.
 *
 * Object color encodes WHAT (segment/line/shot/station/comment); a shared
 * modifier encodes the ACTION (delete = red, move = slate). See the
 * project's Cave Survey icon design spec.
 *
 * Base preset colors live here (NOT in persisted config). The config stores only
 * sparse per-preset overrides; the resolved color for a role is
 * `overrides[preset][role] ?? SYNTACTIC_PRESETS[preset][role]`.
 */
import type { ToolType } from '../store/uiStore';
import type { SyntacticHighlightPreset, SyntacticHighlightLevel } from '../types/config';

/** Severity rank of each highlighting level (off < toolbar < icons < full). */
const LEVEL_RANK: Record<string, number> = {
  off: 0,
  toolbar_only: 1,
  icons: 2,
  full: 3,
  unknown: 3, // unknown normalizes to full
};

/** Whether the active level is at least `target` (e.g. action icons show at icons+full). */
export function syntacticLevelAtLeast(
  level: SyntacticHighlightLevel,
  target: SyntacticHighlightLevel,
): boolean {
  return (LEVEL_RANK[level] ?? 3) >= (LEVEL_RANK[target] ?? 3);
}

/** Editable color roles, grouped for the settings list. */
export type SyntacticRole =
  // entities (cave geometry), ordered by hierarchy: segment > line > shot > station
  | 'segment'
  | 'line'
  | 'shot'
  | 'station'
  | 'comment'
  // action modifiers
  | 'delete'
  | 'move'
  // functional tool accents
  | 'select'
  | 'measure'
  | 'merge'
  | 'pan'
  | 'zoom';

export type SyntacticRoleGroup = 'entity' | 'action' | 'tool';

export interface SyntacticRoleDef {
  id: SyntacticRole;
  group: SyntacticRoleGroup;
  /** i18n key under `settings.syntactic.roles`. */
  labelKey: string;
}

/** Ordered roles for the settings UI (entities first, then actions, then tools). */
export const SYNTACTIC_ROLES: SyntacticRoleDef[] = [
  { id: 'segment', group: 'entity', labelKey: 'settings.syntactic.roles.segment' },
  { id: 'line', group: 'entity', labelKey: 'settings.syntactic.roles.section' },
  { id: 'shot', group: 'entity', labelKey: 'settings.syntactic.roles.shot' },
  { id: 'station', group: 'entity', labelKey: 'settings.syntactic.roles.station' },
  { id: 'comment', group: 'entity', labelKey: 'settings.syntactic.roles.comment' },
  { id: 'delete', group: 'action', labelKey: 'settings.syntactic.roles.delete' },
  { id: 'move', group: 'action', labelKey: 'settings.syntactic.roles.move' },
  { id: 'select', group: 'tool', labelKey: 'settings.syntactic.roles.select' },
  { id: 'measure', group: 'tool', labelKey: 'settings.syntactic.roles.measure' },
  { id: 'merge', group: 'tool', labelKey: 'settings.syntactic.roles.merge' },
  { id: 'pan', group: 'tool', labelKey: 'settings.syntactic.roles.pan' },
  { id: 'zoom', group: 'tool', labelKey: 'settings.syntactic.roles.zoom' },
];

export type SyntacticPalette = Record<SyntacticRole, string>;

/**
 * Default preset - the spec palette (`icons.jsx`). Hex values for the oklch
 * entries are exact sRGB conversions of the spec's oklch colors so the color
 * pickers stay lossless.
 */
const DEFAULT_PALETTE: SyntacticPalette = {
  segment: '#ff8000', // Legendary orange
  line: '#a335ee', // Epic purple
  shot: '#0070dd', // Rare blue
  station: '#5fcf83', // Uncommon green  (oklch 0.77 0.15 152)
  comment: '#eab946', // amber           (oklch 0.81 0.14 85)
  delete: '#f75d59', // red              (oklch 0.68 0.19 25)
  move: '#c0d0e1', // slate              (oklch 0.85 0.03 250)
  select: '#c2cfde', // neutral pointer  (oklch 0.85 0.025 250)
  measure: '#e787d3', // pink            (oklch 0.75 0.15 335)
  merge: '#35cbd2', // cyan              (oklch 0.77 0.12 200)
  pan: '#63c0ee', // sky                 (oklch 0.77 0.11 232)
  zoom: '#84a8fd', // indigo             (oklch 0.74 0.13 266)
};

// Okabe-Ito colorblind-safe palette - mutually distinguishable across the major
// CVD types. Used as the source colors for the colorblind presets below.
const OI = {
  orange: '#e69f00',
  sky: '#56b4e9',
  green: '#009e73',
  yellow: '#f0e442',
  blue: '#0072b2',
  vermillion: '#d55e00',
  purple: '#cc79a7',
  gray: '#999999',
  slate: '#aeb6c2',
} as const;

/**
 * Deuteranopia (green-weak) - lean on the orange↔blue↔yellow↔purple axis;
 * station uses sky-blue (separated from shot's dark blue by lightness) to avoid
 * a green that could blur against warm hues.
 */
const DEUTERANOPIA_PALETTE: SyntacticPalette = {
  segment: OI.orange,
  line: OI.purple,
  shot: OI.blue,
  station: OI.sky,
  comment: OI.yellow,
  delete: OI.vermillion,
  move: OI.slate,
  select: OI.slate,
  measure: OI.purple,
  merge: OI.green,
  pan: OI.sky,
  zoom: OI.blue,
};

/**
 * Protanopia (red-weak) - same safe axis; since none of the entities are red,
 * station can stay bluish-green (red-green confusion only bites against reds,
 * and `delete`'s vermillion lives in a distinct action context).
 */
const PROTANOPIA_PALETTE: SyntacticPalette = {
  segment: OI.orange,
  line: OI.purple,
  shot: OI.blue,
  station: OI.green,
  comment: OI.yellow,
  delete: OI.vermillion,
  move: OI.slate,
  select: OI.slate,
  measure: OI.purple,
  merge: OI.sky,
  pan: OI.sky,
  zoom: OI.blue,
};

/**
 * Tritanopia (blue-yellow weak) - favor the red↔green↔pink axis and lightness
 * contrast; blue vs green are separated by Okabe-Ito's lightness spread.
 */
const TRITANOPIA_PALETTE: SyntacticPalette = {
  segment: OI.vermillion,
  line: OI.purple,
  shot: OI.blue,
  station: OI.green,
  comment: OI.orange,
  delete: OI.vermillion,
  move: OI.slate,
  select: OI.slate,
  measure: OI.purple,
  merge: OI.green,
  pan: OI.sky,
  zoom: OI.blue,
};

/** Built-in preset palettes, keyed by preset id. */
export const SYNTACTIC_PRESETS: Record<string, SyntacticPalette> = {
  default: DEFAULT_PALETTE,
  deuteranopia: DEUTERANOPIA_PALETTE,
  protanopia: PROTANOPIA_PALETTE,
  tritanopia: TRITANOPIA_PALETTE,
};

/** Fallback preset when an unknown value is encountered. */
const FALLBACK_PRESET = 'default';

/**
 * Map each tool to the role that drives its icon's object color. The move group
 * tools reuse their entity's color and add the slate `move` modifier in the icon
 * itself.
 */
export const TOOL_ROLE_MAP: Record<ToolType, SyntacticRole> = {
  select: 'select',
  pan: 'pan',
  zoom: 'zoom',
  merge: 'merge',
  measure: 'measure',
  move_station: 'station',
  move_segment: 'segment',
  move_shot: 'shot',
  comment: 'comment',
  move_comment: 'comment',
};

/** Entities that can render a colored blob in Full mode. */
export type BlobEntity = 'segment' | 'line' | 'shot' | 'station' | 'comment';

/**
 * Resolve the effective palette for a preset, layering any user overrides on top
 * of the preset's base colors. Unknown presets fall back to Default.
 */
export function resolveSyntacticColors(
  preset: SyntacticHighlightPreset,
  overrides: Record<string, Record<string, string>> | undefined,
): SyntacticPalette {
  const base = SYNTACTIC_PRESETS[preset] ?? SYNTACTIC_PRESETS[FALLBACK_PRESET]!;
  const presetOverrides = overrides?.[preset] ?? {};
  const resolved = { ...base };
  for (const { id } of SYNTACTIC_ROLES) {
    const override = presetOverrides[id];
    if (typeof override === 'string' && /^#[0-9a-fA-F]{6}$/.test(override)) {
      resolved[id] = override;
    }
  }
  return resolved;
}

/** CSS custom-property name for a role, e.g. `--sh-station`. */
export function syntacticVarName(role: SyntacticRole): string {
  return `--sh-${role}`;
}
