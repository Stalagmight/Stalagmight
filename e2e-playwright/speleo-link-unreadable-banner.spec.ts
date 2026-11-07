import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage, waitForMapIdle } from './helpers/app';

/**
 * E2E for a project whose SpeleoDB link row cannot be read (#576 B35(b) review).
 *
 * The backend opens such a file as a local project but refuses every save over it -
 * the rebuilt file would lose the link row - and reports the open's verdict on the
 * binary envelope (`speleo_link_unreadable`). The mock reports it when seeded with
 * __MOCK_SET_PROJECT_SPELEO_LINK_UNREADABLE__ and refuses in-place saves for that
 * project, as the backend does. This pins the UI half: the banner says why on open,
 * before the first save fails, and a Save As to another file ends it.
 */

async function seedLinkUnreadable(page: Page, unreadable: boolean): Promise<void> {
  await page.evaluate((u) => {
    (window as unknown as { __MOCK_SET_PROJECT_SPELEO_LINK_UNREADABLE__: (u: boolean) => void })
      .__MOCK_SET_PROJECT_SPELEO_LINK_UNREADABLE__(u);
  }, unreadable);
}

type E2E = {
  __E2E_TEST__: {
    projectStore: {
      getState: () => {
        loadProject: (p: string) => Promise<unknown>;
        saveProject: (p?: string) => Promise<unknown>;
      };
    };
  };
};

/** Open a project through the real load path (load_project_full_binary). */
async function loadProject(page: Page, path: string): Promise<void> {
  await page.evaluate(async (p) => {
    await (window as unknown as E2E).__E2E_TEST__.projectStore.getState().loadProject(p);
  }, path);
  await expect(page.getByTestId('single-system-view')).toBeVisible();
  await waitForMapIdle(page);
}

test.describe('A project whose SpeleoDB link cannot be read', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('says so when it opens, and a Save As elsewhere ends it', async ({ page }) => {
    await seedLinkUnreadable(page, true);
    await loadProject(page, '/mock/CaveA.stalx');

    const banner = page.getByTestId('speleo-link-unreadable-banner');
    await expect(banner).toBeVisible();
    await expect(banner).toContainText("SpeleoDB link could not be read");
    await expect(banner).toContainText('Use Save As');
    await expect(banner.getByRole('button')).toHaveCount(0);

    await seedLinkUnreadable(page, false);
    await page.evaluate(async () => {
      await (window as unknown as E2E).__E2E_TEST__.projectStore.getState().saveProject('/mock/CaveA-rescue.stalx');
    });
    await expect(banner).toHaveCount(0);
  });

  test('an ordinary project has no banner', async ({ page }) => {
    await seedLinkUnreadable(page, false);
    await loadProject(page, '/mock/CaveB.stalx');
    await expect(page.getByTestId('speleo-link-unreadable-banner')).toHaveCount(0);
  });
});
