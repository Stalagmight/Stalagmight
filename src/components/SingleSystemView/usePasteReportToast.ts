import { useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';

/**
 * Paste-outcome toast for the Plan-mode grids, sharing the data table's
 * `dataTable.pasteReport.*` strings (same shape as `ShotTableEditor`'s inline
 * version). The plan grids gate cells per-row (the usable-gas column outside
 * Free strategy, the DPV column without a rig), and the grid drops a paste into
 * a read-only cell before the reducer ever sees it - this toast is what turns
 * that silent drop into "N skipped (read-only or invalid)" (Codex P2 on #590).
 */
export function usePasteReportToast(toastKey: string) {
  const { t } = useTranslation();
  const showToast = useUIStore((s) => s.showToast);
  return useCallback(
    ({ written, clipped, skipped }: { written: number; clipped: number; skipped: number }) => {
      const parts: string[] = [];
      if (written > 0) parts.push(t('dataTable.pasteReport.written', { count: written }));
      if (clipped > 0) parts.push(t('dataTable.pasteReport.clipped', { count: clipped }));
      if (skipped > 0) parts.push(t('dataTable.pasteReport.skipped', { count: skipped }));
      if (parts.length > 0) {
        const kind = clipped > 0 || skipped > 0 ? 'warning' : 'success';
        showToast(parts.join(' · '), toastKey, kind);
      }
    },
    [t, showToast, toastKey]
  );
}
