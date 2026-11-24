/**
 * Line pill colours: everything a link pill (`DetailLink`, `EntityBlob`)
 * paints, derived from the colour of the line it belongs to and the theme.
 *
 * A line colour is user data and can be anything (near-white, near-black, pure
 * yellow, a CSS name). A pill shows its line by its leading dot, its fill and
 * its border (RS-NODOTS took the dot away and gave the fill and the border
 * the colour; RS-LEADDOT brought the leading dot back), so:
 *
 * - background: the panel lifted a little toward the line's hue (dark themes
 *   lift it, light themes sink it), up to 80% of the line's chroma within a
 *   cap, so a yellow pill and a navy pill have nearly the same lightness and
 *   read as one family, each in its own hue; a pale line sits at the band's
 *   lighter edge, a dark one at its darker edge;
 * - border: the line colour itself (what the dot used to show), moved toward
 *   the theme's text pole only as far as it takes to stand out from the
 *   panel and the fill ({@link BORDER_MIN}): the border carries the colour
 *   where the fill cannot (a grey line has no hue, so a near-white, a grey
 *   and a near-black line differ by their borders' lightness);
 * - hover: a stronger tint and border, inside the same legibility limits;
 * - dot: the line colour itself (as the Sections panel draws it), with a ring
 *   in the line's hue on the text side only when the colour alone does not
 *   stand out from the pill's fill (a near-black line on a dark theme, a
 *   near-white or a yellow one on a light theme);
 * - text: the theme's own text tokens, nudged toward black or white only on a
 *   theme whose token cannot reach the floor on the band.
 *
 * Floors (pinned by `__tests__/pillColors.test.ts` over a sweep of colours on
 * every theme): main text 7:1 (WCAG AAA), muted context and Station ID 4.5:1
 * (AA), the dot or its ring 3:1 (non-text), on the resting and the hover
 * background; the border at least {@link BORDER_MIN} against the panel and
 * the fill. Every value is `#rrggbb`.
 */
import { themeTokens } from './tokens';
import {
  contrastOfHex,
  flattenOver,
  hexOfOklch,
  hexOfRgb,
  oklabOf,
  oklchOfHex,
  oklchOfOklab,
  parseCssColor,
  type Oklch,
} from './cssColor';

export const PILL_TEXT_MIN = 7;
export const PILL_MUTED_MIN = 4.5;
export const PILL_DOT_MIN = 3;
/** A pill's border against the panel and its fill: enough to show the line's colour as an edge (RS-NODOTS). */
export const BORDER_MIN = 1.8;

/** Solve to these, a little above the floors, so rounding to 8 bits never dips under. */
const TEXT_TARGET = PILL_TEXT_MIN + 0.1;
const MUTED_TARGET = PILL_MUTED_MIN + 0.1;
const DOT_TARGET = PILL_DOT_MIN + 0.25;
const BORDER_TARGET = BORDER_MIN + 0.05;

/** The theme as the pills need it, see {@link pillTheme}. */
export interface PillTheme {
  /** `--cc-panel`: what the pill sits on. */
  panel: string;
  /** Main text: `--cc-text-primary`, unless that cannot reach 7:1 on the band. */
  text: string;
  /** Context and Station ID: `--cc-text-secondary`, unless that cannot reach 4.5:1. */
  muted: string;
  /** The missing-target glyph: `--cc-warning-text`, unless that cannot reach 3:1. */
  warning: string;
  /** A dark panel: borders and dot rings go toward white; on a light one toward black. */
  dark: boolean;
  /**
   * Which way the pill backgrounds leave the panel's lightness: +1 lighter,
   * -1 darker. Dark themes lift and light themes sink, unless the theme's own
   * text tokens only stay legible the other way (nord, one_dark lift-fail, so
   * their pills sink below the panel instead of the text being changed).
   */
  lift: 1 | -1;
  /**
   * The tinted boxes prose sits in besides the panel (the accent, warning and
   * error washes at 10%), flattened on the panel: a prose glow is a
   * wash, so it is held legible over each of them too (RS-NODOTS).
   */
  surfaces: string[];
}

