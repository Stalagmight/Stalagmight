import { beforeEach, expect, it, vi } from 'vitest';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import SpeleoUnpushedLocalDialog from './SpeleoUnpushedLocalDialog';
import { useUIStore } from '../../store/uiStore';

const { takeRemote, keepLocal } = vi.hoisted(() => ({ takeRemote: vi.fn(), keepLocal: vi.fn() }));
vi.mock('../../hooks/useOpenRemoteProject', () => ({ useOpenRemoteProject: () => ({
  takeRemoteDiscardingLocal: takeRemote, keepLocalCopy: keepLocal,
}) }));

beforeEach(() => {
  takeRemote.mockReset().mockResolvedValue({ status: 'cancelled' });
  keepLocal.mockReset().mockResolvedValue({ status: 'cancelled' });
  useUIStore.setState({ activeDialog: 'speleodb-unpushed-local', dialogData: {
    summary: { id: 'synthetic', name: 'Synthetic cave' },
    unpushedLocal: { local_path: '/synthetic/cave.stalx', last_edit_at: null },
  } });
});

it('restores retry and dismissal after a discarded-copy open is superseded', async () => {
  render(<SpeleoUnpushedLocalDialog />);
  const take = screen.getByTestId('speleodb-unpushed-local-take-remote');
  await userEvent.click(take); // Arm the destructive choice.
  await userEvent.click(take);
  await waitFor(() => expect(take).toBeEnabled());
  expect(takeRemote).toHaveBeenCalledOnce();
  expect(screen.queryByTestId('speleodb-unpushed-local-error')).not.toBeInTheDocument();
  await userEvent.click(take);
  expect(takeRemote).toHaveBeenCalledTimes(2);
  await userEvent.click(screen.getByTestId('speleodb-unpushed-local-cancel'));
  expect(useUIStore.getState().activeDialog).toBeNull();
});


it('a superseded handler cannot clear a replacement prompt operation', async () => {
  let cancelOld!: (outcome: unknown) => void;
  takeRemote.mockReturnValueOnce(new Promise((resolve) => { cancelOld = resolve; }))
    .mockReturnValueOnce(new Promise(() => {}));
  render(<SpeleoUnpushedLocalDialog />);
  const take = screen.getByTestId('speleodb-unpushed-local-take-remote');
  await userEvent.click(take);
  await userEvent.click(take);
  expect(take).toBeDisabled();
  act(() => useUIStore.setState((state) => ({ dialogData: { ...state.dialogData } })));
  await waitFor(() => expect(take).toBeEnabled());
  await userEvent.click(take);
  await userEvent.click(take);
  expect(take).toBeDisabled();
  await act(async () => { cancelOld({ status: 'cancelled' }); });
  expect(take).toBeDisabled();
  expect(screen.getByTestId('speleodb-unpushed-local-cancel')).toBeDisabled();
});
