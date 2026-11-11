import { expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import UndoHistoryDropdown from '../UndoHistoryDropdown';
import { useProjectStore } from '../../../store/projectStore';
vi.mock('../../../utils/ipc', () => ({ invokeIpc: vi.fn(), getErrorMessage: String }));
it('keeps empty local Undo inert; remote history belongs to the workspace', () => {
  useProjectStore.setState({ getUndoStack: vi.fn(async () => []), canUndo: false });
  render(<UndoHistoryDropdown mode="undo" disabled onSingleAction={vi.fn()} testId="local-undo" ariaLabel="Undo">Undo</UndoHistoryDropdown>);
  const button = screen.getByRole('button', { name: 'Undo' });
  expect(button).toBeDisabled(); fireEvent.mouseEnter(button);
  expect(screen.queryByTestId('local-undo-unwind')).toBeNull();
});
