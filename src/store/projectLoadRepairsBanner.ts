import type { StoreApi } from 'zustand';
import type { LoadRepairs } from '../utils/loadProjectBinary';
import type { ProjectStore } from './projectStore';

/**
 * [#576 B35(h)] The load-repairs notice: what opening the project reset or dropped
 * because it could not use it (non-finite shot values, section and station moves,
 * GPS tie points), so a damaged file cannot change the user's data silently. The
 * backend counts it per kind on the load result; App.tsx shows ONE banner after the
 * open, with a Dismiss - like the discarded-history banner (#473), which reports
 * the same class of load-time loss.
 *
 * A slice of its own, like `projectHistoryBannerActions.ts`, so `projectStore.ts`
 * (at the 1500-LOC ratchet) only spreads it: `loadRepairsStateFor` on load,
 * `LOAD_REPAIRS_CLEARED` on create and close, and the action factory.
 */
export interface ProjectLoadRepairsState {
  /** What the load of the OPEN project reset or dropped, or null when nothing. */
  loadRepairs: LoadRepairs | null;
  /** True once the user dismissed the notice for the open project. */
  loadRepairsDismissed: boolean;
}

export interface ProjectLoadRepairsActions {
  /** Hide the load-repairs notice for the open project. */
  dismissLoadRepairs: () => void;
}

/** The notice's cleared state as ONE spreadable value (the HISTORY_BANNER_CLEARED
 *  pattern): every reset site spreads it, so a new one cannot forget a field. */
export const LOAD_REPAIRS_CLEARED: ProjectLoadRepairsState = {
  loadRepairs: null,
  loadRepairsDismissed: false,
};

/** The notice's state for a fresh load: what THAT load reset (an undefined from a
 *  result without the field reads as nothing), not yet dismissed. */
export function loadRepairsStateFor(loadRepairs: LoadRepairs | null | undefined): ProjectLoadRepairsState {
  return { loadRepairs: loadRepairs ?? null, loadRepairsDismissed: false };
}

export function createProjectLoadRepairsActions(
  set: StoreApi<ProjectStore>['setState'],
): ProjectLoadRepairsActions {
  return {
    dismissLoadRepairs: () => set({ loadRepairsDismissed: true }),
  };
}
