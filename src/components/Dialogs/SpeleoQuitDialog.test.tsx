/**
 * Component test - [#270] the three ways out of a quit with unpushed SpeleoDB
 * edits, and the one way that must NOT exist.
 *
 * The contract locked here:
 *  1. "Push, release & quit" runs the existing `done` hand-off and quits ONLY
 *     once the lock is actually released;
 *  2. a failed push does NOT quit - the dialog stays up with the failure inline;
 *  3. "Keep the write lock & quit" quits with NO release IPC of any kind (that is the
 *     entire point of the option: the backend veto keeps the edits safe behind
 *     the still-held lock);
 *  4. "Discard & release" is two-step, releases explicitly, then quits - and a
 *     failed release does not quit;
 *  5. a disk-dirty project is saved first, and a failed save aborts the quit, so
 *     the two dirty states compose instead of one swallowing the other.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (_e: unknown, f?: string) => f ?? 'the server said no',
}));

const confirmMock = vi.hoisted(() => vi.fn().mockResolvedValue(true));
vi.mock('@tauri-apps/plugin-dialog', () => ({ confirm: confirmMock }));

const performQuitMock = vi.hoisted(() => vi.fn());
// `holdsWriteLock` is the real predicate, not a stub: this dialog adapts its whole
// action set to it, so a mocked-away version would let the cases below pass against
// a lock state the component never actually derived.
vi.mock('../../utils/quit', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/quit')>()),
  performQuit: performQuitMock,
}));

import SpeleoQuitDialog from './SpeleoQuitDialog';
import SpeleoUnwoundPushDialog from './SpeleoUnwoundPushDialog';
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

/** Every IPC the release paths could reach. */
const RELEASE_COMMANDS = [
  'speleodb_release_current_lock',
  'speleodb_release_lock',
  'speleodb_release_all_locks',
  'speleodb_done',
];

const calledCommands = () => invokeIpcMock.mock.calls.map((c) => c[0] as string);

let saveProject: ReturnType<typeof vi.fn>;

function fillCommitMessage() {
  const input = screen.queryByRole('textbox', { name: 'Commit message' }) as HTMLTextAreaElement | null;
  if (input && !input.value) fireEvent.change(input, { target: { value: 'Survey update' } });
}

beforeEach(() => {
  invokeIpcMock.mockReset().mockImplementation(async (command) => command === 'speleodb_project_access_at'
    ? { release_context: { project_id: 'synthetic', endpoint: 'https://example.com', user_email: 'u@x', session_generation: '1' } }
    : null);
  confirmMock.mockReset().mockResolvedValue(true);
  performQuitMock.mockReset();
  saveProject = vi.fn(async () => ({ success: true }) as never);
  // A georeferenced zero datum keeps the unrelated `no_anchor` notice out of the
  // banner slot, so banner assertions are about the push failure only.
  useProjectStore.setState({
    project: { zero_datums: [{}], file_path: '/synthetic/remote.stalx' } as never,
    isDirty: false,
    saveProject: saveProject as never,
    closeProject: vi.fn(async () => { useProjectStore.setState({ project: null }); return { success: true }; }) as never,
  });
  useSpeleoStore.setState({ link: { ...LINK }, banner: null, busy: false });
  useUIStore.setState({
    activeDialog: 'speleodb-quit',
    dialogData: {},
    showToast: vi.fn(),
  });
});

