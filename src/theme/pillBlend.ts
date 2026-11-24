/**
 * The blended seams of a two-tone shot pill (RS-BLEND, the user's request:
 * "for the multi-color pills, can you blend the color transition"; RS-ARROW:
 * "the color should blend over in the middle of the -> arrow, not before
 * it").
 *
 * A shot with an end of another line is drawn in segments, each in its own
 * line's colours (RS-OWNERSHIP): an optional head (the context, in the
 * shot's colour, when the from end's half is another line's), the from end's half, the arrow, the to end's half.
 *
 * - The arrow is a segment of its own, a **bridge**: its colour runs from
 *   the from half's to the to half's across its whole width, so the
 *   cross-fade is centred on the arrow and the arrow sits half in each
 *   colour. The halves beside it keep their own colour up to it.
 * - The head and the from half meet in a **seam**: each fades over the last
 *   {@link SEAM_EM} from its own colour to the midpoint with the other, so
 *   the two meet in one colour across a 2 x SEAM_EM cross-fade.
 *
 * Colours mix in OKLab (`mixHex`: yellow and blue meet in a neutral that
 * keeps some colour, not sRGB's duller grey), with stops at the quarters so
 * the browser's sRGB interpolation between stops stays near the OKLab path.
 *
 * - A link pill paints its fill on the padding box and its border on the
 *   border box (`--seg-fill`, `--seg-edge`, and their hover forms), so the
 *   border blends with the fill and the rounded ends stay intact.
 * - A prose pill has no fill or border but a glow, a soft-edged capsule per
 *   line whose colour follows the same blends, mixed in OKLab, straight from
 *   one colour to the other with no dip toward the panel
 *   (`theme/proseGlow.ts`, `components/common/useProseGlow.ts`); here only
 *   its text colours.
 *
 * Text over a blend must read on every colour it passes through:
 * {@link blendBackdrops} and {@link bridgeBackdrops} list them (sampled),
 * and the arrow takes the text colour that reads on all of them
 * (`__tests__/twoTonePills.test.ts` sweeps the samples).
 */
import type { CSSProperties } from 'react';
import { contrastOfHex, oklabOf, rgbOfOklab, hexOfRgb } from './cssColor';
import { PILL_MUTED_MIN, PILL_TEXT_MIN, pillThemeOf, proseBackdrops, proseTintOf, type PillColors } from './pillColors';

/** How far a segment fades at a seam; the cross-fade is twice this, centred on the seam. */
export const SEAM_EM = 0.4;

/** Where the samples of a seam's blend are taken, as the share of the segment's own colour (0.5 at the seam). */
export const BLEND_SAMPLES = [0.5, 0.625, 0.75, 0.875] as const;

/** Where the samples of a bridge are taken, as the share of its left colour. */
export const BRIDGE_SAMPLES = [1, 0.875, 0.75, 0.625, 0.5, 0.375, 0.25, 0.125, 0] as const;

function rgbOfHex(hex: string): [number, number, number] {
  const n = parseInt(hex.slice(1, 7), 16);
  return [((n >> 16) & 0xff) / 255, ((n >> 8) & 0xff) / 255, (n & 0xff) / 255];
}

/** `a` and `b` mixed in OKLab: `t` of `a` (0..1), as `#rrggbb`. */
export function mixHex(a: string, b: string, t: number): string {
  if (a === b || t >= 1) return a;
  if (t <= 0) return b;
  const x = oklabOf(...rgbOfHex(a));
  const y = oklabOf(...rgbOfHex(b));
  const lab = { L: x.L * t + y.L * (1 - t), a: x.a * t + y.a * (1 - t), b: x.b * t + y.b * (1 - t) };
  const [r, g, bl] = rgbOfOklab(lab);
  return hexOfRgb(r, g, bl);
}

/**
 * What a segment meets at each side: the colours at that edge of its
 * neighbour (null: an outer end). A seam blends toward them; the edge of a
 * bridge is the segment's own colour, so nothing changes there.
 */
export interface Neighbours {
  left: PillColors | null;
  right: PillColors | null;
}

/**
 * Every colour a segment's text can sit on: its fill and hover fill
 * (a prose pill's tint centre and the bare panel, `panel` given), and the
 * blends toward each neighbour at {@link BLEND_SAMPLES} (the hover blends
 * too).
 */
