/**
 * What a committed draft is allowed to do to the model.
 *
 * Three rules meet in `commitEdit`, and they used to be one:
 *  - an EMPTIED cell is a clear: mark it missing and tell a provisional host to
 *    drop the field;
 *  - a NON-EMPTY draft the column cannot parse ("15.2m", "1o5") is a typo, and
 *    gets the same treatment as a value `validate` rejects - flash, keep the
 *    editor open, touch nothing. Lumping it in with the clear meant a host that
 *    acts on `onClearMissing` wrote its unset sentinel over a saved measurement
 *    the user was only mistyping a correction to;
 *  - a rejected draft keeps the editor OPEN, so Tab must not walk the selection
 *    off the cell being fixed.
 *
 * Plus the two paths that reach the same code from elsewhere: Space on a cell
 * with no button to press must still type a space, and a lone bulk edit must
 * reach the host as the batch it is.
 */
import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';

vi.mock('@tanstack/react-virtual', () => import('../../../../test/factories/virtualizer'));

import { SpreadsheetGrid } from '../SpreadsheetGrid';
import type { CellEdit, GridColumn } from '../types';

interface Row {
  id: number;
  length: number;
  left: number | null;
  kind: string;
}

const cycled: { row: number; dir: number }[] = [];

const columns: GridColumn<Row>[] = [
  {
    key: 'length',
    header: 'Len',
    kind: 'number',
    getValue: (r) => r.length,
    editable: true,
    validate: (v) => (typeof v === 'number' && v > 100 ? 'too long' : null),
  },
  { key: 'left', header: 'L', kind: 'number-nullable', getValue: (r) => r.left, editable: true },
  {
    key: 'kind',
    header: 'J/R',
    kind: 'text',
    getValue: (r) => r.kind,
    editable: true,
    noInlineEdit: true,
    activate: (_row, index, dir) => cycled.push({ row: index, dir }),
  },
];

const rows: Row[] = [
  { id: 1, length: 10, left: 2, kind: 'Normal' },
  { id: 2, length: 20, left: null, kind: 'Normal' },
];

function renderGrid(props: Partial<Parameters<typeof SpreadsheetGrid<Row>>[0]> = {}) {
  const onEdit = vi.fn();
  const onClearMissing = vi.fn();
  render(
    <SpreadsheetGrid<Row>
      columns={columns}
      rows={rows}
      rowKey={(r) => r.id}
      onEdit={onEdit}
      onClearMissing={onClearMissing}
      height={200}
      {...props}
    />
  );
  return { onEdit, onClearMissing };
}

const scroller = () => document.querySelector('.sg-grid [tabindex="0"]')!;
const editor = () => screen.queryByTestId('grid-cell-editor');

/** Open the cell's editor and put `draft` in it, without committing. */
function typeInto(testId: string, draft: string) {
  fireEvent.doubleClick(screen.getByTestId(testId));
  fireEvent.change(screen.getByTestId('grid-cell-editor'), { target: { value: draft } });
}

describe('SpreadsheetGrid commit of an unparseable draft', () => {
  it('rejects it instead of clearing the cell, and keeps the editor open', () => {
    const { onEdit, onClearMissing } = renderGrid();
    typeInto('grid-cell-0-length', '15.2m');
    fireEvent.keyDown(screen.getByTestId('grid-cell-editor'), { key: 'Enter' });

    expect(onEdit).not.toHaveBeenCalled();
    expect(onClearMissing).not.toHaveBeenCalled();
    expect(screen.getByTestId('grid-invalid-flash')).toBeInTheDocument();
    expect(editor()).toHaveValue('15.2m');
  });

  it('discards it on blur, still without touching the model', () => {
    const { onEdit, onClearMissing } = renderGrid();
    typeInto('grid-cell-0-length', '1o5');
    fireEvent.blur(screen.getByTestId('grid-cell-editor'));

    expect(onEdit).not.toHaveBeenCalled();
    expect(onClearMissing).not.toHaveBeenCalled();
    expect(editor()).toBeNull();
  });

  it('still clears a cell the user actually emptied', () => {
    const { onEdit, onClearMissing } = renderGrid();
    typeInto('grid-cell-0-length', '   ');
    fireEvent.keyDown(screen.getByTestId('grid-cell-editor'), { key: 'Enter' });

    expect(onEdit).not.toHaveBeenCalled();
    expect(onClearMissing).toHaveBeenCalledWith(0, 'length');
    expect(editor()).toBeNull();
  });

  it('still commits a value that parses', () => {
    const { onEdit } = renderGrid();
    typeInto('grid-cell-0-length', '42');
    fireEvent.keyDown(screen.getByTestId('grid-cell-editor'), { key: 'Enter' });
    expect(onEdit).toHaveBeenCalledWith(0, 'length', 42);
  });

  it('still rejects a value validate refuses', () => {
    const { onEdit, onClearMissing } = renderGrid();
    typeInto('grid-cell-0-length', '500');
    fireEvent.keyDown(screen.getByTestId('grid-cell-editor'), { key: 'Enter' });
    expect(onEdit).not.toHaveBeenCalled();
    expect(onClearMissing).not.toHaveBeenCalled();
    expect(editor()).toHaveValue('500');
  });
});

