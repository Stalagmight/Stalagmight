import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage, createProject } from './helpers/app';

/**
 * E2E for the structural-integrity warn + repair banner (#17). The mock's
 * check_project_integrity returns issues seeded via __MOCK_SET_INTEGRITY_ISSUES__;
 * repair_project_integrity clears them and returns the count.
 */

async function seedAndCheck(page: Page, issues: string[]): Promise<void> {
  await page.evaluate((seed) => {
    (window as unknown as { __MOCK_SET_INTEGRITY_ISSUES__: (i: string[]) => void })
      .__MOCK_SET_INTEGRITY_ISSUES__(seed);
  }, issues);
  await page.evaluate(async () => {
    await (window as unknown as {
      __E2E_TEST__: { projectStore: { getState: () => { checkProjectIntegrity: () => Promise<string[]> } } };
    }).__E2E_TEST__.projectStore.getState().checkProjectIntegrity();
  });
}

test.describe('Structural-integrity banner', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    // The integrity banner is tied to an OPEN project: checkProjectIntegrity
    // drops its result when no project is loaded (so it can't leak onto the
    // global view). Open a project so the banner can render.
    await createProject(page, 'Integrity Test');
  });

  test('no banner when the project is structurally sound', async ({ page }) => {
    await seedAndCheck(page, []);
    await expect(page.getByTestId('integrity-banner')).toHaveCount(0);
  });

  test('shows issue details in a dialog and repairs from there', async ({ page }) => {
    await seedAndCheck(page, [
      'Merged stations Side / 0 and Main / 13 are 30.03 m apart but should be the same point',
      'Merge references a missing line',
    ]);

    const banner = page.getByTestId('integrity-banner');
    await expect(banner).toBeVisible();
    await expect(banner).toContainText('2 integrity issues');

    // Details are shown in a dialog on Review, not on hover.
    await page.getByTestId('integrity-review-button').click();
    const dialog = page.getByTestId('integrity-dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog.getByTestId('integrity-dialog-list')).toContainText('30.03 m apart');
    await expect(dialog.getByTestId('integrity-dialog-list')).toContainText('references a missing line');

    await page.getByTestId('integrity-dialog-repair').click();

    // Dialog and banner clear; a confirmation toast appears.
    await expect(page.getByTestId('integrity-dialog')).toHaveCount(0);
    await expect(page.getByTestId('integrity-banner')).toHaveCount(0);
    // Scoped to the toast: ToastStack renders every message TWICE on purpose - once
    // in an `sr-only` live region that announces it, and once as the visible,
    // `aria-hidden` copy - so an unscoped text match is ambiguous by design. The
    // visible one is what this test is about.
    await expect(
      page.getByTestId('toast').filter({ hasText: /Repaired 2 issues/ }),
    ).toBeVisible();
  });

  test('repair reports issues that still need manual attention', async ({ page }) => {
    await seedAndCheck(page, [
      'Merge references a missing line',
      'Loop closure Main / 2 ↔ Side / 0 is marked closed but its ends are 8.10 m apart - recompute or remove it',
    ]);
    // One of the two issues is detect-only and survives repair.
    await page.evaluate(() => {
      (window as unknown as { __MOCK_SET_INTEGRITY_REMAINING__: (n: number) => void })
        .__MOCK_SET_INTEGRITY_REMAINING__(1);
    });

    await page.getByTestId('integrity-review-button').click();
    await page.getByTestId('integrity-dialog-repair').click();

    // Honest message: fixed one, one needs manual attention. Scoped to the toast
    // for the same reason as above - the live region carries the same text.
    await expect(
      page.getByTestId('toast').filter({ hasText: /Fixed 1 issue; 1 need.* manual attention/ }),
    ).toBeVisible();
    // The banner stays (the residual issue is still present).
    await expect(page.getByTestId('integrity-banner')).toContainText('1 integrity issue');
  });

  test('review dialog can be cancelled without repairing', async ({ page }) => {
    await seedAndCheck(page, ['Zero datum references a missing line']);
    await expect(page.getByTestId('integrity-banner')).toContainText('1 integrity issue');
    await page.getByTestId('integrity-review-button').click();
    await expect(page.getByTestId('integrity-dialog')).toBeVisible();
    await page.getByTestId('integrity-dialog-cancel').click();
    await expect(page.getByTestId('integrity-dialog')).toHaveCount(0);
    // Banner stays - nothing was repaired.
    await expect(page.getByTestId('integrity-banner')).toBeVisible();
  });

  test('banner can be dismissed without opening the dialog', async ({ page }) => {
    await seedAndCheck(page, ['Zero datum references a missing line']);
    await expect(page.getByTestId('integrity-banner')).toContainText('1 integrity issue');
    await page.getByTestId('integrity-dismiss-button').click();
    await expect(page.getByTestId('integrity-banner')).toHaveCount(0);
  });
});
