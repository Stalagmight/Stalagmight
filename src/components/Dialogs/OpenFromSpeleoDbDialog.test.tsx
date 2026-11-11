import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const confirmMock = vi.hoisted(() => vi.fn().mockResolvedValue(true));
// The unsaved-edits question before a switch is in-app (`askExitChoice`); answer it
// through `confirmMock`: true = discard, false = cancel, 'save' = save on this
// computer first. The rest of the module (the drafts question) stays real.
vi.mock('../../utils/exitConfirm', async (importOriginal) => ({
  ...await importOriginal<typeof import('../../utils/exitConfirm')>(),
  askExitChoice: (options: unknown) => Promise.resolve(confirmMock(options))
    .then((answer: unknown) => (answer === 'save' ? 'save' : answer ? 'confirm' : 'cancel')),
}));
const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (e: unknown, f?: string) => (e as { message?: string } | null)?.message ?? f ?? 'error',
}));

import OpenFromSpeleoDbDialog from './OpenFromSpeleoDbDialog';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';

let loadProjectMock: ReturnType<typeof vi.fn>;
let closeDialogMock: ReturnType<typeof vi.fn>;
let showToastMock: ReturnType<typeof vi.fn>;

const REMOTE_PROJECT = {
  id: 'p1',
  name: 'Native Cave',
  permission: 'READ_AND_WRITE',
  type_tag: 'stalagmight',
  latest_commit_sha: 'abc123',
  lock_holder: null,
};

/** Default routing: authenticated, one native project, successful open. */
function mockRoutes(overrides: Record<string, () => Promise<unknown>> = {}) {
  invokeIpcMock.mockImplementation((cmd: string) => {
    const routes: Record<string, () => Promise<unknown>> = {
      speleodb_session: () => Promise.resolve({ logged_in: true }),
      speleodb_list_projects: () => Promise.resolve([REMOTE_PROJECT]),
      speleodb_open_project: () =>
        Promise.resolve({ local_path: '/cache/p1.stalx' }),
      ...overrides,
    };
    return (routes[cmd] ?? (() => Promise.resolve(undefined)))();
  });
}

beforeEach(() => {
  confirmMock.mockReset().mockResolvedValue(true);
  invokeIpcMock.mockReset();
  loadProjectMock = vi.fn(async () => ({ success: true }));
  closeDialogMock = vi.fn();
  showToastMock = vi.fn();
  useProjectStore.setState({ loadProject: loadProjectMock });
  useSpeleoStore.setState({
    session: null,
    setSession: vi.fn(),
    clearSession: vi.fn(),
    setLinkOnOpen: vi.fn(),
    clearLink: vi.fn(),
  });
  useUIStore.setState({
    closeDialog: closeDialogMock,
    openDialog: vi.fn(),
    showToast: showToastMock,
  });
});

/**
 * #270 (P1) - a TRANSIENT project-list failure must not log an authenticated user
 * out of the browser. The mount effect used to wrap the session probe and the list
 * fetch in one try, so any list hiccup fell into a catch that unconditionally showed
 * the login prompt. Auth failures still route to logged-out; list failures keep the
 * authenticated list phase with the inline error + Refresh.
 */
describe('OpenFromSpeleoDbDialog list-failure resilience (#270)', () => {
  const clearSession = () => useSpeleoStore.getState().clearSession as ReturnType<typeof vi.fn>;

  it('a flaky list fetch keeps the authenticated list phase (no login prompt)', async () => {
    mockRoutes({
      speleodb_list_projects: () => Promise.reject({ message: 'boom', action: 'Retry', status: 503 }),
    });
    render(<OpenFromSpeleoDbDialog />);

    await waitFor(() => expect(screen.getByTestId('open-from-speleodb-error')).toBeInTheDocument());
    expect(screen.queryByTestId('open-from-speleodb-logged-out')).not.toBeInTheDocument();
    // Still the authenticated surface: the filter + Refresh are reachable.
    expect(screen.getByTestId('open-from-speleodb-filter')).toBeInTheDocument();
    expect(clearSession()).not.toHaveBeenCalled();
  });

  it('the empty state says the list could not load, not that there are no projects', async () => {
    mockRoutes({
      speleodb_list_projects: () => Promise.reject({ message: 'boom', action: 'Retry', status: 503 }),
    });
    render(<OpenFromSpeleoDbDialog />);

    const empty = await screen.findByTestId('open-from-speleodb-empty');
    expect(empty.textContent).toContain('could not be loaded');
  });

  it('Refresh recovers into the list once the fetch succeeds', async () => {
    let fail = true;
    mockRoutes({
      speleodb_list_projects: () =>
        fail
          ? Promise.reject({ message: 'boom', action: 'Retry', status: 503 })
          : Promise.resolve([REMOTE_PROJECT]),
    });
    render(<OpenFromSpeleoDbDialog />);

    await screen.findByTestId('open-from-speleodb-empty');
    fail = false;
    await userEvent.click(screen.getByText('Refresh'));

    expect(await screen.findByTestId('open-from-speleodb-open-p1')).toBeInTheDocument();
    expect(screen.queryByTestId('open-from-speleodb-error')).not.toBeInTheDocument();
  });

  it('a dead token surfacing from the list call DOES drop the session', async () => {
    mockRoutes({
      speleodb_list_projects: () =>
        Promise.reject({ message: 'token expired', action: 'ReLogin', status: 401 }),
    });
    render(<OpenFromSpeleoDbDialog />);

    await waitFor(() =>
      expect(screen.getByTestId('open-from-speleodb-logged-out')).toBeInTheDocument(),
    );
    expect(clearSession()).toHaveBeenCalled();
  });

  it('a failing session probe still routes to the login prompt', async () => {
    mockRoutes({
      speleodb_session: () => Promise.reject({ message: 'no token', action: 'ReLogin', status: 401 }),
    });
    render(<OpenFromSpeleoDbDialog />);

    await waitFor(() =>
      expect(screen.getByTestId('open-from-speleodb-logged-out')).toBeInTheDocument(),
    );
    expect(clearSession()).toHaveBeenCalled();
  });

  it('a logged-out session still routes to the login prompt', async () => {
    mockRoutes({ speleodb_session: () => Promise.resolve({ logged_in: false }) });
    render(<OpenFromSpeleoDbDialog />);

    await waitFor(() =>
      expect(screen.getByTestId('open-from-speleodb-logged-out')).toBeInTheDocument(),
    );
    expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_list_projects')).toBe(false);
  });
});

