/**
 * The host-facing props the Data workspace drives: header sort affordance,
 * header class/title, find-match marks, row tints, scroll requests, the
 * rowsVersion reset, the extra context-menu items, PageUp/PageDown paging, the
 * memoized rows and the header move-drag threshold.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';

const { scrollToIndex } = vi.hoisted(() => ({ scrollToIndex: vi.fn() }));
vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () =>
      Array.from({ length: count }, (_, index) => ({ index, key: index, start: 33 + index * 33, size: 33 })),
    getTotalSize: () => count * 33,
    scrollToIndex,
  }),
}));

import { SpreadsheetGrid, type SpreadsheetGridProps } from '../SpreadsheetGrid';
import type { CellMarks, GridColumn, GridScrollRequest } from '../types';

interface Row {
  id: number;
  name: string;
  comment: string;
}

const COLUMNS: GridColumn<Row>[] = [
  { key: 'name', header: 'Name', kind: 'text', editable: true, sortable: true, getValue: (r) => r.name },
  {
    key: 'comment',
    header: 'Comment',
    kind: 'text',
    editable: true,
    getValue: (r) => r.comment,
    headerClassName: 'revealed',
    headerTitle: 'Shown by search',
  },
  { key: 'tag', header: 'Tag', sortable: true, getValue: (r) => r.id, render: (r) => <b>{`#${r.id}`}</b> },
];

const rowsOf = (n: number): Row[] =>
  Array.from({ length: n }, (_, i) => ({ id: i, name: `n${i}`, comment: i === 2 ? 'shot one' : `c${i}` }));
const ROWS = rowsOf(30);

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
const cell = (r: number, k: string) => screen.getByTestId(`grid-cell-${r}-${k}`);
const rectOf = () => screen.getByTestId('grid-selection-overlay').getAttribute('data-rect');
const key = (k: string, init: Partial<KeyboardEventInit> = {}) => fireEvent.keyDown(scroller(), { key: k, ...init });

beforeEach(() => scrollToIndex.mockClear());

describe('header sort affordance', () => {
  it('shows a button on sortable columns only, cycling asc -> desc -> none', () => {
    const onSortChange = vi.fn();
    const { rerender } = render(<SpreadsheetGrid<Row> {...base({ onSortChange, sort: null })} />);
    expect(screen.getByTestId('grid-sort-name')).toHaveAttribute('aria-label', 'Sort by Name, ascending');
    expect(screen.queryByTestId('grid-sort-comment')).toBeNull();
    fireEvent.click(screen.getByTestId('grid-sort-name'));
    expect(onSortChange).toHaveBeenLastCalledWith('name', 'asc');

    rerender(<SpreadsheetGrid<Row> {...base({ onSortChange, sort: { colKey: 'name', dir: 'asc' } })} />);
    expect(screen.getByTestId('grid-header-name')).toHaveAttribute('aria-sort', 'ascending');
    expect(screen.getByTestId('grid-sort-name')).toHaveClass('sg-sort-active');
    expect(screen.getByTestId('grid-header-tag')).not.toHaveAttribute('aria-sort');
    fireEvent.click(screen.getByTestId('grid-sort-name'));
    expect(onSortChange).toHaveBeenLastCalledWith('name', 'desc');

    rerender(<SpreadsheetGrid<Row> {...base({ onSortChange, sort: { colKey: 'name', dir: 'desc' } })} />);
    expect(screen.getByTestId('grid-header-name')).toHaveAttribute('aria-sort', 'descending');
    fireEvent.click(screen.getByTestId('grid-sort-name'));
    expect(onSortChange).toHaveBeenLastCalledWith('name', null);
  });

  it('never selects or drags the column, and keeps Enter for itself', () => {
    const onSortChange = vi.fn();
    render(<SpreadsheetGrid<Row> {...base({ onSortChange, onColumnOrderChange: vi.fn() })} />);
    const btn = screen.getByTestId('grid-sort-name');
    fireEvent.mouseDown(btn);
    fireEvent.mouseUp(window);
    expect(rectOf()).toBe('0,0,0,0');
    fireEvent.keyDown(btn, { key: 'Enter' });
    expect(rectOf()).toBe('0,0,0,0');
    // The header itself still selects the column.
    fireEvent.mouseDown(screen.getByTestId('grid-header-name'));
    fireEvent.mouseUp(window);
    expect(rectOf()).toBe(`0,0,${ROWS.length - 1},0`);
  });

  it('without onSortChange: no button, but the sorted column still says so', () => {
    render(<SpreadsheetGrid<Row> {...base({ sort: { colKey: 'tag', dir: 'desc' } })} />);
    expect(screen.queryByTestId('grid-sort-name')).toBeNull();
    expect(screen.getByTestId('grid-header-tag')).toHaveAttribute('aria-sort', 'descending');
    expect(screen.getByTestId('grid-header-tag').querySelector('.sg-sort-indicator')).not.toBeNull();
  });

  it('puts the label span first, so autofit still measures the label', () => {
    render(<SpreadsheetGrid<Row> {...base({ onSortChange: vi.fn() })} />);
    expect(screen.getByTestId('grid-header-name').querySelector('span')).toHaveTextContent('Name');
  });

  it('applies headerClassName and headerTitle', () => {
    render(<SpreadsheetGrid<Row> {...base()} />);
    expect(screen.getByTestId('grid-header-comment')).toHaveClass('revealed');
    expect(screen.getByTestId('grid-header-comment')).toHaveAttribute('title', 'Shown by search');
  });
});

describe('cellMarks', () => {
  const marks = (_r: Row, i: number, k: string): CellMarks | undefined =>
    i === 2 && k === 'comment'
      ? { ranges: [[0, 4], [5, 8]], current: [5, 8] }
      : i === 3 && k === 'tag'
        ? { ranges: [[0, 1]] }
        : i === 4 && k === 'comment'
          ? { ranges: [[0, 1]] }
          : undefined;

  it('wraps matches in <mark>, the current one stronger, and tags the cell', () => {
    render(<SpreadsheetGrid<Row> {...base({ cellMarks: marks })} />);
    const c = cell(2, 'comment');
    const found = [...c.querySelectorAll('mark')].map((m) => `${m.className}:${m.textContent}`);
    expect(found).toEqual(['sg-mark:shot', 'sg-mark sg-mark-current:one']);
    expect(c).toHaveTextContent('shot one');
    expect(c).toHaveClass('sg-cell-match', 'sg-cell-match-current');
    expect(cell(4, 'comment')).toHaveClass('sg-cell-match');
    expect(cell(4, 'comment')).not.toHaveClass('sg-cell-match-current');
  });

  it('outlines a custom-render cell instead of drawing marks in it', () => {
    render(<SpreadsheetGrid<Row> {...base({ cellMarks: marks })} />);
    expect(cell(3, 'tag')).toHaveClass('sg-cell-match', 'sg-cell-match-outline');
    expect(cell(3, 'tag').querySelector('mark')).toBeNull();
    // A plain-text cell draws its marks and is not outlined.
    expect(cell(4, 'comment')).not.toHaveClass('sg-cell-match-outline');
    expect(cell(3, 'tag')).toHaveTextContent('#3');
  });

  it('keeps the marks under a range selection, whose fill is a class, not an inline colour', () => {
    render(<SpreadsheetGrid<Row> {...base({ cellMarks: marks })} />);
    fireEvent.mouseDown(cell(1, 'comment'));
    fireEvent.mouseUp(window);
    fireEvent.mouseDown(cell(4, 'comment'), { shiftKey: true });
    fireEvent.mouseUp(window);
    const c = cell(2, 'comment');
    expect(c).toHaveClass('sg-cell-sel');
    expect(c.style.backgroundColor).toBe('');
    expect(c.querySelectorAll('mark')).toHaveLength(2);
    // The active cell is not filled.
    expect(cell(1, 'comment')).not.toHaveClass('sg-cell-sel');
  });
});

describe('rowStyle', () => {
  it('applies the host style and flags a tinted row; the grid geometry wins', () => {
    const rowStyle = (r: Row) =>
      r.id === 1 ? ({ '--sg-row-tint': '#ff0000', top: 999 } as React.CSSProperties) : r.id === 2 ? { opacity: 0.5 } : undefined;
    render(<SpreadsheetGrid<Row> {...base({ rowStyle })} />);
    const row1 = screen.getByTestId('grid-row-1');
    expect(row1).toHaveClass('sg-row-tinted');
    expect(row1.style.getPropertyValue('--sg-row-tint')).toBe('#ff0000');
    expect(row1.style.top).toBe('33px');
    expect(screen.getByTestId('grid-row-2')).not.toHaveClass('sg-row-tinted');
    expect(screen.getByTestId('grid-row-2').style.opacity).toBe('0.5');
  });
});

describe('scrollTo', () => {
  const req = (over: Partial<GridScrollRequest>): GridScrollRequest => ({ row: 0, nonce: 1, ...over });

  it('scrolls, makes the cell the whole selection, and never calls onActiveCellChange', () => {
    const onActiveCellChange = vi.fn();
    const { rerender } = render(<SpreadsheetGrid<Row> {...base({ onActiveCellChange })} />);
    rerender(
      <SpreadsheetGrid<Row>
        {...base({ onActiveCellChange, scrollTo: req({ row: 20, colKey: 'comment', align: 'center' }) })}
      />,
    );
    expect(scrollToIndex).toHaveBeenLastCalledWith(20, { align: 'center' });
    expect(rectOf()).toBe('20,1,20,1');
    expect(cell(20, 'comment')).toHaveAttribute('data-active-cell', 'true');
    expect(onActiveCellChange).not.toHaveBeenCalled();
  });

  it('re-fires only on a new nonce', () => {
    const { rerender } = render(<SpreadsheetGrid<Row> {...base({ scrollTo: req({ row: 5 }) })} />);
    expect(scrollToIndex).toHaveBeenCalledTimes(1);
    expect(scrollToIndex).toHaveBeenLastCalledWith(5, { align: 'auto' });
    // Move away, then send the same request object again: no-op.
    fireEvent.mouseDown(cell(1, 'name'));
    fireEvent.mouseUp(window);
    rerender(<SpreadsheetGrid<Row> {...base({ scrollTo: req({ row: 5 }) })} />);
    expect(scrollToIndex).toHaveBeenCalledTimes(1);
    expect(rectOf()).toBe('1,0,1,0');
    // Same row, new nonce: it fires again.
    rerender(<SpreadsheetGrid<Row> {...base({ scrollTo: req({ row: 5, nonce: 2 }) })} />);
    expect(scrollToIndex).toHaveBeenCalledTimes(2);
    expect(rectOf()).toBe('5,0,5,0');
  });

  it('ignores an out-of-range row', () => {
    render(<SpreadsheetGrid<Row> {...base({ scrollTo: req({ row: 99 }) })} />);
    expect(scrollToIndex).not.toHaveBeenCalled();
    expect(rectOf()).toBe('0,0,0,0');
  });

  it('leaves focus in a host input, takes it from <body>, and honours focus:true', () => {
    const Host = ({ scrollTo }: { scrollTo: GridScrollRequest | null }) => (
      <div>
        <input data-testid="find" />
        <SpreadsheetGrid<Row> {...base({ scrollTo })} />
      </div>
    );
    const { rerender } = render(<Host scrollTo={null} />);
    const find = screen.getByTestId('find');
    find.focus();
    rerender(<Host scrollTo={req({ row: 3, nonce: 1 })} />);
    expect(document.activeElement).toBe(find);
    rerender(<Host scrollTo={req({ row: 4, nonce: 2, focus: true })} />);
    expect(document.activeElement).toBe(scroller());
    (document.activeElement as HTMLElement).blur();
    expect(document.activeElement).toBe(document.body);
    rerender(<Host scrollTo={req({ row: 5, nonce: 3 })} />);
    expect(document.activeElement).toBe(scroller());
    find.focus();
    rerender(<Host scrollTo={req({ row: 6, nonce: 4, focus: false })} />);
    expect(document.activeElement).toBe(find);
  });
});

describe('rowsVersion', () => {
  it('drops the copy marquee, cancels the editor and collapses the range', async () => {
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText: vi.fn().mockResolvedValue(undefined) },
      configurable: true,
    });
    const onEdit = vi.fn();
    const { rerender } = render(<SpreadsheetGrid<Row> {...base({ onEdit, rowsVersion: 1 })} />);
    fireEvent.mouseDown(cell(2, 'name'));
    fireEvent.mouseUp(window);
    fireEvent.mouseDown(cell(5, 'comment'), { shiftKey: true });
    fireEvent.mouseUp(window);
    await act(async () => {
      key('c', { ctrlKey: true });
    });
    expect(screen.getByTestId('grid-copy-marquee')).toBeInTheDocument();
    rerender(<SpreadsheetGrid<Row> {...base({ onEdit, rowsVersion: 2 })} />);
    expect(screen.queryByTestId('grid-copy-marquee')).toBeNull();
    expect(rectOf()).toBe('2,0,2,0');

    fireEvent.doubleClick(cell(3, 'comment'));
    fireEvent.change(screen.getByTestId('grid-cell-editor'), { target: { value: 'typed' } });
    rerender(<SpreadsheetGrid<Row> {...base({ onEdit, rowsVersion: 3 })} />);
    expect(screen.queryByTestId('grid-cell-editor')).toBeNull();
    expect(onEdit).not.toHaveBeenCalled();
    Object.defineProperty(navigator, 'clipboard', { value: undefined, configurable: true });
  });

  it('does nothing while the version holds', () => {
    const { rerender } = render(<SpreadsheetGrid<Row> {...base({ rowsVersion: 'a' })} />);
    fireEvent.mouseDown(cell(2, 'name'));
    fireEvent.mouseUp(window);
    fireEvent.mouseDown(cell(4, 'name'), { shiftKey: true });
    fireEvent.mouseUp(window);
    rerender(<SpreadsheetGrid<Row> {...base({ rowsVersion: 'a' })} />);
    expect(rectOf()).toBe('2,0,4,0');
  });
});

describe('extra context-menu items', () => {
  it('honour testId, disabled and separatorBefore, and tolerate duplicate labels', () => {
    const a = vi.fn();
    const b = vi.fn();
    const c = vi.fn();
    const errors = vi.spyOn(console, 'error').mockImplementation(() => {});
    render(
      <SpreadsheetGrid<Row>
        {...base({
          extraContextMenuItems: () => [
            { label: 'Same', onClick: a, testId: 'menu-open-map' },
            { label: 'Same', onClick: b, disabled: true },
            { label: 'Other', onClick: c, separatorBefore: true },
          ],
        })}
      />,
    );
    fireEvent.contextMenu(cell(1, 'name'));
    const menu = screen.getByTestId('grid-context-menu');
    expect(screen.getByTestId('menu-open-map')).toHaveTextContent('Same');
    const disabled = screen.getByTestId('grid-context-extra-1');
    expect(disabled).toBeDisabled();
    fireEvent.click(disabled);
    expect(b).not.toHaveBeenCalled();
    // One separator before the extras block, one requested before "Other".
    const other = screen.getByTestId('grid-context-extra-2');
    expect(other.previousElementSibling).toHaveClass('sg-context-sep');
    expect(screen.getByTestId('menu-open-map').previousElementSibling).toHaveClass('sg-context-sep');
    expect(menu).toBeInTheDocument();
    fireEvent.click(other);
    expect(c).toHaveBeenCalled();
    expect(errors.mock.calls.some((args) => String(args[0]).includes('same key'))).toBe(false);
    errors.mockRestore();
  });
});

describe('PageUp / PageDown', () => {
  it('pages by the measured viewport, not the height prop', () => {
    render(<SpreadsheetGrid<Row> {...base({ height: 900 })} />);
    // A 363px viewport = 33px header + 10 rows; a page keeps one row of overlap.
    Object.defineProperty(scroller(), 'clientHeight', { value: 363, configurable: true });
    fireEvent.mouseDown(cell(0, 'name'));
    fireEvent.mouseUp(window);
    key('PageDown');
    expect(rectOf()).toBe('9,0,9,0');
    key('PageDown');
    expect(rectOf()).toBe('18,0,18,0');
    key('PageUp');
    expect(rectOf()).toBe('9,0,9,0');
  });

  it('falls back to the height prop when nothing is measured', () => {
    render(<SpreadsheetGrid<Row> {...base({ height: 363 })} />);
    fireEvent.mouseDown(cell(0, 'name'));
    fireEvent.mouseUp(window);
    key('PageDown');
    expect(rectOf()).toBe('9,0,9,0');
  });
});

describe('memoized rows', () => {
  it('re-renders only the rows whose own state changed on a click', () => {
    const seen: number[] = [];
    const rowClassName = (_r: Row, i: number) => {
      seen.push(i);
      return undefined;
    };
    const props = base({ rowClassName });
    const { rerender } = render(<SpreadsheetGrid<Row> {...props} />);
    seen.length = 0;
    fireEvent.mouseDown(cell(5, 'name'));
    fireEvent.mouseUp(window);
    expect(new Set(seen)).toEqual(new Set([0, 5]));
    seen.length = 0;
    key('ArrowDown');
    expect(new Set(seen)).toEqual(new Set([5, 6]));
    // A host re-render with the same props re-renders nothing.
    seen.length = 0;
    rerender(<SpreadsheetGrid<Row> {...props} />);
    expect(seen).toEqual([]);
  });
});

describe('header move-drag threshold', () => {
  it('a click with a little drift on a selected header never reorders', () => {
    const onColumnOrderChange = vi.fn();
    render(<SpreadsheetGrid<Row> {...base({ onColumnOrderChange })} />);
    const header = screen.getByTestId('grid-header-name');
    fireEvent.mouseDown(header, { clientX: 60, clientY: 10 });
    fireEvent.mouseUp(window);
    // Already selected: this press arms a MOVE.
    fireEvent.mouseDown(header, { clientX: 60, clientY: 10 });
    fireEvent.mouseMove(window, { clientX: 63, clientY: 11, buttons: 1 });
    fireEvent.mouseUp(window);
    expect(onColumnOrderChange).not.toHaveBeenCalled();
    // A real drag past the threshold still reorders (to after `comment`).
    fireEvent.mouseDown(header, { clientX: 60, clientY: 10 });
    fireEvent.mouseMove(window, { clientX: 48 + 120 * 2 + 70, clientY: 10, buttons: 1 });
    fireEvent.mouseUp(window);
    expect(onColumnOrderChange).toHaveBeenCalledWith(['comment', 'tag', 'name']);
  });
});