describe('SpeleoQuitDialog - push, release & quit', () => {
  it('runs the existing done verb and quits once the lock is released', async () => {
    invokeIpcMock.mockResolvedValue({ kind: 'released' });
    render(<SpeleoQuitDialog />);

    fillCommitMessage();
    await userEvent.click(screen.getByTestId('speleodb-quit-push-release'));

    await waitFor(() => expect(performQuitMock).toHaveBeenCalledTimes(1));
    expect(calledCommands()).toContain('speleodb_done');
    expect(useUIStore.getState().activeDialog).toBeNull();
  });

  it('does NOT quit when the push fails, and surfaces the failure inline', async () => {
    invokeIpcMock.mockRejectedValue(new Error('blackholed'));
    render(<SpeleoQuitDialog />);

    fillCommitMessage();
    await userEvent.click(screen.getByTestId('speleodb-quit-push-release'));

    const notice = await screen.findByTestId('speleodb-push-failure-inline');
    const failure = useSpeleoStore.getState().banner;
    expect(failure).toMatchObject({ kind: 'push_failed', origin: 'done' });
    expect(notice.textContent).toContain((failure as { message: string }).message);
    expect(performQuitMock).not.toHaveBeenCalled();
    expect(screen.getByTestId('speleodb-quit-dialog')).toBeInTheDocument();
    expect(useUIStore.getState().activeDialog).toBe('speleodb-quit');
  });

  it('does NOT quit when the push lands unconfirmed or diverges', async () => {
    invokeIpcMock.mockResolvedValue({ kind: 'divergence_needed' });
    render(<SpeleoQuitDialog />);

    fillCommitMessage();
    await userEvent.click(screen.getByTestId('speleodb-quit-push-release'));

    await waitFor(() => expect(calledCommands()).toContain('speleodb_done'));
    expect(performQuitMock).not.toHaveBeenCalled();
  });
});

describe('SpeleoQuitDialog - retain local edits and release on quit', () => {
  // Leaving without an upload still releases the current project's lock.
  it('releases the current lock before quitting', async () => {
    render(<SpeleoQuitDialog />);

    await userEvent.click(screen.getByTestId('speleodb-quit-release'));

    await waitFor(() => expect(performQuitMock).toHaveBeenCalledTimes(1));
    expect(calledCommands()).toEqual(['speleodb_project_access_at', 'speleodb_release_current_lock']);
    expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_release_current_lock', expect.objectContaining({ ownOnly: true, allowUnpushed: true }));
  });

  it('saves locally and releases before quitting', async () => {
    useProjectStore.setState({ isDirty: true });
    render(<SpeleoQuitDialog />);

    await userEvent.click(screen.getByTestId('speleodb-quit-release'));

    await waitFor(() => expect(performQuitMock).toHaveBeenCalledTimes(1));
    expect(saveProject).toHaveBeenCalledTimes(1);
    expect(calledCommands()).toEqual(['speleodb_project_access_at', 'speleodb_release_current_lock']);
    expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_release_current_lock', expect.objectContaining({ ownOnly: true, allowUnpushed: true }));
  });
});

describe('SpeleoQuitDialog - keep-lock exit', () => {
  it('closes locally before quitting without a release request', async () => {
    render(<SpeleoQuitDialog />);
    await userEvent.click(screen.getByTestId('speleodb-quit-keep-lock'));
    await waitFor(() => expect(performQuitMock).toHaveBeenCalledTimes(1));
    expect(useProjectStore.getState().closeProject).toHaveBeenCalledWith({ releaseLock: false, request: expect.any(Number) });
    expect(invokeIpcMock).not.toHaveBeenCalled();
  });
  it('does not release or quit when acknowledgment is cancelled', async () => {
    confirmMock.mockResolvedValue(false);
    render(<SpeleoQuitDialog />);
    await userEvent.click(screen.getByTestId('speleodb-quit-release'));
    expect(calledCommands()).toEqual(['speleodb_project_access_at']);
    expect(performQuitMock).not.toHaveBeenCalled();
  });
  it('focuses the keep-lock action by default', () => {
    render(<SpeleoQuitDialog />);
    expect(screen.getByTestId('speleodb-quit-keep-lock')).toHaveAttribute('data-primary-action');
    expect(screen.getByTestId('speleodb-quit-push-release')).not.toHaveAttribute('data-primary-action');
  });
});