/**
 * #270 - the browser is one of the two entry points into `useOpenRemoteProject`
 * (GlobalView's remote rows are the other). When the backend refuses the open
 * because the cached .stalx still holds un-pushed edits, the browser must hand off
 * to the keep-local / take-remote prompt instead of painting its generic error
 * banner, and must not stay stuck behind its busy flag.
 */
describe('OpenFromSpeleoDbDialog refused open with unpushed local edits (#270)', () => {
  const REFUSAL = {
    message: 'local copy has unpushed edits',
    action: 'UnpushedLocalEdits',
    status: null,
    unpushed_local: {
      project_id: 'p1',
      local_path: '/cache/p1/project.stalx',
      last_edit_at: '2026-07-20T10:30:00Z',
    },
  };

  it('raises the prompt instead of a generic error, and tells it the way back here', async () => {
    const openDialogMock = vi.fn();
    useUIStore.setState({ openDialog: openDialogMock });
    mockRoutes({ speleodb_open_project: () => Promise.reject(REFUSAL) });
    render(<OpenFromSpeleoDbDialog />);

    await userEvent.click(await screen.findByTestId('open-from-speleodb-open-p1'));

    await waitFor(() =>
      expect(openDialogMock).toHaveBeenCalledWith('speleodb-unpushed-local', {
        summary: expect.objectContaining({ id: 'p1' }),
        unpushedLocal: REFUSAL.unpushed_local,
        // #270: the prompt replaces this browser in the single dialog slot, so it
        // carries the route back. Without it, cancelling the prompt left nothing
        // open and nothing opened - a dead end.
        cancelReturnsTo: 'open-from-speleodb',
      }),
    );
    expect(screen.queryByTestId('open-from-speleodb-error')).not.toBeInTheDocument();
    // Nothing was downloaded or opened.
    expect(loadProjectMock).not.toHaveBeenCalled();
  });
});

/**
 * #270 - this browser is also a way OUT of the open project: the shared flow ends in
 * `loadProject`, which replaces it backend-side. It ran no guard at all - not the
 * `needsSpeleoHandBackPrompt` every other exit shares, not even a disk-dirty check -
 * so opening a different project here discarded unsaved edits with no prompt and
 * walked away from work SpeleoDB has never seen, lock still held. The hook now
 * refuses; this is the half the user actually sees.
 */
describe('OpenFromSpeleoDbDialog with work at risk in the open project (#270)', () => {
  afterEach(() => {
    useProjectStore.setState({ project: null, isDirty: false });
    useSpeleoStore.setState({ link: null, remoteProjectId: null });
  });

  it('opens another project while preserving saved unpushed work', async () => {
    // A DIFFERENT remote project is open, saved to disk but never uploaded.
    useProjectStore.setState({ project: { file_path: '/cache/p9.stalx' } as never, isDirty: false });
    useSpeleoStore.setState({
      link: {
        permission: 'READ_AND_WRITE',
        editState: 'read-only',
        lockHolder: null,
        lockedByOther: false,
        hasUnpushedEdits: true,
        unwound: false,

        undoMode: 'full',
        remoteState: 'unknown',
        noAccess: false,
        serverError: null,
      },
      remoteProjectId: 'p9',
    });
    // The pull would have SUCCEEDED, so an unguarded open runs all the way through.
    mockRoutes({
      speleodb_open_project: () =>
        Promise.resolve({ local_path: '/cache/p1/project.stalx' }),
    });
    render(<OpenFromSpeleoDbDialog />);

    await userEvent.click(await screen.findByTestId('open-from-speleodb-open-p1'));

    await waitFor(() => expect(loadProjectMock).toHaveBeenCalled());
    expect(confirmMock).not.toHaveBeenCalled();

  });
});

