/**
 * The Data grid's column groups: the shot-only columns (`appliesTo: 'shot'`)
 * form one block. A station row has no value in any of them, so its link
 * chips span the block's shown part.
 */
import { DATA_COLUMN_BY_KEY } from '../columnsMeta';
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
