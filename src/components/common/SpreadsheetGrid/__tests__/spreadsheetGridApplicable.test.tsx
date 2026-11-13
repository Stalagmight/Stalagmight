/**
 * Not-applicable cells and group-header rows: the grid the Data workspace
 * builds, where STATION and SHOT rows interleave (a shot-only column has no
 * cell on a station row) under section header bands.
 *
 * Every `getValue` here THROWS on a row its column does not apply to, so any
 * path that still reads an N/A cell (render, copy, fill, Ctrl+Arrow, move)
 * fails loudly. The edit gates (`isApplicable` and `isEditable`) are pinned on
 * every write path: type, F2, double-click, paste, fill, clear and move.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';

vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () =>
      Array.from({ length: count }, (_, index) => ({ index, key: index, start: 33 + index * 33, size: 33 })),
    getTotalSize: () => count * 33,
    scrollToIndex: vi.fn(),
  }),
}));

import { SpreadsheetGrid, type SpreadsheetGridProps } from '../SpreadsheetGrid';
import type { CellEdit, GridColumn } from '../types';

interface Row {
  id: number;
  kind: 'group' | 'station' | 'shot';
  name: string;
  len: number | null;
  comment: string;
}

const na = (what: string, r: Row): never => {
  throw new Error(`${what} read on a ${r.kind} row`);
};

const renderBadge = vi.fn((r: Row) => <span>{`st-${r.name}`}</span>);

/** name: every non-group row; len: shots only; comment: every row; ro:
 *  read-only; badge: custom-render, stations only. */
const columns: GridColumn<Row>[] = [
  { key: 'name', header: 'Name', kind: 'text', editable: true, getValue: (r) => (r.kind === 'group' ? na('name', r) : r.name) },
  {
    key: 'len',
    header: 'Len',
    kind: 'number-nullable',
    editable: true,
    isApplicable: (r) => r.kind === 'shot',
    getValue: (r) => (r.kind === 'shot' ? r.len : na('len', r)),
  },
  { key: 'comment', header: 'Comment', kind: 'text', editable: true, getValue: (r) => (r.kind === 'group' ? na('comment', r) : r.comment) },
  { key: 'ro', header: 'RO', getValue: (r) => (r.kind === 'group' ? na('ro', r) : r.id) },
  {
    key: 'badge',
    header: 'Badge',
    isApplicable: (r) => r.kind === 'station',
    getValue: (r) => (r.kind === 'station' ? r.name : na('badge', r)),
    render: renderBadge,
  },
];

const ROWS: Row[] = [
  { id: 0, kind: 'group', name: 'A', len: null, comment: '' },
  { id: 1, kind: 'station', name: 'A0', len: null, comment: 'entrance' },
  { id: 2, kind: 'shot', name: 'A1', len: 5, comment: 'one' },
  { id: 3, kind: 'station', name: 'A1', len: null, comment: '' },
  { id: 4, kind: 'shot', name: 'A2', len: 7, comment: 'two' },
  { id: 5, kind: 'station', name: 'A2', len: null, comment: 'end' },
  { id: 6, kind: 'group', name: 'B', len: null, comment: '' },
  { id: 7, kind: 'station', name: 'B0', len: null, comment: '' },
];

const onCollapse = vi.fn();
const groupRowContent = (r: Row) =>
  r.kind === 'group' ? (
    <span>
      {`Section ${r.name}`}
      <button
        type="button"
        data-testid={`collapse-${r.name}`}
        onMouseDown={(e) => e.stopPropagation()}
        onClick={(e) => {
          e.stopPropagation();
          onCollapse(r.name);
        }}
      >
        v
      </button>
    </span>
  ) : undefined;

function renderGrid(over: Partial<SpreadsheetGridProps<Row>> = {}) {
  const onEdit = vi.fn();
  const onBatchEdit = vi.fn<(edits: CellEdit[]) => void>();
  const onPasteReport = vi.fn();
  const utils = render(
    <SpreadsheetGrid<Row>
      columns={columns}
      rows={ROWS}
      rowKey={(r) => r.id}
      rowLabel={(r) => (r.kind === 'group' ? `grp:${r.name}` : r.name)}
      groupRowContent={groupRowContent}
      onEdit={onEdit}
      onBatchEdit={onBatchEdit}
      onPasteReport={onPasteReport}
      height={400}
      {...over}
    />,
  );
  return { onEdit, onBatchEdit, onPasteReport, ...utils };
}