describe('SpeleoQuitDialog - the disk-dirty half composes', () => {
  it('offers the save-locally checkbox, defaulted on, when the project is disk-dirty', () => {
    useProjectStore.setState({ isDirty: true });
    render(<SpeleoQuitDialog />);
    expect(screen.getByTestId('speleodb-quit-save-locally')).toBeChecked();
  });

  it('notes the work is already on disk when it is not dirty', () => {
    render(<SpeleoQuitDialog />);
    expect(screen.getByTestId('speleodb-quit-saved-locally-note')).toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-quit-save-locally')).not.toBeInTheDocument();
  });

  it('saves before pushing when disk-dirty', async () => {
    useProjectStore.setState({ isDirty: true });
    invokeIpcMock.mockResolvedValue({ kind: 'released' });
    render(<SpeleoQuitDialog />);

    fillCommitMessage();
    await userEvent.click(screen.getByTestId('speleodb-quit-push-release'));

    await waitFor(() => expect(performQuitMock).toHaveBeenCalledTimes(1));
    // The local flush happens BEFORE the upload. (`done` also re-flushes after a
    // confirmed push to clear `isDirty`, hence the order check rather than a count.)
    expect(saveProject).toHaveBeenCalled();
    expect(saveProject.mock.invocationCallOrder[0]!).toBeLessThan(
      invokeIpcMock.mock.invocationCallOrder[0]!,
    );
  });

  it('aborts the whole quit when the requested local save fails', async () => {
    useProjectStore.setState({ isDirty: true });
    saveProject.mockResolvedValue({ success: false, error: 'disk full' } as never);
    render(<SpeleoQuitDialog />);

    fillCommitMessage();
    await userEvent.click(screen.getByTestId('speleodb-quit-push-release'));

    await waitFor(() => expect(saveProject).toHaveBeenCalled());
    expect(invokeIpcMock).not.toHaveBeenCalled();
    expect(performQuitMock).not.toHaveBeenCalled();
  });

  it('skips the save when the user unticks the checkbox', async () => {
    useProjectStore.setState({ isDirty: true });
    render(<SpeleoQuitDialog />);

    await userEvent.click(screen.getByTestId('speleodb-quit-save-locally'));
    await userEvent.click(screen.getByTestId('speleodb-quit-release'));

    await waitFor(() => expect(performQuitMock).toHaveBeenCalledTimes(1));
    expect(saveProject).not.toHaveBeenCalled();
  });
});

describe('SpeleoQuitDialog - cancel', () => {
  it('stays in the app and touches nothing', async () => {
    render(<SpeleoQuitDialog />);

    await userEvent.click(screen.getByTestId('speleodb-quit-cancel'));

    expect(performQuitMock).not.toHaveBeenCalled();
    expect(invokeIpcMock).not.toHaveBeenCalled();
    expect(useUIStore.getState().activeDialog).toBeNull();
  });
});

describe('SpeleoQuitDialog - reached without the write lock', () => {
  // The guard now fires on unpushed edits alone, so this dialog is also reached
  // read-only: after a re-verify divergence, or once a ReLogin error ran
  // `clearSession()`. Every lock verb would fail there, so it must collapse to the
  // one honest choice rather than offering buttons that cannot work.
  it('offers only save-locally and Quit anyway, with no lock verbs', () => {
    useSpeleoStore.setState({ link: { ...LINK, editState: 'read-only' } });
    render(<SpeleoQuitDialog />);

    expect(screen.getByTestId('speleodb-quit-anyway')).toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-quit-push-release')).not.toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-quit-release')).not.toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-quit-release')).not.toBeInTheDocument();
  });

  it('does the same for a force-edit session, which holds no lock either', () => {
    useSpeleoStore.setState({ link: { ...LINK, forceEditing: true } });
    render(<SpeleoQuitDialog />);

    expect(screen.getByTestId('speleodb-quit-anyway')).toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-quit-push-release')).not.toBeInTheDocument();
  });

  it('Quit anyway saves the .stalx first and releases nothing', async () => {
    useSpeleoStore.setState({ link: { ...LINK, editState: 'read-only' } });
    useProjectStore.setState({ isDirty: true });
    render(<SpeleoQuitDialog />);

    await userEvent.click(screen.getByTestId('speleodb-quit-anyway'));

    await waitFor(() => expect(performQuitMock).toHaveBeenCalledTimes(1));
    expect(saveProject).toHaveBeenCalled();
    expect(calledCommands().filter((c) => RELEASE_COMMANDS.includes(c))).toEqual([]);
  });
});

