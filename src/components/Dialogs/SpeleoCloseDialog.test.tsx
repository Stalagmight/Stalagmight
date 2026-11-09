/**
 * Component test - [#270] the close-flow guard, and the lock-less state it newly has
 * to survive.
 *
 * `useProject.handleCloseProject` used to gate this dialog on `editState === 'editing'`
 * alone. That is the same defect `guardQuit` was fixed for: a re-verify divergence, or
 * a `clearSession()` after a dead token, drops to read-only while KEEPING the unpushed
 * edits - so closing a saved-but-unpushed remote project returned straight to the home
 * screen with no notice. The guard now also fires on the unpushed work alone, which
 * means this dialog can be reached with NO lock - where every verb it used to offer
 * ("Push, release & close", "Close & keep the write lock") could only fail.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent, act } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const invokeIpcMock = vi.hoisted(() => vi.fn());
const confirmMock = vi.hoisted(() => vi.fn().mockResolvedValue(true));
vi.mock('@tauri-apps/plugin-dialog', () => ({ confirm: confirmMock }));
vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (_e: unknown, f?: string) => f ?? 'the server said no',
}));

import SpeleoCloseDialog from './SpeleoCloseDialog';
import SpeleoUnwoundPushDialog from './SpeleoUnwoundPushDialog';
import { useSpeleoStore, type SpeleoLinkState } from '../../store/speleoStore';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';

// The real dialog actions, captured before `beforeEach` swaps in spies. The [#303]
// hand-off at the bottom of this file moves the user BETWEEN two dialogs, so
// `activeDialog` has to actually change rather than be recorded.
const REAL_OPEN_DIALOG = useUIStore.getState().openDialog;
const REAL_CLOSE_DIALOG = useUIStore.getState().closeDialog;

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

const seedLink = (o: Partial<SpeleoLinkState> = {}) =>
  useSpeleoStore.setState({ link: { ...LINK, ...o }, banner: null, busy: false });

let closeProject: ReturnType<typeof vi.fn>;
let saveProject: ReturnType<typeof vi.fn>;
let closeDialog: ReturnType<typeof vi.fn>;

function fillCommitMessage() {
  const input = screen.queryByRole('textbox', { name: 'Commit message' }) as HTMLTextAreaElement | null;
  if (input && !input.value) fireEvent.change(input, { target: { value: 'Survey update' } });
}

beforeEach(() => {
  confirmMock.mockReset().mockResolvedValue(true);
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation(async (command, args) => command === 'speleodb_project_access_at' ? (args.path === '/synthetic/remote.stalx' ? { release_context: { session_generation: '1' } } : null) : { kind: 'released' });
  closeProject = vi.fn(async () => ({ success: true }));
  saveProject = vi.fn(async () => ({ success: true }));
  closeDialog = vi.fn();
  useProjectStore.setState({
    // A georeferenced zero datum keeps the unrelated no-anchor notice quiet.
    project: { zero_datums: [{}], file_path: '/synthetic/remote.stalx' } as never,
    isDirty: false,
    closeProject: closeProject as never,
    saveProject: saveProject as never,
  });
  seedLink();
  useUIStore.setState({
    activeDialog: 'speleodb-close',
    dialogData: {},
    closeDialog,
    openDialog: vi.fn(),
    showToast: vi.fn(),
  });
});

describe('SpeleoCloseDialog while we hold the lock', () => {
  it('offers both lock verbs and pushes + releases through `done`', async () => {
    render(<SpeleoCloseDialog />);
    expect(screen.getByTestId('speleodb-close-release')).toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-close-anyway')).not.toBeInTheDocument();

    fillCommitMessage();
    await userEvent.click(screen.getByTestId('speleodb-close-push-release'));

    await waitFor(() => expect(closeProject).toHaveBeenCalled());
    expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_done')).toBe(true);
  });
});

describe('SpeleoCloseDialog with no lock (#270)', () => {
  const noLock = () => seedLink({ editState: 'read-only', hasUnpushedEdits: true });

  it('offers no lock verb at all - only the honest "close anyway"', () => {
    noLock();
    render(<SpeleoCloseDialog />);
    expect(screen.queryByTestId('speleodb-close-push-release')).not.toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-close-release')).not.toBeInTheDocument();
    expect(screen.getByTestId('speleodb-close-anyway')).toBeInTheDocument();
  });

  it('closes without issuing any push or release IPC', async () => {
    noLock();
    render(<SpeleoCloseDialog />);

    await userEvent.click(screen.getByTestId('speleodb-close-anyway'));

    await waitFor(() => expect(closeProject).toHaveBeenCalledWith({ request: expect.any(Number) }));
    expect(
      invokeIpcMock.mock.calls.filter((c) => String(c[0]).startsWith('speleodb_')),
    ).toEqual([]);
  });

  it('still flushes the .stalx first when the project is disk-dirty', async () => {
    // The work is only on this computer, so the one thing that can still protect it
    // is the local save - and the close must abort if that save fails.
    noLock();
    useProjectStore.setState({ isDirty: true });
    render(<SpeleoCloseDialog />);

    await userEvent.click(screen.getByTestId('speleodb-close-anyway'));

    await waitFor(() => expect(saveProject).toHaveBeenCalled());
    expect(closeProject).toHaveBeenCalled();
  });

  it('does NOT close when that save fails', async () => {
    noLock();
    useProjectStore.setState({
      isDirty: true,
      saveProject: vi.fn(async () => ({ success: false, error: 'disk full' })) as never,
    });
    render(<SpeleoCloseDialog />);

    await userEvent.click(screen.getByTestId('speleodb-close-anyway'));

    await waitFor(() => expect(screen.getByTestId('speleodb-close-dialog')).toBeInTheDocument());
    expect(closeProject).not.toHaveBeenCalled();
  });

  it('treats a force-edit session as lock-less - there is nothing to push or release', () => {
    // #213 force-editing has the edit gate open WITHOUT a lock.
    seedLink({ editState: 'editing', forceEditing: true, hasUnpushedEdits: true });
    render(<SpeleoCloseDialog />);
    expect(screen.getByTestId('speleodb-close-anyway')).toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-close-push-release')).not.toBeInTheDocument();
  });
});

/**
 * The other two hand-back exits (#270) route through this same dialog rather than
 * getting one of their own, and carry their request in. A prompt that swallowed the
 * action that raised it would only trade a silent swap for a dead end.
 */
