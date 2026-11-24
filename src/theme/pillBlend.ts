/**
 * The blended seams of a two-tone shot pill (RS-BLEND, the user's request:
 * "for the multi-color pills, can you blend the color transition"; RS-ARROW:
 * "the color should blend over in the middle of the -> arrow, not before
 * it").
 *
 * A shot with an end of another line is drawn in segments, each in its own
 * line's colours (RS-OWNERSHIP): an optional head (the leading dot and the
 * context, in the shot's colour, when the from end's half is another
 * line's), the from end's half, the arrow, the to end's half.
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
 * - A prose pill has no fill or border but a radial tint; a segment's tint
 *   is an SVG background whose colour follows the same stops, masked by the
 *   tint's fade, which is solid at every joined side: one continuous tint,
 *   fading only at the outer ends and top and bottom, with no dip. A wrapped
 *   pill is tinted per line (`components/common/useSegmentTints.ts`).
 *
 * Text and dots over a blend must read on every colour it passes through:
 * {@link blendBackdrops} and {@link bridgeBackdrops} list them (sampled);
 * dots get their ring against them and the arrow takes the text colour that
 * reads on all of them (`__tests__/twoTonePills.test.ts` sweeps the samples).
 */
import type { CSSProperties } from 'react';
import { contrastOfHex, oklabOf, rgbOfOklab, hexOfRgb } from './cssColor';
import { pillDot, pillThemeOf, proseOverlayOf, type PillColors, type PillDot } from './pillColors';

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
 * Every colour a segment's text and dots can sit on: its fill and hover fill
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

/** Every colour under a bridge from `left` to `right` at {@link BRIDGE_SAMPLES} (hover too; the panel for prose). */
export function bridgeBackdrops(left: PillColors, right: PillColors, panel?: string): string[] {
  const out: string[] = panel === undefined ? [] : [panel];
  for (const t of BRIDGE_SAMPLES) {
    out.push(mixHex(left.bg, right.bg, t));
    if (panel === undefined) out.push(mixHex(left.hoverBg, right.hoverBg, t));
  }
  return out;
}

/** The first of `candidates` reaching `min` on every backdrop, else the one whose worst contrast is best. */
export function readableOn(candidates: readonly string[], backdrops: readonly string[], min: number): string {
  let best = candidates[0]!;
  let bestWorst = -1;
  for (const c of candidates) {
    const worst = Math.min(...backdrops.map((bg) => contrastOfHex(c, bg)));
    if (worst >= min) return c;
    if (worst > bestWorst) {
      best = c;
      bestWorst = worst;
    }
  }
  return best;
}

/** Colour stops across a segment: [offset (0..1, or a CSS length string), colour]. */
type Stop = [number | string, string];

