import { useCallback } from 'react';
import { useProjectStore } from '../store/projectStore';
import { useUIStore } from '../store/uiStore';
import { useSpeleoStore } from '../store/speleoStore';
import { useShallow } from 'zustand/react/shallow';
import { refusedLocally } from '../store/commandResult';
import { saveFailureOf, surfaceSaveFailure } from '../utils/saveFeedback';
import { openOrImport } from '../utils/openOrImport';
import { newProjectGuarded, saveProjectAs } from '../utils/projectActions';
import { debugLog } from '../utils/debugLog';
import { needsSpeleoHandBackPrompt } from '../utils/quit';

export function useProject() {
  const { setView, openDialog } = useUIStore(
    useShallow((s) => ({ setView: s.setView, openDialog: s.openDialog }))
  );

  // Subscribe to store fields/actions individually rather than the whole store.
  // A no-selector `useProjectStore()` returns a fresh snapshot on every mutation
  // (every edit toggling isDirty, autosave tick, stats/geometry update), which
  // gave the handler callbacks below new identities each time (their deps held
  // `store`) and defeated the useProjectMenuItems / GlobalView menu memos that
  // depend on them - rebuilding the AppMenu items array + icon JSX on every tick.
  // Store ACTIONS have stable identities in zustand, so the handlers built from
  // them stay stable; the data fields re-render only when they actually change.
  const saveProject = useProjectStore((s) => s.saveProject);
  const closeProject = useProjectStore((s) => s.closeProject);
  const undo = useProjectStore((s) => s.undo);
  const redo = useProjectStore((s) => s.redo);

  const project = useProjectStore((s) => s.project);
  const stats = useProjectStore((s) => s.stats);
  const stations = useProjectStore((s) => s.stations);
  const recentProjects = useProjectStore((s) => s.recentProjects);
  const loading = useProjectStore((s) => s.loading);
  const canUndo = useProjectStore((s) => s.canUndo);
  const canRedo = useProjectStore((s) => s.canRedo);

  // New / Open / Save As all delegate to the shared, React-free helpers so the
  // hamburger-menu path and the keyboard dispatchers share one implementation
  // (the New-project unsaved-changes guard, the picker/route flow, and the
  // Save-As + overwrite-confirm flow each live in exactly one place).
  const handleNewProject = useCallback(() => newProjectGuarded(), []);

  const handleSaveProjectAs = useCallback(
    (opts?: { closeAfterSave?: boolean }) => saveProjectAs(opts),
    [],
  );

  const handleSaveProject = useCallback(async (opts?: { closeAfterSave?: boolean }) => {
    // An untitled project goes through Save As, which surfaces its own failures
    // (saveProjectAs guards on !success). Surfacing again here would double-report
    // - the second openDialog('error') is treated as dialog-chaining. So only the
    // direct save-to-existing-path branch surfaces.
    if (!useProjectStore.getState().project?.file_path) {
      return handleSaveProjectAs(opts);
    }
    try {
      const result = await saveProject();
      if (result && !result.success) {
        surfaceSaveFailure(result, openDialog, useProjectStore.getState());
      }
      return result;
    } catch (e) {
      // store.saveProject rejects only on an IPC-transport failure (a refused save
      // resolves as the refused outcome, handled above). Surface
      // it here rather than letting it escape as an unhandled rejection - callers
      // like the save-status retry and the menu Save invoke this without awaiting
      // or catching (`void onSave()` / `() => onSave()`).
      const failure = saveFailureOf(e, 'Failed to save');
      surfaceSaveFailure(failure, openDialog, useProjectStore.getState());
      return refusedLocally(failure.error);
    }
  }, [saveProject, handleSaveProjectAs, openDialog]);

  const handleCloseProject = useCallback(async () => {
    // A SpeleoDB project we still hold the write lock for routes through the close-
    // flow guard (push & release, or keep the lock) BEFORE the normal unsaved-changes
    // prompt, so no in-app close affordance - hamburger Close, Save-and-Close, the
    // overwrite-confirm close - can leave the server mutex stranded (fix 1). The
    // dialog owns the actual close and handles both dirty states itself.
    //
    // #270: the lock is not the only thing worth guarding, and gating on it ALONE
    // was the same defect `guardQuit` was fixed for - the states that strand the
    // work hardest (`notifyReverifyDivergence`, a `ReLogin` that runs
    // `clearSession()`) are exactly the ones that drop to read-only while KEEPING
    // the unpushed edits. Closing there returned straight to the home screen with
    // no notice at all. `needsSpeleoHandBackPrompt` is the exact predicate the two
    // OTHER hand-back exits (open-another-.stalx, New Project) now use, and its
    // unpushed-work half is literally shared with `guardQuit`, so none of the four
    // can silently diverge again.
    if (needsSpeleoHandBackPrompt(useSpeleoStore.getState().link)) {
      openDialog('speleodb-close');
      return { success: false, error: 'Showing dialog' };
    }
    // Check for unsaved changes - use getState() to get current value, not stale closure
    const currentIsDirty = useProjectStore.getState().isDirty;
    debugLog('[useProject] handleCloseProject called, isDirty:', currentIsDirty);
    if (currentIsDirty) {
      // Open styled dialog - the dialog will handle the actual close
      openDialog('unsaved-changes');
      return { success: false, error: 'Showing dialog' };
    }

    // [perf] Instrument the back path so we can see where the back-button
    // freeze lives. Surfaced in the MemoryProbe panel for visibility
    // without devtools. We measure four checkpoints to pinpoint the freeze:
    //   t0→t1  closeProject IPC roundtrip
    //   t1→t2  setView store update (synchronous)
    //   t2→t3  first rAF after setView (= React commit + layout queued)
    //   t3→t4  next rAF = AFTER first paint (browser had time to repaint)
    // If t3→t4 is huge, the freeze is in JSC GC / paint between commit and
    // visible update - the classic "old screen frozen, then snaps" symptom.
    const t0 = performance.now();
    // Concurrency guard is handled at store level (store.isClosing)
    const result = await closeProject();
    const t1 = performance.now();
    if (result.success) {
      setView('global');
    }
    const t2 = performance.now();
    const { recordPerfTiming } = await import('../utils/memoryProbe');
    requestAnimationFrame(() => {
      const t3 = performance.now();
      requestAnimationFrame(() => {
        const t4 = performance.now();
        const closeMs = t1 - t0;
        const toRafMs = t3 - t2;
        const rafToPaintMs = t4 - t3;
        const totalMs = t4 - t0;
        debugLog(
          `[perf:close] closeProject=${closeMs.toFixed(0)}ms  toRAF=${toRafMs.toFixed(0)}ms  rAF→paint=${rafToPaintMs.toFixed(0)}ms  total=${totalMs.toFixed(0)}ms`
        );
        recordPerfTiming('closeProject IPC', closeMs);
        recordPerfTiming('setView→firstRAF', toRafMs);
        recordPerfTiming('rAF→actual paint', rafToPaintMs);
        recordPerfTiming('back total', totalMs);
      });
    });
    return result;
  }, [closeProject, setView, openDialog]);

  /**
   * Unified "Open / Import": opens a picker accepting native projects and every
   * importable format, then routes the chosen file to the matching flow
   * (open `.stalx`, or import DMP/Ariane/Compass/GPX) based on its extension.
   */
  const handleOpenOrImport = useCallback(() => openOrImport(), []);

  return {
    project,
    stats,
    stations,
    recentProjects,
    isLoading: loading.isLoading,
    loading,
    canUndo,
    canRedo,

    handleNewProject,
    handleOpenOrImport,
    handleSaveProject,
    handleSaveProjectAs,
    handleCloseProject,

    undo,
    redo,
  };
}
