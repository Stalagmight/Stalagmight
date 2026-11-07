/**
 * The row type for shots that have not been committed yet.
 *
 * Its own module so the column definitions and the editor component can both
 * import it without a cycle. `ShotTableEditor` re-exports it, because half a
 * dozen call sites (batchImportHelpers, shotConversion, several tests) have
 * always imported it from there.
 */
import type { Shot } from '../../types/project';

export interface EditableShot extends Omit<Shot, 'marker_index' | 'temperature'> {
  /**
   * Device readings carried through untouched (#576 B24): no column edits them,
   * so a row read from an mNemo dump keeps the water temperature and marker
   * index it recorded, a cell edit (which spreads the row) keeps them, and a
   * row added in the editor has none. Absent reads as "not recorded".
   */
  readonly temperature?: number | null;
  readonly marker_index?: number | null;
  /** Added in this editing session, not yet persisted. */
  isNew?: boolean;
  /** Soft-deleted: kept in the array so indices stay stable, hidden from view. */
  isDeleted?: boolean;
  /** Identity for a row that has no server id yet. */
  tempId?: number;
}

/**
 * A row the user has added but not measured, for any host of `ShotTableEditor`.
 *
 * Non-finite, NOT zero: a 0 length with a 0 azimuth and 0 depths is a due-north,
 * zero-depth shot nobody took, and every one of those values passes range
 * validation on the way to the backend. Blank renders blank in the grid and reads
 * as missing to the completeness gates.
 *
 * One factory because the three hosts each had their own copy and they drifted -
 * two of them were still building zeroes after the third was fixed.
 */
export function blankEditableShot(tempId: number): EditableShot {
  return {
    id: tempId,
    tempId,
    azimuth_in: NaN,
    azimuth_out: NaN,
    length: NaN,
    depth_in: NaN,
    depth_out: NaN,
    pitch_in: null,
    pitch_out: null,
    left: null,
    right: null,
    up: null,
    down: null,
    comment: null,
    kind: 'Normal',
    isNew: true,
  };
}
