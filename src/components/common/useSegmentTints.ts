/**
 * The tints of a two-tone prose pill, per line (RS-BLEND).
 *
 * A prose pill wraps like the words around it, and a segment's tint is one
 * background over every line it spans. A tint that blends into its
 * neighbour is solid at the seam, so on its own it would end in a hard edge
 * wherever a line break falls at the seam or inside the segment ("Upper
 * Branch towards the Dry | Chamber: 12 -> 1" in a narrow panel). Once laid
 * out, each line's fragment of a segment gets its own tint: blended at a
 * seam only where its neighbour is on the same line, fading at every other
 * edge, as a one-colour prose pill's fragments do.
 *
 * The segments use `box-decoration-break: slice`, so their background is
 * laid out over the fragments put end to end; each fragment's tint is one
 * background layer sized and placed (in percent of that strip) over its own
 * fragment. Until laid out (and in tests without layout), the segment's
 * one-line tint stands (`proseSegmentStyle`).
 */
import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type RefObject } from 'react';
import { proseSegmentTint, SEAM_EM } from '../../theme/pillBlend';
import type { PillColors } from '../../theme/pillColors';
import type { SegmentKey, ShotTone } from './LinePill';

/** A fragment's box, as `getClientRects` gives it. */
export interface FragmentBox {
  top: number;
  bottom: number;
  width: number;
}

/** A segment once laid out: its colours and its fragments, one per line, in order. */
export interface SegmentBox {
  colors: PillColors;
  rects: readonly FragmentBox[];
}

/** Two fragments on one line (their middles within half a line). */
function sameLine(a: FragmentBox, b: FragmentBox): boolean {
  const half = Math.min(a.bottom - a.top, b.bottom - b.top) / 2;
  return Math.abs((a.top + a.bottom) / 2 - (b.top + b.bottom) / 2) < half;
}

const pct = (v: number) => `${(v * 100).toFixed(4)}%`;

/**
 * Each segment's tint layers (`--seg-tint`, `--seg-tint-size`,
 * `--seg-tint-pos`): one per fragment, blended toward a neighbour only at a
 * seam on the fragment's own line. `fontPx` sizes the seam fade
 * ({@link SEAM_EM}). Null when a segment has no fragment (not laid out).
 */
export function fragmentTints(segs: readonly SegmentBox[], panel: string, fontPx: number): CSSProperties[] | null {
  if (segs.length === 0 || segs.some((s) => s.rects.length === 0)) return null;
  return segs.map((seg, i) => {
    const prev = segs[i - 1];
    const next = segs[i + 1];
    const total = seg.rects.reduce((sum, r) => sum + r.width, 0);
    const images: string[] = [];
    const sizes: string[] = [];
    const positions: string[] = [];
    let x = 0;
    seg.rects.forEach((r, j) => {
      const first = j === 0;
      const last = j === seg.rects.length - 1;
      const left = first && prev && sameLine(prev.rects[prev.rects.length - 1]!, r) ? prev.colors : null;
      const right = last && next && sameLine(r, next.rects[0]!) ? next.colors : null;
      images.push(proseSegmentTint(seg.colors, { left, right }, panel, (SEAM_EM * fontPx) / Math.max(r.width, 1)));
      sizes.push(`${pct(r.width / total)} 100%`);
      // A percentage places that share of the layer at that share of the
      // strip: x / (strip - layer) puts the layer's left edge at x.
      positions.push(`${total - r.width > 0 ? pct(x / (total - r.width)) : '0%'} 0`);
      x += r.width;
    });
    return {
      '--seg-tint': images.join(', '),
      '--seg-tint-size': sizes.join(', '),
      '--seg-tint-pos': positions.join(', '),
    } as CSSProperties;
  });
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
 * The per-line tints of the segments drawn in `vis` for `tone` (see the
 * module doc), measured after every render and when the block around the
 * pill resizes; null until laid out or for a one-colour pill.
 */
export function useSegmentTints(
  vis: RefObject<HTMLElement | null>,
  tone: ShotTone | null,
): Partial<Record<SegmentKey, CSSProperties>> | null {
  const [tints, setTints] = useState<{ key: string; styles: Partial<Record<SegmentKey, CSSProperties>> } | null>(null);
  const toneRef = useRef(tone);
  toneRef.current = tone;
  const measure = useRef(() => {});
  measure.current = () => {
    const el = vis.current;
    const t = toneRef.current;
    if (!el || !t) return;
    const segs = Array.from(el.querySelectorAll<HTMLElement>(':scope > .sh-ppill-seg'));
    if (segs.length !== t.segments.length) return;
    const boxes = segs.map((seg, i) => ({
      colors: t.segments[i]!.colors,
      rects: Array.from(seg.getClientRects()).filter((r) => r.width > 0.5),
    }));
    const styles = fragmentTints(boxes, t.panel, parseFloat(getComputedStyle(el).fontSize) || 13);
    if (!styles) return;
    const next = Object.fromEntries(t.segments.map((s, i) => [s.key, styles[i]!])) as Partial<Record<SegmentKey, CSSProperties>>;
    const key = JSON.stringify(next);
    setTints((cur) => (cur?.key === key ? cur : { key, styles: next }));
  };
  // After every render: the words, the colours or the wrap may have changed.
  useLayoutEffect(() => measure.current());
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
  return tone ? (tints?.styles ?? null) : null;
}
