import { test, expect } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  waitForMapReady,
  waitForNoDialogs,
} from './helpers/app';

test.describe('Placement Overlay Coverage', () => {
  test('shows placement preview and cancels on Escape @smoke', async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Placement Overlay');
    await waitForMapReady(page);
    await waitForNoDialogs(page);

    await page.evaluate(() => {
      (window as any).__PLACEMENT_CANCELLED__ = false;
      const ui = (window as any).__E2E_TEST__.uiStore.getState();
      ui.startPlacementMode(
        {
          name: 'Placement Line',
          surveyor: null,
          date: null,
          color: '#ff6b6b',
          direction: 'in',
          quality: 1,
          shots: [
            {
              azimuth_in: 45,
              azimuth_out: 225,
              length: 12,
              depth_in: -2,
              depth_out: -3,
              pitch_in: null,
              pitch_out: null,
              left: null,
              right: null,
              up: null,
              down: null,
              comment: null,
              kind: 'Normal',
              marker_index: null,
              temperature: null,
            },
            {
              azimuth_in: 90,
              azimuth_out: 270,
              length: 8,
              depth_in: -3,
              depth_out: -4,
              pitch_in: null,
              pitch_out: null,
              left: null,
              right: null,
              up: null,
              down: null,
              comment: null,
              kind: 'Normal',
              marker_index: null,
              temperature: null,
            },
          ],
        },
        0,
        undefined,
        () => {
          (window as any).__PLACEMENT_CANCELLED__ = true;
        }
      );
    });

    const mapCanvas = page.getByTestId('map-canvas');
    await expect(mapCanvas).toBeVisible();

    const box = await mapCanvas.boundingBox();
    if (!box) throw new Error('Map canvas not measurable');

    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);

    await expect(page.getByTestId('placement-preview')).toBeVisible();
    await expect(page.getByTestId('placement-mode-indicator')).toBeVisible();

    await page.keyboard.press('Escape');

    await expect(page.getByTestId('placement-mode-indicator')).not.toBeVisible({ timeout: 5000 });
    await expect(page.getByTestId('placement-preview')).not.toBeVisible({ timeout: 5000 });

    const cancelled = await page.evaluate(() => (window as any).__PLACEMENT_CANCELLED__);
    expect(cancelled).toBe(true);
  });
});
