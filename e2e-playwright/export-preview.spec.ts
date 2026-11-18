import { test, expect } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  createLineViaStore,
} from './helpers/app';

/**
 * Helper to create a new project and import some data for testing
 */
async function createProjectWithData(page: import('./fixtures/coverage').Page) {
  await createProject(page, 'Export Test Project');
  await createLineViaStore(page, {
    name: 'Main Passage',
    shots: [
      { azimuth_in: 10, azimuth_out: 10, length: 12, depth_in: -1.0, depth_out: -1.8 },
      { azimuth_in: 25, azimuth_out: 25, length: 9, depth_in: -1.8, depth_out: -2.4 },
      { azimuth_in: 40, azimuth_out: 40, length: 8, depth_in: -2.4, depth_out: -3.0 },
    ],
  });
  await createLineViaStore(page, {
    name: 'Side Tunnel',
    shots: [
      { azimuth_in: 95, azimuth_out: 95, length: 6, depth_in: -2.4, depth_out: -2.6 },
      { azimuth_in: 115, azimuth_out: 115, length: 5, depth_in: -2.6, depth_out: -2.9 },
    ],
  });
}

/**
 * Helper to open the export dialog
 */
async function openExportDialog(page: import('./fixtures/coverage').Page) {
  await page.keyboard.press('Control+e');
  await expect(page.getByTestId('export-dialog')).toBeVisible({ timeout: 5000 });
}

test.describe('Export Preview', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProjectWithData(page);
  });

  test('should show preview panel when image export is selected', async ({ page }) => {
    await openExportDialog(page);

    // By default, data export is selected, preview should not be visible
    const preview = page.locator('[data-testid="export-preview"]');
    await expect(preview).not.toBeVisible();

    // Switch to image export
    await page.click('[data-testid="export-type-image"]');
    await expect(page.getByTestId('export-preview-loading')).toBeHidden();

    // Preview should now be visible
    await expect(preview).toBeVisible({ timeout: 5000 });
  });

  test('should hide preview when switching back to data export', async ({ page }) => {
    await openExportDialog(page);

    // Switch to image export
    await page.click('[data-testid="export-type-image"]');
    const preview = page.locator('[data-testid="export-preview"]');
    await expect(preview).toBeVisible({ timeout: 5000 });

    // Switch back to data export
    await page.click('[data-testid="export-type-data"]');

    // Preview should be hidden
    await expect(preview).not.toBeVisible();
  });

  test('should render inline SVG from preview_svg_export', async ({ page }) => {
    await openExportDialog(page);
    await page.click('[data-testid="export-type-image"]');
    await expect(page.getByTestId('export-preview-loading')).toBeHidden({ timeout: 5000 });

    // The preview component injects the backend SVG into the DOM. Mock returns
    // a 1-line stick map with a known viewBox.
    const svg = page.locator('[data-testid="export-preview"] svg');
    await expect(svg).toBeVisible();
    await expect(svg).toHaveAttribute('viewBox', '0 0 100 100');
  });

  test('should re-render preview when threshold slider changes', async ({ page }) => {
    await openExportDialog(page);
    await page.click('[data-testid="export-type-image"]');
    await expect(page.getByTestId('export-preview-loading')).toBeHidden({ timeout: 5000 });

    // Mock encodes the threshold as a data attribute on the root <svg>. So we
    // can observe that the preview re-fetches when the slider moves.
    const svg = page.locator('[data-testid="export-preview"] svg');
    await expect(svg).toHaveAttribute('data-threshold', '2');

    await page.locator('[data-testid="branching-threshold-slider"]').fill('7');
    await expect(svg).toHaveAttribute('data-threshold', '7', { timeout: 5000 });
  });

  test('should have correct preview dimensions (300x300)', async ({ page }) => {
    await openExportDialog(page);

    // Switch to image export
    await page.click('[data-testid="export-type-image"]');

    const preview = page.locator('[data-testid="export-preview"]');
    await expect(preview).toBeVisible({ timeout: 5000 });

    // Check dimensions
    const boundingBox = await preview.boundingBox();
    expect(boundingBox).not.toBeNull();
    if (boundingBox) {
      expect(boundingBox.width).toBe(300);
      expect(boundingBox.height).toBe(300);
    }
  });

  test('should close export dialog via Cancel button', async ({ page }) => {
    await openExportDialog(page);

    const dialog = page.locator('[data-testid="export-dialog"]');
    await expect(dialog).toBeVisible({ timeout: 5000 });

    // Close via Cancel
    await page.click('button:has-text("Cancel")');

    await expect(dialog).not.toBeVisible({ timeout: 3000 });
  });

  test('should close export dialog via Escape key', async ({ page }) => {
    await openExportDialog(page);

    const dialog = page.locator('[data-testid="export-dialog"]');
    await expect(dialog).toBeVisible({ timeout: 5000 });

    // Close via Escape
    await page.keyboard.press('Escape');

    await expect(dialog).not.toBeVisible({ timeout: 3000 });
  });
});
