/**
 * The row IDs the Data grid's gutter shows. Only the IDs: the section is named
 * by its header band (and the header corner once that scrolls away), so the
 * row name no longer repeats it.
 *
 * - 'canonical' (the default): the app's own station / shot IDs, "0:5" and
 *   "0:6" (`formatStationId` / `formatShotId`), as Survey's table pills and
 *   the Detail panel write them.
 * - 'index': the bare index in the section, "5" / "6".
 *
 * `ROW_ID_MODE` is the one switch for the whole workspace (gutter, find,
 * hover card, Back to Data); flip it to compare the two.
 */
import { formatShotId, formatStationId } from '../../../utils/formatters';
import type { DataEntityRow } from '../types';

export type RowIdMode = 'canonical' | 'index';

export const ROW_ID_MODE: RowIdMode = 'canonical';

/** A station's ID ("0:5", or "5" in 'index' mode). */
export function stationIdLabel(sectionId: number, stationIndex: number, mode: RowIdMode = ROW_ID_MODE): string {
  return mode === 'canonical' ? formatStationId(sectionId, stationIndex) : String(stationIndex);
}

/** A shot's ID from its 1-based position ("0:6", or "6" in 'index' mode). */
export function shotIdLabel(sectionId: number, position: number, mode: RowIdMode = ROW_ID_MODE): string {
  return mode === 'canonical' ? formatShotId(sectionId, position) : String(position);
}

/** The ID a station / shot row shows in the gutter. */
export function rowIdLabel(row: DataEntityRow, mode: RowIdMode = ROW_ID_MODE): string {
  return row.kind === 'station'
    ? stationIdLabel(row.sectionId, row.stationIndex, mode)
    : shotIdLabel(row.sectionId, row.position, mode);
}
