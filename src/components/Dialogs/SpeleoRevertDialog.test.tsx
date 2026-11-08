/**
 * The revert confirm. Two things are load-bearing here: the armed two-press discard
 * (with focus parked on Cancel, so keyboard repetition can never reach it), and
 * staying OPEN on a push failure - the revision is in the working copy and the lock
 * is still held, so closing would hide the one state the user has to act on.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../utils/ipc');
stubGetErrorMessage(messageOr('error'));

import SpeleoRevertDialog from './SpeleoRevertDialog';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore, type SpeleoLinkState } from '../../store/speleoStore';
import { invokeIpcMock, stubGetErrorMessage, messageOr } from '../../test/factories/ipc';
import { makeSpeleoLink } from '../../test/factories/speleo';

/** [#576 NF-W8FE-2] Done and Push answer the wire envelope, never a bare outcome. */
function report<R>(result: R): { result: R; save_warnings: [] } {
  return { result, save_warnings: [] };
}

const LINK: SpeleoLinkState = makeSpeleoLink();

const RESTORED = { local_path: '/cache/p1.stalx', concurrent_session: false };

function ipc(routes: Record<string, () => unknown>): void {
  invokeIpcMock.mockImplementation(async (command: string) => routes[command]?.() ?? null);
}
const ok = (value: unknown) => () => value;
const rejects = (error: unknown) => () => {
  throw error;
};

const restoreCalls = () =>
  invokeIpcMock.mock.calls.filter((c) => c[0] === 'speleodb_restore_revision');

/** The speleodb_* commands actually issued, in order. */
const issued = () =>
  invokeIpcMock.mock.calls.map((c) => c[0] as string).filter((c) => c.startsWith('speleodb_'));

let closeDialogMock: ReturnType<typeof vi.fn>;
let openDialogMock: ReturnType<typeof vi.fn>;

beforeEach(() => {
  invokeIpcMock.mockReset();
  closeDialogMock = vi.fn();
  openDialogMock = vi.fn();
  useUIStore.setState({
    closeDialog: closeDialogMock,
    openDialog: openDialogMock,
    showToast: vi.fn(),
    setView: vi.fn(),
    clearSelection: vi.fn(),
    dialogData: {
      projectId: 'p1',
      projectName: 'Cave A',
      hexsha: 'abcdef0123',
      shortSha: 'abcdef01',
    },
  });
  useProjectStore.setState({
    refreshProjectState: vi.fn(async () => {}),
    checkProjectIntegrity: vi.fn(async () => []),
    saveProject: vi.fn(async () => ({ success: true }) as never),
    isDirty: false,
  });
  useSpeleoStore.setState({
    link: { ...LINK },
    remoteProjectId: 'p1',
    divergence: null,
    banner: null,
    busy: false,
    refreshLinkStatus: vi.fn(async () => {}),
  });
});

