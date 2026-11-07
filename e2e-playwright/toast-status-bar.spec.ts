import { test, expect, type Locator, type Page } from './fixtures/coverage';
import { setupMockedPage, createProject } from './helpers/app';
import type { E2EWindow } from './helpers/e2eWindow';

/**
 * A toast sits above the status bar, never over it (#576 W70).
 *
 * The toast stack is anchored 24 px (`bottom-6`) above the window's bottom edge, and
 * both the project list and the project view have a status bar along that edge, so
 * a bottom-left toast covered the status bar's text ("Sections: 0 | Shots: 0 ...").
 * Each status bar now registers its height with the stack, as the DMP wizard footer
 * does, and the stack keeps its 24 px gap above the bar.
 */

const GAP_PX = 24;

async function showToast(page: Page, message: string): Promise<void> {
  await page.evaluate((m) => {
    (window as E2EWindow).__E2E_TEST__.uiStore.getState().showToast(m, 'project-changed', 'warning');
  }, message);
}

/** The settled toast's bottom edge sits the stack's gap above the status bar's top edge. */
async function expectAboveStatusBar(toast: Locator, statusBar: Locator): Promise<void> {
  await expect(toast).toBeVisible();
  // The toast rises into its slot on entry, so poll until it has arrived.
  await expect(async () => {
    const t = await toast.boundingBox();
    const s = await statusBar.boundingBox();
    expect(t, 'toast box').not.toBeNull();
    expect(s, 'status bar box').not.toBeNull();
    if (!t || !s) return;
    const gap = s.y - (t.y + t.height);
    expect(gap, `gap between the toast's bottom (${t.y + t.height}) and the status bar's top (${s.y})`).toBeGreaterThanOrEqual(GAP_PX - 1);
    expect(gap, 'the toast keeps the usual gap, not more').toBeLessThanOrEqual(GAP_PX + 1);
  }).toPass({ timeout: 5000 });
}

test.describe('Toasts and the status bar', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('a toast in the project view sits above the status bar', async ({ page }) => {
    await createProject(page, 'Toast Cave');
    await showToast(page, 'The project changed before this finished.');
    await expectAboveStatusBar(
      page.locator('[data-toast-key="project-changed"]'),
      page.getByTestId('status-bar'),
    );
  });

  test('a toast in the project list sits above the status bar', async ({ page }) => {
    await showToast(page, 'The project changed before this finished.');
    await expectAboveStatusBar(
      page.locator('[data-toast-key="project-changed"]'),
      page.getByTestId('global-view').locator('footer'),
    );
  });
});
