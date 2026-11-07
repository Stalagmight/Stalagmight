/**
 * Component test (#218) - the restore-revision confirm. On confirm it runs
 * speleodb_restore_revision then reloads the project; it warns about unsaved
 * changes, and surfaces a failure without reloading. Restore never fires on
 * mount or cancel.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../utils/ipc');
stubGetErrorMessage(fallbackOr('error'));

import SpeleoRestoreDialog from './SpeleoRestoreDialog';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { invokeIpcMock, stubGetErrorMessage, fallbackOr } from '../../test/factories/ipc';

let closeDialogMock: ReturnType<typeof vi.fn>;
let showToastMock: ReturnType<typeof vi.fn>;
let refreshProjectStateMock: ReturnType<typeof vi.fn>;
let refreshLinkStatusMock: ReturnType<typeof vi.fn>;

beforeEach(() => {
  invokeIpcMock.mockReset();
  closeDialogMock = vi.fn();
  showToastMock = vi.fn();
  refreshProjectStateMock = vi.fn(async () => {});
  refreshLinkStatusMock = vi.fn(async () => {});
  useUIStore.setState({
    closeDialog: closeDialogMock,
    showToast: showToastMock,
    setView: vi.fn(),
    dialogData: { projectId: 'p1', projectName: 'Cave A', hexsha: 'abcdef0123', shortSha: 'abcdef01' },
  });
  useProjectStore.setState({
    refreshProjectState: refreshProjectStateMock,
    isDirty: false,
    checkProjectIntegrity: vi.fn(async () => []),
  });
  useSpeleoStore.setState({
    remoteProjectId: 'p1',
    refreshLinkStatus: refreshLinkStatusMock,
  });
});

describe('SpeleoRestoreDialog (#218)', () => {
  it('does not restore on mount or cancel', async () => {
    render(<SpeleoRestoreDialog />);
    expect(screen.getByTestId('speleodb-restore-dialog')).toBeInTheDocument();
    await userEvent.click(screen.getByTestId('speleodb-restore-cancel'));
    expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_restore_revision')).toBe(false);
    expect(closeDialogMock).toHaveBeenCalled();
  });

  it('confirm restores the revision then refreshes the swapped tree', async () => {
    invokeIpcMock.mockResolvedValue({ local_path: '/cache/p1.stalx', concurrent_session: false });
    render(<SpeleoRestoreDialog />);
    await userEvent.click(screen.getByTestId('speleodb-restore-confirm-btn'));

    // The backend reopened the restored tree; the FE pulls the swapped geometry +
    // link state (not loadProject, which would short-circuit on the open project).
    await waitFor(() => expect(refreshProjectStateMock).toHaveBeenCalled());
    expect(refreshLinkStatusMock).toHaveBeenCalled();
    const call = invokeIpcMock.mock.calls.find((c) => c[0] === 'speleodb_restore_revision');
    expect(call?.[1]).toEqual({
      projectId: 'p1',
      projectName: 'Cave A',
      hexsha: 'abcdef0123',
      restore: true,
      // The first attempt never waives the un-pushed-edits guard - only the armed
      // second press does, after the refusal has named what is being discarded.
      discardLocal: false,
      // [#258] and it names no dump entry, so the backend classifies normally.
      entryPath: null,
      // [#303] A plain Restore drops the downloaded copy's undo history like every
      // other adopt path; only the Unwind keeps it.
      keepUndoHistory: false,
      // ...so there is no un-pushed-work waiver to acknowledge either. Pinned false:
      // this flag is the only thing that lets the backend discard work the user never
      // pushed, and it belongs to the Unwind confirmation alone.
      discardUnpushed: false,
      // ...and with no history kept there is no chain step, so no direction to pick a
      // guard with either.
      chainForward: false,
    });
    expect(closeDialogMock).toHaveBeenCalled();
    expect(showToastMock).toHaveBeenCalled();
  });

  it('warns about unsaved changes when the project is dirty', () => {
    useProjectStore.setState({ isDirty: true });
    render(<SpeleoRestoreDialog />);
    expect(screen.getByTestId('speleodb-restore-unsaved-warning')).toBeInTheDocument();
  });

  it('surfaces an error and does not refresh when the restore fails', async () => {
    invokeIpcMock.mockRejectedValue({ message: 'offline' });
    render(<SpeleoRestoreDialog />);
    await userEvent.click(screen.getByTestId('speleodb-restore-confirm-btn'));

    await waitFor(() =>
      expect(screen.getByTestId('speleodb-restore-error')).toBeInTheDocument(),
    );
    expect(refreshProjectStateMock).not.toHaveBeenCalled();
  });
});

describe('SpeleoRestoreDialog - a second restore (#270)', () => {
  // A restore STAMPS the working copy as un-pushed, so the un-pushed-edits guard
  // refuses the NEXT one - which made Restore single-use: restoring a second
  // revision, or reverting a restore the user disliked, failed with a raw "local
  // edits never pushed" line about edits the previous restore had just created.
  const REFUSAL = {
    message: 'local edits never pushed',
    code: 'SPELEODB_UNPUSHED_LOCAL_EDITS',
    unpushed_local: {
      project_id: 'p1',
      local_path: '/cache/p1.stalx',
      last_edit_at: '2026-07-26T09:00:00Z',
    },
  };

  it('offers an explicit discard step instead of dead-ending on the refusal', async () => {
    invokeIpcMock.mockRejectedValue(REFUSAL);
    render(<SpeleoRestoreDialog />);

    await userEvent.click(screen.getByTestId('speleodb-restore-confirm-btn'));

    await waitFor(() =>
      expect(screen.getByTestId('speleodb-restore-discard-warning')).toBeInTheDocument(),
    );
    // The refusal is NOT shown as a plain error - it is an actionable choice.
    expect(screen.queryByTestId('speleodb-restore-error')).not.toBeInTheDocument();
    // OFFERED, not armed: a distinct control that still needs its own press. The
    // plain retry is gone (it would only be refused again).
    expect(screen.getByTestId('speleodb-restore-discard-offer-btn')).toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-restore-discard-confirm-btn')).not.toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-restore-confirm-btn')).not.toBeInTheDocument();
    expect(closeDialogMock).not.toHaveBeenCalled();
  });

  it('takes two further presses - the refusal alone can never discard', async () => {
    invokeIpcMock.mockRejectedValueOnce(REFUSAL);
    invokeIpcMock.mockResolvedValue({ local_path: '/cache/p1.stalx', concurrent_session: false });
    render(<SpeleoRestoreDialog />);

    await userEvent.click(screen.getByTestId('speleodb-restore-confirm-btn'));
    // Press 1 on the new control only arms it.
    await userEvent.click(await screen.findByTestId('speleodb-restore-discard-offer-btn'));
    expect(
      invokeIpcMock.mock.calls.filter((c) => c[0] === 'speleodb_restore_revision'),
    ).toHaveLength(1);
    // Press 2 performs it.
    await userEvent.click(await screen.findByTestId('speleodb-restore-discard-confirm-btn'));

    await waitFor(() => expect(refreshProjectStateMock).toHaveBeenCalled());
    const calls = invokeIpcMock.mock.calls.filter((c) => c[0] === 'speleodb_restore_revision');
    expect(calls).toHaveLength(2);
    expect(calls[0]?.[1]).toMatchObject({ discardLocal: false });
    expect(calls[1]?.[1]).toMatchObject({ restore: true, discardLocal: true });
    expect(closeDialogMock).toHaveBeenCalled();
  });

  // The regression: the refusal is a purely LOCAL read, so the failed first attempt
  // can resolve inside a double-click interval. It used to re-label the SAME
  // auto-focused primary button into "Replace my changes & restore" wired straight to
  // the discard - so the tail of a double click, or a second Enter on a still-focused
  // button, permanently destroyed un-pushed survey work the user never chose to lose.
  it('a second Enter on the focused control after the refusal does NOT discard', async () => {
    invokeIpcMock.mockRejectedValueOnce(REFUSAL);
    invokeIpcMock.mockResolvedValue({ local_path: '/cache/p1.stalx', concurrent_session: false });
    render(<SpeleoRestoreDialog />);

    await userEvent.click(screen.getByTestId('speleodb-restore-confirm-btn'));
    await waitFor(() =>
      expect(screen.getByTestId('speleodb-restore-discard-warning')).toBeInTheDocument(),
    );

    // Whatever the focus heuristic landed on, hitting Enter again must not discard.
    await userEvent.keyboard('{Enter}');
    // ...and neither must a second click at the primary button's old position, which
    // is now Cancel / the un-armed offer, never the discard itself.
    await userEvent.keyboard('{Enter}');

    const calls = invokeIpcMock.mock.calls.filter((c) => c[0] === 'speleodb_restore_revision');
    expect(calls.some((c) => (c[1] as { discardLocal?: boolean }).discardLocal === true)).toBe(
      false,
    );
  });
});

/**
 * [#576 W55 NF-W53-1] The backend refuses the same way when the cached copy is there but
 * could not be read (a hot journal after a crash, a lock another program holds), with
 * `state_unknown` set. The discard step must say that, not claim changes the copy may
 * never have had. The two-press discard and its labels stay as they are.
 */