describe('SpeleoRevertDialog', () => {
  it('does not touch SpeleoDB on mount or cancel', async () => {
    ipc({});
    render(<SpeleoRevertDialog />);
    expect(screen.getByTestId('speleodb-revert-dialog')).toBeInTheDocument();
    await userEvent.click(screen.getByTestId('speleodb-revert-cancel'));
    expect(invokeIpcMock).not.toHaveBeenCalled();
    expect(closeDialogMock).toHaveBeenCalled();
  });

  it('prefills the commit message and sends the edited text to the push', async () => {
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: ok(RESTORED),
      speleodb_done: ok(report({ kind: 'released' })),
    });
    render(<SpeleoRevertDialog />);

    const box = screen.getByTestId('speleodb-revert-message') as HTMLTextAreaElement;
    expect(box.value).toContain('abcdef01');
    await userEvent.clear(box);
    await userEvent.type(box, 'roll back the bad merge');
    await userEvent.click(screen.getByTestId('speleodb-revert-confirm-btn'));

    await waitFor(() =>
      expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_done')).toBe(true),
    );
    const call = invokeIpcMock.mock.calls.find((c) => c[0] === 'speleodb_done');
    expect(call?.[1]).toEqual({
      message: 'roll back the bad merge',
      acceptOlderVersion: false,
      expectedProject: null,
    });
    expect(closeDialogMock).toHaveBeenCalled();
  });

  it('returns to the dialog it was opened from when one is named', async () => {
    useUIStore.setState({
      dialogData: {
        projectId: 'p1',
        projectName: 'Cave A',
        hexsha: 'abcdef0123',
        shortSha: 'abcdef01',
        returnTo: 'speleodb-project',
      },
    });
    ipc({});
    render(<SpeleoRevertDialog />);

    await userEvent.click(screen.getByTestId('speleodb-revert-cancel'));

    expect(openDialogMock).toHaveBeenCalledWith('speleodb-project', undefined);
    expect(closeDialogMock).not.toHaveBeenCalled();
  });

  it('reopens the dialog it came from on the tab it came from', async () => {
    // Reopening by id alone reset `dialogData` to `{}`, so the project dialog read
    // `tab` as undefined and defaulted to General - every exit from the revert flow
    // dropped the user on the metadata form instead of the revision list they
    // pressed Revert from.
    useUIStore.setState({
      dialogData: {
        projectId: 'p1',
        projectName: 'Cave A',
        hexsha: 'abcdef0123',
        shortSha: 'abcdef01',
        returnTo: 'speleodb-project',
        returnData: { tab: 'history' },
      },
    });
    ipc({});
    render(<SpeleoRevertDialog />);

    await userEvent.click(screen.getByTestId('speleodb-revert-cancel'));

    expect(openDialogMock).toHaveBeenCalledWith('speleodb-project', { tab: 'history' });
  });

  it('stays open and explains the held lock when the upload fails', async () => {
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: ok(RESTORED),
      speleodb_done: rejects({ message: 'offline' }),
    });
    render(<SpeleoRevertDialog />);

    await userEvent.click(screen.getByTestId('speleodb-revert-confirm-btn'));

    const error = await screen.findByTestId('speleodb-revert-error');
    // Names the state the user is now in: the revision is local, the lock is not back.
    expect(error.textContent).toMatch(/write lock/i);
    expect(closeDialogMock).not.toHaveBeenCalled();
    expect(screen.getByTestId('speleodb-revert-dialog')).toBeInTheDocument();
  });
});

/**
 * [#260] A failed upload used to leave this dialog open pointing at two controls its
 * OWN scrim covers - the shell banner's Retry and the toolbar's SpeleoDB button
 * (`AlertDialog` is `fixed inset-0 z-[60]`, so both are dimmed and unclickable). The
 * store half already suppresses the banner while this dialog owns the failure, which
 * means without this the failure has no reachable surface at all.
 */
