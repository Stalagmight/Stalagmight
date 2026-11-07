/**
 * [#576 NF-W72-1] A sign-in that replaces a live session hands the old session's lock
 * back first (FZ-SDB-3-D1). When that release fails, the sign-in still completes, and
 * then warns, naming each project still locked on SpeleoDB, as a logout does
 * (Q-W61-3-D1).
 *
 * The backend's `speleodb_login` answers `locks_left_held`; the failed hand-back used
 * to be only logged, and the user was never told.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { useSpeleoStore } from '../speleoStore';
import { useUIStore } from '../uiStore';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { invokeIpcMock } from '../../test/factories/ipc';
import type { SpeleoLockLeftHeld } from '../../types/ipc.generated';

vi.mock('../../utils/ipc');

/** Signed in as `old@x`, with a sign-in as `new@y` staged. */
function signedInWithLoginPending() {
  useSpeleoStore.getState().setSessionFromLogin({
    user_email: 'old@x',
    endpoint: 'https://x/',
    keychain_backed: true,
    storage: 'keychain',
    keychain: 'available',
    needs_storage_choice: false,
    replaced_session: false,
    locks_left_held: [],
  });
  useSpeleoStore.getState().setPendingAuth({
    kind: 'login',
    email: 'new@y',
    password: 'pw',
    token: '',
    useToken: false,
  });
}

/** The backend replaced the old session; its hand-back left `left` locked. */
function loginAnswers(left: SpeleoLockLeftHeld[]) {
  invokeIpcMock.mockImplementation(async (cmd: string) => {
    if (cmd === 'speleodb_login')
      return {
        user_email: 'new@y',
        endpoint: 'https://y/',
        keychain_backed: true,
        storage: 'keychain',
        keychain: 'available',
        needs_storage_choice: false,
        replaced_session: true,
        locks_left_held: left,
      };
    // The re-seed after a replaced session: never answers, so only the sign-in's own
    // outcome is asserted.
    if (cmd === 'speleodb_link_status') return new Promise(() => {});
    return [];
  });
}

beforeEach(() => {
  useUIStore.setState({ toasts: [] });
  signedInWithLoginPending();
});

afterEach(() => {
  invokeIpcMock.mockReset();
  useUIStore.setState({ toasts: [] });
});

describe('sign-in whose hand-back could not release a lock (#576 NF-W72-1)', () => {
  it('signs in, then warns naming each project still locked', async () => {
    loginAnswers([{ project_id: '11111111-1111-1111-1111-111111111111', project_name: 'CaveA' }]);

    const result = await useSpeleoStore.getState().applyPendingAuth('https://y/');

    expect(result.ok).toBe(true);
    expect(useSpeleoStore.getState().session?.userEmail).toBe('new@y');
    const toasts = useUIStore.getState().toasts;
    expect(toasts).toHaveLength(1);
    const toast = toasts[0]!;
    expect(toast.key).toBe(TOAST_KEYS.SPELEODB_SIGNIN_LOCKS_HELD);
    expect(toast.kind).toBe('warning');
    expect(toast.message).toBe(
      'Signed in, but "CaveA" is still locked on SpeleoDB by the previous session. Release it on the SpeleoDB website, or sign back in to that account and close it.',
    );
  });

  it('names every project, in one warning', async () => {
    loginAnswers([
      { project_id: 'a', project_name: 'CaveA' },
      { project_id: 'b', project_name: 'CaveB' },
    ]);

    await useSpeleoStore.getState().applyPendingAuth('https://y/');

    const toasts = useUIStore.getState().toasts;
    expect(toasts).toHaveLength(1);
    expect(toasts[0]!.message).toBe(
      'Signed in, but "CaveA", "CaveB" are still locked on SpeleoDB by the previous session. Release them on the SpeleoDB website, or sign back in to that account and close them.',
    );
  });

  it('warns about nothing when the hand-back released every lock', async () => {
    loginAnswers([]);

    const result = await useSpeleoStore.getState().applyPendingAuth('https://y/');

    expect(result.ok).toBe(true);
    expect(useSpeleoStore.getState().session?.userEmail).toBe('new@y');
    expect(useUIStore.getState().toasts).toHaveLength(0);
  });
});
