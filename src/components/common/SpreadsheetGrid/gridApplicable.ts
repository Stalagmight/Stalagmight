/**
 * "Does this column apply to this row at all?" - the rule behind
 * {@link GridColumn.isApplicable} and group-header rows, in one place so every
 * edit path (type, paste, fill, clear, move), the copy path, Tab and the renderer
 * agree on it.
 *
 * A not-applicable (N/A) cell is not a locked one: it has no value to protect,
 * so it reads as "nothing here" rather than "forbidden". It renders blank, never
 * takes an edit, copies as '' and is skipped by Tab.
 */
import type { ReactNode } from 'react';
import type { GridColumn } from './types';

/** Stash marker for a copied N/A cell. An internal paste skips it rather than
 *  clearing the destination: the source had no value, so there is nothing to
 *  move - and on a grid that mixes row kinds a misaligned paste would otherwise
 *  blank real data with the gaps of the other row kind. */
export const NOT_APPLICABLE: unique symbol = Symbol('sg-not-applicable');

export type GroupRowContent<Row> = (row: Row, rowIndex: number) => ReactNode | undefined;

/** True when the host renders this row as a group-header band. `null`,
 *  `undefined` and `false` all mean "an ordinary row". */
export function isGroupRow<Row>(
  groupRowContent: GroupRowContent<Row> | undefined,
  row: Row,
  rowIdx: number,
): boolean {
  if (!groupRowContent) return false;
  const content = groupRowContent(row, rowIdx);
  return content !== undefined && content !== null && content !== false;
}

/** The column's own applicability for a row (group rows are handled by the
 *  caller, which already knows whether it is drawing a band). */
export function columnApplies<Row>(col: GridColumn<Row>, row: Row, rowIdx: number): boolean {
  return col.isApplicable ? col.isApplicable(row, rowIdx) : true;
}

/** Full applicability: false on every cell of a group-header row. */
export function cellApplies<Row>(
  col: GridColumn<Row>,
  row: Row,
  rowIdx: number,
  groupRowContent: GroupRowContent<Row> | undefined,
): boolean {
  if (isGroupRow(groupRowContent, row, rowIdx)) return false;
  return columnApplies(col, row, rowIdx);
}
