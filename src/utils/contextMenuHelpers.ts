import type { LineId, ShotId } from '../types/project';

export type RangeDeleteCase = 'whole' | 'prefix' | 'suffix' | 'interior';

/**
 * Classify a contiguous shot-range deletion `[firstIdx..lastIdx]` within a
 * line of `total` shots. Mirrors the backend `RangeDeleteKind`:
 * - `whole`    - every shot selected → the line is removed
 * - `prefix`   - starts at shot 0, shots remain after → gap-close (shift)
 * - `suffix`   - ends at the last shot, shots remain before → truncate (shift)
 * - `interior` - shots remain before AND after → split-vs-shift choice
 * `splitRequired` is true only for an interior range (the only case that can
 * split the line and so needs two line names).
 */
export function deriveRangeDeleteParams(
  firstIdx: number,
  lastIdx: number,
  total: number
): { kind: RangeDeleteCase; removesLine: boolean; splitRequired: boolean } {
  const atStart = firstIdx === 0;
  const atEnd = lastIdx === total - 1;
  let kind: RangeDeleteCase;
  if (atStart && atEnd) kind = 'whole';
  else if (atStart) kind = 'prefix';
  else if (atEnd) kind = 'suffix';
  else kind = 'interior';
  return { kind, removesLine: kind === 'whole', splitRequired: kind === 'interior' };
}

/**
 * Determines which dialog to open for a Delete Shot action based on the shot's
 * position within its line.
 *
 * - totalShots <= 0: invalid state -> falls through to edge-shot confirm
 * - totalShots === 1: last remaining shot -> confirm delete entire line
 * - mid-line (not first, not last, totalShots >= 3): -> delete-shot-choice dialog
 * - edge shot (first or last): -> simple confirm delete
 *
 * Both the DataTable context menu and the map context menu call this function
 * so they use identical position-aware logic.
 */
export function getDeleteShotDialog(
  lineId: LineId,
  shotId: ShotId,
  isFirst: boolean,
  isLast: boolean,
  totalShots: number,
  lineName: string,
): { dialog: string; data: Record<string, unknown> } {
  if (totalShots === 1) {
    // Last remaining shot: confirm removal of entire line.
    // ConfirmDialog provides the default message for 'last-shot' type.
    return {
      dialog: 'confirm-delete',
      data: {
        type: 'last-shot',
        lineId,
        shotId,
        lineName,
      },
    };
  }

  // Mid-line shot requires at least 3 shots (so the shot is truly between
  // first and last). Also guards against invalid state (totalShots <= 0) where
  // isFirst and isLast are both false but there is no real mid-shot.
  const isMidShot = totalShots >= 3 && !isFirst && !isLast;
  if (isMidShot) {
    // Mid-line shot: choose split vs shift geometry
    return {
      dialog: 'delete-shot-choice',
      data: {
        lineId,
        shotId,
        lineName,
      },
    };
  }

  // Edge shot (first or last with other shots in line): simple confirm
  return {
    dialog: 'confirm-delete',
    data: {
      type: 'shot',
      lineId,
      shotId,
      isFirst,
      isLast,
      totalShots,
      lineName,
    },
  };
}