/** A seam segment's stops: its own colour, fading to the midpoint with each neighbour over SEAM_EM. */
function seamStops(own: string, left: string | null, right: string | null, seam: number | null): Stop[] {
  const at = (fromRight: boolean, share: number): number | string => {
    if (seam !== null) return fromRight ? 1 - seam * share : seam * share;
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
 * names), its dot and ring checked against every backdrop, and its fill and
 * border as gradients, at rest and on hover.
 */
export function linkSegmentStyle(own: PillColors, near: Neighbours, themeName: string): CSSProperties {
  const theme = pillThemeOf(themeName);
  const dot: PillDot = pillDot(own.dot, blendBackdrops(own, near), theme);
  return {
    '--pill-bg': own.bg,
    '--pill-border': own.border,
    '--pill-bg-hover': own.hoverBg,
    '--pill-border-hover': own.hoverBorder,
    '--pill-text': own.text,
    '--pill-muted': own.muted,
    '--pill-dot': dot.dot,
    '--pill-ring': dot.ring ?? 'transparent',
    ...linkPaint((pick) => seamStops(pick(own), near.left ? pick(near.left) : null, near.right ? pick(near.right) : null, null)),
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

/** An overlay colour for an SVG stop: `stop-color` and `stop-opacity`. */
function overlayStop(offset: number, target: string, panel: string): string {
  const o = proseOverlayOf(target, panel);
  const [r, g, b] = o.rgb;
  return `<stop offset='${offset.toFixed(4)}' stop-color='rgb(${r},${g},${b})' stop-opacity='${o.alpha.toFixed(4)}'/>`;
}

/**
 * A prose tint as an SVG `url()`: the colour stops (overlays that give
 * those colours over the panel), masked by the tint's fade (solid to 45%
 * of the radius, nothing at the edge, as the one-colour tint), whose centre
 * is a joined side: a radial fade centred on the one joined side (the
 * middle, when neither is), a top-and-bottom fade only when both are.
 */
function proseTint(stops: Stop[], panel: string, joinLeft: boolean, joinRight: boolean): string {
  const colour = stops.map(([o, c]) => overlayStop(typeof o === 'number' ? o : 0, c, panel)).join('');
  const fadeStops = `<stop offset='0' stop-color='white'/><stop offset='0.45' stop-color='white'/><stop offset='1' stop-color='white' stop-opacity='0'/>`;
  let fade: string;
  if (joinLeft && joinRight) {
    fade = `<linearGradient id='f' x1='0' y1='0' x2='0' y2='1'><stop offset='0' stop-color='white' stop-opacity='0'/><stop offset='0.275' stop-color='white'/><stop offset='0.725' stop-color='white'/><stop offset='1' stop-color='white' stop-opacity='0'/></linearGradient>`;
  } else {
    // A radial fade centred on the joined side (or the middle): the ellipse
    // reaches the far end and the top and bottom.
    const cx = joinRight ? 1 : joinLeft ? 0 : 0.5;
    const rx = joinLeft || joinRight ? 1 : 0.5;
    fade = `<radialGradient id='f' cx='${cx}' cy='0.5' r='1' gradientTransform='translate(${cx} 0.5) scale(${rx} 0.5) translate(${-cx} -0.5)'>${fadeStops}</radialGradient>`;
  }
  const svg =
    `<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100' preserveAspectRatio='none'>` +
    `<defs><linearGradient id='c' x1='0' y1='0' x2='1' y2='0'>${colour}</linearGradient>${fade}` +
    `<mask id='m'><rect width='100' height='100' fill='url(#f)'/></mask></defs>` +
    `<rect width='100' height='100' fill='url(#c)' mask='url(#m)'/></svg>`;
  return `url("data:image/svg+xml,${encodeURIComponent(svg)}")`;
}

/**
 * A prose segment's tint (see {@link proseTint}): its own colour, shifting
 * to the midpoint with a neighbour over `seam` of its width at a seam; a
 * side whose neighbour is a bridge (its own colour) is joined but keeps its
 * colour.
 */
export function proseSegmentTint(own: PillColors, near: Neighbours, panel: string, seam: number): string {
  const s = Math.min(0.45, Math.max(0.08, seam));
  return proseTint(seamStops(own.bg, near.left?.bg ?? null, near.right?.bg ?? null, s), panel, near.left !== null, near.right !== null);
}

/** A prose bridge's tint: `left`'s tint to `right`'s across its width; `join` says which sides meet a segment on the same line. */
export function proseBridgeTint(left: PillColors, right: PillColors, panel: string, join: { left: boolean; right: boolean }): string {
  return proseTint(bridgeStops(left.bg, right.bg), panel, join.left, join.right);
}

/**
 * A prose pill segment's custom properties: its text, muted part, its dot
 * and ring checked against every backdrop (the tint, the panel and the
 * blends), and its tint (`--seg-tint`, {@link proseSegmentTint}).
 * `widthEm` estimates the segment's width, so the seam fade is about
 * {@link SEAM_EM} wide.
 */
export function proseSegmentStyle(own: PillColors, near: Neighbours, themeName: string, widthEm: number): CSSProperties {
  const theme = pillThemeOf(themeName);
  const dot: PillDot = pillDot(own.dot, blendBackdrops(own, near, theme.panel), theme);
  return {
    '--pill-text': own.text,
    '--pill-muted': own.muted,
    '--pill-dot': dot.dot,
    '--pill-ring': dot.ring ?? 'transparent',
    '--seg-tint': proseSegmentTint(own, near, theme.panel, SEAM_EM / Math.max(widthEm, 0.1)),
  } as CSSProperties;
}

/** The arrow of a prose pill: a bridge (both sides joined), its text in a colour that reads on every colour under it. */
export function proseBridgeStyle(left: PillColors, right: PillColors, themeName: string): CSSProperties {
  const theme = pillThemeOf(themeName);
  const under = bridgeBackdrops(left, right, theme.panel);
  return {
    '--pill-text': readableOn([left.text, right.text, theme.text], under, 7),
    '--pill-muted': readableOn([left.muted, right.muted, theme.muted], under, 4.5),
    '--seg-tint': proseBridgeTint(left, right, theme.panel, { left: true, right: true }),
  } as CSSProperties;
}