/** A dot and, when it needs one, the ring that keeps it visible on the pill. */
export interface PillDot {
  dot: string;
  ring: string | null;
}

export interface PillColors extends PillDot {
  bg: string;
  border: string;
  hoverBg: string;
  hoverBorder: string;
  text: string;
  muted: string;
}

/** A pill whose target is gone: neutral (no line colour), dashed, a warning glyph in place of the dot. */
export interface MissingPillColors {
  bg: string;
  border: string;
  text: string;
  muted: string;
  warning: string;
}

/**
 * The band, as offsets of OKLCH lightness from the panel's (toward white on a
 * dark theme, toward black on a light one) and chroma caps. The line's own
 * lightness picks a point inside `BG_LIFT`, so a pale line's pill is a touch
 * lighter than a dark line's, never more.
 */
const BG_LIFT = { min: 0.03, max: 0.06 } as const;
/** The fill's share of the line's chroma, and its cap (RS-NODOTS: up from 0.5 and 0.05 / 0.04 with the dot gone). */
const BG_CHROMA = { share: 0.8, max: { dark: 0.08, light: 0.07 } } as const;
const HOVER_LIFT = 0.02;
const HOVER_CHROMA = { share: 0.9, max: { dark: 0.1, light: 0.09 } } as const;
/** How far the hover border goes past the resting one toward the text pole. */
const BORDER_HOVER_STEP = 0.1;
/** The missing-target pill's grey border, off the panel. */
const MISSING_BORDER_LIFT = 0.26;
const RING_CHROMA_MAX = 0.12;
/** Below this chroma a colour is a grey: its hue is noise, so the tints stay neutral. */
const GREY_CHROMA = 0.02;

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

/** Move `hex` toward black or white in OKLab until `ok(hex)` holds (or it gets there). */
function pushUntil(hex: string, towardWhite: boolean, ok: (candidate: string) => boolean): string {
  if (ok(hex)) return hex;
  const start = oklchOfHex(hex);
  for (let step = 1; step <= 50; step++) {
    const t = step / 50;
    const candidate = hexOfOklch({
      L: start.L + ((towardWhite ? 1 : 0) - start.L) * t,
      C: start.C * (1 - t),
      h: start.h,
    });
    if (ok(candidate)) return candidate;
  }
  return towardWhite ? '#ffffff' : '#000000';
}

/** Neutrals at both edges of the band going `lift` from the panel (the far one with its hover tint). */
function bandEdges(panelL: number, lift: 1 | -1): string[] {
  return [BG_LIFT.min, BG_LIFT.max + HOVER_LIFT].map((d) =>
    hexOfOklch({ L: Math.min(1, Math.max(0, panelL + lift * d)), C: 0, h: 0 }),
  );
}

/**
 * The theme's pill tokens from its `--cc-*` values. Text comes from the
 * theme's tokens; only a theme whose token cannot reach the floor on the
 * band's worst case gets it nudged toward black or white (same hue).
 */
export function pillTheme(tokens: {
  panel: string;
  textPrimary: string;
  textSecondary: string;
  warningText: string;
  /** Tinted box colours (any CSS colour, translucent ones flattened on the panel). */
  surfaces?: readonly string[];
}): PillTheme {
  const panel = tokens.panel.toLowerCase();
  const panelL = oklchOfHex(panel).L;
  const dark = panelL < 0.6;
  const text = tokens.textPrimary.toLowerCase();
  const muted = tokens.textSecondary.toLowerCase();
  const reaches = (edges: string[], min: number) => (c: string) => edges.every((e) => contrastOfHex(c, e) >= min);
  const tokensFit = (lift: 1 | -1) => {
    const edges = bandEdges(panelL, lift);
    return reaches(edges, TEXT_TARGET)(text) && reaches(edges, MUTED_TARGET)(muted);
  };
  const usual: 1 | -1 = dark ? 1 : -1;
  const lift: 1 | -1 = tokensFit(usual) || !tokensFit(-usual as 1 | -1) ? usual : (-usual as 1 | -1);
  const edges = bandEdges(panelL, lift);
  return {
    panel,
    text: pushUntil(text, dark, reaches(edges, TEXT_TARGET)),
    muted: pushUntil(muted, dark, reaches(edges, MUTED_TARGET)),
    warning: pushUntil(tokens.warningText.toLowerCase(), dark, reaches(edges, DOT_TARGET)),
    dark,
    lift,
    surfaces: (tokens.surfaces ?? []).map((c) => lineHex(c, panel)).filter((c): c is string => c !== null),
  };
}

