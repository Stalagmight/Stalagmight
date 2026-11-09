import { beforeEach, expect, it, vi } from 'vitest';
import { releaseSpeleoLock } from '../releaseSpeleoLock';
import { useProjectStore } from '../../store/projectStore';
const mocks = vi.hoisted(() => ({ invoke: vi.fn(), confirm: vi.fn() }));
vi.mock('../ipc', () => ({ invokeIpc: mocks.invoke }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ confirm: mocks.confirm }));
const expected = { session_generation: '42', project_id: 'synthetic', endpoint: 'https://example.com', user_email: 'u@x' };
beforeEach(() => {
  mocks.invoke.mockReset().mockResolvedValue({ release_context: expected });
  mocks.confirm.mockReset().mockResolvedValue(true);
  useProjectStore.setState({ project: { file_path: '/synthetic.stalx' } as never });
});
it('inspects locally before confirming and releases only with the exact session context', async () => {
  expect(await releaseSpeleoLock(() => true)).toBe(true);
  expect(mocks.invoke.mock.calls.map(([name]) => name)).toEqual(['speleodb_project_access_at', 'speleodb_release_current_lock']);
  expect(mocks.invoke).toHaveBeenNthCalledWith(1, 'speleodb_project_access_at', { path: '/synthetic.stalx', localOnly: true });
  expect(mocks.invoke).toHaveBeenNthCalledWith(2, 'speleodb_release_current_lock', { ownOnly: true, allowUnpushed: true, expected });
});
it('does not release after cancelled consent', async () => {
  mocks.confirm.mockResolvedValue(false);
  expect(await releaseSpeleoLock(() => true)).toBe(false);
  expect(mocks.invoke).toHaveBeenCalledTimes(1);
});
it('does not prompt after inspection loses ownership', async () => {
  expect(await releaseSpeleoLock(() => false)).toBe(false);
  expect(mocks.confirm).not.toHaveBeenCalled();
});
it('does not release when the session changes while consent is pending', async () => {
  let current = true;
  mocks.confirm.mockImplementation(async () => { current = false; return true; });
  expect(await releaseSpeleoLock(() => current)).toBe(false);
  expect(mocks.invoke).toHaveBeenCalledTimes(1);
});
it('rejects a missing release context before any mutation', async () => {
  mocks.invoke.mockResolvedValue({ release_context: null });
  await expect(releaseSpeleoLock(() => true)).rejects.toThrow('account changed');
  expect(mocks.invoke).toHaveBeenCalledTimes(1);
});
