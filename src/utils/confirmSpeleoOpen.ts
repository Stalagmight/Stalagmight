import { invokeIpc } from './ipc';
import type { SpeleoFileAccess } from '../types/ipc.generated';

/** Keep the disk preflight and request ownership check; opening read-only needs no advance confirmation. */
export async function confirmSpeleoFileOpen(path: string, current: () => boolean = () => true, localOnly = false): Promise<boolean> {
  await invokeIpc<SpeleoFileAccess | null>('speleodb_project_access_at', localOnly ? { path, localOnly: true } : { path });
  return current();
}
