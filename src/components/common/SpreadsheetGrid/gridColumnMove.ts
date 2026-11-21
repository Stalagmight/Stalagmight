/**
 * Header drag-to-reorder as pure functions: the insertion boundary under the
 * pointer, the full column order a drop produces (hidden columns keep their
 * slots), the host's `constrainColumnOrder` applied on top of it, and where the
 * drop line goes so it shows where the block will actually land.
 */

/** Insertion boundary (0..widths.length) among the visible columns at content
 *  x `x` (gutter excluded): past a column's midpoint means after it. */
export function dropBoundaryAt(x: number, offsets: readonly number[], widths: readonly number[]): number {
  let idx = 0;
  for (let i = 0; i < widths.length; i++) {
    if (x > (offsets[i] ?? 0) + (widths[i] ?? 0) / 2) idx = i + 1;
    else break;
  }
  return idx;
}

/** The full order (hidden columns included) after moving the visible block
 *  [fromC0..fromC1] to boundary `dropIdx`: the block lands before the first
 *  visible non-block column at or after `dropIdx`, or at the very end when
 *  there is none. Every hidden column keeps its slot, so it reappears where it
 *  was when re-shown. */
export function movedColumnOrder(
  colKeys: readonly string[],
  orderedKeys: readonly string[],
  fromC0: number,
  fromC1: number,
  dropIdx: number,
): string[] {
  const block = colKeys.slice(fromC0, fromC1 + 1);
  const blockSet = new Set(block);
  const rest = orderedKeys.filter((k) => !blockSet.has(k));
  const anchorKey = colKeys.slice(dropIdx).find((k) => !blockSet.has(k));
  const insertAt = anchorKey !== undefined ? rest.indexOf(anchorKey) : rest.length;
  return [...rest.slice(0, insertAt), ...block, ...rest.slice(insertAt)];
}

/** A host constraint's answer as a permutation of `orderedKeys`: unknown and
 *  repeated keys are dropped, missing ones appended in their current order. */
export function normalizeColumnOrder(order: readonly string[], orderedKeys: readonly string[]): string[] {
  const known = new Set(orderedKeys);
  const seen = new Set<string>();
  const out: string[] = [];
  for (const k of order) {
    if (known.has(k) && !seen.has(k)) {
      seen.add(k);
      out.push(k);
    }
  }
  for (const k of orderedKeys) if (!seen.has(k)) out.push(k);
  return out;
}

export interface ColumnDrop {
  /** The full order to apply on release; null = the drop changes nothing, so
   *  release does nothing. */
  order: string[] | null;
  /** Content x of the drop line (gutter included); null = draw no line. */
  lineX: number | null;
}

export interface PlanColumnDropArgs {
  colKeys: readonly string[];
  orderedKeys: readonly string[];
  fromC0: number;
  fromC1: number;
  dropIdx: number;
  offsets: readonly number[];
  gutterWidth: number;
  constrain?: ((order: string[]) => string[]) | undefined;
}

/**
 * What a header drop at boundary `dropIdx` does. Without `constrain` this is
 * the historical behaviour: the line sits at the pointer's boundary and release
 * applies the moved order (even an unchanged one). With it, the moved order
 * goes through the constraint first; when the result equals the current order
 * there is no line and release is a no-op, otherwise the line is drawn where
 * the block lands, in the CURRENT layout: before the first visible non-block
 * column that follows the block in the constrained order (the far right edge
 * when none does).
 */
export function planColumnDrop(a: PlanColumnDropArgs): ColumnDrop {
  const moved = movedColumnOrder(a.colKeys, a.orderedKeys, a.fromC0, a.fromC1, a.dropIdx);
  if (!a.constrain) return { order: moved, lineX: a.gutterWidth + (a.offsets[a.dropIdx] ?? 0) };
  const order = normalizeColumnOrder(a.constrain(moved), a.orderedKeys);
  if (order.every((k, i) => k === a.orderedKeys[i])) return { order: null, lineX: null };
  const visible = new Set(a.colKeys);
  const block = new Set(a.colKeys.slice(a.fromC0, a.fromC1 + 1));
  const vis = order.filter((k) => visible.has(k));
  const first = Math.max(0, vis.findIndex((k) => block.has(k)));
  const anchor = vis.slice(first).find((k) => !block.has(k));
  const boundary = anchor === undefined ? a.colKeys.length : a.colKeys.indexOf(anchor);
  return { order, lineX: a.gutterWidth + (a.offsets[boundary] ?? 0) };
}

/** The moved block's leftmost and rightmost keys in the new visible order, so
 *  the block stays selected after the move (a constraint may reorder it). */
export function movedBlockEdges(
  order: readonly string[],
  colKeys: readonly string[],
  fromC0: number,
  fromC1: number,
): { first: string; last: string } | null {
  const block = new Set(colKeys.slice(fromC0, fromC1 + 1));
  const inBlock = order.filter((k) => block.has(k));
  const first = inBlock[0];
  const last = inBlock[inBlock.length - 1];
  return first !== undefined && last !== undefined ? { first, last } : null;
}
