/**
 * The two "measure, then decide" helpers behind the resize handle's double-click
 * gestures. Neither touches grid state (the measuring one only borrows the DOM
 * for a synchronous read), so they live outside the component file - which is
 * at its size ratchet.
 */
import type { GridColumn } from './types';

/**
 * Room a fitted column needs around its content: the `px-2` padding (8 px each
 * side) plus the 1 px `border-r` that header and body cells both carry inside
 * their border-box width, plus 1 px so content rounded up to a whole pixel can
 * never tip into its ellipsis.
 */
export const AUTOFIT_CHROME_PX = 18;

/**
 * The natural CONTENT width of a column: the widest of its header label and its
 * rendered cells' content, rounded up to a whole pixel. Returns 0 when nothing
 * was measurable. The column width is this plus {@link AUTOFIT_CHROME_PX}.
 *
 * It measures content, never the boxes holding it (D11, #576). A body cell is
 * a fixed-width overflow-hidden box its text clips to, and the header label
 * stretches across its header (flex-1), so the scrollWidth of either is the
 * column width itself: fitting to it added the padding allowance on every
 * double-click, growing the column ~17 px each time toward the cap. Each
 * measured element is instead laid out at `max-content` for the read - inline
 * styles set on all of them, read in one layout pass, restored before anything
 * paints - which yields what the content wants however the host draws it
 * (plain text, or a `render()` whose root is `w-full`: a percentage of a
 * max-content box resolves against its content).
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
  const targets: HTMLElement[] = [];
  const header = root.querySelector(`[data-testid="${testIdPrefix}-header-${key}"] span`);
  if (header instanceof HTMLElement) targets.push(header);
  root.querySelectorAll(`[data-testid^="${testIdPrefix}-cell-"]`).forEach((el) => {
    const cell = el as HTMLElement;
    // The cell's one child is the wrapper its text or render() output sits in.
    const content = cell.firstElementChild;
    if (cell.dataset.testid?.endsWith(`-${key}`) && content instanceof HTMLElement) targets.push(content);
  });
  return Math.ceil(maxContentWidth(targets));
}

/** The widest `max-content` width among `els`, leaving their styles as found. */
function maxContentWidth(els: HTMLElement[]): number {
  const saved = els.map((el) => el.getAttribute('style'));
  for (const el of els) {
    el.style.flex = 'none';
    el.style.width = 'max-content';
    el.style.minWidth = '0';
    el.style.maxWidth = 'none';
  }
  let max = 0;
  for (const el of els) max = Math.max(max, el.getBoundingClientRect().width);
  els.forEach((el, i) => {
    const style = saved[i];
    if (style == null) el.removeAttribute('style');
    else el.setAttribute('style', style);
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