describe('SpeleoQuitDialog - not dismissable mid-verb', () => {
  it('ignores Escape while a push is in flight, so the quit cannot slip through', async () => {
    // Dismissing mid-push used to close the dialog while `pushReleaseThenQuit` was
    // still awaiting; it then resolved `released` and called `performQuit()`, which
    // arms the one-shot confirmation - so the resulting close request skipped
    // `guardQuit` entirely and the window went away with no prompt.
    let releaseDone: (v: unknown) => void = () => {};
    invokeIpcMock.mockImplementation((command: string) => {
      if (command === 'speleodb_done') {
        return new Promise((resolve) => {
          releaseDone = resolve;
        });
      }
      return Promise.resolve(null);
    });
    render(<SpeleoQuitDialog />);

    fillCommitMessage();
    await userEvent.click(screen.getByTestId('speleodb-quit-push-release'));
    await waitFor(() => expect(calledCommands()).toContain('speleodb_done'));

    await userEvent.keyboard('{Escape}');
    expect(useUIStore.getState().activeDialog).toBe('speleodb-quit');

    // Let the verb finish so the test leaves nothing pending.
    releaseDone({ kind: 'released' });
    await waitFor(() => expect(performQuitMock).toHaveBeenCalledTimes(1));
  });
});

/**
 * [#270] `uiStore` has ONE `activeDialog` slot, so raising this prompt evicts
 * whatever was on screen. That is fine for the exit and wrong for Cancel: the app
 * keeps running, and the evicted dialog can be a multi-step import wizard whose
 * placed sections the user never agreed to drop except as part of quitting.
 */
describe('SpeleoQuitDialog - Cancel puts back the dialog it displaced', () => {
  const displaced = () =>
    useUIStore.setState({
      dialogData: {
        cancelReturnsTo: 'dmp-selection',
        cancelReturnsData: { dmpFile: { sections: 3 } },
      },
    });

  it('reopens it with its data, rather than dropping to a bare view', async () => {
    displaced();
    render(<SpeleoQuitDialog />);

    await userEvent.click(screen.getByTestId('speleodb-quit-cancel'));

    expect(useUIStore.getState().activeDialog).toBe('dmp-selection');
    expect(useUIStore.getState().dialogData).toEqual({ dmpFile: { sections: 3 } });
  });

  it('reopens it on Escape too - the same handler', async () => {
    displaced();
    render(<SpeleoQuitDialog />);

    await userEvent.keyboard('{Escape}');

    expect(useUIStore.getState().activeDialog).toBe('dmp-selection');
  });

  it('closes plainly when nothing was displaced', async () => {
    render(<SpeleoQuitDialog />);

    await userEvent.click(screen.getByTestId('speleodb-quit-cancel'));

    expect(useUIStore.getState().activeDialog).toBeNull();
  });

  it('does NOT restore it on the way out - the window is going away', async () => {
    displaced();
    invokeIpcMock.mockResolvedValue({ kind: 'released' });
    render(<SpeleoQuitDialog />);

    fillCommitMessage();
    await userEvent.click(screen.getByTestId('speleodb-quit-push-release'));

    await waitFor(() => expect(performQuitMock).toHaveBeenCalledTimes(1));
    expect(useUIStore.getState().activeDialog).toBeNull();
  });
});

/**
 * [#303] The unwound branch, kept in lock-step with SpeleoCloseDialog.
 *
 * A copy that is dirty ONLY because a chain step walked it back holds nothing of the
 * user's. Calling that "changes that have not been uploaded" and pre-focusing "Push,
 * release & quit" pointed a stray Enter at the one destructive answer: pushing records
 * the older survey over the newer one for everyone, and `done` then short-circuits into
 * its own confirmation, which replaces this dialog in the single `activeDialog` slot -
 * so the quit was silently abandoned as well. The confirmation now carries the quit
 * (`pendingExit`); the hand-off is pinned in the last describe of this file.
 */
