/**
 * Semantic test ids for the Data workspace grid. A row is named by what it is,
 * never by its position in the (filtered, sorted) view:
 *   station  `st-<sectionId>-<stationIndex>`  (0-based, as formatStationId)
 *   shot     `sh-<sectionId>-<position>`      (1-based, as formatShotId)
 *   header   `sec-<sectionId>`
 */
import type { DataRow } from './types';

export function dataRowPart(row: DataRow): string {
  if (row.kind === 'station') return `st-${row.sectionId}-${row.stationIndex}`;
  if (row.kind === 'shot') return `sh-${row.sectionId}-${row.position}`;
  return `sec-${row.sectionId}`;
}

/** The gutter label of a row (`rowTestId`). */
export const dataRowTestId = (row: DataRow) => `data-row-${dataRowPart(row)}`;
/** A cell's content (`cellTestId`). */
export const dataCellTestId = (row: DataRow, colKey: string) => `data-cell-${dataRowPart(row)}-${colKey}`;