describe('SpreadsheetGrid Tab out of a rejected draft', () => {
  const activeCellId = () =>
    document.querySelector('[data-active-cell="true"]')?.getAttribute('data-testid');

  it('leaves the focus on the cell whose editor is still open', () => {
    renderGrid();
    typeInto('grid-cell-0-length', 'abc');
    fireEvent.keyDown(screen.getByTestId('grid-cell-editor'), { key: 'Tab' });

    expect(editor()).toHaveValue('abc');
    expect(activeCellId()).toBe('grid-cell-0-length');
  });

  it('still advances after a draft that commits', () => {
    const { onEdit } = renderGrid();
    typeInto('grid-cell-0-length', '7');
    fireEvent.keyDown(screen.getByTestId('grid-cell-editor'), { key: 'Tab' });

    expect(onEdit).toHaveBeenCalledWith(0, 'length', 7);
    expect(editor()).toBeNull();
    expect(activeCellId()).toBe('grid-cell-0-left');
  });
});

describe('SpreadsheetGrid Space', () => {
  it('presses a click-only cell that has an activate handler', () => {
    cycled.length = 0;
    renderGrid();
    fireEvent.mouseDown(screen.getByTestId('grid-cell-1-kind'));
    fireEvent.keyDown(scroller(), { key: ' ' });

    expect(cycled).toEqual([{ row: 1, dir: 1 }]);
    expect(editor()).toBeNull();
  });

  it('types a space into an ordinary cell instead of scrolling the grid', () => {
    // The cell has no button, so Space is just a printable character - the
    // type-to-edit path, exactly as it was before click-only columns existed.
    renderGrid();
    fireEvent.mouseDown(screen.getByTestId('grid-cell-0-left'));
    const event = fireEvent.keyDown(scroller(), { key: ' ' });

    expect(editor()).toHaveValue(' ');
    expect(event).toBe(false); // preventDefault() ran, so the page does not scroll
  });
});

describe('SpreadsheetGrid bulk edits with one cell in them', () => {
  it('sends a lone clear through onBatchEdit, not the single-edit path', () => {
    // A batch of one is still a batch. `onEdit` is the interactive path, and
    // hosts implement it against the row the user is sitting on - which is not
    // where a paste that grew the table put its one value.
    const onBatchEdit = vi.fn<(edits: CellEdit[]) => void>();
    const { onEdit } = renderGrid({ onBatchEdit });
    fireEvent.mouseDown(screen.getByTestId('grid-cell-0-left'));
    fireEvent.keyDown(scroller(), { key: 'Delete' });

    expect(onBatchEdit).toHaveBeenCalledWith([{ row: 0, colKey: 'left', value: null }]);
    expect(onEdit).not.toHaveBeenCalled();
  });

  it('falls back to onEdit per cell when the host has no batch callback', () => {
    const { onEdit } = renderGrid();
    fireEvent.mouseDown(screen.getByTestId('grid-cell-0-left'));
    fireEvent.keyDown(scroller(), { key: 'Delete' });
    expect(onEdit).toHaveBeenCalledWith(0, 'left', null);
  });

  it('emits nothing for a clear that would rewrite an already-empty cell', () => {
    const onBatchEdit = vi.fn<(edits: CellEdit[]) => void>();
    const { onEdit } = renderGrid({ onBatchEdit });
    fireEvent.mouseDown(screen.getByTestId('grid-cell-1-left'));
    fireEvent.keyDown(scroller(), { key: 'Delete' });

    expect(onBatchEdit).not.toHaveBeenCalled();
    expect(onEdit).not.toHaveBeenCalled();
  });
});
