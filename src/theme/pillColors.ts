/**
 * Line pill colours: everything a link pill (`DetailLink`, `EntityBlob`)
 * paints, derived from the colour of the line it belongs to and the theme.
 *
 * A line colour is user data and can be anything (near-white, near-black, pure
 * yellow, a CSS name), so nothing here uses it as a fill directly except the
 * dot. The rest is a tint of its hue, in OKLCH, with lightness and chroma
 * clamped to one band per theme:
 *
 * - background: the panel lifted a little toward the line's hue (dark themes
 *   lift it, light themes sink it), chroma capped, so a yellow pill and a navy
 *   pill have nearly the same lightness and read as one family;
 * - border: the same hue further from the panel, so the edge carries the line
 *   colour without the fill having to;
 * - hover: a stronger tint and border, inside the same legibility limits;
 * - dot: the line colour itself (as the Sections panel draws it), with a ring
 *   in the line's hue on the text side when the colour alone is too close to
 *   the pill's background (a near-black line on a dark theme);
 * - text: the theme's own text tokens, nudged toward black or white only on a
 *   theme whose token cannot reach the floor on the band.
 *
 * Floors (pinned by `__tests__/pillColors.test.ts` over a sweep of colours on
 * every theme): main text 7:1 (WCAG AAA), muted context and Station ID 4.5:1
 * (AA), dot or its ring 3:1 (non-text), on the resting and the hover
 * background. Every value is `#rrggbb`.
 */
import { themeTokens } from './tokens';
import {
  contrastOfHex,
  flattenOver,
  hexOfOklch,
  oklabOf,
  oklchOfHex,
  oklchOfOklab,
  parseCssColor,
  type Oklch,
} from './cssColor';

export const PILL_TEXT_MIN = 7;
export const PILL_MUTED_MIN = 4.5;
export const PILL_DOT_MIN = 3;

/** Solve to these, a little above the floors, so rounding to 8 bits never dips under. */
const TEXT_TARGET = PILL_TEXT_MIN + 0.1;
const MUTED_TARGET = PILL_MUTED_MIN + 0.1;
const DOT_TARGET = PILL_DOT_MIN + 0.25;

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

/** A pill whose target is gone: neutral (no line colour), dashed, a warning glyph for the dot. */
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
const BG_CHROMA = { share: 0.5, max: { dark: 0.05, light: 0.04 } } as const;
const HOVER_LIFT = 0.02;
const HOVER_CHROMA = { share: 0.7, max: { dark: 0.07, light: 0.06 } } as const;
const BORDER_LIFT = { rest: 0.17, hover: 0.26 } as const;
const BORDER_CHROMA = { rest: { share: 0.7, max: 0.1 }, hover: { share: 0.85, max: 0.14 } } as const;
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
  // The border leaves from whichever of the panel and the fill is further out,
  // so a pill sunk below the panel still has an edge that reads on the panel.
  const borderBase = edge > 0 ? Math.max(bgL, panelL) : Math.min(bgL, panelL);
  const border = hexOfOklch({
    L: clamp(borderBase + edge * BORDER_LIFT.rest, 0, 1),
    C: Math.min(tone.C * BORDER_CHROMA.rest.share, BORDER_CHROMA.rest.max),
    h: tone.h,
  });
  const hoverBorder = hexOfOklch({
    L: clamp(borderBase + edge * BORDER_LIFT.hover, 0, 1),
    C: Math.min(tone.C * BORDER_CHROMA.hover.share, BORDER_CHROMA.hover.max),
    h: tone.h,
  });
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
  const border = hexOfOklch({ L: clamp(base + edge * BORDER_LIFT.hover, 0, 1), C: 0, h: 0 });
  const warning = pushUntil(theme.warning, theme.dark, (c) => contrastOfHex(c, bg) >= DOT_TARGET);
  return { bg, border, text: theme.text, muted: theme.muted, warning };
}

/**
 * A prose pill's colours (RS-PROSE): the pill inline in running text paints
 * no fill and no border but a soft radial tint of the line's hue, strongest
 * at its centre (the hover band's tint, `hoverBg`) and fading to nothing at
 * its edges, where the bare panel shows. So everything on it is held on both
 * ends of the fade: main text 7:1, the muted context and Station ID 4.5:1, the
 * dot or its ring and the home-line dot 3:1, on the tint and on the panel.
 * `bg` is the tint's centre and `hoverBg` the panel (what {@link pillMarkOf}
 * checks a home-line dot against); there is no hover, and `border` is the
 * resting pill's, unused. Text is the theme's own unless a floor needs it
 * nudged, as for the link pill.
 */
export function prosePillColors(lineColor: string | null | undefined, theme: PillTheme): PillColors {
  const pill = pillColors(lineColor, theme);
  const on = [pill.hoverBg, theme.panel];
  const reaches = (min: number) => (c: string) => on.every((bg) => contrastOfHex(c, bg) >= min);
  const text = pushUntil(pill.text, theme.dark, reaches(TEXT_TARGET));
  const muted = pushUntil(pill.muted, theme.dark, reaches(MUTED_TARGET));
  const { dot, ring } = pillDot(lineHex(lineColor, theme.panel) ?? muted, on, theme);
  return { ...pill, bg: pill.hoverBg, hoverBg: theme.panel, text, muted, dot, ring };
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
  const t = all[themeName] ?? themeTokens.dark;
  const theme = pillTheme({
    panel: t['--cc-panel']!,
    textPrimary: t['--cc-text-primary']!,
    textSecondary: t['--cc-text-secondary']!,
    warningText: t['--cc-warning-text']!,
  });
  themeCache.set(themeName, theme);
  return theme;
}

const themeCache = new Map<string, PillTheme>();
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

/** The second dot (a station's home line) on a pill already coloured `on`. */
export function pillMarkOf(markColor: string | null | undefined, on: PillColors, themeName: string): PillDot {
  const theme = pillThemeOf(themeName);
  return pillDot(lineHex(markColor, theme.panel) ?? theme.muted, [on.bg, on.hoverBg], theme);
}
