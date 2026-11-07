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
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../utils/ipc');
stubGetErrorMessage(fallbackOr('the server said no'));

import SpeleoCloseDialog from './SpeleoCloseDialog';
import SpeleoUnwoundPushDialog from './SpeleoUnwoundPushDialog';
import { useSpeleoStore, type SpeleoLinkState } from '../../store/speleoStore';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { invokeIpcMock, stubGetErrorMessage, fallbackOr } from '../../test/factories/ipc';
import { makeSpeleoLink } from '../../test/factories/speleo';

/** [#576 NF-W8FE-2] Done and Push answer the wire envelope, never a bare outcome. */
function report<R>(result: R): { result: R; save_warnings: [] } {
  return { result, save_warnings: [] };
}

// The real dialog actions, captured before `beforeEach` swaps in spies. The [#303]
// hand-off at the bottom of this file moves the user BETWEEN two dialogs, so
// `activeDialog` has to actually change rather than be recorded.
const REAL_OPEN_DIALOG = useUIStore.getState().openDialog;
const REAL_CLOSE_DIALOG = useUIStore.getState().closeDialog;

const LINK: SpeleoLinkState = makeSpeleoLink({ editState: 'editing', hasUnpushedEdits: true });

const seedLink = (o: Partial<SpeleoLinkState> = {}) =>
  useSpeleoStore.setState({ link: { ...LINK, ...o }, banner: null, busy: false });

let closeProject: ReturnType<typeof vi.fn>;
let saveProject: ReturnType<typeof vi.fn>;
let closeDialog: ReturnType<typeof vi.fn>;

beforeEach(() => {
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue(report({ kind: 'released' }));
  closeProject = vi.fn(async () => ({ success: true }));
  saveProject = vi.fn(async () => ({ success: true }));
  closeDialog = vi.fn();
  useProjectStore.setState({
    // A georeferenced zero datum keeps the unrelated no-anchor notice quiet.
    project: { zero_datums: [{}] } as never,
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
    expect(screen.getByTestId('speleodb-close-keep-lock')).toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-close-anyway')).not.toBeInTheDocument();

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
    expect(screen.queryByTestId('speleodb-close-keep-lock')).not.toBeInTheDocument();
    expect(screen.getByTestId('speleodb-close-anyway')).toBeInTheDocument();
  });

  it('closes without issuing any push or release IPC', async () => {
    noLock();
    render(<SpeleoCloseDialog />);

    await userEvent.click(screen.getByTestId('speleodb-close-anyway'));

    await waitFor(() => expect(closeProject).toHaveBeenCalledWith({ releaseLock: false }));
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

    await waitFor(() => expect(loadProject).toHaveBeenCalledWith('/cave/other.stalx'));
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
    await userEvent.click(screen.getByTestId('speleodb-close-push-release'));
    return screen.findByTestId('speleodb-unwound-push-dialog');
  };

  it('closes AND replays the open that raised the prompt, once the push releases', async () => {
    await reachConfirmation();
    expect(closeProject).not.toHaveBeenCalled();

    await userEvent.click(screen.getByTestId('speleodb-unwound-push-push-anyway'));

    await waitFor(() => expect(loadProject).toHaveBeenCalledWith('/cave/other.stalx'));
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
