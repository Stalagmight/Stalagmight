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
 * matched nothing in a grid that sets one - the new-line and shot-editor
 * grids - so the width stayed 0 and a double-click silently did nothing there
 * while working in the data table.
 */
export function measureColumnWidth(
  root: HTMLElement,
  testIdPrefix: string,
  key: string
): number {
  let max = 0;
  const header = root.querySelector(`[data-testid="${testIdPrefix}-header-${key}"] span`);
  if (header) max = Math.max(max, (header as HTMLElement).scrollWidth);
  root.querySelectorAll(`[data-testid^="${testIdPrefix}-cell-"]`).forEach((el) => {
    const node = el as HTMLElement;
    if (node.dataset.testid?.endsWith(`-${key}`)) max = Math.max(max, node.scrollWidth);
  });
  return max;
}

/**
 * How far a Google-Sheets style auto-fill should reach: down to the data extent
 * of the guide column, stopping at its first empty cell. Returns `from` when
 * there is nothing below to fill into.
 */
export function fillExtent<Row>(
  guide: GridColumn<Row>,
  rows: readonly Row[],
  from: number,
  rowCount: number
): number {
  let end = from;
  for (let r = from + 1; r < rowCount; r++) {
    const data = rows[r];
    if (!data) break;
    const v = guide.getValue(data, r);
    if (v == null || v === '') break;
    end = r;
  }
  return end;
}
