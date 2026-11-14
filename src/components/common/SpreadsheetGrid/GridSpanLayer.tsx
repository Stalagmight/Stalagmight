/**
 * Spanning cells (`spanCells`): one element drawn over a contiguous run of
 * visible columns on a row, e.g. a station's link chips across the shot-only
 * columns it has no value in.
 *
 * The span is decoration over real cells, never a cell itself: it sits above
 * the cells and below the selection overlay (z 2, overlays start at 3), lets
 * the pointer through to the cells under it (`pointer-events: none`) except on
 * its interactive descendants (see `.sg-span` in SpreadsheetGrid.css), and the
 * covered cells keep every cell semantic (N/A, edits, selection, Tab).
 */
import type { ReactNode } from 'react';

/** A span's `from`/`to` keys resolved to visible column indexes, or null when
 *  no visible column lies between them. */
export type SpanRange = (from: string, to: string) => { c0: number; c1: number } | null;

export interface SpanResult {
  from: string;
  to: string;
  content: ReactNode;
}

/**
 * Build the O(1) resolver for one column layout. `orderedKeys` is the full
 * display order (hidden columns included), `colKeys` the visible subsequence.
 * The run covers every visible column whose display position lies between
 * `from` and `to` inclusive: a hidden end clamps inward to the nearest visible
 * column, reversed ends are swapped, an unknown key resolves to nothing.
 */
export function spanRangeResolver(orderedKeys: readonly string[], colKeys: readonly string[]): SpanRange {
  const pos = new Map(orderedKeys.map((k, i) => [k, i]));
  const n = orderedKeys.length;
  // atOrAfter[p] / atOrBefore[p]: visible index of the nearest visible column
  // at display position >= p / <= p, -1 when there is none.
  const atOrAfter = new Array<number>(n).fill(-1);
  const atOrBefore = new Array<number>(n).fill(-1);
  const visIdx = new Map(colKeys.map((k, i) => [k, i]));
  let last = -1;
  let next = -1;
  for (let p = 0; p < n; p++) {
    last = visIdx.get(orderedKeys[p] ?? '') ?? last;
    atOrBefore[p] = last;
    const q = n - 1 - p;
    next = visIdx.get(orderedKeys[q] ?? '') ?? next;
    atOrAfter[q] = next;
  }
  return (from, to) => {
    const a = pos.get(from);
    const b = pos.get(to);
    if (a === undefined || b === undefined) return null;
    const c0 = atOrAfter[Math.min(a, b)] ?? -1;
    const c1 = atOrBefore[Math.max(a, b)] ?? -1;
    return c0 < 0 || c1 < 0 || c0 > c1 ? null : { c0, c1 };
  };
}

/** The span element for one row: left at the gutter-relative x of the first
 *  covered column, as wide as the covered columns, content clipped to it. */
export function GridSpan({
  c0,
  c1,
  offsets,
  gutterWidth,
  testId,
  children,
}: {
  c0: number;
  c1: number;
  offsets: readonly number[];
  gutterWidth: number;
  testId: string;
  children: ReactNode;
}) {
  const left = offsets[c0] ?? 0;
  return (
    <div
      className="sg-span"
      style={{
        position: 'absolute',
        top: 0,
        left: gutterWidth + left,
        width: (offsets[c1 + 1] ?? left) - left,
        height: '100%',
        zIndex: 2,
        pointerEvents: 'none',
        overflow: 'hidden',
      }}
      data-testid={testId}
      data-span-cols={`${c0},${c1}`}
    >
      {children}
    </div>
  );
}
