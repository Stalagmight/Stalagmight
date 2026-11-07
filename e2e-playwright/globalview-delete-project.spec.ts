import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage } from './helpers/app';
import { systemCardTestId } from './helpers/testIds';
import { recordIpcCalls, waitForIpcCall } from './helpers/ipcRecorder';

/**
 * E2E for the destructive "Delete project" action (#118) - distinct from the
 * non-destructive "Remove from recent". Covers the GlobalView context-menu item,
 * its single-selection gating, and the confirm-dialog flow.
 *
 * The mocked `delete_project` (mocks/tauri-ipc.ts) drops the path from the
 * mocked recents list, as the backend does, so the confirm test can see the
 * card go; the file-system side is covered by the store (vitest) and Rust tests.
 */

const ALPHA = '/projects/cave-alpha.stalx';
const BETA = '/projects/cave-beta.stalx';

async function switchToListTab(page: Page): Promise<void> {
  const listTab = page.getByTestId('globalview-view-toggle-list');
  await expect(listTab).toBeVisible();
  await listTab.click();
  await expect(page.getByTestId(systemCardTestId(ALPHA))).toBeVisible();
}

test.describe('GlobalView - Delete project (destructive)', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await switchToListTab(page);
  });

  test('context menu offers Delete project as a distinct item from Remove from recent', async ({ page }) => {
    await page.getByTestId(systemCardTestId(ALPHA)).click({ button: 'right' });
    await expect(page.getByTestId('project-context-menu')).toBeVisible();
    await expect(page.getByTestId('context-menu-remove')).toBeVisible();
    await expect(page.getByTestId('context-menu-delete')).toBeVisible();
  });

  test('Delete project opens a confirm dialog naming the project, and confirming deletes @smoke', async ({ page }) => {
    await page.getByTestId(systemCardTestId(ALPHA)).click({ button: 'right' });
    await page.getByTestId('context-menu-delete').click();

    const dialog = page.getByTestId('delete-project-dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText('Cave System Alpha');
    await expect(dialog).toContainText('/projects/cave-alpha.stalx');
    await expect(dialog).toContainText('It cannot be undone');

    await recordIpcCalls(page, ['delete_project']);
    await page.getByTestId('delete-project-confirm').click();
    // A successful delete closes the dialog...
    await expect(dialog).toBeHidden();
    // ...after sending exactly one delete, for ALPHA's file...
    const calls = await waitForIpcCall(page, 'delete_project');
    expect(calls.map((c) => c.args.path)).toEqual([ALPHA]);
    // ...and ALPHA leaves the list (the recents refresh after the delete),
    // while the other loadable projects are unaffected.
    await expect(page.getByTestId(systemCardTestId(ALPHA))).toHaveCount(0);
    await expect(page.getByTestId(systemCardTestId(BETA))).toBeVisible();
  });

  test('Cancel closes the dialog without deleting', async ({ page }) => {
    await page.getByTestId(systemCardTestId(ALPHA)).click({ button: 'right' });
    await page.getByTestId('context-menu-delete').click();

    const dialog = page.getByTestId('delete-project-dialog');
    await expect(dialog).toBeVisible();
    await dialog.getByRole('button', { name: 'Cancel' }).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByTestId(systemCardTestId(ALPHA))).toBeVisible();
  });

  test('Delete project is hidden when multiple projects are selected', async ({ page }) => {
    const cardAlpha = page.getByTestId(systemCardTestId(ALPHA));
    const cardBeta = page.getByTestId(systemCardTestId(BETA));
    await cardAlpha.click();
    await cardBeta.click({ modifiers: ['ControlOrMeta'] });
    await cardAlpha.click({ button: 'right' });

    await expect(page.getByTestId('project-context-menu')).toBeVisible();
    // Multi-select keeps batch Remove + Merge, but the single-target Delete is gated out.
    await expect(page.getByTestId('context-menu-merge')).toBeVisible();
    await expect(page.getByTestId('context-menu-delete')).toHaveCount(0);
  });
});
