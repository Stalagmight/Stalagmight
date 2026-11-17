/**
 * The new-line dialog's slice of {@link useGridViewPersistence}.
 *
 * A thin wrapper rather than its own copy: the seed/snapshot/debounce/rollback
 * machinery is subtle enough that a second copy would be free to drift, and
 * there are now three grids persisting a column layout.
 */
import {
  useGridViewPersistence,
  type GridViewSetters,
  type GridViewState,
} from './useGridViewPersistence';
import { TOAST_KEYS } from '../../utils/toastKeys';

export type NewLineViewState = GridViewState;
export type NewLineViewSetters = GridViewSetters;

export function useNewLineViewPersistence(
  view: NewLineViewState,
  setters: NewLineViewSetters,
  /** Hidden set for a config that has never stored one. */
  fallbackHidden: readonly string[]
): void {
  useGridViewPersistence(view, setters, {
    orderField: 'new_line_column_order',
    hiddenField: 'new_line_hidden_columns',
    widthsField: 'new_line_column_widths',
    command: 'update_new_line_view',
    fallbackHidden,
    toastKey: TOAST_KEYS.NEW_LINE_VIEW_SAVE,
  });
}