describe('SpeleoRevertDialog - a failed upload is actionable from the dialog', () => {
  const pushFails = () =>
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: ok(RESTORED),
      speleodb_done: rejects({ message: 'the server refused the upload' }),
    });

  it('shows the server reason inline instead of only behind the scrim', async () => {
    pushFails();
    render(<SpeleoRevertDialog />);

    await userEvent.click(screen.getByTestId('speleodb-revert-confirm-btn'));

    const notice = await screen.findByTestId('speleodb-push-failure-inline');
    expect(notice.textContent).toContain('the server refused the upload');
  });

  it('swaps the primary control for a retry of the upload', async () => {
    pushFails();
    render(<SpeleoRevertDialog />);

    await userEvent.click(screen.getByTestId('speleodb-revert-confirm-btn'));

    expect(await screen.findByTestId('speleodb-revert-retry-upload-btn')).toBeInTheDocument();
    // Not the plain Revert: re-running it from the top hits the pre-flight
    // un-pushed-edits refusal, whose "replace my changes" is the user's own revert.
    expect(screen.queryByTestId('speleodb-revert-confirm-btn')).not.toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-revert-discard-offer-btn')).not.toBeInTheDocument();
  });

  it('re-issues the upload and finishes when the retry succeeds', async () => {
    pushFails();
    render(<SpeleoRevertDialog />);
    await userEvent.click(screen.getByTestId('speleodb-revert-confirm-btn'));
    const retry = await screen.findByTestId('speleodb-revert-retry-upload-btn');

    // The retry runs against a working copy that now HAS un-pushed edits (the revert
    // itself) under a lock that is still ours - the state the resume skips the
    // pre-flight guard for, since that guard tests the bit the restore just set.
    useSpeleoStore.setState({ link: { ...LINK, editState: 'editing', hasUnpushedEdits: true } });
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: ok(RESTORED),
      speleodb_done: ok(report({ kind: 'released' })),
    });
    await userEvent.click(retry);

    await waitFor(() => expect(closeDialogMock).toHaveBeenCalled());
    expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_done')).toBe(true);
  });

  it('retries the upload ALONE - it does not re-acquire or re-restore', async () => {
    // The whole point of the control. The acquire and the restore both SUCCEEDED and
    // their results are still standing, so re-running them would re-download the
    // revision and swap in a tree that is already correct, discarding anything edited
    // since. Worse, on the failure whose upload actually landed (a 200 whose body did
    // not parse), the acquire's reconcile has no authorship recovery: it reads the
    // user's own just-landed revert as a divergence, releases the lock, and prompts
    // them to resolve a conflict against themselves. The push path recognises that
    // commit as ours and finishes, so the retry has to stay on the push path.
    pushFails();
    render(<SpeleoRevertDialog />);
    await userEvent.click(screen.getByTestId('speleodb-revert-confirm-btn'));
    const retry = await screen.findByTestId('speleodb-revert-retry-upload-btn');

    useSpeleoStore.setState({ link: { ...LINK, editState: 'editing', hasUnpushedEdits: true } });
    invokeIpcMock.mockClear();
    // Every verb stays routed, so anything the retry issues is recorded.
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: ok(RESTORED),
      speleodb_done: ok(report({ kind: 'released' })),
    });
    await userEvent.click(retry);

    await waitFor(() => expect(closeDialogMock).toHaveBeenCalled());
    expect(issued()).toEqual(['speleodb_done']);
    expect(issued()).not.toContain('speleodb_start_editing');
    expect(issued()).not.toContain('speleodb_restore_revision');
  });

  it('offers the same upload-only retry when the push comes back unconfirmed', async () => {
    // `push_unconfirmed` is the classic lost-response case, and the backend keeps BOTH
    // the lock and the dirty bit on that arm expressly so a retry recovers by
    // ancestry - adopting the commit if it landed, re-uploading if it did not. So it
    // is the same footing as a thrown push, one step further along, and it gets the
    // same control. The composite would take the one route that cannot recognise our
    // own commit.
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: ok(RESTORED),
      speleodb_done: ok(report({ kind: 'push_unconfirmed' })),
    });
    render(<SpeleoRevertDialog />);
    await userEvent.click(screen.getByTestId('speleodb-revert-confirm-btn'));

    const retry = await screen.findByTestId('speleodb-revert-retry-upload-btn');
    // Not the plain Revert, whose three presses end in a lock-releasing divergence
    // prompt over the user's own revert.
    expect(screen.queryByTestId('speleodb-revert-confirm-btn')).not.toBeInTheDocument();

    useSpeleoStore.setState({ link: { ...LINK, editState: 'editing', hasUnpushedEdits: true } });
    invokeIpcMock.mockClear();
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: ok(RESTORED),
      speleodb_done: ok(report({ kind: 'released' })),
    });
    await userEvent.click(retry);

    await waitFor(() => expect(closeDialogMock).toHaveBeenCalled());
    expect(issued()).toEqual(['speleodb_done']);
  });
});

