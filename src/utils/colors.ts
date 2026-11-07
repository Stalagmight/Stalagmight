/**
 * Traffic-light severity palette. Single source of truth for the
 * green / yellow / orange / red used by closure-error coloring
 * (getErrorColor) and the map's Issue-severity Display mode
 * (src/utils/themeColors.ts). Keep as literals here; consumers import.
 */
import { clamp } from './clamp';
import { mapCanvasFor } from '../types/config';

export const SEVERITY_GREEN = '#22c55e';
export const SEVERITY_YELLOW = '#eab308';
export const SEVERITY_ORANGE = '#f97316';
export const SEVERITY_RED = '#ef4444';

/**
 * Get color for closure error percentage severity.
 * Green (<1%), Yellow (1-3%), Orange (3-5%), Red (>5%)
 */
export function getErrorColor(errorPct: number): string {
  if (errorPct < 1) return SEVERITY_GREEN;
  if (errorPct < 3) return SEVERITY_YELLOW;
  if (errorPct < 5) return SEVERITY_ORANGE;
  return SEVERITY_RED;
}

/**
 * Parse a hex color to RGB values
 */
export function hexToRgb(hex: string): [number, number, number] {
  const result = /^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$/i.exec(hex);
  if (!result) {
    return [0, 0, 0];
  }
  return [
    parseInt(result[1] ?? '0', 16),
    parseInt(result[2] ?? '0', 16),
    parseInt(result[3] ?? '0', 16),
  ];
}

/**
 * Convert hex color to HSL
 * Returns null if the hex color is invalid
 * Exported for testing
 */
export function hexToHSL(hex: string): { h: number; s: number; l: number } | null {
  const result = /^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$/i.exec(hex);
  if (!result) {
    return null;
  }

  const r = parseInt(result[1] ?? '0', 16) / 255;
  const g = parseInt(result[2] ?? '0', 16) / 255;
  const b = parseInt(result[3] ?? '0', 16) / 255;

  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const l = (max + min) / 2;

  if (max === min) {
    // Achromatic (gray)
    return { h: 0, s: 0, l };
  }

  const d = max - min;
  const s = l > 0.5 ? d / (2 - max - min) : d / (max + min);

  let h: number;
  if (max === r) {
    h = ((g - b) / d + (g < b ? 6 : 0)) / 6;
  } else if (max === g) {
    h = ((b - r) / d + 2) / 6;
  } else {
    h = ((r - g) / d + 4) / 6;
  }

  return { h: h * 360, s, l };
}

/**
 * Convert HSL to hex color
 * h: 0-360, s: 0-1, l: 0-1
 * Exported for testing
 */
export function hslToHex(h: number, s: number, l: number): string {
  // Normalize h to 0-360 range
  h = ((h % 360) + 360) % 360;

  const c = (1 - Math.abs(2 * l - 1)) * s;
  const x = c * (1 - Math.abs(((h / 60) % 2) - 1));
  const m = l - c / 2;

  let r = 0;
  let g = 0;
  let b = 0;

  if (h < 60) {
    [r, g, b] = [c, x, 0];
  } else if (h < 120) {
    [r, g, b] = [x, c, 0];
  } else if (h < 180) {
    [r, g, b] = [0, c, x];
  } else if (h < 240) {
    [r, g, b] = [0, x, c];
  } else if (h < 300) {
    [r, g, b] = [x, 0, c];
  } else {
    [r, g, b] = [c, 0, x];
  }

  return rgbToHex((r + m) * 255, (g + m) * 255, (b + m) * 255);
}

/**
 * Interpolate between two hues taking the shortest path around the color wheel
 * h1, h2: 0-360, t: 0-1
 * Returns: 0-360
 * Exported for testing
 */
