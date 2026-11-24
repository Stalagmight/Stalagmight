/**
 * The glow of a two-colour prose pill, per word box, once laid out
 * (RS-NODOTS; `theme/proseGlow.ts` has the shape).
 *
 * A two-colour shot's glow is one capsule per line whose colour runs from one
 * line's to the other's across the arrow. Its words are boxes of their own
 * (`.sh-ppill-w`, one per word, space or the arrow; none of them wraps
 * inside), so each line's piece of the pill is a run of whole boxes. Once
 * laid out, the first box of each piece paints the whole piece, rounded at
 * both ends, its colour sampled along it: its border box is stretched over
 * the piece and the room for the rounded ends by padding it gives back as a
 * negative margin, so painting never moves a word, and one image per piece
 * leaves no seam between boxes (adjacent images at fractional positions
 * showed a hairline in the first build). The pill's own two ends have their
 * room from the CSS from the start.
 *
 * It measures after a render from outside and when the block around the
 * pill resizes, never after a render its own styles caused: while a panel
 * slides open every measure differs, and measuring that render would loop
 * until React gives up (#185, RS-BLEND).
 */
import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type RefObject } from 'react';
import { SEAM_EM } from '../../theme/pillBlend';
import { GLOW, GLOW_REACH, colourAlong, glowSvg, samplePoints, type PieceSegment } from '../../theme/proseGlow';
import type { ShotTone } from './LinePill';

/** One word box as laid out: its segment, and its words' left and right and its border box's top and bottom, px. */
export interface LaidBox {
  seg: number;
  left: number;
  right: number;
  top: number;
  bottom: number;
}

/** Two boxes on one line (their middles within half a box). */
function sameLine(a: LaidBox, b: LaidBox): boolean {
  const half = Math.min(a.bottom - a.top, b.bottom - b.top) / 2;
  return Math.abs((a.top + a.bottom) / 2 - (b.top + b.bottom) / 2) < half;
}

const px = (v: number) => `${Math.round(v * 100) / 100}px`;

/**
 * Each box's style from the laid-out boxes, the segments' glow colours (a
 * bridge's run to `bridgeTo`) and the text's font size: the first box of
 * each line's piece paints the whole piece (`--w-glow`), its border box
 * stretched over the piece by padding it gives back as a negative margin,
 * so no word moves and no seam shows between boxes; the other boxes paint
 * nothing. A box with no width (a space at a line break) is in no piece.
 */
export function glowStyles(
  boxes: readonly LaidBox[],
  colours: ReadonlyArray<{ colour: string; bridgeTo?: string | undefined }>,
  panel: string,
  fontPx: number,
): CSSProperties[] {
  const reach = GLOW_REACH * fontPx;
  const room = (GLOW.padX + GLOW_REACH) * fontPx;
  // What the CSS gives the pill's own two ends: their room, less what goes back to the line.
  const ends = (GLOW.padX + GLOW.marginX) * fontPx;
  const seam = SEAM_EM * fontPx;
  const styles: CSSProperties[] = boxes.map(() => ({}));
  const shown = boxes.map((b, i) => ({ b, i })).filter(({ b }) => b.right - b.left > 0.5);
  // Pieces: runs of boxes on one line.
  const pieces: Array<typeof shown> = [];
  for (const s of shown) {
    const piece = pieces[pieces.length - 1];
    const prev = piece?.[piece.length - 1];
    if (piece && prev && sameLine(prev.b, s.b)) piece.push(s);
    else pieces.push([s]);
  }
  for (const piece of pieces) {
    // The piece's segments: its runs of boxes of one segment.
    const segs: Array<PieceSegment & { seg: number }> = [];
    for (const { b } of piece) {
      const last = segs[segs.length - 1];
      const c = colours[b.seg] ?? colours[0]!;
      if (last && last.seg === b.seg) last.right = b.right;
      else segs.push({ left: b.left, right: b.right, colour: c.colour, bridgeTo: c.bridgeTo, seg: b.seg });
    }
    const { b: first, i } = piece[0]!;
    const right = piece[piece.length - 1]!.b.right;
    const from = first.left - room;
    const to = right + room;
    const stops = samplePoints(segs, from, to, seam).map((x) => [x - from, colourAlong(segs, x, seam)] as [number, string]);
    const stretch = to - first.right;
    styles[i] = {
      '--w-glow': glowSvg({
        width: to - from,
        height: first.bottom - first.top,
        reach,
        capLeft: true,
        capRight: true,
        spread: GLOW.spread * fontPx,
        sigma: (GLOW.blur / 2) * fontPx,
        stops,
        panel,
      }),
      paddingLeft: px(room),
      marginLeft: px((i === 0 ? ends : 0) - room),
      paddingRight: px(stretch),
      marginRight: px((i === boxes.length - 1 ? ends : 0) - stretch),
    } as CSSProperties;
  }
  return styles;
}

/** The nearest ancestor that is not inline: its size changes when the pill's lines do. */
function blockOf(el: HTMLElement): HTMLElement | null {
  let at = el.parentElement;
  while (at) {
    const display = getComputedStyle(at).display;
    if (!display.startsWith('inline') && display !== 'contents') return at;
    at = at.parentElement;
  }
  return null;
}

/**
 * The styles of the word boxes drawn in `vis` for `tone` (see the module
 * doc), by box order; null until laid out or for a one-colour pill.
 */
export function useProseGlow(vis: RefObject<HTMLElement | null>, tone: ShotTone | null): CSSProperties[] | null {
  const [glow, setGlow] = useState<{ key: string; styles: CSSProperties[] } | null>(null);
  const toneRef = useRef(tone);
  toneRef.current = tone;
  const applied = useRef('');
  const ownRender = useRef(false);
  const measure = useRef(() => {});
  measure.current = () => {
    const el = vis.current;
    const t = toneRef.current;
    if (!el || !t) return;
    const boxes = Array.from(el.querySelectorAll<HTMLElement>('.sh-ppill-w')).map((box) => {
      const r = box.getBoundingClientRect();
      const cs = getComputedStyle(box);
      return {
        seg: Number(box.dataset.seg ?? 0),
        left: r.left + (parseFloat(cs.paddingLeft) || 0),
        right: r.right - (parseFloat(cs.paddingRight) || 0),
        top: r.top,
        bottom: r.bottom,
      };
    });
    if (boxes.length === 0 || boxes.every((b) => b.bottom - b.top <= 0)) return;
    const colours = t.segments.map((s) => ({ colour: s.colors.bg, bridgeTo: s.bridgeTo?.bg }));
    const styles = glowStyles(boxes, colours, t.panel, parseFloat(getComputedStyle(el).fontSize) || 13);
    const key = JSON.stringify(styles);
    if (key === applied.current) return;
    applied.current = key;
    ownRender.current = true;
    setGlow({ key, styles });
  };
  useLayoutEffect(() => {
    if (ownRender.current) {
      ownRender.current = false;
      return;
    }
    measure.current();
  });
  const split = tone !== null;
  useEffect(() => {
    const el = vis.current;
    if (!split || !el || typeof ResizeObserver === 'undefined') return;
    const block = blockOf(el);
    if (!block) return;
    const observer = new ResizeObserver(() => measure.current());
    observer.observe(block);
    void document.fonts?.ready.then(() => measure.current());
    return () => observer.disconnect();
  }, [vis, split]);
  return tone ? (glow?.styles ?? null) : null;
}