describe('SpeleoRestoreDialog - a copy that could not be read (#576 W55 NF-W53-1)', () => {
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

  it('says the copy could not be read instead of claiming un-pushed changes', async () => {
    invokeIpcMock.mockRejectedValue(refusal(true));
    render(<SpeleoRestoreDialog />);

    await userEvent.click(screen.getByTestId('speleodb-restore-confirm-btn'));

    const warning = (await screen.findByTestId('speleodb-restore-discard-warning')).textContent;
    console.log('[W55] restore discard warning (state_unknown):', warning);
    expect(warning).toContain('could not be read');
    expect(warning).toContain('A save may have been interrupted, or another program may be using it.');
    expect(warning).not.toContain('The working copy has changes that are not on SpeleoDB');
    // Same offer, same label: only the warning's wording follows the flag.
    expect(screen.getByTestId('speleodb-restore-discard-offer-btn')).toHaveTextContent(
      'Replace my changes & restore',
    );
  });

  it('keeps the un-pushed wording for a copy that was read and found dirty', async () => {
    invokeIpcMock.mockRejectedValue(refusal(false));
    render(<SpeleoRestoreDialog />);

    await userEvent.click(screen.getByTestId('speleodb-restore-confirm-btn'));

    const warning = (await screen.findByTestId('speleodb-restore-discard-warning')).textContent;
    expect(warning).toContain('The working copy has changes that are not on SpeleoDB');
    expect(warning).not.toContain('could not be read');
  });
});
