/**
 * Demo finding N-9: a merge joins two station comments with a blank line, and
 * the one-line Comment cell collapsed the break to a space, so "T-junction,
 * line arrow points out" and "Silt bank, low visibility" read as one run-on
 * sentence. A multiline column's cell now shows the parts separated by " / ";
 * its tooltip, its editor and the clipboard keep the text with its line breaks.
 * Injection: rendering `cellText` as is in the cell fails the first case.
 */
import { describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';

vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () =>
      Array.from({ length: count }, (_, index) => ({ index, key: index, start: 33 + index * 33, size: 33 })),
    getTotalSize: () => count * 33,
    scrollToIndex: vi.fn(),
  }),
}));

import { SpreadsheetGrid } from '../SpreadsheetGrid';
import { oneLineCellText } from '../gridCellText';
import type { GridColumn } from '../types';

interface Row {
  id: number;
  comment: string;
  name: string;
}

const JOINED = 'T-junction, line arrow points out\n\nSilt bank, low visibility';

const columns: GridColumn<Row>[] = [
  { key: 'name', header: 'Name', kind: 'text', getValue: (r) => r.name, editable: true },
  { key: 'comment', header: 'Comment', kind: 'text', multiline: true, getValue: (r) => r.comment, editable: true },
];

function renderGrid(comment: string) {
  render(
    <SpreadsheetGrid<Row>
      columns={columns}
      rows={[{ id: 1, comment, name: 'a\nb' }]}
      rowKey={(r) => r.id}
      onEdit={vi.fn()}
      height={200}
    />,
  );
}

const cellText = (colKey: string) => screen.getByTestId(`grid-cell-0-${colKey}`).querySelector('.sg-cell-text');

describe('a multi-line value in a one-line cell (N-9)', () => {
  it('shows the parts separated, and the full text in the tooltip', () => {
    renderGrid(JOINED);
    const text = cellText('comment');
    expect(text?.textContent).toBe('T-junction, line arrow points out / Silt bank, low visibility');
    expect(text?.getAttribute('title')).toBe(JOINED);
    // A single-line column is left as it is.
    expect(cellText('name')?.textContent).toBe('a\nb');
  });

  it('edits the text with its line breaks', () => {
    renderGrid(JOINED);
    fireEvent.doubleClick(screen.getByTestId('grid-cell-0-comment'));
    expect(screen.getByTestId('grid-cell-editor')).toHaveValue(JOINED);
  });

  it('copies the text with its line breaks', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
    renderGrid(JOINED);
    fireEvent.mouseDown(screen.getByTestId('grid-cell-0-comment'));
    const scroller = document.querySelector('.sg-grid [tabindex="0"]')!;
    await act(async () => {
      fireEvent.keyDown(scroller, { key: 'c', ctrlKey: true });
    });
    expect(writeText).toHaveBeenCalledTimes(1);
    expect(String(writeText.mock.calls[0]?.[0])).toContain('Silt bank, low visibility');
    expect(String(writeText.mock.calls[0]?.[0])).not.toContain(' / ');
  });
});

describe('oneLineCellText', () => {
  it('joins the non-empty lines, trimmed', () => {
    expect(oneLineCellText('one line')).toBe('one line');
    expect(oneLineCellText(JOINED)).toBe('T-junction, line arrow points out / Silt bank, low visibility');
    expect(oneLineCellText('a\r\nb\rc\n')).toBe('a / b / c');
    expect(oneLineCellText('  a  \n\n\n  b ')).toBe('a / b');
    expect(oneLineCellText('\n')).toBe('');
  });
});
