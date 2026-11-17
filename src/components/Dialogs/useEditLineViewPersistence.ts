/**
 * The edit-line dialog's slice of {@link useGridViewPersistence}.
 *
 * Its own `panels` fields rather than sharing the new-line dialog's. The two
 * grids now have the SAME columns, so the reason is not the column set - it is
 * that they are different jobs. Entering a brand-new line starts simple
 * (length, one azimuth, both depths); correcting a survey that already exists
 * opens on everything it has. One shared hidden list would collapse those into
 * each other, so narrowing the entry form would narrow the review form too.
 *
 * Do not merge them on the grounds that the defaults have converged.
 *
 * The IMPORT-time grids get a third slice for the same reason again: the wizard
 * and zero-datum edit shots that are not in the project yet, and sharing the
 * edit-line fields meant reshaping columns mid-import silently rewrote the
 * layout used to review a saved line.
 */
import {
  useGridViewPersistence,
  type GridViewSetters,
  type GridViewState,
} from './useGridViewPersistence';
import { TOAST_KEYS } from '../../utils/toastKeys';

export function useEditLineViewPersistence(
  view: GridViewState,
  setters: GridViewSetters,
  fallbackHidden: readonly string[]
): void {
  useGridViewPersistence(view, setters, {
    orderField: 'edit_line_column_order',
    hiddenField: 'edit_line_hidden_columns',
    widthsField: 'edit_line_column_widths',
    command: 'update_edit_line_view',
    fallbackHidden,
    toastKey: TOAST_KEYS.EDIT_LINE_VIEW_SAVE,
  });
}

/** The import-time shot grids (MNemo wizard, zero-datum config). */
export function useImportShotsViewPersistence(
  view: GridViewState,
  setters: GridViewSetters,
  fallbackHidden: readonly string[]
): void {
  useGridViewPersistence(view, setters, {
    orderField: 'import_shots_column_order',
    hiddenField: 'import_shots_hidden_columns',
    widthsField: 'import_shots_column_widths',
    command: 'update_import_shots_view',
    fallbackHidden,
    toastKey: TOAST_KEYS.IMPORT_SHOTS_VIEW_SAVE,
  });
}
