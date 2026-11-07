import { useEffect } from 'react';
import { IPC_EVENTS, listenIpc } from '../utils/ipcEvents';
import { invokeIpc } from '../utils/ipc';
import { useInternalErrorStore } from '../store/internalErrorStore';

/**
 * [#576 A11] Listen for the backend's one-shot `internal-error` event and record it
 * for `InternalErrorBanner`. Mount once at the app root.
 *
 * Registered the way `useNetworkStatus` does it: `await listenIpc()` inside
 * try/catch, so a rejected registration is reported where it fails, and a listener
 * that resolves after cleanup is released as soon as it arrives. Then it asks
 * `get_internal_error_status` once, for a report it missed (#576 RV5-PI-3).
 */
export function useInternalErrorEvent() {
  useEffect(() => {
    let disposed = false;
    let release: (() => void) | null = null;
    const register = async () => {
      try {
        const unlisten = await listenIpc(IPC_EVENTS.INTERNAL_ERROR, (event) => {
          const cell = event.payload.cell;
          console.error(`[internal-error] the backend recovered a lock poisoned by a crash (${cell})`);
          useInternalErrorStore.getState().raise(cell);
        });
        const unlistenSafely = () => {
          void new Promise<void>((resolve) => resolve(unlisten())).catch((e: unknown) => {
            console.warn('[useInternalErrorEvent] listener release failed:', e);
          });
        };
        if (disposed) {
          unlistenSafely();
          return;
        }
        release = unlistenSafely;
      } catch (e) {
        console.warn('[useInternalErrorEvent] internal-error listener not available:', e);
      }
      // [#576 RV5-PI-3] The event goes out once per session. A crash during
      // startup fires it before this listener exists, and a webview reload resets
      // the store after it went out; either way the banner would be gone while
      // undo, redo and the push keep refusing. So ask once the listener is in
      // place (a report arriving through both paths raises the banner once).
      try {
        const notice = await invokeIpc('get_internal_error_status');
        if (notice && !disposed) useInternalErrorStore.getState().raise(notice.cell);
      } catch (e) {
        console.warn('[useInternalErrorEvent] internal-error status not available:', e);
      }
    };
    void register();
    return () => {
      disposed = true;
      release?.();
    };
  }, []);
}
