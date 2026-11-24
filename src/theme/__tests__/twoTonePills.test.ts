/**
 * RS-OWNERSHIP: a shot pill with an end of another line is two-tone, the
 * half holding that end in that line's colours and the rest in the shot's.
 * Each half must meet the floors on its own background: main text 7:1 and
 * the muted context 4.5:1, on the shot's half the leading dot (the shot's
 * line, RS-LEADDOT) or its ring 3:1, and on a foreign end's half that end's
 * superscript home dot (its owner's colour, RS-LEADDOT) or its ring 2:1
 * (`HOME_DOT_MIN`: it repeats what the words say), at
 * rest and on hover for a link pill, on the glow and on the bare panel for a
 * prose pill, along every blend, and for a from end over the start of the
 * arrow it overhangs. Swept over ordered pairs of hard line colours
 * (near-white next to near-black, yellow next to blue, greys, pure
 * primaries, a hue sweep) and three-colour shots on every theme, measured
 * with `utils/colors`.
 * Injections: a prose segment whose dot keeps the ring of its own glow
 * alone (not of the blend toward its neighbour) fails "the leading dot reads
 * on every blend" on the dark theme; a link segment's dot with no ring fails
 * it on every theme (a link pill's fills sit in one narrow band, so its
 * blends move the dot's backdrop little); a from end's home dot ringed
 * against its half alone (the bridge it overhangs dropped from
 * `homeDotStyle`) fails "both ends foreign, three colours" for prose on
 * cave (on every theme but nord at the old 3:1 floor).
 */
import { describe, expect, it } from 'vitest';
import { themeTokens } from '../tokens';
import { hexOfOklch, oklchOfHex } from '../cssColor';
import { contrastRatio } from '../../utils/colors';
import {
  blendBackdrops,
  bridgeBackdrops,
  homeDotStyle,
  linkBridgeStyle,
  linkSegmentStyle,
  mixHex,
  proseBridgeStyle,
  proseSegmentStyle,
  type Neighbours,
} from '../pillBlend';
import {
  HOME_DOT_MIN,
  PILL_DOT_MIN,
  PILL_MUTED_MIN,
  PILL_TEXT_MIN,
  pillColors,
  pillColorsOf,
  pillDot,
  pillThemeOf,
  proseBackdrops,
  prosePillColors,
  prosePillColorsOf,
  type PillColors,
} from '../pillColors';

const THEMES = Object.keys(themeTokens);

const HARD = [
  '#f6f6f2', // near-white
  '#101010', // near-black
  '#ffe600', // yellow
  '#1f4fff', // blue
  '#8a8a8a', // grey
  '#000000',
  '#ffffff',
  '#ff0000',
  '#00ff00',
  'navy',
  '#c2185b',
  ...[0, 60, 120, 180, 240, 300].flatMap((h) => [hexOfOklch({ L: 0.5, C: 0.15, h }), hexOfOklch({ L: 0.88, C: 0.12, h })]),
];

const PAIRS: Array<[string, string]> = HARD.flatMap((a) => HARD.filter((b) => b !== a).map((b) => [a, b] as [string, string]));

/** The floors of one half: its text and muted part, and its leading dot when it holds it, on each of its backgrounds. */
function checkHalf(label: string, half: PillColors, backgrounds: readonly string[], failures: string[], withDot = false) {
  const check = (what: string, value: number, min: number) => {
    if (!(value >= min)) failures.push(`${label} ${what} ${value.toFixed(2)} < ${min}`);
  };
  for (const bg of backgrounds) {
    check(`text on ${bg}`, contrastRatio(half.text, bg), PILL_TEXT_MIN);
    check(`muted on ${bg}`, contrastRatio(half.muted, bg), PILL_MUTED_MIN);
    if (withDot) check(`dot ${half.dot} on ${bg}`, contrastRatio(half.ring ?? half.dot, bg), PILL_DOT_MIN);
  }
}

