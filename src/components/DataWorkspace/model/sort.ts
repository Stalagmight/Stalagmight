/**
 * Column sort for the Data workspace. Entity rows only (the view drops the
 * section headers while sorted, and the link rail with them).
 *
 * - Numbers compare numerically, text with the locale's collation and numeric
 *   ordering ("A2" before "A10"), case- and accent-insensitive.
 * - Empty / not-applicable values (null, NaN, '') always sort LAST, in either
 *   direction, so flipping the direction never buries the data under blanks.
 * - Ties keep survey order (`row.order`) in either direction: stable.
 * - A column mixing numbers and text puts the numbers first.
 */
import type { DataColumnDef, DataEntityRow, DataModelCtx, SortState } from '../types';

function makeCollator(locale: string): Intl.Collator {
  try {
    return new Intl.Collator(locale, { numeric: true, sensitivity: 'base' });
  } catch {
    return new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' });
  }
}

/** 0 = number, 1 = text, 2 = empty. */
type Rank = 0 | 1 | 2;

export function sortRows(
  rows: readonly DataEntityRow[],
  col: DataColumnDef,
  dir: SortState['dir'],
  ctx: DataModelCtx,
): DataEntityRow[] {
  const n = rows.length;
  const ranks = new Uint8Array(n);
  const nums = new Float64Array(n);
  const strs: string[] = new Array<string>(n);
  for (let i = 0; i < n; i++) {
    const v = col.value(rows[i]!, ctx);
    if (typeof v === 'number') {
      if (Number.isFinite(v)) {
        ranks[i] = 0;
        nums[i] = v;
      } else ranks[i] = 2;
    } else if (typeof v === 'string' && v.trim() !== '') {
      ranks[i] = 1;
      strs[i] = v;
    } else ranks[i] = 2;
  }
  const collator = makeCollator(ctx.locale);
  const sign = dir === 'asc' ? 1 : -1;
  const idx = Array.from({ length: n }, (_, i) => i);
  idx.sort((a, b) => {
    const ra = ranks[a] as Rank;
    const rb = ranks[b] as Rank;
    if (ra !== rb) return ra - rb;
    let c = 0;
    if (ra === 0) c = nums[a]! - nums[b]!;
    else if (ra === 1) c = collator.compare(strs[a]!, strs[b]!);
    if (c !== 0) return sign * c;
    return rows[a]!.order - rows[b]!.order;
  });
  return idx.map((i) => rows[i]!);
}
