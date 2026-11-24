/**
 * The blended seams of a two-tone shot pill (RS-BLEND, the user's request:
 * "for the multi-color pills, can you blend the color transition").
 *
 * A shot with an end of another line is drawn in segments, each in its own
 * line's colours (RS-OWNERSHIP). Instead of a hard seam, each segment fades
 * over the last {@link SEAM_EM} at a seam from its own colour to the
 * midpoint with its neighbour, mixed in OKLab (so yellow and blue meet in a
 * clean green-grey, not a muddy one): two segments meet in the same colour,
 * and the cross-fade is 2 x SEAM_EM wide, centred on the seam (just before
 * the arrow). Each half still carries its line's colour away from the seam.
 * A stop at three quarters keeps the browser's sRGB interpolation between
 * stops close to the OKLab path.
 *
 * - A link pill paints its fill on the padding box and its border on the
 *   border box (`--seg-fill`, `--seg-edge`, and their hover forms), so the
 *   border blends with the fill and the rounded ends stay intact.
 * - A prose pill has no fill or border but a radial tint; a segment's tint
 *   is an SVG background whose colour shifts toward the midpoint at a seam,
 *   masked by the tint's fade, centred on the seam so the alpha is the same
 *   on both sides of it: one continuous tint, fading only at the outer
 *   edges and top and bottom, with no dip at the seam.
 *
 * Text and dots that sit over a blend must read on every colour it passes
 * through: {@link blendBackdrops} lists them (sampled), and dots get their
 * ring against them (`__tests__/twoTonePills.test.ts` sweeps the samples).
 */
import type { CSSProperties } from 'react';
import { oklabOf, rgbOfOklab, hexOfRgb } from './cssColor';
import { pillDot, pillThemeOf, proseOverlayOf, type PillColors, type PillDot } from './pillColors';

/** How far a segment fades at a seam; the cross-fade is twice this, centred on the seam. */
export const SEAM_EM = 0.4;

/** Where the samples of a blend are taken, as the share of the segment's own colour (0.5 at the seam). */
export const BLEND_SAMPLES = [0.5, 0.625, 0.75, 0.875] as const;

function rgbOfHex(hex: string): [number, number, number] {
  const n = parseInt(hex.slice(1, 7), 16);
  return [((n >> 16) & 0xff) / 255, ((n >> 8) & 0xff) / 255, (n & 0xff) / 255];
}

/** `a` and `b` mixed in OKLab: `t` of `a` (0..1), as `#rrggbb`. */
export function mixHex(a: string, b: string, t: number): string {
  const x = oklabOf(...rgbOfHex(a));
  const y = oklabOf(...rgbOfHex(b));
  const lab = { L: x.L * t + y.L * (1 - t), a: x.a * t + y.a * (1 - t), b: x.b * t + y.b * (1 - t) };
  const [r, g, bl] = rgbOfOklab(lab);
  return hexOfRgb(r, g, bl);
}

/** The neighbours of a segment: the one before it and the one after it, when there are. */
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
    if (!other) continue;
    for (const t of BLEND_SAMPLES) {
      out.push(mixHex(own.bg, other.bg, t));
      if (panel === undefined) out.push(mixHex(own.hoverBg, other.hoverBg, t));
    }
  }
  return out;
}

/** A gradient across a segment: its own colour, fading to the midpoint with each neighbour at that side. */
function seamGradient(own: string, left: string | null, right: string | null): string {
  const stops: string[] = [];
  const half = SEAM_EM / 2;
  if (left !== null) {
    stops.push(`${mixHex(own, left, 0.5)} 0`, `${mixHex(own, left, 0.75)} ${half}em`, `${own} ${SEAM_EM}em`);
  } else {
    stops.push(`${own} 0`);
  }
  if (right !== null) {
    stops.push(
      `${own} calc(100% - ${SEAM_EM}em)`,
      `${mixHex(own, right, 0.75)} calc(100% - ${half}em)`,
      `${mixHex(own, right, 0.5)} 100%`,
    );
  } else {
    stops.push(`${own} 100%`);
  }
  return `linear-gradient(to right, ${stops.join(', ')})`;
}

/**
 * A link pill segment's custom properties: its own colours (the `.sh-lpill`
 * names), its dot and ring checked against every backdrop, and its fill and
 * border as blended gradients, at rest and on hover.
 */