const scroller = () => document.querySelector('.sg-grid [tabindex="0"]') as HTMLElement;
const cell = (r: number, k: string) => screen.getByTestId(`grid-cell-${r}-${k}`);
const rectOf = () => screen.getByTestId('grid-selection-overlay').getAttribute('data-rect');
const activeCell = () => document.querySelector('[data-active-cell="true"]')?.getAttribute('data-testid');
const key = (k: string, init: Partial<KeyboardEventInit> = {}) => fireEvent.keyDown(scroller(), { key: k, ...init });
const select = (r0: number, k0: string, r1: number, k1: string) => {
  fireEvent.mouseDown(cell(r0, k0));
  fireEvent.mouseUp(window);
  fireEvent.mouseDown(cell(r1, k1), { shiftKey: true });
  fireEvent.mouseUp(window);
};
/** Select from a group row (via its band) to a cell, like a gutter drag. */
const selectFromBand = (groupRow: number, r1: number, k1: string) => {
  fireEvent.mouseDown(screen.getByTestId(`grid-group-${groupRow}`));
  fireEvent.mouseUp(window);
  fireEvent.mouseDown(cell(r1, k1), { shiftKey: true });
  fireEvent.mouseUp(window);
};
const paste = (text: string) => fireEvent.paste(scroller(), { clipboardData: { getData: () => text } });

let writeText: ReturnType<typeof vi.fn>;
beforeEach(() => {
  writeText = vi.fn().mockResolvedValue(undefined);
  Object.defineProperty(navigator, 'clipboard', {
    value: { writeText, readText: vi.fn().mockRejectedValue(new Error('denied')) },
    configurable: true,
  });
  renderBadge.mockClear();
  onCollapse.mockClear();
});
afterEach(() => {
  Object.defineProperty(navigator, 'clipboard', { value: undefined, configurable: true });
});

describe('N/A cells render blank and quiet', () => {
  it('draws sg-cell-na with no content, never locked, without reading the value', () => {
    renderGrid();
    const st = cell(1, 'len');
    expect(st).toHaveClass('sg-cell-na');
    expect(st).not.toHaveClass('sg-cell-locked');
    expect(st.textContent).toBe('');
    expect(cell(2, 'len')).toHaveTextContent('5');
    expect(cell(2, 'len')).not.toHaveClass('sg-cell-na');
  });

  it('keeps the locked look for an applicable read-only cell only', () => {
    renderGrid();
    expect(cell(1, 'ro')).toHaveClass('sg-cell-locked');
    // N/A beats locked: the badge column is read-only too, but on a shot it
    // is not there at all.
    expect(cell(2, 'badge')).toHaveClass('sg-cell-na');
    expect(cell(2, 'badge')).not.toHaveClass('sg-cell-locked');
  });

  it('never calls render or cellClassName for an N/A cell', () => {
    const cellClassName = vi.fn(() => undefined);
    renderGrid({ cellClassName });
    expect(renderBadge.mock.calls.every(([r]) => r.kind === 'station')).toBe(true);
    expect(renderBadge).toHaveBeenCalled();
    expect(cellClassName.mock.calls.some(([r, , k]) => k === 'len' && (r as Row).kind !== 'shot')).toBe(false);
  });
});

