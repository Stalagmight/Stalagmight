/**
 * The shot-only columns stay one block: what a header drag, a loaded order and
 * the Columns menu's reset all go through (constrainDataColumnOrder), the
 * persisted-order normaliser, and the span a station's link chips take.
 */
import { describe, expect, it } from 'vitest';
import { DATA_COLUMNS } from '../../columnsMeta';
import { constrainDataColumnOrder, isShotOnlyColumn, normalizeColumnOrder, shotBlockSpan } from '../columnGroups';
import type { DataColumnKey } from '../../types';

const DEFAULT: DataColumnKey[] = DATA_COLUMNS.map((c) => c.key);
const SHOT = DEFAULT.filter(isShotOnlyColumn);

/** The order a header drag of `keys` (adjacent) produces, dropped before `before` (null: at the end). */
function drag(order: readonly DataColumnKey[], keys: readonly DataColumnKey[], before: DataColumnKey | null): DataColumnKey[] {
  const rest = order.filter((k) => !keys.includes(k));
  const at = before === null ? rest.length : rest.indexOf(before);
  return [...rest.slice(0, at), ...keys, ...rest.slice(at)];
}

/** The shot-only columns of `order` form one run. */
function contiguous(order: readonly string[]): boolean {
  const idx = order.map((k, i) => (isShotOnlyColumn(k) ? i : -1)).filter((i) => i >= 0);
  return idx.length === 0 || idx[idx.length - 1]! - idx[0]! === idx.length - 1;
}

const sub = (order: readonly string[], pred: (k: string) => boolean) => order.filter(pred);