/**
 * frontend#3 - Escape, the X or a backdrop click used to close the browser while an
 * open was in flight; the unmount then made the open's outcome stale, so its error,
 * including the 401 that should drop the session, was never shown.
 */
describe('OpenFromSpeleoDbDialog while an open is in flight (frontend#3)', () => {
  const realClearSession = useSpeleoStore.getState().clearSession;
  const deferredOpen = () => {
    let fail!: (error: unknown) => void;
    const pending = new Promise<never>((_resolve, reject) => { fail = reject; });
    return { pending, fail };
  };

  it('cannot be dismissed by Escape, the X, the backdrop or Close until the open ends', async () => {
    const open = deferredOpen();
    mockRoutes({ speleodb_open_project: () => open.pending });
    render(<OpenFromSpeleoDbDialog />);
    await userEvent.click(await screen.findByTestId('open-from-speleodb-open-p1'));

    await userEvent.keyboard('{Escape}');
    await userEvent.click(screen.getByTestId('open-from-speleodb-dialog-close'));
    const backdrop = screen.getByTestId('open-from-speleodb-dialog-backdrop');
    await userEvent.click(backdrop);
    expect(screen.getByTestId('open-from-speleodb-close')).toBeDisabled();
    expect(closeDialogMock).not.toHaveBeenCalled();

    open.fail({ message: 'Project not found', action: 'Fatal', status: 404 });
    expect(await screen.findByTestId('open-from-speleodb-error')).toHaveTextContent('Project not found');
    await waitFor(() => expect(screen.getByTestId('open-from-speleodb-close')).toBeEnabled());
    await userEvent.keyboard('{Escape}');
    expect(closeDialogMock).toHaveBeenCalledOnce();
  });

  it('a 401 from the open signs out and keeps its message on the login prompt', async () => {
    useSpeleoStore.setState({ clearSession: realClearSession, session: { loggedIn: true, userEmail: 'u@example.com', endpoint: 'https://x/' } });
    let signedIn = true;
    mockRoutes({
      speleodb_session: () => Promise.resolve({ logged_in: signedIn }),
      speleodb_open_project: () => {
        signedIn = false;
        return Promise.reject({ message: 'Your SpeleoDB session expired', action: 'ReLogin', status: 401 });
      },
    });
    const generation = useSpeleoStore.getState().sessionGeneration;
    render(<OpenFromSpeleoDbDialog />);
    await userEvent.click(await screen.findByTestId('open-from-speleodb-open-p1'));

    expect(await screen.findByTestId('open-from-speleodb-logged-out')).toBeInTheDocument();
    // The session change re-ran the browser's session check; the reason survived it.
    await waitFor(() => expect(invokeIpcMock.mock.calls.filter((c) => c[0] === 'speleodb_session')).toHaveLength(2));
    expect(screen.getByTestId('open-from-speleodb-error')).toHaveTextContent('Your SpeleoDB session expired');
    expect(useSpeleoStore.getState().session).toBeNull();
    expect(useSpeleoStore.getState().sessionGeneration).toBe(generation + 1);
  });

  it('an open error that outlives the browser is toasted, and a 401 still drops the session', async () => {
    const open = deferredOpen();
    mockRoutes({ speleodb_open_project: () => open.pending });
    const { unmount } = render(<OpenFromSpeleoDbDialog />);
    await userEvent.click(await screen.findByTestId('open-from-speleodb-open-p1'));
    // Something else took the dialog slot mid-open.
    unmount();

    open.fail({ message: 'Your SpeleoDB session expired', action: 'ReLogin', status: 401 });
    await waitFor(() => expect(showToastMock).toHaveBeenCalledWith(
      expect.stringContaining('Your SpeleoDB session expired'), 'speleodb-open-remote-error', 'error',
    ));
    expect(useSpeleoStore.getState().clearSession).toHaveBeenCalledOnce();
  });
});

it.each(['read-only', 'locked'])('restores the browser after cancelling an unsaved switch to a %s project', async (kind) => {
  useProjectStore.setState({ project: { file_path: '/old.stalx' } as never, isDirty: true });
  confirmMock.mockResolvedValue(false);
  mockRoutes({ speleodb_list_projects: async () => [{ ...REMOTE_PROJECT,
    permission: kind === 'read-only' ? 'READ_ONLY' : 'READ_AND_WRITE',
    lock_holder: kind === 'locked' ? 'other@example.com' : null,
  }] });
  render(<OpenFromSpeleoDbDialog />);
  const open = await screen.findByTestId('open-from-speleodb-open-p1');
  await userEvent.click(open);
  await waitFor(() => expect(open).toBeEnabled());
  expect(confirmMock).toHaveBeenCalledOnce();
  expect(loadProjectMock).not.toHaveBeenCalled();
  expect(invokeIpcMock.mock.calls.some((c) => c[0] === 'speleodb_open_project')).toBe(false);
  await userEvent.click(open);
  expect(confirmMock).toHaveBeenCalledTimes(2);
});
