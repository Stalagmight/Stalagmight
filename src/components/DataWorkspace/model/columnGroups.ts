/**
 * The Data grid's column groups: the shot-only columns (`appliesTo: 'shot'`)
 * form ONE contiguous block. Every other column - station-only (name, depth,
 * position) and both-kinds (type, section, comment) - sits before or after
 * it, never inside. A station row has no value in any shot-only column, so
 * its link chips span the block's shown part.
 *
 * `constrainDataColumnOrder` keeps an order that way. It is pure and applied
 * to every order the workspace takes: a header drag (the grid's
 * `constrainColumnOrder`, which also places the drop indicator), the
 * persisted order on load, and the Columns menu's reset.
 */
import { DATA_COLUMNS, DATA_COLUMN_BY_KEY } from '../columnsMeta';
import { orderColumns } from './search';
import type { DataColumnKey } from '../types';

export function isShotOnlyColumn(key: string): boolean {
  return DATA_COLUMN_BY_KEY.get(key as DataColumnKey)?.appliesTo === 'shot';
}

/**
 * The first and last SHOWN shot-only column in display order (what a
 * station's link chips span), or null when none is shown.
 */
export function shotBlockSpan(
  orderedKeys: readonly string[],
  hidden: ReadonlySet<string>,
): { from: DataColumnKey; to: DataColumnKey } | null {
  let from: string | null = null;
  let to: string | null = null;
  for (const key of orderedKeys) {
    if (hidden.has(key) || !isShotOnlyColumn(key)) continue;
    from ??= key;
    to = key;
  }
  return from !== null && to !== null ? { from: from as DataColumnKey, to: to as DataColumnKey } : null;
}

/**
 * Keep the shot-only columns of `order` (hidden ones included) in one block:
 *
 * - The block anchors on the longest run of adjacent shot-only columns (the
 *   first such run on a tie); other shot-only columns join it in their
 *   relative order. So a shot-only column dragged out of the block goes back
 *   to the block edge nearest where it was dropped (the side it lies on).
 * - A column that is not shot-only lying between the block's shot-only
 *   columns snaps to the nearest block edge: before the block when it lies
 *   before the anchor run, after it otherwise. A single one dropped inside
 *   the block splits it in two; the longer part anchors, so it lands on the
 *   edge it was nearest to.
 * - Every other relative order is kept. An order already in shape comes back
 *   as it is (the same array), so applying it twice changes nothing.
 *
 * Unknown keys are treated as not shot-only (they stay where they are).
 */
export function constrainDataColumnOrder<K extends string>(order: readonly K[]): K[] {
  const shot = order.map(isShotOnlyColumn);
  // The anchor: the longest run of adjacent shot-only columns.
  let best = -1;
  let bestLen = 0;
  for (let i = 0; i < order.length; ) {
    if (!shot[i]) {
      i++;
      continue;
    }
    let j = i;
    while (j < order.length && shot[j]) j++;
    if (j - i > bestLen) {
      best = i;
      bestLen = j - i;
    }
    i = j;
  }
  const shotCount = shot.filter(Boolean).length;
  if (bestLen === shotCount) return order as K[];
  const before: K[] = [];
  const block: K[] = [];
  const after: K[] = [];
  order.forEach((key, i) => {
    if (shot[i]) block.push(key);
    else if (i < best) before.push(key);
    else after.push(key);
  });
  return [...before, ...block, ...after];
}

const DEFAULT_ORDER: readonly DataColumnKey[] = DATA_COLUMNS.map((c) => c.key);

/**
 * A persisted / held column order brought in line: completed with the columns
 * it does not name (where `orderColumns` shows them: after it, in default
 * order), then constrained. The default order comes back as `[]`, the
 * store's "default" - so a view that only ever differed by an illegal order
 * reads as untouched.
 */
export function normalizeColumnOrder(order: readonly DataColumnKey[]): DataColumnKey[] {
  if (order.length === 0) return [];
  const full = orderColumns(DATA_COLUMNS, order).map((c) => c.key);
  const constrained = constrainDataColumnOrder(full);
  return constrained.every((key, i) => key === DEFAULT_ORDER[i]) ? [] : constrained;
}
