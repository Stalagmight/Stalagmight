import { beforeEach, describe, expect, it, vi } from 'vitest';
// The draft question is in-app; the native dialog must never be reached.
const nativeConfirm = vi.hoisted(() => vi.fn(async () => true));
vi.mock('@tauri-apps/plugin-dialog', () => ({ confirm: nativeConfirm, ask: nativeConfirm, message: nativeConfirm }));
import { confirmWorkspaceDrafts, hasWorkspaceDrafts, registerWorkspaceDraft, type AccountTarget } from '../workspaceDrafts';
import { useExitConfirm } from '../exitConfirm';
import { captureUploadOwner, getUploadContinuation, setUploadContinuation } from '../uploadContinuation';
import { getPublishContinuation, setPublishContinuation } from '../publishContinuation';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';

const answer = async (yes: boolean) => {
  await vi.waitFor(() => expect(useExitConfirm.getState().request).not.toBeNull());
  const request = useExitConfirm.getState().request;
  request?.answer(yes);
  return request;
};

const signedInAs = (email: string) => useSpeleoStore.setState({
  session: { loggedIn: true, userEmail: email, endpoint: 'https://speleodb.example' },
});

beforeEach(() => {
  useExitConfirm.setState({ request: null });
  nativeConfirm.mockClear();
  setUploadContinuation(null);
  setPublishContinuation(null);
  useProjectStore.setState({ project: { file_path: '/synthetic/cave.stalx', created_at: 'synthetic' } as never, isDirty: true });
  useSpeleoStore.setState({ busy: false, link: null, session: null });
  useUIStore.setState({ activeDialog: null, dialogData: {} });
});

describe('workspace draft guard', () => {
  it('asks in-app, and discards only on a yes', async () => {
    const discard = vi.fn();
    const unregister = registerWorkspaceDraft({ label: () => 'the synthetic form', dirty: () => true, discard });
    expect(hasWorkspaceDrafts()).toBe(true);
    const declined = confirmWorkspaceDrafts();
    expect((await answer(false))?.message).toContain('the synthetic form');
    expect(await declined).toBe(false);
    expect(discard).not.toHaveBeenCalled();
    const accepted = confirmWorkspaceDrafts();
    await answer(true);
    expect(await accepted).toBe(true);
    expect(discard).toHaveBeenCalledOnce();
    expect(nativeConfirm).not.toHaveBeenCalled();
    unregister();
    expect(hasWorkspaceDrafts()).toBe(false);
  });

  it('asks nothing when no draft holds input', async () => {
    const unregister = registerWorkspaceDraft({ label: () => 'x', dirty: () => false, discard: vi.fn() });
    expect(await confirmWorkspaceDrafts()).toBe(true);
    expect(useExitConfirm.getState().request).toBeNull();
    unregister();
  });

  it('a flow that stopped being current while asking discards nothing', async () => {
    const discard = vi.fn();
    const unregister = registerWorkspaceDraft({ label: () => 'x', dirty: () => true, discard });
    let current = true;
    const pending = confirmWorkspaceDrafts(() => current);
    current = false;
    await answer(true);
    expect(await pending).toBe(false);
    expect(discard).not.toHaveBeenCalled();
    unregister();
  });
});

describe('the upload description draft', () => {
  it('is a draft while it has something to describe, and Discard drops it and closes the upload dialog', async () => {
    setUploadContinuation({ path: '/synthetic/cave.stalx', message: 'Surveyed the synthetic sump', closeAfterSave: false });
    const closeDialog = vi.fn();
    useUIStore.setState({ activeDialog: 'speleodb-done', closeDialog });
    expect(hasWorkspaceDrafts()).toBe(true);
    const pending = confirmWorkspaceDrafts();
    expect((await answer(true))?.message).toContain('the description of your upload');
    expect(await pending).toBe(true);
    expect(getUploadContinuation('/synthetic/cave.stalx')).toBeNull();
    expect(closeDialog).toHaveBeenCalledOnce();
  });

  it.each([
    ['a blank description', '   ', {}],
    ['nothing left to upload', 'Done already', { isDirty: false }],
    ['an upload in flight', 'Being sent', { busy: true }],
  ])('is not a draft with %s', (_label, message, state) => {
    setUploadContinuation({ path: '/synthetic/cave.stalx', message, closeAfterSave: false });
    if ('isDirty' in state) useProjectStore.setState({ isDirty: state.isDirty });
    if ('busy' in state) useSpeleoStore.setState({ busy: state.busy });
    expect(hasWorkspaceDrafts()).toBe(false);
  });

  const switchTo = (email: string | null): AccountTarget => ({ email, endpoint: 'https://speleodb.example' });
  it.each<[string, AccountTarget, boolean]>([
    ['a sign-out keeps it for the next sign-in', { signedOut: true }, false],
    ['the same account keeps it', switchTo('Writer@Example.com'), false],
    ['another account drops it', switchTo('other@example.com'), true],
    ['a pasted token of unknown account may drop it', switchTo(null), true],
    ['another server drops it', { email: 'writer@example.com', endpoint: 'https://other.example' }, true],
  ])('account change: %s', (_label, target, dropped) => {
    signedInAs('writer@example.com');
    setUploadContinuation({ path: '/synthetic/cave.stalx', message: 'Synthetic note', closeAfterSave: false }, captureUploadOwner());
    expect(hasWorkspaceDrafts(target)).toBe(dropped);
  });

  it('a draft written while signed out follows whoever signs in', () => {
    setUploadContinuation({ path: '/synthetic/cave.stalx', message: 'Synthetic note', closeAfterSave: false });
    expect(hasWorkspaceDrafts(switchTo('anyone@example.com'))).toBe(false);
  });
});

describe('the parked project form draft', () => {
  it('is a draft while it holds input, and Discard drops it', async () => {
    setPublishContinuation({ description: 'Synthetic system', country: '', latitude: '', longitude: '' });
    expect(hasWorkspaceDrafts()).toBe(true);
    const pending = confirmWorkspaceDrafts();
    expect((await answer(true))?.message).toContain('the SpeleoDB project form');
    expect(await pending).toBe(true);
    expect(getPublishContinuation()).toBeNull();
  });

  it('an empty form is not a draft', () => {
    setPublishContinuation({ description: ' ', country: '', latitude: '', longitude: '' });
    expect(hasWorkspaceDrafts()).toBe(false);
  });
});
