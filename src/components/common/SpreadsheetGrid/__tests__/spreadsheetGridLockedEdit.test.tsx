/**
 * `onLockedEdit`: a key meant to edit a cell the grid will not write - a typed
 * character, F2, a double-click, Delete - tells the host, so a read-only data
 * table can say why instead of swallowing the key (demo finding I-10). A cell
 * the grid does write never calls it. Injection: dropping the `onLockedEdit`
 * calls from `typeToEdit`, `beginEdit` or the Delete branch fails the
 * matching case.
 */
import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';

vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () =>
      Array.from({ length: count }, (_, index) => ({
        index,
        key: index,
        start: 33 + index * 33,
        size: 33,
      })),
    getTotalSize: () => count * 33,
    scrollToIndex: vi.fn(),
  }),
}));

import { SpreadsheetGrid } from '../SpreadsheetGrid';
import type { GridColumn } from '../types';

interface Row {
  id: number;
  comment: string;
  note: string;
}

const columns: GridColumn<Row>[] = [
  { key: 'comment', header: 'Comment', kind: 'text', getValue: (r) => r.comment, editable: false },
  { key: 'note', header: 'Note', kind: 'text', getValue: (r) => r.note, editable: true },
];

const rows: Row[] = [{ id: 1, comment: 'Silt bank', note: 'tie-off' }];

function renderGrid() {
  const onEdit = vi.fn();
  const onLockedEdit = vi.fn();
  render(
    <SpreadsheetGrid<Row>
      columns={columns}
      rows={rows}
      rowKey={(r) => r.id}
      onEdit={onEdit}
      onLockedEdit={onLockedEdit}
      height={200}
    />,
  );
  return { onEdit, onLockedEdit };
}

const scroller = () => document.querySelector('.sg-grid [tabindex="0"]')!;

describe('SpreadsheetGrid onLockedEdit', () => {
  it('reports a character typed into a locked cell and edits nothing', () => {
    const { onEdit, onLockedEdit } = renderGrid();
    fireEvent.mouseDown(screen.getByTestId('grid-cell-0-comment'));
    fireEvent.keyDown(scroller(), { key: 'x' });
    expect(onLockedEdit).toHaveBeenCalledTimes(1);
    expect(screen.queryByTestId('grid-cell-editor')).toBeNull();
    expect(onEdit).not.toHaveBeenCalled();
  });

  it('reports Delete, F2 and a double-click on a locked cell', () => {
    const { onEdit, onLockedEdit } = renderGrid();
    fireEvent.mouseDown(screen.getByTestId('grid-cell-0-comment'));
    fireEvent.keyDown(scroller(), { key: 'Delete' });
    expect(onLockedEdit).toHaveBeenCalledTimes(1);
    fireEvent.keyDown(scroller(), { key: 'F2' });
    expect(onLockedEdit).toHaveBeenCalledTimes(2);
    fireEvent.doubleClick(screen.getByTestId('grid-cell-0-comment'));
    expect(onLockedEdit).toHaveBeenCalledTimes(3);
    expect(onEdit).not.toHaveBeenCalled();
  });

  it('still clears the writable cells of a range that starts on a locked one', () => {
    const { onEdit, onLockedEdit } = renderGrid();
    fireEvent.mouseDown(screen.getByTestId('grid-cell-0-comment'));
    fireEvent.keyDown(scroller(), { key: 'ArrowRight', shiftKey: true });
    fireEvent.keyDown(scroller(), { key: 'Delete' });
    expect(onLockedEdit).toHaveBeenCalledTimes(1);
    expect(onEdit).toHaveBeenCalledWith(0, 'note', expect.anything());
    expect(onEdit).not.toHaveBeenCalledWith(0, 'comment', expect.anything());
  });

  it('never reports an edit of a cell it writes', () => {
    const { onLockedEdit } = renderGrid();
    fireEvent.mouseDown(screen.getByTestId('grid-cell-0-note'));
    fireEvent.keyDown(scroller(), { key: 'x' });
    expect(screen.getByTestId('grid-cell-editor')).toBeInTheDocument();
    fireEvent.keyDown(screen.getByTestId('grid-cell-editor'), { key: 'Escape' });
    fireEvent.keyDown(scroller(), { key: 'Delete' });
    expect(onLockedEdit).not.toHaveBeenCalled();
  });
});
