import { create } from 'zustand';
import type { AdjustmentReport } from '../types/ipc/AdjustmentReport';
import type { Station } from '../types/project';
import { invokeIpc } from '../utils/ipc';

/**
 * What the Closures Computed View left out (#737): the loop closures it skips
 * because they span a single shot, and the parts of the survey whose solve
 * failed and are shown measured. Fetched right after the adjusted geometry
 * loads (`projectStore.loadAdjustedGeometry`), because the backend reports a
 * failure only from the solve it has just memoized.
 *
 * Kept beside, not inside, `projectStore`: the report belongs to ONE adjusted
 * geometry, so it records which (`forStations`) and consumers read it through
 * `useCurrentAdjustmentReport`, which drops it the moment `stationsAdjusted` is
 * replaced or cleared. That way it is cleared everywhere the adjusted geometry
 * is, without a second clear at each of those sites.
 */
export interface AdjustmentReportState {
  report: AdjustmentReport | null;
  /** The `stationsAdjusted` array the report was fetched for. */
  forStations: readonly Station[] | null;
  /**
   * Fetch the report for `forStations`. `onFailures` runs when it names a
   * failed part, so the Warnings tab (whose failure entry is read from the same
   * solve) can refresh.
   */
  load: (forStations: readonly Station[], onFailures?: () => Promise<void> | void) => Promise<void>;
  clear: () => void;
}

/** Bumped by every load and clear, so an answer that arrives late is dropped. */
let requestSeq = 0;

export const useAdjustmentReportStore = create<AdjustmentReportState>((set) => ({
  report: null,
  forStations: null,
  load: async (forStations, onFailures) => {
    const seq = ++requestSeq;
    try {
      const report = await invokeIpc<AdjustmentReport>('get_adjustment_report', undefined, {
        purpose: 'report what the closure-adjusted view left out',
      });
      if (seq !== requestSeq) return;
      set({ report, forStations });
      if (report.failures.length > 0) await onFailures?.();
    } catch (e) {
      if (seq !== requestSeq) return;
      console.warn('Failed to load the adjustment report:', e);
      set({ report: null, forStations: null });
    }
  },
  clear: () => {
    requestSeq += 1;
    set({ report: null, forStations: null });
  },
}));
