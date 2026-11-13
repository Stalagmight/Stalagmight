import { test, expect } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
} from './helpers/app';

test.describe('Export Preview Regression Flow', () => {
  test('renders and updates stick-map preview using a seeded real project state', async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Export Regression Scenario');

    await seedProjectWithSections(page, [
      {
        name: 'Mainline',
        shots: [
          { azimuth_in: 45, azimuth_out: 45, length: 10, depth_in: -1, depth_out: -2 },
          { azimuth_in: 55, azimuth_out: 55, length: 8, depth_in: -2, depth_out: -2.8 },
        ],
      },
      {
        name: 'Sidebranch',
        shots: [
          { azimuth_in: 120, azimuth_out: 120, length: 6, depth_in: -2.8, depth_out: -3.1 },
          { azimuth_in: 135, azimuth_out: 135, length: 5, depth_in: -3.1, depth_out: -3.6 },
        ],
      },
    ]);

    // Ctrl+E enters the Export workspace, which opens on the image export and
    // its live preview.
    await page.keyboard.press('Control+e');
    await expect(page.getByTestId('export-panel')).toBeVisible({ timeout: 5000 });

    await expect(page.getByTestId('export-type-image')).toBeChecked();
    await expect(page.getByTestId('export-preview')).toBeVisible({ timeout: 10000 });

    // Drag the threshold slider. The mock preview encodes the threshold in a
    // `data-threshold` attribute so we can observe the re-fetch from the UI.
    await page.locator('[data-testid="branching-threshold-slider"]').fill('4');
    await expect(page.locator('[data-testid="export-preview"] svg'))
      .toHaveAttribute('data-threshold', '4', { timeout: 5000 });

    await page.getByTestId('export-button').click();

    await page.waitForFunction(() => {
      const mocks = (window as unknown as {
        __E2E_MOCKS__?: {
          getLastExportImageArgs: () => { options?: { branching_threshold?: number; format?: string } } | null;
        };
      }).__E2E_MOCKS__;

      const args = mocks?.getLastExportImageArgs?.();
      return Boolean(args && args.options);
    });

    const exportArgs = await page.evaluate(() => {
      const mocks = (window as unknown as {
        __E2E_MOCKS__?: {
          getLastExportImageArgs: () => { options?: { branching_threshold?: number; format?: string } } | null;
        };
      }).__E2E_MOCKS__;
      return mocks?.getLastExportImageArgs?.() ?? null;
    });

    expect(exportArgs).not.toBeNull();
    expect(exportArgs?.options?.branching_threshold).toBe(4);
    expect(exportArgs?.options?.format).toBe('png');
  });
});