/** The line colour as painted on the panel: parsed, translucency flattened; null when unpaintable. */
function lineHex(lineColor: string | null | undefined, panel: string): string | null {
  const parsed = parseCssColor(lineColor);
  if (!parsed) return null;
  return flattenOver(parsed, panel);
}

/** The text pair a background must carry (main 7:1, muted 4.5:1). */
function legible(bg: string, theme: PillTheme): boolean {
  return contrastOfHex(theme.text, bg) >= TEXT_TARGET && contrastOfHex(theme.muted, bg) >= MUTED_TARGET;
}

/**
 * A tint at `L`/`C` of hue `h`, stepped back toward the safe side (darker on a
 * dark theme, lighter on a light one) until the theme's text reads on it.
 */
function legibleTint(L: number, C: number, h: number, theme: PillTheme): string {
  for (let step = 0; step <= 60; step++) {
    const l = theme.dark ? L - step * 0.005 : L + step * 0.005;
    const bg = hexOfOklch({ L: clamp(l, 0, 1), C, h });
    if (legible(bg, theme)) return bg;
  }
  return theme.panel;
}

/**
 * The dot for `color` on the backgrounds it is drawn on, with a ring in its
 * hue on the text side (lighter on a dark theme, darker on a light one) when
 * the colour itself does not reach 3:1 on every one of them.
 */
export function pillDot(color: string, backgrounds: readonly string[], theme: PillTheme): PillDot {
  const dot = color.toLowerCase();
  const clears = (c: string) => backgrounds.every((bg) => contrastOfHex(c, bg) >= DOT_TARGET);
  if (clears(dot)) return { dot, ring: null };
  const lch = oklchOfHex(dot);
  const C = lch.C < GREY_CHROMA ? 0 : Math.min(lch.C, RING_CHROMA_MAX);
  // Walk from the background toward the text pole: the first ring that clears
  // the floor is the gentlest one that does.
  const bgL = oklchOfHex(backgrounds[0] ?? theme.panel).L;
  for (let step = 1; step <= 100; step++) {
    const L = theme.dark ? bgL + (1 - bgL) * (step / 100) : bgL * (1 - step / 100);
    const ring = hexOfOklch({ L, C, h: lch.h });
    if (clears(ring)) return { dot, ring };
  }
  return { dot, ring: theme.text };
}

/** The tint parameters of a line colour: its hue, and its lightness/chroma as the band reads them. */
function lineTone(hex: string | null): Oklch {
  if (hex === null) return { L: 0.6, C: 0, h: 0 };
  const lch = oklchOfOklab(oklabOf(...rgbOfHex(hex)));
  return lch.C < GREY_CHROMA ? { ...lch, C: 0 } : lch;
}

function rgbOfHex(hex: string): [number, number, number] {
  const n = parseInt(hex.slice(1), 16);
  return [((n >> 16) & 0xff) / 255, ((n >> 8) & 0xff) / 255, (n & 0xff) / 255];
}

/**
 * Every colour of a line's pill on `theme`. `lineColor` is any CSS colour; an
 * unpaintable one (null, a typo) gives a neutral pill with a muted dot.
 */
