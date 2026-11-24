/**
 * RS-FX6, P3-16 (shot 08 of the RS capture): the history list cut every row
 * at 40 characters, so "Merge Side / 0 into Main / 12 (keeps 12)" read
 * "Merge Side / 0 into Main / 12 (kee...": the part that says which station
 * was kept was the part cut. A row now shows its whole description, wrapped
 * to at most three lines. Injection: cut the description again.
 */
import { afterEach, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';

vi.mock('../../../utils/ipc', () => ({ invokeIpc: vi.fn(), getErrorMessage: String }));

import '../../../i18n';
import UndoHistoryDropdown, { HISTORY_ROW_CLASS, HISTORY_TOGGLE_ATTR } from '../UndoHistoryDropdown';
import { useProjectStore } from '../../../store/projectStore';
import type { UndoHistoryItem } from '../../../types/ipc.generated';

const MERGE = 'Merge Side / 0 into Main / 12 (keeps 12)';

afterEach(() => {
  cleanup();
});

it('shows a long description whole, wrapped', async () => {
  const items: UndoHistoryItem[] = [
    { index: 0, description: MERGE, timestamp: null, commandType: 'merge_stations', previewData: { changes: [], additionalChanges: 0 } },
  ];
  useProjectStore.setState({ getUndoStack: vi.fn(async () => items), canUndo: true });
  render(
    <UndoHistoryDropdown mode="undo" disabled={false} onSingleAction={() => {}} testId="undo" ariaLabel="Undo">
      <span data-testid="chevron" {...{ [HISTORY_TOGGLE_ATTR]: '' }}>v</span>
    </UndoHistoryDropdown>,
  );
  await act(async () => {
    fireEvent.click(screen.getByTestId('chevron'));
  });
  const description = await screen.findByTestId('undo-description-0');
  expect(description).toHaveTextContent(MERGE);
  expect(description.textContent).not.toContain('…');
  expect(description.className).toBe(HISTORY_ROW_CLASS);
  expect(HISTORY_ROW_CLASS).toContain('line-clamp-3');
  expect(HISTORY_ROW_CLASS).not.toContain('truncate');
});
