import { getPublishContinuation, setPublishContinuation } from './publishContinuation';
import { beforeEach, expect, it, vi } from 'vitest';
import { finishSpeleoSettings } from './finishSpeleoSettings';
import { captureUploadOwner, getUploadContinuation, setUploadContinuation } from './uploadContinuation';
import { useProjectStore } from '../store/projectStore';
import { useSpeleoStore } from '../store/speleoStore';
import { useUIStore } from '../store/uiStore';
const openDialog = vi.fn();
beforeEach(() => {
  vi.clearAllMocks();
  setUploadContinuation(null);
  setPublishContinuation(null);
  useProjectStore.setState({ project: { file_path: '/synthetic.stalx' } as never });
  useSpeleoStore.setState({ session: null });
  useUIStore.setState({ closeDialog: vi.fn(), openDialog });
});
it('returns to Save confirmation with its description and close intent after authentication', () => {
  setUploadContinuation({ path: '/synthetic.stalx', message: 'Validated synthetic description', closeAfterSave: true });
  useSpeleoStore.setState({ session: { loggedIn: true, endpoint: 'https://synthetic.example', userEmail: 'user@example.com' } });
  finishSpeleoSettings();
  expect(openDialog).toHaveBeenCalledWith('speleodb-done', { mode: 'push', closeAfterSave: true });
  expect(getUploadContinuation('/synthetic.stalx')?.message).toBe('Validated synthetic description');
});
it.each(['project', 'account'])('does not resume a save after a different %s replaces its owner', (kind) => {
  useSpeleoStore.setState({ session: { loggedIn: true, endpoint: 'https://synthetic.example', userEmail: 'user@example.com' } });
  setUploadContinuation({ path: '/synthetic.stalx', message: 'Original work', closeAfterSave: true }, captureUploadOwner());
  if (kind === 'project') useProjectStore.setState({ project: { file_path: '/other.stalx' } as never });
  else useSpeleoStore.setState({ session: { loggedIn: true, endpoint: 'https://synthetic.example', userEmail: 'other@example.com' } });
  finishSpeleoSettings();
  expect(openDialog).not.toHaveBeenCalled();
});


it('returns to project creation with its conversion draft after Settings authentication without publishing', () => {
  const draft = { description: 'Original creation draft', country: 'US', latitude: '', longitude: '' };
  setPublishContinuation(draft);
  useSpeleoStore.setState({ session: { loggedIn: true, endpoint: 'https://synthetic.example', userEmail: 'user@example.com' } });
  finishSpeleoSettings();
  expect(openDialog).toHaveBeenCalledWith('publish-to-speleodb');
  expect(getPublishContinuation()).toEqual(draft);
});
it.each(['project', 'account'])('does not resume conversion after its %s changes in Settings', (change) => {
  useSpeleoStore.setState({ session: { loggedIn: true, endpoint: 'https://synthetic.example', userEmail: 'user@example.com' } });
  setPublishContinuation({ description: 'Original draft', country: 'US', latitude: '', longitude: '' });
  if (change === 'project') useProjectStore.setState({ project: { file_path: '/other.stalx' } as never });
  else useSpeleoStore.setState({ session: { loggedIn: true, endpoint: 'https://synthetic.example', userEmail: 'other@example.com' } });
  finishSpeleoSettings();
  expect(openDialog).not.toHaveBeenCalled();
});
