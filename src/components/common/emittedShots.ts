/**
 * One answer to "which rows will actually be imported", and the index maths that
 * has to agree with it.
 *
 * Dropping wholly blank new rows from the payloads created two pictures of a
 * line: the one on screen, which counts a blank row, and the one sent to the
 * backend, which does not. Every place that turns a station the user PICKED into
 * a station the backend will see had to be updated to the second picture, and
 * three were missed - zero-datum reanchoring and both DMP placement paths - each
 * capable of landing a GPS anchor or a merge point on the wrong station.
 *
 * So the predicate lives here, with the two translations that depend on it, and
 * both payload builders and index maths import them. A site that needs to count
 * shots should reach for one of these rather than write `!s.isDeleted` again.
 */
import type { EditableShot } from './editableShot';
import { isBlankNewShot } from './editableShotStatus';

/** Whether this row reaches the backend. Deleted rows never do; a row added and
 *  left wholly blank is an invitation, not an obligation, and is dropped too. */
export const isEmittedShot = (s: EditableShot | undefined): boolean =>
  s !== undefined && s.isDeleted !== true && !isBlankNewShot(s);

/** How many emitted rows precede this index in the FULL array. A station index
 *  counts shots, so this bridges array space to emitted-station space. */
export function emittedBefore(shots: readonly EditableShot[], arrayIndex: number): number {
  let count = 0;
  for (let i = 0; i < arrayIndex && i < shots.length; i++) {
    if (isEmittedShot(shots[i])) count++;
  }
  return count;
}

/** Whether the user can SEE this row. Deleted rows are hidden; a blank added row
 *  is on screen and occupies a station, even though it never reaches the
 *  backend - which is exactly why the two spaces need separate counts. */
export const isVisibleShot = (s: EditableShot | undefined): boolean =>
  s !== undefined && s.isDeleted !== true;

/** How many visible rows precede this index in the FULL array. The counterpart
 *  of {@link emittedBefore} for a pick stored in visible space. */
export function visibleBefore(shots: readonly EditableShot[], arrayIndex: number): number {
  let count = 0;
  for (let i = 0; i < arrayIndex && i < shots.length; i++) {
    if (isVisibleShot(shots[i])) count++;
  }
  return count;
}

/** Rows on screen. The station dropdown lists this many plus one. */
export function visibleShotCount(shots: readonly EditableShot[]): number {
  return shots.filter(isVisibleShot).length;
}

/**
 * A station index the user picked - counted over the rows they can SEE, blanks
 * included - expressed in the emitted line's numbering. Clamped, so a pick
 * past the end of the shortened line lands on its last station rather than
 * out of range.
 */
export function toEmittedStationIndex(
  shots: readonly EditableShot[],
  visibleStationIndex: number
): number {
  let visible = 0;
  let emitted = 0;
  for (const s of shots) {
    if (s.isDeleted === true) continue;
    if (visible >= visibleStationIndex) break;
    visible++;
    if (isEmittedShot(s)) emitted++;
  }
  const total = shots.filter(isEmittedShot).length;
  return Math.min(emitted, total);
}

/**
 * The other direction: a station a PREVIEW drew, expressed over the rows on
 * screen.
 *
 * The previews render the emitted rows (a blank row has no geometry to draw and
 * a NaN there poisons every station after it), so a click on a station dot
 * arrives numbered in emitted space while the pick is stored, listed and
 * re-anchored in visible space. Where several visible stations map to one drawn
 * station - a blank row occupies no distance - this answers with the first of
 * them, so a line with no blanks answers with the index it was handed.
 */
export function fromEmittedStationIndex(
  shots: readonly EditableShot[],
  emittedStationIndex: number
): number {
  let emitted = 0;
  let visible = 0;
  let endOfEmitted = 0;
  for (const s of shots) {
    if (s.isDeleted === true) continue;
    if (isEmittedShot(s)) {
      if (emitted >= emittedStationIndex) return visible;
      emitted++;
      endOfEmitted = visible + 1;
    }
    visible++;
  }
  // Past the last drawn station - it is the end of the emitted run, whatever
  // blank rows trail after it.
  return endOfEmitted;
}
