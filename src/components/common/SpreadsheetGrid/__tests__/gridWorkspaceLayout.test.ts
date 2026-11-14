/**
 * Pure layout helpers behind the workspace props: the header drop plan with a
 * column-order constraint, the span-run resolver, and the first-visible-row
 * index behind `headerCorner`.
 */
import { describe, expect, it } from 'vitest';
import {
  dropBoundaryAt,
  movedBlockEdges,
  movedColumnOrder,
  normalizeColumnOrder,
  planColumnDrop,
} from '../gridColumnMove';
import { spanRangeResolver } from '../GridSpanLayer';
import { firstVisibleRowAt } from '../useFirstVisibleRow';
import { columnOffsets } from '../gridGeometry';

const KEYS = ['a', 'b', 'c', 'd'];
const WIDTHS = [100, 80, 60, 50];
const OFFSETS = columnOffsets(WIDTHS); // 0, 100, 180, 240, 290
const G = 48;

/** Keep the [b, c] group contiguous: wherever one of them lands, the other
 *  joins it (the first group member in the proposed order anchors the group). */
const keepBC = (order: string[]): string[] => {
  const at = order.findIndex((k) => k === 'b' || k === 'c');
  const rest = order.filter((k) => k !== 'b' && k !== 'c');
  const before = order.slice(0, at).filter((k) => k !== 'b' && k !== 'c').length;
  const group = order.filter((k) => k === 'b' || k === 'c');
  return [...rest.slice(0, before), ...group, ...rest.slice(before)];
};
/** `a` is pinned first. */
const pinA = (order: string[]) => ['a', ...order.filter((k) => k !== 'a')];

const plan = (fromC0: number, fromC1: number, dropIdx: number, constrain?: (o: string[]) => string[]) =>
  planColumnDrop({ colKeys: KEYS, orderedKeys: KEYS, fromC0, fromC1, dropIdx, offsets: OFFSETS, gutterWidth: G, constrain });

describe('dropBoundaryAt', () => {
  it('flips to "after" a column past its midpoint', () => {
    expect(dropBoundaryAt(0, OFFSETS, WIDTHS)).toBe(0);
    expect(dropBoundaryAt(50, OFFSETS, WIDTHS)).toBe(0);
    expect(dropBoundaryAt(51, OFFSETS, WIDTHS)).toBe(1);
    expect(dropBoundaryAt(141, OFFSETS, WIDTHS)).toBe(2);
    expect(dropBoundaryAt(1000, OFFSETS, WIDTHS)).toBe(4);
  });
});

describe('movedColumnOrder / normalizeColumnOrder', () => {
  it('moves a block before the anchor and keeps a hidden column in its slot', () => {
    // x is hidden between b and c.
    const ordered = ['a', 'b', 'x', 'c', 'd'];
    expect(movedColumnOrder(KEYS, ordered, 0, 0, 3)).toEqual(['b', 'x', 'c', 'a', 'd']);
    expect(movedColumnOrder(KEYS, ordered, 3, 3, 0)).toEqual(['d', 'a', 'b', 'x', 'c']);
    expect(movedColumnOrder(KEYS, ordered, 0, 1, 4)).toEqual(['x', 'c', 'd', 'a', 'b']);
  });

  it('turns a constraint answer into a permutation of the current keys', () => {
    expect(normalizeColumnOrder(['d', 'zz', 'a', 'd'], KEYS)).toEqual(['d', 'a', 'b', 'c']);
    expect(normalizeColumnOrder([], KEYS)).toEqual(KEYS);
  });
});