describe('SpeleoQuitDialog - unwound copy', () => {
  const unwind = () =>
    useSpeleoStore.setState({ link: { ...LINK, hasUnpushedEdits: true, unwound: true } });

  it('defaults to keeping the lock and marks pushing an older version destructive', () => {
    unwind();
    render(<SpeleoQuitDialog />);

    const keep = screen.getByTestId('speleodb-quit-keep-lock');
    const push = screen.getByTestId('speleodb-quit-push-release');
    expect(keep).toHaveAttribute('data-primary-action');
    expect(push).not.toHaveAttribute('data-primary-action');
    // Still reachable - deliberately publishing an older version is a real choice.
    expect(push).toBeInTheDocument();
  });

  it('does not call an unwound copy unpushed work', () => {
    unwind();
    render(<SpeleoQuitDialog />);

    const body = screen.getByTestId('speleodb-quit-dialog').textContent ?? '';
    expect(body).toContain('older version of the survey');
    expect(body).not.toContain('changes that have not been uploaded');
    // The "your changes are saved locally" reassurance describes work that does not
    // exist here, so it is withheld rather than reworded.
    expect(screen.queryByTestId('speleodb-quit-saved-locally-note')).not.toBeInTheDocument();
  });

  it('keeps the ordinary wording and default when the copy really is edited', () => {
    useSpeleoStore.setState({ link: { ...LINK, hasUnpushedEdits: true, unwound: false } });
    render(<SpeleoQuitDialog />);

    expect(screen.getByTestId('speleodb-quit-keep-lock')).toHaveAttribute('data-primary-action');
    expect(screen.getByTestId('speleodb-quit-release')).not.toHaveAttribute(
      'data-primary-action',
    );
    expect(screen.getByTestId('speleodb-quit-dialog').textContent ?? '').toContain(
      'have not been uploaded',
    );
  });
});

/**
 * [#303] The blocker the describe above only documented: pressing "Upload this older
 * version" from a QUIT stranded the user.
 *
 * `done` never pushes on an unwound copy - it raises `SpeleoUnwoundPushDialog` and answers
 * `null`. That answer is not "no": the push then happens from inside the confirmation, so
 * the lock was released and the app carried on running, in a project the user had asked to
 * leave. Cancel was worse - the quit prompt had already been evicted from uiStore's single
 * `activeDialog` slot, so the exit request vanished with nothing on screen to say so.
 *
 * These cases render whichever dialog is active, exactly as App does, so the hand-off is
 * exercised end to end rather than asserted on the argument that carries it.
 */
