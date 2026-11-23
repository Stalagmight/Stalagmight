/**
 * CSS colour strings -> sRGB, and the OKLab / OKLCH maths the line pills are
 * derived in (`pillColors.ts`). Pure, no DOM: a line colour is whatever the
 * user or an importer stored (`#rgb`, `#rrggbb`, `rgb()`, `hsl()`, a CSS name,
 * ...), and the pill has to read it the way the browser paints the Sections
 * panel's dot.
 *
 * OKLab is Bjorn Ottosson's (https://bottosson.github.io/posts/oklab/), the
 * space CSS `oklab()` / `oklch()` and `color-mix(in oklab, ...)` use.
 */

/** sRGB channels in 0..1 and alpha in 0..1. */
export interface Rgba {
  r: number;
  g: number;
  b: number;
  a: number;
}

/** OKLab: L in 0..1, a/b roughly -0.4..0.4. */
export interface Oklab {
  L: number;
  a: number;
  b: number;
}

/** OKLCH: L in 0..1, C >= 0, h in degrees. */
export interface Oklch {
  L: number;
  C: number;
  h: number;
}

/** The CSS Color 4 named colours (lowercase), as `#rrggbb`. */
const NAMED: Record<string, string> = {
  aliceblue: '#f0f8ff', antiquewhite: '#faebd7', aqua: '#00ffff', aquamarine: '#7fffd4',
  azure: '#f0ffff', beige: '#f5f5dc', bisque: '#ffe4c4', black: '#000000',
  blanchedalmond: '#ffebcd', blue: '#0000ff', blueviolet: '#8a2be2', brown: '#a52a2a',
  burlywood: '#deb887', cadetblue: '#5f9ea0', chartreuse: '#7fff00', chocolate: '#d2691e',
  coral: '#ff7f50', cornflowerblue: '#6495ed', cornsilk: '#fff8dc', crimson: '#dc143c',
  cyan: '#00ffff', darkblue: '#00008b', darkcyan: '#008b8b', darkgoldenrod: '#b8860b',
  darkgray: '#a9a9a9', darkgreen: '#006400', darkgrey: '#a9a9a9', darkkhaki: '#bdb76b',
  darkmagenta: '#8b008b', darkolivegreen: '#556b2f', darkorange: '#ff8c00', darkorchid: '#9932cc',
  darkred: '#8b0000', darksalmon: '#e9967a', darkseagreen: '#8fbc8f', darkslateblue: '#483d8b',
  darkslategray: '#2f4f4f', darkslategrey: '#2f4f4f', darkturquoise: '#00ced1', darkviolet: '#9400d3',
  deeppink: '#ff1493', deepskyblue: '#00bfff', dimgray: '#696969', dimgrey: '#696969',
  dodgerblue: '#1e90ff', firebrick: '#b22222', floralwhite: '#fffaf0', forestgreen: '#228b22',
  fuchsia: '#ff00ff', gainsboro: '#dcdcdc', ghostwhite: '#f8f8ff', gold: '#ffd700',
  goldenrod: '#daa520', gray: '#808080', green: '#008000', greenyellow: '#adff2f',
  grey: '#808080', honeydew: '#f0fff0', hotpink: '#ff69b4', indianred: '#cd5c5c',
  indigo: '#4b0082', ivory: '#fffff0', khaki: '#f0e68c', lavender: '#e6e6fa',
  lavenderblush: '#fff0f5', lawngreen: '#7cfc00', lemonchiffon: '#fffacd', lightblue: '#add8e6',
  lightcoral: '#f08080', lightcyan: '#e0ffff', lightgoldenrodyellow: '#fafad2', lightgray: '#d3d3d3',
  lightgreen: '#90ee90', lightgrey: '#d3d3d3', lightpink: '#ffb6c1', lightsalmon: '#ffa07a',
  lightseagreen: '#20b2aa', lightskyblue: '#87cefa', lightslategray: '#778899', lightslategrey: '#778899',
  lightsteelblue: '#b0c4de', lightyellow: '#ffffe0', lime: '#00ff00', limegreen: '#32cd32',
  linen: '#faf0e6', magenta: '#ff00ff', maroon: '#800000', mediumaquamarine: '#66cdaa',
  mediumblue: '#0000cd', mediumorchid: '#ba55d3', mediumpurple: '#9370db', mediumseagreen: '#3cb371',
  mediumslateblue: '#7b68ee', mediumspringgreen: '#00fa9a', mediumturquoise: '#48d1cc', mediumvioletred: '#c71585',
  midnightblue: '#191970', mintcream: '#f5fffa', mistyrose: '#ffe4e1', moccasin: '#ffe4b5',
  navajowhite: '#ffdead', navy: '#000080', oldlace: '#fdf5e6', olive: '#808000',
  olivedrab: '#6b8e23', orange: '#ffa500', orangered: '#ff4500', orchid: '#da70d6',
  palegoldenrod: '#eee8aa', palegreen: '#98fb98', paleturquoise: '#afeeee', palevioletred: '#db7093',
  papayawhip: '#ffefd5', peachpuff: '#ffdab9', peru: '#cd853f', pink: '#ffc0cb',
  plum: '#dda0dd', powderblue: '#b0e0e6', purple: '#800080', rebeccapurple: '#663399',
  red: '#ff0000', rosybrown: '#bc8f8f', royalblue: '#4169e1', saddlebrown: '#8b4513',
  salmon: '#fa8072', sandybrown: '#f4a460', seagreen: '#2e8b57', seashell: '#fff5ee',
  sienna: '#a0522d', silver: '#c0c0c0', skyblue: '#87ceeb', slateblue: '#6a5acd',
  slategray: '#708090', slategrey: '#708090', snow: '#fffafa', springgreen: '#00ff7f',
  steelblue: '#4682b4', tan: '#d2b48c', teal: '#008080', thistle: '#d8bfd8',
  tomato: '#ff6347', turquoise: '#40e0d0', violet: '#ee82ee', wheat: '#f5deb3',
  white: '#ffffff', whitesmoke: '#f5f5f5', yellow: '#ffff00', yellowgreen: '#9acd32',
};

