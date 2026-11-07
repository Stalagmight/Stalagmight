import type { StoreApi } from 'zustand';
import { invokeIpc } from '../utils/ipc';
import type { ProjectStore } from './projectStore';

/*
 * The loading state of a project load, create or file import - what the loading
 * modal and the import dialogs' progress line show - and its actions. A slice of
 * its own, like `projectLoadRepairsBanner.ts`, so `projectStore.ts` (at the
 * 1500-LOC ratchet) only spreads it.
 */

/**
 * Structured loading state for project load/create operations.
 * Tracks progress phases, current/total counts, and error states.
 */
export interface LoadingState {
  /**
   * Backend loading ID for filtering stale progress events.
   * Set from the first progress event or result.loadingId.
   * Value of 0 means "accept events until we learn the backend ID".
   */
  loadingId: number;
  /**
   * Client operation ID for race detection (local to frontend).
   * Incremented on each load start. Used to detect if a newer load started.
   */
  clientOpId: number;
  /** Whether a load operation is currently in progress */
  isLoading: boolean;
  /** Name of the project being loaded */
  projectName?: string | undefined;
  /** Path to the project file (for retry functionality) */
  projectPath?: string | undefined;
  /** Current phase enum value (snake_case from backend) */
  phase?: string | undefined;
  /** Index of current phase (0-based) */
  phaseIndex?: number | undefined;
  /** Total number of phases */
  phaseCount?: number | undefined;
  /** Current phase progress percentage (0-100), undefined for indeterminate */
  phaseProgress?: number | undefined;
  /** Current item count in phase */
  phaseCurrent?: number | undefined;
  /** Total items in phase */
  phaseTotal?: number | undefined;
  /** Overall loading progress percentage (0-100) */
  overallProgress: number;
  /** Error information if load failed */
  error?: { message: string; details?: string | undefined } | undefined;
  /** Timestamp of last event (for timeout detection) */
  lastEventTime?: number | undefined;
  /**
   * [#576 W44-NF3] The name the frontend gave the load, create or file import this
   * state is for (`utils/loadToken.ts`), sent with the command that started it. A
   * Cancel made for it names it, and the backend cancels nothing else.
   */
  loadToken?: string | undefined;
}

export const initialLoadingState: LoadingState = {
  loadingId: 0,
  clientOpId: 0,
  isLoading: false,
  overallProgress: 0,
};

// Separate client operation ID for race detection (independent of backend loading ID)
let clientOpIdCounter = 0;

export interface ProjectLoadingActions {
  /** Start a new loading operation, returns the loadingId. `loadToken`: the load's name, if it has one. */
  startLoading: (projectName: string, projectPath: string, loadToken?: string) => number;
  /** Update loading progress state */
  setLoadingProgress: (progress: Partial<LoadingState>) => void;
  /** Reset loading state to initial */
  resetLoading: () => void;
  /** Set loading error (keeps modal open) */
  setLoadingError: (message: string, details?: string) => void;
  /** Cancel the in-progress loading operation the loading state is for (#576 W44-NF3: by its name) */
  cancelLoading: () => Promise<void>;
}

export function createProjectLoadingActions(
  set: StoreApi<ProjectStore>['setState'],
  get: StoreApi<ProjectStore>['getState'],
): ProjectLoadingActions {
  return {
    startLoading: (projectName: string, projectPath: string, loadToken?: string) => {
      clientOpIdCounter += 1;
      const newClientOpId = clientOpIdCounter;
      set({
        loading: {
          ...initialLoadingState,
          loadingId: 0, // Will be set from first progress event or result
          clientOpId: newClientOpId,
          isLoading: true,
          projectName,
          projectPath,
          phase: 'opening_database',
          lastEventTime: Date.now(),
          loadToken,
        },
        autosaveFailure: null, // A new project's autosave state starts clean
        autosaveFailureKind: null,
      });
      return newClientOpId;
    },

    setLoadingProgress: (progress: Partial<LoadingState>) =>
      set((state) => ({
        loading: {
          ...state.loading,
          ...progress,
          lastEventTime: Date.now(),
        },
      })),

    resetLoading: () =>
      set((state) => ({
        loading: { ...initialLoadingState, clientOpId: state.loading.clientOpId },
      })),

    setLoadingError: (message: string, details?: string) =>
      set((state) => ({
        loading: {
          ...state.loading,
          isLoading: true, // Keep modal open for error display
          error: { message, details },
        },
      })),

    cancelLoading: async () => {
      // [#576 W44-NF3] Named: the load this state is for, read now. The IPC can run
      // after that load ended and a newer one started; it must not cancel that one.
      // No name (no load of the store's in progress): it cancels nothing.
      const { clientOpId, loadToken } = get().loading;
      // Safe to call even when no load is active - backend is idempotent
      try {
        await invokeIpc('cancel_loading', { loadToken: loadToken ?? null }, {
          purpose: 'cancel project loading',
        });
      } catch {
        // Ignore errors - might be called when no load active
      }
      // [#576 W50-NF3] Reset only the state of the load the Cancel named. Once a newer
      // load started (a new clientOpId) or an import named this one (a new loadToken),
      // the state is that load's progress, which this Cancel did not stop.
      const now = get().loading;
      if (now.clientOpId === clientOpId && now.loadToken === loadToken) get().resetLoading();
    },
  };
}