describe('planColumnDrop', () => {
  it('without a constraint: the line at the pointer boundary, order applied even when unchanged', () => {
    expect(plan(0, 0, 3)).toEqual({ order: ['b', 'c', 'a', 'd'], lineX: G + 240 });
    expect(plan(1, 1, 1)).toEqual({ order: KEYS, lineX: G + 100 });
  });

  it('a drop into a forbidden spot snaps: the line shows where the column actually lands', () => {
    // Drag d before everything: pinA puts it right after a, i.e. before b.
    expect(plan(3, 3, 0, pinA)).toEqual({ order: ['a', 'd', 'b', 'c'], lineX: G + OFFSETS[1]! });
    // Drag a between c and d: the group [b, c] must stay together and a can
    // move freely, so it lands as asked.
    expect(plan(0, 0, 3, keepBC)).toEqual({ order: ['b', 'c', 'a', 'd'], lineX: G + OFFSETS[3]! });
    // Drag b to the far right: the group re-forms where c stands, so b lands
    // between c and d. The line is before d, not at the pointer (far right).
    expect(plan(1, 1, 4, keepBC)).toEqual({ order: ['a', 'c', 'b', 'd'], lineX: G + OFFSETS[3]! });
    // Without the constraint the same drop is drawn at the far right edge.
    expect(plan(1, 1, 4).lineX).toBe(G + OFFSETS[4]!);
    // Drag d between b and c (splitting the group): d is put after the group.
    expect(plan(3, 3, 2, keepBC)).toEqual({ order: null, lineX: null });
    // Drag d before b: allowed, the group stays whole.
    expect(plan(3, 3, 1, keepBC)).toEqual({ order: ['a', 'd', 'b', 'c'], lineX: G + OFFSETS[1]! });
  });

  it('a drop the constraint turns back into the current order is a no-op with no line', () => {
    expect(plan(0, 0, 3, pinA)).toEqual({ order: null, lineX: null });
    expect(plan(0, 0, 0, (o) => o)).toEqual({ order: null, lineX: null });
  });

  it('a constraint answer with stray or missing keys is normalized', () => {
    expect(plan(3, 3, 0, () => ['d', 'nope', 'a'])).toEqual({ order: ['d', 'a', 'b', 'c'], lineX: G + 0 });
  });
});

describe('movedBlockEdges', () => {
  it('names the block by its leftmost and rightmost keys in the new order', () => {
    expect(movedBlockEdges(['a', 'd', 'b', 'c'], KEYS, 1, 2)).toEqual({ first: 'b', last: 'c' });
    expect(movedBlockEdges(['c', 'b', 'a', 'd'], KEYS, 1, 2)).toEqual({ first: 'c', last: 'b' });
    expect(movedBlockEdges(KEYS, [], 0, 0)).toBeNull();
  });
});

describe('spanRangeResolver', () => {
  it('resolves visible keys to their indexes, swapping reversed ends', () => {
    const r = spanRangeResolver(KEYS, KEYS);
    expect(r('b', 'c')).toEqual({ c0: 1, c1: 2 });
    expect(r('c', 'c')).toEqual({ c0: 2, c1: 2 });
    expect(r('d', 'a')).toEqual({ c0: 0, c1: 3 });
  });

  it('clamps hidden ends inward to the visible run', () => {
    // b and d hidden.
    const r = spanRangeResolver(KEYS, ['a', 'c']);
    expect(r('b', 'c')).toEqual({ c0: 1, c1: 1 });
    expect(r('a', 'b')).toEqual({ c0: 0, c1: 0 });
    expect(r('b', 'd')).toEqual({ c0: 1, c1: 1 });
    expect(r('a', 'd')).toEqual({ c0: 0, c1: 1 });
  });

  it('resolves to nothing when no visible column lies in the run, or a key is unknown', () => {
    const r = spanRangeResolver(KEYS, ['a', 'd']);
    expect(r('b', 'c')).toBeNull();
    expect(r('b', 'b')).toBeNull();
    expect(r('a', 'zz')).toBeNull();
    expect(spanRangeResolver(KEYS, [])('a', 'd')).toBeNull();
  });

  it('follows the display order', () => {
    const order = ['c', 'a', 'b', 'd'];
    const r = spanRangeResolver(order, order);
    expect(r('a', 'b')).toEqual({ c0: 1, c1: 2 });
    expect(r('c', 'a')).toEqual({ c0: 0, c1: 1 });
  });
});

describe('firstVisibleRowAt', () => {
  it('is the first row whose top is at or below the header bottom', () => {
    expect(firstVisibleRowAt(0, 30)).toBe(0);
    expect(firstVisibleRowAt(1, 30)).toBe(1);
    expect(firstVisibleRowAt(33, 30)).toBe(1);
    expect(firstVisibleRowAt(34, 30)).toBe(2);
    expect(firstVisibleRowAt(165.2, 30)).toBe(5); // sub-pixel tolerance
    expect(firstVisibleRowAt(10_000, 30)).toBe(29);
    expect(firstVisibleRowAt(0, 0)).toBe(-1);
  });
});