describe('SpeleoQuitDialog - the unwound-push confirmation carries the quit', () => {
  /** App's dialog switch, narrowed to the two dialogs this hand-off spans. */
  const DialogHost = () => {
    const active = useUIStore((s) => s.activeDialog);
    if (active === 'speleodb-quit') return <SpeleoQuitDialog />;
    if (active === 'speleodb-unwound-push') return <SpeleoUnwoundPushDialog />;
    return null;
  };

  /** Quit prompt raised over an unwound copy, then "Push older version & quit" pressed. */
  const reachConfirmation = async () => {
    useSpeleoStore.setState({ link: { ...LINK, hasUnpushedEdits: true, unwound: true } });
    render(<DialogHost />);
    fillCommitMessage();
    await userEvent.click(screen.getByTestId('speleodb-quit-push-release'));
    return screen.findByTestId('speleodb-unwound-push-dialog');
  };

  it('quits once the push actually releases the lock', async () => {
    invokeIpcMock.mockResolvedValue({ kind: 'released' });
    await reachConfirmation();
    // The confirmation is up INSTEAD of the quit prompt, and nothing was uploaded yet.
    expect(calledCommands()).not.toContain('speleodb_done');

    await userEvent.click(screen.getByTestId('speleodb-unwound-push-push-anyway'));

    await waitFor(() => expect(performQuitMock).toHaveBeenCalledTimes(1));
    expect(calledCommands()).toContain('speleodb_done');
    expect(useUIStore.getState().activeDialog).toBeNull();
  });

  it('puts the quit prompt back on Cancel, instead of dropping the request', async () => {
    await reachConfirmation();

    await userEvent.click(screen.getByTestId('speleodb-unwound-push-cancel'));

    expect(useUIStore.getState().activeDialog).toBe('speleodb-quit');
    expect(screen.getByTestId('speleodb-quit-dialog')).toBeInTheDocument();
    expect(performQuitMock).not.toHaveBeenCalled();
    expect(invokeIpcMock).not.toHaveBeenCalled();
  });

  it('does NOT quit when the older-version push fails - and hands the prompt back with it', async () => {
    invokeIpcMock.mockRejectedValue(new Error('blackholed'));
    await reachConfirmation();

    await userEvent.click(screen.getByTestId('speleodb-unwound-push-push-anyway'));

    // Back on the quit prompt, which is the only surface that renders the failure inline
    // and whose own button is the retry. Closing over it would lose the quit twice.
    await waitFor(() => expect(useUIStore.getState().activeDialog).toBe('speleodb-quit'));
    expect(await screen.findByTestId('speleodb-push-failure-inline')).toBeInTheDocument();
    expect(performQuitMock).not.toHaveBeenCalled();
  });

  it('hands the prompt back after "discard and take the latest", rather than quitting on it', async () => {
    // That button keeps the write lock so the user can carry on editing, so quitting on it
    // would contradict its own label - but the quit request still has to survive.
    useProjectStore.setState({
      refreshProjectState: vi.fn(async () => {}) as never,
      checkProjectIntegrity: vi.fn(async () => []) as never,
      // [#473] Stubbed like its two siblings above. Discard replaces the tree even when
      // it returns null (it fast-forwards to the tip BEFORE deciding whether to release
      // the lock), so the seam re-reads the discarded-history flag here and the exact-IPC
      // assertion below is about the discard flow's own calls, not the reconciliation.
      checkHistoryDiscarded: vi.fn(async () => {}) as never,
    });
    useSpeleoStore.setState({ refreshLinkStatus: vi.fn(async () => {}) as never });
    invokeIpcMock.mockResolvedValue(null);
    await reachConfirmation();

    await userEvent.click(screen.getByTestId('speleodb-unwound-push-discard'));

    await waitFor(() => expect(useUIStore.getState().activeDialog).toBe('speleodb-quit'));
    expect(calledCommands()).toEqual(['speleodb_discard_changes']);
    expect(performQuitMock).not.toHaveBeenCalled();
  });
});


describe('quit release safety', () => {
  it('requires a commit message before uploading', () => {
    render(<SpeleoQuitDialog />);
    expect(screen.getByTestId('speleodb-quit-push-release')).toBeDisabled();
    fillCommitMessage();
    expect(screen.getByTestId('speleodb-quit-push-release')).toBeEnabled();
  });

  it('a failed release without upload keeps the app open', async () => {
    invokeIpcMock.mockRejectedValue(new Error('offline'));
    render(<SpeleoQuitDialog />);
    await userEvent.click(screen.getByTestId('speleodb-quit-release'));
    await waitFor(() => expect(useUIStore.getState().showToast).toHaveBeenCalled());
    expect(performQuitMock).not.toHaveBeenCalled();
  });
});

it('never quits a replacement project after an earlier local close completes', async () => {
  let finish!: (value: { success: boolean }) => void;
  useProjectStore.setState({ closeProject: vi.fn(() => new Promise((resolve) => { finish = resolve; })) as never });
  render(<SpeleoQuitDialog />);
  fireEvent.click(screen.getByTestId('speleodb-quit-keep-lock'));
  await waitFor(() => expect(useProjectStore.getState().closeProject).toHaveBeenCalledOnce());
  useProjectStore.setState({ project: { file_path: '/replacement.stalx' } as never });
  finish({ success: true });
  await waitFor(() => expect(screen.getByTestId('speleodb-quit-keep-lock')).toBeEnabled());
  expect(performQuitMock).not.toHaveBeenCalled();
});