export function interpolateHue(h1: number, h2: number, t: number): number {
  // Calculate the difference
  let diff = h2 - h1;

  // Take the shortest path around the wheel
  if (diff > 180) {
    diff -= 360;
  } else if (diff < -180) {
    diff += 360;
  }

  // Interpolate and normalize to 0-360
  const result = h1 + diff * t;
  return ((result % 360) + 360) % 360;
}

/**
 * Convert RGB values to hex color
 */
export function rgbToHex(r: number, g: number, b: number): string {
  return (
    '#' +
    [r, g, b]
      .map((x) => {
        const hex = Math.round(x).toString(16);
        return hex.length === 1 ? '0' + hex : hex;
      })
      .join('')
  );
}

/**
 * Interpolate between two colors based on a value between 0 and 1
 */
export function interpolateColor(
  color1: string,
  color2: string,
  t: number
): string {
  const [r1, g1, b1] = hexToRgb(color1);
  const [r2, g2, b2] = hexToRgb(color2);

  const r = r1 + (r2 - r1) * t;
  const g = g1 + (g2 - g1) * t;
  const b = b1 + (b2 - b1) * t;

  return rgbToHex(r, g, b);
}

/**
 * Get a color for a depth value using HSL gradient interpolation
 * Returns null for invalid inputs - caller should fallback to section color
 *
 * @param depth - The depth value to get color for
 * @param shallowColor - Color for shallowest depth. REQUIRED - there is no
 *   default; callers resolve it from the live theme's `--cc-depth-shallow`
 *   (see `getMapThemeColors`). The hexes previously named here as defaults
 *   were never defaults and no longer match any theme.
 * @param deepColor - Color for deepest depth. REQUIRED, as above, from
 *   `--cc-depth-deep`.
 * @param minDepth - Shallowest depth value (smallest z)
 * @param maxDepth - Deepest depth value (largest z)
 * @returns Hex color string or null if inputs are invalid
 */
export function getDepthColor(
  depth: number,
  shallowColor: string,
  deepColor: string,
  minDepth: number,
  maxDepth: number
): string | null {
  // Return null for invalid inputs - caller should fallback to section color
  // Use Number.isFinite for strict validation (doesn't coerce non-numbers)
  if (!Number.isFinite(depth) || !Number.isFinite(minDepth) || !Number.isFinite(maxDepth)) {
    return null;
  }

  // Handle equal depths (flat cave) - validate shallowColor first
  if (maxDepth === minDepth) {
    // Validate shallowColor is a valid hex before returning
    return hexToHSL(shallowColor) ? shallowColor : null;
  }

  // Clamp depth to range
  const clampedDepth = clamp(depth, minDepth, maxDepth);
  const t = (clampedDepth - minDepth) / (maxDepth - minDepth);

  // Convert hex to HSL
  const shallowHSL = hexToHSL(shallowColor);
  const deepHSL = hexToHSL(deepColor);

  if (!shallowHSL || !deepHSL) {
    return null; // Invalid color format
  }

  // Interpolate in HSL space (shortest hue path)
  const h = interpolateHue(shallowHSL.h, deepHSL.h, t);
  const s = shallowHSL.s + (deepHSL.s - shallowHSL.s) * t;
  const l = shallowHSL.l + (deepHSL.l - shallowHSL.l) * t;

  return hslToHex(h, s, l);
}

/**
 * Convert hex color to rgba string with alpha
 */
export function hexToRgba(hex: string, alpha: number): string {
  const [r, g, b] = hexToRgb(hex);
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}

/**
 * Lighten a color by a percentage (0-100)
 */
export function lightenColor(hex: string, percent: number): string {
  const [r, g, b] = hexToRgb(hex);
  const factor = percent / 100;

  return rgbToHex(
    r + (255 - r) * factor,
    g + (255 - g) * factor,
    b + (255 - b) * factor
  );
}

/**
 * Darken a color by a percentage (0-100)
 */
export function darkenColor(hex: string, percent: number): string {
  const [r, g, b] = hexToRgb(hex);
  const factor = 1 - percent / 100;

  return rgbToHex(r * factor, g * factor, b * factor);
}