export function pillColors(lineColor: string | null | undefined, theme: PillTheme): PillColors {
  const line = lineHex(lineColor, theme.panel);
  const tone = lineTone(line);
  const panelL = oklchOfHex(theme.panel).L;
  const sign = theme.lift;
  const edge = theme.dark ? 1 : -1;
  // A pale line sits at the band's lighter edge, a dark one at its darker edge.
  const t = clamp(tone.L, 0, 1);
  const lift = BG_LIFT.min + (BG_LIFT.max - BG_LIFT.min) * (sign > 0 ? t : 1 - t);
  const restL = panelL + sign * lift;
  const mode = theme.dark ? 'dark' : 'light';
  const bg = legibleTint(restL, Math.min(tone.C * BG_CHROMA.share, BG_CHROMA.max[mode]), tone.h, theme);
  const bgL = oklchOfHex(bg).L;
  const hoverBg = legibleTint(
    bgL + sign * HOVER_LIFT,
    Math.min(tone.C * HOVER_CHROMA.share, HOVER_CHROMA.max[mode]),
    tone.h,
    theme,
  );
  // The border is the line colour, moved toward the text pole only as far
  // as it must go to stand out from the panel and the fill (RS-NODOTS).
  const towardText = theme.dark;
  const standsOn = (fill: string) => (c: string) =>
    contrastOfHex(c, theme.panel) >= BORDER_TARGET && contrastOfHex(c, fill) >= BORDER_TARGET;
  const border = pushUntil(line ?? hexOfOklch({ L: tone.L, C: 0, h: 0 }), towardText, standsOn(bg));
  // On hover the border goes further toward the text pole, at most half the
  // way there, so a border already near it (a yellow line on a dark theme)
  // keeps its hue instead of turning white or black.
  const borderL = oklchOfHex(border);
  const room = edge > 0 ? 1 - borderL.L : borderL.L;
  const hoverBorder = pushUntil(
    hexOfOklch({ L: clamp(borderL.L + edge * Math.min(BORDER_HOVER_STEP, room / 2), 0, 1), C: borderL.C, h: borderL.h }),
    towardText,
    standsOn(hoverBg),
  );
  // The leading dot: the line colour, ringed only where it alone does not
  // stand out from the fill, at rest or on hover (RS-LEADDOT).
  const { dot, ring } = pillDot(line ?? theme.muted, [bg, hoverBg], theme);
  return { bg, border, hoverBg, hoverBorder, text: theme.text, muted: theme.muted, dot, ring };
}

/** The pill of a missing target on `theme`: the resting band in grey, its border dashed by the CSS. */
export function missingPillColors(theme: PillTheme): MissingPillColors {
  const panelL = oklchOfHex(theme.panel).L;
  const bg = legibleTint(panelL + theme.lift * BG_LIFT.min, 0, 0, theme);
  const edge = theme.dark ? 1 : -1;
  const bgL = oklchOfHex(bg).L;
  const base = edge > 0 ? Math.max(bgL, panelL) : Math.min(bgL, panelL);
  const border = hexOfOklch({ L: clamp(base + edge * MISSING_BORDER_LIFT, 0, 1), C: 0, h: 0 });
  const warning = pushUntil(theme.warning, theme.dark, (c) => contrastOfHex(c, bg) >= DOT_TARGET);
  return { bg, border, text: theme.text, muted: theme.muted, warning };
}

/**
 * A prose glow's opacity over what it sits on (RS-NODOTS): every prose glow is
 * a wash of one colour at this alpha, so it moves an accent or warning box
 * the way it moves the panel, the glows of two stacked lines stay see-through
 * where they meet, and a two-colour glow's stops interpolate exactly.
 */
export const PROSE_ALPHA = 0.5;
/**
 * How far a prose glow's colour leaves the panel's lightness (OKLCH L), toward
 * the text pole (lighter on a dark theme, darker on a light one): a line far
 * from the pole (a pale line on a light theme) at `near`, one at the pole at
 * `far`, so a near-white, a grey and a near-black line, which share no hue,
 * differ by lightness. `least` is as close to the panel as it steps back.
 */
