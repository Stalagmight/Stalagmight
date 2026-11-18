/**
 * What a row in the editable shot grid is missing, and whether it counts at all.
 *
 * One module because three places have to agree: the grid tints the missing
 * CELLS, the host refuses to Save while a row is incomplete, and the host's save
 * payload drops the rows that were never filled in. Two copies of "what counts
 * as empty" would eventually disagree and either block a Save that should pass
 * or write a NaN.
 *
 * Mirrors the new-line dialog's rule (`isDraftComplete` /
 * `draftMissing`): a shot needs a positive length, both depths, and at least one
 * azimuth; inclination, LRUD and comment stay optional.
 */
import type { EditableShot } from './editableShot';
// The same guard the draft rules use, not a local copy of it - see the mirror
// note above.
import { isFiniteNumber as fin } from '../SingleSystemView/dataTableDrafts';

/**
 * A row the user added and then left (or emptied) completely: no measurement, no
 * comment, nothing. It is an invitation, not an obligation - it must not block
 * Save, and it must not be written.
 *
 * Only ever true for `isNew` rows. An EXISTING shot emptied to nothing is a
 * different thing: it is a shot that used to have values, so it stays incomplete
 * (and blocking) rather than quietly vanishing on save.
 */
export function isBlankNewShot(shot: EditableShot): boolean {
  if (!shot.isNew) return false;
  return (
    !fin(shot.length) &&
    !fin(shot.azimuth_in) &&
    !fin(shot.azimuth_out) &&
    !fin(shot.depth_in) &&
    !fin(shot.depth_out) &&
    !fin(shot.pitch_in) &&
    !fin(shot.pitch_out) &&
    shot.left === null &&
    shot.right === null &&
    shot.up === null &&
    shot.down === null &&
    (shot.comment === null || shot.comment === '') &&
    (shot.kind ?? 'Normal') === 'Normal'
  );
}

/**
 * Grid column keys whose required value this row is missing. Empty for a row
 * that is complete, and empty for a blank new row - nothing is "missing" from a
 * row that holds nothing.
 *
 * The azimuth mark goes on whichever azimuth column is actually SHOWING. The
 * average is the default, but the eye menu can swap it for the in/out legs, and
 * marking a hidden column paints a red cell nobody can see while Save refuses to
 * proceed - the same dead end the new-line dialog solves by revealing.
 */
export function missingRequiredCells(
  shot: EditableShot,
  azimuthAvgKey: string,
  /** Hidden column keys, so the mark lands somewhere visible. Omit when the
   *  caller only needs completeness, not cell identity. */
  hiddenColumns?: ReadonlySet<string>
): Set<string> {
  const missing = new Set<string>();
  if (isBlankNewShot(shot)) return missing;
  if (!fin(shot.length) || shot.length <= 0) missing.add('length');
  if (!fin(shot.depth_in)) missing.add('depth_in');
  if (!fin(shot.depth_out)) missing.add('depth_out');
  // BOTH legs, not either. The Rust boundary rejects any non-finite geometry
  // (`ShotData::all_finite` / `ShotUpdates::all_finite`), so a row with one leg
  // cleared used to pass every UI check and then fail at Save with an error the
  // user could do nothing about where it appeared. Refusing here puts the
  // refusal on the cell. The average column still DISPLAYS a lone leg as the
  // mean - reading one is honest, saving half a pair is not.
  const legsMissing = !fin(shot.azimuth_in) || !fin(shot.azimuth_out);
  if (legsMissing) {
    if (hiddenColumns?.has(azimuthAvgKey) === true) {
      // Avg is hidden, so the legs are what the user can see and fill.
      if (!fin(shot.azimuth_in)) missing.add('azimuth_in');
      if (!fin(shot.azimuth_out)) missing.add('azimuth_out');
    } else if (!fin(shot.azimuth_in) && !fin(shot.azimuth_out)) {
      missing.add(azimuthAvgKey);
    } else {
      // Exactly one leg: the avg cell shows a value, so marking it would look
      // wrong. Mark the empty leg, which is what has to be filled.
      missing.add(fin(shot.azimuth_in) ? 'azimuth_out' : 'azimuth_in');
    }
  }
  return missing;
}

/** Whether this row can be written: nothing required is missing. A blank new row
 *  is "complete" in the sense that it blocks nothing - callers drop it. */
export function isShotComplete(shot: EditableShot, azimuthAvgKey: string): boolean {
  return missingRequiredCells(shot, azimuthAvgKey).size === 0;
}