describe('N/A cells never take an edit', () => {
  it('type-to-edit, F2 and double-click open nothing', () => {
    const { onEdit } = renderGrid();
    fireEvent.mouseDown(cell(1, 'len'));
    key('7');
    expect(screen.queryByTestId('grid-cell-editor')).toBeNull();
    key('F2');
    expect(screen.queryByTestId('grid-cell-editor')).toBeNull();
    fireEvent.doubleClick(cell(1, 'len'));
    expect(screen.queryByTestId('grid-cell-editor')).toBeNull();
    expect(onEdit).not.toHaveBeenCalled();
    // An applicable neighbour still edits.
    fireEvent.doubleClick(cell(2, 'len'));
    expect(screen.getByTestId('grid-cell-editor')).toBeInTheDocument();
  });

  it('a paste skips N/A cells and reports them as skipped', () => {
    const { onBatchEdit, onPasteReport } = renderGrid();
    select(1, 'len', 4, 'len');
    paste('9');
    expect(onBatchEdit).toHaveBeenCalledTimes(1);
    expect(onBatchEdit.mock.calls[0]![0]).toEqual([
      { row: 2, colKey: 'len', value: 9 },
      { row: 4, colKey: 'len', value: 9 },
    ]);
    expect(onPasteReport).toHaveBeenCalledWith({ written: 2, clipped: 0, skipped: 2 });
  });

  it('fill down skips an N/A destination and never fills from an N/A source', () => {
    const { onBatchEdit } = renderGrid();
    select(2, 'len', 4, 'len');
    key('d', { ctrlKey: true });
    expect(onBatchEdit).toHaveBeenLastCalledWith([{ row: 4, colKey: 'len', value: 5 }]);
    onBatchEdit.mockClear();
    // Source row 1 is a station: its blank must not be stamped over row 2's 5.
    select(1, 'len', 2, 'len');
    key('d', { ctrlKey: true });
    expect(onBatchEdit).not.toHaveBeenCalled();
  });

  it('clear skips N/A and read-only cells', () => {
    const { onBatchEdit } = renderGrid();
    select(1, 'len', 2, 'ro');
    key('Delete');
    expect(onBatchEdit).toHaveBeenCalledTimes(1);
    expect(onBatchEdit.mock.calls[0]![0]).toEqual([
      { row: 1, colKey: 'comment', value: '' },
      { row: 2, colKey: 'len', value: null },
      { row: 2, colKey: 'comment', value: '' },
    ]);
  });

  it('an internal paste never moves an N/A source onto data', async () => {
    const { onBatchEdit, onPasteReport } = renderGrid();
    // Copy [station N/A, shot 5] and paste it one row down, misaligned.
    select(1, 'len', 2, 'len');
    await act(async () => {
      key('c', { ctrlKey: true });
    });
    expect(writeText).toHaveBeenCalledWith('\n5');
    fireEvent.mouseDown(cell(2, 'len'));
    fireEvent.mouseUp(window);
    paste('\n5');
    // Row 2 would have taken the station's blank, row 3 is a station.
    expect(onBatchEdit).not.toHaveBeenCalled();
    expect(onPasteReport).toHaveBeenCalledWith({ written: 0, clipped: 0, skipped: 2 });
  });

  it('a cut-move carries N/A gaps along but aborts onto an N/A destination', () => {
    const { onBatchEdit } = renderGrid();
    select(1, 'len', 2, 'len'); // [station N/A, shot 5]
    // Grab the selection's edge at row 1, len (gutter 48 + 120 for `name`).
    const grab = { clientX: 48 + 120 + 10, clientY: 33 + 1 * 33 + 5 };
    const edge = document.querySelector('.sg-move-edge') as HTMLElement;
    // One row down: the shot's 5 would land on station row 3 - abort.
    fireEvent.mouseDown(edge, grab);
    fireEvent.mouseMove(window, { ...grab, clientY: grab.clientY + 33, buttons: 1 });
    fireEvent.mouseUp(window);
    expect(onBatchEdit).not.toHaveBeenCalled();
    // Two rows down: station gap onto station gap, 5 onto shot row 4.
    fireEvent.mouseDown(document.querySelector('.sg-move-edge') as HTMLElement, grab);
    fireEvent.mouseMove(window, { ...grab, clientY: grab.clientY + 66, buttons: 1 });
    fireEvent.mouseUp(window);
    expect(onBatchEdit).toHaveBeenCalledWith([
      { row: 4, colKey: 'len', value: 5 },
      { row: 2, colKey: 'len', value: null },
    ]);
  });
});

describe('isEditable gates every bulk path (per-row lock)', () => {
  const lockedShot = (r: Row) => r.id !== 4;
  const gated = columns.map((c) => (c.key === 'len' || c.key === 'comment' ? { ...c, isEditable: lockedShot } : c));

  it('paste, fill and clear all skip the locked row', () => {
    const { onBatchEdit, onPasteReport } = renderGrid({ columns: gated });
    expect(cell(4, 'len')).toHaveClass('sg-cell-locked');
    select(2, 'comment', 5, 'comment');
    paste('x');
    expect(onBatchEdit.mock.calls[0]![0].map((e: CellEdit) => e.row)).toEqual([2, 3, 5]);
    expect(onPasteReport).toHaveBeenCalledWith({ written: 3, clipped: 0, skipped: 1 });
    onBatchEdit.mockClear();
    key('d', { ctrlKey: true });
    expect(onBatchEdit.mock.calls[0]![0].map((e: CellEdit) => e.row)).toEqual([3, 5]);
    onBatchEdit.mockClear();
    key('Delete');
    expect(onBatchEdit.mock.calls[0]![0].map((e: CellEdit) => e.row)).toEqual([2, 5]);
  });
});

