/**
 * [#576 RV4-R-S6] Show what the pre-push save could not keep when a SpeleoDB Done or
 * Push then FAILS.
 *
 * Done and Push save the project before their upload, and a save that goes through can
 * still have lost something (the display settings, the undo history). A successful
 * Done / Push answers with those warnings and `useSpeleoActions` shows them; a failure
 * cannot carry them, and they are one-shot (the next save does not repeat them), so the
 * backend sends them as the `speleodb-pre-push-save` event instead. Mounted once, in
 * `App`, so each failure shows them once whichever component started the verb.
 */
import { useEffect } from 'react';
import type { UnlistenFn } from '@tauri-apps/api/event';
import { IPC_EVENTS, listenIpc } from '../utils/ipcEvents';
import { surfaceSaveWarnings } from '../utils/saveFeedback';

export function useSpeleoPrePushSave(): void {
  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | null = null;
    void listenIpc(IPC_EVENTS.SPELEODB_PRE_PUSH_SAVE, (event) => {
      surfaceSaveWarnings(event.payload.warnings);
    })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch((e: unknown) => {
        console.error('Failed to listen for the pre-push save warnings:', e);
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);
}
