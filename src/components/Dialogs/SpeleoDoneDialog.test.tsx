/**
 * Component test - #260: a failed "Upload & release" is readable and actionable
 * WHERE THE USER IS.
 *
 * The defect: the dialog correctly stayed open when the upload threw (the work is
 * not done), but said nothing about why. The durable failure surface was the shell
 * banner, which renders behind this dialog's modal scrim - dimmed, unreadable, and
 * with its Retry/Dismiss unclickable. So at the exact moment the user needed the
 * error they could neither read nor act on it.
 *
 * The contract locked here:
 *  1. the failure renders INLINE in the dialog after a failed attempt;
 *  2. the retry is the dialog's EXISTING primary button (no second control), and
 *     re-pressing it re-attempts cleanly - the inline error clears, then updates;
 *  3. a success clears the inline notice AND the store's banner slot, and closes.
 * The "banner stays hidden while this dialog is open / returns after it closes"
 * half of the contract lives in SpeleoBanner.test.tsx.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (_e: unknown, f?: string) => f ?? 'the server said no',
}));

import SpeleoDoneDialog from './SpeleoDoneDialog';
import { useSpeleoStore, type SpeleoLinkState } from '../../store/speleoStore';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';

const LINK: SpeleoLinkState = {
  permission: 'READ_AND_WRITE',
  editState: 'editing',
  lockHolder: null,
  lockedByOther: false,
  hasUnpushedEdits: true,
  unwound: false,
  forceEditing: false,
  forceEditAllowed: false,
  undoMode: 'full',
  remoteState: 'up_to_date',
  noAccess: false,
  serverError: null,
};

function fillCommitMessage() {
  const input = screen.queryByTestId('speleodb-done-message') as HTMLTextAreaElement | null;
  if (input && !input.value) fireEvent.change(input, { target: { value: 'Survey update' } });
}

beforeEach(() => {
  invokeIpcMock.mockReset();
  // A georeferenced zero datum keeps the unrelated `no_anchor` notice out of the
  // banner slot, so the assertions below are about the push-failure surface only.
  useProjectStore.setState({
    project: { zero_datums: [{}] } as never,
    saveProject: vi.fn(async () => ({ success: true }) as never),
  });
  useSpeleoStore.setState({ link: { ...LINK }, banner: null, busy: false });
  useUIStore.setState({
    activeDialog: 'speleodb-done',
    dialogData: {},
    showToast: vi.fn(),
  });
});

describe('SpeleoDoneDialog - a failed upload is surfaced inside the dialog (#260)', () => {
  it('shows nothing until an attempt actually fails', () => {
    render(<SpeleoDoneDialog />);
    expect(screen.queryByTestId('speleodb-push-failure-inline')).not.toBeInTheDocument();
  });

  it('renders the humanized failure inline and keeps the dialog open', async () => {
    invokeIpcMock.mockRejectedValue(new Error('blackholed'));
    render(<SpeleoDoneDialog />);

    fillCommitMessage();
    await userEvent.click(screen.getByTestId('speleodb-done-confirm'));

    const notice = await screen.findByTestId('speleodb-push-failure-inline');
    // The SAME humanized line the shell banner carries - one source of truth.
    const failure = useSpeleoStore.getState().banner;
    expect(failure).toMatchObject({ kind: 'push_failed', origin: 'done' });
    expect(notice.textContent).toContain((failure as { message: string }).message);
    // The dialog is still up (the work is NOT done) and the dialog was not closed.
    expect(screen.getByTestId('speleodb-done-dialog')).toBeInTheDocument();
    expect(useUIStore.getState().activeDialog).toBe('speleodb-done');
  });

  it('offers no second retry control - the existing primary button is the retry', async () => {
    invokeIpcMock.mockRejectedValue(new Error('blackholed'));
    render(<SpeleoDoneDialog />);

    fillCommitMessage();
    await userEvent.click(screen.getByTestId('speleodb-done-confirm'));
    await screen.findByTestId('speleodb-push-failure-inline');

    expect(screen.queryByTestId('speleo-banner-retry-push')).not.toBeInTheDocument();
    expect(screen.getByTestId('speleodb-done-confirm')).toBeEnabled();
  });

  it('re-pressing the button retries with the typed message and updates the inline error', async () => {
    invokeIpcMock.mockRejectedValue(new Error('blackholed'));
    render(<SpeleoDoneDialog />);
    await userEvent.type(screen.getByTestId('speleodb-done-message'), 'sump survey');
    fillCommitMessage();
    await userEvent.click(screen.getByTestId('speleodb-done-confirm'));
    await screen.findByTestId('speleodb-push-failure-inline');

    fillCommitMessage();
    await userEvent.click(screen.getByTestId('speleodb-done-confirm'));

    await waitFor(() =>
      expect(
        invokeIpcMock.mock.calls.filter(
          (c) =>
            c[0] === 'speleodb_done' && (c[1] as { message?: string }).message === 'sump survey',
        ).length,
      ).toBe(2),
    );
    // Still shown (the retry failed too) - one notice, from the LATEST attempt.
    expect(screen.getAllByTestId('speleodb-push-failure-inline')).toHaveLength(1);
  });

  it('a successful retry clears the inline notice and the banner slot, and closes', async () => {
    invokeIpcMock.mockRejectedValueOnce(new Error('blackholed'));
    render(<SpeleoDoneDialog />);
    fillCommitMessage();
    await userEvent.click(screen.getByTestId('speleodb-done-confirm'));
    await screen.findByTestId('speleodb-push-failure-inline');

    invokeIpcMock.mockResolvedValue({ kind: 'released' });
    fillCommitMessage();
    await userEvent.click(screen.getByTestId('speleodb-done-confirm'));

    await waitFor(() => expect(useUIStore.getState().activeDialog).toBeNull());
    // BOTH surfaces are gone: the store's push_failed slot is cleared, so nothing
    // is left for the shell banner to re-raise once the dialog is down.
    expect(useSpeleoStore.getState().banner).toBeNull();
    expect(screen.queryByTestId('speleodb-push-failure-inline')).not.toBeInTheDocument();
  });

  it('push mode surfaces its failure the same way', async () => {
    useUIStore.setState({ activeDialog: 'speleodb-done', dialogData: { mode: 'push' } });
    invokeIpcMock.mockRejectedValue(new Error('blackholed'));
    render(<SpeleoDoneDialog />);

    fillCommitMessage();
    await userEvent.click(screen.getByTestId('speleodb-done-confirm'));

    await screen.findByTestId('speleodb-push-failure-inline');
    expect(useSpeleoStore.getState().banner).toMatchObject({
      kind: 'push_failed',
      origin: 'push',
    });
  });
});

it('requires an associated message and saves to disk before uploading', async () => {
  useUIStore.setState({ dialogData: { mode: 'push' } });
  const order: string[] = [];
  useProjectStore.setState({ saveProject: vi.fn(async () => { order.push('save'); return { success: true }; }) as never });
  invokeIpcMock.mockImplementation(async (command) => { if (command === 'speleodb_push') order.push('push'); return { kind: 'pushed' }; });
  render(<SpeleoDoneDialog />);
  const textarea = screen.getByLabelText('Commit message');
  expect(textarea).toHaveAttribute('id', 'speleodb-done-message');
  expect(screen.getByTestId('speleodb-done-confirm')).toBeDisabled();
  fireEvent.change(textarea, { target: { value: 'Synthetic update' } });
  await userEvent.click(screen.getByTestId('speleodb-done-confirm'));
  await waitFor(() => expect(order.slice(0, 2)).toEqual(['save', 'push']));
});
it('disk-save failure prevents upload', async () => {
  useUIStore.setState({ dialogData: { mode: 'push' }, openDialog: vi.fn() });
  useProjectStore.setState({ saveProject: vi.fn(async () => ({ success: false, error: 'Synthetic disk failure' })) as never });
  render(<SpeleoDoneDialog />);
  fillCommitMessage();
  await userEvent.click(screen.getByTestId('speleodb-done-confirm'));
  expect(invokeIpcMock).not.toHaveBeenCalled();
});
it.each(['not_editing', 'divergence_needed', 'push_unconfirmed'])('retains the message after %s without reporting combined success', async (kind) => {
  useUIStore.setState({ dialogData: { mode: 'push' } });
  invokeIpcMock.mockResolvedValue({ kind });
  render(<SpeleoDoneDialog />);
  fillCommitMessage();
  await userEvent.click(screen.getByTestId('speleodb-done-confirm'));
  expect(await screen.findByTestId('speleodb-save-upload-failure')).toHaveTextContent(kind === 'push_unconfirmed' ? 'could not be confirmed' : 'but not uploaded');
  expect(screen.getByLabelText('Commit message')).toHaveValue('Survey update');
  expect(useUIStore.getState().activeDialog).toBe('speleodb-done');
});
it('blocks duplicate confirmation while saving and ignores a replacement dialog', async () => {
  useUIStore.setState({ dialogData: { mode: 'push' } });
  let finish!: (result: { success: boolean }) => void;
  const save = vi.fn(() => new Promise<{ success: boolean }>((resolve) => { finish = resolve; }));
  useProjectStore.setState({ saveProject: save as never });
  render(<SpeleoDoneDialog />);
  fillCommitMessage();
  fireEvent.click(screen.getByTestId('speleodb-done-confirm'));
  fireEvent.click(screen.getByTestId('speleodb-done-confirm'));
  expect(save).toHaveBeenCalledTimes(1);
  useUIStore.setState({ activeDialog: 'replacement', dialogData: {} });
  finish({ success: true });
  await waitFor(() => expect(screen.getByTestId('speleodb-done-confirm')).not.toBeDisabled());
  expect(invokeIpcMock).not.toHaveBeenCalled();
  expect(useUIStore.getState().activeDialog).toBe('replacement');
});
