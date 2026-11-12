/**
 * A popover-edited column (`openEditor`): every way into an edit opens the
 * host's editor, and none of them reaches the grid's own text editor or
 * commits a typed character through the column's paste parser. A people cell
 * whose parser turned one typed letter into a one-letter person is the bug
 * this pins.
 */
import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';

vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () =>
      Array.from({ length: count }, (_, index) => ({ index, key: index, start: 33 + index * 33, size: 33 })),
    getTotalSize: () => count * 33,
    scrollToIndex: vi.fn(),
  }),
}));

import { SpreadsheetGrid } from '../SpreadsheetGrid';
import type { GridColumn } from '../types';

interface Row {
  id: number;
  name: string;
  people: string;
}

const rows: Row[] = [
  { id: 1, name: 'a', people: 'Ann' },
  { id: 2, name: 'b', people: '' },
];

function renderGrid(editable = true) {
  const onEdit = vi.fn();
  const onBatchEdit = vi.fn();
  const opened: { row: number; seed: string }[] = [];
  const parse = vi.fn((raw: string) => raw.split(';'));
  const columns: GridColumn<Row>[] = [
    { key: 'name', header: 'Name', kind: 'text', getValue: (r) => r.name, editable: true },
    {
      key: 'people',
      header: 'People',
      kind: 'text',
      getValue: (r) => r.people,
      parse,
      editable,
      noInlineEdit: true,
      openEditor: (_row, index, seed) => opened.push({ row: index, seed }),
    },
  ];
  render(
    <SpreadsheetGrid<Row>
      columns={columns}
      rows={rows}
      rowKey={(r) => r.id}
      onEdit={onEdit}
      onBatchEdit={onBatchEdit}
      height={200}
    />,
  );
  return { onEdit, onBatchEdit, opened, parse };
}

const scroller = () => document.querySelector('.sg-grid [tabindex="0"]')!;
const inlineEditor = () => screen.queryByTestId('grid-cell-editor');
const activeCellId = () => document.querySelector('[data-active-cell="true"]')?.getAttribute('data-testid');

describe('SpreadsheetGrid openEditor', () => {
  it('opens the host editor with a typed character, never committing it through parse', () => {
    const { onEdit, onBatchEdit, opened, parse } = renderGrid();
    fireEvent.mouseDown(screen.getByTestId('grid-cell-1-people'));
    const event = fireEvent.keyDown(scroller(), { key: 'A' });

    expect(opened).toEqual([{ row: 1, seed: 'A' }]);
    expect(event).toBe(false); // consumed: the letter is the editor's now
    expect(parse).not.toHaveBeenCalled();
    expect(onEdit).not.toHaveBeenCalled();
    expect(onBatchEdit).not.toHaveBeenCalled();
    expect(inlineEditor()).toBeNull();
  });

  it('opens it empty from Enter, F2, Space and a double click anywhere in the cell', () => {
    const { onEdit, opened } = renderGrid();
    fireEvent.mouseDown(screen.getByTestId('grid-cell-0-people'));
    fireEvent.keyDown(scroller(), { key: 'Enter' });
    fireEvent.keyDown(scroller(), { key: 'F2' });
    fireEvent.keyDown(scroller(), { key: ' ' });
    fireEvent.doubleClick(screen.getByTestId('grid-cell-0-people'));

    expect(opened).toEqual([
      { row: 0, seed: '' },
      { row: 0, seed: '' },
      { row: 0, seed: '' },
      { row: 0, seed: '' },
    ]);
    // Enter opened the editor instead of moving down.
    expect(activeCellId()).toBe('grid-cell-0-people');
    expect(inlineEditor()).toBeNull();
    expect(onEdit).not.toHaveBeenCalled();
  });

  it('leaves a read-only cell alone: Enter moves down and a letter does nothing', () => {
    const { onEdit, opened, parse } = renderGrid(false);
    fireEvent.mouseDown(screen.getByTestId('grid-cell-0-people'));
    fireEvent.keyDown(scroller(), { key: 'A' });
    fireEvent.doubleClick(screen.getByTestId('grid-cell-0-people'));
    fireEvent.keyDown(scroller(), { key: 'Enter' });

    expect(opened).toEqual([]);
    expect(parse).not.toHaveBeenCalled();
    expect(onEdit).not.toHaveBeenCalled();
    expect(inlineEditor()).toBeNull();
    expect(activeCellId()).toBe('grid-cell-1-people');
  });

  it('still pastes, fills and clears through parse', () => {
    const { onBatchEdit, opened } = renderGrid();
    fireEvent.mouseDown(screen.getByTestId('grid-cell-0-people'));
    fireEvent.keyDown(scroller(), { key: 'Delete' });

    expect(opened).toEqual([]);
    expect(onBatchEdit).toHaveBeenCalledWith([{ row: 0, colKey: 'people', value: [''] }]);
  });
});
