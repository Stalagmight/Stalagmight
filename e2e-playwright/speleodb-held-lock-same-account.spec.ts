import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage } from './helpers/app';
import { setIpcOverrides } from './helpers/ipcOverrides';

/**
 * The held-lock prompt in a second live session of the same account (demo item 1,
 * screenshot 28), end to end on the mocked IPC. Synthetic projects only.
 *
 * SpeleoDB's lock belongs to the account, so signing in on a second computer finds
 * the lock the first one is editing under. The prompt used to offer a bare Release
 * as if a session had crashed. It now says the lock may be Stalagmight open on
 * another computer with this account and what releasing does, keeps Not now as the
 * default, offers "Release anyway" (a crashed session's lock must stay
 * releasable), and says a lock this computer never took is held by this account,
 * not by this computer.
 */

const ME = 'diver@example.com';

type Call = { cmd: string; args: Record<string, unknown> | null };

async function releases(page: Page): Promise<Call[]> {
  const all = await page.evaluate(
    () => ((window as unknown as { __MOCK_EXIT_CALLS__?: Call[] }).__MOCK_EXIT_CALLS__ ?? []),
  );
  return all.filter((c) => c.cmd === 'speleodb_release_lock');
}

/** Signed in as ME; SpeleoDB names ME as the holder of "a", and nothing of "a" is here. */
async function signInWithALockHeldElsewhere(page: Page): Promise<void> {
  await setupMockedPage(page);
  await setIpcOverrides(page, {
    speleodb_session: { logged_in: true, user_email: ME, endpoint: 'https://synthetic.example' },
    speleodb_list_projects: [{
      id: 'a', name: 'Synthetic a', permission: 'READ_AND_WRITE', lock_holder: ME,
      type_tag: 'stalx', latest_commit_sha: null,
    }],
    speleodb_local_project_states: {
      a: {
        has_local_copy: false, has_unpushed_edits: false, last_edit_at: null, is_open: false,
        state_unknown: false, lock_claimed_here: false,
      },
    },
  });
  await page.evaluate(async () => {
    (window as unknown as { __MOCK_EXIT_CALLS__?: unknown[] }).__MOCK_EXIT_CALLS__ = [];
    const store = (window as unknown as {
      __E2E_TEST__: { speleoStore: { getState: () => {
        refreshSession: () => Promise<unknown>;
        scanLocksOnOpen: (opts?: { prompt?: boolean }) => Promise<void>;
      } } };
    }).__E2E_TEST__.speleoStore;
    await store.getState().refreshSession();
    // The sign-in's scan.
    await store.getState().scanLocksOnOpen({ prompt: true });
  });
}

test('a lock another session of the account may hold is kept by default', async ({ page }) => {
  await signInWithALockHeldElsewhere(page);
  const dialog = page.getByTestId('speleodb-held-lock-dialog');
  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText('another computer with this account');
  await expect(dialog).toContainText('lets others change the project under it');
  await expect(page.getByTestId('speleodb-held-lock-elsewhere-a'))
    .toHaveText('Held by this account, not by this computer');
  await expect(page.getByTestId('speleodb-held-lock-release-a')).toHaveText('Release anyway');

  // Not now is the default: Enter keeps the lock.
  await expect(page.getByTestId('speleodb-held-lock-keep')).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(dialog).toBeHidden();
  expect(await releases(page)).toEqual([]);
});

test('Release anyway still hands the lock back', async ({ page }) => {
  await signInWithALockHeldElsewhere(page);
  await page.getByTestId('speleodb-held-lock-release-a').click();
  await expect(page.getByTestId('speleodb-held-lock-dialog')).toBeHidden();
  expect((await releases(page)).map((c) => c.args)).toEqual([{ projectId: 'a' }]);
});
