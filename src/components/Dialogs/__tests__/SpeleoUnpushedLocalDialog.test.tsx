// @vitest-environment jsdom
/**
 * [#270] The refused-open prompt: `speleodb_open_project` refuses to overwrite a
 * cached .stalx that still holds edits SpeleoDB has never seen, and the user gets a
 * binary choice instead of a generic error.
 *
 * The load-bearing assertions here are the NEGATIVE ones: keeping the local copy
 * must issue NO SpeleoDB command at all, and the destructive branch must not run
 * until it has been confirmed twice.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../../utils/ipc');
stubGetErrorMessage(fallbackOr('error'));

import SpeleoUnpushedLocalDialog from '../SpeleoUnpushedLocalDialog';
import { useProjectStore } from '../../../store/projectStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import { useUIStore } from '../../../store/uiStore';
import { invokeIpcMock, stubGetErrorMessage, fallbackOr } from '../../../test/factories/ipc';

const SUMMARY = {
  id: 'p1',
  name: 'Cenote Azul',
  permission: 'READ_AND_WRITE',
  lock_holder: null,
  type_tag: 'stalx',
  latest_commit_sha: 'abc123',
};

const UNPUSHED = {
  project_id: 'p1',
  local_path: '/cache/p1/project.stalx',
  last_edit_at: '2026-07-20T10:30:00Z',
};

let loadProjectMock: ReturnType<typeof vi.fn>;
let closeDialogMock: ReturnType<typeof vi.fn>;

beforeEach(() => {
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue(undefined);
  loadProjectMock = vi.fn(async () => ({ success: true }));
  closeDialogMock = vi.fn();
  // `keepLocalCopy` compares the refused path against the LIVE open project.
  useProjectStore.setState({ loadProject: loadProjectMock, project: null });
  useSpeleoStore.setState({ setLinkOnOpen: vi.fn(), clearLink: vi.fn(), clearSession: vi.fn() });
  useUIStore.setState({
    closeDialog: closeDialogMock,
    openDialog: vi.fn(),
    showToast: vi.fn(),
    dialogData: { summary: SUMMARY, unpushedLocal: UNPUSHED },
  });
});

/** Every SpeleoDB command the app could have issued from this prompt. */
const speleoCalls = () =>
  invokeIpcMock.mock.calls.filter((c) => String(c[0]).startsWith('speleodb_'));

