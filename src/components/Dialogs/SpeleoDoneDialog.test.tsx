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
import { act, render, screen, waitFor, fireEvent } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (_e: unknown, f?: string) => f ?? 'the server said no',
}));

import SpeleoDoneDialog from './SpeleoDoneDialog';
import { getUploadContinuation } from '../../utils/uploadContinuation';
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
  useSpeleoStore.setState({ link: { ...LINK }, banner: null, busy: false, session: { loggedIn: true, userEmail: 'me@x', endpoint: 'https://synthetic.example' } });
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
  expect(screen.queryByTestId('speleodb-upload-progress')).toBeNull();
});
it.each(['not_editing', 'divergence_needed', 'push_unconfirmed'])('retains the message after %s without reporting combined success', async (kind) => {
  useUIStore.setState({ dialogData: { mode: 'push' } });
  invokeIpcMock.mockResolvedValue({ kind });
  render(<SpeleoDoneDialog />);
  fillCommitMessage();
  await userEvent.click(screen.getByTestId('speleodb-done-confirm'));
  if (kind === 'not_editing') expect(await screen.findByTestId('speleodb-push-failure-inline')).toBeVisible();
  else expect(await screen.findByTestId('speleodb-save-upload-failure')).toHaveTextContent(kind === 'push_unconfirmed' ? 'could not be confirmed' : 'but not uploaded');
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

it.each(['read-only', 'signed-out', 'missing-permission'] as const)('explains and blocks upload while %s', async (kind) => {
  useUIStore.setState({ dialogData: { mode: 'push', closeAfterSave: true } });
  useSpeleoStore.setState({
    link: { ...LINK, editState: kind === 'read-only' ? 'read-only' : 'editing', permission: kind === 'missing-permission' ? null : 'READ_AND_WRITE' },
    session: kind === 'signed-out' ? null : { loggedIn: true, userEmail: 'me@x', endpoint: null },
  });
  render(<SpeleoDoneDialog />);
  fillCommitMessage();
  expect(screen.getByTestId('speleodb-upload-blocked')).toBeVisible();
  expect(screen.getByTestId('speleodb-done-confirm')).toBeDisabled();
  expect(invokeIpcMock.mock.calls.filter(([command]) => command === 'speleodb_done' || command === 'speleodb_push')).toHaveLength(0);
});

it.each(['released', 'push_unconfirmed', 'release_failed', 'newer-edits'] as const)('Save and Close only leaves after confirmed upload and release (%s)', async (kind) => {
  const close = vi.fn(async () => ({ success: true }) as never);
  useProjectStore.setState({ isDirty: false, closeProject: close });
  useUIStore.setState({ dialogData: { mode: 'push', closeAfterSave: true } });
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command !== 'speleodb_done') return null;
    if (kind === 'release_failed') throw new Error('Release failed');
    if (kind === 'newer-edits') useProjectStore.setState({ isDirty: true });
    return { kind: kind === 'push_unconfirmed' ? kind : 'released' };
  });
  render(<SpeleoDoneDialog />);
  fillCommitMessage();
  await userEvent.click(screen.getByTestId('speleodb-done-confirm'));
  await waitFor(() => expect(invokeIpcMock.mock.calls.some(([command]) => command === 'speleodb_done')).toBe(true));
  if (kind === 'released') await waitFor(() => expect(close).toHaveBeenCalledOnce());
  else {
    if (kind === 'newer-edits') await waitFor(() => expect(screen.getByTestId('speleodb-save-upload-failure')).toHaveTextContent('Newer changes'));
    else await waitFor(() => expect(screen.getByTestId('speleodb-done-confirm')).not.toBeDisabled());
    expect(close).not.toHaveBeenCalled();
    expect(useUIStore.getState().activeDialog).toBe('speleodb-done');
  }
});


it('Save progress blocks the app only during upload after the local save completes', async () => {
  useUIStore.setState({ dialogData: { mode: 'push' } });
  let saved!: (value: { success: boolean }) => void;
  let uploaded!: (value: { kind: 'pushed' }) => void;
  const savedPromise = new Promise<{ success: boolean }>((resolve) => { saved = resolve; });
  useProjectStore.setState({ saveProject: vi.fn(() => savedPromise) as never });
  invokeIpcMock.mockImplementation((cmd) => cmd === 'speleodb_push' ? new Promise((resolve) => { uploaded = resolve; }) : Promise.resolve(null));
  render(<SpeleoDoneDialog />);
  fillCommitMessage();
  fireEvent.click(screen.getByTestId('speleodb-done-confirm'));
  expect(screen.queryByTestId('speleodb-upload-progress')).toBeNull();
  await act(async () => saved({ success: true }));
  const progress = screen.getByTestId('speleodb-upload-progress');
  expect(progress).toHaveFocus();
  expect(progress).toHaveAttribute('aria-busy', 'true');
  expect(screen.getByTestId('speleodb-done-dialog').closest('[inert]')).not.toBeNull();
  fireEvent.keyDown(progress, { key: 'Escape' });
  expect(useUIStore.getState().activeDialog).toBe('speleodb-done');
  await act(async () => uploaded({ kind: 'pushed' }));
  expect(screen.queryByTestId('speleodb-upload-progress')).toBeNull();
  expect(useUIStore.getState().activeDialog).toBeNull();
});

