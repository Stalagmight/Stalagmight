import { confirm } from '@tauri-apps/plugin-dialog';
import { useProjectStore } from '../store/projectStore';
import type { SpeleoFileAccess } from '../types/ipc.generated';
import { invokeIpc } from './ipc';

/** Consent is bound to the backend session that supplied this local inspection. */
export async function releaseSpeleoLock(current: () => boolean, ownOnly = true): Promise<boolean> {
  const path = useProjectStore.getState().project?.file_path;
  if (!path) throw new Error('No linked project is open.');
  const access = await invokeIpc<SpeleoFileAccess | null>('speleodb_project_access_at', { path, localOnly: true });
  if (!current()) return false;
  if (!access?.release_context) throw new Error('The project or account changed. Try again.');
  const acknowledged = await confirm('Release without uploading? Your edits remain local. Another collaborator may advance the remote project.', {
    kind: 'warning', okLabel: 'Release without uploading', cancelLabel: 'Cancel',
  });
  if (!acknowledged || !current()) return false;
  await invokeIpc('speleodb_release_current_lock', { ownOnly, allowUnpushed: true, expected: access.release_context });
  return current();
}