const PROSE_LIFT = { near: 0.09, far: 0.2, least: 0.03 } as const;
/** The prose glow's share of the line's chroma, and its cap. */
const PROSE_CHROMA = { share: 0.9, max: 0.1 } as const;
/** The prose pill's muted words stay at least this far (contrast) from its main text, however the glow pushes them. */
const PROSE_MUTED_APART = 1.4;
/**
 * The prose floors are solved with this much to spare: across a two-colour
 * glow the OKLab blend of two hues can be a little lighter (on a dark theme)
 * or darker (on a light one) in WCAG luminance than both ends, costing a word
 * up to 1.4% of its contrast (measured over hard colour pairs on every
 * theme), so a word that reads on both ends reads on the blend too.
 */
const PROSE_SPARE = 1.02;

/** `target` moved to the nearest colour a wash at {@link PROSE_ALPHA} over `panel` can give. */
function proseReachable(target: string, panel: string): string {
  const t = rgbOfHex(target);
  const p = rgbOfHex(panel);
  const out = t.map((v, i) => p[i]! + PROSE_ALPHA * (clamp(p[i]! + (v - p[i]!) / PROSE_ALPHA, 0, 1) - p[i]!));
  return hexOfRgb(out[0]!, out[1]!, out[2]!);
}

/**
 * A prose pill's colours (RS-PROSE, RS-NODOTS): the pill inline in running
 * text paints no border but a glow of the line's colour, a capsule solid
 * behind the name whose halo fades to the bare panel (index.css
 * `.sh-ppill-vis`). `bg` is the glow over the panel: the line's hue with up
 * to 90% of its chroma, its lightness off the panel's toward the text pole
 * by {@link PROSE_LIFT} (further for a line nearer the pole), reachable by a
 * wash at {@link PROSE_ALPHA}. Always toward the pole, even on a theme whose
 * link pills sink below a dark panel (`theme.lift`): a wash that darkens a
 * near-black panel has no room left for a hue. Main text
 * holds 7:1 and the muted context and Station ID 4.5:1 on the glow and on
 * the panel (with {@link PROSE_SPARE} to spare, for a two-colour blend), the
 * theme's tokens nudged toward the text pole where the glow needs it; the glow steps back toward the panel only as far as it must for
 * those floors and to keep the muted words {@link PROSE_MUTED_APART} from the
 * main text. `hoverBg` is the panel; there is no hover, and `border` is the
 * resting pill's, unused. The leading dot (RS-LEADDOT) sits inside the
 * glow's core at its start: the line colour, ringed where it alone does not
 * reach 3:1 on the glow, over the panel and over the tinted boxes
 * ({@link proseBackdrops}).
 */
export function prosePillColors(lineColor: string | null | undefined, theme: PillTheme): PillColors {
  const pill = pillColors(lineColor, theme);
  const line = lineHex(lineColor, theme.panel);
  const tone = lineTone(line);
  const lineL = clamp(tone.L, 0, 1);
  // A pale line on a dark theme (a dark one on a light theme) goes furthest,
  // spread over the room the theme's text leaves (`proseRoom`).
  const pole = theme.dark ? lineL : 1 - lineL;
  const room = proseRoom(theme);
  const far = room >= PROSE_LIFT.near + PROSE_SPREAD_MIN ? room : PROSE_LIFT.far;
  const want = PROSE_LIFT.near + (far - PROSE_LIFT.near) * pole;
  const C = Math.min(tone.C * PROSE_CHROMA.share, PROSE_CHROMA.max);
  const colors = proseFurthest(pill, theme, want, C, tone.h).colors;
  return { ...colors, ...pillDot(line ?? colors.muted, proseBackdrops(colors.bg, theme), theme) };
}

/**
 * The prose colours at the furthest lift up to `want` whose words keep their
 * floors (they only get harder further from the panel, so found by halving),
 * or at `PROSE_LIFT.least` when even that one fails.
 */
