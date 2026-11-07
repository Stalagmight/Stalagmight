import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';

vi.mock('@tanstack/react-virtual', () => import('../../../../test/factories/virtualizer'));

import { SpreadsheetGrid } from '../SpreadsheetGrid';
import type { GridColumn } from '../types';

interface Row {
  id: number;
  value: number;
}

const rows: Row[] = [{ id: 42, value: 10 }];
const columns: GridColumn<Row>[] = [
  {
    key: 'value',
    header: 'Value',
    getValue: (row) => row.value,
    editable: true,
    kind: 'number',
  },
];

describe('SpreadsheetGrid semantic test IDs', () => {
  it('adds clickable row and cell aliases without replacing index-based IDs', () => {
    const onActiveCellChange = vi.fn();
    render(
      <SpreadsheetGrid
        columns={columns}
        rows={rows}
        rowKey={(row) => row.id}
        rowTestId={(row) => `shot-row-7-${row.id}`}
        cellTestId={(row, _index, colKey) => `shot-cell-7-${row.id}-${colKey}`}
        onEdit={vi.fn()}
        onActiveCellChange={onActiveCellChange}
        height={120}
      />,
    );

    expect(screen.getByTestId('grid-row-0')).toBeInTheDocument();
    expect(screen.getByTestId('grid-cell-0-value')).toBeInTheDocument();
    fireEvent.mouseDown(screen.getByTestId('shot-row-7-42'));
    fireEvent.mouseDown(screen.getByTestId('shot-cell-7-42-value'));
    expect(onActiveCellChange).toHaveBeenLastCalledWith(0, 'value');
  });
});
