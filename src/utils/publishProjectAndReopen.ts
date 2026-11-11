import { useProjectStore } from '../store/projectStore';
import { useSpeleoStore } from '../store/speleoStore';
import type { SpeleoLinkStatus, SpeleoOpenResult, SpeleoPublishParams, SpeleoPublishResult } from '../types/ipc.generated';
import { autoAcquireSpeleoLock } from './autoAcquireSpeleoLock';
import { invokeIpc } from './ipc';
import { beginProjectMutation, isProjectMutationCurrent, reserveProjectRequest } from './projectTransition';
import { speleoErrorAction } from './speleoError';

export type PublishReopenOutcome = 'opened' | 'upload_pending' | 'reload_failed' | 'recent_cleanup_failed' | 'stale';

/** The outcome, plus whether the first upload left the undo history out to fit the
 *  server's size limit (sdk#3) - orthogonal to the outcome, so reported beside it. */
export interface PublishReopen {
  outcome: PublishReopenOutcome;
  undoHistoryOmitted: boolean;
}

/** Creation saves and uploads; only a clean confirmed upload may be downloaded again. */
export async function publishProjectAndReopen(params: SpeleoPublishParams): Promise<PublishReopen> {
  const notes = { undoHistoryOmitted: false };
  const outcome = await publishAndReopen(params, notes);
  return { outcome, undoHistoryOmitted: notes.undoHistoryOmitted };
}

async function publishAndReopen(
  params: SpeleoPublishParams,
  notes: { undoHistoryOmitted: boolean },
): Promise<PublishReopenOutcome> {
  const initial = useProjectStore.getState();
  if (!initial.project || useSpeleoStore.getState().link) return 'stale';
  const originalPath = initial.project.file_path;
  let path = originalPath;
  const createdAt = initial.project.created_at;
  let requireOriginalCreation = true;
  const tree = initial.projectTreeGeneration;
  const session = useSpeleoStore.getState().sessionGeneration;
  const request = reserveProjectRequest();
  if (!beginProjectMutation(request)) return 'stale';
  const current = () => isProjectMutationCurrent(request) &&
    useSpeleoStore.getState().sessionGeneration === session &&
    useProjectStore.getState().project?.file_path === path &&
    (!requireOriginalCreation || useProjectStore.getState().project?.created_at === createdAt);
  const busy = useSpeleoStore.getState().beginBusy();
  let created = false;
  try {
    const published = await invokeIpc<SpeleoPublishResult>('speleodb_publish', { params });
    if (!current()) return 'stale';
    created = true;
    notes.undoHistoryOmitted = published.undo_history_omitted === true;
    // Creation is irreversible. Converge on the linked cache immediately, even
    // when upload or the following network download needs a later retry.
    path = published.local_path;
    useProjectStore.setState((state) => ({ project: state.project ? { ...state.project, file_path: published.local_path } : null }));
    useSpeleoStore.getState().syncProjectLink(true, published.local_path);
    useSpeleoStore.setState({ remoteProjectId: published.project_id });
    const status = await invokeIpc<SpeleoLinkStatus>('speleodb_link_status');
    if (!current()) return 'stale';
    if (!status.is_remote || status.project_id !== published.project_id) return 'reload_failed';
    useSpeleoStore.getState().seedFromStatus(status);
    const latest = useProjectStore.getState();
    if (latest.projectTreeGeneration !== tree && latest.isDirty) return 'upload_pending';
    // The publish command saved this version before attempting its first upload.
    useProjectStore.setState({ isDirty: false });
    if (status.has_unpushed_edits) return 'upload_pending';
    const downloaded = await invokeIpc<SpeleoOpenResult>('speleodb_open_project', {
      projectId: published.project_id, projectName: params.name, latestCommitSha: null,
    });
    if (!current()) return 'stale';
    // An offline cached open cannot satisfy a fresh-from-server conversion.
    if (downloaded.offline) return 'reload_failed';
    path = downloaded.local_path;
    const loaded = await useProjectStore.getState().loadProject(downloaded.local_path, {
      accessConfirmed: true, acquireLock: false, request,
    });
    if (!loaded.success) return loaded.superseded ? 'stale' : 'reload_failed';
    // A fresh server tree can carry normalized project metadata. The owned load
    // result, path and request token now identify the replacement session.
    requireOriginalCreation = false;
    if (!current()) return 'stale';
    await autoAcquireSpeleoLock(downloaded.local_path, current);
    if (!current()) return 'stale';
    // Forget only the successful conversion's original local shortcut. The
    // existing recents action never deletes the project file and refreshes both
    // the listing and persisted configuration after removal.
    if (originalPath && originalPath !== downloaded.local_path) {
      const removed = await useProjectStore.getState().removeFromRecentProjects(originalPath);
      if (!current()) return 'stale';
      if (!removed.success) return 'recent_cleanup_failed';
    }
    return 'opened';
  } catch (error) {
    if (!current()) return 'stale';
    if (speleoErrorAction(error) === 'ReLogin') useSpeleoStore.getState().clearSession();
    if (created) return 'reload_failed';
    throw error;
  } finally { useSpeleoStore.getState().endBusy(busy); }
}
