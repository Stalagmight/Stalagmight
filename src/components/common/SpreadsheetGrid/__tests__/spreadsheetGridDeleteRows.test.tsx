import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';

vi.mock('@tanstack/react-virtual', () => import('../../../../test/factories/virtualizer'));

import { SpreadsheetGrid } from '../SpreadsheetGrid';
import type { GridColumn } from '../types';

/**
 * Deleting rows must leave the cursor as a single cell, not a surviving range.
 *
 * The rows below a delete shift up, so a row-range selection stops meaning what
 * it did: after a run at the END it lands on whatever is now last, after one at
 * the START it lands on the next N rows - highlighting rows nobody picked, with
 * Delete still armed on them. The grid's reconciliation effect only clamps the
 * range into bounds, which is what made the wrong selection look plausible
 * instead of dropping it.
 */

interface Row {
  id: number;
  value: number;
}

const columns: GridColumn<Row>[] = [
  { key: 'value', header: 'Value', getValue: (r) => r.value, editable: true, kind: 'number' },
];

const rowsOf = (n: number): Row[] =>
  Array.from({ length: n }, (_, i) => ({ id: i, value: i * 10 }));

function renderGrid(rowCount: number, onDeleteRows: (from: number, to: number) => void) {
  return render(
    <SpreadsheetGrid
      columns={columns}
      rows={rowsOf(rowCount)}
      rowKey={(row) => row.id}
      cellTestId={(_row, index, colKey) => `cell-${index}-${colKey}`}
      onEdit={vi.fn()}
      onDeleteRows={onDeleteRows}
      height={200}
    />
  );
}

/** Select whole rows by dragging the gutter, the way a user picks a run. */
function selectRowRun(from: number, to: number) {
  const gutters = document.querySelectorAll('.sg-gutter');
  fireEvent.mouseDown(gutters[from]!);
  fireEvent.mouseDown(gutters[to]!, { shiftKey: true });
  fireEvent.mouseUp(gutters[to]!, { shiftKey: true });
}

const selectedRowCount = () => document.querySelectorAll('.sg-gutter-selected').length;

/** Which row the single active cell sits on, or -1. Stronger than counting the
 *  selection: it also catches a collapse that lands on the wrong row. */
const activeRow = () =>
  [...document.querySelectorAll('.sg-gutter')].findIndex((g) =>
    g.className.includes('sg-gutter-active')
  );

/** Open the row context menu over a cell in the selection and hit Delete rows. */
function deleteViaContextMenu(row: number) {
  fireEvent.contextMenu(screen.getByTestId(`cell-${row}-value`));
  fireEvent.click(screen.getByTestId('grid-context-delete-rows'));
}

/** The keyboard path: Delete on the grid's focusable scroller, which owns the
 *  key handling, with whole rows selected. */
function deleteViaKeyboard() {
  fireEvent.keyDown(document.querySelector('.sg-grid [tabindex="0"]')!, { key: 'Delete' });
}

describe('SpreadsheetGrid row deletion', () => {
  it('collapses a trailing run selection instead of leaving it on what is now last', () => {
    const onDeleteRows = vi.fn();
    renderGrid(6, onDeleteRows);
    selectRowRun(3, 5);
    expect(selectedRowCount()).toBe(3);

    deleteViaContextMenu(4);
    expect(onDeleteRows).toHaveBeenCalledWith(3, 5);
    // One cell stays active - the grid always has a cursor - and it sits just
    // ABOVE the run, not on whatever slid up into it.
    expect(selectedRowCount()).toBeLessThanOrEqual(1);
    expect(activeRow()).toBe(2);
  });

  it('collapses a leading run selection instead of sliding it onto the next rows', () => {
    const onDeleteRows = vi.fn();
    renderGrid(6, onDeleteRows);
    selectRowRun(0, 2);
    expect(selectedRowCount()).toBe(3);

    deleteViaContextMenu(1);
    expect(onDeleteRows).toHaveBeenCalledWith(0, 2);
    expect(selectedRowCount()).toBeLessThanOrEqual(1);
    // Nowhere above row 0 to go.
    expect(activeRow()).toBe(0);
  });

  it('collapses after the keyboard Delete path too', () => {
    const onDeleteRows = vi.fn();
    renderGrid(6, onDeleteRows);
    selectRowRun(2, 4);
    deleteViaKeyboard();
    expect(onDeleteRows).toHaveBeenCalledWith(2, 4);
    expect(selectedRowCount()).toBeLessThanOrEqual(1);
  });

  it('still reports the range the user picked, unchanged', () => {
    const onDeleteRows = vi.fn();
    renderGrid(8, onDeleteRows);
    selectRowRun(5, 7);
    deleteViaContextMenu(6);
    expect(onDeleteRows).toHaveBeenCalledTimes(1);
    expect(onDeleteRows).toHaveBeenCalledWith(5, 7);
  });
});

describe('SpreadsheetGrid row deletion the host refuses', () => {
  it('keeps the selection when onDeleteRows returns false', () => {
    // A host refuses (mid-section run) or defers behind a confirm. The rows are
    // still there, so clearing the selection would strand the user - and for the
    // confirm it would clear it before they answered.
    const onDeleteRows = vi.fn(() => false);
    renderGrid(6, onDeleteRows);
    selectRowRun(3, 5);
    deleteViaContextMenu(4);
    expect(onDeleteRows).toHaveBeenCalledWith(3, 5);
    expect(selectedRowCount()).toBe(3);
  });

  it('still collapses when the host returns nothing', () => {
    // `void` is the common case - a host that cannot refuse.
    const onDeleteRows = vi.fn();
    renderGrid(6, onDeleteRows);
    selectRowRun(3, 5);
    deleteViaContextMenu(4);
    expect(selectedRowCount()).toBeLessThanOrEqual(1);
  });
});