describe('SpeleoCloseDialog replays the exit that raised it', () => {
  let loadProject: ReturnType<typeof vi.fn>;
  let openDialog: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    loadProject = vi.fn(async () => ({ success: true }));
    openDialog = vi.fn();
    useProjectStore.setState({ loadProject: loadProject as never });
    useUIStore.setState({ openDialog: openDialog as never });
  });

  it('opens the chosen project after the close lands', async () => {
    seedLink({ editState: 'read-only', hasUnpushedEdits: true });
    useUIStore.setState({ dialogData: { pendingOpenPath: '/cave/other.stalx' } });
    render(<SpeleoCloseDialog />);
    expect(screen.getByTestId('speleodb-close-pending-note')).toBeInTheDocument();

    await userEvent.click(screen.getByTestId('speleodb-close-anyway'));

    await waitFor(() => expect(loadProject).toHaveBeenCalledWith('/cave/other.stalx', { accessConfirmed: true, request: expect.any(Number) }));
  });

  it('reopens the new-project dialog after the close lands', async () => {
    seedLink({ editState: 'read-only', hasUnpushedEdits: true });
    useUIStore.setState({ dialogData: { pendingNewProject: true } });
    render(<SpeleoCloseDialog />);

    await userEvent.click(screen.getByTestId('speleodb-close-anyway'));

    await waitFor(() => expect(openDialog).toHaveBeenCalledWith('new-project'));
  });

  // A close that did NOT happen leaves the project open, so replaying would open a
  // second project over one the user never left.
  it('replays nothing when the close itself fails', async () => {
    seedLink({ editState: 'read-only', hasUnpushedEdits: true });
    useUIStore.setState({ dialogData: { pendingOpenPath: '/cave/other.stalx' } });
    useProjectStore.setState({
      closeProject: vi.fn(async () => ({ success: false, error: 'busy' })) as never,
    });
    render(<SpeleoCloseDialog />);

    await userEvent.click(screen.getByTestId('speleodb-close-anyway'));

    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(loadProject).not.toHaveBeenCalled();
  });

  it('shows no follow-up note for a plain Close', () => {
    seedLink({ editState: 'read-only', hasUnpushedEdits: true });
    render(<SpeleoCloseDialog />);
    expect(screen.queryByTestId('speleodb-close-pending-note')).not.toBeInTheDocument();
  });
});