describe('two-tone shot pills (RS-OWNERSHIP)', () => {
  it.each(THEMES)('%s: each half of a link pill meets the floors at rest and on hover', (name) => {
    const theme = pillThemeOf(name);
    const failures: string[] = [];
    for (const [shotLine, owner] of PAIRS) {
      const shotHalf = pillColors(shotLine, theme);
      const ownerHalf = pillColors(owner, theme);
      checkHalf(`${shotLine}|${owner} shot half`, shotHalf, [shotHalf.bg, shotHalf.hoverBg], failures, true);
      checkHalf(`${shotLine}|${owner} owner half`, ownerHalf, [ownerHalf.bg, ownerHalf.hoverBg], failures);
    }
    expect(failures).toEqual([]);
  });

  it.each(THEMES)('%s: each half of a prose pill meets the floors on its tint and on the panel', (name) => {
    const theme = pillThemeOf(name);
    const failures: string[] = [];
    for (const [shotLine, owner] of PAIRS) {
      const shotHalf = prosePillColors(shotLine, theme);
      const ownerHalf = prosePillColors(owner, theme);
      checkHalf(`${shotLine}|${owner} shot half`, shotHalf, proseBackdrops(shotHalf.bg, theme), failures, true);
      checkHalf(`${shotLine}|${owner} owner half`, ownerHalf, [ownerHalf.bg, theme.panel], failures);
    }
    expect(failures).toEqual([]);
  });

  it.each(THEMES)('%s: near-white next to near-black gives two different halves, both legible', (name) => {
    const theme = pillThemeOf(name);
    const white = pillColors('#f6f6f2', theme);
    const black = pillColors('#101010', theme);
    // The halves are told apart by their fills, their borders and, at the
    // start, the dot (RS-LEADDOT).
    expect(white.bg).not.toBe(black.bg);
    expect(white.dot).not.toBe(black.dot);
    expect(contrastRatio(white.border, black.border)).toBeGreaterThanOrEqual(2);
    for (const half of [white, black]) {
      expect(contrastRatio(half.text, half.bg)).toBeGreaterThanOrEqual(PILL_TEXT_MIN);
      expect(contrastRatio(half.ring ?? half.dot, half.bg)).toBeGreaterThanOrEqual(PILL_DOT_MIN);
    }
  });
});

// ---- RS-BLEND: the blended seams ------------------------------------------------


/** sRGB halfway between two colours: what a browser draws between two gradient stops. */
function srgbMid(a: string, b: string): string {
  const ch = (h: string, i: number) => parseInt(h.slice(1 + 2 * i, 3 + 2 * i), 16);
  return `#${[0, 1, 2].map((i) => Math.round((ch(a, i) + ch(b, i)) / 2).toString(16).padStart(2, '0')).join('')}`;
}

/**
 * Every colour a blended segment shows: its backdrops (`blendBackdrops`) and,
 * between its gradient stops (own, 75%, 50% toward a neighbour), the sRGB
 * points the browser interpolates.
 */
function blendedColours(own: PillColors, near: Neighbours, panel?: string): string[] {
  const out = blendBackdrops(own, near, panel);
  for (const other of [near.left, near.right]) {
    if (!other) continue;
    // A link pill's stops are its own colour, 75% and 50% toward the
    // neighbour; a prose glow's are eighths of the blend (`samplePoints`).
    const shares = panel === undefined ? [1, 0.75, 0.5] : [1, 0.875, 0.75, 0.625, 0.5];
    const stops = shares.map((t) => mixHex(own.bg, other.bg, t));
    for (let i = 1; i < stops.length; i++) out.push(srgbMid(stops[i - 1]!, stops[i]!));
  }
  return out;
}

/** A prose glow's colours over the panel as they also fall on every tinted surface of `theme` (`proseBackdrops`). */
const onSurfaces = (colours: readonly string[], theme: string) => colours.flatMap((c) => proseBackdrops(c, pillThemeOf(theme)));

/**
 * The text and muted colours a segment is painted with (its style's), on
 * every colour of it; with `withDot`, its leading dot too (the ring when it
 * has one, else the dot): the dot sits in the shot line's part at the start,
 * a head before a foreign from half or the from half itself (RS-LEADDOT).
 */