describe('SpeleoRevertDialog - the armed discard', () => {
  beforeEach(() => {
    // An un-pushed copy: the pre-flight refuses without touching SpeleoDB at all.
    useSpeleoStore.setState({ link: { ...LINK, hasUnpushedEdits: true } });
  });

  it('offers the discard as a distinct, un-armed control', async () => {
    ipc({});
    render(<SpeleoRevertDialog />);

    await userEvent.click(screen.getByTestId('speleodb-revert-confirm-btn'));

    expect(await screen.findByTestId('speleodb-revert-discard-warning')).toBeInTheDocument();
    expect(screen.getByTestId('speleodb-revert-discard-offer-btn')).toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-revert-discard-confirm-btn')).not.toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-revert-confirm-btn')).not.toBeInTheDocument();
    // Refused before any IPC - no lock was taken to hand back.
    expect(invokeIpcMock).not.toHaveBeenCalled();
  });

  it('parks focus on Cancel the moment the destructive route appears', async () => {
    // React reconciles the discard control into the primary button's slot, so it would
    // otherwise inherit that button's focus and two more Enters would run the discard.
    ipc({});
    render(<SpeleoRevertDialog />);

    await userEvent.click(screen.getByTestId('speleodb-revert-confirm-btn'));
    await screen.findByTestId('speleodb-revert-discard-offer-btn');

    expect(screen.getByTestId('speleodb-revert-cancel')).toHaveFocus();
  });

  it('takes two further presses, and only then waives the guard', async () => {
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: ok(RESTORED),
      speleodb_done: ok(report({ kind: 'released' })),
    });
    render(<SpeleoRevertDialog />);

    await userEvent.click(screen.getByTestId('speleodb-revert-confirm-btn'));
    // Press 1 on the new control only arms it.
    await userEvent.click(await screen.findByTestId('speleodb-revert-discard-offer-btn'));
    expect(restoreCalls()).toHaveLength(0);
    // Press 2 performs it.
    await userEvent.click(await screen.findByTestId('speleodb-revert-discard-confirm-btn'));

    await waitFor(() => expect(restoreCalls()).toHaveLength(1));
    expect(restoreCalls()[0]?.[1]).toMatchObject({ restore: true, discardLocal: true });
    expect(closeDialogMock).toHaveBeenCalled();
  });

  it('hands the CONFIRMED discard over to the upload-only retry when the push fails', async () => {
    // The discard route and the retry route are mutually exclusive in the render, and
    // the discard one is tested first - so leaving `needsDiscard` set for the life of
    // the dialog made the retry unreachable for exactly the users who needed it most.
    // What they got instead was the still-ARMED discard confirm: ONE press to re-run
    // the whole composite, which re-restores over anything edited since and, on the
    // attempt whose upload really landed, puts them through the acquire's reconcile -
    // no authorship recovery there, so their own revert reads as a divergence, the
    // lock is released, and they are asked to resolve a conflict with themselves.
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: ok(RESTORED),
      speleodb_done: rejects({ message: 'offline' }),
    });
    render(<SpeleoRevertDialog />);

    await userEvent.click(screen.getByTestId('speleodb-revert-confirm-btn'));
    await userEvent.click(await screen.findByTestId('speleodb-revert-discard-offer-btn'));
    await userEvent.click(await screen.findByTestId('speleodb-revert-discard-confirm-btn'));

    expect(await screen.findByTestId('speleodb-revert-retry-upload-btn')).toBeInTheDocument();
    // The destructive question was asked AND answered; that consent is spent.
    expect(screen.queryByTestId('speleodb-revert-discard-confirm-btn')).not.toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-revert-discard-offer-btn')).not.toBeInTheDocument();

    // And the retry really is the upload alone - no second acquire, no second restore
    // under the discard policy.
    invokeIpcMock.mockClear();
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: ok(RESTORED),
      speleodb_done: ok(report({ kind: 'released' })),
    });
    await userEvent.click(screen.getByTestId('speleodb-revert-retry-upload-btn'));

    await waitFor(() => expect(closeDialogMock).toHaveBeenCalled());
    expect(issued()).toEqual(['speleodb_done']);
  });

  it('does the same when the confirmed discard ends in an unconfirmed push', async () => {
    // The arm that MOST needs the upload-only retry: the commit may already be on the
    // server, and the composite is the one route that cannot tell it is ours.
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: ok(RESTORED),
      speleodb_done: ok(report({ kind: 'push_unconfirmed' })),
    });
    render(<SpeleoRevertDialog />);

    await userEvent.click(screen.getByTestId('speleodb-revert-confirm-btn'));
    await userEvent.click(await screen.findByTestId('speleodb-revert-discard-offer-btn'));
    await userEvent.click(await screen.findByTestId('speleodb-revert-discard-confirm-btn'));

    expect(await screen.findByTestId('speleodb-revert-retry-upload-btn')).toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-revert-discard-confirm-btn')).not.toBeInTheDocument();
  });

  it('disarms the discard when the confirmed run loses the session mid-flight', async () => {
    // `not_editing` has no lock to retry with, so there is no retry control - but the
    // restore still landed, and an armed destructive button is the wrong thing to
    // leave a signed-out user pointing at. The plain Revert comes back instead.
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: ok(RESTORED),
      speleodb_done: ok(report({ kind: 'not_editing' })),
    });
    render(<SpeleoRevertDialog />);

    await userEvent.click(screen.getByTestId('speleodb-revert-confirm-btn'));
    await userEvent.click(await screen.findByTestId('speleodb-revert-discard-offer-btn'));
    await userEvent.click(await screen.findByTestId('speleodb-revert-discard-confirm-btn'));

    await screen.findByTestId('speleodb-revert-error');
    expect(screen.queryByTestId('speleodb-revert-discard-confirm-btn')).not.toBeInTheDocument();
    expect(screen.getByTestId('speleodb-revert-confirm-btn')).toBeInTheDocument();
  });

  it('a second Enter on the focused control after the refusal does NOT discard', async () => {
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: ok(RESTORED),
      speleodb_done: ok(report({ kind: 'released' })),
    });
    render(<SpeleoRevertDialog />);

    await userEvent.click(screen.getByTestId('speleodb-revert-confirm-btn'));
    await screen.findByTestId('speleodb-revert-discard-warning');

    await userEvent.keyboard('{Enter}');
    await userEvent.keyboard('{Enter}');

    expect(
      restoreCalls().some((c) => (c[1] as { discardLocal?: boolean }).discardLocal === true),
    ).toBe(false);
  });
});

