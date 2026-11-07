/**
 * [#576 SIGNOUT-PROMPT-D1] Signing out while this session holds SpeleoDB write locks
 * asks first: "You hold the lock on CaveA. Release it before signing out?"
 *
 * - Release and sign out: the sign-out releases (a failed release still ends in the
 *   "still locked" warning, Q-W61-3-D1 / NF-W72-1).
 * - Keep locks and sign out: the sign-out sends no release.
 * - Cancel: nothing happens; the staged change stays staged.
 *
 * Both paths: a logout, and a sign-in as another account, which signs the live
 * session out first and gets the same prompt before the session is replaced. With
 * no lock held there is no prompt.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { useSpeleoStore, type SignOutChoice } from '../speleoStore';
import { useUIStore } from '../uiStore';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { invokeIpcMock } from '../../test/factories/ipc';
import type { SpeleoLockLeftHeld } from '../../types/ipc.generated';

vi.mock('../../utils/ipc');

const CAVE_A: SpeleoLockLeftHeld = {
  project_id: '11111111-1111-1111-1111-111111111111',
  project_name: 'CaveA',
};
const CAVE_B: SpeleoLockLeftHeld = { project_id: 'b', project_name: 'CaveB' };

interface Path {
  kind: 'logout' | 'switch';
  /** The command that asks which locks the sign-out would hand back. */
  ask: string;
  /** The command that signs out (and, for a switch, signs in). */
  act: string;
  stage: () => void;
  /** Whether the sign-out happened, read off the session. */
  signedOut: () => boolean;
  /** The warning a release that failed leaves, and its key. */
  warning: string;
  warningKey: string;
}

const signedInAsOld = () =>
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

const PATHS: Path[] = [
  {
    kind: 'logout',
    ask: 'speleodb_sign_out_locks',
    act: 'speleodb_logout',
    stage: () => useSpeleoStore.getState().setPendingAuth({ kind: 'logout' }),
    signedOut: () => useSpeleoStore.getState().session === null,
    warning:
      'Signed out, but "CaveA" is still locked on SpeleoDB. Release it on the SpeleoDB website, or sign in and close it.',
    warningKey: TOAST_KEYS.SPELEODB_LOGOUT_LOCKS_HELD,
  },
  {
    kind: 'switch',
    ask: 'speleodb_sign_in_hand_back_locks',
    act: 'speleodb_login',
    stage: () =>
      useSpeleoStore
        .getState()
        .setPendingAuth({ kind: 'login', email: 'new@y', password: 'pw', token: '', useToken: false }),
    signedOut: () => useSpeleoStore.getState().session?.userEmail === 'new@y',
    warning:
      'Signed in, but "CaveA" is still locked on SpeleoDB by the previous session. Release it on the SpeleoDB website, or sign back in to that account and close it.',
    warningKey: TOAST_KEYS.SPELEODB_SIGNIN_LOCKS_HELD,
  },
];

/** The backend: `held` are the locks the sign-out would hand back; `leftHeld` what
 *  its release could not release. */
function backend(path: Path, held: SpeleoLockLeftHeld[], leftHeld: SpeleoLockLeftHeld[] = []) {
  invokeIpcMock.mockImplementation(async (cmd: string) => {
    if (cmd === path.ask) return held;
    if (cmd === 'speleodb_logout') return { locks_left_held: leftHeld };
    if (cmd === 'speleodb_login')
      return {
        user_email: 'new@y',
        endpoint: 'https://x/',
        keychain_backed: true,
        storage: 'keychain',
        keychain: 'available',
        needs_storage_choice: false,
        replaced_session: held.length > 0,
        locks_left_held: leftHeld,
      };
    // The re-seed after a replaced session: never answers.
    if (cmd === 'speleodb_link_status') return new Promise(() => {});
    return [];
  });
}

/** The calls made to `cmd`, as their argument objects. */
const callsTo = (cmd: string) =>
  invokeIpcMock.mock.calls.filter(([name]) => name === cmd).map(([, args]) => args as Record<string, unknown>);

/** Apply the staged change, answer the prompt with `choice`, and return the result. */
async function applyAnswering(choice: SignOutChoice) {
  const applied = useSpeleoStore.getState().applyPendingAuth('https://x/');
  await vi.waitFor(() => expect(useSpeleoStore.getState().signOutPrompt).not.toBeNull());
  useSpeleoStore.getState().answerSignOutPrompt(choice);
  return applied;
}