it('Save upload progress expires after 120 seconds and retains the description without retrying', async () => {
  useProjectStore.setState({ project: { file_path: '/progress.stalx', zero_datums: [{}] } as never });
  useUIStore.setState({ dialogData: { mode: 'push' } });
  let uploaded!: (value: { kind: 'pushed' }) => void;
  invokeIpcMock.mockImplementation((cmd) => cmd === 'speleodb_push' ? new Promise((resolve) => { uploaded = resolve; }) : Promise.resolve(null));
  render(<SpeleoDoneDialog />);
  fillCommitMessage();
  vi.useFakeTimers();
  try {
    await act(async () => fireEvent.click(screen.getByTestId('speleodb-done-confirm')));
    expect(screen.getByTestId('speleodb-upload-progress')).toBeInTheDocument();
    act(() => vi.advanceTimersByTime(119_999));
    expect(screen.getByTestId('speleodb-upload-progress')).toBeInTheDocument();
    act(() => vi.advanceTimersByTime(1));
    expect(screen.queryByTestId('speleodb-upload-progress')).toBeNull();
    expect(screen.getByTestId('speleodb-upload-timeout')).toHaveTextContent('120 seconds');
    expect(screen.getByTestId('speleodb-done-confirm')).toBeDisabled();
    expect(screen.getByTestId('speleodb-done-cancel')).toBeEnabled();
    fireEvent.click(screen.getByTestId('speleodb-done-cancel'));
    expect(getUploadContinuation('/progress.stalx')?.message).toBe('Survey update');
    useUIStore.setState({ activeDialog: 'settings', dialogData: {} });
    await act(async () => uploaded({ kind: 'pushed' }));
    expect(useUIStore.getState().activeDialog).toBe('settings');
    expect(invokeIpcMock.mock.calls.filter(([cmd]) => cmd === 'speleodb_push')).toHaveLength(1);
    // frontend#2: the dialog is gone, but the result is still applied and reported.
    expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(false);
    expect(useUIStore.getState().showToast).toHaveBeenCalledWith(expect.stringContaining('pushed'), 'speleodb-pushed', 'success');
    expect(getUploadContinuation('/progress.stalx')).toBeNull();
  } finally { vi.useRealTimers(); }
});

it('a failure that lands after the dialog was closed raises the banner and a toast (frontend#2)', async () => {
  useProjectStore.setState({ project: { file_path: '/late.stalx', zero_datums: [{}] } as never });
  useUIStore.setState({ dialogData: { mode: 'push' } });
  let fail!: (error: Error) => void;
  invokeIpcMock.mockImplementation((cmd) => cmd === 'speleodb_push' ? new Promise((_resolve, reject) => { fail = reject; }) : Promise.resolve(null));
  render(<SpeleoDoneDialog />);
  fillCommitMessage();
  vi.useFakeTimers();
  try {
    await act(async () => fireEvent.click(screen.getByTestId('speleodb-done-confirm')));
    act(() => vi.advanceTimersByTime(120_000));
    fireEvent.click(screen.getByTestId('speleodb-done-cancel'));
    expect(useUIStore.getState().activeDialog).toBeNull();
    await act(async () => fail(new Error('Synthetic late failure')));
    expect(useSpeleoStore.getState().banner?.kind).toBe('push_failed');
    expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(true);
    expect(useUIStore.getState().showToast).toHaveBeenCalledWith(expect.stringContaining('did not reach SpeleoDB'), 'speleodb-late-upload', 'error');
    // The description survives for the retry.
    expect(getUploadContinuation('/late.stalx')?.message).toBe('Survey update');
  } finally { vi.useRealTimers(); }
});

it('the older-version confirmation replacing the dialog is not a walked-away upload: no late toast', async () => {
  useUIStore.setState({
    dialogData: { mode: 'push' },
    // The single dialog slot: the confirmation evicts this dialog.
    openDialog: ((name: string) => useUIStore.setState({ activeDialog: name, dialogData: {} })) as never,
  });
  useSpeleoStore.setState({ link: { ...LINK, unwound: true } });
  render(<SpeleoDoneDialog />);
  fillCommitMessage();
  await userEvent.click(screen.getByTestId('speleodb-done-confirm'));
  await waitFor(() => expect(useUIStore.getState().activeDialog).toBe('speleodb-unwound-push'));
  expect(useUIStore.getState().showToast).not.toHaveBeenCalledWith(expect.anything(), 'speleodb-late-upload', expect.anything());
});

