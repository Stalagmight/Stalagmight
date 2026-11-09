// @vitest-environment jsdom
/**
 * Canonical-storage prompt: opening a `.stalx` that carries a `speleo_link` row
 * from a non-canonical path is refused backend-side (SPELEODB_MANAGED), and this
 * dialog offers the two ways forward - open the managed copy from SpeleoDB, or
 * strip the file's link and open it as a plain local project.
 *
 * The load-bearing assertions are the NEGATIVE ones: Cancel issues no IPC at
 * all, the strip never runs the remote flow, and a failed strip keeps the
 * prompt (and its error) on screen instead of leaving nothing open.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, act } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const confirmMock = vi.hoisted(() => vi.fn().mockResolvedValue(true));
vi.mock('@tauri-apps/plugin-dialog', () => ({ confirm: confirmMock }));
const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (_e: unknown, f?: string) => f ?? 'error',
}));

import SpeleoManagedFileDialog from '../SpeleoManagedFileDialog';
import { useProjectStore } from '../../../store/projectStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import type { SpeleoSession } from '../../../store/speleoStore';
import { useUIStore } from '../../../store/uiStore';

const MANAGED = {
  path: '/home/diver/expedition/copy.stalx',
  projectId: 'p1',
  projectName: 'Cenote Azul',
  endpoint: 'http://sdb.example/',
  canOpenRemote: true,
};

const SUMMARY = {
  id: 'p1',
  name: 'Cenote Azul',
  permission: 'READ_AND_WRITE',
  lock_holder: null,
  type_tag: 'stalx',
  latest_commit_sha: 'abc123',
};

// Only the fields this dialog reads (loggedIn, endpoint); the rest of the
// session shape is irrelevant here.
const SESSION = {
  loggedIn: true,
  userEmail: 'diver@example.com',
  endpoint: 'http://sdb.example/',
  } as SpeleoSession;

let loadProjectMock: ReturnType<typeof vi.fn>;
let closeDialogMock: ReturnType<typeof vi.fn>;

beforeEach(() => {
  confirmMock.mockReset().mockResolvedValue(true);
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue(undefined);
  loadProjectMock = vi.fn(async () => ({ success: true }));
  closeDialogMock = vi.fn();
  useProjectStore.setState({ loadProject: loadProjectMock, project: null });
  useSpeleoStore.setState({
    session: SESSION,
    refreshSession: vi.fn(async () => {}),
    setLinkOnOpen: vi.fn(),
    clearLink: vi.fn(),
    clearSession: vi.fn(),
  });
  useUIStore.setState({
    closeDialog: closeDialogMock,
    openDialog: vi.fn(),
    showToast: vi.fn(),
    dialogData: { managed: MANAGED },
  });
});

const ipcCalls = () => invokeIpcMock.mock.calls.map((c) => String(c[0]));

describe('SpeleoManagedFileDialog', () => {
  it('names the file and the project it belongs to', () => {
    render(<SpeleoManagedFileDialog />);
    expect(screen.getByTestId('speleodb-managed-file-dialog')).toBeInTheDocument();
    expect(screen.getByText(/copy\.stalx/)).toBeInTheDocument();
    expect(screen.getByText(/Cenote Azul/)).toBeInTheDocument();
  });

  it('Cancel issues no IPC at all', async () => {
    render(<SpeleoManagedFileDialog />);
    await userEvent.click(screen.getByTestId('speleodb-managed-file-cancel'));
    expect(closeDialogMock).toHaveBeenCalled();
    expect(ipcCalls()).toEqual([]);
  });

  it('signed out: the remote action is disabled with the reason, the strip still works', async () => {
    useSpeleoStore.setState({ session: { ...SESSION, loggedIn: false } });
    render(<SpeleoManagedFileDialog />);

    expect(screen.getByTestId('speleodb-managed-file-open-remote')).toBeDisabled();
    expect(screen.getByTestId('speleodb-managed-file-remote-blocked')).toBeInTheDocument();

    await userEvent.click(screen.getByTestId('speleodb-managed-file-remove-link'));
    await waitFor(() =>
      expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_remove_link_at', {
        path: MANAGED.path,
        expected: MANAGED,
      }),
    );
    await waitFor(() => expect(loadProjectMock).toHaveBeenCalledWith(MANAGED.path));
  });

  it('an unreadable project id disables the remote action even when signed in', () => {
    useUIStore.setState({ dialogData: { managed: { ...MANAGED, canOpenRemote: false } } });
    render(<SpeleoManagedFileDialog />);
    expect(screen.getByTestId('speleodb-managed-file-open-remote')).toBeDisabled();
    expect(screen.getByTestId('speleodb-managed-file-remote-blocked')).toBeInTheDocument();
  });

  it('reports a project the account cannot see inline, without opening anything', async () => {
    invokeIpcMock.mockImplementation((cmd: string) =>
      cmd === 'speleodb_list_projects' ? Promise.resolve([]) : Promise.resolve(undefined),
    );
    render(<SpeleoManagedFileDialog />);
    await userEvent.click(screen.getByTestId('speleodb-managed-file-open-remote'));

    await waitFor(() =>
      expect(screen.getByTestId('speleodb-managed-file-error')).toBeInTheDocument(),
    );
    expect(ipcCalls()).not.toContain('speleodb_open_project');
    expect(loadProjectMock).not.toHaveBeenCalled();
    expect(closeDialogMock).not.toHaveBeenCalled();
  });

  it('refuses to match the project id against a different deployment', async () => {
    // The file says which server it belongs to, and a project id only means
    // anything on THAT server. A cloned deployment or a restored database can
    // hold the same UUID, so matching on the id alone would open a different
    // cave system entirely - the list must not even be consulted.
    useSpeleoStore.setState({
      session: { ...SESSION, endpoint: 'http://other-sdb.example/' } as SpeleoSession,
    });
    invokeIpcMock.mockImplementation((cmd: string) =>
      // The SAME id exists over here, which is exactly the trap.
      cmd === 'speleodb_list_projects' ? Promise.resolve([SUMMARY]) : Promise.resolve(undefined),
    );
    render(<SpeleoManagedFileDialog />);
    await userEvent.click(screen.getByTestId('speleodb-managed-file-open-remote'));

    await waitFor(() =>
      expect(screen.getByTestId('speleodb-managed-file-error')).toBeInTheDocument(),
    );
    expect(screen.getByTestId('speleodb-managed-file-error').textContent).toContain(
      'http://sdb.example/',
    );
    expect(ipcCalls()).not.toContain('speleodb_list_projects');
    expect(ipcCalls()).not.toContain('speleodb_open_project');
    expect(loadProjectMock).not.toHaveBeenCalled();
  });

  it('ignores an in-flight action once a newer refusal has replaced the prompt', async () => {
    // The dialog is a singleton: a newer refusal swaps `dialogData.managed` in
    // place and React keeps this instance, with the running handler still
    // closed over the OLD file. Acting on that result would open file A's
    // project over B's prompt - superseding what the user is now looking at.
    let releaseList: (projects: unknown[]) => void = () => {};
    invokeIpcMock.mockImplementation((cmd: string) =>
      cmd === 'speleodb_list_projects'
        ? new Promise((resolve) => {
            releaseList = resolve as (projects: unknown[]) => void;
          })
        : Promise.resolve(undefined),
    );
    render(<SpeleoManagedFileDialog />);
    await userEvent.click(screen.getByTestId('speleodb-managed-file-open-remote'));

    // A newer refusal takes the slot while the list request is still pending.
    // (`openDialog` is a spy here, so the slot is set the way it would leave it.)
    act(() => {
      useUIStore.setState({
        dialogData: {
          managed: { ...MANAGED, path: '/home/diver/other/newer.stalx', projectId: 'p2' },
        },
      });
    });
    releaseList([SUMMARY]);

    await waitFor(() => expect(ipcCalls()).toContain('speleodb_list_projects'));
    expect(ipcCalls()).not.toContain('speleodb_open_project');
    expect(loadProjectMock).not.toHaveBeenCalled();
    expect(screen.queryByTestId('speleodb-managed-file-error')).not.toBeInTheDocument();
    // And the newer prompt is USABLE: it must not have inherited the previous
    // one's in-flight `busy`, which would leave every button - Cancel included -
    // disabled with nothing left to re-enable them.
    await waitFor(() =>
      expect(screen.getByTestId('speleodb-managed-file-cancel')).not.toBeDisabled(),
    );
    expect(screen.getByTestId('speleodb-managed-file-remove-link')).not.toBeDisabled();
  });

  it('a replacement at the SAME path but a different project is still a different prompt', async () => {
    // The identity is the whole payload, not the path: a file replaced in place
    // is refused at the same path while naming another project, which is
    // exactly what the strip command re-checks for backend-side.
    let releaseList: (projects: unknown[]) => void = () => {};
    invokeIpcMock.mockImplementation((cmd: string) =>
      cmd === 'speleodb_list_projects'
        ? new Promise((resolve) => {
            releaseList = resolve as (projects: unknown[]) => void;
          })
        : Promise.resolve(undefined),
    );
    render(<SpeleoManagedFileDialog />);
    await userEvent.click(screen.getByTestId('speleodb-managed-file-open-remote'));

    act(() => {
      useUIStore.setState({
        dialogData: { managed: { ...MANAGED, projectId: 'p2', projectName: 'Other Cenote' } },
      });
    });
    releaseList([SUMMARY]);

    await waitFor(() => expect(ipcCalls()).toContain('speleodb_list_projects'));
    expect(ipcCalls()).not.toContain('speleodb_open_project');
    expect(loadProjectMock).not.toHaveBeenCalled();
  });

  it('strips the link and reopens the same path', async () => {
    render(<SpeleoManagedFileDialog />);
    await userEvent.click(screen.getByTestId('speleodb-managed-file-remove-link'));

    await waitFor(() =>
      expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_remove_link_at', {
        path: MANAGED.path,
        expected: MANAGED,
      }),
    );
    await waitFor(() => expect(loadProjectMock).toHaveBeenCalledWith(MANAGED.path));
    expect(closeDialogMock).toHaveBeenCalled();
    // The strip never touches the remote flow.
    expect(ipcCalls()).not.toContain('speleodb_list_projects');
    expect(ipcCalls()).not.toContain('speleodb_open_project');
  });

  it('keeps the prompt open with the reason when the strip fails', async () => {
    invokeIpcMock.mockImplementation((cmd: string) =>
      cmd === 'speleodb_remove_link_at'
        ? Promise.reject({ message: 'file is locked', action: 'None', status: null })
        : Promise.resolve(undefined),
    );
    render(<SpeleoManagedFileDialog />);
    await userEvent.click(screen.getByTestId('speleodb-managed-file-remove-link'));

    await waitFor(() =>
      expect(screen.getByTestId('speleodb-managed-file-error')).toBeInTheDocument(),
    );
    expect(screen.getByTestId('speleodb-managed-file-dialog')).toBeInTheDocument();
    expect(loadProjectMock).not.toHaveBeenCalled();
    expect(closeDialogMock).not.toHaveBeenCalled();
  });

  it('cannot be dismissed while an action is in flight', async () => {
    let rejectStrip: (e: unknown) => void = () => {};
    invokeIpcMock.mockImplementation((cmd: string) =>
      cmd === 'speleodb_remove_link_at'
        ? new Promise((_resolve, reject) => {
            rejectStrip = reject;
          })
        : Promise.resolve(undefined),
    );
    const openDialogMock = vi.fn();
    useUIStore.setState({ openDialog: openDialogMock });
    render(<SpeleoManagedFileDialog />);
    await userEvent.click(screen.getByTestId('speleodb-managed-file-remove-link'));

    // Escape mid-flight is inert...
    await userEvent.keyboard('{Escape}');
    expect(closeDialogMock).not.toHaveBeenCalled();
    expect(openDialogMock).not.toHaveBeenCalled();
    expect(screen.getByTestId('speleodb-managed-file-dialog')).toBeInTheDocument();

    // ...so the failure still has somewhere to land.
    rejectStrip({ message: 'file is locked', action: 'None', status: null });
    await waitFor(() =>
      expect(screen.getByTestId('speleodb-managed-file-error')).toBeInTheDocument(),
    );
  });
});


it.each(['read-only', 'locked'])('restores the managed prompt after a cancelled %s warning', async (kind) => {
  confirmMock.mockResolvedValue(false);
  invokeIpcMock.mockResolvedValue([{ ...SUMMARY,
    permission: kind === 'read-only' ? 'READ_ONLY' : 'READ_AND_WRITE',
    lock_holder: kind === 'locked' ? 'other@example.com' : null,
  }]);
  render(<SpeleoManagedFileDialog />);
  const open = screen.getByTestId('speleodb-managed-file-open-remote');
  await userEvent.click(open);
  await waitFor(() => expect(open).toBeEnabled());
  expect(screen.getByTestId('speleodb-managed-file-cancel')).toBeEnabled();
  expect(loadProjectMock).not.toHaveBeenCalled();
  expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_open_project')).toBe(false);
  await userEvent.click(open);
  expect(confirmMock).toHaveBeenCalledTimes(2);
});