export function blendBackdrops(own: PillColors, near: Neighbours, panel?: string): string[] {
  const out = panel === undefined ? [own.bg, own.hoverBg] : [own.bg, panel];
  for (const other of [near.left, near.right]) {
    if (!other || other.bg === own.bg) continue;
    for (const t of BLEND_SAMPLES) {
      out.push(mixHex(own.bg, other.bg, t));
      if (panel === undefined) out.push(mixHex(own.hoverBg, other.hoverBg, t));
    }
  }
  return out;
}

/** The sRGB point halfway between two colours: what a gradient paints between two stops. */
function srgbMidHex(a: string, b: string): string {
  const [x, y] = [rgbOfHex(a), rgbOfHex(b)];
  return hexOfRgb((x[0] + y[0]) / 2, (x[1] + y[1]) / 2, (x[2] + y[2]) / 2);
}

/**
 * Every colour a prose glow paints along a path of stops: the stops and the
 * sRGB midpoints between them (an SVG gradient interpolates in sRGB, and a
 * mix of two hues can be lighter there than either stop).
 */
function painted(path: readonly string[]): string[] {
  const out = [...path];
  for (let i = 1; i < path.length; i++) out.push(srgbMidHex(path[i - 1]!, path[i]!));
  return out;
}

/** Every colour under a bridge from `left` to `right` at {@link BRIDGE_SAMPLES} (hover too; the panel for prose). */
export function bridgeBackdrops(left: PillColors, right: PillColors, panel?: string): string[] {
  const out: string[] = panel === undefined ? [] : [panel];
  for (const t of BRIDGE_SAMPLES) {
    out.push(mixHex(left.bg, right.bg, t));
    if (panel === undefined) out.push(mixHex(left.hoverBg, right.hoverBg, t));
  }
  return out;
}

/**
 * The first of `candidates` reaching `min` on every backdrop. When none does
 * (a blend of two glows can be lighter than either, OKLab lightness and
 * WCAG luminance part ways where the hue turns), the best of them moved
 * toward the text pole (`towardWhite`: a dark theme's) until it does.
 */
export function readableOn(candidates: readonly string[], backdrops: readonly string[], min: number, towardWhite?: boolean): string {
  let best = candidates[0]!;
  let bestWorst = -1;
  const worstOf = (c: string) => Math.min(...backdrops.map((bg) => contrastOfHex(c, bg)));
  for (const c of candidates) {
    const worst = worstOf(c);
    if (worst >= min) return c;
    if (worst > bestWorst) {
      best = c;
      bestWorst = worst;
    }
  }
  if (towardWhite === undefined) return best;
  const pole = towardWhite ? '#ffffff' : '#000000';
  for (let step = 1; step <= 20; step++) {
    const c = mixHex(best, pole, 1 - step / 20);
    if (worstOf(c) >= min) return c;
  }
  return pole;
}

/** Colour stops across a segment: [offset (0..1, or a CSS length string), colour]. */
type Stop = [number | string, string];

/** A seam segment's stops: its own colour, fading to the midpoint with each neighbour over SEAM_EM. */
function seamStops(own: string, left: string | null, right: string | null): Stop[] {
  const at = (fromRight: boolean, share: number): string => {
    const em = `${SEAM_EM * share}em`;
    return fromRight ? `calc(100% - ${em})` : em;
  };
  const stops: Stop[] = [];
  if (left !== null && left !== own) {
    stops.push([0, mixHex(own, left, 0.5)], [at(false, 0.5), mixHex(own, left, 0.75)], [at(false, 1), own]);
  } else {
    stops.push([0, own]);
  }
  if (right !== null && right !== own) {
    stops.push([at(true, 1), own], [at(true, 0.5), mixHex(own, right, 0.75)], [1, mixHex(own, right, 0.5)]);
  } else {
    stops.push([1, own]);
  }
  return stops;
}

/** A bridge's stops: `left` to `right` across its width, OKLab mixes at the quarters. */
function bridgeStops(left: string, right: string): Stop[] {
  return [0, 0.25, 0.5, 0.75, 1].map((p) => [p, mixHex(left, right, 1 - p)] as Stop);
}

const cssOffset = (o: number | string) => (typeof o === 'number' ? `${(o * 100).toFixed(2)}%` : o);
const cssGradient = (stops: Stop[]) => `linear-gradient(to right, ${stops.map(([o, c]) => `${c} ${cssOffset(o)}`).join(', ')})`;

