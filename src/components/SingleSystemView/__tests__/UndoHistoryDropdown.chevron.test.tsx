/**
 * Demo finding I-11: the chevron inside the Undo / Redo button reads as "show
 * the list", so a click on it opens the history; only a click on the rest of
 * the button performs the single undo. Injection: dropping the
 * `HISTORY_TOGGLE_ATTR` branch from `handleButtonClick` makes the chevron
 * click undo (the first test fails on `onSingleAction`).
 */
import { afterEach, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';

vi.mock('../../../utils/ipc', () => ({ invokeIpc: vi.fn(), getErrorMessage: String }));

import '../../../i18n';
import UndoHistoryDropdown, { HISTORY_TOGGLE_ATTR } from '../UndoHistoryDropdown';
import { useProjectStore } from '../../../store/projectStore';
import type { UndoHistoryItem } from '../../../types/ipc.generated';

const ITEMS: UndoHistoryItem[] = [
  {
    index: 0,
    description: 'Delete shot Far Room: 1 → 2',
    timestamp: null,
    commandType: 'delete_shot',
    previewData: { changes: [], additionalChanges: 0 },
  },
];

afterEach(() => {
  cleanup();
});

function renderUndo(onSingleAction: () => void) {
  useProjectStore.setState({ getUndoStack: vi.fn(async () => ITEMS), canUndo: true });
  render(
    <UndoHistoryDropdown mode="undo" disabled={false} onSingleAction={onSingleAction} testId="undo" ariaLabel="Undo">
      <span>
        <span data-testid="label">Undo</span>
        <span data-testid="chevron" {...{ [HISTORY_TOGGLE_ATTR]: '' }}>v</span>
      </span>
    </UndoHistoryDropdown>,
  );
}

it('opens the history on a chevron click and does not undo', async () => {
  const onSingleAction = vi.fn();
  renderUndo(onSingleAction);
  await act(async () => {
    fireEvent.click(screen.getByTestId('chevron'));
  });
  expect(await screen.findByTestId('undo-dropdown')).toBeInTheDocument();
  expect(screen.getByTestId('undo-item-0')).toHaveTextContent('Delete shot Far Room');
  expect(onSingleAction).not.toHaveBeenCalled();

  // A second click on the chevron keeps the list open (it never toggles it
  // shut under a pointer that hovered it open).
  await act(async () => {
    fireEvent.click(screen.getByTestId('chevron'));
  });
  expect(screen.getByTestId('undo-dropdown')).toBeInTheDocument();
  expect(onSingleAction).not.toHaveBeenCalled();
});

it('still undoes once on a click on the rest of the button', async () => {
  const onSingleAction = vi.fn();
  renderUndo(onSingleAction);
  await act(async () => {
    fireEvent.click(screen.getByTestId('label'));
  });
  expect(onSingleAction).toHaveBeenCalledTimes(1);
  expect(screen.queryByTestId('undo-dropdown')).toBeNull();
});
