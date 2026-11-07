/**
 * #576 FILL-1: a fill reports the cells its target column refused, the way a
 * paste does - in every grid, through the one report callback.
 *
 * Paste always said "N skipped" when a value did not land; the fill paths (drag
 * fill, fill-handle double-click, Ctrl+D / Ctrl+R, the context menu's Fill down)
 * dropped every value the column's parser or validator refused without a word.
 * Decided ("Report refusals grid-wide"): a fill reports only cells whose value
 * was REFUSED; a locked/read-only cell it passes over is not counted (it already
 * looks recessed, and counting it would warn on every fill across frozen rows).
 */
import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';

vi.mock('@tanstack/react-virtual', () => import('../../../../test/factories/virtualizer'));

import { SpreadsheetGrid } from '../SpreadsheetGrid';
import type { GridColumn } from '../types';

interface Row {
  id: number;
  label: string;
  speed: number | null;
  locked: boolean;
}

const columns: GridColumn<Row>[] = [
  { key: 'label', header: 'Label', kind: 'text', getValue: (r) => r.label },
  {
    key: 'speed',
    header: 'Speed',
    kind: 'number-nullable',
    getValue: (r) => r.speed,
    isEditable: (r) => !r.locked,
    // The Segments grid's rule: no pace at or below zero.
    validate: (v) => (typeof v === 'number' && v <= 0 ? 'must be above zero' : null),
  },
];

function renderGrid(rows: Row[]) {
  const onBatchEdit = vi.fn();
  const onPasteReport = vi.fn();
  render(
    <SpreadsheetGrid<Row>
      columns={columns}
      rows={rows}
      rowKey={(r) => r.id}
      onEdit={vi.fn()}
      onBatchEdit={onBatchEdit}
      onPasteReport={onPasteReport}
      height={300}
    />
  );
  return { onBatchEdit, onPasteReport };
}

const scroller = () => document.querySelector('.sg-grid [tabindex="0"]')!;

/** Select from one cell to another, as a click and a shift-click do. */
function selectRange(from: string, to: string) {
  fireEvent.mouseDown(screen.getByTestId(from));
  fireEvent.mouseUp(screen.getByTestId(from));
  fireEvent.mouseDown(screen.getByTestId(to), { shiftKey: true });
  fireEvent.mouseUp(screen.getByTestId(to));
}

const row = (id: number, speed: number | null, locked = false): Row => ({ id, label: `S${id}`, speed, locked });

describe('fills report refused cells (FILL-1)', () => {
  it('Ctrl+D down a column reports the cells the validator refused', () => {
    // A stored speed of 0, as a hand-edited plan file can carry.
    const { onPasteReport, onBatchEdit } = renderGrid([row(1, 0), row(2, 12), row(3, 14)]);
    selectRange('grid-cell-0-speed', 'grid-cell-2-speed');
    fireEvent.keyDown(scroller(), { key: 'd', ctrlKey: true });

    expect(onPasteReport, 'the fill said nothing about the two refused cells').toHaveBeenCalledWith({
      source: 'fill',
      written: 0,
      clipped: 0,
      skipped: 2,
    });
    expect(onBatchEdit).not.toHaveBeenCalled();
  });

  it('Ctrl+R across columns reports a value the target column cannot parse', () => {
    const { onPasteReport } = renderGrid([row(1, 10)]);
    selectRange('grid-cell-0-label', 'grid-cell-0-speed');
    fireEvent.keyDown(scroller(), { key: 'r', ctrlKey: true });

    expect(onPasteReport).toHaveBeenCalledWith({ source: 'fill', written: 0, clipped: 0, skipped: 1 });
  });

  // #576 RV3-FE-9: the fill-handle path (commitFill: drag fill and handle
  // double-click) reports on its own, apart from Ctrl+D / Ctrl+R (doFill).
  it('a fill-handle fill reports the cells the validator refused', () => {
    const { onPasteReport, onBatchEdit } = renderGrid([row(1, 0), row(2, 12), row(3, 14)]);
    fireEvent.mouseDown(screen.getByTestId('grid-cell-0-speed'));
    fireEvent.mouseUp(screen.getByTestId('grid-cell-0-speed'));
    // Double-click fills down to the left neighbour's data extent (rows 1-2),
    // tiling the stored 0, which the column refuses in both.
    fireEvent.doubleClick(screen.getByTestId('grid-fill-handle'));

    expect(onPasteReport, 'the fill-handle fill said nothing about the two refused cells').toHaveBeenCalledWith({
      source: 'fill',
      written: 0,
      clipped: 0,
      skipped: 2,
    });
    expect(onBatchEdit).not.toHaveBeenCalled();
  });

  it('does not count a locked cell the fill passes over, and stays quiet when nothing was refused', () => {
    const { onPasteReport, onBatchEdit } = renderGrid([row(1, 9), row(2, 12, true), row(3, 14)]);
    selectRange('grid-cell-0-speed', 'grid-cell-2-speed');
    fireEvent.keyDown(scroller(), { key: 'd', ctrlKey: true });

    expect(onBatchEdit).toHaveBeenCalledWith([{ row: 2, colKey: 'speed', value: 9 }]);
    expect(onPasteReport).not.toHaveBeenCalled();
  });

  it('a paste still reports through the same callback, now saying it was a paste', async () => {
    const { onPasteReport } = renderGrid([row(1, 9), row(2, 12)]);
    fireEvent.mouseDown(screen.getByTestId('grid-cell-0-speed'));
    fireEvent.mouseUp(screen.getByTestId('grid-cell-0-speed'));
    const paste = new Event('paste', { bubbles: true, cancelable: true }) as Event & {
      clipboardData: { getData: (type: string) => string };
    };
    paste.clipboardData = { getData: (type) => (type === 'text/plain' ? '-1\n5' : '') };
    scroller().dispatchEvent(paste);

    expect(onPasteReport).toHaveBeenCalledWith({ source: 'paste', written: 1, clipped: 0, skipped: 1 });
  });
});