it('Finish editing applies a release that lands after the dialog was closed (frontend#2)', async () => {
  useUIStore.setState({ dialogData: {} });
  let released!: (value: { kind: 'released' }) => void;
  invokeIpcMock.mockImplementation((cmd) => cmd === 'speleodb_done' ? new Promise((resolve) => { released = resolve; }) : Promise.resolve(null));
  render(<SpeleoDoneDialog />);
  fillCommitMessage();
  vi.useFakeTimers();
  try {
    await act(async () => fireEvent.click(screen.getByTestId('speleodb-done-confirm')));
    act(() => vi.advanceTimersByTime(120_000));
    fireEvent.click(screen.getByTestId('speleodb-done-cancel'));
    await act(async () => released({ kind: 'released' }));
    expect(useSpeleoStore.getState().link?.editState).toBe('read-only');
    expect(useUIStore.getState().showToast).toHaveBeenCalledWith(expect.any(String), 'speleodb-released', 'success');
  } finally { vi.useRealTimers(); }
});

it('Save upload progress disappears on failure while the description remains available for retry', async () => {
  useUIStore.setState({ dialogData: { mode: 'push' } });
  let fail!: (error: Error) => void;
  invokeIpcMock.mockImplementation((cmd) => cmd === 'speleodb_push' ? new Promise((_resolve, reject) => { fail = reject; }) : Promise.resolve(null));
  render(<SpeleoDoneDialog />);
  fillCommitMessage();
  await act(async () => fireEvent.click(screen.getByTestId('speleodb-done-confirm')));
  expect(screen.getByTestId('speleodb-upload-progress')).toBeInTheDocument();
  await act(async () => fail(new Error('Synthetic upload failure')));
  expect(screen.queryByTestId('speleodb-upload-progress')).toBeNull();
  expect(screen.getByTestId('speleodb-push-failure-inline')).toBeVisible();
  expect(screen.getByTestId('speleodb-done-message')).toHaveValue('Survey update');
});

it('opened by Save without the lock, it takes the lock once so the upload can go ahead (frontend#6)', async () => {
  useUIStore.setState({ dialogData: { mode: 'push', acquire: true } });
  useSpeleoStore.setState({ link: { ...LINK, editState: 'read-only' }, remoteProjectId: 'p1' });
  invokeIpcMock.mockImplementation(async (command: string) => (command === 'speleodb_start_editing' ? { kind: 'editing' } : null));
  render(<SpeleoDoneDialog />);
  await waitFor(() => expect(useSpeleoStore.getState().link?.editState).toBe('editing'));
  expect(invokeIpcMock.mock.calls.filter(([command]) => command === 'speleodb_start_editing')).toHaveLength(1);
  fillCommitMessage();
  expect(screen.queryByTestId('speleodb-upload-blocked')).toBeNull();
  expect(screen.getByTestId('speleodb-done-confirm')).toBeEnabled();
});

it('a refused lock leaves the blocked panel, with Save on this computer as the way out (frontend#6)', async () => {
  const save = vi.fn(async () => ({ success: true }));
  useProjectStore.setState({ isDirty: true, saveProject: save as never });
  useUIStore.setState({ dialogData: { mode: 'push', acquire: true } });
  useSpeleoStore.setState({ link: { ...LINK, editState: 'read-only' }, remoteProjectId: 'p1' });
  invokeIpcMock.mockImplementation(async (command: string) => (command === 'speleodb_start_editing' ? { kind: 'locked_by', email: 'other@example.com' } : null));
  render(<SpeleoDoneDialog />);
  await waitFor(() => expect(screen.getByTestId('speleodb-upload-save-local')).toBeEnabled());
  expect(screen.getByTestId('speleodb-done-confirm')).toBeDisabled();
  await userEvent.click(screen.getByTestId('speleodb-upload-save-local'));
  await waitFor(() => expect(useUIStore.getState().activeDialog).toBeNull());
  expect(save).toHaveBeenCalledOnce();
  expect(useUIStore.getState().showToast).toHaveBeenCalledWith(expect.stringContaining('Saved on this computer'), 'speleodb-saved-locally', 'warning');
  expect(invokeIpcMock.mock.calls.filter(([command]) => command === 'speleodb_push' || command === 'speleodb_done')).toHaveLength(0);
});

it('refuses a description longer than SpeleoDB can store, counting a line break as two (sdk#2)', async () => {
  useUIStore.setState({ dialogData: { mode: 'push' } });
  render(<SpeleoDoneDialog />);
  const textarea = screen.getByTestId('speleodb-done-message');
  fireEvent.change(textarea, { target: { value: 'x'.repeat(1000) } });
  expect(screen.getByTestId('speleodb-done-message-count')).toHaveTextContent('1000 / 1000');
  expect(screen.queryByTestId('speleodb-done-message-too-long')).toBeNull();
  expect(screen.getByTestId('speleodb-done-confirm')).toBeEnabled();
  // 1000 typed characters, one of them a line break: 1001 once stored.
  fireEvent.change(textarea, { target: { value: `${'a'.repeat(500)}\n${'b'.repeat(499)}` } });
  expect(screen.getByTestId('speleodb-done-message-count')).toHaveTextContent('1001 / 1000');
  expect(screen.getByTestId('speleodb-done-message-too-long')).toHaveTextContent('line break counts as two');
  expect(screen.getByTestId('speleodb-done-confirm')).toBeDisabled();
  fireEvent.click(screen.getByTestId('speleodb-done-confirm'));
  expect(invokeIpcMock).not.toHaveBeenCalled();
});