function proseFurthest(pill: PillColors, theme: PillTheme, want: number, C: number, h: number): { colors: PillColors; lift: number } {
  const panelL = oklchOfHex(theme.panel).L;
  // A glow lightens a dark panel and darkens a light one.
  const toward = theme.dark ? 1 : -1;
  const apart = Math.min(PROSE_MUTED_APART, contrastOfHex(theme.text, theme.muted));
  const [textMin, mutedMin] = [TEXT_TARGET * PROSE_SPARE, MUTED_TARGET * PROSE_SPARE];
  const at = (d: number): { colors: PillColors; ok: boolean } => {
    const bg = proseReachable(hexOfOklch({ L: clamp(panelL + toward * d, 0, 1), C, h }), theme.panel);
    // The glow and what it gives over every surface prose sits on, and those surfaces bare.
    const under = proseBackdrops(bg, theme);
    const on = (min: number) => (c: string) => under.every((u) => contrastOfHex(c, u) >= min);
    const text = pushUntil(pill.text, theme.dark, on(textMin));
    const muted = pushUntil(pill.muted, theme.dark, on(mutedMin));
    const ok = on(textMin)(text) && on(mutedMin)(muted) && contrastOfHex(text, muted) >= apart;
    return { colors: { ...pill, bg, hoverBg: theme.panel, text, muted }, ok };
  };
  const wanted = at(want);
  if (wanted.ok) return { colors: wanted.colors, lift: want };
  let lo = at(PROSE_LIFT.least);
  let [near, far]: [number, number] = [PROSE_LIFT.least, want];
  for (let i = 0; i < 12 && lo.ok; i++) {
    const mid = (near + far) / 2;
    const m = at(mid);
    if (m.ok) [lo, near] = [m, mid];
    else far = mid;
  }
  return { colors: lo.colors, lift: near };
}

/** Below this much room past `PROSE_LIFT.near` the band is not spread: every grey glows alike (nord). */
const PROSE_SPREAD_MIN = 0.04;

const roomCache = new WeakMap<PillTheme, number>();

/**
 * How far a neutral prose glow can leave the panel on `theme` with its words
 * legible on it and on the tinted surfaces (at most `PROSE_LIFT.far`): the
 * band a pole-most line takes, so a theme whose text leaves less room spreads
 * the greys over what there is instead of piling the palest together.
 */
function proseRoom(theme: PillTheme): number {
  const cached = roomCache.get(theme);
  if (cached !== undefined) return cached;
  const room = proseFurthest(pillColors(null, theme), theme, PROSE_LIFT.far, 0, 0).lift;
  roomCache.set(theme, room);
  return room;
}

/**
 * Everything a prose pill's words can sit on for a glow of colour `bg` (over
 * the panel): the glow, the panel, and each of the theme's tinted surfaces,
 * bare and under the glow's wash ({@link PROSE_ALPHA}, the colour that gives
 * `bg` over the panel).
 */
export function proseBackdrops(bg: string, theme: PillTheme): string[] {
  const o = proseOverlayOf(bg, theme.panel);
  const wash = (s: string) => {
    const p = rgbOfHex(s);
    return hexOfRgb(...(p.map((v, i) => v + o.alpha * (o.rgb[i]! / 255 - v)) as [number, number, number]));
  };
  return [bg, theme.panel, ...theme.surfaces, ...theme.surfaces.map(wash)];
}

/**
 * The prose glow as CSS colours (RS-PROSE, RS-NODOTS): the wash that,
 * composited at {@link PROSE_ALPHA} over the panel, gives `target` (the
 * glow's colour, `prosePillColors().bg`), and the same colour fully
 * transparent. Prose sits on more than the panel (an accent or warning wash,
 * a toast, a lighter box), and an opaque glow computed for the panel read
 * there as a smudge; the wash moves whatever is behind it the same way it
 * moves the panel.
 */
export function proseTintOf(target: string, panel: string): { tint: string; clear: string } {
  const o = proseOverlayOf(target, panel);
  if (o.alpha === 0) return { tint: 'rgba(0, 0, 0, 0)', clear: 'rgba(0, 0, 0, 0)' };
  const rgb = `${o.rgb[0]}, ${o.rgb[1]}, ${o.rgb[2]}`;
  return { tint: `rgba(${rgb}, ${o.alpha})`, clear: `rgba(${rgb}, 0)` };
}

