/**
 * security#4 / #668 - Connect in Settings first asks what the account change
 * needs (unsent drafts it would drop), then saves the staged settings, then signs
 * in over the connection ON SCREEN (the staged endpoint, and - in a development
 * build only - the staged insecure-HTTP value), refreshes and closes Settings. A
 * failure applies nothing, keeps Settings open and is shown in an in-app error
 * dialog that names the server that was tried. Disconnect runs the sign-out flow
 * first; cancelling it leaves the user connected and Settings open.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

import '../../i18n';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (e: unknown) => (e && typeof e === 'object' && 'message' in e ? String(e.message) : String(e)),
}));

import SettingsDialog from './SettingsDialog';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { defaultConfig } from '../../types/config';
import type { Config } from '../../types/config';
import { useExitConfirm } from '../../utils/exitConfirm';
import { useSpeleoSignOut } from '../../utils/speleoSignOut';
import { captureUploadOwner, getUploadContinuation, setUploadContinuation } from '../../utils/uploadContinuation';

const ENDPOINT = 'https://lab.example/';
const savedConfig: Config = {
  ...defaultConfig,
  speleodb: { ...defaultConfig.speleodb, endpoint: ENDPOINT, insecure_http: false, email: null },
};
let closeDialog: ReturnType<typeof vi.fn>;
let updateConfig: ReturnType<typeof vi.fn>;

beforeEach(() => {
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation(async (cmd: string) => {
    if (cmd === 'speleodb_login') return { user_email: 'synthetic@example.com', endpoint: ENDPOINT };
    if (cmd === 'get_config') return useProjectStore.getState().config;
    if (cmd === 'speleodb_session') return { logged_in: false };
    if (cmd === 'speleodb_list_projects') return [];
    return null;
  });
  closeDialog = vi.fn();
  updateConfig = vi.fn(async (config: Config) => {
    useProjectStore.setState({ config });
    return { success: true };
  });
  useProjectStore.setState({ config: savedConfig, updateConfig, isDirty: false, project: null });
  useUIStore.setState({
    activeDialog: 'settings', dialogData: { initialCategory: 'speleodb' }, closeDialog, showToast: vi.fn(),
  });
  useSpeleoStore.setState({
    session: null, pendingAuth: null, authError: null, link: null, projectLinkKind: 'local',
    refreshSession: vi.fn(async () => {}), scanLocksOnOpen: vi.fn(async () => {}), refreshLinkStatus: vi.fn(async () => {}),
  });
});

async function connect() {
  await userEvent.type(screen.getByTestId('speleodb-settings-email'), 'synthetic@example.com');
  await userEvent.type(screen.getByTestId('speleodb-settings-password'), '[example_password]');
  await userEvent.click(screen.getByTestId('speleodb-settings-try-login'));
}

const loginCall = () => invokeIpcMock.mock.calls.find((c) => c[0] === 'speleodb_login');
const called = (cmd: string) => invokeIpcMock.mock.calls.some((c) => c[0] === cmd);
const orderOf = (cmd: string) => invokeIpcMock.mock.invocationCallOrder[invokeIpcMock.mock.calls.findIndex((c) => c[0] === cmd)] ?? -1;

afterEach(() => { vi.unstubAllEnvs(); });

describe('Settings Connect', () => {
  it('signs in with the connection on screen and closes Settings', async () => {
    render(<SettingsDialog />);
    await screen.findByTestId('speleodb-settings-try-login');
    await connect();

    await waitFor(() => expect(closeDialog).toHaveBeenCalledOnce());
    expect(loginCall()?.[1]).toMatchObject({ endpoint: ENDPOINT, insecureHttp: false });
  });

  it('a development build sends the insecure-HTTP box on screen, saved with the rest first', async () => {
    vi.stubEnv('DEV', true);
    render(<SettingsDialog />);
    await userEvent.click(await screen.findByTestId('toggle-speleodb-insecure-http'));
    await connect();

    await waitFor(() => expect(loginCall()).toBeDefined());
    expect(loginCall()?.[1]).toMatchObject({ insecureHttp: true });
    expect((updateConfig.mock.calls[0]?.[0] as Config).speleodb.insecure_http).toBe(true);
    expect(updateConfig.mock.invocationCallOrder[0] ?? Infinity).toBeLessThan(orderOf('speleodb_login'));
  });

  it('a release build has no insecure-HTTP control and sends no choice, even with one stored', async () => {
    vi.stubEnv('DEV', false);
    useProjectStore.setState({ config: { ...savedConfig, speleodb: { ...savedConfig.speleodb, insecure_http: true } } });
    render(<SettingsDialog />);
    await screen.findByTestId('speleodb-settings-try-login');
    expect(screen.queryByTestId('toggle-speleodb-insecure-http')).not.toBeInTheDocument();
    await connect();

    await waitFor(() => expect(loginCall()).toBeDefined());
    expect(loginCall()?.[1]).not.toHaveProperty('insecureHttp');
  });

  it('saves the other tabs\' staged edits before it signs in, so closing discards nothing', async () => {
    render(<SettingsDialog />);
    await userEvent.click(await screen.findByTestId('settings-tab-general'));
    await userEvent.click(await screen.findByTestId('toggle-auto-save'));
    await userEvent.click(screen.getByTestId('settings-tab-speleodb'));
    await connect();

    await waitFor(() => expect(closeDialog).toHaveBeenCalledOnce());
    expect(updateConfig).toHaveBeenCalledOnce();
    const written = updateConfig.mock.calls[0]?.[0] as Config;
    expect(written.general.auto_save).toBe(!savedConfig.general.auto_save);
    const saveOrder = updateConfig.mock.invocationCallOrder[0] ?? Infinity;
    const loginIndex = invokeIpcMock.mock.calls.findIndex((c) => c[0] === 'speleodb_login');
    expect(saveOrder).toBeLessThan(invokeIpcMock.mock.invocationCallOrder[loginIndex] ?? -1);
  });

  it('a failed sign-in applies no session, keeps Settings open and shows the error in a dialog', async () => {
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'speleodb_login') throw { message: 'Invalid credentials', action: 'ReLogin', status: 400 };
      return null;
    });
    render(<SettingsDialog />);
    await screen.findByTestId('speleodb-settings-try-login');
    await connect();

    const dialog = await screen.findByTestId('speleodb-connection-error');
    expect(dialog).toHaveTextContent('Could not connect to SpeleoDB: Invalid credentials');
    expect(closeDialog).not.toHaveBeenCalled();
    expect(useSpeleoStore.getState().session).toBeNull();
  });
});

describe('Settings Connect failure feedback', () => {
  const failWith = (error: unknown) => invokeIpcMock.mockImplementation(async (cmd: string) => {
    if (cmd === 'speleodb_login') throw error;
    if (cmd === 'get_config') return useProjectStore.getState().config;
    return null;
  });

  it('names the server on screen, not "the configured server", when it cannot be reached', async () => {
    failWith({ message: 'Network error talking to SpeleoDB: refused', action: 'Retry', status: null, transport: 'Refused' });
    render(<SettingsDialog />);
    const endpoint = await screen.findByTestId('input-speleodb-endpoint');
    await userEvent.clear(endpoint);
    await userEvent.type(endpoint, 'https://unsaved.example/');
    await connect();

    const dialog = await screen.findByTestId('speleodb-connection-error');
    expect(dialog).toHaveTextContent('Could not connect to SpeleoDB at https://unsaved.example/.');
    expect(dialog).not.toHaveTextContent('configured server');
  });

  it('explains a plain-HTTP refusal with the automatic exceptions', async () => {
    failWith({
      message: 'HTTPS is required. HTTP is allowed only on localhost, 127.0.0.1 or 192.168.x.x. Rejected endpoint: http://lab.example/',
      action: 'Fatal', status: null, transport: null, endpoint_refusal: 'PlainHttp',
    });
    render(<SettingsDialog />);
    await screen.findByTestId('speleodb-settings-try-login');
    await connect();

    const dialog = await screen.findByTestId('speleodb-connection-error');
    expect(dialog).toHaveTextContent(ENDPOINT);
    expect(dialog).toHaveTextContent('HTTPS is required');
    expect(dialog).toHaveTextContent('192.168.x.x');
    expect(dialog).not.toHaveTextContent('Rejected endpoint');
  });
});

describe('Settings Connect asks first, then saves', () => {
  const PATH = '/synthetic/cave.stalx';
  beforeEach(() => {
    // An upload description written for another account on this server, kept
    // through a sign-out: connecting as someone else would drop it.
    useExitConfirm.setState({ request: null });
    useProjectStore.setState({ project: { file_path: PATH, created_at: 'synthetic' } as never });
    useSpeleoStore.setState({
      session: { loggedIn: true, userEmail: 'writer@example.com', endpoint: ENDPOINT },
      link: { permission: 'READ_AND_WRITE', editState: 'read-only', lockHolder: null, lockedByOther: false,
        hasUnpushedEdits: true, unwound: false, undoMode: 'full', remoteState: 'up_to_date', noAccess: false, serverError: null },
    });
    setUploadContinuation({ path: PATH, message: 'Synthetic description', closeAfterSave: false }, captureUploadOwner());
    useSpeleoStore.setState({ session: null });
  });
  afterEach(() => { setUploadContinuation(null); });

  async function connectWithAStagedEdit() {
    render(<SettingsDialog />);
    await userEvent.click(await screen.findByTestId('settings-tab-general'));
    await userEvent.click(await screen.findByTestId('toggle-auto-save'));
    await userEvent.click(screen.getByTestId('settings-tab-speleodb'));
    await connect();
    await waitFor(() => expect(useExitConfirm.getState().request).not.toBeNull());
    expect(useExitConfirm.getState().request?.message).toContain('the description of your upload');
    // Asked BEFORE anything is saved or sent.
    expect(updateConfig).not.toHaveBeenCalled();
    expect(loginCall()).toBeUndefined();
  }

  it('Cancel saves nothing, sends nothing and keeps the draft and Settings', async () => {
    await connectWithAStagedEdit();
    await act(async () => useExitConfirm.getState().request?.answer(false));
    await waitFor(() => expect(screen.getByTestId('speleodb-settings-try-login')).toBeEnabled());
    expect(updateConfig).not.toHaveBeenCalled();
    expect(loginCall()).toBeUndefined();
    expect(closeDialog).not.toHaveBeenCalled();
    expect(getUploadContinuation(PATH)?.message).toBe('Synthetic description');
  });

  it('Discard drops the draft, then saves the staged settings, then signs in', async () => {
    await connectWithAStagedEdit();
    await act(async () => useExitConfirm.getState().request?.answer(true));
    await waitFor(() => expect(closeDialog).toHaveBeenCalledOnce());
    expect(updateConfig).toHaveBeenCalledOnce();
    expect(updateConfig.mock.invocationCallOrder[0] ?? Infinity).toBeLessThan(orderOf('speleodb_login'));
    expect(getUploadContinuation(PATH)).toBeNull();
  });
});

describe('Settings Disconnect runs the sign-out flow first', () => {
  beforeEach(() => {
    useSpeleoSignOut.setState({ request: null });
    useSpeleoStore.setState({ session: { loggedIn: true, userEmail: 'writer@example.com', endpoint: ENDPOINT } });
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'get_config') return useProjectStore.getState().config;
      if (cmd === 'speleodb_session') return { logged_in: true, user_email: 'writer@example.com', endpoint: ENDPOINT };
      // The account holds a lock over a copy with un-uploaded work.
      if (cmd === 'speleodb_list_projects') {
        return [{ id: 'held', name: 'Synthetic held', permission: 'READ_AND_WRITE', lock_holder: 'writer@example.com',
          type_tag: 'stalx', latest_commit_sha: null }];
      }
      if (cmd === 'speleodb_local_project_states') {
        return { held: { has_local_copy: true, has_unpushed_edits: true, last_edit_at: null, is_open: false, state_unknown: false } };
      }
      return null;
    });
  });

  async function disconnect() {
    render(<SettingsDialog />);
    await userEvent.click(await screen.findByTestId('speleodb-settings-logout'));
    await waitFor(() => expect(useSpeleoSignOut.getState().request?.survey?.unuploaded).toHaveLength(1));
    expect(called('speleodb_logout')).toBe(false);
  }

  it('Cancel in the sign-out dialog leaves the user connected and Settings open', async () => {
    await disconnect();
    await act(async () => useSpeleoSignOut.getState().request?.settle(false));
    await waitFor(() => expect(screen.getByTestId('speleodb-settings-logout')).toBeEnabled());
    expect(useSpeleoStore.getState().session?.loggedIn).toBe(true);
    expect(useSpeleoStore.getState().pendingAuth).toBeNull();
    expect(screen.getByTestId('speleodb-account-signed-in')).toBeVisible();
    expect(called('speleodb_logout')).toBe(false);
    expect(closeDialog).not.toHaveBeenCalled();
  });

  it('once the flow lets it go ahead, signs out and stays on the SpeleoDB tab', async () => {
    await disconnect();
    await act(async () => useSpeleoSignOut.getState().request?.settle(true));
    await waitFor(() => expect(called('speleodb_logout')).toBe(true));
    expect(await screen.findByTestId('speleodb-account-login-form')).toBeVisible();
    // The whole Disconnect has settled (its form is no longer busy).
    await waitFor(() => expect(screen.getByTestId('speleodb-settings-email')).toBeEnabled());
    expect(closeDialog).not.toHaveBeenCalled();
  });
});