/**
 * [#303] The close half of the stranded-exit blocker.
 *
 * On an unwound copy `done` pushes nothing: it raises `SpeleoUnwoundPushDialog` over this
 * prompt and answers `null`, so `pushReleaseThenClose` stopped there - while the
 * confirmation went on to push and release anyway. The project stayed open, and with it
 * the Open / New Project the close was standing in for (#270), which is the request that
 * was never the user's to lose twice.
 */
describe('SpeleoCloseDialog - the unwound-push confirmation carries the close', () => {
  let loadProject: ReturnType<typeof vi.fn>;

  const DialogHost = () => {
    const active = useUIStore((s) => s.activeDialog);
    if (active === 'speleodb-close') return <SpeleoCloseDialog />;
    if (active === 'speleodb-unwound-push') return <SpeleoUnwoundPushDialog />;
    return null;
  };

  beforeEach(() => {
    loadProject = vi.fn(async () => ({ success: true }));
    useProjectStore.setState({ loadProject: loadProject as never });
    useUIStore.setState({ openDialog: REAL_OPEN_DIALOG, closeDialog: REAL_CLOSE_DIALOG });
  });

  const reachConfirmation = async () => {
    seedLink({ unwound: true, hasUnpushedEdits: true });
    useUIStore.setState({
      activeDialog: 'speleodb-close',
      dialogData: { pendingOpenPath: '/cave/other.stalx' },
    });
    render(<DialogHost />);
    fillCommitMessage();
    await userEvent.click(screen.getByTestId('speleodb-close-push-release'));
    return screen.findByTestId('speleodb-unwound-push-dialog');
  };

  it('closes AND replays the open that raised the prompt, once the push releases', async () => {
    await reachConfirmation();
    expect(closeProject).not.toHaveBeenCalled();

    await userEvent.click(screen.getByTestId('speleodb-unwound-push-push-anyway'));

    await waitFor(() => expect(loadProject).toHaveBeenCalledWith('/cave/other.stalx', { accessConfirmed: true, request: expect.any(Number) }));
    expect(closeProject).toHaveBeenCalled();
    expect(useUIStore.getState().activeDialog).toBeNull();
  });

  it('puts the close prompt back on Cancel, with the pending open intact', async () => {
    await reachConfirmation();

    await userEvent.click(screen.getByTestId('speleodb-unwound-push-cancel'));

    expect(useUIStore.getState().activeDialog).toBe('speleodb-close');
    expect(useUIStore.getState().dialogData).toEqual({ pendingOpenPath: '/cave/other.stalx' });
    expect(screen.getByTestId('speleodb-close-pending-note')).toBeInTheDocument();
    expect(closeProject).not.toHaveBeenCalled();
  });
});


describe('close and project-switch release safety', () => {
  it('requires a commit message before uploading', () => {
    render(<SpeleoCloseDialog />);
    expect(screen.getByTestId('speleodb-close-push-release')).toBeDisabled();
    fillCommitMessage();
    expect(screen.getByTestId('speleodb-close-push-release')).toBeEnabled();
  });

  it('saves and releases before closing or continuing a project switch', async () => {
    useProjectStore.setState({ isDirty: true });
    render(<SpeleoCloseDialog />);
    await userEvent.click(screen.getByTestId('speleodb-close-release'));
    await waitFor(() => expect(closeProject).toHaveBeenCalled());
    const release = invokeIpcMock.mock.calls.findIndex((c) => c[0] === 'speleodb_release_current_lock');
    expect(release).toBeGreaterThanOrEqual(0);
    expect(saveProject.mock.invocationCallOrder[0]).toBeLessThan(invokeIpcMock.mock.invocationCallOrder[release]!);
    expect(invokeIpcMock.mock.invocationCallOrder[release]).toBeLessThan(closeProject.mock.invocationCallOrder[0]!);
  });

  it('a failed release leaves the project open', async () => {
    invokeIpcMock.mockRejectedValue(new Error('offline'));
    render(<SpeleoCloseDialog />);
    await userEvent.click(screen.getByTestId('speleodb-close-release'));
    await waitFor(() => expect(useUIStore.getState().openDialog).toHaveBeenCalledWith('error', expect.any(Object)));
    expect(closeProject).not.toHaveBeenCalled();
  });
});


