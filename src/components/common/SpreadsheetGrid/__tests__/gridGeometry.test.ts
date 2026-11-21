import { describe, it, expect } from 'vitest';
import {
  clamp,
  selectionRect,
  isInRect,
  rectIsSingleCell,
  columnOffsets,
  arrowTarget,
  tabTarget,
  placePaste,
  fillTargets,
  cycleActiveInRect,
  type Rect,
  collapseAfterRowDelete,
  reconcileSelection,
} from '../gridGeometry';

const cols = ['id', 'a', 'b', 'c'];
const colIndexOf = (k: string) => cols.indexOf(k);

describe('clamp', () => {
  it('clamps to range', () => {
    expect(clamp(-1, 0, 5)).toBe(0);
    expect(clamp(9, 0, 5)).toBe(5);
    expect(clamp(3, 0, 5)).toBe(3);
  });
});

describe('selectionRect', () => {
  it('normalizes anchor/focus regardless of order', () => {
    const sel = {
      anchor: { row: 5, colKey: 'c' },
      focus: { row: 2, colKey: 'a' },
      active: { row: 5, colKey: 'c' },
    };
    expect(selectionRect(sel, colIndexOf)).toEqual({ r0: 2, r1: 5, c0: 1, c1: 3 });
  });
  it('single cell', () => {
    const sel = {
      anchor: { row: 1, colKey: 'b' },
      focus: { row: 1, colKey: 'b' },
      active: { row: 1, colKey: 'b' },
    };
    const rect = selectionRect(sel, colIndexOf);
    expect(rect).toEqual({ r0: 1, r1: 1, c0: 2, c1: 2 });
    expect(rectIsSingleCell(rect)).toBe(true);
  });
});

describe('isInRect', () => {
  const rect: Rect = { r0: 1, r1: 3, c0: 1, c1: 2 };
  it('includes interior + edges, excludes outside', () => {
    expect(isInRect(rect, 2, 1)).toBe(true);
    expect(isInRect(rect, 1, 2)).toBe(true);
    expect(isInRect(rect, 0, 1)).toBe(false);
    expect(isInRect(rect, 2, 3)).toBe(false);
  });
});

describe('columnOffsets', () => {
  it('prefix-sums widths with a leading 0 and trailing total', () => {
    expect(columnOffsets([100, 120, 80])).toEqual([0, 100, 220, 300]);
  });
});

describe('arrowTarget (no wrap, clamped)', () => {
  it('moves one cell', () => {
    expect(arrowTarget(2, 1, 1, 0, 10, 4)).toEqual({ row: 3, colIdx: 1 });
    expect(arrowTarget(2, 1, 0, 1, 10, 4)).toEqual({ row: 2, colIdx: 2 });
  });
  it('does not wrap at edges', () => {
    expect(arrowTarget(0, 0, -1, -1, 10, 4)).toEqual({ row: 0, colIdx: 0 });
    expect(arrowTarget(9, 3, 1, 1, 10, 4)).toEqual({ row: 9, colIdx: 3 });
  });
});

describe('tabTarget (skip read-only, wrap rows, clamp ends)', () => {
  // writable columns are indices 1 and 3 (0 and 2 are read-only)
  const writable = [1, 3];
  it('moves to next writable column in the row', () => {
    expect(tabTarget(2, 1, 1, writable, 10)).toEqual({ row: 2, colIdx: 3 });
  });
  it('wraps to next row first writable when past the last writable', () => {
    expect(tabTarget(2, 3, 1, writable, 10)).toEqual({ row: 3, colIdx: 1 });
  });
  it('shift-tab moves to previous writable / wraps to prev row last writable', () => {
    expect(tabTarget(2, 3, -1, writable, 10)).toEqual({ row: 2, colIdx: 1 });
    expect(tabTarget(2, 1, -1, writable, 10)).toEqual({ row: 1, colIdx: 3 });
  });
  it('clamps at the grid extremes', () => {
    expect(tabTarget(9, 3, 1, writable, 10)).toEqual({ row: 9, colIdx: 3 });
    expect(tabTarget(0, 1, -1, writable, 10)).toEqual({ row: 0, colIdx: 1 });
  });
  it('returns null when nothing is writable', () => {
    expect(tabTarget(0, 0, 1, [], 10)).toBeNull();
  });
});

