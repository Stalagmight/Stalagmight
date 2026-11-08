import type { StoreApi } from 'zustand';
import type { MergeSourceRepairs } from '../types/ipc.generated';
import { loadRepairsFromEnvelope, type LoadRepairs } from '../utils/loadProjectBinary';
import { extractFilename } from '../utils/path';
import type { ProjectStore } from './projectStore';

/**
 * [#576 B35(h)] The load-repairs notice: what opening the project reset or dropped
 * because it could not use it (non-finite shot values, section and station moves,
 * GPS tie points), so a damaged file cannot change the user's data silently. The
 * backend counts it per kind on the load result; App.tsx shows ONE banner after the
 * open, with a Dismiss - like the discarded-history banner (#473), which reports
 * the same class of load-time loss.
 *
 * [#576 W57-NF2] A merge reads each source the same way, and the merged file holds
 * what that read kept, so the same notice lists what it reset or dropped, named by
 * source (`showMergeSourceRepairs`).
 *
 * A slice of its own, like `projectHistoryBannerActions.ts`, so `projectStore.ts`
 * (at the 1500-LOC ratchet) only spreads it: `loadRepairsStateFor` on load,
 * `LOAD_REPAIRS_CLEARED` on create and close, and the action factory.
 */
export interface ProjectLoadRepairsState {
  /** What the load of the OPEN project reset or dropped, or null when nothing. */
  loadRepairs: LoadRepairs | null;
  /** [#576 W57-NF2] What the last merge's read of each source reset or dropped,
   *  one entry per such source; empty when nothing. */
  mergeSourceRepairs: MergeSourceRepairsNotice[];
  /** True once the user dismissed the notice for the open project. */
  loadRepairsDismissed: boolean;
}

/** [#576 W57-NF2] One merge source's line in the notice. */
export interface MergeSourceRepairsNotice {
  /** The source project's name. */
  name: string;
  /** The source file's name, so two sources with one project name stay apart. */
  fileName: string;
  repairs: LoadRepairs;
}

export interface ProjectLoadRepairsActions {
  /** Hide the load-repairs notice for the open project. */
  dismissLoadRepairs: () => void;
  /** [#576 W57-NF2] Show what a merge's read of its sources reset or dropped (the
   *  merge's `source_repairs`). Called after the merged project is opened, if it
   *  is, since a load replaces the notice. Nothing to show leaves it as it is. */
  showMergeSourceRepairs: (sources: MergeSourceRepairs[]) => void;
}

/** The notice's cleared state as ONE spreadable value (the HISTORY_BANNER_CLEARED
 *  pattern): every reset site spreads it, so a new one cannot forget a field. */
export const LOAD_REPAIRS_CLEARED: ProjectLoadRepairsState = {
  loadRepairs: null,
  mergeSourceRepairs: [],
  loadRepairsDismissed: false,
};

/** The notice's state for a fresh load: what THAT load reset (an undefined from a
 *  result without the field reads as nothing), not yet dismissed. */
export function loadRepairsStateFor(loadRepairs: LoadRepairs | null | undefined): ProjectLoadRepairsState {
  return { loadRepairs: loadRepairs ?? null, mergeSourceRepairs: [], loadRepairsDismissed: false };
}

export function createProjectLoadRepairsActions(
  set: StoreApi<ProjectStore>['setState'],
): ProjectLoadRepairsActions {
  return {
    dismissLoadRepairs: () => set({ loadRepairsDismissed: true }),
    showMergeSourceRepairs: (sources) => {
      const notices = sources.flatMap((source) => {
        // The same per-kind counts, and the same decode, as an open's receipt.
        const repairs = loadRepairsFromEnvelope(source.repairs);
        return repairs ? [{ name: source.name, fileName: extractFilename(source.path), repairs }] : [];
      });
      if (notices.length === 0) return;
      set((state) => ({
        mergeSourceRepairs: notices,
        // The open project's own line stays hidden once the user dismissed it.
        loadRepairs: state.loadRepairsDismissed ? null : state.loadRepairs,
        loadRepairsDismissed: false,
      }));
    },
  };
}