it('cancelling restricted destination access preserves the current project before hand-back', async () => {
  const { handBackProject } = await import('../../utils/quit');
  invokeIpcMock.mockResolvedValue({ project: { id: 'p', name: 'Restricted', permission: 'READ_ONLY', lock_holder: null }, endpoint: 'https://example.com', verification: 'verified' });
  confirmMock.mockResolvedValue(false);
  await handBackProject({ pendingOpenPath: '/cold.stalx' });
  expect(closeProject).not.toHaveBeenCalled();
});


it('cancelling destination access from the actual close dialog does not save or release', async () => {
  useUIStore.setState({ dialogData: { pendingOpenPath: '/cold.stalx' } });
  invokeIpcMock.mockResolvedValue({ project: { id: 'p', name: 'Restricted', permission: 'READ_ONLY', lock_holder: null }, endpoint: 'https://example.com', verification: 'verified' });
  confirmMock.mockResolvedValue(false);
  render(<SpeleoCloseDialog />);
  await userEvent.click(screen.getByTestId('speleodb-close-release'));
  expect(closeProject).not.toHaveBeenCalled();
  expect(saveProject).not.toHaveBeenCalled();
  expect(invokeIpcMock.mock.calls.some(([cmd]) => cmd === 'speleodb_release_current_lock')).toBe(false);
});


describe('close destination probe ownership', () => {
  it.each(['speleodb-close-release', 'speleodb-close-push-release'])('protects the entire %s attempt', async (button) => {
    let resolve!: (value: unknown) => void;
    const probe = new Promise((yes) => { resolve = yes; });
    invokeIpcMock.mockImplementation((command) => command === 'speleodb_project_access_at' ? probe : Promise.resolve({ kind: 'released' }));
    useUIStore.setState({ dialogData: { pendingOpenPath: '/synthetic/destination.stalx' } });
    useProjectStore.setState({ isDirty: true });
    render(<SpeleoCloseDialog />);
    fillCommitMessage();
    fireEvent.click(screen.getByTestId(button));
    fireEvent.click(screen.getByTestId(button));
    expect(screen.getByTestId('speleodb-close-cancel')).toBeDisabled();
    expect(screen.getByTestId('speleodb-close-save-locally')).toBeDisabled();
    fireEvent.keyDown(document, { key: 'Escape' });
    fireEvent.click(screen.getByTestId('speleodb-close-dialog-close'));
    expect(closeDialog).not.toHaveBeenCalled();
    confirmMock.mockResolvedValue(false);
    resolve({ project: { permission: 'READ_ONLY', name: 'Synthetic', lock_holder: null }, endpoint: 'https://example.com', verification: 'verified' });
    await waitFor(() => expect(screen.getByTestId(button)).toBeEnabled());
    expect(saveProject).not.toHaveBeenCalled();
    expect(closeProject).not.toHaveBeenCalled();
    expect(invokeIpcMock.mock.calls.map((c) => c[0])).toEqual(['speleodb_project_access_at']);
  });

  it.each(['dialog', 'project', 'session', 'unmount'])('stops after %s replacement during the probe', async (replacement) => {
    let resolve!: (value: unknown) => void;
    invokeIpcMock.mockReturnValue(new Promise((yes) => { resolve = yes; }));
    useUIStore.setState({ dialogData: { pendingOpenPath: '/synthetic/destination.stalx' } });
    const view = render(<SpeleoCloseDialog />);
    fireEvent.click(screen.getByTestId('speleodb-close-release'));
    if (replacement === 'dialog') useUIStore.setState({ dialogData: {} });
    if (replacement === 'project') useProjectStore.setState({ project: { file_path: '/other.stalx' } as never });
    if (replacement === 'session') useSpeleoStore.getState().clearSession();
    if (replacement === 'unmount') view.unmount();
    await act(async () => { resolve(null); });
    expect(invokeIpcMock).toHaveBeenCalledOnce();
    expect(closeProject).not.toHaveBeenCalled();
    expect(saveProject).not.toHaveBeenCalled();
  });
});