/**
 * Check if a color is light (for choosing contrast text color)
 */
export function isLightColor(hex: string): boolean {
  const [r, g, b] = hexToRgb(hex);
  // Using perceived luminance formula
  const luminance = (0.299 * r + 0.587 * g + 0.114 * b) / 255;
  return luminance > 0.5;
}

/**
 * Get a contrasting text color for a background
 */
export function getContrastColor(backgroundColor: string): string {
  return isLightColor(backgroundColor) ? '#000000' : '#ffffff';
}

/**
 * Default section color palette
 */
export const SECTION_COLORS = [
  '#4ecdc4', // Teal
  '#ff6b6b', // Coral
  '#95e1d3', // Mint
  '#f38181', // Salmon
  '#aa96da', // Lavender
  '#fcbad3', // Pink
  '#a8d8ea', // Sky Blue
  '#ffcb77', // Peach
];

/**
 * Light-CANVAS counterpart of `SECTION_COLORS` (#357 item 9). `SECTION_COLORS`
 * is dark-tuned pastel and draws at ~1.5:1 or worse on the light map canvas
 * (`#faf7f2`), so a survey seeded under a light theme got barely-visible
 * section lines. Section colour is per-project user data - a section keeps
 * whatever colour it was created with, so this only affects what a NEW
 * section seeds, never an already-persisted one.
 *
 * Same hue order/character as `SECTION_COLORS` (teal, coral, mint, salmon,
 * lavender, pink, sky blue, peach), darker/more saturated so every entry
 * clears WCAG's 3:1 floor for a non-text graphical object against `#faf7f2`.
 * Measured (not eyeballed) in `__tests__/colors.test.ts`, which also asserts
 * every pair is distinguishable from its neighbours.
 */
export const LIGHT_SECTION_COLORS = [
  '#1a8a7d', // Teal
  '#c0392b', // Coral
  '#2e7d6e', // Mint
  '#b94a4a', // Salmon
  '#6b4fa0', // Lavender
  '#c4587a', // Pink
  '#2f7d9e', // Sky Blue
  '#a86a1e', // Peach
];

/**
 * Move-comment tool palette (#52). Vivid, high-saturation colors used to
 * tint each comment label and the shot/station it belongs to, so the two
 * are visually paired while repositioning. Deliberately distinct from the
 * muted `SECTION_COLORS` above (so a tint reads clearly even when coloring
 * by section) and saturated/bright enough to stay readable on the dark map
 * background.
 *
 * Assignment (see `commentColorForKey` in `surveyLayer/features.ts`) is a
 * deterministic hash of each comment's owner identity (section + shot/station),
 * not a build-order ordinal, so a color stays stable across adds/removes/moves
 * of other comments. Two accepted tradeoffs of hashing into only 8 slots:
 * nearby comments can collide on the same color, and a survey with more than
 * 8 comments is guaranteed to have at least one collision somewhere. Both are
 * cosmetic (color is a visual pairing aid, not a unique ID).
 */
export const COMMENT_PALETTE = [
  '#ff3b30', // Red
  '#ff9500', // Orange
  '#ffd60a', // Yellow
  '#34c759', // Green
  '#0a84ff', // Blue
  '#5e5ce6', // Indigo
  '#bf5af2', // Purple
  '#ff2d92', // Magenta
];

/**
 * Generate a random color with good saturation and lightness
 * that is visually distinct from existing colors
 */
