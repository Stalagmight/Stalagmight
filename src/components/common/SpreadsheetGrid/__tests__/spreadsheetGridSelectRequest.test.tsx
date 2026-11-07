import { describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render } from '@testing-library/react';

vi.mock('@tanstack/react-virtual', () => import('../../../../test/factories/virtualizer'));

import { SpreadsheetGrid } from '../SpreadsheetGrid';
import type { GridColumn, GridSelectRequest } from '../types';

/**
 * `selectRequest` (D21, #576): the host's "put the cursor on this row" is
 * honoured once per nonce. The old level-triggered `selectedRow` acted only
 * when its VALUE changed, so asking again for the row the user had moved off
 * did nothing, and a row shift under an old value moved the cursor uninvited.
 */

interface Row {
  id: number;
  value: number;
}

const columns: GridColumn<Row>[] = [
  { key: 'value', header: 'Value', getValue: (r) => r.value, editable: true, kind: 'number' },
];

const rowsOf = (n: number): Row[] => Array.from({ length: n }, (_, i) => ({ id: i, value: i * 10 }));

function renderGrid(selectRequest: GridSelectRequest | null, rowCount = 5) {
  const onActiveCellChange = vi.fn();
  const view = (req: GridSelectRequest | null, n: number) => (
    <SpreadsheetGrid
      columns={columns}
      rows={rowsOf(n)}
      rowKey={(row) => row.id}
      onEdit={vi.fn()}
      onActiveCellChange={onActiveCellChange}
      selectRequest={req}
      height={200}
    />
  );
  const { rerender } = render(view(selectRequest, rowCount));
  return {
    onActiveCellChange,
    update: (req: GridSelectRequest | null, n = rowCount) => rerender(view(req, n)),
  };
}

const activeRow = () =>
  [...document.querySelectorAll('.sg-gutter')].findIndex((g) => g.className.includes('sg-gutter-active'));

const arrowDown = () =>
  act(() => {
    fireEvent.keyDown(document.querySelector('.sg-grid [tabindex="0"]')!, { key: 'ArrowDown' });
  });

describe('SpreadsheetGrid selectRequest', () => {
  it('moves the cursor to the requested row without reporting it back as a user move', () => {
    const { onActiveCellChange } = renderGrid({ row: 2, nonce: 1 });
    expect(activeRow()).toBe(2);
    expect(onActiveCellChange).not.toHaveBeenCalled();
  });

  it('a new nonce for the SAME row re-selects it after the user moved away', () => {
    const { update } = renderGrid({ row: 2, nonce: 1 });
    arrowDown();
    expect(activeRow()).toBe(3);

    update({ row: 2, nonce: 2 });
    expect(activeRow()).toBe(2);
  });

  it('a handled request whose row shifts does not pull the cursor back', () => {
    const { update } = renderGrid({ row: 2, nonce: 1 });
    arrowDown();
    expect(activeRow()).toBe(3);

    // Same request, its row moved down by one (a row inserted above it).
    update({ row: 3, nonce: 1 }, 6);
    arrowDown();
    expect(activeRow()).toBe(4);
    update({ row: 2, nonce: 1 }, 6);
    expect(activeRow()).toBe(4);
  });

  it('an out-of-range request waits until its row exists', () => {
    const { update } = renderGrid({ row: 6, nonce: 1 }, 3);
    expect(activeRow()).toBe(0);

    update({ row: 6, nonce: 1 }, 8);
    expect(activeRow()).toBe(6);
  });
});