/** Every CSS colour name the parser knows (for tests). */
export const CSS_COLOR_NAMES: readonly string[] = Object.keys(NAMED);

const clamp01 = (v: number) => (v < 0 ? 0 : v > 1 ? 1 : v);

function parseHex(hex: string): Rgba | null {
  if (!/^[0-9a-f]+$/.test(hex)) return null;
  const n = hex.length;
  if (n === 3 || n === 4) {
    const ch = (i: number) => parseInt(hex[i]! + hex[i]!, 16) / 255;
    return { r: ch(0), g: ch(1), b: ch(2), a: n === 4 ? ch(3) : 1 };
  }
  if (n === 6 || n === 8) {
    const ch = (i: number) => parseInt(hex.slice(i, i + 2), 16) / 255;
    return { r: ch(0), g: ch(2), b: ch(4), a: n === 8 ? ch(6) : 1 };
  }
  return null;
}

/** A number, or a percentage of `percentOf`; NaN when it is neither. */
function num(token: string, percentOf: number): number {
  const t = token.trim();
  if (t === 'none') return 0;
  if (t.endsWith('%')) return (parseFloat(t) / 100) * percentOf;
  return /^[-+]?(\d+\.?\d*|\.\d+)(e[-+]?\d+)?$/.test(t) ? parseFloat(t) : NaN;
}

/** A hue in degrees (`deg`, `grad`, `rad`, `turn` or bare). */
function hue(token: string): number {
  const t = token.trim();
  const v = parseFloat(t);
  if (Number.isNaN(v)) return t === 'none' ? 0 : NaN;
  if (t.endsWith('turn')) return v * 360;
  if (t.endsWith('grad')) return v * 0.9;
  if (t.endsWith('rad')) return (v * 180) / Math.PI;
  return v;
}