/**
 * Un-SAVED is not un-PUSHED. A revert closes and reopens the project at the checked-out
 * tree, so an edit that only exists in memory is gone and `useRestoreRevision` then
 * clears `isDirty` behind it - nothing is left that says it was ever there.
 */
describe('SpeleoRevertDialog - unsaved local edits', () => {
  beforeEach(() => {
    useProjectStore.setState({ isDirty: true });
  });

  it('refuses the revert instead of replacing work that is only in memory', async () => {
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: ok(RESTORED),
      speleodb_done: ok(report({ kind: 'released' })),
    });
    render(<SpeleoRevertDialog />);

    await userEvent.click(screen.getByTestId('speleodb-revert-confirm-btn'));

    const error = await screen.findByTestId('speleodb-revert-error');
    expect(error.textContent).toMatch(/saved to disk/i);
    // Refused before any IPC: no lock taken, no tree replaced, dialog still open.
    expect(invokeIpcMock).not.toHaveBeenCalled();
    expect(closeDialogMock).not.toHaveBeenCalled();
  });

  it('does NOT offer the discard route for it - that waiver saves nothing', async () => {
    // "Replace my changes & revert" waives the SpeleoDB guard and writes nothing to
    // disk, so routing an unsaved edit through it would destroy work the user could
    // still keep with one keystroke, under copy that only mentions SpeleoDB.
    useSpeleoStore.setState({ link: { ...LINK, hasUnpushedEdits: true } });
    ipc({});
    render(<SpeleoRevertDialog />);

    await userEvent.click(screen.getByTestId('speleodb-revert-confirm-btn'));

    await screen.findByTestId('speleodb-revert-error');
    expect(screen.queryByTestId('speleodb-revert-discard-offer-btn')).not.toBeInTheDocument();
    // The plain Revert stays the primary: nothing was taken, so saving and pressing it
    // again is the whole way forward.
    expect(screen.getByTestId('speleodb-revert-confirm-btn')).toBeInTheDocument();
  });
});

