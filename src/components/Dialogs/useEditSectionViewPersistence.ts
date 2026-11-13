/**
 * The edit-section dialog's slice of {@link useGridViewPersistence}.
 *
 * Its own `panels` fields rather than sharing the new-section dialog's. The two
 * grids now have the SAME columns, so the reason is not the column set - it is
 * that they are different jobs. Entering a brand-new section starts simple
 * (length, one azimuth, both depths); correcting a survey that already exists
 * opens on everything it has. One shared hidden list would collapse those into
 * each other, so narrowing the entry form would narrow the review form too.
 *
 * Do not merge them on the grounds that the defaults have converged.
 *
 * The IMPORT-time grids get a third slice for the same reason again: the wizard
 * and zero-datum edit shots that are not in the project yet, and sharing the
 * edit-section fields meant reshaping columns mid-import silently rewrote the
 * layout used to review a saved section.
 */
import {
  useGridViewPersistence,
  type GridViewSetters,
  type GridViewState,
} from './useGridViewPersistence';
import { TOAST_KEYS } from '../../utils/toastKeys';

export function useEditSectionViewPersistence(
  view: GridViewState,
  setters: GridViewSetters,
  fallbackHidden: readonly string[]
): void {
  useGridViewPersistence(view, setters, {
    orderField: 'edit_section_column_order',
    hiddenField: 'edit_section_hidden_columns',
    widthsField: 'edit_section_column_widths',
    command: 'update_edit_section_view',
    fallbackHidden,
    toastKey: TOAST_KEYS.EDIT_SECTION_VIEW_SAVE,
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
