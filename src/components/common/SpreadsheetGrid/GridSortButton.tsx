/**
 * The header's sort affordance: a small chevron button after the column label.
 *
 * Hidden until the header is hovered (or the button is focused), and always
 * shown, with its direction, on the sorted column. A click cycles
 * asc -> desc -> none through the host's `onSortChange`; the grid never sorts
 * rows itself. Mousedown is swallowed so pressing it never selects, drags,
 * or steals keyboard focus from the grid, and its keydown is kept from the grid
 * so Enter/Space press the button instead of moving the active cell.
 */
import { ChevronDownIcon, ChevronUpIcon } from '../icons';
import type { GridLabels } from './gridLabels';

export type SortDir = 'asc' | 'desc';

/** asc -> desc -> none -> asc. */
export function nextSortDir(dir: SortDir | null): SortDir | null {
  return dir === null ? 'asc' : dir === 'asc' ? 'desc' : null;
}

export function GridSortButton({
  colKey,
  header,
  dir,
  testIdPrefix,
  labels,
  onSortChange,
}: {
  colKey: string;
  header: string;
  /** This column's current direction, or null when another / no column sorts. */
  dir: SortDir | null;
  testIdPrefix: string;
  labels: Pick<GridLabels, 'sortAscending' | 'sortDescending' | 'sortClear'>;
  onSortChange: (colKey: string, dir: SortDir | null) => void;
}) {
  const next = nextSortDir(dir);
  const label =
    next === 'asc' ? labels.sortAscending(header) : next === 'desc' ? labels.sortDescending(header) : labels.sortClear(header);
  const Icon = dir === 'desc' ? ChevronDownIcon : ChevronUpIcon;
  return (
    <button
      type="button"
      className={`sg-sort-btn${dir ? ' sg-sort-active' : ''}`}
      data-testid={`${testIdPrefix}-sort-${colKey}`}
      data-sort-dir={dir ?? 'none'}
      aria-label={label}
      title={label}
      onMouseDown={(e) => {
        e.stopPropagation();
        e.preventDefault();
      }}
      onDoubleClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => e.stopPropagation()}
      onClick={(e) => {
        e.stopPropagation();
        onSortChange(colKey, next);
      }}
    >
      <Icon size={14} />
    </button>
  );
}
