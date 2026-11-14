/**
 * The Data workspace's layout props: `headerCorner`, `spanCells`,
 * `groupContentStart: 'gutter'` + `groupContentInset`, and
 * `constrainColumnOrder`. The pure geometry behind them is in
 * gridWorkspaceLayout.test.ts; this file checks the wiring in the real grid.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';

vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () =>
      Array.from({ length: count }, (_, index) => ({ index, key: index, start: 33 + index * 33, size: 33 })),
    getTotalSize: () => count * 33,
    scrollToIndex: vi.fn(),
  }),
}));

import { SpreadsheetGrid, type SpreadsheetGridProps } from '../SpreadsheetGrid';
import type { GridColumn } from '../types';

interface Row {
  id: number;
  kind: 'section' | 'station' | 'shot';
  name: string;
}

const G = 48; // default gutter
// a: every row; b, c: shots only (N/A on stations, which span them); d: every row.
const shotOnly = (r: Row) => r.kind === 'shot';
const COLUMNS: GridColumn<Row>[] = [
  { key: 'a', header: 'A', width: 100, kind: 'text', editable: true, getValue: (r) => r.name },
  { key: 'b', header: 'B', width: 80, kind: 'text', editable: true, isApplicable: shotOnly, getValue: (r) => `b${r.id}` },
  { key: 'c', header: 'C', width: 60, kind: 'text', editable: true, isApplicable: shotOnly, getValue: (r) => `c${r.id}` },
  { key: 'd', header: 'D', width: 50, kind: 'text', editable: true, getValue: (r) => `d${r.id}` },
];
const ROWS: Row[] = [
  { id: 0, kind: 'section', name: 'S1' },
  { id: 1, kind: 'station', name: 'st1' },
  { id: 2, kind: 'shot', name: 'sh1' },
  { id: 3, kind: 'station', name: 'st2' },
  ...Array.from({ length: 26 }, (_, i): Row => ({ id: 4 + i, kind: 'shot', name: `sh${i + 2}` })),
];

type Props = SpreadsheetGridProps<Row>;
const base = (over: Partial<Props> = {}): Props => ({
  columns: COLUMNS,
  rows: ROWS,
  rowKey: (r) => r.id,
  onEdit: vi.fn(),
  height: 400,
  ...over,
});
const scroller = () => document.querySelector('.sg-grid [tabindex="0"]') as HTMLElement;
const root = () => document.querySelector('.sg-grid') as HTMLElement;
const cell = (r: number, k: string) => screen.getByTestId(`grid-cell-${r}-${k}`);
const rectOf = () => screen.getByTestId('grid-selection-overlay').getAttribute('data-rect');
const activeCell = () => document.querySelector('[data-active-cell="true"]')?.getAttribute('data-testid');
const scrollTo = (top: number) => {
  Object.defineProperty(scroller(), 'scrollTop', { value: top, configurable: true, writable: true });
  fireEvent.scroll(scroller());
};
const sectionContent = (r: Row) =>
  r.kind === 'section' ? (
    <span>
      {`Section ${r.name}`}
      <button type="button" data-testid="sec-toggle" onMouseDown={(e) => e.stopPropagation()} onClick={(e) => { e.stopPropagation(); onToggle(); }}>
        v
      </button>
    </span>
  ) : undefined;
const onToggle = vi.fn();

afterEach(() => {
  onToggle.mockClear();
  vi.unstubAllGlobals();
});

describe('headerCorner', () => {
  it('renders the host content in the corner, left-aligned, with its text as the tooltip', () => {
    render(<SpreadsheetGrid<Row> {...base({ headerCorner: (i) => <b>{`row ${i}`}</b> })} />);
    const corner = screen.getByTestId('grid-header-corner');
    expect(corner.closest('.sg-corner')).not.toBeNull();
    expect(corner).toHaveClass('sg-corner-content');
    expect(corner).toHaveTextContent('row 0');
    expect(corner).toHaveAttribute('title', 'row 0');
    expect(corner).toHaveAttribute('data-first-visible-row', '0');
  });

  it('follows the first row whose top is at or below the header bottom', () => {
    render(<SpreadsheetGrid<Row> {...base({ headerCorner: (i) => `row ${i}` })} />);
    const corner = () => screen.getByTestId('grid-header-corner');
    scrollTo(5 * 33);
    expect(corner()).toHaveTextContent('row 5');
    expect(corner()).toHaveAttribute('title', 'row 5');
    scrollTo(5 * 33 + 1); // row 5's top has passed under the header
    expect(corner()).toHaveTextContent('row 6');
    scrollTo(0);
    expect(corner()).toHaveTextContent('row 0');
  });

  it('scrolling re-renders only the corner, and only when the index changes', () => {
    const headerCorner = vi.fn((i: number) => `row ${i}`);
    const rowClassName = vi.fn(() => undefined);
    render(<SpreadsheetGrid<Row> {...base({ headerCorner, rowClassName })} />);
    headerCorner.mockClear();
    rowClassName.mockClear();
    scrollTo(3 * 33);
    expect(headerCorner.mock.calls).toEqual([[3]]);
    scrollTo(3 * 33 - 10); // still row 3
    scrollTo(3 * 33 - 20);
    expect(headerCorner).toHaveBeenCalledTimes(1);
    expect(rowClassName).not.toHaveBeenCalled();
  });

  it('gets -1 with no rows; without the prop the corner stays empty', () => {
    const { unmount } = render(<SpreadsheetGrid<Row> {...base({ rows: [], headerCorner: (i) => `row ${i}` })} />);
    expect(screen.getByTestId('grid-header-corner')).toHaveTextContent('row -1');
    unmount();
    render(<SpreadsheetGrid<Row> {...base()} />);
    expect(screen.queryByTestId('grid-header-corner')).toBeNull();
    expect(document.querySelector('.sg-corner')?.childElementCount).toBe(0);
  });
});

describe('spanCells', () => {
  const onChip = vi.fn();
  const spans = (from = 'b', to = 'c') => (r: Row) =>
    r.kind === 'station'
      ? {
          from,
          to,
          content: (
            <button type="button" data-testid={`chip-${r.id}`} onMouseDown={(e) => e.stopPropagation()} onClick={(e) => { e.stopPropagation(); onChip(r.id); }}>
              link
            </button>
          ),
        }
      : undefined;
  const span = (i: number) => screen.getByTestId(`grid-span-${i}`);

  it('draws one box over the run, between the cells and the selection overlay, letting the pointer through', () => {
    render(<SpreadsheetGrid<Row> {...base({ spanCells: spans() })} />);
    const s = span(1);
    expect(s.parentElement).toBe(screen.getByTestId('grid-row-1'));
    expect(s.style.left).toBe(`${G + 100}px`);
    expect(s.style.width).toBe('140px');
    expect(s.style.pointerEvents).toBe('none');
    expect(s.style.overflow).toBe('hidden');
    expect(Number(s.style.zIndex)).toBeGreaterThan(0);
    expect(Number(s.style.zIndex)).toBeLessThan(Number(screen.getByTestId('grid-selection-overlay').style.zIndex));
    expect(s).toHaveAttribute('data-span-cols', '1,2');
    // Only the rows the host spans; never a shot or the section band.
    expect(screen.queryByTestId('grid-span-2')).toBeNull();
    expect(screen.queryByTestId('grid-span-0')).toBeNull();
    // The covered cells are still there, N/A as before.
    expect(cell(1, 'b')).toHaveClass('sg-cell-na');
  });

  it('measures over the visible columns: hidden ends clamp, reversed ends swap, nothing visible draws nothing', () => {
    const { rerender } = render(<SpreadsheetGrid<Row> {...base({ spanCells: spans('b', 'd'), hiddenColumns: ['b'] })} />);
    // b hidden: the run starts at c (now right after a).
    expect(span(1).style.left).toBe(`${G + 100}px`);
    expect(span(1).style.width).toBe('110px');
    rerender(<SpreadsheetGrid<Row> {...base({ spanCells: spans('d', 'b') })} />);
    expect(span(1).style.left).toBe(`${G + 100}px`);
    expect(span(1).style.width).toBe('190px');
    rerender(<SpreadsheetGrid<Row> {...base({ spanCells: spans('b', 'c'), hiddenColumns: ['b', 'c'] })} />);
    expect(screen.queryByTestId('grid-span-1')).toBeNull();
    rerender(<SpreadsheetGrid<Row> {...base({ spanCells: spans('b', 'nope') })} />);
    expect(screen.queryByTestId('grid-span-1')).toBeNull();
  });

  it('follows reorders and resizes', () => {
    const spanCells = spans('b', 'c');
    const { rerender } = render(<SpreadsheetGrid<Row> {...base({ spanCells, columnOrder: ['c', 'a', 'b', 'd'] })} />);
    // Display order c, a, b: the run from b to c (reversed on screen) covers all three.
    expect(span(1).style.left).toBe(`${G}px`);
    expect(span(1).style.width).toBe(`${60 + 100 + 80}px`);
    rerender(<SpreadsheetGrid<Row> {...base({ spanCells, columnWidths: { a: 150 } })} />);
    expect(span(1).style.left).toBe(`${G + 150}px`);
    expect(span(1).style.width).toBe('140px');
  });

  it('clicks reach the cells under the span; Tab still skips the covered N/A cells', () => {
    render(<SpreadsheetGrid<Row> {...base({ spanCells: spans() })} />);
    fireEvent.mouseDown(cell(1, 'c'));
    fireEvent.mouseUp(window);
    expect(rectOf()).toBe('1,2,1,2');
    fireEvent.mouseDown(cell(3, 'b'), { shiftKey: true });
    fireEvent.mouseUp(window);
    expect(rectOf()).toBe('1,1,3,2');
    fireEvent.mouseDown(cell(1, 'a'));
    fireEvent.mouseUp(window);
    fireEvent.keyDown(scroller(), { key: 'Tab' });
    expect(activeCell()).toBe('grid-cell-1-d');
  });

  it('an interactive child of the span takes its own click and leaves the selection alone', () => {
    render(<SpreadsheetGrid<Row> {...base({ spanCells: spans() })} />);
    fireEvent.mouseDown(cell(2, 'a'));
    fireEvent.mouseUp(window);
    const chip = screen.getByTestId('chip-3');
    expect(chip.closest('.sg-span')).toBe(span(3));
    fireEvent.mouseDown(chip);
    fireEvent.mouseUp(window);
    fireEvent.click(chip);
    expect(onChip).toHaveBeenCalledWith(3);
    expect(rectOf()).toBe('2,0,2,0');
  });

  it('memoized rows: same props call nothing, a new spanCells re-renders the spans', () => {
    const first = vi.fn(spans());
    const props = base({ spanCells: first });
    const { rerender } = render(<SpreadsheetGrid<Row> {...props} />);
    first.mockClear();
    rerender(<SpreadsheetGrid<Row> {...props} />);
    expect(first).not.toHaveBeenCalled();
    rerender(<SpreadsheetGrid<Row> {...base({ spanCells: spans('a', 'a') })} />);
    expect(span(1).style.left).toBe(`${G}px`);
    expect(span(1).style.width).toBe('100px');
  });
});

describe("groupContentStart: 'gutter'", () => {
  const grid = (over: Partial<Props> = {}) =>
    base({ groupRowContent: sectionContent, groupContentStart: 'gutter', groupContentInset: 12, rowLabel: (r) => `L${r.id}`, ...over });

  it('puts the content inside the gutter at the inset, over the gutter label', () => {
    render(<SpreadsheetGrid<Row> {...grid()} />);
    const content = screen.getByTestId('grid-group-content-0');
    const gutter = screen.getByTestId('grid-gutter-0');
    expect(content.parentElement).toBe(gutter);
    expect(content).toHaveClass('sg-group-content-gutter');
    expect(content.style.left).toBe('12px');
    expect(content).toHaveTextContent('Section S1');
    expect(gutter).toHaveTextContent('L0');
    expect(screen.getByTestId('grid-group-0')).not.toHaveTextContent('Section S1');
  });

  it("the default 'band' keeps the content in the band, just right of the gutter", () => {
    render(<SpreadsheetGrid<Row> {...base({ groupRowContent: sectionContent })} />);
    const content = screen.getByTestId('grid-group-content-0');
    expect(content.parentElement).toBe(screen.getByTestId('grid-group-0'));
    expect(content).not.toHaveClass('sg-group-content-gutter');
    expect(content.style.left).toBe(`${G}px`);
  });

  it('a click, hover or right-click on the content acts like one on the gutter', () => {
    const onActiveCellChange = vi.fn();
    const onRowHover = vi.fn();
    render(<SpreadsheetGrid<Row> {...grid({ onActiveCellChange, onRowHover })} />);
    fireEvent.mouseDown(cell(2, 'a'));
    fireEvent.mouseUp(window);
    const content = screen.getByTestId('grid-group-content-0');
    fireEvent.mouseEnter(content);
    expect(onRowHover).toHaveBeenLastCalledWith(0);
    fireEvent.mouseDown(content);
    fireEvent.mouseUp(window);
    expect(rectOf()).toBe('0,0,0,3');
    expect(onActiveCellChange).toHaveBeenLastCalledWith(0, 'a');
    expect(screen.getByTestId('grid-selection-overlay')).toHaveAttribute('data-band-focus', 'true');
    fireEvent.mouseDown(cell(2, 'a'));
    fireEvent.mouseUp(window);
    fireEvent.contextMenu(content);
    expect(screen.getByTestId('grid-context-menu')).toBeInTheDocument();
    expect(rectOf()).toBe('0,0,0,3');
  });

  it('interactive children keep their own clicks', () => {
    render(<SpreadsheetGrid<Row> {...grid()} />);
    fireEvent.mouseDown(cell(2, 'a'));
    fireEvent.mouseUp(window);
    fireEvent.mouseDown(screen.getByTestId('sec-toggle'));
    fireEvent.click(screen.getByTestId('sec-toggle'));
    expect(onToggle).toHaveBeenCalledTimes(1);
    expect(rectOf()).toBe('2,0,2,0');
  });

  it('caps the content at the measured viewport, so it never reaches past it', () => {
    vi.stubGlobal('ResizeObserver', class { observe() {} disconnect() {} });
    const desc = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientWidth');
    Object.defineProperty(HTMLElement.prototype, 'clientWidth', { configurable: true, get: () => 500 });
    try {
      render(<SpreadsheetGrid<Row> {...grid()} />);
      expect(screen.getByTestId('grid-group-content-0').style.maxWidth).toBe('488px');
    } finally {
      if (desc) Object.defineProperty(HTMLElement.prototype, 'clientWidth', desc);
    }
  });
});

describe('constrainColumnOrder', () => {
  /** Column `a` is pinned first. */
  const pinA = (order: string[]) => ['a', ...order.filter((k) => k !== 'a')];
  const dropLine = () => document.querySelector('.sg-drop-line') as HTMLElement | null;
  /** Select a column, then press it again and drag to content x `toX`. */
  const dragHeader = (key: string, fromX: number, toX: number) => {
    const header = screen.getByTestId(`grid-header-${key}`);
    fireEvent.mouseDown(header, { clientX: fromX, clientY: 10 });
    fireEvent.mouseUp(window);
    fireEvent.mouseDown(header, { clientX: fromX, clientY: 10 });
    fireEvent.mouseMove(window, { clientX: G + toX, clientY: 10, buttons: 1 });
  };

  it('a drop into a forbidden spot snaps: the line and the applied order show where it lands', () => {
    const onColumnOrderChange = vi.fn();
    render(<SpreadsheetGrid<Row> {...base({ onColumnOrderChange, constrainColumnOrder: pinA })} />);
    // Drag d (at 240..290) before a: pinA keeps a first, so d lands before b.
    dragHeader('d', G + 265, 10);
    expect(dropLine()?.style.left).toBe(`${G + 100}px`);
    expect(root()).not.toHaveClass('sg-drop-blocked');
    fireEvent.mouseUp(window);
    expect(onColumnOrderChange).toHaveBeenCalledWith(['a', 'd', 'b', 'c']);
    // The moved column stays selected.
    expect(screen.getByTestId('grid-header-d')).toHaveClass('sg-header-selected');
    expect(dropLine()).toBeNull();
  });

  it('a drop the constraint turns back into the current order shows no line and does nothing', () => {
    const onColumnOrderChange = vi.fn();
    render(<SpreadsheetGrid<Row> {...base({ onColumnOrderChange, constrainColumnOrder: pinA })} />);
    dragHeader('a', G + 50, 250);
    expect(dropLine()).toBeNull();
    expect(root()).toHaveClass('sg-drop-blocked');
    fireEvent.mouseUp(window);
    expect(onColumnOrderChange).not.toHaveBeenCalled();
    expect(root()).not.toHaveClass('sg-drop-blocked');
  });

  it('passes the full order (hidden columns in their slots) through the constraint', () => {
    const onColumnOrderChange = vi.fn();
    const constrainColumnOrder = vi.fn(pinA);
    render(<SpreadsheetGrid<Row> {...base({ onColumnOrderChange, constrainColumnOrder, hiddenColumns: ['b'] })} />);
    // Visible a, c, d (c at 100..160, d at 160..210): drag d before c.
    dragHeader('d', G + 185, 110);
    expect(constrainColumnOrder).toHaveBeenLastCalledWith(['a', 'b', 'd', 'c']);
    fireEvent.mouseUp(window);
    expect(onColumnOrderChange).toHaveBeenCalledWith(['a', 'b', 'd', 'c']);
  });

  it('without a constraint the line still follows the pointer', () => {
    const onColumnOrderChange = vi.fn();
    render(<SpreadsheetGrid<Row> {...base({ onColumnOrderChange })} />);
    dragHeader('d', G + 265, 10);
    expect(dropLine()?.style.left).toBe(`${G}px`);
    fireEvent.mouseUp(window);
    expect(onColumnOrderChange).toHaveBeenCalledWith(['d', 'a', 'b', 'c']);
  });
});
