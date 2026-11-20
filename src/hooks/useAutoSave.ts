import { useEffect, useRef, useCallback, useState } from 'react';
import { useProjectStore } from '../store/projectStore';
import { classifySaveError, type SaveErrorKind } from '../utils/saveErrors';

export type AutoSaveStatus = 'idle' | 'pending' | 'saving' | 'saved' | 'error';

export interface AutoSaveState {
  status: AutoSaveStatus;
  countdown: number;
  cancelPending: () => void;
}

/**
 * Debounced auto-save hook.
 *
 * Watches `isDirty` in the project store and, when auto-save is enabled and the
 * project has a file path, starts a debounce timer. Repeated changes reset the
 * timer.  Manual saves (isDirty going false externally) cancel the timer and
 * briefly show "Saved".
 */
/** Save failures that pause auto-save until the user resolves them. */
function isPausingFailure(kind: SaveErrorKind | null): boolean {
  return kind === 'externally-modified' || kind === 'read-only-file';
}

export function useAutoSave(): AutoSaveState {
  const [status, setStatus] = useState<AutoSaveStatus>('idle');
  const [countdown, setCountdown] = useState(0);

  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const countdownRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const flashRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Track whether we initiated the save so we can distinguish our own
  // isDirty→false from a manual Ctrl+S save.
  const savingRef = useRef(false);
  // Paused after a save-blocking failure (unresolvable cross-references): retrying
  // on every edit is futile and noisy, so we stop attempting until the project is
  // no longer save-blocked (e.g. the user runs Repair), then resume automatically.
  const pausedRef = useRef(false);

  const clearAllTimers = useCallback(() => {
    if (debounceRef.current) {
      clearTimeout(debounceRef.current);
      debounceRef.current = null;
    }
    if (countdownRef.current) {
      clearInterval(countdownRef.current);
      countdownRef.current = null;
    }
    if (flashRef.current) {
      clearTimeout(flashRef.current);
      flashRef.current = null;
    }
    setCountdown(0);
  }, []);

  const cancelPending = useCallback(() => {
    clearAllTimers();
    setStatus((prev) => (prev === 'pending' ? 'idle' : prev));
  }, [clearAllTimers]);

  // Flash a transient status then return to idle.
  const flash = useCallback(
    (s: 'saved' | 'error', durationMs: number) => {
      setStatus(s);
      if (flashRef.current) clearTimeout(flashRef.current);
      flashRef.current = setTimeout(() => {
        flashRef.current = null;
        setStatus('idle');
      }, durationMs);
    },
    [],
  );

  useEffect(() => {
    // Zustand v4 subscribe(listener) - listener receives (state, prevState).
    const unsub = useProjectStore.subscribe((state, prevState) => {
      const isDirty = state.isDirty;
      const wasDirty = prevState.isDirty;
      const filePath = state.project?.file_path ?? null;
      const autoSave = state.config?.general?.auto_save ?? false;
      const intervalSeconds =
        state.config?.general?.auto_save_interval_seconds ?? 30;
      const hasProject = state.project != null;

      // --- Resume after a save-block clears ---
      // Auto-save pauses itself after a save-blocking failure (see the fire
      // handler below). Resume the moment the project stops being save-blocked -
      // i.e. the integrity issues were resolved (Repair) - so the next pending
      // edit saves normally.
      if (
        pausedRef.current &&
        prevState.integritySaveBlocked &&
        !state.integritySaveBlocked
      ) {
        pausedRef.current = false;
      }
      // ...or once the failure that paused it is gone: a file another program
      // changed (the `.stalx` v2 guard, resolved in the save prompt) or a
      // read-only file. A save that lands clears `autosaveFailureKind`, and so
      // does dismissing the banner (the next edit then tries again and, if the
      // file is still changed, brings the banner back).
      if (
        pausedRef.current &&
        isPausingFailure(prevState.autosaveFailureKind) &&
        !isPausingFailure(state.autosaveFailureKind)
      ) {
        pausedRef.current = false;
      }

      // --- Project closed ---
      if (!hasProject) {
        pausedRef.current = false;
        clearAllTimers();
        savingRef.current = false;

        setStatus('idle');
        return;
      }

      // --- isDirty went false (save completed) ---
      if (wasDirty && !isDirty) {
        clearAllTimers();
        savingRef.current = false;

        flash('saved', 2000);
        return;
      }

      // --- isDirty is true (change happened) ---
      if (isDirty) {
        // Guard: skip if auto-save disabled, no project, or never saved
        if (!autoSave || !filePath) return;

        // If we're already executing a save, don't restart timers
        if (savingRef.current) return;

        // Paused after a save-blocking failure - don't keep retrying a save that
        // will just fail again. The status stays "didn't run - not saved" until
        // the resume check above fires (issues resolved).
        if (pausedRef.current) return;

        const seconds = intervalSeconds;

        // Cancel existing debounce timer (reset on every change)
        if (debounceRef.current) clearTimeout(debounceRef.current);
        if (countdownRef.current) clearInterval(countdownRef.current);
        if (flashRef.current) {
          clearTimeout(flashRef.current);
          flashRef.current = null;
        }

        setStatus('pending');
        setCountdown(seconds);

        // 1-second countdown interval
        countdownRef.current = setInterval(() => {
          setCountdown((c) => {
            if (c <= 1) return 0;
            return c - 1;
          });
        }, 1000);

        // Debounce timer - fires the actual save
        debounceRef.current = setTimeout(async () => {
          debounceRef.current = null;
          if (countdownRef.current) {
            clearInterval(countdownRef.current);
            countdownRef.current = null;
          }
          setCountdown(0);

          const store = useProjectStore.getState();
          // Re-check guards at fire time
          if (
            !store.isDirty ||
            !store.project?.file_path ||
            !store.config?.general?.auto_save
          ) {
            setStatus('idle');
            return;
          }

          savingRef.current = true;
          setStatus('saving');
          const result = await store.saveProject(undefined, true);
          // saveProject sets isDirty=false on success, which triggers the
          // subscriber above to flash "saved".  On failure we handle here.
          if (!result.success) {
            savingRef.current = false;
            // Auto-save must never persist a broken project, so the backend
            // refuses the save when the project has unresolvable cross-references.
            // Surface that auto-save did NOT run *persistently* (not a transient
            // flash) so the user knows their changes are not on disk; the integrity
            // banner shows how to fix it (Repair).
            // When the failure is the integrity save-block, PAUSE auto-save rather
            // than retry on every edit - it would just fail again. It resumes via
            // the check at the top of the subscriber once the block clears. (The
            // failure branch of saveProject awaits checkProjectIntegrity, so this
            // flag is fresh here.)
            if (useProjectStore.getState().integritySaveBlocked) {
              pausedRef.current = true;
            }
            // The same for a file another program changed, or a read-only
            // one: retrying would fail the same way until the user decides.
            if (isPausingFailure(classifySaveError(result.error))) {
              pausedRef.current = true;
            }
            if (flashRef.current) {
              clearTimeout(flashRef.current);
              flashRef.current = null;
            }
            // A save that landed WHILE this one was in flight (a manual Ctrl+S,
            // a Save As to a working folder) already put the changes on disk,
            // so this failure is moot - and painting it red anyway leaves the
            // Save menu item red with nothing to save, for as long as the
            // project stays clean (#429).
            setStatus(useProjectStore.getState().isDirty ? 'error' : 'idle');
          }
        }, seconds * 1000);
      }
    });

    return () => {
      unsub();
      clearAllTimers();
    };
  }, [clearAllTimers, flash]);

  return { status, countdown, cancelPending };
}