/**
 * {@link proseTintOf} as numbers: the wash's 8-bit channels at
 * {@link PROSE_ALPHA} (a target out of its reach is clamped), or alpha 0
 * when the target is the panel.
 */
export function proseOverlayOf(target: string, panel: string): { rgb: [number, number, number]; alpha: number } {
  const t = rgbOfHex(target.toLowerCase());
  const p = rgbOfHex(panel.toLowerCase());
  if (t.every((v, i) => v === p[i])) return { rgb: [0, 0, 0], alpha: 0 };
  const channel = (i: number) => Math.round(clamp(p[i]! + (t[i]! - p[i]!) / PROSE_ALPHA, 0, 1) * 255);
  return { rgb: [channel(0), channel(1), channel(2)], alpha: PROSE_ALPHA };
}

const proseCache = new Map<string, PillColors>();

/** {@link prosePillColors} for a named app theme, cached per (theme, colour). */
export function prosePillColorsOf(lineColor: string | null | undefined, themeName: string): PillColors {
  const key = `${themeName}\u0000${lineColor ?? ''}`;
  const cached = proseCache.get(key);
  if (cached) return cached;
  const colors = prosePillColors(lineColor, pillThemeOf(themeName));
  if (proseCache.size > 2048) proseCache.clear();
  proseCache.set(key, colors);
  return colors;
}

/** {@link pillTheme} of a named app theme (`resolvedTheme`); an unknown name is the dark theme. */
export function pillThemeOf(themeName: string): PillTheme {
  const cached = themeCache.get(themeName);
  if (cached) return cached;
  const all = themeTokens as Record<string, Record<string, string> | undefined>;
  const t: Record<string, string> = all[themeName] ?? themeTokens.dark;
  const theme = pillTheme({
    panel: t['--cc-panel']!,
    textPrimary: t['--cc-text-primary']!,
    textSecondary: t['--cc-text-secondary']!,
    warningText: t['--cc-warning-text']!,
    surfaces: PROSE_SURFACES.map((k) => t[k]).filter((c): c is string => c !== undefined),
  });
  themeCache.set(themeName, theme);
  return theme;
}

const themeCache = new Map<string, PillTheme>();

/**
 * The theme tokens of the tinted boxes prose sits in (dialog notes, the
 * conversion banner, the Station details box). No prose sits on a 20% wash
 * (only the data table's view toggle uses one).
 */
const PROSE_SURFACES = ['--cc-accent-10', '--cc-warning-10', '--cc-error-10'];
const colorCache = new Map<string, PillColors>();

/** {@link pillColors} for a named app theme, cached per (theme, colour). */
export function pillColorsOf(lineColor: string | null | undefined, themeName: string): PillColors {
  const key = `${themeName}\u0000${lineColor ?? ''}`;
  const cached = colorCache.get(key);
  if (cached) return cached;
  const colors = pillColors(lineColor, pillThemeOf(themeName));
  if (colorCache.size > 2048) colorCache.clear();
  colorCache.set(key, colors);
  return colors;
}

/** {@link missingPillColors} for a named app theme, cached. */
export function missingPillColorsOf(themeName: string): MissingPillColors {
  const cached = missingCache.get(themeName);
  if (cached) return cached;
  const colors = missingPillColors(pillThemeOf(themeName));
  missingCache.set(themeName, colors);
  return colors;
}

const missingCache = new Map<string, MissingPillColors>();

/**
 * A line's colour dot drawn straight on the panel (the Sections panel's
 * rows), with the pill's ring rule (RS-FX6, P3-17): a near-white line on a
 * light theme or a near-black one on a dark theme gets a ring in its hue on
 * the text side, so the dot does not vanish; any other colour none.
 */
export function panelDotOf(lineColor: string | null | undefined, themeName: string): PillDot {
  const theme = pillThemeOf(themeName);
  return pillDot(lineHex(lineColor, theme.panel) ?? theme.muted, [theme.panel], theme);
}