describe('placePaste', () => {
  const rectAt = (r0: number, c0: number, r1 = r0, c1 = c0): Rect => ({ r0, r1, c0, c1 });

  it('1×1 clipboard fills the whole selection', () => {
    const { targets, clipped } = placePaste([['X']], rectAt(0, 0, 1, 1), 10, 4);
    expect(clipped).toBe(0);
    expect(targets).toHaveLength(4);
    expect(targets.every((t) => t.raw === 'X')).toBe(true);
  });

  it('pastes an R×C block anchored at the selection top-left (auto-grow)', () => {
    const clip = [['a', 'b'], ['c', 'd']];
    const { targets } = placePaste(clip, rectAt(2, 1), 10, 4); // single active cell
    expect(targets).toContainEqual({ row: 2, colIdx: 1, raw: 'a' });
    expect(targets).toContainEqual({ row: 2, colIdx: 2, raw: 'b' });
    expect(targets).toContainEqual({ row: 3, colIdx: 1, raw: 'c' });
    expect(targets).toContainEqual({ row: 3, colIdx: 2, raw: 'd' });
  });

  it('tiles when the selection is an exact integer multiple of the clipboard', () => {
    const clip = [['a', 'b']]; // 1×2
    const { targets } = placePaste(clip, rectAt(0, 0, 0, 3), 10, 4); // 1×4 selection
    expect(targets.map((t) => t.raw)).toEqual(['a', 'b', 'a', 'b']);
  });

  it('clips cells past the last row / last column (never grows the grid)', () => {
    const clip = [['a', 'b'], ['c', 'd']];
    const { targets, clipped } = placePaste(clip, rectAt(9, 3), 10, 4); // bottom-right corner
    expect(targets).toEqual([{ row: 9, colIdx: 3, raw: 'a' }]);
    expect(clipped).toBe(3);
  });

  it('emits rows past the end under growRows, still clipping columns', () => {
    // A host whose rows are provisional (the new-section dialog) grows to fit,
    // so a block pasted into a one-row grid must not be truncated to that row.
    const clip = [['a', 'b'], ['c', 'd']];
    const { targets, clipped } = placePaste(clip, rectAt(0, 0), 1, 2, true);
    expect(targets).toEqual([
      { row: 0, colIdx: 0, raw: 'a' },
      { row: 0, colIdx: 1, raw: 'b' },
      { row: 1, colIdx: 0, raw: 'c' },
      { row: 1, colIdx: 1, raw: 'd' },
    ]);
    expect(clipped).toBe(0);

    // The column set is fixed either way, so an overhanging column still clips.
    const narrow = placePaste(clip, rectAt(0, 1), 1, 2, true);
    expect(narrow.targets).toEqual([
      { row: 0, colIdx: 1, raw: 'a' },
      { row: 1, colIdx: 1, raw: 'c' },
    ]);
    expect(narrow.clipped).toBe(2);
  });

  describe('rowTakes: rows no target column applies to (an external paste)', () => {
    // Shots on the odd rows, stations on the even ones; column 0 is shot-only,
    // column 1 applies to both kinds.
    const takes = (r: number, c0: number, c1: number) => r % 2 === 1 || (c0 <= 1 && c1 >= 1);

    it('passes over a gap row: each line lands on the next row that takes it', () => {
      const { targets, clipped } = placePaste([['3'], ['4'], ['5']], rectAt(1, 0), 10, 2, false, takes);
      expect(targets).toEqual([
        { row: 1, colIdx: 0, raw: '3' },
        { row: 3, colIdx: 0, raw: '4' },
        { row: 5, colIdx: 0, raw: '5' },
      ]);
      expect(clipped).toBe(0);
    });

    it('keeps a row where SOME target column applies (positional within the row)', () => {
      const both = placePaste([['a', 'x'], ['b', 'y']], rectAt(1, 0), 10, 2, false, takes);
      expect(both.targets.map((t) => [t.row, t.colIdx, t.raw])).toEqual([
        [1, 0, 'a'], [1, 1, 'x'], [2, 0, 'b'], [2, 1, 'y'],
      ]);
      // A column every row takes stays one line per row.
      const comment = placePaste([['a'], ['b'], ['c']], rectAt(1, 1), 10, 2, false, takes);
      expect(comment.targets.map((t) => t.row)).toEqual([1, 2, 3]);
    });

    it('clips the lines left over at the bottom, gap rows not counted', () => {
      const { targets, clipped } = placePaste([['3'], ['4'], ['5']], rectAt(5, 0), 8, 2, false, takes);
      expect(targets.map((t) => t.row)).toEqual([5, 7]);
      expect(clipped).toBe(1);
    });

    it('fills and tiles over the selection rows that take a column', () => {
      const fill = placePaste([['9']], rectAt(0, 0, 4, 0), 10, 2, false, takes);
      expect(fill.targets.map((t) => t.row)).toEqual([1, 3]);
      // 4 shot rows in 0..8 hold the 2-line clipboard twice; the 9-row
      // selection alone is no multiple of 2.
      const tile = placePaste([['1'], ['2']], rectAt(0, 0, 8, 0), 10, 2, false, takes);
      expect(tile.targets.map((t) => [t.row, t.raw])).toEqual([[1, '1'], [3, '2'], [5, '1'], [7, '2']]);
    });

    it('never asks about rows past the end of a growing paste', () => {
      const asked: number[] = [];
      const spy = (r: number) => (asked.push(r), r % 2 === 1);
      const { targets } = placePaste([['a'], ['b'], ['c']], rectAt(1, 0), 3, 1, true, spy);
      expect(targets.map((t) => t.row)).toEqual([1, 3, 4]);
      expect(Math.max(...asked)).toBeLessThan(3);
    });
  });
});

