import { test, expect, Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithLines,
} from './helpers/app';

async function openEditLineDialog(page: Page): Promise<void> {
  await page.evaluate(() => {
    const e2e = (window as unknown as {
      __E2E_TEST__?: {
        uiStore: { getState: () => { openDialog: (dialog: string, data?: Record<string, unknown>) => void } };
        projectStore: { getState: () => { project: { lines: Array<{ id: number }> } | null } };
      };
    }).__E2E_TEST__;

    if (!e2e) {
      throw new Error('E2E stores not available on window');
    }

    const lineId = e2e.projectStore.getState().project?.lines?.[0]?.id ?? 0;
    e2e.uiStore.getState().openDialog('edit-line', { lineId });
  });

  await expect(page.getByTestId('edit-line-dialog')).toBeVisible({ timeout: 5000 });
}

test.describe('Dialog Shortcut Guards', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Shortcut Guard');
    await seedProjectWithLines(page, [
      {
        name: 'Main Passage',
        direction: 'in',
        shots: [
          {
            azimuth_in: 45,
            azimuth_out: 225,
            length: 10.5,
            depth_in: -2,
            depth_out: -2.5,
          },
        ],
      },
    ]);
  });

  test('tool shortcuts do not fire when a dialog is open', async ({ page }) => {
    await openEditLineDialog(page);

    const mapCanvas = page.getByTestId('map-canvas');
    const beforeTool = await mapCanvas.getAttribute('data-active-tool');

    // M (merge), not P: the pan tool is hidden by default (#409) and its
    // shortcut is inert with no dialog open either, so a P probe would pass
    // whether or not the dialog guard existed.
    await page.keyboard.press('M');

    await expect(mapCanvas).toHaveAttribute('data-active-tool', beforeTool ?? 'select');
    await expect(page.getByTestId('edit-line-dialog')).toBeVisible();
  });

  test('file shortcuts do not replace the active dialog', async ({ page }) => {
    await openEditLineDialog(page);

    await page.keyboard.press('Control+I');

    await expect(page.getByTestId('dmp-selection-dialog')).toHaveCount(0);
    await expect(page.getByTestId('edit-line-dialog')).toBeVisible();
  });
});
