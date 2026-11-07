import { useEffect, useRef } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { confirm } from '@tauri-apps/plugin-dialog';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { debugLog } from '../utils/debugLog';
import { consumeQuitConfirmation, consumeUpdateRestartArmed, abandonUpdateRestart } from '../utils/quit';
import { invokeIpc } from '../utils/ipc';
import { useUIStore } from '../store/uiStore';
import { useUpdateStore } from '../store/updateStore';
import { TOAST_KEYS } from '../utils/toastKeys';
import i18n from '../i18n';

/**
 * Hook to show a confirmation dialog when the user tries to close the app window
 * or navigate away while certain conditions are met (e.g., import in progress).
 *
 * IMPORTANT: The Tauri backend always prevents window close and emits 'tauri://close-requested'.
 * This hook MUST always listen for that event, otherwise the window cannot be closed.
 * When shouldBlock is false, we immediately close. When true, we show a confirmation.
 *
 * `guard` is the second, in-app tier: the OS window X arrives here as a
 * `tauri://close-requested` event, so this is the ONLY place that can route it
 * through the app's own exit guards (`guardQuit`). Before it existed, closing via
 * X destroyed the window whenever `shouldBlock` was false - discarding unsaved-
 * to-disk work and unpushed SpeleoDB edits with no prompt at all, while the menu
 * Quit prompted for both. A native `confirm` string is not a substitute for the
 * app's own dialog, which is why the guard opens a real dialog and aborts the
 * close instead of answering here.
 *
 * @param shouldBlock - Whether to block the close action and show confirmation
 * @param message - The message to show in the confirmation dialog
 * @param onConfirm - Optional callback to run cleanup before closing (e.g., delete placed sections)
 * @param guard - Optional in-app guard run after any `shouldBlock` confirmation.
 *   Returns true when it opened a dialog and the close must be abandoned.
 */
