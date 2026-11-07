import { useEffect, useRef, useCallback, useState } from 'react';
import { useProjectStore, selectNewerFileReadOnly, type ProjectStore } from '../store/projectStore';
import { useUIStore } from '../store/uiStore';
import { isAutosaveHeld, useAutosaveHoldStore } from '../utils/autosaveHold';

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
 *
 * [#576 D34] Held while a "leave this project" prompt is open or a discard is
 * switching projects (`utils/autosaveHold`): the pending timer is dropped, nothing
 * arms or fires until the hold ends, and a project still dirty then gets a fresh
 * full interval.
 */
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
    // Drop the pending save while autosave is held (see autosaveHold). Leaves a
    // "saved"/"error" status alone - only the countdown it replaces goes.
    const pause = () => {
      if (debounceRef.current) {
        clearTimeout(debounceRef.current);
        debounceRef.current = null;
      }
      if (countdownRef.current) {
        clearInterval(countdownRef.current);
        countdownRef.current = null;
      }
      setCountdown(0);
      setStatus((prev) => (prev === 'pending' ? 'idle' : prev));
    };

    // Start (or restart) the debounce for a dirty project, unless something
    // says not to save it now.
    const arm = (state: ProjectStore) => {
      const filePath = state.project?.file_path ?? null;
      const autoSave = state.config?.general?.auto_save ?? false;
      const intervalSeconds =
        state.config?.general?.auto_save_interval_seconds ?? 30;

      // Guard: skip if auto-save disabled, no project, or never saved
      if (!autoSave || !filePath) return;

      // [#576 B6-D1] Never on a project a newer Stalagmight saved: the backend
      // refuses every save of it, so an attempt could only fail - and fail again
      // on every debounce. Nothing should dirty such a project (every edit is
      // refused), but autosave must not be the thing that finds out.
      if (selectNewerFileReadOnly(state)) return;

      // [#576 B15] Nor while stored project settings could not be read: every
      // save is refused until the user sets them again (the banner says why).
      if (state.unreadableProjectSettings.length > 0) return;

      // [#576 RV2-FE-G4] Nor while the file's SpeleoDB link could not be read: a
      // save over it is refused (only a Save As to another file goes ahead, and
      // that clears the flag), and each refused attempt would raise the
      // autosave-failed banner beside the link banner that already says why.
      if (state.speleoLinkUnreadable) return;

      // If we're already executing a save, don't restart timers
      if (savingRef.current) return;

      // Paused after a save-blocking failure - don't keep retrying a save that
      // will just fail again. The status stays "didn't run - not saved" until
      // the resume check in the store subscriber fires (issues resolved).
      if (pausedRef.current) return;

      // [#576 D34] A leave prompt is open or a discard is switching projects:
      // the user is deciding what happens to these edits. Re-armed on release.
      if (isAutosaveHeld()) {
        pause();
        return;
      }

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
          !store.config?.general?.auto_save ||
          selectNewerFileReadOnly(store) ||
          store.unreadableProjectSettings.length > 0 ||
          store.speleoLinkUnreadable ||
          isAutosaveHeld()
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
    };

    // [#576 D34] Follow the hold: drop the pending save when it starts, and
    // give a still-dirty project a fresh full interval when it ends.
    let held = isAutosaveHeld();
    const onHoldChange = () => {
      const now = isAutosaveHeld();
      if (now === held) return;
      held = now;
      if (now) {
        pause();
      } else {
        const state = useProjectStore.getState();
        if (state.project && state.isDirty) arm(state);
      }
    };
    const unsubUi = useUIStore.subscribe(onHoldChange);
    const unsubHold = useAutosaveHoldStore.subscribe(onHoldChange);

    // Zustand v4 subscribe(listener) - listener receives (state, prevState).
    const unsub = useProjectStore.subscribe((state, prevState) => {
      const isDirty = state.isDirty;
      const wasDirty = prevState.isDirty;
      const hasProject = state.project != null;

      // --- Resume after a save-block clears ---
      // Auto-save pauses itself after a save-blocking failure (see the fire
      // handler in `arm`). Resume the moment the project stops being save-blocked -
      // i.e. the integrity issues were resolved (Repair) - so the next pending
      // edit saves normally.
      if (
        pausedRef.current &&
        prevState.integritySaveBlocked &&
        !state.integritySaveBlocked
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
      if (isDirty) arm(state);
    });

    return () => {
      unsub();
      unsubUi();
      unsubHold();
      clearAllTimers();
    };
  }, [clearAllTimers, flash]);

  return { status, countdown, cancelPending };
}