beforeEach(() => {
  useUIStore.setState({ toasts: [] });
  useSpeleoStore.setState({ signOutPrompt: null });
  signedInAsOld();
});

afterEach(() => {
  invokeIpcMock.mockReset();
  useUIStore.setState({ toasts: [] });
  useSpeleoStore.setState({ session: null, pendingAuth: null, signOutPrompt: null });
});

describe.each(PATHS)('sign-out prompt: $kind (#576 SIGNOUT-PROMPT-D1)', (path) => {
  beforeEach(() => path.stage());

  it('asks nothing when no lock is held, and the sign-out releases as before', async () => {
    backend(path, []);
    const raised: unknown[] = [];
    const unsubscribe = useSpeleoStore.subscribe((s) => {
      if (s.signOutPrompt) raised.push(s.signOutPrompt);
    });

    const result = await useSpeleoStore.getState().applyPendingAuth('https://x/');
    unsubscribe();

    expect(result.ok).toBe(true);
    expect(raised).toEqual([]);
    expect(callsTo(path.act)).toHaveLength(1);
    expect(callsTo(path.act)[0]).toMatchObject({ releaseLocks: true });
  });

  it('asks first when locks are held, naming them, and signs nothing out until answered', async () => {
    backend(path, [CAVE_A, CAVE_B]);

    const applied = useSpeleoStore.getState().applyPendingAuth('https://x/');
    await vi.waitFor(() => expect(useSpeleoStore.getState().signOutPrompt).not.toBeNull());

    expect(useSpeleoStore.getState().signOutPrompt).toEqual({ kind: path.kind, locks: [CAVE_A, CAVE_B] });
    expect(callsTo(path.act)).toHaveLength(0);
    expect(useSpeleoStore.getState().session?.userEmail).toBe('old@x');

    useSpeleoStore.getState().answerSignOutPrompt('cancel');
    await applied;
  });

  it('Release and sign out: releases, then signs out', async () => {
    backend(path, [CAVE_A]);

    const result = await applyAnswering('release');

    expect(result.ok).toBe(true);
    expect(useSpeleoStore.getState().signOutPrompt).toBeNull();
    expect(callsTo(path.act)).toHaveLength(1);
    expect(callsTo(path.act)[0]).toMatchObject({ releaseLocks: true });
    expect(path.signedOut()).toBe(true);
    expect(useUIStore.getState().toasts).toHaveLength(0);
  });

  it('Keep locks and sign out: signs out without releasing', async () => {
    backend(path, [CAVE_A]);

    const result = await applyAnswering('keep');

    expect(result.ok).toBe(true);
    expect(callsTo(path.act)).toHaveLength(1);
    expect(callsTo(path.act)[0]).toMatchObject({ releaseLocks: false });
    expect(path.signedOut()).toBe(true);
    expect(useUIStore.getState().toasts).toHaveLength(0);
  });

  it('Cancel: nothing happens, and the change stays staged', async () => {
    backend(path, [CAVE_A]);
    const staged = useSpeleoStore.getState().pendingAuth;

    const result = await applyAnswering('cancel');

    expect(result).toEqual({ ok: false, cancelled: true });
    expect(callsTo('speleodb_logout')).toHaveLength(0);
    expect(callsTo('speleodb_login')).toHaveLength(0);
    expect(useSpeleoStore.getState().session?.userEmail).toBe('old@x');
    expect(useSpeleoStore.getState().pendingAuth).toEqual(staged);
    expect(useUIStore.getState().toasts).toHaveLength(0);
  });

  it('Release with a release SpeleoDB refuses: signs out and warns, naming the project', async () => {
    backend(path, [CAVE_A], [CAVE_A]);

    const result = await applyAnswering('release');

    expect(result.ok).toBe(true);
    expect(path.signedOut()).toBe(true);
    const toasts = useUIStore.getState().toasts;
    expect(toasts).toHaveLength(1);
    expect(toasts[0]!.key).toBe(path.warningKey);
    expect(toasts[0]!.kind).toBe('warning');
    expect(toasts[0]!.message).toBe(path.warning);
  });
});
