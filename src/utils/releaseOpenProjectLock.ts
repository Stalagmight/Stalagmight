import i18n from '../i18n';
import { useProjectStore } from '../store/projectStore';
import { useSpeleoStore } from '../store/speleoStore';
import { useUIStore } from '../store/uiStore';
import type { SpeleoFileAccess } from '../types/ipc.generated';
import { getErrorMessage, invokeIpc } from './ipc';

/** Switch to read-only only after all work is saved locally and uploaded. */
export async function releaseOpenProjectLock(): Promise<void> {
  const speleo = useSpeleoStore.getState();
  const project = useProjectStore.getState();
  const path = project.project?.file_path;
  if (speleo.busy || !path) return;
  const session = speleo.sessionGeneration;
  const tree = project.projectTreeGeneration;
  const current = () => useProjectStore.getState().project?.file_path === path &&
    useProjectStore.getState().projectTreeGeneration === tree &&
    useSpeleoStore.getState().sessionGeneration === session;
  const pending = () => useProjectStore.getState().isDirty || useSpeleoStore.getState().link?.hasUnpushedEdits;
  const token = speleo.beginBusy();
  try {
    if (pending()) throw new Error('Save your changes using Save Changes before switching to read-only mode.');
    const access = await invokeIpc<SpeleoFileAccess | null>('speleodb_project_access_at', { path, localOnly: true });
    if (!current()) return;
    if (pending()) throw new Error('Save your changes using Save Changes before switching to read-only mode.');
    if (!access?.release_context) throw new Error('The project or account changed. Try again.');
    await invokeIpc('speleodb_release_current_lock', { expected: access.release_context });
    if (!current()) return;
    useSpeleoStore.setState((state) => ({ link: state.link ? {
      ...state.link, editState: 'read-only', lockHolder: null, lockedByOther: false,
    } : null }));
    await useSpeleoStore.getState().refreshLinkStatus();
  } catch (error) {
    if (current()) useUIStore.getState().openDialog('error', {
      title: i18n.t('speleodb.release.failed'), message: getErrorMessage(error),
    });
  } finally { useSpeleoStore.getState().endBusy(token); }
}
