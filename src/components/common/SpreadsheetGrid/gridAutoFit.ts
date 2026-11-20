/**
 * The two "measure, then decide" helpers behind the resize handle's double-click
 * gestures. Both are pure given their inputs, and neither touches grid state, so
 * they live outside the component file - which is at its size ratchet.
 */
import type { GridColumn } from './types';

/**
 * The natural width of a column: the widest of its header and its rendered
 * cells, plus padding, clamped. Returns 0 when nothing was measurable.
 *
 * Selectors are built from the grid's OWN `testIdPrefix`. Hardcoding `grid-`
 * matched nothing in a grid that sets one - the new-section and shot-editor
 * grids - so the width stayed 0 and a double-click silently did nothing there
 * while working in the data table.
 */
export function measureColumnWidth(
  root: HTMLElement,
  testIdPrefix: string,
  key: string
): number {
  let max = 0;
  const headerCell = root.querySelector(`[data-testid="${testIdPrefix}-header-${key}"]`);
  const header = headerCell?.querySelector('span');
  // A sort button sits after the label and must still fit beside it.
  const sortBtn = headerCell?.querySelector<HTMLElement>('.sg-sort-btn, .sg-sort-indicator');
  if (header) max = Math.max(max, header.scrollWidth + (sortBtn ? sortBtn.offsetWidth + 4 : 0));
  root.querySelectorAll(`[data-testid^="${testIdPrefix}-cell-"]`).forEach((el) => {
    const node = el as HTMLElement;
    if (node.dataset.testid?.endsWith(`-${key}`)) max = Math.max(max, node.scrollWidth);
  });
  return max;
}

/**
 * How far a Google-Sheets style auto-fill should reach: down to the data extent
 * of the guide column, stopping at its first empty cell. Returns `from` when
 * there is nothing below to fill into. A guide cell that does not apply to its
 * row (`applies` false) is passed over, neither data nor a gap, so on a grid
 * that interleaves row kinds the extent runs through the other kind's rows.
 */
export function fillExtent<Row>(
  guide: GridColumn<Row>,
  rows: readonly Row[],
  from: number,
  rowCount: number,
  applies: (rowIdx: number) => boolean = () => true
): number {
  let end = from;
  for (let r = from + 1; r < rowCount; r++) {
    const data = rows[r];
    if (!data) break;
    if (!applies(r)) continue;
    const v = guide.getValue(data, r);
    if (v == null || v === '') break;
    end = r;
  }
  return end;
}
