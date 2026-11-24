/**
 * The prose glow (RS-NODOTS; the user: "the prose glow is too weak, it should
 * also cover, it dosent spread far enough", "the glow should be much more
 * pill shaped"): a soft-edged capsule behind each name in running text.
 *
 * The capsule's core is solid behind the whole name: the painted box's
 * content area, its font scaled to {@link GLOW}`.scale` so it hugs the
 * glyphs from above the ascenders to below the descenders instead of the
 * font's whole content area (which filled 19 px of a 20 px line), plus a
 * little padding, with rounded ends `padX` beyond the words. A halo in the
 * same colour carries it further: solid for `spread`, then fading as a
 * Gaussian of sigma `blur / 2` (a CSS box-shadow's blur), gone by
 * {@link GLOW_REACH}. All of it paints over the line box without growing it:
 * inline padding, a box-shadow and a background never change the line pitch.
 *
 * - A one-colour pill is CSS alone (`.sh-ppill-vis` in index.css): the
 *   background is the core, the box-shadow the halo, cloned per line where
 *   the pill wraps.
 * - A two-colour shot (RS-OWNERSHIP) is one capsule per line whose colour
 *   runs from one line's to the other's across the arrow. A box-shadow has
 *   one colour, so its words are boxes of their own (`.sh-ppill-w`), each
 *   painting its share of the capsule as an SVG ({@link glowSvg}) once laid
 *   out (`useProseGlow`): rounded only where its line's piece of the pill
 *   starts or ends, its colour sampled from {@link colourAlong}.
 *
 * Every number is in em of the text the glow sits behind; index.css holds
 * the same numbers as custom properties on `.sh-ppill` (a test pins them).
 */
import { mixHex } from './pillBlend';
import { proseOverlayOf } from './pillColors';

export const GLOW = {
  /** The painted box's font size, as a share of the text's: its content area is the core. */
  scale: 0.75,
  /** The core beyond the scaled content area, above and below. */
  padTop: 0.04,
  padBottom: 0.06,
  /** The core beyond the words at each end of a piece. */
  padX: 0.22,
  /** Given back to the line at each end, so a pill takes `padX + marginX` more than its words. */
  marginX: -0.1,
  /** The halo: solid this far beyond the core... */
  spread: 0.14,
  /** ...then a Gaussian fade of sigma `blur / 2`, as a box-shadow's blur radius. */
  blur: 0.28,
} as const;

/** How far beyond the core the halo reaches (its spread and three sigma): the room a painted box keeps for it. */
export const GLOW_REACH = GLOW.spread + 1.5 * GLOW.blur;

/** A segment of a two-colour pill on one line: its colour (a glow colour over the panel) and where it lies. */
export interface PieceSegment {
  /** Left and right of its words, in px along the line. */
  left: number;
  right: number;
  colour: string;
  /** A bridge (the arrow): its colour runs from `colour` to `bridgeTo` across its width. */
  bridgeTo?: string | undefined;
}

/**
 * The glow colour at `x` along one line's piece of a two-colour pill: a
 * segment's own colour; across a bridge, its two colours mixed in OKLab
 * (`mixHex`), the midpoint at its middle; at a seam between two regular
 * segments, the two blending over `seamPx` on each side. Outside the words
 * (the rounded ends), the colour at the nearest end.
 */
export function colourAlong(segs: readonly PieceSegment[], x: number, seamPx: number): string {
  const first = segs[0]!;
  const last = segs[segs.length - 1]!;
  if (x <= first.left) return first.colour;
  if (x >= last.right) return last.bridgeTo ?? last.colour;
  const i = Math.max(
    0,
    segs.findIndex((s) => x < s.right),
  );
  const seg = segs[i]!;
  if (seg.bridgeTo !== undefined) {
    const t = seg.right > seg.left ? (x - seg.left) / (seg.right - seg.left) : 0.5;
    return mixHex(seg.colour, seg.bridgeTo, 1 - Math.min(1, Math.max(0, t)));
  }
  // A seam with a regular neighbour: half way at the boundary, its own colour seamPx in.
  const prev = segs[i - 1];
  const next = segs[i + 1];
  if (prev && prev.bridgeTo === undefined && x - seg.left < seamPx) {
    return mixHex(seg.colour, prev.colour, 0.5 + (0.5 * (x - seg.left)) / seamPx);
  }
  if (next && next.bridgeTo === undefined && seg.right - x < seamPx) {
    return mixHex(seg.colour, next.colour, 0.5 + (0.5 * (seg.right - x)) / seamPx);
  }
  return seg.colour;
}