describe('copy', () => {
  it('copies N/A cells and group rows as empty strings', async () => {
    renderGrid();
    selectFromBand(0, 2, 'len');
    await act(async () => {
      key('c', { ctrlKey: true });
    });
    expect(writeText).toHaveBeenCalledWith('\t\nA0\t\nA1\t5');
  });
});

describe('keyboard over N/A cells', () => {
  it('Tab and Shift+Tab skip N/A cells per row, and whole group rows', () => {
    renderGrid();
    fireEvent.mouseDown(cell(1, 'name'));
    key('Tab');
    expect(activeCell()).toBe('grid-cell-1-comment'); // len is N/A on a station
    key('Tab');
    expect(activeCell()).toBe('grid-cell-2-name');
    key('Tab');
    expect(activeCell()).toBe('grid-cell-2-len');
    fireEvent.mouseDown(cell(5, 'comment'));
    key('Tab');
    expect(activeCell()).toBe('grid-cell-7-name'); // row 6 is a group band
    key('Tab', { shiftKey: true });
    expect(activeCell()).toBe('grid-cell-5-comment');
  });

  it('arrow keys still land on N/A cells and group rows', () => {
    renderGrid();
    fireEvent.mouseDown(cell(2, 'len'));
    key('ArrowDown');
    expect(activeCell()).toBe('grid-cell-3-len');
    fireEvent.mouseDown(cell(5, 'name'));
    key('ArrowDown');
    expect(rectOf()).toBe('6,0,6,0');
  });

  it('Ctrl+Arrow passes over N/A cells: a block is the run the column applies to', () => {
    // Shots on the odd rows: 5, 7, (empty), 9; stations between them.
    const lens = [null, 5, null, 7, null, null, null, 9, null];
    const rows: Row[] = lens.map((len, i) => ({
      id: i,
      kind: i % 2 === 1 ? 'shot' : 'station',
      name: `n${i}`,
      len,
      comment: '',
    }));
    renderGrid({ rows, groupRowContent: undefined });
    fireEvent.mouseDown(cell(1, 'len'));
    key('ArrowDown', { ctrlKey: true });
    expect(activeCell()).toBe('grid-cell-3-len'); // end of the 5,7 run
    key('ArrowDown', { ctrlKey: true });
    expect(activeCell()).toBe('grid-cell-7-len'); // over the empty shot to 9
    key('ArrowDown', { ctrlKey: true });
    expect(activeCell()).toBe('grid-cell-7-len'); // nothing applicable below
  });
});

describe('group-header rows', () => {
  it('render one band with the content, and the gutter keeps rowLabel', () => {
    renderGrid();
    const band = screen.getByTestId('grid-group-0');
    expect(band).toHaveTextContent('Section A');
    expect(band).toHaveAttribute('aria-colspan', String(columns.length));
    expect(screen.queryByTestId('grid-cell-0-name')).toBeNull();
    expect(screen.getByTestId('grid-gutter-0')).toHaveTextContent('grp:A');
    expect(screen.getByTestId('grid-row-0')).toHaveClass('sg-row-group');
  });

  it('a click on the band selects the row like the gutter; its own controls stay independent', () => {
    const onActiveCellChange = vi.fn();
    renderGrid({ onActiveCellChange });
    fireEvent.mouseDown(screen.getByTestId('grid-group-6'));
    fireEvent.mouseUp(window);
    expect(rectOf()).toBe(`6,0,6,${columns.length - 1}`);
    expect(onActiveCellChange).toHaveBeenLastCalledWith(6, 'name');
    // The chevron stops propagation: no selection change, its own handler runs.
    fireEvent.mouseDown(cell(2, 'name'));
    fireEvent.mouseUp(window);
    fireEvent.mouseDown(screen.getByTestId('collapse-A'));
    fireEvent.click(screen.getByTestId('collapse-A'));
    expect(onCollapse).toHaveBeenCalledWith('A');
    expect(rectOf()).toBe('2,0,2,0');
  });

  it('takes no edit from paste, fill or typing', () => {
    const { onBatchEdit, onEdit } = renderGrid();
    fireEvent.mouseDown(screen.getByTestId('grid-group-0'));
    fireEvent.mouseUp(window);
    paste('x');
    expect(onBatchEdit).not.toHaveBeenCalled();
    selectFromBand(0, 1, 'name');
    key('d', { ctrlKey: true });
    expect(onBatchEdit).not.toHaveBeenCalled();
    key('x');
    expect(onEdit).not.toHaveBeenCalled();
  });
});
