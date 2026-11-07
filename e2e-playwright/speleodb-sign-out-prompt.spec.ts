import { test, expect, type Page, type TestInfo } from './fixtures/coverage';
import { openSettingsFromMenu, setupMockedPage } from './helpers/app';
import { setIpcOverrides } from './helpers/ipcOverrides';
import { getIpcCalls, recordIpcCalls } from './helpers/ipcRecorder';
import type { SpeleoLockLeftHeld } from '../src/types/ipc/SpeleoLockLeftHeld';
import type { SpeleoLoginResult } from '../src/types/ipc/SpeleoLoginResult';
import type { SpeleoSessionStatus } from '../src/types/ipc/SpeleoSessionStatus';
import type { SpeleoSignOutLocks } from '../src/types/ipc/SpeleoSignOutLocks';

/**
 * [#576 SIGNOUT-PROMPT-D1] Signing out while this session holds SpeleoDB write locks
 * asks first: release them, keep them held, or cancel. Driven through Settings, as
 * the user does it: Log out + Save, and a sign-in as another account + Save (the
 * account panel shows the form while signed in once a sign-in is staged). The mock
 * answers which locks are held through an override; the backend half (what each
 * answer releases) is pinned in Rust (sdb_silent_release_claim_tests,
 * sdb_login_handback_tests).
 */

const OLD = 'old@example.invalid';
const NEW = 'new@example.invalid';
const ENDPOINT = 'https://speleodb.invalid/';
const CAVE_A: SpeleoLockLeftHeld = {
  project_id: '11111111-1111-1111-1111-111111111111',
  project_name: 'CaveA',
};
const CAVE_B: SpeleoLockLeftHeld = {
  project_id: '22222222-2222-2222-2222-222222222222',
  project_name: 'CaveB',
};
const CAVE_C: SpeleoLockLeftHeld = {
  project_id: '33333333-3333-3333-3333-333333333333',
  project_name: 'CaveC',
};
const CAVE_D: SpeleoLockLeftHeld = {
  project_id: '44444444-4444-4444-4444-444444444444',
  project_name: 'CaveD',
};

const SIGNED_IN: SpeleoSessionStatus = {
  logged_in: true,
  user_email: OLD,
  endpoint: ENDPOINT,
  keychain_backed: true,
  storage: 'keychain',
  keychain: 'available',
};

function loginReply(leftHeld: SpeleoLockLeftHeld[]): SpeleoLoginResult {
  return {
    user_email: NEW,
    endpoint: ENDPOINT,
    keychain_backed: true,
    storage: 'keychain',
    keychain: 'available',
    needs_storage_choice: false,
    replaced_session: true,
    locks_left_held: leftHeld,
  };
}

type Kind = 'logout' | 'switch';

interface Backend {
  /** The locks the sign-out would hand back (what the prompt names). */
  held: SpeleoLockLeftHeld[];
  /** What the release could not release. */
  leftHeld?: SpeleoLockLeftHeld[];
  /** [#576 SIGNOUT-PROMPT-D2] The locks the sign-out keeps for un-pushed edits. */
  unpushed?: SpeleoLockLeftHeld[];
}

const NONE: SpeleoSignOutLocks = { releasable: [], unpushed: [] };

/** The command each path signs out with. */
const ACT: Record<Kind, 'speleodb_logout' | 'speleodb_login'> = {
  logout: 'speleodb_logout',
  switch: 'speleodb_login',
};

/**
 * Signed in as OLD, Settings open on the SpeleoDB account panel with the sign-out
 * staged (Log out, or a sign-in as NEW), then Save.
 */
async function stageAndSave(page: Page, kind: Kind, backend: Backend): Promise<void> {
  const leftHeld = backend.leftHeld ?? [];
  const locks: SpeleoSignOutLocks = { releasable: backend.held, unpushed: backend.unpushed ?? [] };
  await setIpcOverrides(page, {
    speleodb_session: SIGNED_IN,
    speleodb_sign_out_locks: kind === 'logout' ? locks : NONE,
    speleodb_sign_in_hand_back_locks: kind === 'switch' ? locks : NONE,
    speleodb_logout: { locks_left_held: leftHeld },
    speleodb_login: loginReply(leftHeld),
  });
  await page.evaluate(
    ({ email, endpoint }) => {
      const store = (
        window as unknown as {
          __E2E_TEST__: { speleoStore: { getState: () => { setSessionFromLogin: (r: unknown) => void } } };
        }
      ).__E2E_TEST__.speleoStore;
      store.getState().setSessionFromLogin({
        user_email: email,
        endpoint,
        keychain_backed: true,
        storage: 'keychain',
        keychain: 'available',
        needs_storage_choice: false,
        replaced_session: false,
        locks_left_held: [],
      });
    },
    { email: OLD, endpoint: ENDPOINT },
  );
  await recordIpcCalls(page, ['speleodb_logout', 'speleodb_login']);
  await openSettingsFromMenu(page);
  await page.getByTestId('settings-tab-speleodb').click();
  if (kind === 'logout') {
    await page.getByTestId('speleodb-settings-logout').click();
    await expect(page.getByTestId('speleodb-account-staged-logout')).toBeVisible();
  } else {
    // A sign-in staged over the live session: the panel shows the form with it.
    await page.evaluate((email) => {
      const store = (
        window as unknown as {
          __E2E_TEST__: { speleoStore: { getState: () => { setPendingAuth: (p: unknown) => void } } };
        }
      ).__E2E_TEST__.speleoStore;
      store.getState().setPendingAuth({ kind: 'login', email, password: 'pw', token: '', useToken: false });
    }, NEW);
    await expect(page.getByTestId('speleodb-settings-email')).toHaveValue(NEW);
  }
  await page.getByTestId('settings-save-button').click();
}

