/**
 * [#576 Q-RVFc-1-D1] An edit refused because a project switch is under way
 * (`switchGate.ts`, #576 FZ-FZ-INT-7) says so with a quiet toast. The refusal used
 * to be silent: an edit made while a project opened did nothing, with nothing on
 * screen to say why. Saves are refused the same way and stay silent (an autosave
 * runs on its own); only the edits the user makes get the notice, and the saves the
 * exit prompts make when a button there is pressed (`UnsavedChangesDialog`,
 * `SpeleoCloseDialog`, `SpeleoQuitDialog`, #576 W87a).
 */
import i18n from '../i18n';
import { TOAST_KEYS } from '../utils/toastKeys';
import type { CommandRefused } from './commandResult';
import { refuseDuringSwitch } from './switchGate';
import { useUIStore } from './uiStore';

/** {@link refuseDuringSwitch} for an edit: the refusal, with its notice; null when there is none. */
export function refuseEditDuringSwitch(): CommandRefused | null {
  const refused = refuseDuringSwitch();
  if (refused) {
    useUIStore.getState().showToast(i18n.t('editGate.switchPendingNotice'), TOAST_KEYS.EDIT_DURING_SWITCH, 'info');
  }
  return refused;
}