function checkBlended(label: string, style: Record<string, string>, on: readonly string[], failures: string[], withDot = false) {
  const ring = style['--pill-ring'];
  const dot = ring && ring !== 'transparent' ? ring : style['--pill-dot'];
  for (const bg of on) {
    if (contrastRatio(style['--pill-text']!, bg) < PILL_TEXT_MIN) failures.push(`${label} text on ${bg}`);
    if (contrastRatio(style['--pill-muted']!, bg) < PILL_MUTED_MIN) failures.push(`${label} muted on ${bg}`);
    if (withDot && !(contrastRatio(dot!, bg) >= PILL_DOT_MIN)) failures.push(`${label} dot ${dot} on ${bg}`);
  }
}

/** A segment's style as the variant paints it. */
const segmentStyle = (variant: 'link' | 'prose', own: PillColors, near: Neighbours, theme: string) =>
  (variant === 'link' ? linkSegmentStyle(own, near, theme) : proseSegmentStyle(own, near, theme)) as Record<string, string>;

/** Shots of three regions (head, from half, to half): the pairs, each with a third hard colour. */
const TRIPLES: Array<[string, string, string]> = PAIRS.flatMap(([a, b]) =>
  ['#f6f6f2', '#101010', '#ffe600', '#1f4fff'].filter((c) => c !== a && c !== b).map((c) => [a, b, c] as [string, string, string]),
);

type PillTint = 'link' | 'prose';
/** A variant's colours on a named theme (cached: the sweeps ask for each colour many times). */
const colorsOf = (variant: PillTint, name: string) => (c: string) => (variant === 'link' ? pillColorsOf(c, name) : prosePillColorsOf(c, name));

describe.each(['link', 'prose'] as const)('blended two-tone %s pills (RS-BLEND)', (variant) => {
  it.each(THEMES)('%s: text, muted parts and the leading dot read on every blend of every pair', (name) => {
    const theme = pillThemeOf(name);
    const panel = variant === 'prose' ? theme.panel : undefined;
    const failures: string[] = [];
    const of = colorsOf(variant, name);
    for (const [shotLine, owner] of PAIRS) {
      const A = of(shotLine);
      const B = of(owner);
      const onA = blendedColours(A, { left: null, right: B }, panel);
      const onB = blendedColours(B, { left: A, right: null }, panel);
      const styleA = segmentStyle(variant, A, { left: null, right: B }, name);
      const styleB = segmentStyle(variant, B, { left: A, right: null }, name);
      checkBlended(`${shotLine}|${owner} shot half`, styleA, panel ? onSurfaces(onA, name) : onA, failures, true);
      checkBlended(`${shotLine}|${owner} owner half`, styleB, panel ? onSurfaces(onB, name) : onB, failures);
    }
    expect(failures.slice(0, 20)).toEqual([]);
  });

  it.each(THEMES)('%s: a middle region blends at both seams and still reads', (name) => {
    const theme = pillThemeOf(name);
    const panel = variant === 'prose' ? theme.panel : undefined;
    const failures: string[] = [];
    const of = colorsOf(variant, name);
    for (const [head, from, to] of TRIPLES) {
      const [H, F, T] = [of(head), of(from), of(to)];
      const near = { left: H, right: T };
      const on = blendedColours(F, near, panel);
      checkBlended(`${head}|${from}|${to} middle`, segmentStyle(variant, F, near, name), panel ? onSurfaces(on, name) : on, failures);
    }
    expect(failures.slice(0, 20)).toEqual([]);
  });
});

describe('the blend mixes in OKLab (RS-BLEND)', () => {
  it('meets in a clean midpoint: yellow and blue keep chroma, near-white and near-black a mid grey', () => {
    const lab = (hex: string) => oklchOfHex(hex);
    const yb = lab(mixHex('#ffe600', '#1f4fff', 0.5));
    const srgb = lab(srgbMid('#ffe600', '#1f4fff'));
    // The sRGB midpoint of yellow and blue is a grey; OKLab keeps more colour.
    expect(yb.C).toBeGreaterThan(srgb.C);
    const wb = lab(mixHex('#f6f6f2', '#101010', 0.5));
    expect(wb.L).toBeGreaterThan(0.45);
    expect(wb.L).toBeLessThan(0.65);
    expect(mixHex('#123456', '#abcdef', 1)).toBe('#123456');
    expect(mixHex('#123456', '#abcdef', 0)).toBe('#abcdef');
  });
});

