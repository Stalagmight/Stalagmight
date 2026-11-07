/**
 * The placement-mode dim of the toolbar, drawn without a CSS filter (#576 NF-W12DMP-1, #605).
 *
 * On WebKitGTK's GPU painter every repaint of an element under a CSS `filter` paints through
 * a GPU promise image, and a critical memory release that lands while one is in flight
 * deadlocks the web process. The toolbar's placement-mode dim was `opacity-45 grayscale`.
 * Every colour the toolbar paints comes from a custom property (the theme tokens and the
 * syntactic palette), and `grayscale(1)` is a linear map of sRGB, so re-declaring each of
 * them, desaturated, on the dimmed element paints what the filter painted, within 8-bit
 * rounding (measured in Chromium on every theme: no pixel off by more than 7/255).
 */
import { themeTokens } from '../theme/tokens';
import { SYNTACTIC_ROLES, syntacticVarName } from '../theme/syntacticPalette';

/** The sRGB weights of `grayscale(1)` (Filter Effects 1, the grayscale matrix at amount 1). */
const WEIGHTS = [0.2126, 0.7152, 0.0722] as const;

/** Round to 3 decimals: enough for an 8-bit channel, and stable to compare. */
const round = (v: number) => Math.round(v * 1000) / 1000;

function parseColor(value: string): [number, number, number, number] | null {
  const v = value.trim().toLowerCase();
  if (v === 'transparent') return [0, 0, 0, 0];
  const hex = /^#([0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$/.exec(v)?.[1];
  if (hex) {
    const full = hex.length <= 4 ? [...hex].map((c) => c + c).join('') : hex;
    const channels = (full.match(/../g) ?? []).map((h) => parseInt(h, 16));
    const [r = 0, g = 0, b = 0, a = 255] = channels;
    return [r, g, b, a / 255];
  }
  const fn = /^rgba?\(([^)]*)\)$/.exec(v)?.[1];
  if (fn === undefined) return null;
  const parts = fn.split(/[\s,/]+/).filter(Boolean);
  if (parts.length < 3 || parts.length > 4) return null;
  const nums = parts.map((p) => (p.endsWith('%') ? Number(p.slice(0, -1)) / 100 : Number(p)));
  if (nums.some((n) => !Number.isFinite(n)) || parts.slice(0, 3).some((p) => p.endsWith('%'))) return null;
  const [r = 0, g = 0, b = 0, a = 1] = nums;
  return [r, g, b, a];
}

/** `value` desaturated as `grayscale(1)` paints it, or null when it is not a plain colour. */
export function grayscaleColor(value: string): string | null {
  const rgba = parseColor(value);
  if (!rgba) return null;
  const [r, g, b, a] = rgba;
  const l = round(WEIGHTS[0] * r + WEIGHTS[1] * g + WEIGHTS[2] * b);
  return a === 1 ? `rgb(${l} ${l} ${l})` : `rgb(${l} ${l} ${l} / ${round(a)})`;
}

/** Every custom property the app's colours come from: the theme tokens and the syntactic palette. */
export function colorPropertyNames(): string[] {
  return [...Object.keys(themeTokens.dark), ...SYNTACTIC_ROLES.map(({ id }) => syntacticVarName(id))];
}

/**
 * The colour custom properties as `read` gives them (the document root's computed values),
 * desaturated, ready to set on the dimmed element; a property that is not a plain colour
 * (a filter value, an empty read) is left out.
 */
export function grayscaleTokenStyle(read: (name: string) => string): Record<string, string> {
  const style: Record<string, string> = {};
  for (const name of colorPropertyNames()) {
    const gray = grayscaleColor(read(name));
    if (gray !== null) style[name] = gray;
  }
  return style;
}