/** `fn(a b c / alpha)` or `fn(a, b, c, alpha)` -> its channel tokens and alpha. */
function args(body: string): { ch: string[]; alpha: number } | null {
  const [main, slashAlpha] = body.split('/');
  const ch = main!.includes(',') ? main!.split(',') : main!.trim().split(/\s+/);
  let alphaToken = slashAlpha;
  if (alphaToken === undefined && ch.length === 4) alphaToken = ch.pop();
  if (ch.length !== 3) return null;
  const alpha = alphaToken === undefined ? 1 : num(alphaToken, 1);
  return Number.isNaN(alpha) ? null : { ch: ch.map((c) => c.trim()), alpha: clamp01(alpha) };
}

function hslToRgb(h: number, s: number, l: number): [number, number, number] {
  const k = (n: number) => (n + h / 30) % 12;
  const a = s * Math.min(l, 1 - l);
  const f = (n: number) => l - a * Math.max(-1, Math.min(k(n) - 3, 9 - k(n), 1));
  return [f(0), f(8), f(4)];
}

/**
 * Parse a CSS colour: hex (3/4/6/8 digits), `rgb[a]()`, `hsl[a]()`, `hwb()`,
 * `oklab()`, `oklch()` (comma or space syntax), a CSS name or `transparent`.
 * Anything else (`lab()`, `color()`, a typo) is null: the caller decides what
 * an unpaintable colour looks like.
 */
export function parseCssColor(input: string | null | undefined): Rgba | null {
  if (typeof input !== 'string') return null;
  const s = input.trim().toLowerCase();
  if (s === '') return null;
  if (s.startsWith('#')) return parseHex(s.slice(1));
  if (s === 'transparent') return { r: 0, g: 0, b: 0, a: 0 };
  const named = NAMED[s];
  if (named) return parseHex(named.slice(1));
  const fn = /^([a-z]+)\((.*)\)$/.exec(s);
  if (!fn) return null;
  const parsed = args(fn[2]!);
  if (!parsed) return null;
  const { ch, alpha } = parsed;
  let rgb: [number, number, number] | null = null;
  switch (fn[1]) {
    case 'rgb':
    case 'rgba':
      rgb = [num(ch[0]!, 255) / 255, num(ch[1]!, 255) / 255, num(ch[2]!, 255) / 255];
      break;
    case 'hsl':
    case 'hsla': {
      const h = ((hue(ch[0]!) % 360) + 360) % 360;
      rgb = hslToRgb(h, clamp01(num(ch[1]!, 100) / 100), clamp01(num(ch[2]!, 100) / 100));
      break;
    }
    case 'hwb': {
      const h = ((hue(ch[0]!) % 360) + 360) % 360;
      const w = clamp01(num(ch[1]!, 100) / 100);
      const bl = clamp01(num(ch[2]!, 100) / 100);
      if (w + bl >= 1) {
        const grey = w / (w + bl);
        rgb = [grey, grey, grey];
        break;
      }
      const [r, g, b] = hslToRgb(h, 1, 0.5);
      const scale = 1 - w - bl;
      rgb = [r * scale + w, g * scale + w, b * scale + w];
      break;
    }
    case 'oklab':
      rgb = rgbOfOklab({ L: num(ch[0]!, 1), a: num(ch[1]!, 0.4), b: num(ch[2]!, 0.4) });
      break;
    case 'oklch':
      rgb = rgbOfOklab(oklabOfOklch({ L: num(ch[0]!, 1), C: num(ch[1]!, 0.4), h: hue(ch[2]!) }));
      break;
    default:
      return null;
  }
  if (rgb.some((v) => Number.isNaN(v))) return null;
  return { r: clamp01(rgb[0]), g: clamp01(rgb[1]), b: clamp01(rgb[2]), a: alpha };
}

const toLinear = (c: number) => (c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4));
const toGamma = (c: number) => (c <= 0.0031308 ? 12.92 * c : 1.055 * Math.pow(c, 1 / 2.4) - 0.055);