// ---- RS-ARROW: the arrow is the blend ------------------------------------------------

/** Every colour under a bridge: its samples and the sRGB points the browser draws between its quarter stops. */
function bridgeColours(left: PillColors, right: PillColors, panel?: string): string[] {
  const out = bridgeBackdrops(left, right, panel);
  // A link pill's bridge has stops at its quarters, a prose glow's at its eighths (`samplePoints`).
  const shares = panel === undefined ? [1, 0.75, 0.5, 0.25, 0] : [1, 0.875, 0.75, 0.625, 0.5, 0.375, 0.25, 0.125, 0];
  for (const pick of panel === undefined ? [(c: PillColors) => c.bg, (c: PillColors) => c.hoverBg] : [(c: PillColors) => c.bg]) {
    const q = shares.map((t) => mixHex(pick(left), pick(right), t));
    for (let i = 0; i < q.length - 1; i++) out.push(srgbMid(q[i]!, q[i + 1]!));
  }
  return out;
}

describe.each(['link', 'prose'] as const)('the arrow of a two-tone %s pill reads on its whole blend (RS-ARROW)', (variant) => {
  it.each(THEMES)('%s: the arrow (text 7:1) on every colour from one half to the other, both ways', (name) => {
    const theme = pillThemeOf(name);
    const panel = variant === 'prose' ? theme.panel : undefined;
    const of = colorsOf(variant, name);
    const style = variant === 'link' ? linkBridgeStyle : proseBridgeStyle;
    const failures: string[] = [];
    for (const [a, b] of PAIRS) {
      const L = of(a);
      const R = of(b);
      const css = style(L, R, name) as Record<string, string>;
      const text = css['--pill-text']!;
      for (const bg of variant === 'prose' ? onSurfaces(bridgeColours(L, R, panel), name) : bridgeColours(L, R, panel)) {
        if (contrastRatio(text, bg) < PILL_TEXT_MIN) failures.push(`${a}->${b} arrow ${text} on ${bg}`);
      }
    }
    expect(failures.slice(0, 20)).toEqual([]);
  });
});

// ---- RS-LEADDOT: a foreign end's superscript home dot -----------------------------------

/**
 * Every colour a foreign end's home dot can sit on, listed here apart from
 * `homeDotStyle`: its half's colours and blends ({@link blendedColours})
 * and, for the from end, the arrow's bridge from its own colour to 37.5% of
 * the way to the to half's (the dot takes no room, so it overhangs about the
 * first third of the arrow), with the sRGB points a gradient draws between.
 */
function homeDotColours(own: PillColors, near: Neighbours, bridgeTo: PillColors | null, panel?: string): string[] {
  const out = blendedColours(own, near, panel);
  if (bridgeTo) {
    for (const pick of panel === undefined ? [(c: PillColors) => c.bg, (c: PillColors) => c.hoverBg] : [(c: PillColors) => c.bg]) {
      const q = [1, 0.875, 0.75, 0.625].map((t) => mixHex(pick(own), pick(bridgeTo), t));
      out.push(...q);
      for (let i = 1; i < q.length; i++) out.push(srgbMid(q[i - 1]!, q[i]!));
    }
  }
  return out;
}

/**
 * The home dot of `own`'s half (its colour or its ring) at the home dot's
 * floor (2:1, `HOME_DOT_MIN`) on every colour of `on`, ringed only where it
 * needs it, and its ring no stronger on the half than the ring a 3:1 dot
 * would need there (the lower floor makes it quieter, never louder).
 */
