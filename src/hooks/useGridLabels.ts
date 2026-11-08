/**
 * Localized chrome strings for SpreadsheetGrid.
 *
 * The grid is deliberately i18next-free (see `gridLabels.ts`), so every host has
 * to hand it a `t()`-backed `GridLabels`. Extracted from DataTable once a second
 * host appeared (the new-section dialog): the strings are identical everywhere
 * except the domain row noun, which is what makes a context menu say "Delete
 * shot" rather than "Delete row".
 */
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import type { GridLabels } from '../components/common/SpreadsheetGrid';

/** i18n key for the row noun, taking a `count` for pluralization. */
export type RowNounKey =
  | 'dataTable.shotNoun'
  | 'dataTable.stationNoun'
  | 'gasPanel.segments.segmentNoun'
  | 'gasPanel.tanks.tankNoun';

export function useGridLabels(rowNounKey: RowNounKey): GridLabels {
  const { t } = useTranslation();
  return useMemo(
    () => ({
      copy: (suffix) => t('grid.contextMenu.copy', { suffix }),
      cut: (suffix) => t('grid.contextMenu.cut', { suffix }),
      clear: (suffix) => t('grid.contextMenu.clear', { suffix }),
      paste: t('grid.contextMenu.paste'),
      fillDown: t('grid.contextMenu.fillDown'),
      insertAbove: (rows) => t('grid.contextMenu.insertAbove', { rows }),
      insertBelow: (rows) => t('grid.contextMenu.insertBelow', { rows }),
      deleteRows: (rows) => t('grid.contextMenu.deleteRows', { rows }),
      rowNoun: (count) => t(rowNounKey, { count }),
      columnNoun: (count) => t('grid.columnNoun', { count }),
      editCellAria: (column, row) => t('grid.editCellAria', { column, row }),
      shortcutCopy: t('grid.shortcuts.copy'),
      shortcutCut: t('grid.shortcuts.cut'),
      shortcutPaste: t('grid.shortcuts.paste'),
      shortcutClear: t('grid.shortcuts.clear'),
      shortcutFillDown: t('grid.shortcuts.fillDown'),
    }),
    [t, rowNounKey]
  );
}