export function useBeforeUnload(
  shouldBlock: boolean,
  message: string,
  onConfirm?: () => Promise<void>,
  guard?: () => boolean
) {
  // Track if a dialog is currently being shown to prevent duplicates
  const isShowingDialog = useRef(false);
  // Track the unlisten function directly
  const unlistenRef = useRef<UnlistenFn | null>(null);
  // Track if we're in the process of closing
  const isClosing = useRef(false);
  // Track current blocking state for the listener callback
  const shouldBlockRef = useRef(shouldBlock);
  // Kept in a ref (not a dep) so a re-rendered guard identity never tears down
  // and re-registers the close listener.
  const guardRef = useRef(guard);
  // Tier 1's answer for the quit attempt currently in flight.
  //
  // A tier-2 dialog aborts the close and re-issues it (via `performQuit`) once the
  // user acts, so a single quit reaches this listener twice. Remembering the answer
  // is what lets tier 1 run BEFORE that dialog without asking twice - and running
  // it before is the whole point: the dialog's actions are irreversible (a push and
  // a lock RELEASE), so nothing that can still cancel the quit may come after them.
  // Dropped on any un-confirmed close request, so an attempt the user abandoned at
  // the dialog cannot lend its answer to a later one.
  const tier1Confirmed = useRef(false);

  // Keep ref in sync with prop
  useEffect(() => {
    shouldBlockRef.current = shouldBlock;
  }, [shouldBlock]);

  useEffect(() => {
    guardRef.current = guard;
  }, [guard]);

  useEffect(() => {
    // Always clean up any existing listener first
    if (unlistenRef.current) {
      unlistenRef.current();
      unlistenRef.current = null;
    }

    let isMounted = true;

    // Close the window immediately without confirmation. Returns false when the
    // destroy REJECTED and the window is therefore still here - the caller has to
    // know, because everything it does around this call is one-way.
    const closeWindowNow = async (): Promise<boolean> => {
      debugLog('[useBeforeUnload] closeWindowNow called');
      try {
        const appWindow = getCurrentWindow();
        debugLog('[useBeforeUnload] Got window, calling destroy');
        await appWindow.destroy();
        debugLog('[useBeforeUnload] Window destroyed');
        return true;
      } catch (e) {
        console.error('[useBeforeUnload] Window destroy failed:', e);
        return false;
      }
    };

    // Handle Tauri window close request
    // ALWAYS listen for this event - the backend always prevents default close
    const setupTauriListener = async () => {
      try {
        debugLog('[useBeforeUnload] Setting up close listener');
        const unlisten = await listen('tauri://close-requested', async () => {
          debugLog('[useBeforeUnload] Close requested, shouldBlock:', shouldBlockRef.current, 'isClosing:', isClosing.current);

          // If we're already closing, don't do anything
          if (isClosing.current) {
            debugLog('[useBeforeUnload] Already closing, ignoring');
            return;
          }

          // A quit the user already confirmed in-app (performQuit) comes back
          // through this same event. Consume the one-shot so the in-app guard
          // below is skipped for THIS close only - otherwise e.g. "Discard &
          // Quit" would re-open the dialog it came from, forever.
          const preConfirmed = consumeQuitConfirmation();

          // A close request that is NOT pre-confirmed starts a fresh quit attempt,
          // so any tier-1 answer left over from one the user abandoned at a tier-2
          // dialog must not carry into it.
          if (!preConfirmed) tier1Confirmed.current = false;

          // Tier 1: the in-memory states with no in-app dialog of their own (an
          // import wizard mid-flight, a dirty dive plan). Deliberately NOT skipped
          // by `preConfirmed` alone: a quit confirms the PROJECT, which says nothing
          // about the dive plan the same close would drop. It is skipped only when
          // THIS attempt already asked it - see `tier1Confirmed`. Since `quitApp`
          // now routes menu Quit / Ctrl+Q through this same listener, that ask
          // always happens before the tier-2 dialog acts.
          if (shouldBlockRef.current && !tier1Confirmed.current) {
            // Prevent multiple dialogs
            if (isShowingDialog.current || !isMounted) return;
            isShowingDialog.current = true;
            let proceed = false;
            try {
              proceed = await confirm(message, {
                title: 'Close Application',
                kind: 'warning',
                okLabel: 'Discard and Close',
                cancelLabel: 'Cancel',
              });
            } catch (e) {
              console.error('Close confirmation error:', e);
              proceed = false;
            } finally {
              isShowingDialog.current = false;
            }
            if (!proceed) {
              // [#579] The user declined the exit outright, so an update
              // restart that was riding on it is abandoned - on both sides.
              abandonUpdateRestart();
              return;
            }
            tier1Confirmed.current = true;
          }

          // Tier 2: the app's own exit guards (unpushed SpeleoDB edits under our
          // write lock, then unsaved-to-disk changes). Opening a dialog abandons
          // this close; the dialog's own action re-issues it via performQuit,
          // which arms `preConfirmed` so we land past this branch next time.
          if (!preConfirmed && guardRef.current?.()) {
            debugLog('[useBeforeUnload] In-app guard opened a dialog, aborting close');
            return;
          }

          // Past every guard: the window IS going away, so the tier-1 cleanup is
          // finally safe to perform. It is destructive by contract ("e.g. delete
          // placed sections"), so it may only run once the close is actually going
          // ahead - running it inside the tier-1 confirmation performed the deletions
          // for a close that tier 2 then aborted. Keyed on `shouldBlock` rather than
          // on "we prompted this pass", so it still runs on the pre-confirmed pass
          // that skips an already-answered prompt.
          if (shouldBlockRef.current && onConfirm) {
            try {
              await onConfirm();
            } catch (e) {
              console.error('Cleanup callback failed:', e);
            }
          }

          // Mark that we're closing, so a close request arriving while `destroy()`
          // is in flight is ignored rather than running this whole flow again.
          isClosing.current = true;

          // [#579] An update restart armed by `requestUpdateRestart` has now
          // passed every guard exactly like a plain quit would - swap the
          // destroy for the install-and-restart command instead. A rejection
          // (the download went stale, disk full, etc.) leaves the window open
          // with an error toast, rather than destroying a window the update
          // never actually applied to.
          if (consumeUpdateRestartArmed()) {
            try {
              await invokeIpc('install_app_update_and_restart', undefined, {
                purpose: 'install downloaded app update and restart',
              });
              // Never returns on success - the process exits from under us.
            } catch (e) {
              console.error('[useBeforeUnload] install_app_update_and_restart failed:', e);
              // The backend arm was spent by the attempt itself, so there is
              // nothing left to unarm; what has to be undone is the BANNER,
              // which is sitting on "ready" offering a restart that just
              // failed. Dropping it back to "available" re-offers Download,
              // which is the only action that can recover - the bytes may well
              // be the reason the install failed.
              useUpdateStore.getState().setDownloadFailed();
              useUIStore
                .getState()
                .showToast(
                  i18n.t('updateBanner.toasts.installFailed'),
                  TOAST_KEYS.APP_UPDATE_INSTALL_FAILED,
                  'error',
                );
              isClosing.current = false;
            }
            return;
          }

          // Destroy the window. Both the latch above and the unregister below are
          // one-way, and `destroy()` CAN reject (a revoked/regressed
          // `core:window:allow-destroy` capability, an IPC failure). Doing either of
          // them up front made a single failed destroy permanently un-quittable: the
          // window stayed, and every later close request hit the `isClosing` early
          // return - with no listener left to reach in any case, so not even the OS
          // X could try again. So the latch is released and the listener is left
          // registered whenever the close did not actually happen.
          if (!(await closeWindowNow())) {
            debugLog('[useBeforeUnload] Destroy failed - staying quittable');
            isClosing.current = false;
            return;
          }

          // The window IS gone. Drop the listener so nothing can re-enter this flow
          // during teardown.
          if (unlistenRef.current) {
            unlistenRef.current();
            unlistenRef.current = null;
          }
        });

        if (isMounted) {
          unlistenRef.current = unlisten;
        } else {
          // Component unmounted before listener was set up
          unlisten();
        }
      } catch (e) {
        // Tauri API not available (dev mode without Tauri)
        console.warn('Tauri close-requested listener not available:', e);
      }
    };

    setupTauriListener();

    // Handle browser beforeunload (for dev mode) - only when blocking
    const handleBeforeUnload = (e: BeforeUnloadEvent) => {
      if (isClosing.current || !shouldBlockRef.current) return;
      e.preventDefault();
      e.returnValue = message;
      return message;
    };

    window.addEventListener('beforeunload', handleBeforeUnload);

    return () => {
      isMounted = false;
      window.removeEventListener('beforeunload', handleBeforeUnload);
      if (unlistenRef.current) {
        unlistenRef.current();
        unlistenRef.current = null;
      }
    };
  }, [message, onConfirm]); // Note: shouldBlock not in deps, we use ref for immediate access
}
