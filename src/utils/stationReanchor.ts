/**
 * Keeping a committed station pick on the same physical station across a shot
 * edit.
 *
 * Pure index arithmetic, deliberately free of any opinion about WHICH rows
 * count: the zero-datum dialog re-anchors over the rows it will emit, the MNemo
 * wizard over the rows the user can see, and both spaces obey the same maths.
 * The caller counts, converts its array index to a position in that space, and
 * passes the totals in.
 *
 * Its own module because two dialogs need it and both are already past the
 * file-size ratchet's budget - anything shared has to leave.
 */

/**
 * Keep a committed station pick on the same physical station across a shot edit.
 *
 * The pick addresses the POST-EDIT order (a section of `s` surviving shots has
 * stations `0..s`), so adding or removing a survivor renumbers every station
 * beyond it. `editPosition` is the survivor position the edit lands at:
 * deleting the shot there merges stations `editPosition` and `editPosition + 1`,
 * inserting there splits them. Stations at or before it keep their number.
 *
 * Re-anchoring on EDIT is what makes the pick correct in both orderings.
 * Translating on pick instead (the shape the zero-datum dialog used to have)
 * only moves the breakage from delete-then-pick to pick-then-delete. This map is
 * also total, unlike an `EditableShot.id` lookup, so it can address a station
 * that a leading insert has only just created.
 */
export function reanchorStation(
  pick: number,
  editPosition: number,
  /** Rows added (positive) or removed (negative) at `editPosition`. Was `1 | -1`
   *  when every edit was a single row; a bulk insert shifts by its whole count,
   *  and the arithmetic below never cared how big the step was. */
  delta: number,
  survivorsAfter: number,
): number {
  const shifted = pick > editPosition ? pick + delta : pick;
  return Math.max(0, Math.min(survivorsAfter, shifted));
}

/**
 * A pick that sat INSIDE a removed run collapses to the run's start - the
 * boundary the deleted span closed up to, and the station that still physically
 * exists where the pick was.
 *
 * {@link reanchorStation} alone applies the whole run's delta to any pick past
 * the run's first station, which for a pick inside the run subtracts more
 * stations than precede it: deleting stations 2..7 with 5 picked gave 5 - 6,
 * clamped to 0, sending the GPS anchor to the head of the section.
 */
export function reanchorAfterRunDelete(
  pick: number,
  runStart: number,
  countedGone: number,
  survivorsAfter: number,
): number {
  if (pick > runStart && pick <= runStart + countedGone) return runStart;
  return reanchorStation(pick, runStart, -countedGone, survivorsAfter);
}