/** The screen the Save leaves, prompt or not. */
async function shoot(page: Page, testInfo: TestInfo, name: string): Promise<void> {
  await page.screenshot({ path: testInfo.outputPath(name) });
}

const sessionEmail = (page: Page) =>
  page.evaluate(
    () =>
      (
        window as unknown as {
          __E2E_TEST__: { speleoStore: { getState: () => { session: { userEmail: string | null } | null } } };
        }
      ).__E2E_TEST__.speleoStore.getState().session?.userEmail ?? null,
  );

const PROMPT = 'speleodb-sign-out-dialog';

for (const kind of ['logout', 'switch'] as const) {
  const signedOutAs = kind === 'logout' ? null : NEW;

  test.describe(`Sign-out prompt: ${kind}`, () => {
    test.beforeEach(async ({ page }) => {
      await setupMockedPage(page);
    });

    test('asks first, naming the locks this session holds', async ({ page }, testInfo) => {
      await stageAndSave(page, kind, { held: [CAVE_A, CAVE_B] });
      try {
        await expect(page.getByTestId(PROMPT)).toBeVisible();
        await expect(page.getByTestId('speleodb-sign-out-message')).toHaveText(
          'You hold the locks on CaveA, CaveB. Release them before signing out?',
        );
        await expect(page.getByTestId('speleodb-sign-out-release')).toHaveText('Release and sign out');
        await expect(page.getByTestId('speleodb-sign-out-keep')).toHaveText('Keep locks and sign out');
        await expect(page.getByTestId('speleodb-sign-out-cancel')).toBeVisible();
        expect(await getIpcCalls(page, ACT[kind])).toEqual([]);
      } finally {
        await shoot(page, testInfo, `sign-out-prompt-${kind}.png`);
      }
    });

    test('asks nothing when no lock is held', async ({ page }) => {
      await stageAndSave(page, kind, { held: [] });
      await expect(page.getByTestId('settings-dialog')).toBeHidden();
      await expect(page.getByTestId(PROMPT)).toBeHidden();
      const calls = await getIpcCalls(page, ACT[kind]);
      expect(calls).toHaveLength(1);
      expect(calls[0]!.args).toMatchObject({ releaseLocks: true });
    });

    test('Release and sign out releases, then signs out', async ({ page }) => {
      await stageAndSave(page, kind, { held: [CAVE_A] });
      await page.getByTestId('speleodb-sign-out-release').click();
      await expect(page.getByTestId('settings-dialog')).toBeHidden();
      await expect(page.getByTestId(PROMPT)).toBeHidden();
      const calls = await getIpcCalls(page, ACT[kind]);
      expect(calls).toHaveLength(1);
      expect(calls[0]!.args).toMatchObject({ releaseLocks: true });
      expect(await sessionEmail(page)).toBe(signedOutAs);
    });

    test('Keep locks and sign out signs out without releasing', async ({ page }) => {
      await stageAndSave(page, kind, { held: [CAVE_A] });
      await expect(page.getByTestId('speleodb-sign-out-keep')).toHaveText('Keep lock and sign out');
      await page.getByTestId('speleodb-sign-out-keep').click();
      await expect(page.getByTestId('settings-dialog')).toBeHidden();
      const calls = await getIpcCalls(page, ACT[kind]);
      expect(calls).toHaveLength(1);
      expect(calls[0]!.args).toMatchObject({ releaseLocks: false });
      expect(await sessionEmail(page)).toBe(signedOutAs);
      const warned = await page.evaluate(
        () =>
          (
            window as unknown as {
              __E2E_TEST__: { uiStore: { getState: () => { toasts: { message: string }[] } } };
            }
          ).__E2E_TEST__.uiStore
            .getState()
            .toasts.filter((t) => t.message.includes('still locked')).length,
      );
      expect(warned).toBe(0);
    });

    test('Cancel does nothing, and Settings keeps what it staged', async ({ page }) => {
      await stageAndSave(page, kind, { held: [CAVE_A] });
      await page.getByTestId('speleodb-sign-out-cancel').click();
      await expect(page.getByTestId(PROMPT)).toBeHidden();
      await expect(page.getByTestId('settings-dialog')).toBeVisible();
      await expect(page.getByTestId('speleodb-account-settings-error')).toBeHidden();
      if (kind === 'logout') {
        await expect(page.getByTestId('speleodb-account-staged-logout')).toBeVisible();
      } else {
        await expect(page.getByTestId('speleodb-settings-email')).toHaveValue(NEW);
      }
      expect(await getIpcCalls(page, ACT[kind])).toEqual([]);
      expect(await sessionEmail(page)).toBe(OLD);
    });

    test('Release with a release SpeleoDB refuses still signs out and warns', async ({ page }) => {
      await stageAndSave(page, kind, { held: [CAVE_A], leftHeld: [CAVE_A] });
      await page.getByTestId('speleodb-sign-out-release').click();
      await expect(page.getByTestId('settings-dialog')).toBeHidden();
      await expect(
        page.getByTestId('toast').filter({ hasText: '"CaveA" is still locked on SpeleoDB' }),
      ).toBeVisible();
      expect(await sessionEmail(page)).toBe(signedOutAs);
    });

    // [#576 SIGNOUT-PROMPT-D2] A lock kept for un-pushed edits is named, and alone it
    // leaves only Sign out and Cancel.
    test('mixed: names the locks to release and the ones kept for un-pushed edits', async ({
      page,
    }, testInfo) => {
      await stageAndSave(page, kind, { held: [CAVE_A], unpushed: [CAVE_C] });
      try {
        await expect(page.getByTestId(PROMPT)).toBeVisible();
        await expect(page.getByTestId('speleodb-sign-out-message')).toHaveText(
          'You hold the lock on CaveA. Release it before signing out?',
        );
        await expect(page.getByTestId('speleodb-sign-out-unpushed')).toHaveText(
          'CaveC has edits not pushed to SpeleoDB yet; its lock stays held until you push or discard them.',
        );
        await expect(page.getByTestId('speleodb-sign-out-release')).toBeVisible();
        await expect(page.getByTestId('speleodb-sign-out-keep')).toBeVisible();
        await expect(page.getByTestId('speleodb-sign-out-ok')).toBeHidden();
      } finally {
        await shoot(page, testInfo, `sign-out-prompt-mixed-${kind}.png`);
      }
      await page.getByTestId('speleodb-sign-out-release').click();
      await expect(page.getByTestId('settings-dialog')).toBeHidden();
      const calls = await getIpcCalls(page, ACT[kind]);
      expect(calls).toHaveLength(1);
      expect(calls[0]!.args).toMatchObject({ releaseLocks: true });
    });

    test('un-pushed only: says the locks stay held, with Sign out and Cancel only', async ({
      page,
    }, testInfo) => {
      await stageAndSave(page, kind, { held: [], unpushed: [CAVE_C, CAVE_D] });
      try {
        await expect(page.getByTestId(PROMPT)).toBeVisible();
        await expect(page.getByTestId('speleodb-sign-out-unpushed')).toHaveText(
          'CaveC, CaveD have edits not pushed to SpeleoDB yet; their locks stay held until you push or discard them.',
        );
        await expect(page.getByTestId('speleodb-sign-out-message')).toBeHidden();
        await expect(page.getByTestId('speleodb-sign-out-release')).toBeHidden();
        await expect(page.getByTestId('speleodb-sign-out-keep')).toBeHidden();
        await expect(page.getByTestId('speleodb-sign-out-cancel')).toBeVisible();
        await expect(page.getByTestId('speleodb-sign-out-ok')).toHaveText('Sign out');
        expect(await getIpcCalls(page, ACT[kind])).toEqual([]);
      } finally {
        await shoot(page, testInfo, `sign-out-prompt-unpushed-${kind}.png`);
      }
      await page.getByTestId('speleodb-sign-out-ok').click();
      await expect(page.getByTestId('settings-dialog')).toBeHidden();
      const calls = await getIpcCalls(page, ACT[kind]);
      expect(calls).toHaveLength(1);
      expect(calls[0]!.args).toMatchObject({ releaseLocks: true });
      expect(await sessionEmail(page)).toBe(signedOutAs);
    });
  });
}
