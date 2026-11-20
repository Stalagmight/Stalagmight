import { test, expect } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  createSectionViaStore,
  enterExportWorkspace,
} from './helpers/app';

/**
 * Create a project with two sections, so the preview has something to draw.
 */
async function createProjectWithData(page: import('./fixtures/coverage').Page) {
  await createProject(page, 'Export Test Project');
  await createSectionViaStore(page, {
    name: 'Main Passage',
    shots: [
      { azimuth_in: 10, azimuth_out: 10, length: 12, depth_in: -1.0, depth_out: -1.8 },
      { azimuth_in: 25, azimuth_out: 25, length: 9, depth_in: -1.8, depth_out: -2.4 },
      { azimuth_in: 40, azimuth_out: 40, length: 8, depth_in: -2.4, depth_out: -3.0 },
    ],
  });
  await createSectionViaStore(page, {
    name: 'Side Tunnel',
    shots: [
      { azimuth_in: 95, azimuth_out: 95, length: 6, depth_in: -2.4, depth_out: -2.6 },
      { azimuth_in: 115, azimuth_out: 115, length: 5, depth_in: -2.6, depth_out: -2.9 },
    ],
  });
}

/**
 * The Export workspace's live preview: the backend's `preview_svg_export` markup,
 * drawn over the map area for the image export (the retired modal dialog showed a
 * fixed 300x300 copy of it beside its form).
 */
test.describe('Export Preview', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProjectWithData(page);
    await enterExportWorkspace(page);
  });

  const preview = (page: import('./fixtures/coverage').Page) =>
    page.locator('[data-testid="export-mode-preview"] [data-testid="export-preview"]');

  test('shows the preview for the image export it opens on', async ({ page }) => {
    await expect(page.getByTestId('export-type-image')).toBeChecked();
    await expect(page.getByTestId('export-preview-loading')).toBeHidden({ timeout: 5000 });
    await expect(preview(page)).toBeVisible({ timeout: 5000 });
  });

  test('hides the preview for a data export and brings it back for an image', async ({ page }) => {
    await expect(preview(page)).toBeVisible({ timeout: 5000 });

    await page.click('[data-testid="export-type-data"]');
    await expect(page.getByTestId('export-mode-preview')).toHaveCount(0);

    await page.click('[data-testid="export-type-image"]');
    await expect(preview(page)).toBeVisible({ timeout: 5000 });
  });

  test('should render inline SVG from preview_svg_export', async ({ page }) => {
    await expect(page.getByTestId('export-preview-loading')).toBeHidden({ timeout: 5000 });

    // The preview component injects the backend SVG into the DOM. Mock returns
    // a 1-line stick map with a known viewBox.
    const svg = preview(page).locator('svg');
    await expect(svg).toBeVisible();
    await expect(svg).toHaveAttribute('viewBox', '0 0 100 100');
  });

  test('should re-render preview when threshold slider changes', async ({ page }) => {
    await expect(page.getByTestId('export-preview-loading')).toBeHidden({ timeout: 5000 });

    // Mock encodes the threshold as a data attribute on the root <svg>. So we
    // can observe that the preview re-fetches when the slider moves.
    const svg = preview(page).locator('svg');
    await expect(svg).toHaveAttribute('data-threshold', '2');

    await page.locator('[data-testid="branching-threshold-slider"]').fill('7');
    await expect(svg).toHaveAttribute('data-threshold', '7', { timeout: 5000 });
  });

  test('fills the map area rather than a fixed thumbnail', async ({ page }) => {
    await expect(preview(page)).toBeVisible({ timeout: 5000 });
    const area = await page.getByTestId('export-mode-preview').boundingBox();
    const box = await preview(page).boundingBox();
    expect(area).not.toBeNull();
    expect(box).not.toBeNull();
    if (area && box) {
      // Larger than the dialog's old 300x300 box, and inside the overlay.
      expect(box.width).toBeGreaterThan(300);
      expect(box.height).toBeGreaterThan(300);
      expect(box.x).toBeGreaterThanOrEqual(area.x);
      expect(box.x + box.width).toBeLessThanOrEqual(area.x + area.width + 1);
    }
  });

  test('Close export takes the preview away with the workspace', async ({ page }) => {
    await expect(preview(page)).toBeVisible({ timeout: 5000 });
    await page.getByTestId('export-close-button').click();
    await expect(page.getByTestId('export-mode-preview')).toHaveCount(0);
    await expect(page.getByTestId('export-panel')).toHaveCount(0);
  });

  test('Escape closes the workspace and its preview', async ({ page }) => {
    await expect(preview(page)).toBeVisible({ timeout: 5000 });
    await page.keyboard.press('Escape');
    await expect(page.getByTestId('export-mode-preview')).toHaveCount(0);
    await expect(page.getByTestId('export-panel')).toHaveCount(0);
  });
});
