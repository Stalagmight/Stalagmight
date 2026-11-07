import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage } from './helpers/app';
import { setIpcOverrides } from './helpers/ipcOverrides';
import type { SpeleoLockLeftHeld } from '../src/types/ipc/SpeleoLockLeftHeld';
import type { SpeleoLoginResult } from '../src/types/ipc/SpeleoLoginResult';

/**
 * [#576 NF-W72-1] A sign-in that replaced a live session whose lock release
 * SpeleoDB refused.
 *
 * The backend hands the old session's lock back first (FZ-SDB-3-D1), signs in
 * anyway, and answers which projects are still locked (`speleodb_login` ->
 * `locks_left_held`); the frontend then warns, naming each one and how to release
 * it, as after a logout (Q-W61-3-D1). The mock answers the failed hand-back through
 * an override; the backend half is pinned in Rust (sdb_login_handback_tests).
 */

type E2E = {
  __E2E_TEST__: {
    speleoStore: {
      getState: () => {
        setSessionFromLogin: (r: unknown) => void;
        setPendingAuth: (p: unknown) => void;
        applyPendingAuth: (endpoint?: string) => Promise<{ ok: boolean }>;
        session: { userEmail: string } | null;
      };
    };
  };
};

/** Sign in (store only) as old@, stage a sign-in as new@, and apply it as Settings' Save does. */
async function signInOverSession(page: Page): Promise<{ ok: boolean; signedInAs: string | null }> {
  return page.evaluate(async () => {
    const store = (window as unknown as E2E).__E2E_TEST__.speleoStore;
    store.getState().setSessionFromLogin({
      user_email: 'old@example.invalid',
      endpoint: 'https://speleodb.invalid/',
      keychain_backed: true,
      storage: 'keychain',
      keychain: 'available',
      needs_storage_choice: false,
      replaced_session: false,
      locks_left_held: [],
    });
    store.getState().setPendingAuth({
      kind: 'login',
      email: 'new@example.invalid',
      password: 'pw',
      token: '',
      useToken: false,
    });
    const result = await store.getState().applyPendingAuth('https://speleodb.invalid/');
    return { ok: result.ok, signedInAs: store.getState().session?.userEmail ?? null };
  });
}

/** The backend's answer: the session was replaced; `left` is still locked. */
function loginReply(left: SpeleoLockLeftHeld[]): SpeleoLoginResult {
  return {
    user_email: 'new@example.invalid',
    endpoint: 'https://speleodb.invalid/',
    keychain_backed: true,
    storage: 'keychain',
    keychain: 'available',
    needs_storage_choice: false,
    replaced_session: true,
    locks_left_held: left,
  };
}

test.describe('Sign-in whose hand-back could not release a write lock', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('signs in and warns, naming the project still locked', async ({ page }, testInfo) => {
    await setIpcOverrides(page, {
      speleodb_login: loginReply([
        { project_id: '11111111-1111-1111-1111-111111111111', project_name: 'CaveA' },
      ]),
    });

    const outcome = await signInOverSession(page);
    expect(outcome).toEqual({ ok: true, signedInAs: 'new@example.invalid' });

    const toast = page.getByTestId('toast').filter({ hasText: 'still locked on SpeleoDB' });
    try {
      await expect(toast).toHaveText(
        /Signed in, but "CaveA" is still locked on SpeleoDB by the previous session\. Release it on the SpeleoDB website, or sign back in to that account and close it\./,
      );
    } finally {
      // The screen the sign-in leaves, warning or not.
      await page.screenshot({ path: testInfo.outputPath('signin-locks-held.png') });
    }
  });

  test('a sign-in whose hand-back released every lock warns about nothing', async ({ page }) => {
    await setIpcOverrides(page, { speleodb_login: loginReply([]) });

    const outcome = await signInOverSession(page);
    expect(outcome).toEqual({ ok: true, signedInAs: 'new@example.invalid' });
    // The store, not the DOM: the toast is queued before the sign-in answers, so an
    // empty queue here cannot be a render that has not happened yet.
    const queued = await page.evaluate(
      () =>
        (
          window as unknown as {
            __E2E_TEST__: { uiStore: { getState: () => { toasts: { message: string }[] } } };
          }
        ).__E2E_TEST__.uiStore
          .getState()
          .toasts.filter((t) => t.message.includes('still locked')).length,
    );
    expect(queued).toBe(0);
  });
});
