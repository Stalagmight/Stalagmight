import type { StoreApi } from 'zustand';
import { invokeIpc } from '../utils/ipc';
import type { ProjectWarning } from '../types/commands';
import { useSpeleoStore } from './speleoStore';
import type { ProjectStore } from './projectStore';

/**
 * The validation warnings and their ignores (resolve / unresolve).
 *
 * Split out of `projectStore.ts` (held by the TS file-size ratchet) at the API
 * boundary, the way `projectHistoryBannerActions.ts` is.
 *
 * On the station-graph model an ignore is survey content (plan 6.5): the
 * backend keeps it in memory, marks the project dirty and writes it with the
 * next save, with no undo entry. It used to be written into the file at once,
 * so the frontend never dirtied the project for one. Now it must, or autosave
 * would not save the ignore and the exit prompt would let "Don't save" pass
 * without asking. The backend's own flag (`has_unsaved_changes`) is the
 * authority; asking it covers a resolve and unresolve that cancel out too.
 *
 * An ignore also moves `projectTreeGeneration`, as every edit does: a save in
 * flight clears `isDirty` only when the tree is the one it was issued for, so
 * an ignore that reached the backend after that save stays unsaved here too.
 */
export interface ProjectWarningActions {
  refreshWarnings: () => Promise<void>;
  resolveWarning: (lineId: number, shotId: number, warningCode: string, comment?: string) => Promise<boolean>;
  unresolveWarning: (lineId: number, shotId: number, warningCode: string) => Promise<void>;
}

export function createProjectWarningActions(
  set: StoreApi<ProjectStore>['setState'],
  get: StoreApi<ProjectStore>['getState'],
  nextTreeGeneration: () => number,
): ProjectWarningActions {
  /** Follow the backend's dirty flag after an ignore changed (see above). */
  const syncDirtyAfterIgnore = async (): Promise<void> => {
    const issuedFor = get().project?.created_at ?? null;
    if (issuedFor !== null) set({ projectTreeGeneration: nextTreeGeneration() });
    try {
      const dirty = await invokeIpc<boolean>('has_unsaved_changes', undefined, {
        purpose: 'keep the unsaved-changes state in step after a warning ignore',
      });
      // Only ever SET it from here: a cleared backend flag says nothing about
      // edits the frontend is still tracking (a save in flight clears both).
      if (dirty && (get().project?.created_at ?? null) === issuedFor) set({ isDirty: true });
    } catch (e) {
      // The ignore itself landed; failing safe means assuming it is unsaved.
      console.error('Failed to read the unsaved-changes flag:', e);
      if ((get().project?.created_at ?? null) === issuedFor) set({ isDirty: true });
    }
  };

  return {
    refreshWarnings: async () => {
      try {
        const warnings = await invokeIpc<ProjectWarning[]>('get_project_warnings', undefined, {
          purpose: 'refresh validation warnings',
        });
        set({ warnings });
      } catch (e) {
        console.error('Failed to refresh warnings:', e);
        set({ warnings: [] });
      }
    },

    resolveWarning: async (lineId, shotId, warningCode, comment) => {
      try {
        await invokeIpc(
          'resolve_warning',
          { lineId, shotId, warningCode, comment },
          { purpose: 'acknowledge validation warning', fields: { lineId, shotId, warningCode } },
        );
        useSpeleoStore.getState().markForwardEdit();
        await syncDirtyAfterIgnore();
        await get().refreshWarnings();
        return true;
      } catch (e) {
        // Surface failure to the caller (e.g. the shot no longer exists) so the
        // resolve flow doesn't report a suppression that never persisted (#57).
        console.error('Failed to resolve warning:', e);
        return false;
      }
    },

    unresolveWarning: async (lineId, shotId, warningCode) => {
      try {
        await invokeIpc(
          'unresolve_warning',
          { lineId, shotId, warningCode },
          { purpose: 'reopen validation warning', fields: { lineId, shotId, warningCode } },
        );
        useSpeleoStore.getState().markForwardEdit();
        await syncDirtyAfterIgnore();
        await get().refreshWarnings();
      } catch (e) {
        console.error('Failed to unresolve warning:', e);
      }
    },
  };
}
