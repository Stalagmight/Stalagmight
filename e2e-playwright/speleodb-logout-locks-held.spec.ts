import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage } from './helpers/app';
import { setIpcOverrides } from './helpers/ipcOverrides';

/**
 * [#576 Q-W61-3-D1] A sign-out whose lock release SpeleoDB refused.
 *
 * The backend signs out anyway and answers which projects are still locked
 * (`speleodb_logout` -> `{ locks_left_held }`); the frontend then warns, naming
 * each one and how to release it. The mock answers the failed release through an
 * override; the backend half is pinned in Rust (sdb_silent_release_claim_tests).
 */

type E2E = {
  __E2E_TEST__: {
    speleoStore: {
      getState: () => {
        setSessionFromLogin: (r: unknown) => void;
        setPendingAuth: (p: { kind: 'logout' }) => void;
        applyPendingAuth: (endpoint?: string) => Promise<{ ok: boolean }>;
        session: unknown;
      };
    };
  };
};

/** Sign in (store only), stage the logout, and apply it as Settings' Save does. */
async function signOut(page: Page): Promise<{ ok: boolean; signedOut: boolean }> {
  return page.evaluate(async () => {
    const store = (window as unknown as E2E).__E2E_TEST__.speleoStore;
    store.getState().setSessionFromLogin({
      user_email: 'u@example.invalid',
      endpoint: 'https://speleodb.invalid/',
      keychain_backed: true,
      storage: 'keychain',
      keychain: 'available',
      needs_storage_choice: false,
      replaced_session: false,
    });
    store.getState().setPendingAuth({ kind: 'logout' });
    const result = await store.getState().applyPendingAuth('https://speleodb.invalid/');
    return { ok: result.ok, signedOut: store.getState().session === null };
  });
}

test.describe('Sign-out that could not release a write lock', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('signs out and warns, naming the project still locked', async ({ page }, testInfo) => {
    await setIpcOverrides(page, {
      speleodb_logout: {
        locks_left_held: [
          { project_id: '11111111-1111-1111-1111-111111111111', project_name: 'CaveA' },
        ],
      },
    });

    const outcome = await signOut(page);
    expect(outcome).toEqual({ ok: true, signedOut: true });

    const toast = page.getByTestId('toast').filter({ hasText: 'still locked on SpeleoDB' });
    try {
      await expect(toast).toHaveText(
        /Signed out, but "CaveA" is still locked on SpeleoDB\. Release it on the SpeleoDB website, or sign in and close it\./,
      );
    } finally {
      // The screen the sign-out leaves, warning or not.
      await page.screenshot({ path: testInfo.outputPath('logout-locks-held.png') });
    }
  });

  test('a sign-out whose releases all landed warns about nothing', async ({ page }) => {
    const outcome = await signOut(page);
    expect(outcome).toEqual({ ok: true, signedOut: true });
    // The store, not the DOM: the toast is queued before the sign-out answers, so an
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