/** A link segment's colour custom properties from `stops` picked per colour (fill, border, at rest and on hover). */
function linkPaint(stopsOf: (pick: (c: PillColors) => string) => Stop[]): CSSProperties {
  return {
    '--seg-fill': cssGradient(stopsOf((c) => c.bg)),
    '--seg-fill-hover': cssGradient(stopsOf((c) => c.hoverBg)),
    '--seg-edge': cssGradient(stopsOf((c) => c.border)),
    '--seg-edge-hover': cssGradient(stopsOf((c) => c.hoverBorder)),
  } as CSSProperties;
}

/**
 * A link pill segment's custom properties: its own colours (the `.sh-lpill`
 * names), and its fill and border as gradients, at rest and on hover.
 */
export function linkSegmentStyle(own: PillColors, near: Neighbours): CSSProperties {
  return {
    '--pill-bg': own.bg,
    '--pill-border': own.border,
    '--pill-bg-hover': own.hoverBg,
    '--pill-border-hover': own.hoverBorder,
    '--pill-text': own.text,
    '--pill-muted': own.muted,
    ...linkPaint((pick) => seamStops(pick(own), near.left ? pick(near.left) : null, near.right ? pick(near.right) : null)),
  } as CSSProperties;
}

/**
 * The arrow of a link pill: a bridge from `left`'s colours to `right`'s,
 * its text in whichever of their text colours reads on every colour under
 * it.
 */
export function linkBridgeStyle(left: PillColors, right: PillColors, themeName: string): CSSProperties {
  const theme = pillThemeOf(themeName);
  const under = bridgeBackdrops(left, right);
  return {
    '--pill-bg': left.bg,
    '--pill-border': left.border,
    '--pill-bg-hover': left.hoverBg,
    '--pill-border-hover': left.hoverBorder,
    '--pill-text': readableOn([left.text, right.text, theme.text], under, 7),
    '--pill-muted': readableOn([left.muted, right.muted, theme.muted], under, 4.5),
    ...linkPaint((pick) => bridgeStops(pick(left), pick(right))),
  } as CSSProperties;
}

/**
 * A prose pill segment's custom properties: its text and muted part, each
 * whichever of its own, its neighbours' and the theme's reads on every
 * colour it can sit on (its glow, the blends toward its neighbours and the
 * panel, {@link blendBackdrops}: a near-black half's words beside a
 * near-white half sit on a lighter blend than their own glow), and its
 * glow's wash for the CSS fallback (`--pill-tint`). The glow itself is
 * painted per line once laid out (`useProseGlow`).
 */
export function proseSegmentStyle(own: PillColors, near: Neighbours, themeName: string): CSSProperties {
  const theme = pillThemeOf(themeName);
  // The glow's stops toward each neighbour (own colour to the midpoint, as
  // `blendBackdrops` samples them) and what is painted between them, over
  // the panel and over every tinted surface prose sits on.
  const path = [own.bg];
  for (const other of [near.left, near.right]) {
    if (other && other.bg !== own.bg) path.push(...painted([own.bg, ...[...BLEND_SAMPLES].reverse().map((t) => mixHex(own.bg, other.bg, t))]));
  }
  const under = path.flatMap((c) => proseBackdrops(c, theme));
  const others = [near.left, near.right].filter((c): c is PillColors => c !== null);
  return {
    '--pill-text': readableOn([own.text, ...others.map((c) => c.text), theme.text], under, PILL_TEXT_MIN + 0.1, theme.dark),
    '--pill-muted': readableOn([own.muted, ...others.map((c) => c.muted), theme.muted], under, PILL_MUTED_MIN + 0.1, theme.dark),
    '--pill-tint': proseTintOf(own.bg, theme.panel).tint,
  } as CSSProperties;
}

/** The arrow of a prose pill: a bridge, its text in a colour that reads on every colour under it. */
export function proseBridgeStyle(left: PillColors, right: PillColors, themeName: string): CSSProperties {
  const theme = pillThemeOf(themeName);
  const under = painted(BRIDGE_SAMPLES.map((t) => mixHex(left.bg, right.bg, t))).flatMap((c) => proseBackdrops(c, theme));
  return {
    '--pill-text': readableOn([left.text, right.text, theme.text], under, PILL_TEXT_MIN + 0.1, theme.dark),
    '--pill-muted': readableOn([left.muted, right.muted, theme.muted], under, PILL_MUTED_MIN + 0.1, theme.dark),
    '--pill-tint': proseTintOf(left.bg, theme.panel).tint,
  } as CSSProperties;
}