export function linkSegmentStyle(own: PillColors, near: Neighbours, themeName: string): CSSProperties {
  const theme = pillThemeOf(themeName);
  const dot: PillDot = pillDot(own.dot, blendBackdrops(own, near), theme);
  const side = (pick: (c: PillColors) => string) =>
    seamGradient(pick(own), near.left ? pick(near.left) : null, near.right ? pick(near.right) : null);
  return {
    '--pill-bg': own.bg,
    '--pill-border': own.border,
    '--pill-bg-hover': own.hoverBg,
    '--pill-border-hover': own.hoverBorder,
    '--pill-text': own.text,
    '--pill-muted': own.muted,
    '--pill-dot': dot.dot,
    '--pill-ring': dot.ring ?? 'transparent',
    '--seg-fill': side((c) => c.bg),
    '--seg-fill-hover': side((c) => c.hoverBg),
    '--seg-edge': side((c) => c.border),
    '--seg-edge-hover': side((c) => c.hoverBorder),
  } as CSSProperties;
}

/** An overlay colour for an SVG stop: `stop-color` and `stop-opacity`. */
function overlayStop(offset: number, target: string, panel: string): string {
  const o = proseOverlayOf(target, panel);
  const [r, g, b] = o.rgb;
  return `<stop offset='${offset.toFixed(4)}' stop-color='rgb(${r},${g},${b})' stop-opacity='${o.alpha.toFixed(4)}'/>`;
}

/**
 * A prose segment's tint as an SVG `url()`: the colour gradient (its tint
 * centre, shifting to the midpoint with a neighbour over `seam` of its width
 * at that side; overlays that give those colours over the panel), masked by
 * the tint's fade (solid to 45% of the radius, nothing at the edge, as the
 * one-colour tint), whose centre is the seam: a radial fade centred on a
 * seam at one side, a top-and-bottom fade only between two seams.
 */
export function proseSegmentTint(own: PillColors, near: Neighbours, panel: string, seam: number): string {
  const s = Math.min(0.45, Math.max(0.08, seam));
  const stops: string[] = [];
  if (near.left) {
    stops.push(overlayStop(0, mixHex(own.bg, near.left.bg, 0.5), panel));
    stops.push(overlayStop(s / 2, mixHex(own.bg, near.left.bg, 0.75), panel));
    stops.push(overlayStop(s, own.bg, panel));
  } else {
    stops.push(overlayStop(0, own.bg, panel));
  }
  if (near.right) {
    stops.push(overlayStop(1 - s, own.bg, panel));
    stops.push(overlayStop(1 - s / 2, mixHex(own.bg, near.right.bg, 0.75), panel));
    stops.push(overlayStop(1, mixHex(own.bg, near.right.bg, 0.5), panel));
  } else {
    stops.push(overlayStop(1, own.bg, panel));
  }
  const fadeStops = `<stop offset='0' stop-color='white'/><stop offset='0.45' stop-color='white'/><stop offset='1' stop-color='white' stop-opacity='0'/>`;
  let fade: string;
  if (near.left && near.right) {
    fade = `<linearGradient id='f' x1='0' y1='0' x2='0' y2='1'><stop offset='0' stop-color='white' stop-opacity='0'/><stop offset='0.275' stop-color='white'/><stop offset='0.725' stop-color='white'/><stop offset='1' stop-color='white' stop-opacity='0'/></linearGradient>`;
  } else {
    // A radial fade centred on the seam (or the middle, for no seam): the
    // ellipse reaches the far edge and the top and bottom.
    const cx = near.right ? 1 : near.left ? 0 : 0.5;
    const rx = near.left || near.right ? 1 : 0.5;
    fade = `<radialGradient id='f' cx='${cx}' cy='0.5' r='1' gradientTransform='translate(${cx} 0.5) scale(${rx} 0.5) translate(${-cx} -0.5)'>${fadeStops}</radialGradient>`;
  }
  const svg =
    `<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100' preserveAspectRatio='none'>` +
    `<defs><linearGradient id='c' x1='0' y1='0' x2='1' y2='0'>${stops.join('')}</linearGradient>${fade}` +
    `<mask id='m'><rect width='100' height='100' fill='url(#f)'/></mask></defs>` +
    `<rect width='100' height='100' fill='url(#c)' mask='url(#m)'/></svg>`;
  return `url("data:image/svg+xml,${encodeURIComponent(svg)}")`;
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
