/**
 * The row type for shots that have not been committed yet.
 *
 * Its own module so the column definitions and the editor component can both
 * import it without a cycle. `ShotTableEditor` re-exports it, because half a
 * dozen call sites (useImportState, batchImportHelpers, shotConversion, several
 * tests) have always imported it from there.
 */
import type { Shot } from '../../types/project';

export interface EditableShot extends Omit<Shot, 'marker_index' | 'temperature'> {
  /** Added in this editing session, not yet persisted. */
  isNew?: boolean;
  /** Soft-deleted: kept in the array so indices stay stable, hidden from view. */
  isDeleted?: boolean;
  /** Identity for a row that has no server id yet. */
  tempId?: number;
  /**
   * The mNemo station number of this shot's END station, for a row read from a
   * DMP file (`mnemoStationNumber`). Absent on rows the user added. Carried
   * through every wizard edit (they spread the row) and turned into per-station
   * names by `stationNamesForEmittedShots` when the import is sent.
   */
  mnemoStation?: number;
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