function checkHomeDot(label: string, style: Record<string, string>, own: PillColors, on: readonly string[], failures: string[], theme: string) {
  const ring = style['--home-ring']!;
  const dot = style['--home-dot']!;
  if (dot !== own.dot) failures.push(`${label} home dot ${dot} is not the line colour ${own.dot}`);
  const shown = ring !== 'transparent' ? ring : dot;
  for (const bg of on) {
    const c = contrastRatio(shown, bg);
    if (!(c >= HOME_DOT_MIN)) failures.push(`${label} home dot ${shown} on ${bg} ${c.toFixed(2)}`);
  }
  const alone = Math.min(...on.map((bg) => contrastRatio(dot, bg)));
  if (ring !== 'transparent' && alone >= HOME_DOT_MIN + 0.3) failures.push(`${label} home dot ringed for nothing (${alone.toFixed(2)})`);
  if (ring !== 'transparent') {
    const full = pillDot(dot, on, pillThemeOf(theme));
    const strong = (c: string) => contrastRatio(c, on[0]!);
    if (strong(ring) > strong(full.ring ?? dot) + 0.05) failures.push(`${label} home ring ${ring} louder than a 3:1 one ${full.ring}`);
  }
}

describe.each(['link', 'prose'] as const)('the home dot of a foreign end on a two-tone %s pill (RS-LEADDOT)', (variant) => {
  it.each(THEMES)('%s: on its own half, every blend, and over the arrow it overhangs, every pair both ways', (name) => {
    const theme = pillThemeOf(name);
    const panel = variant === 'prose' ? theme.panel : undefined;
    const on = (c: string[]) => (panel ? onSurfaces(c, name) : c);
    const of = colorsOf(variant, name);
    const failures: string[] = [];
    for (const [shotLine, owner] of PAIRS) {
      const S = of(shotLine);
      const O = of(owner);
      // A foreign from end: head (the shot's) | from (the owner's) | arrow to the shot's to half.
      const fromNear = { left: S, right: O };
      const from = homeDotStyle(variant, O, fromNear, S, name) as Record<string, string>;
      checkHomeDot(`${shotLine}|${owner} from end`, from, O, on(homeDotColours(O, fromNear, S, panel)), failures, name);
      // A foreign to end: the arrow's right edge is its own colour; the pill ends after it.
      const toNear = { left: O, right: null };
      const to = homeDotStyle(variant, O, toNear, null, name) as Record<string, string>;
      checkHomeDot(`${shotLine}|${owner} to end`, to, O, on(homeDotColours(O, toNear, null, panel)), failures, name);
    }
    expect(failures.slice(0, 20)).toEqual([]);
  });

  it.each(THEMES)('%s: both ends foreign, three colours: each home dot on its own half', (name) => {
    const theme = pillThemeOf(name);
    const panel = variant === 'prose' ? theme.panel : undefined;
    const on = (c: string[]) => (panel ? onSurfaces(c, name) : c);
    const of = colorsOf(variant, name);
    const failures: string[] = [];
    for (const [head, fromLine, toLine] of TRIPLES) {
      const [H, F, T] = [of(head), of(fromLine), of(toLine)];
      const fromNear = { left: H, right: F };
      checkHomeDot(`${head}|${fromLine}|${toLine} from`, homeDotStyle(variant, F, fromNear, T, name) as Record<string, string>, F, on(homeDotColours(F, fromNear, T, panel)), failures, name);
      const toNear = { left: T, right: null };
      checkHomeDot(`${head}|${fromLine}|${toLine} to`, homeDotStyle(variant, T, toNear, null, name) as Record<string, string>, T, on(homeDotColours(T, toNear, null, panel)), failures, name);
    }
    expect(failures.slice(0, 20)).toEqual([]);
  });
});

describe('a two-tone pill carries its home dots on the foreign halves (RS-LEADDOT)', () => {
  it('a near-black end on a near-white shot (dark theme): ringed toward the text pole, over the arrow too', () => {
    const W = prosePillColorsOf('#f6f6f2', 'dark');
    const B = prosePillColorsOf('#101010', 'dark');
    const style = homeDotStyle('prose', B, { left: W, right: B }, W, 'dark') as Record<string, string>;
    expect(style['--home-dot']).toBe('#101010');
    expect(style['--home-ring']).not.toBe('transparent');
    expect(oklchOfHex(style['--home-ring']!).L).toBeGreaterThan(oklchOfHex(B.bg).L);
    // A yellow end on the dark theme stands out alone: no ring.
    const Y = pillColorsOf('#ffe600', 'dark');
    expect((homeDotStyle('link', Y, { left: Y, right: null }, null, 'dark') as Record<string, string>)['--home-ring']).toBe('transparent');
  });
});
