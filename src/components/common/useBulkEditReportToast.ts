import { useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import type { BulkEditReport } from './SpreadsheetGrid/types';

/**
 * The one toast every editable grid shows for a bulk edit it could not fully
 * apply - the grid's `onPasteReport`. A paste says what landed, what fell
 * outside the table, and what was skipped (read-only or invalid), sharing the
 * data table's `dataTable.pasteReport.*` strings; it is what turns a paste into a
 * cell the grid gates per row (the plan grids' usable-gas or DPV columns, frozen
 * shots) from a silent drop into "N skipped" (Codex P2 on #590).
 *
 * A fill (drag, fill-handle double-click, Ctrl+D / Ctrl+R, Fill down) reports
 * only values its target column refused, in its own words
 * (`dataTable.fillReport.*`) - #576 FILL-1. One hook for every grid, so they
 * cannot drift apart; `toastKey` is the grid's own dedup key, so a repeated
 * paste or fill replaces its notice instead of stacking.
 */
export function useBulkEditReportToast(toastKey: string) {
  const { t } = useTranslation();
  const showToast = useUIStore((s) => s.showToast);
  return useCallback(
    ({ source, written, clipped, skipped }: BulkEditReport) => {
      const strings = source === 'fill' ? 'dataTable.fillReport' : 'dataTable.pasteReport';
      const parts: string[] = [];
      if (written > 0) parts.push(t(`${strings}.written`, { count: written }));
      if (clipped > 0) parts.push(t('dataTable.pasteReport.clipped', { count: clipped }));
      if (skipped > 0) parts.push(t(`${strings}.skipped`, { count: skipped }));
      if (parts.length > 0) {
        const kind = clipped > 0 || skipped > 0 ? 'warning' : 'success';
        showToast(parts.join(' · '), toastKey, kind);
      }
    },
    [t, showToast, toastKey]
  );
}
