import { beforeEach, expect, it, vi } from 'vitest';
import { dispatchProjectSave } from '../dispatchProjectSave';
import { setUploadContinuation } from '../uploadContinuation';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore, type SpeleoLinkState } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { invokeIpc } from '../ipc';
vi.mock('../ipc', () => ({ invokeIpc: vi.fn(), getErrorMessage: String }));
const link: SpeleoLinkState = { permission: 'READ_AND_WRITE', editState: 'editing', lockHolder: null,
  lockedByOther: false, hasUnpushedEdits: true, unwound: false, undoMode: 'full', remoteState: 'up_to_date', noAccess: false, serverError: null };
const signedIn = { loggedIn: true, userEmail: 'me@example.com', endpoint: 'https://synthetic.example' };
beforeEach(() => {
  vi.mocked(invokeIpc).mockReset();
  setUploadContinuation(null);
  useProjectStore.setState({ project: { file_path: '/synthetic.stalx' } as never, isDirty: false, saveProject: vi.fn(async () => ({ success: true }) as never) });
  useSpeleoStore.setState({ link: null, session: signedIn as never });
  useUIStore.setState({ openDialog: vi.fn(), showToast: vi.fn() });
});
it('keeps local Save local', async () => {
  await dispatchProjectSave();
  expect(useProjectStore.getState().saveProject).toHaveBeenCalledOnce();
  expect(useUIStore.getState().openDialog).not.toHaveBeenCalled();
});
it('uses upload confirmation while editing, without uploading or saving immediately', async () => {
  useSpeleoStore.setState({ link });
  await dispatchProjectSave({ closeAfterSave: true });
  expect(useUIStore.getState().openDialog).toHaveBeenCalledWith('speleodb-done', { mode: 'push', closeAfterSave: true });
  expect(useProjectStore.getState().saveProject).not.toHaveBeenCalled();
});
it('reports no changes without creating an empty upload', async () => {
  useSpeleoStore.setState({ link: { ...link, hasUnpushedEdits: false } });
  await dispatchProjectSave();
  expect(useUIStore.getState().showToast).toHaveBeenCalledWith('No changes to upload', expect.any(String), 'info');
  expect(useUIStore.getState().openDialog).not.toHaveBeenCalled();
});
/**
 * Pre-PR delta review finding 4: a SpeleoDB copy with no unsaved edits and
 * nothing to upload can still hold changes only the backend tracks (a 0.6.0
 * copy's SpeleoDB name fold), which the exit prompt asks about. Ctrl+S said
 * "No changes to upload" and saved nothing, so the user was then asked about
 * unsaved changes they had just tried to save. Injection: dropping the
 * backend ask shows the toast and saves nothing.
 */
it('nothing to upload but changes only the backend tracks: saves them on this computer', async () => {
  vi.mocked(invokeIpc).mockImplementation((async (command: string) => command === 'has_unsaved_changes') as never);
  useSpeleoStore.setState({ link: { ...link, hasUnpushedEdits: false } });
  const result = await dispatchProjectSave();
  expect(invokeIpc).toHaveBeenCalledWith('has_unsaved_changes', undefined, expect.anything());
  expect(useProjectStore.getState().saveProject).toHaveBeenCalledOnce();
  expect(result).toEqual({ success: true });
  expect(useUIStore.getState().showToast).not.toHaveBeenCalledWith('No changes to upload', expect.anything(), expect.anything());
  expect(useUIStore.getState().openDialog).not.toHaveBeenCalled();
});
it('retains the close continuation across recovery', async () => {
  useSpeleoStore.setState({ link });
  setUploadContinuation({ path: '/synthetic.stalx', message: 'Retained description', closeAfterSave: true });
  await dispatchProjectSave();
  expect(useUIStore.getState().openDialog).toHaveBeenCalledWith('speleodb-done', { mode: 'push', closeAfterSave: true });
});

// frontend#6: when the project cannot be uploaded right now, Save still saves.
it('without the lock but able to take it: saves locally first, then opens Save Changes to take the lock', async () => {
  useSpeleoStore.setState({ link: { ...link, editState: 'read-only' } });
  useProjectStore.setState({ isDirty: true });
  const order: string[] = [];
  useProjectStore.setState({ saveProject: vi.fn(async () => { order.push('save'); return { success: true }; }) as never });
  useUIStore.setState({ openDialog: vi.fn(() => { order.push('dialog'); }) as never });
  expect(await dispatchProjectSave({ closeAfterSave: true })).toBeNull();
  expect(order).toEqual(['save', 'dialog']);
  expect(useUIStore.getState().openDialog).toHaveBeenCalledWith('speleodb-done', { mode: 'push', closeAfterSave: true, acquire: true });
});
it.each([
  ['signed out', { session: null }, 'Connect to SpeleoDB'],
  ['locked by someone else', { link: { ...link, editState: 'read-only', lockedByOther: true, lockHolder: 'other@example.com' } }, 'other@example.com'],
  ['read-only permission', { link: { ...link, editState: 'read-only', permission: 'READ_ONLY' } }, 'cannot upload'],
  ['no access', { link: { ...link, editState: 'editing', noAccess: true } }, 'cannot upload'],
] as const)('%s: saves on this computer, says why it was not uploaded, and lets Save and Close close', async (_name, state, reason) => {
  useSpeleoStore.setState({ link, ...state } as never);
  useProjectStore.setState({ isDirty: true });
  const result = await dispatchProjectSave({ closeAfterSave: true });
  expect(useProjectStore.getState().saveProject).toHaveBeenCalledOnce();
  expect(result).toEqual({ success: true });
  expect(useUIStore.getState().openDialog).not.toHaveBeenCalled();
  expect(useUIStore.getState().showToast).toHaveBeenCalledWith(expect.stringContaining(reason), 'speleodb-saved-locally', 'warning');
  expect(useUIStore.getState().showToast).toHaveBeenCalledWith(expect.stringContaining('Saved on this computer'), expect.any(String), 'warning');
});
it('a failed local save stops there and reports the failure', async () => {
  useSpeleoStore.setState({ link: { ...link, editState: 'read-only' }, session: null });
  useProjectStore.setState({ isDirty: true, saveProject: vi.fn(async () => ({ success: false, error: 'Synthetic disk failure' })) as never });
  const result = await dispatchProjectSave();
  expect(result).toEqual({ success: false, error: 'Synthetic disk failure' });
  expect(useUIStore.getState().showToast).not.toHaveBeenCalledWith(expect.anything(), 'speleodb-saved-locally', expect.anything());
});