it('continues after an ordinary save refreshes the project object', async () => {
  useProjectStore.setState({ isDirty: true });
  saveProject.mockImplementation(async () => {
    useProjectStore.setState((s) => ({ project: { ...s.project, modified_at: 'synthetic update' } as never }));
    return { success: true };
  });
  render(<SpeleoCloseDialog />);
  await userEvent.click(screen.getByTestId('speleodb-close-release'));
  await waitFor(() => expect(closeProject).toHaveBeenCalledOnce());
});


it.each(['speleodb-close-release', 'speleodb-close-push-release'])('continues %s across a benign session refresh', async (button) => {
  let resolve!: (value: unknown) => void;
  const probe = new Promise((yes) => { resolve = yes; });
  invokeIpcMock.mockImplementation((command, args) => command === 'speleodb_project_access_at' ? (args.path === '/synthetic/remote.stalx' ? Promise.resolve({ release_context: { session_generation: '1' } }) : probe) : Promise.resolve({ kind: 'released' }));
  useSpeleoStore.getState().setSession({ logged_in: true, user_email: 'synthetic@example.com', endpoint: 'https://synthetic.example' });
  useUIStore.setState({ dialogData: { pendingOpenPath: '/synthetic/destination.stalx' } });
  render(<SpeleoCloseDialog />);
  fillCommitMessage();
  fireEvent.click(screen.getByTestId(button));
  useSpeleoStore.getState().setSession({ logged_in: true, user_email: 'synthetic@example.com', endpoint: 'https://synthetic.example' });
  await act(async () => { resolve(null); });
  await waitFor(() => expect(closeProject).toHaveBeenCalledOnce());
});

it.each([false, true])('keeps the lock locally without a server call, dirty=%s', async (dirty) => {
  useProjectStore.setState({ isDirty: dirty });
  render(<SpeleoCloseDialog />);
  await userEvent.click(screen.getByTestId('speleodb-close-keep-lock'));
  await waitFor(() => expect(closeProject).toHaveBeenCalledOnce());
  expect(closeProject).toHaveBeenCalledWith({ releaseLock: false, request: expect.any(Number) });
  expect(saveProject).toHaveBeenCalledTimes(dirty ? 1 : 0);
  expect(invokeIpcMock).not.toHaveBeenCalled();
});
it('keeps the project open when the requested keep-lock local save fails', async () => {
  useProjectStore.setState({ isDirty: true });
  saveProject.mockResolvedValue({ success: false, error: 'Synthetic disk failure' });
  render(<SpeleoCloseDialog />);
  await userEvent.click(screen.getByTestId('speleodb-close-keep-lock'));
  expect(closeProject).not.toHaveBeenCalled();
  expect(invokeIpcMock).not.toHaveBeenCalled();
});
it('opens an available linked destination using only a local access inspection', async () => {
  const load = vi.fn(async () => ({ success: true }));
  useProjectStore.setState({ loadProject: load as never });
  useUIStore.setState({ dialogData: { pendingOpenPath: '/destination.stalx' } });
  invokeIpcMock.mockResolvedValue({ project: { name: 'Synthetic destination', permission: null, lock_holder: null }, verification: 'unavailable', endpoint: 'https://example.com' });
  render(<SpeleoCloseDialog />);
  await userEvent.click(screen.getByTestId('speleodb-close-keep-lock'));
  await waitFor(() => expect(load).toHaveBeenCalledOnce());
  expect(invokeIpcMock).toHaveBeenCalledExactlyOnceWith('speleodb_project_access_at', { path: '/destination.stalx', localOnly: true });
  expect(closeProject).toHaveBeenCalledWith({ releaseLock: false, request: expect.any(Number) });
});
it('continues New after keeping the current lock without any access probe', async () => {
  const open = vi.fn();
  useUIStore.setState({ dialogData: { pendingNewProject: true }, openDialog: open });
  render(<SpeleoCloseDialog />);
  await userEvent.click(screen.getByTestId('speleodb-close-keep-lock'));
  await waitFor(() => expect(open).toHaveBeenCalledWith('new-project'));
  expect(invokeIpcMock).not.toHaveBeenCalled();
});
