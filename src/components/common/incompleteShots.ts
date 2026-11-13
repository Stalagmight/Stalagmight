/**
 * How many rows hold SOMETHING but are not a usable shot.
 *
 * Clearing a required cell writes `NaN` to mean "no measurement" - the grid will
 * never put a 0 into a survey - so every host that can send its rows onward has
 * to refuse while one of them is half-filled, or that `NaN` reaches the backend.
 * A row added and left entirely blank is nothing at all and does not count: it is
 * an invitation, not an obligation, and the hosts drop it.
 *
 * Its own module because three dialogs ask the same question and two of them
 * (the MNemo wizard, zero-datum config) are already past the file-size ratchet's
 * budget - anything shared has to leave.
 */
import type { TFunction } from 'i18next';
import { useUIStore } from '../../store/uiStore';
import type { EditableShot } from './editableShot';
import { isBlankNewShot, isShotComplete } from './editableShotStatus';
import { EDITABLE_AZIMUTH_AVG_KEY } from './editableShotColumns';

export function countIncompleteShots(shots: readonly EditableShot[]): number {
  let n = 0;
  for (const shot of shots) {
    if (shot.isDeleted === true) continue;
    if (isBlankNewShot(shot)) continue;
    if (!isShotComplete(shot, EDITABLE_AZIMUTH_AVG_KEY)) n += 1;
  }
  return n;
}

/**
 * Refuse to proceed while any row is half-filled, having said so. Returns true
 * when the caller must stop. `toastKey` dedupes per dialog - callers must pass
 * a value from `TOAST_KEYS` (src/utils/toastKeys.ts).
 */
export function blockedByIncompleteShots(
  count: number,
  t: TFunction,
  toastKey: string
): boolean {
  if (count === 0) return false;
  useUIStore
    .getState()
    .showToast(t('editSection.incompleteBlocked', { count }), toastKey, 'warning');
  return true;
}
