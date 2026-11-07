/**
 * [#576 Q-W61-3-D1] A logout whose lock release the server refused still signs out,
 * and then warns, naming each project whose write lock is still held on SpeleoDB.
 *
 * The backend's `speleodb_logout` answers `{ locks_left_held }`; it used to answer
 * nothing and leave the lock held without a word.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { useSpeleoStore } from '../speleoStore';
import { useUIStore } from '../uiStore';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { invokeIpcMock } from '../../test/factories/ipc';

vi.mock('../../utils/ipc');

function signedInWithLogoutPending() {
  useSpeleoStore.getState().setSessionFromLogin({
    user_email: 'u@x',
    endpoint: 'https://x/',
    keychain_backed: true,
    storage: 'keychain',
    keychain: 'available',
    needs_storage_choice: false,
    replaced_session: false,
    locks_left_held: [],
  });
  useSpeleoStore.getState().setPendingAuth({ kind: 'logout' });
}

/** The logout answers `report`; no lock held beforehand, so no sign-out prompt
 * (#576 SIGNOUT-PROMPT-D1 - the prompt has its own tests). */
function logoutAnswers(report: unknown) {
  invokeIpcMock.mockImplementation(async (cmd: string) =>
    cmd === 'speleodb_sign_out_locks' ? { releasable: [], unpushed: [] } : report,
  );
}

beforeEach(() => {
  useUIStore.setState({ toasts: [] });
  signedInWithLogoutPending();
});

afterEach(() => {
  invokeIpcMock.mockReset();
  useUIStore.setState({ toasts: [] });
});

describe('logout that could not release a lock (#576 Q-W61-3-D1)', () => {
  it('signs out, then warns naming each project still locked', async () => {
    logoutAnswers({
      locks_left_held: [
        { project_id: '11111111-1111-1111-1111-111111111111', project_name: 'CaveA' },
      ],
    });

    const result = await useSpeleoStore.getState().applyPendingAuth('https://x/');

    expect(result.ok).toBe(true);
    expect(useSpeleoStore.getState().session).toBeNull();
    const toasts = useUIStore.getState().toasts;
    expect(toasts).toHaveLength(1);
    const toast = toasts[0]!;
    expect(toast.key).toBe(TOAST_KEYS.SPELEODB_LOGOUT_LOCKS_HELD);
    expect(toast.kind).toBe('warning');
    expect(toast.message).toBe(
      'Signed out, but "CaveA" is still locked on SpeleoDB. Release it on the SpeleoDB website, or sign in and close it.',
    );
  });

  it('names every project, in one warning', async () => {
    logoutAnswers({
      locks_left_held: [
        { project_id: 'a', project_name: 'CaveA' },
        { project_id: 'b', project_name: 'CaveB' },
      ],
    });

    await useSpeleoStore.getState().applyPendingAuth('https://x/');

    const toasts = useUIStore.getState().toasts;
    expect(toasts).toHaveLength(1);
    expect(toasts[0]!.message).toBe(
      'Signed out, but "CaveA", "CaveB" are still locked on SpeleoDB. Release them on the SpeleoDB website, or sign in and close them.',
    );
  });

  it('warns about nothing when every release landed', async () => {
    logoutAnswers({ locks_left_held: [] });

    const result = await useSpeleoStore.getState().applyPendingAuth('https://x/');

    expect(result.ok).toBe(true);
    expect(useSpeleoStore.getState().session).toBeNull();
    expect(useUIStore.getState().toasts).toHaveLength(0);
  });
});