function generateRandomColor(usedColors: string[]): string {
  // Generate random hue, avoiding hues too similar to existing colors
  const usedHues = usedColors.map((c) => {
    const [r, g, b] = hexToRgb(c);
    // Convert RGB to HSL to get hue
    const max = Math.max(r, g, b);
    const min = Math.min(r, g, b);
    if (max === min) return 0;
    let h = 0;
    if (max === r) {
      h = (g - b) / (max - min);
    } else if (max === g) {
      h = 2 + (b - r) / (max - min);
    } else {
      h = 4 + (r - g) / (max - min);
    }
    h *= 60;
    if (h < 0) h += 360;
    return h;
  });

  // Find a hue that's at least 30 degrees away from all used hues
  let bestHue = Math.random() * 360;
  let bestMinDistance = 0;

  for (let attempt = 0; attempt < 20; attempt++) {
    const candidateHue = Math.random() * 360;
    const minDistance = Math.min(
      ...usedHues.map((h) => {
        const diff = Math.abs(candidateHue - h);
        return Math.min(diff, 360 - diff);
      }),
      360 // Default if no used hues
    );
    if (minDistance > bestMinDistance) {
      bestMinDistance = minDistance;
      bestHue = candidateHue;
    }
  }

  // Convert HSL to RGB with good saturation (60-80%) and lightness (50-60%)
  const h = bestHue;
  const s = 0.6 + Math.random() * 0.2; // 60-80% saturation
  const l = 0.5 + Math.random() * 0.1; // 50-60% lightness

  // HSL to RGB conversion
  const c = (1 - Math.abs(2 * l - 1)) * s;
  const x = c * (1 - Math.abs(((h / 60) % 2) - 1));
  const m = l - c / 2;

  let r = 0,
    g = 0,
    b = 0;
  if (h < 60) {
    [r, g, b] = [c, x, 0];
  } else if (h < 120) {
    [r, g, b] = [x, c, 0];
  } else if (h < 180) {
    [r, g, b] = [0, c, x];
  } else if (h < 240) {
    [r, g, b] = [0, x, c];
  } else if (h < 300) {
    [r, g, b] = [x, 0, c];
  } else {
    [r, g, b] = [c, 0, x];
  }

  return rgbToHex((r + m) * 255, (g + m) * 255, (b + m) * 255);
}

/**
 * WCAG relative luminance (0-1). Shared by `contrastRatio` and the
 * canvas-luminance check `isLightCanvasTheme` uses to pick a section palette.
 */
export function relativeLuminance(hex: string): number {
  const channel = (v: number) => {
    const c = v / 255;
    return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
  };
  const [r, g, b] = hexToRgb(hex);
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

/**
 * WCAG contrast ratio between two colors, 1:1 (identical) to 21:1
 * (black/white).
 */
export function contrastRatio(a: string, b: string): number {
  const [hi, lo] = [relativeLuminance(a), relativeLuminance(b)].sort((x, y) => y - x);
  return (hi! + 0.05) / (lo! + 0.05);
}

/**
 * Whether `resolvedTheme`'s MAP CANVAS - not its chrome - is light. Themes
 * like `solarized_light` and `catppuccin_latte` have light chrome but leave
 * `--cc-map-background` unset, so they inherit `:root`'s dark canvas and must
 * NOT get the light section palette; `mapCanvasFor` resolves that distinction.
 */
export function isLightCanvasTheme(resolvedTheme: string): boolean {
  return relativeLuminance(mapCanvasFor(resolvedTheme)) > 0.5;
}

/**
 * The section-color palette a NEW section seeded under `resolvedTheme` should
 * draw from - `LIGHT_SECTION_COLORS` on a light canvas, `SECTION_COLORS`
 * otherwise. Never applies to a section's already-persisted color.
 */
export function sectionColorPalette(resolvedTheme: string): string[] {
  return isLightCanvasTheme(resolvedTheme) ? LIGHT_SECTION_COLORS : SECTION_COLORS;
}

/**
 * Get the next available color from the theme-appropriate palette.
 */
export function getNextSectionColor(usedColors: string[], resolvedTheme: string): string {
  for (const color of sectionColorPalette(resolvedTheme)) {
    if (!usedColors.includes(color)) {
      return color;
    }
  }
  // If all palette colors used, generate a random distinct color
  return generateRandomColor(usedColors);
}
