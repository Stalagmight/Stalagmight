import { getUploadContinuation } from './uploadContinuation';
import { useProjectStore } from '../store/projectStore';
import { useSpeleoStore, type SpeleoLinkState } from '../store/speleoStore';
import { useUIStore } from '../store/uiStore';
import { saveProjectAs } from './projectActions';
import { saveOpenProjectLocally } from './saveOpenProjectLocally';
import { canResumeSpeleoEditing } from './speleoPermission';
import { TOAST_KEYS } from './toastKeys';
import i18n from '../i18n';

/** The open linked project can be uploaded right now: we hold the lock, signed in, with write access. */
export function speleoUploadAllowedNow(link: SpeleoLinkState | null): boolean {
  return link?.editState === 'editing' && canResumeSpeleoEditing(link, useSpeleoStore.getState().session);
}

/** Why a linked project's work stays on this computer, in the user's words. */
function notUploadedReason(link: SpeleoLinkState): string {
  const session = useSpeleoStore.getState().session;
  if (session?.loggedIn !== true) return i18n.t('speleodb.saveLocal.signedOut');
  if (link.lockedByOther) {
    return i18n.t('speleodb.saveLocal.lockedBy', { email: link.lockHolder ?? i18n.t('speleodb.editBar.someone') });
  }
  return i18n.t('speleodb.saveLocal.noAccess');
}

/**
 * Explicit Save only (menu Save, Cmd/Ctrl+S, Save and Close). Autosave and
 * autosave retry call saveProject directly.
 *
 * A linked project uploads through the Save Changes dialog when it can. When it
 * cannot (frontend#6), the work is first saved to the local copy - never left
 * only discardable - and then:
 * - the editing lock can be taken (signed in, write access, nobody else
 *   holding it): the Save Changes dialog opens and takes it, so the user can
 *   upload with a description;
 * - otherwise the user is told the work is on this computer and why it was not
 *   uploaded, and the local save's result is returned (so Save and Close goes on
 *   to close).
 */
export async function dispatchProjectSave(opts?: { closeAfterSave?: boolean }) {
  const state = useProjectStore.getState();
  const ui = useUIStore.getState();
  const link = useSpeleoStore.getState().link;
  if (!state.project) return null;
  if (link) {
    const pending = getUploadContinuation(state.project.file_path);
    const closeAfterSave = opts?.closeAfterSave === true || pending?.closeAfterSave === true;
    if (!link.hasUnpushedEdits && !state.isDirty && !closeAfterSave) {
      ui.showToast('No changes to upload', 'speleodb-no-upload-changes', 'info');
      return null;
    }
    if (speleoUploadAllowedNow(link)) {
      ui.openDialog('speleodb-done', { mode: 'push', closeAfterSave });
      return null;
    }
    const saved = await saveOpenProjectLocally();
    if (!saved?.success) return saved;
    const acquirable = canResumeSpeleoEditing(link, useSpeleoStore.getState().session) && !link.lockedByOther;
    if (acquirable) {
      ui.openDialog('speleodb-done', { mode: 'push', closeAfterSave, acquire: true });
      return null;
    }
    ui.showToast(notUploadedReason(link), TOAST_KEYS.SPELEODB_SAVED_LOCALLY, 'warning');
    return saved;
  }
  if (!state.project.file_path) return saveProjectAs(opts);
  return saveOpenProjectLocally();
}