/** sRGB (0..1, gamma-encoded) -> OKLab. */
export function oklabOf(r: number, g: number, b: number): Oklab {
  const lr = toLinear(r);
  const lg = toLinear(g);
  const lb = toLinear(b);
  const l = Math.cbrt(0.4122214708 * lr + 0.5363325363 * lg + 0.0514459929 * lb);
  const m = Math.cbrt(0.2119034982 * lr + 0.6806995451 * lg + 0.1073969566 * lb);
  const s = Math.cbrt(0.0883024619 * lr + 0.2817188376 * lg + 0.6299787005 * lb);
  return {
    L: 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
    a: 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
    b: 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
  };
}

/** OKLab -> sRGB (0..1, gamma-encoded), NOT clamped: out of gamut shows as < 0 or > 1. */
export function rgbOfOklab({ L, a, b }: Oklab): [number, number, number] {
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  const lin = [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ];
  return lin.map((c) => (c < 0 ? -toGamma(-c) : toGamma(c))) as [number, number, number];
}

export function oklchOfOklab({ L, a, b }: Oklab): Oklch {
  const C = Math.hypot(a, b);
  const h = (Math.atan2(b, a) * 180) / Math.PI;
  return { L, C, h: h < 0 ? h + 360 : h };
}

export function oklabOfOklch({ L, C, h }: Oklch): Oklab {
  const rad = (h * Math.PI) / 180;
  return { L, a: C * Math.cos(rad), b: C * Math.sin(rad) };
}

const IN_GAMUT_EPS = 1e-4;
const inGamut = (rgb: readonly number[]) => rgb.every((c) => c >= -IN_GAMUT_EPS && c <= 1 + IN_GAMUT_EPS);

/**
 * OKLCH -> `#rrggbb`, keeping L and h and lowering C until the colour fits
 * sRGB (the CSS Color 4 approach, by bisection on chroma).
 */
export function hexOfOklch(lch: Oklch): string {
  const L = clamp01(lch.L);
  let rgb = rgbOfOklab(oklabOfOklch({ L, C: lch.C, h: lch.h }));
  if (!inGamut(rgb)) {
    let lo = 0;
    let hi = lch.C;
    for (let i = 0; i < 24; i++) {
      const mid = (lo + hi) / 2;
      if (inGamut(rgbOfOklab(oklabOfOklch({ L, C: mid, h: lch.h })))) lo = mid;
      else hi = mid;
    }
    rgb = rgbOfOklab(oklabOfOklch({ L, C: lo, h: lch.h }));
  }
  return hexOfRgb(rgb[0], rgb[1], rgb[2]);
}

/** sRGB (0..1) -> `#rrggbb`, clamped and rounded. */
export function hexOfRgb(r: number, g: number, b: number): string {
  const byte = (c: number) => Math.round(clamp01(c) * 255).toString(16).padStart(2, '0');
  return `#${byte(r)}${byte(g)}${byte(b)}`;
}

/** `#rrggbb` -> OKLCH. */
export function oklchOfHex(hex: string): Oklch {
  const c = parseHex(hex.replace(/^#/, '').toLowerCase())!;
  return oklchOfOklab(oklabOf(c.r, c.g, c.b));
}

/** WCAG 2 relative luminance of `#rrggbb`. */
export function luminanceOfHex(hex: string): number {
  const c = parseHex(hex.replace(/^#/, '').toLowerCase())!;
  return 0.2126 * toLinear(c.r) + 0.7152 * toLinear(c.g) + 0.0722 * toLinear(c.b);
}

/** WCAG 2 contrast ratio of two `#rrggbb` colours (1..21). */
export function contrastOfHex(a: string, b: string): number {
  const la = luminanceOfHex(a);
  const lb = luminanceOfHex(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

/** `color` painted over the opaque `under` (both `#rrggbb` after parsing), as `#rrggbb`. */
export function flattenOver(color: Rgba, under: string): string {
  const u = parseHex(under.replace(/^#/, '').toLowerCase())!;
  const mix = (c: number, base: number) => c * color.a + base * (1 - color.a);
  return hexOfRgb(mix(color.r, u.r), mix(color.g, u.g), mix(color.b, u.b));
}
