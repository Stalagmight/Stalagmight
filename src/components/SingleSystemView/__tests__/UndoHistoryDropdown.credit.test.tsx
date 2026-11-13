/**
 * A survey-credit edit in the undo history reads as what it did: the backend's
 * label with its shot count ("Edit surveyors (2 shots)"), and a hover preview
 * of the names before and after plus how many shots changed.
 */
import { expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, within } from '@testing-library/react';

vi.mock('../../../utils/ipc', () => ({ invokeIpc: vi.fn(), getErrorMessage: String }));

import '../../../i18n';
import UndoHistoryDropdown from '../UndoHistoryDropdown';
import { useProjectStore } from '../../../store/projectStore';
import type { UndoHistoryItem } from '../../../types/ipc.generated';

const ITEMS: UndoHistoryItem[] = [
  {
    index: 0,
    description: 'Edit surveyors (2 shots)',
    timestamp: null,
    commandType: 'edit_shot_credit',
    previewData: {
      changes: [
        { field: 'Surveyors', oldValue: 'Ann Diver', newValue: 'Ann Diver, Bo Lead' },
        { field: 'Shots', oldValue: '', newValue: '2' },
      ],
      additionalChanges: 0,
    },
  },
];

it('lists a credit edit by its label and previews the names before and after', async () => {
  vi.useFakeTimers();
  try {
    useProjectStore.setState({ getUndoStack: vi.fn(async () => ITEMS), canUndo: true });
    render(
      <UndoHistoryDropdown mode="undo" disabled={false} onSingleAction={vi.fn()} testId="undo" ariaLabel="Undo">
        Undo
      </UndoHistoryDropdown>,
    );
    fireEvent.mouseEnter(screen.getByRole('button', { name: 'Undo' }));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000);
    });
    const item = screen.getByTestId('undo-item-0');
    expect(item).toHaveTextContent('Edit surveyors (2 shots)');
    expect(item.querySelector('svg')).not.toBeNull();

    fireEvent.mouseEnter(item);
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1000);
    });
    const preview = screen.getByTestId('detail-preview-popup');
    expect(within(preview).getByText('Surveyors:', { exact: false })).toBeInTheDocument();
    expect(preview).toHaveTextContent('Ann Diver');
    expect(preview).toHaveTextContent('Ann Diver, Bo Lead');
    expect(preview).toHaveTextContent('Shots: 2');
  } finally {
    vi.useRealTimers();
  }
});
