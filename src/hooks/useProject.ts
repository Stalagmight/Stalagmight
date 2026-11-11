import { dispatchProjectSave } from '../utils/dispatchProjectSave';
import i18n from '../i18n';
import { useCallback } from 'react';
import { useProjectStore } from '../store/projectStore';
import { useUIStore } from '../store/uiStore';
import { useShallow } from 'zustand/react/shallow';
import {
  ensureExtension,
  exportSaveFilters,
  saveDialog,
} from '../utils/fileDialogs';
import { openOrImport } from '../utils/openOrImport';
import { newProjectGuarded, saveProjectAs } from '../utils/projectActions';
import { debugLog } from '../utils/debugLog';
import { askSpeleoExit, openProjectNeedsSpeleoExitPrompt } from '../utils/speleoExit';
import { confirmWorkspaceDrafts } from '../utils/workspaceDrafts';

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
  const closeProject = useProjectStore((s) => s.closeProject);
  const exportCsv = useProjectStore((s) => s.exportCsv);
  const exportJson = useProjectStore((s) => s.exportJson);
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

  const handleSaveProject = dispatchProjectSave;

  const handleCloseProject = useCallback(async () => {
    // Unsent SpeleoDB input (an upload description, the conversion form) is asked
    // about first, before any question whose answer uploads, releases or closes.
    if (!await confirmWorkspaceDrafts()) return { success: false, error: 'Cancelled' };
    // Holding the SpeleoDB lock over work SpeleoDB has not seen: the exit prompt
    // decides (upload / keep the lock / discard) and closes the project itself.
    if (openProjectNeedsSpeleoExitPrompt()) {
      const closed = await askSpeleoExit('close');
      if (!closed) return { success: false, error: 'Cancelled' };
      setView('global');
      return { success: true };
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
    } else if (result.error !== 'Cancelled' && result.error !== 'Superseded' && result.error !== 'Already closing') {
      // A failed lock release has already asked "Couldn't hand back the lock"
      // (Cancel resolves 'Cancelled'); this is every other close failure.
      openDialog('error', { title: i18n.t('projectClose.failedTitle'), message: result.error || i18n.t('projectClose.failedMessage') });
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

  const handleExportCsv = useCallback(async () => {
    const selected = await saveDialog({
      filters: exportSaveFilters('CSV', 'csv'),
    });

    if (selected) {
      try {
        await exportCsv(ensureExtension(selected, 'csv'));
      } catch (e) {
        console.error('Failed to export CSV:', e);
      }
    }
  }, [exportCsv]);

  const handleExportJson = useCallback(async () => {
    const selected = await saveDialog({
      filters: exportSaveFilters('JSON', 'json'),
    });

    if (selected) {
      try {
        await exportJson(ensureExtension(selected, 'json'));
      } catch (e) {
        console.error('Failed to export JSON:', e);
      }
    }
  }, [exportJson]);

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
    handleExportCsv,
    handleExportJson,

    undo,
    redo,
  };
}
