import { useSpeleoStore } from '../store/speleoStore';
import { getPublishContinuation } from './publishContinuation';
import { useProjectStore } from '../store/projectStore';
import { useUIStore } from '../store/uiStore';
import { getUploadContinuation } from './uploadContinuation';

/** Return to the originating Save confirmation. Authentication never submits it. */
export function finishSpeleoSettings(): void {
  useSpeleoStore.getState().clearPendingAuth();
  useUIStore.getState().closeDialog();
  const pending = getUploadContinuation(useProjectStore.getState().project?.file_path);
  if (pending) useUIStore.getState().openDialog('speleodb-done', {
    mode: 'push', closeAfterSave: pending.closeAfterSave,
  });
  else if (getPublishContinuation()) useUIStore.getState().openDialog('publish-to-speleodb');
}
