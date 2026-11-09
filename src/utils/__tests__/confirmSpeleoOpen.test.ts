import { beforeEach, expect, it, vi } from 'vitest';
import { confirmSpeleoFileOpen, confirmSpeleoOpen } from '../confirmSpeleoOpen';
import { useSpeleoStore } from '../../store/speleoStore';
import type { SpeleoProjectSummary } from '../../types/ipc.generated';

const mocks = vi.hoisted(() => ({ confirm: vi.fn(), invoke: vi.fn() }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ confirm: mocks.confirm }));
vi.mock('../ipc', () => ({ invokeIpc: mocks.invoke }));
const project: SpeleoProjectSummary = {
  id: 'synthetic', name: 'Synthetic', permission: 'READ_AND_WRITE',
  lock_holder: '  Diver@Example.com ', type_tag: 'STALAGMIGHT', latest_commit_sha: null,
};
beforeEach(() => {
  mocks.confirm.mockReset().mockResolvedValue(true);
  mocks.invoke.mockReset();
  useSpeleoStore.setState({ session: { loggedIn: true, endpoint: 'https://example.com', userEmail: 'diver@example.com' } });
});
it('suppresses normalized own-lock contention only on the matching deployment', async () => {
  expect(await confirmSpeleoOpen(project, 'https://EXAMPLE.com:443/')).toBe(true);
  expect(mocks.confirm).not.toHaveBeenCalled();
  await confirmSpeleoOpen(project, 'https://other.example.com');
  expect(mocks.confirm).toHaveBeenCalledTimes(1);
});
it.each(['other@example.com', '', null])('unknown and different identities never establish ownership: %s', async (email) => {
  useSpeleoStore.setState({ session: { loggedIn: true, endpoint: 'https://example.com', userEmail: email } });
  await confirmSpeleoOpen(project, 'https://example.com');
  expect(mocks.confirm).toHaveBeenCalledTimes(1);
});
it('distinguishes unverified local inspection from actual read-only permission', async () => {
  mocks.invoke.mockResolvedValue({ project: { ...project, permission: null, lock_holder: null }, endpoint: 'https://example.com', verification: 'unavailable' });
  await confirmSpeleoFileOpen('/synthetic.stalx', () => true, true);
  expect(mocks.invoke).toHaveBeenCalledWith('speleodb_project_access_at', { path: '/synthetic.stalx', localOnly: true });
  expect(mocks.confirm.mock.calls[0]?.[0]).toContain('could not be verified');
  mocks.confirm.mockClear();
  await confirmSpeleoOpen({ ...project, permission: 'READ_ONLY', lock_holder: null });
  expect(mocks.confirm.mock.calls[0]?.[0]).not.toContain('could not be verified');
});
it('signed-out local opens warn without granting access and cancellation is returned', async () => {
  useSpeleoStore.setState({ session: null });
  mocks.invoke.mockResolvedValue({ project: { ...project, permission: null }, endpoint: 'https://example.com', verification: 'unavailable' });
  mocks.confirm.mockResolvedValue(false);
  expect(await confirmSpeleoFileOpen('/synthetic.stalx')).toBe(false);
});
it('suppresses a probe that lost request ownership before confirmation', async () => {
  mocks.invoke.mockResolvedValue({ project, endpoint: 'https://example.com', verification: 'verified' });
  expect(await confirmSpeleoFileOpen('/synthetic.stalx', () => false)).toBe(false);
  expect(mocks.confirm).not.toHaveBeenCalled();
});
