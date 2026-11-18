/**
 * CSS color -> packed RGBA8, for the custom-render prototype's per-vertex
 * color attribute (#customRender).
 *
 * The survey build resolves every shot / station color as a CSS string (hex
 * `#rrggbb` from line metadata / theme tokens, occasionally an `rgb()` /
 * `rgba()`). The custom WebGL layer wants those as four normalized `u8`
 * channels in a vertex attribute, so this module parses the common CSS forms
 * without a DOM canvas (keeps it pure + unit-testable in the `node` vitest
 * environment).
 *
 * Depth / severity ramps are OUT of scope for the prototype color path only in
 * the sense that no NEW ramp is invented here - the caller still passes whatever
 * `resolveShotDisplayColor` produced (which already folds in the ramps), so
 * fidelity matches the stock layers for whatever mode is active.
 */

/** Packed color as four 0..255 channels. */
export type Rgba8 = readonly [r: number, g: number, b: number, a: number];

/** Visible fallback so an unparseable color surfaces in an A/B rather than
 *  silently rendering black. All repo colors are valid hex, so this is a
 *  defensive net, not an expected path. */
const FALLBACK: Rgba8 = [255, 0, 255, 255];

function clampByte(n: number): number {
  if (Number.isNaN(n)) return 0;
  if (n < 0) return 0;
  if (n > 255) return 255;
  return Math.round(n);
}

function hex1(c: string): number {
  const v = parseInt(c + c, 16);
  return Number.isNaN(v) ? 0 : v;
}

/** Parse an `hsl()` channel: `s`/`l` are `0..100%` (or a bare 0..1 fraction),
 *  `h` is degrees. Returns NaN so the caller can fall back. */
function hslChannel(raw: string, isHue: boolean): number {
  const t = raw.trim();
  if (isHue) return parseFloat(t);
  if (t.endsWith('%')) return parseFloat(t) / 100;
  const n = parseFloat(t);
  // A bare number for s/l is treated as an already-normalized 0..1 fraction.
  return n > 1 ? n / 100 : n;
}

/**
 * `hsl(h, s, l)` -> `[r,g,b]` (each 0..255), the standard HSL->RGB conversion.
 * `h` in degrees (wrapped), `s`/`l` in 0..1. Used so the global view's
 * per-system `hsl(...)` colors (generateSystemColor) pack into the custom
 * layer's vertex attribute with the same hue the CSS markers show.
 */
function hslToRgb(h: number, s: number, l: number): [number, number, number] {
  const hh = ((h % 360) + 360) % 360;
  const c = (1 - Math.abs(2 * l - 1)) * s;
  const x = c * (1 - Math.abs(((hh / 60) % 2) - 1));
  const m = l - c / 2;
  let r = 0;
  let g = 0;
  let b = 0;
  if (hh < 60) [r, g, b] = [c, x, 0];
  else if (hh < 120) [r, g, b] = [x, c, 0];
  else if (hh < 180) [r, g, b] = [0, c, x];
  else if (hh < 240) [r, g, b] = [0, x, c];
  else if (hh < 300) [r, g, b] = [x, 0, c];
  else [r, g, b] = [c, 0, x];
  return [clampByte((r + m) * 255), clampByte((g + m) * 255), clampByte((b + m) * 255)];
}

/**
 * Parse a CSS color string to `[r,g,b,a]` (each 0..255). Supports `#rgb`,
 * `#rgba`, `#rrggbb`, `#rrggbbaa`, `rgb(r,g,b)`, `rgba(r,g,b,a)` (alpha as
 * 0..1), and `hsl(h,s%,l%)` / `hsla(h,s%,l%,a)`. Anything else returns the
 * visible magenta fallback.
 */
export function packCssColor(input: string): Rgba8 {
  const s = input.trim().toLowerCase();
  if (s.startsWith('#')) {
    const h = s.slice(1);
    if (h.length === 3) return [hex1(h[0]!), hex1(h[1]!), hex1(h[2]!), 255];
    if (h.length === 4) return [hex1(h[0]!), hex1(h[1]!), hex1(h[2]!), hex1(h[3]!)];
    if (h.length === 6) {
      const n = parseInt(h, 16);
      if (Number.isNaN(n)) return FALLBACK;
      return [(n >> 16) & 0xff, (n >> 8) & 0xff, n & 0xff, 255];
    }
    if (h.length === 8) {
      const n = parseInt(h, 16);
      if (Number.isNaN(n)) return FALLBACK;
      return [(n >>> 24) & 0xff, (n >> 16) & 0xff, (n >> 8) & 0xff, n & 0xff];
    }
    return FALLBACK;
  }
  const m = s.match(/^rgba?\(([^)]+)\)$/);
  if (m) {
    const parts = m[1]!.split(',').map((p) => p.trim());
    if (parts.length >= 3) {
      const r = clampByte(parseFloat(parts[0]!));
      const g = clampByte(parseFloat(parts[1]!));
      const b = clampByte(parseFloat(parts[2]!));
      const a = parts.length >= 4 ? clampByte(parseFloat(parts[3]!) * 255) : 255;
      return [r, g, b, a];
    }
  }
  const hm = s.match(/^hsla?\(([^)]+)\)$/);
  if (hm) {
    const parts = hm[1]!.split(',').map((p) => p.trim());
    if (parts.length >= 3) {
      const h = hslChannel(parts[0]!, true);
      const sat = hslChannel(parts[1]!, false);
      const lig = hslChannel(parts[2]!, false);
      if (!Number.isNaN(h) && !Number.isNaN(sat) && !Number.isNaN(lig)) {
        const [r, g, b] = hslToRgb(h, sat, lig);
        const a = parts.length >= 4 ? clampByte(parseFloat(parts[3]!) * 255) : 255;
        return [r, g, b, a];
      }
    }
  }
  return FALLBACK;
}

/** Write a CSS color into a `Uint8Array` at `offset` (4 bytes). Returns the
 *  packed tuple so callers can cache per unique color. */
export function writeCssColor(out: Uint8Array, offset: number, input: string): Rgba8 {
  const rgba = packCssColor(input);
  out[offset] = rgba[0];
  out[offset + 1] = rgba[1];
  out[offset + 2] = rgba[2];
  out[offset + 3] = rgba[3];
  return rgba;
}
