import { useAdjustmentReportStore } from '../store/adjustmentReportStore';
import { useProjectStore } from '../store/projectStore';
import type { AdjustmentReport } from '../types/ipc/AdjustmentReport';

/**
 * The adjustment report (#737) for the adjusted geometry on screen NOW, or
 * null: a report fetched for an earlier `stationsAdjusted` - or one whose
 * geometry has since been cleared by an edit, an undo or a project switch - is
 * not shown.
 */
export function useCurrentAdjustmentReport(): AdjustmentReport | null {
  const stationsAdjusted = useProjectStore((s) => s.stationsAdjusted);
  const report = useAdjustmentReportStore((s) => s.report);
  const forStations = useAdjustmentReportStore((s) => s.forStations);
  if (!report || !stationsAdjusted || forStations !== stationsAdjusted) return null;
  return report;
}