describe('SpeleoUnpushedLocalDialog (#270)', () => {
  it('names the project and when the local copy was last edited', () => {
    render(<SpeleoUnpushedLocalDialog />);
    expect(screen.getByTestId('speleodb-unpushed-local-dialog')).toBeInTheDocument();
    expect(screen.getByText(/Cenote Azul/)).toBeInTheDocument();
    expect(screen.getByTestId('speleodb-unpushed-local-last-edited').textContent).toContain(
      new Date(UNPUSHED.last_edit_at).toLocaleString(),
    );
  });

  it('keeps the local copy: opens local_path and issues NO SpeleoDB command', async () => {
    render(<SpeleoUnpushedLocalDialog />);
    await userEvent.click(screen.getByTestId('speleodb-unpushed-local-keep'));

    await waitFor(() => expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1/project.stalx'));
    expect(speleoCalls()).toEqual([]);
  });

  it('makes keeping the local copy the primary (default-focus) action', () => {
    render(<SpeleoUnpushedLocalDialog />);
    const keep = screen.getByTestId('speleodb-unpushed-local-keep');
    const takeRemote = screen.getByTestId('speleodb-unpushed-local-take-remote');
    expect(keep).toHaveAttribute('data-primary-action');
    expect(takeRemote).not.toHaveAttribute('data-primary-action');
  });

  it('arms the destructive action first: one click discards nothing', async () => {
    render(<SpeleoUnpushedLocalDialog />);
    await userEvent.click(screen.getByTestId('speleodb-unpushed-local-take-remote'));

    expect(speleoCalls()).toEqual([]);
    expect(loadProjectMock).not.toHaveBeenCalled();
    expect(
      screen.getByTestId('speleodb-unpushed-local-discard-warning').textContent,
    ).toContain('Cenote Azul');
  });

  it('takes the server copy on the second click, with the refused open arguments', async () => {
    invokeIpcMock.mockImplementation((cmd: string) =>
      cmd === 'speleodb_open_project_discarding_local'
        ? Promise.resolve({ local_path: '/cache/p1/project.stalx', concurrent_session: false })
        : Promise.resolve(undefined),
    );
    render(<SpeleoUnpushedLocalDialog />);
    await userEvent.click(screen.getByTestId('speleodb-unpushed-local-take-remote'));
    await userEvent.click(screen.getByTestId('speleodb-unpushed-local-take-remote'));

    await waitFor(() => {
      const call = invokeIpcMock.mock.calls.find(
        (c) => c[0] === 'speleodb_open_project_discarding_local',
      );
      expect(call?.[1]).toEqual({
        projectId: 'p1',
        projectName: 'Cenote Azul',
        latestCommitSha: 'abc123',
        // [#258] No chosen entry on a first attempt.
        entryPath: null,
      });
    });
    await waitFor(() => expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1/project.stalx'));
  });

  it('DO NOT REGRESS: releases the prompt when the picker takes over (#258)', async () => {
    // `busy` gates `onClose` AND every button here, so holding it across the picker
    // left this dialog with no way out at all once the user cancelled - on the one
    // route that reaches an ambiguous dump with un-pushed edits, where backing out
    // matters most.
    invokeIpcMock.mockImplementation((cmd: string) =>
      cmd === 'speleodb_open_project_discarding_local'
        ? Promise.reject({
            message: 'more than one',
            code: 'SPELEODB_AMBIGUOUS',
            status: null,
            dump_candidates: [
              { entry_path: 'a.stalx', size_bytes: 1, modified: null },
              { entry_path: 'b.stalx', size_bytes: 2, modified: null },
            ],
          })
        : Promise.resolve(undefined),
    );
    render(<SpeleoUnpushedLocalDialog />);
    await userEvent.click(screen.getByTestId('speleodb-unpushed-local-take-remote'));
    await userEvent.click(screen.getByTestId('speleodb-unpushed-local-take-remote'));

    await waitFor(() =>
      expect(useSpeleoStore.getState().dumpPickerOpen).toBe(true),
    );
    // Cancelling the picker must leave a usable dialog behind it.
    useSpeleoStore.getState().closeDumpPicker();
    await waitFor(() =>
      expect(screen.getByTestId('speleodb-unpushed-local-keep')).not.toBeDisabled(),
    );
  });

  it('keeps the prompt open and shows why when the discarding open fails', async () => {
    invokeIpcMock.mockImplementation((cmd: string) =>
      cmd === 'speleodb_open_project_discarding_local'
        ? Promise.reject({ message: 'server exploded', code: 'FAILED', status: 500 })
        : Promise.resolve(undefined),
    );
    render(<SpeleoUnpushedLocalDialog />);
    await userEvent.click(screen.getByTestId('speleodb-unpushed-local-take-remote'));
    await userEvent.click(screen.getByTestId('speleodb-unpushed-local-take-remote'));

    await waitFor(() =>
      expect(screen.getByTestId('speleodb-unpushed-local-error')).toBeInTheDocument(),
    );
    // Still on screen, nothing loaded, and the prompt was never closed.
    expect(screen.getByTestId('speleodb-unpushed-local-dialog')).toBeInTheDocument();
    expect(loadProjectMock).not.toHaveBeenCalled();
    expect(closeDialogMock).not.toHaveBeenCalled();
    // The button stays armed, so it is a retry rather than a fresh arm.
    expect(screen.getByTestId('speleodb-unpushed-local-take-remote')).not.toBeDisabled();
  });

  // #270: DialogWrapper routes BOTH Escape and a backdrop click into `onClose`, and
  // this dialog handed it `closeDialog` bare. Escaping while
  // `speleodb_open_project_discarding_local` was in flight unmounted the only surface
  // its error had - `setError` landed on an unmounted component, so a failed
  // discard-my-edits open produced no message anywhere at all.
  it('cannot be dismissed while the destructive branch is in flight', async () => {
    let rejectOpen: (e: unknown) => void = () => {};
    invokeIpcMock.mockImplementation((cmd: string) =>
      cmd === 'speleodb_open_project_discarding_local'
        ? new Promise((_resolve, reject) => {
            rejectOpen = reject;
          })
        : Promise.resolve(undefined),
    );
    const openDialogMock = vi.fn();
    useUIStore.setState({ openDialog: openDialogMock });
    render(<SpeleoUnpushedLocalDialog />);
    await userEvent.click(screen.getByTestId('speleodb-unpushed-local-take-remote'));
    await userEvent.click(screen.getByTestId('speleodb-unpushed-local-take-remote'));

    // Escape mid-flight is inert.
    await userEvent.keyboard('{Escape}');
    expect(closeDialogMock).not.toHaveBeenCalled();
    expect(openDialogMock).not.toHaveBeenCalled();
    expect(screen.getByTestId('speleodb-unpushed-local-dialog')).toBeInTheDocument();

    // ...so the failure still has somewhere to land.
    rejectOpen({ message: 'server exploded', code: 'FAILED', status: 500 });
    await waitFor(() =>
      expect(screen.getByTestId('speleodb-unpushed-local-error')).toBeInTheDocument(),
    );
  });

  it('drops the session when the discarding open reports a dead token', async () => {
    // Every other consumer of this outcome handles `reLogin`; reading only
    // `outcome.message` left the shell claiming a session the backend had dropped.
    const clearSession = vi.fn();
    useSpeleoStore.setState({ clearSession });
    invokeIpcMock.mockImplementation((cmd: string) =>
      cmd === 'speleodb_open_project_discarding_local'
        ? Promise.reject({ message: 'unauthorized', code: 'SPELEODB_RELOGIN', status: 401 })
        : Promise.resolve(undefined),
    );
    render(<SpeleoUnpushedLocalDialog />);
    await userEvent.click(screen.getByTestId('speleodb-unpushed-local-take-remote'));
    await userEvent.click(screen.getByTestId('speleodb-unpushed-local-take-remote'));

    await waitFor(() => expect(clearSession).toHaveBeenCalled());
    expect(screen.getByTestId('speleodb-unpushed-local-error')).toBeInTheDocument();
  });

  // #270: this prompt REPLACES the browser in the single `activeDialog` slot, so a
  // bare `closeDialog` on Cancel left nothing open and nothing opened - a dead end.
  it('Cancel returns to the dialog it displaced when told how', async () => {
    const openDialogMock = vi.fn();
    useUIStore.setState({
      openDialog: openDialogMock,
      dialogData: {
        summary: SUMMARY,
        unpushedLocal: UNPUSHED,
        cancelReturnsTo: 'open-from-speleodb',
      },
    });
    render(<SpeleoUnpushedLocalDialog />);
    await userEvent.click(screen.getByTestId('speleodb-unpushed-local-cancel'));

    expect(openDialogMock).toHaveBeenCalledWith('open-from-speleodb');
    expect(closeDialogMock).not.toHaveBeenCalled();
    expect(speleoCalls()).toEqual([]);
  });

  it('Cancel just closes for a host that is not a dialog (GlobalView rows)', async () => {
    render(<SpeleoUnpushedLocalDialog />);
    await userEvent.click(screen.getByTestId('speleodb-unpushed-local-cancel'));
    expect(closeDialogMock).toHaveBeenCalled();
  });

  it('[#258] releases busy when the ADOPT turns ambiguous, so the picker can be cancelled', async () => {
    // Adoption raises the picker on its own account - the Ariane tree can change while
    // the confirm sits open, and the backend answers with a fresh candidate set. This
    // branch had also just cleared `pendingAdopt`, so holding `busy` left a prompt with
    // no confirm on it AND every dismissal route disabled: cancelling the picker handed
    // back to a dialog that could not be closed.
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'speleodb_open_project_discarding_local')
        return Promise.reject({ message: 'ariane', code: 'SPELEODB_ADOPT', status: null });
      if (cmd === 'speleodb_adopt_ariane')
        return Promise.reject({
          message: 'more than one',
          code: 'SPELEODB_AMBIGUOUS',
          status: null,
          dump_candidates: [
            { entry_path: 'a/ariane.tml', size_bytes: 1, modified: null },
            { entry_path: 'b/ariane.tml', size_bytes: 2, modified: null },
          ],
          dump_candidate_kind: 'ariane',
        });
      return undefined;
    });

    render(<SpeleoUnpushedLocalDialog />);
    // Arm, then confirm the destructive take-remote, which routes to the adopt confirm.
    await userEvent.click(screen.getByTestId('speleodb-unpushed-local-take-remote'));
    await userEvent.click(screen.getByTestId('speleodb-unpushed-local-take-remote'));
    await waitFor(() => expect(screen.getByTestId('speleodb-adopt-confirm')).toBeInTheDocument());
    await userEvent.click(screen.getByTestId('speleodb-adopt-confirm-btn'));

    // The picker is up, and this prompt is usable again underneath it.
    await waitFor(() => expect(useSpeleoStore.getState().dumpPickerOpen).toBe(true));
    await waitFor(() =>
      expect(screen.getByTestId('speleodb-unpushed-local-cancel')).not.toBeDisabled(),
    );
  });
});