describe('constrainDataColumnOrder', () => {
  it('the default order is in shape: comes back as it is', () => {
    expect(contiguous(DEFAULT)).toBe(true);
    expect(constrainDataColumnOrder(DEFAULT)).toBe(DEFAULT);
    // The shot-only block: From → To ... Date; Depth and Comment after it.
    expect(SHOT[0]).toBe('fromTo');
    expect(SHOT[SHOT.length - 1]).toBe('date');
    expect(DEFAULT.indexOf('depth')).toBe(DEFAULT.indexOf('date') + 1);
    expect(DEFAULT.indexOf('comment')).toBe(DEFAULT.indexOf('depth') + 1);
  });

  it('a shot-only column dragged out past the end goes back to the end edge', () => {
    const out = drag(DEFAULT, ['length'], null);
    expect(contiguous(out)).toBe(false);
    const fixed = constrainDataColumnOrder(out);
    expect(contiguous(fixed)).toBe(true);
    expect(fixed.indexOf('length')).toBe(fixed.indexOf('date') + 1);
    expect(fixed.indexOf('depth')).toBe(fixed.indexOf('length') + 1);
  });

  it('a shot-only column dragged out to the far left goes back to the start edge', () => {
    const fixed = constrainDataColumnOrder(drag(DEFAULT, ['kind'], 'type'));
    expect(fixed.slice(0, 5)).toEqual(['type', 'section', 'name', 'kind', 'fromTo']);
  });

  it('a shot-only column dragged one past its block neighbour comes straight back', () => {
    // Date dropped after Depth (right of the block's end).
    const fixed = constrainDataColumnOrder(drag(DEFAULT, ['date'], 'comment'));
    expect(fixed).toEqual(DEFAULT);
  });

  it('a comment dropped inside the block snaps to the nearest edge', () => {
    const nearStart = constrainDataColumnOrder(drag(DEFAULT, ['comment'], 'length'));
    expect(nearStart.slice(0, 5)).toEqual(['type', 'section', 'name', 'comment', 'fromTo']);
    const nearEnd = constrainDataColumnOrder(drag(DEFAULT, ['comment'], 'date'));
    expect(nearEnd.indexOf('comment')).toBe(nearEnd.indexOf('date') + 1);
    expect(contiguous(nearStart) && contiguous(nearEnd)).toBe(true);
  });

  it('a station-only or both-kinds column dropped inside snaps out too', () => {
    for (const key of ['depth', 'x', 'name', 'type', 'section'] as DataColumnKey[]) {
      const fixed = constrainDataColumnOrder(drag(DEFAULT, [key], 'down'));
      expect(contiguous(fixed), key).toBe(true);
      // Down sits past the middle of the block: the end edge.
      expect(fixed.indexOf(key), key).toBeGreaterThan(fixed.indexOf('date'));
    }
  });

  it('counts hidden columns too: a hidden one inside the block moves out', () => {
    // The grid passes every column, hidden ones in their slots; X is hidden by
    // default, but would come back inside the block when shown.
    const withX = drag(DEFAULT, ['x'], 'azimuth');
    const fixed = constrainDataColumnOrder(withX);
    expect(contiguous(fixed)).toBe(true);
    expect(fixed.indexOf('x')).toBeLessThan(fixed.indexOf('fromTo'));
  });

  it('a block of several columns dragged out rejoins at the nearest edge', () => {
    const fixed = constrainDataColumnOrder(drag(DEFAULT, ['left', 'right'], null));
    expect(contiguous(fixed)).toBe(true);
    expect(fixed.slice(fixed.indexOf('date') + 1, fixed.indexOf('date') + 3)).toEqual(['left', 'right']);
  });

  it('a mixed drag of shot and other columns comes out in shape', () => {
    const fixed = constrainDataColumnOrder(drag(DEFAULT, ['kind', 'temperature', 'surveyors'], 'type'));
    expect(contiguous(fixed)).toBe(true);
  });

  it('keeps every key once and the relative order within each group, and is idempotent', () => {
    // A seeded shuffle: deterministic, and exercises orders no drag makes.
    let seed = 7;
    const rand = () => ((seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648);
    for (let n = 0; n < 200; n++) {
      const order = [...DEFAULT];
      for (let i = order.length - 1; i > 0; i--) {
        const j = Math.floor(rand() * (i + 1));
        [order[i], order[j]] = [order[j]!, order[i]!];
      }
      const once = constrainDataColumnOrder(order);
      expect(contiguous(once)).toBe(true);
      expect([...once].sort()).toEqual([...order].sort());
      expect(sub(once, isShotOnlyColumn)).toEqual(sub(order, isShotOnlyColumn));
      expect(sub(once, (k) => !isShotOnlyColumn(k))).toEqual(sub(order, (k) => !isShotOnlyColumn(k)));
      expect(constrainDataColumnOrder(once)).toEqual(once);
    }
  });

  it('leaves unknown keys where they are and handles no shot column at all', () => {
    expect(constrainDataColumnOrder(['type', 'mystery', 'comment'])).toEqual(['type', 'mystery', 'comment']);
    expect(constrainDataColumnOrder([])).toEqual([]);
  });
});

describe('normalizeColumnOrder', () => {
  it('keeps "default" as [] and maps the default order to it', () => {
    expect(normalizeColumnOrder([])).toEqual([]);
    expect(normalizeColumnOrder(DEFAULT)).toEqual([]);
  });

  it('completes a partial order the way the grid shows it, then constrains it', () => {
    expect(normalizeColumnOrder(['comment'])).toEqual(['comment', ...DEFAULT.filter((k) => k !== 'comment')]);
    const lengthFirst = normalizeColumnOrder(['length']);
    expect(lengthFirst.slice(0, 5)).toEqual(['type', 'section', 'name', 'length', 'fromTo']);
    expect(contiguous(lengthFirst)).toBe(true);
  });
});

describe('shotBlockSpan', () => {
  it('spans the first to the last shown shot-only column', () => {
    expect(shotBlockSpan(DEFAULT, new Set(['fromTo', 'date']))).toEqual({ from: 'length', to: 'explorers' });
  });

  it('is null when no shot-only column is shown', () => {
    expect(shotBlockSpan(DEFAULT, new Set(SHOT))).toBeNull();
  });
});