/**
 * Where to sample a piece's colour (px along the line): its ends, a bridge
 * at its eighths and a seam at its eighths of the blend (the stops
 * `pillBlend` checks the words against, BLEND_SAMPLES and BRIDGE_SAMPLES),
 * so the gradient between them stays on the OKLab path.
 */
export function samplePoints(segs: readonly PieceSegment[], from: number, to: number, seamPx: number): number[] {
  const xs = new Set<number>([from, to]);
  segs.forEach((s, i) => {
    if (s.bridgeTo !== undefined) {
      for (let q = 0; q <= 8; q++) xs.add(s.left + ((s.right - s.left) * q) / 8);
    } else if (i > 0 && segs[i - 1]!.bridgeTo === undefined) {
      for (let q = -4; q <= 4; q++) xs.add(s.left + (q / 4) * seamPx);
    }
  });
  return [...xs].filter((x) => x >= from && x <= to).sort((a, b) => a - b);
}

/** One painted box of a two-colour pill, laid out: its border box and how its piece treats its sides. */
export interface GlowBox {
  /** The border box, px (its width with the side padding {@link glowSvg} is given for). */
  width: number;
  height: number;
  /** The halo's room inside the box on each side, px: the core starts this far in. */
  reach: number;
  /** A rounded end on that side (the piece starts or ends there); else the capsule runs on past the edge. */
  capLeft: boolean;
  capRight: boolean;
  /** The halo's spread and the Gaussian's sigma, px. */
  spread: number;
  sigma: number;
  /** Glow colours over the panel at box-local x (px), in order. */
  stops: ReadonlyArray<[number, string]>;
  /** The panel: the stops become washes over it at the glow's alpha. */
  panel: string;
}

const n = (v: number) => (Math.round(v * 100) / 100).toString();

/**
 * A box's share of a two-colour capsule as an SVG `url()`: the core (its
 * rounded ends where the piece starts or ends, running on past a joined
 * side) and the halo (the core grown by the spread, blurred), as a mask
 * over the colour stops, each a wash at the glow's alpha. The same shape a
 * one-colour pill's background and box-shadow paint, so the two read alike.
 */
export function glowSvg(box: GlowBox): string {
  const { width: w, height: h, reach, spread, sigma } = box;
  const far = w + h + 4 * reach;
  const x0 = box.capLeft ? reach : -far;
  const x1 = box.capRight ? w - reach : w + far;
  const coreH = Math.max(0, h - 2 * reach);
  const r = coreH / 2;
  const rect = (grow: number, fill: string, extra = '') =>
    `<rect x='${n(x0 - grow)}' y='${n(reach - grow)}' width='${n(x1 - x0 + 2 * grow)}' height='${n(coreH + 2 * grow)}' rx='${n(r + grow)}' fill='${fill}'${extra}/>`;
  const stops = box.stops
    .map(([x, c]) => {
      const o = proseOverlayOf(c, box.panel);
      return `<stop offset='${n(w > 0 ? Math.min(1, Math.max(0, x / w)) : 0)}' stop-color='rgb(${o.rgb.join(',')})' stop-opacity='${o.alpha}'/>`;
    })
    .join('');
  const region = `x='${n(-far)}' y='${n(-h)}' width='${n(w + 2 * far)}' height='${n(3 * h)}'`;
  const svg =
    `<svg xmlns='http://www.w3.org/2000/svg' width='${n(w)}' height='${n(h)}' viewBox='0 0 ${n(w)} ${n(h)}'>` +
    `<defs><linearGradient id='c' x1='0' y1='0' x2='1' y2='0'>${stops}</linearGradient>` +
    `<filter id='b' filterUnits='userSpaceOnUse' ${region}><feGaussianBlur stdDeviation='${n(sigma)}'/></filter>` +
    `<mask id='m' maskUnits='userSpaceOnUse' ${region}>${rect(spread, '#fff', " filter='url(#b)'")}${rect(0, '#fff')}</mask></defs>` +
    `<rect width='${n(w)}' height='${n(h)}' fill='url(#c)' mask='url(#m)'/></svg>`;
  return `url("data:image/svg+xml,${encodeURIComponent(svg)}")`;
}
