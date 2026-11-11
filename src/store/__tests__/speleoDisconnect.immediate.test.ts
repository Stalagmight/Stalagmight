import { afterEach, expect, it, vi } from 'vitest';
import { useSpeleoStore } from '../speleoStore';
import { useProjectStore } from '../projectStore';

const invoke = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({ invokeIpc: invoke }));
const originalSpeleo = useSpeleoStore.getState();
const originalProject = useProjectStore.getState();
afterEach(() => {
  useSpeleoStore.setState(originalSpeleo, true);
  useProjectStore.setState(originalProject, true);
});

it('clears the remote listing before disconnect cleanup finishes', async () => {
  let complete!: () => void;
  invoke.mockImplementation(async (command: string) => {
    // The sign-out flow's survey: no held locks, nothing un-uploaded.
    if (command === 'speleodb_list_projects') return [];
    if (command === 'speleodb_logout') return new Promise<void>((resolve) => { complete = resolve; });
    return null;
  });
  useProjectStore.setState({ isDirty: false, loadConfig: vi.fn(async () => {}), loadRecentProjects: vi.fn(async () => {}) });
  useSpeleoStore.setState({
    session: { loggedIn: true, endpoint: 'https://example.com', userEmail: 'synthetic@example.com' },
    pendingAuth: { kind: 'logout' },
    remoteProjects: { synthetic: 'Survey' },
  });
  const before = useSpeleoStore.getState().sessionGeneration;
  const request = useSpeleoStore.getState().applyPendingAuth('https://example.com');
  // The sign-out flow runs first; the logout then starts with the listing gone.
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith('speleodb_logout'));
  expect(useSpeleoStore.getState().session).toBeNull();
  expect(useSpeleoStore.getState().remoteProjects).toEqual({});
  expect(useSpeleoStore.getState().sessionGeneration).toBeGreaterThan(before);
  complete();
  expect(await request).toEqual({ ok: true });
});
