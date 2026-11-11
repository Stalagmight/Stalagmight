/**
 * frontend#10 - an account change asks, in-app, about the unsent SpeleoDB input it
 * would drop (here: an upload description parked while Settings is open for
 * sign-in), in the ONE question it asks after the sign-out guard - together with
 * unsaved edits, when there are any (protectAccountTransition). A sign-out, a
 * same-account recovery and a draft not yet tied to an account keep the input, so
 * they ask nothing.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

const order = vi.hoisted(() => [] as string[]);
const guardSignOutMock = vi.hoisted(() => vi.fn(async (_kind: string) => { order.push('sign-out guard'); return true; }));
vi.mock('../../utils/speleoSignOut', () => ({ guardSpeleoSignOut: guardSignOutMock }));
const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (e: unknown, fallback?: string) => (e as { message?: string })?.message ?? fallback ?? 'error',
}));

import { useSpeleoStore } from '../speleoStore';
import { useProjectStore } from '../projectStore';
import { useExitConfirm } from '../../utils/exitConfirm';
import { captureUploadOwner, getUploadContinuation, setUploadContinuation } from '../../utils/uploadContinuation';

const PATH = '/synthetic/cave.stalx';
const ENDPOINT = 'https://speleodb.example';

beforeEach(() => {
  order.length = 0;
  guardSignOutMock.mockClear();
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation(async (cmd: string, args?: { email?: string }) => {
    order.push(cmd);
    if (cmd === 'speleodb_login') return { user_email: args?.email ?? 'other@example.com', endpoint: ENDPOINT };
    return null;
  });
  useExitConfirm.setState({ request: null });
  setUploadContinuation(null);
  useProjectStore.setState({
    project: { file_path: PATH, created_at: 'synthetic' } as never, isDirty: false, config: null,
    loadConfig: vi.fn(async () => {}),
  });
  useSpeleoStore.setState({
    busy: false, pendingAuth: null, authError: null,
    link: { permission: 'READ_AND_WRITE', editState: 'read-only', lockHolder: null, lockedByOther: false,
      hasUnpushedEdits: true, unwound: false, undoMode: 'full', remoteState: 'up_to_date', noAccess: false, serverError: null },
    scanLocksOnOpen: vi.fn(async () => {}), refreshLinkStatus: vi.fn(async () => {}),
  });
});

const signedInAs = (email: string) => useSpeleoStore.setState({
  session: { loggedIn: true, userEmail: email, endpoint: ENDPOINT },
});
const parkDescription = () => setUploadContinuation(
  { path: PATH, message: 'Synthetic description', closeAfterSave: false }, captureUploadOwner(),
);
const stageLogin = (email: string) => useSpeleoStore.getState().setPendingAuth({
  kind: 'login', email, password: 'synthetic-pass', token: '', useToken: false,
});

async function answerDrafts(yes: boolean) {
  await vi.waitFor(() => expect(useExitConfirm.getState().request).not.toBeNull());
  order.push('drafts question');
  useExitConfirm.getState().request?.answer(yes);
}

describe('applyPendingAuth and unsent SpeleoDB input', () => {
  it('switching account asks after the sign-out guard; declining cancels the login', async () => {
    signedInAs('writer@example.com');
    parkDescription();
    stageLogin('other@example.com');
    const pending = useSpeleoStore.getState().applyPendingAuth(ENDPOINT, false);
    await answerDrafts(false);
    expect(await pending).toEqual({ ok: false, cancelled: true });
    expect(order).toEqual(['sign-out guard', 'drafts question']);
    expect(getUploadContinuation(PATH)?.message).toBe('Synthetic description');
  });

  it('switching account after Discard drops the input, then signs the old account out and logs in', async () => {
    signedInAs('writer@example.com');
    parkDescription();
    stageLogin('other@example.com');
    const pending = useSpeleoStore.getState().applyPendingAuth(ENDPOINT, false);
    await answerDrafts(true);
    expect(await pending).toEqual({ ok: true });
    expect(order.slice(0, 3)).toEqual(['sign-out guard', 'drafts question', 'speleodb_login']);
    expect(getUploadContinuation(PATH)).toBeNull();
  });

  it('a same-email sign-in on the server the input was NOT written for still asks', async () => {
    signedInAs('writer@example.com');
    parkDescription();
    // Settings moved to another server: that signed the old session out, and the
    // saved account now names the new server, so this login reads as a recovery.
    useSpeleoStore.setState({ session: null });
    useProjectStore.setState({ config: { speleodb: { endpoint: 'https://other.example', email: 'writer@example.com' } } as never });
    stageLogin('writer@example.com');
    const pending = useSpeleoStore.getState().applyPendingAuth('https://other.example', false);
    await answerDrafts(false);
    expect(await pending).toEqual({ ok: false, cancelled: true });
    expect(order).toEqual(['drafts question']);
  });

  it.each([
    ['a sign-out keeps the input for the next sign-in', () => {
      signedInAs('writer@example.com');
      parkDescription();
      useSpeleoStore.getState().setPendingAuth({ kind: 'logout' });
    }],
    ['a same-account recovery keeps the input', () => {
      signedInAs('writer@example.com');
      parkDescription();
      stageLogin('Writer@Example.com');
    }],
    ['input written while signed out follows the first sign-in', () => {
      useSpeleoStore.setState({ session: null });
      parkDescription();
      stageLogin('other@example.com');
    }],
  ])('%s, so nothing is asked', async (_label, arrange) => {
    arrange();
    await useSpeleoStore.getState().applyPendingAuth(ENDPOINT, false);
    expect(useExitConfirm.getState().request).toBeNull();
    expect(getUploadContinuation(PATH)?.message).toBe('Synthetic description');
  });
});

describe('one question for unsaved edits and unsent SpeleoDB input', () => {
  it('lists the drafts in the unsaved-changes question, and Save keeps the edits but drops the drafts', async () => {
    signedInAs('writer@example.com');
    parkDescription();
    const saveProject = vi.fn(async () => {
      useProjectStore.setState({ isDirty: false });
      return { success: true, warnings: [], error: null, section_ids: [] };
    });
    useProjectStore.setState({ isDirty: true, saveProject: saveProject as never });
    stageLogin('other@example.com');
    const pending = useSpeleoStore.getState().applyPendingAuth(ENDPOINT, false);
    await vi.waitFor(() => expect(useExitConfirm.getState().request).not.toBeNull());
    const request = useExitConfirm.getState().request;
    expect(request?.message).toContain('the description of your upload');
    expect(request?.saveLabel).toBeTruthy();
    request?.answer('save');
    expect(await pending).toEqual({ ok: true });
    expect(saveProject).toHaveBeenCalledOnce();
    expect(getUploadContinuation(PATH)).toBeNull();
    // ONE question: nothing else was asked.
    expect(useExitConfirm.getState().request).toBeNull();
  });
});

describe('applyPendingAuth beforeApply (Settings saves its staged settings there)', () => {
  const save = vi.fn(async () => { order.push('save'); return true; });
  beforeEach(() => { save.mockClear(); });

  it('runs after the sign-out guard and the drafts question, before the login', async () => {
    signedInAs('writer@example.com');
    parkDescription();
    stageLogin('other@example.com');
    const pending = useSpeleoStore.getState().applyPendingAuth(ENDPOINT, false, { beforeApply: save });
    await answerDrafts(true);
    expect(await pending).toEqual({ ok: true });
    expect(order.slice(0, 4)).toEqual(['sign-out guard', 'drafts question', 'save', 'speleodb_login']);
  });

  it('a cancelled question never runs it', async () => {
    signedInAs('writer@example.com');
    parkDescription();
    stageLogin('other@example.com');
    const pending = useSpeleoStore.getState().applyPendingAuth(ENDPOINT, false, { beforeApply: save });
    await answerDrafts(false);
    expect(await pending).toEqual({ ok: false, cancelled: true });
    expect(save).not.toHaveBeenCalled();
  });

  it('answering false cancels the account change and keeps the staged credentials', async () => {
    useSpeleoStore.setState({ session: null });
    stageLogin('other@example.com');
    const staged = useSpeleoStore.getState().pendingAuth;
    const result = await useSpeleoStore.getState().applyPendingAuth(ENDPOINT, false, { beforeApply: async () => false });
    expect(result).toEqual({ ok: false, cancelled: true });
    expect(order).not.toContain('speleodb_login');
    expect(useSpeleoStore.getState().pendingAuth).toBe(staged);
  });
});
