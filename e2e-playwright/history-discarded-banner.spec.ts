import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage, createProject } from './helpers/app';

/**
 * E2E for the discarded-history banner (#473).
 *
 * A history table the backend cannot read - corrupt, truncated, or crafted past the
 * decompression bounds - is replaced by an empty stack so the project still opens.
 * That is the right trade, but it silently costs the user every undo AND redo entry
 * they had, and docs/notifications.md forbids a state change with no surface. The
 * mock's `history_discarded` answers `__MOCK_SET_HISTORY_DISCARDED__`.
 */

async function seedFlag(page: Page, discarded: boolean, earlierVersion = false): Promise<void> {
  await page.evaluate(
    ([seed, earlier]) => {
      (window as unknown as { __MOCK_SET_HISTORY_DISCARDED__: (d: boolean, e: boolean) => void })
        .__MOCK_SET_HISTORY_DISCARDED__(seed, earlier);
    },
    [discarded, earlierVersion] as const
  );
}

async function seedAndCheck(page: Page, discarded: boolean, earlierVersion = false): Promise<void> {
  await seedFlag(page, discarded, earlierVersion);
  await page.evaluate(async () => {
    await (window as unknown as {
      __E2E_TEST__: { projectStore: { getState: () => { checkHistoryDiscarded: () => Promise<void> } } };
    }).__E2E_TEST__.projectStore.getState().checkHistoryDiscarded();
  });
}

/**
 * Block until the app has actually ASKED the backend about the flag.
 *
 * Required before asserting the banner is ABSENT: `toHaveCount(0)` is satisfied on its
 * first poll, so without this the assertion passes because the post-load effect has not
 * run yet, and would keep passing if the banner were later rendered unconditionally.
 */
async function waitForTheAppToAsk(page: Page): Promise<void> {
  await expect
    .poll(() =>
      page.evaluate(
        () => (window as unknown as { __MOCK_HISTORY_DISCARDED_CALLS__: () => number })
          .__MOCK_HISTORY_DISCARDED_CALLS__()
      )
    )
    .toBeGreaterThan(0);
}

test.describe('Discarded-history banner', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    // Banner-class means "ongoing condition of the OPEN project", so it is gated on
    // a project being open - open one before asserting anything about it.
    await createProject(page, 'History Test');
  });

  test('stays hidden when the history loaded fine', async ({ page }) => {
    await seedAndCheck(page, false);
    await expect(page.getByTestId('history-discarded-banner')).toHaveCount(0);
  });

  test('tells the user their edit history was discarded', async ({ page }) => {
    await seedAndCheck(page, true);

    const banner = page.getByTestId('history-discarded-banner');
    await expect(banner).toBeVisible();
    // Lead with what happened, then the reassurance that matters most: the survey
    // itself is intact. A user who reads "history discarded" and nothing else has
    // every reason to assume they just lost data.
    await expect(banner).toContainText('Edit history could not be read');
    await expect(banner).toContainText('survey data is unaffected');
    // No action button: there is nothing the user can do about it, and
    // docs/notifications.md reserves actions for surfaces that offer a way out. Assert
    // WHICH button, not just how many - a count of 1 is equally satisfied by a "Repair"
    // or "Reload" action replacing Dismiss, which is the policy this is meant to pin.
    await expect(banner.getByRole('button')).toHaveCount(1);
    await expect(banner.getByTestId('history-discarded-dismiss-button')).toBeVisible();
  });

  test('says so when the history was an earlier version of the app', async ({ page }) => {
    // A project last saved before compact undo entries opens with its history dropped
    // whole. Nothing was damaged, so the banner must not say "could not be read".
    await seedAndCheck(page, true, true);

    const banner = page.getByTestId('history-discarded-banner');
    await expect(banner).toBeVisible();
    await expect(banner).toContainText('Undo history from an earlier version of Stalagmight was not kept');
    await expect(banner).toContainText('survey data is unaffected');
    await expect(banner).not.toContainText('could not be read');
  });

  test('dismisses, and a re-check does not resurrect it', async ({ page }) => {
    await seedAndCheck(page, true);
    await expect(page.getByTestId('history-discarded-banner')).toBeVisible();

    await page.getByTestId('history-discarded-dismiss-button').click();
    await expect(page.getByTestId('history-discarded-banner')).toHaveCount(0);

    // The backend flag describes an ongoing condition, so it keeps answering true.
    // Without a sticky dismissal the next check would put the banner straight back
    // up - which is the repeating-notification failure the policy calls out.
    await seedAndCheck(page, true);
    await expect(page.getByTestId('history-discarded-banner')).toHaveCount(0);
  });

  test('a dismissal does not silence a later project that also loses its history', async ({
    page,
  }) => {
    await seedAndCheck(page, true);
    await page.getByTestId('history-discarded-dismiss-button').click();
    await expect(page.getByTestId('history-discarded-banner')).toHaveCount(0);

    // An ACTUAL second project, not a re-toggled flag. The dismissal is reset by the
    // project switch (`createProject` clears both fields in the store), and testing that
    // with a flag toggle instead would exercise only the clear-on-false path in
    // `checkHistoryDiscarded` while leaving the switch path - the one a user hits -
    // uncovered. Close through the store because the `createProject` helper drives the
    // welcome screen, which is only reachable with no project open.
    await page.evaluate(async () => {
      await (window as unknown as {
        __E2E_TEST__: { projectStore: { getState: () => { closeProject: () => Promise<unknown> } } };
      }).__E2E_TEST__.projectStore.getState().closeProject();
    });
    await createProject(page, 'Second Broken Project');

    await expect(page.getByTestId('history-discarded-banner')).toBeVisible();
  });
});

/**
 * The production trigger, exercised as the app actually runs it.
 *
 * The specs above drive `checkHistoryDiscarded()` from the store, which means every one
 * of them would still pass if the post-load effect in `App.tsx` were deleted and the
 * banner never appeared for a real user. These open a project with the flag already
 * seeded and make NO store call, so the effect - and its `file_path && !isLoading`
 * gate - is the only thing that can produce the banner.
 */
test.describe('Discarded-history banner - post-load effect', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('the post-load effect alone raises the banner', async ({ page }) => {
    await seedFlag(page, true);
    await createProject(page, 'Broken History');

    await expect(page.getByTestId('history-discarded-banner')).toBeVisible();
  });

  test('...and leaves it down when the history was readable', async ({ page }) => {
    await seedFlag(page, false);
    await createProject(page, 'Intact History');

    // Wait until the app has ASKED before asserting it did not paint. Without this the
    // assertion is satisfied by the effect not having run yet, and the test could not
    // fail even if the banner were rendered unconditionally.
    await waitForTheAppToAsk(page);
    await expect(page.getByTestId('history-discarded-banner')).toHaveCount(0);
  });
});