/**
 * [#576 W55 NF-W53-1] The restore inside the revert is refused the same way when the
 * cached copy is there but could not be read (`state_unknown`). The link reads clean, so
 * the pre-flight passes, the lock is taken, and the refusal arrives mid-flight. The
 * discard step and its lock note must say the copy could not be read, not claim
 * un-pushed changes; the two-press discard and its labels stay as they are.
 */
describe('SpeleoRevertDialog - a copy that could not be read (#576 W55 NF-W53-1)', () => {
  const refusal = (stateUnknown: boolean) => ({
    message: 'could not be read',
    code: 'SPELEODB_UNPUSHED_LOCAL_EDITS',
    unpushed_local: {
      project_id: 'p1',
      local_path: '/cache/p1.stalx',
      last_edit_at: null,
      state_unknown: stateUnknown,
    },
  });

  const refusedMidFlight = async (error: unknown) => {
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: rejects(error),
      speleodb_done: ok(report({ kind: 'released' })),
    });
    render(<SpeleoRevertDialog />);
    await userEvent.click(screen.getByTestId('speleodb-revert-confirm-btn'));
    return {
      warning: (await screen.findByTestId('speleodb-revert-discard-warning')).textContent,
      lockNote: screen.getByTestId('speleodb-revert-lock-held').textContent,
    };
  };

  it('says the copy could not be read instead of claiming un-pushed changes', async () => {
    const { warning, lockNote } = await refusedMidFlight(refusal(true));
    console.log('[W55] revert discard warning (state_unknown):', warning);
    console.log('[W55] revert lock note (state_unknown):', lockNote);
    expect(warning).toContain('could not be read');
    expect(warning).toContain('A save may have been interrupted, or another program may be using it.');
    expect(warning).not.toContain('This copy has changes that are not on SpeleoDB');
    expect(lockNote).not.toContain('your unpushed changes');
    // The lock is still kept over a copy nobody could read, and nothing was uploaded.
    expect(issued()).not.toContain('speleodb_done');
    // Same discard route on offer; its labels are pinned in the W59 block below.
    expect(screen.getByTestId('speleodb-revert-discard-offer-btn')).toBeInTheDocument();
  });

  it('keeps the un-pushed wording for a copy that was read and found dirty', async () => {
    const { warning, lockNote } = await refusedMidFlight(refusal(false));
    expect(warning).toContain('This copy has changes that are not on SpeleoDB');
    expect(warning).not.toContain('could not be read');
    expect(lockNote).toContain('your unpushed changes');
  });

  /**
   * [#576 W59 Q-W53-2-D1] When the copy could not be read nobody knows whether it holds
   * changes, so neither press of the two-press discard may call them "my changes": both
   * name the local copy instead. A copy that was read and found dirty keeps the labels.
   */
  describe('discard labels (#576 W59)', () => {
    const discardLabels = async (stateUnknown: boolean) => {
      await refusedMidFlight(refusal(stateUnknown));
      const offer = screen.getByTestId('speleodb-revert-discard-offer-btn').textContent;
      // The arming press: it only re-labels the control, it sends nothing.
      const before = issued().length;
      await userEvent.click(screen.getByTestId('speleodb-revert-discard-offer-btn'));
      expect(issued().length).toBe(before);
      const confirm = screen.getByTestId('speleodb-revert-discard-confirm-btn').textContent;
      return { offer, confirm };
    };

    it('names the local copy, not "my changes", on both presses', async () => {
      const { offer, confirm } = await discardLabels(true);
      console.log('[W59] revert discard labels (state_unknown):', offer, '|', confirm);
      expect(offer).toBe('Replace the local copy & revert');
      expect(confirm).toBe('Yes, replace the local copy & revert');
    });

    it('keeps the "my changes" labels for a copy that was read and found dirty', async () => {
      const { offer, confirm } = await discardLabels(false);
      console.log('[W59] revert discard labels (dirty):', offer, '|', confirm);
      expect(offer).toBe('Replace my changes & revert');
      expect(confirm).toBe('Yes, replace my changes & revert');
    });
  });
});