/**
 * [#576 W53] The backend refuses the same way when the cached copy is there but could
 * not be read (a hot journal after a crash, a lock another program holds): it cannot
 * rule out un-pushed edits. The prompt must say that, not tell the user about changes
 * they may never have made, and must still offer the same two branches.
 */
describe('SpeleoUnpushedLocalDialog: a local copy that could not be read (#576 W53)', () => {
  const UNREADABLE = {
    project_id: 'p1',
    local_path: '/cache/p1/project.stalx',
    last_edit_at: null,
    state_unknown: true,
  };

  beforeEach(() => {
    useUIStore.setState({ dialogData: { summary: SUMMARY, unpushedLocal: UNREADABLE } });
  });

  it('says the copy could not be read and how to recover, without claiming changes', () => {
    render(<SpeleoUnpushedLocalDialog />);
    const text = screen.getByTestId('speleodb-unpushed-local-dialog').textContent ?? '';
    expect(text).toContain('The copy on this computer could not be read');
    expect(text).toContain('Cenote Azul');
    expect(text).toContain('roll back to the last one that finished');
    expect(text).not.toContain('Changes on this computer are not on SpeleoDB');
    expect(text).not.toContain('that local copy has changes SpeleoDB has never seen');
    expect(text).not.toContain('with your changes intact');
  });

  it('still opens local_path on keep-local, with NO SpeleoDB command', async () => {
    render(<SpeleoUnpushedLocalDialog />);
    await userEvent.click(screen.getByTestId('speleodb-unpushed-local-keep'));

    await waitFor(() => expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1/project.stalx'));
    expect(speleoCalls()).toEqual([]);
  });

  it('warns that taking the server copy replaces a copy it could not read', async () => {
    render(<SpeleoUnpushedLocalDialog />);
    await userEvent.click(screen.getByTestId('speleodb-unpushed-local-take-remote'));

    expect(speleoCalls()).toEqual([]);
    const warning =
      screen.getByTestId('speleodb-unpushed-local-discard-warning').textContent ?? '';
    expect(warning).toContain('Cenote Azul');
    expect(warning).toContain('could not be read');
    expect(warning).not.toContain('permanently deletes the changes made');
  });

  it('keeps the un-pushed wording for a copy that was read and found dirty', () => {
    useUIStore.setState({
      dialogData: { summary: SUMMARY, unpushedLocal: { ...UNPUSHED, state_unknown: false } },
    });
    render(<SpeleoUnpushedLocalDialog />);
    const text = screen.getByTestId('speleodb-unpushed-local-dialog').textContent ?? '';
    expect(text).toContain('Changes on this computer are not on SpeleoDB');
    expect(text).toContain('that local copy has changes SpeleoDB has never seen');
    expect(text).not.toContain('could not be read');
  });
});