describe('fillTargets', () => {
  it('fill-down copies the top row into the rows below', () => {
    const out = fillTargets({ r0: 0, r1: 2, c0: 1, c1: 1 }, true);
    expect(out).toEqual([
      { row: 1, colIdx: 1, srcRow: 0, srcColIdx: 1 },
      { row: 2, colIdx: 1, srcRow: 0, srcColIdx: 1 },
    ]);
  });
  it('fill-right copies the left column into the columns to the right', () => {
    const out = fillTargets({ r0: 0, r1: 0, c0: 1, c1: 3 }, false);
    expect(out).toEqual([
      { row: 0, colIdx: 2, srcRow: 0, srcColIdx: 1 },
      { row: 0, colIdx: 3, srcRow: 0, srcColIdx: 1 },
    ]);
  });
});

describe('cycleActiveInRect (Tab/Enter within a selection)', () => {
  const rect: Rect = { r0: 1, r1: 2, c0: 1, c1: 2 }; // 2x2 block

  it('Tab order is row-major and wraps around the whole rectangle', () => {
    // (1,1) -> (1,2) -> (2,1) -> (2,2) -> back to (1,1)
    expect(cycleActiveInRect(rect, 1, 1, true, false)).toEqual({ row: 1, colIdx: 2 });
    expect(cycleActiveInRect(rect, 1, 2, true, false)).toEqual({ row: 2, colIdx: 1 });
    expect(cycleActiveInRect(rect, 2, 2, true, false)).toEqual({ row: 1, colIdx: 1 });
  });

  it('Shift+Tab reverses', () => {
    expect(cycleActiveInRect(rect, 1, 1, true, true)).toEqual({ row: 2, colIdx: 2 });
  });

  it('Enter order is column-major and wraps', () => {
    // (1,1) -> (2,1) -> (1,2) -> (2,2) -> back to (1,1)
    expect(cycleActiveInRect(rect, 1, 1, false, false)).toEqual({ row: 2, colIdx: 1 });
    expect(cycleActiveInRect(rect, 2, 1, false, false)).toEqual({ row: 1, colIdx: 2 });
    expect(cycleActiveInRect(rect, 2, 2, false, false)).toEqual({ row: 1, colIdx: 1 });
  });
});

describe('collapseAfterRowDelete', () => {
  const prev = { active: { colKey: 'length' } };

  it('lands just above the deleted run, keeping the active column', () => {
    expect(collapseAfterRowDelete(prev, 5)).toEqual({
      anchor: { row: 4, colKey: 'length' },
      focus: { row: 4, colKey: 'length' },
      active: { row: 4, colKey: 'length' },
    });
  });

  it('stays at the top when the run started there', () => {
    expect(collapseAfterRowDelete(prev, 0).active.row).toBe(0);
  });

  it('always collapses to a single cell', () => {
    const s = collapseAfterRowDelete(prev, 9);
    expect(s.anchor).toEqual(s.focus);
    expect(s.focus).toEqual(s.active);
  });
});

describe('reconcileSelection', () => {
  const sel = (row: number, colKey: string) => ({
    anchor: { row, colKey },
    focus: { row, colKey },
    active: { row, colKey },
  });

  it('returns the same object when nothing had to move', () => {
    const prev = sel(1, 'a');
    expect(reconcileSelection(prev, 5, ['a', 'b'])).toBe(prev);
  });

  it('clamps a row past the end of a shrunken grid', () => {
    expect(reconcileSelection(sel(9, 'a'), 3, ['a']).active.row).toBe(2);
  });

  it('re-maps a column key that no longer exists', () => {
    expect(reconcileSelection(sel(0, 'gone'), 3, ['a', 'b']).active.colKey).toBe('a');
  });

  it('clamps to row 0 when every row is gone', () => {
    expect(reconcileSelection(sel(4, 'a'), 0, ['a']).active.row).toBe(0);
  });
});
