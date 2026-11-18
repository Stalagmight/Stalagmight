/**
 * E2E tests for line-focus mode (#50).
 *
 * Double-clicking a line row dedicates the screen to that line:
 *  - both side panels (Lines, Detail) collapse,
 *  - the data table opens (~30% of the viewport),
 *  - the toolbar shows the line name (with a map icon) in place of the
 *    project name.
 * Double-clicking that name returns to the normal multi-line layout.
 */

import { test, expect, Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithLines,
  waitForMapIdle,
  waitForNoDialogs,
  type LineSeed,
} from './helpers/app';

const LINE_NAME = 'Focus Test Passage';

const lineSeeds: LineSeed[] = [
  {
    name: LINE_NAME,
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 15, depth_in: -1, depth_out: -3 },
      { azimuth_in: 90, azimuth_out: 270, length: 20, depth_in: -3, depth_out: -6 },
    ],
  },
  {
    name: 'Other Gallery',
    direction: 'out',
    shots: [{ azimuth_in: 270, azimuth_out: 90, length: 8, depth_in: -2, depth_out: -4 }],
  },
];

async function getLineIdByName(page: Page, name: string): Promise<number> {
  const id = await page.evaluate((lineName) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: {
          getState: () => { project: { lines: Array<{ id: number; name: string }> } | null };
        };
      };
    }).__E2E_TEST__;
    const line = stores?.projectStore.getState().project?.lines.find(
      (s) => s.name === lineName,
    );
    return line?.id ?? -1;
  }, name);
  expect(id).toBeGreaterThan(0);
  return id;
}

/**
 * Enter focus mode via the dedicated per-row focus button (revealed on hover).
 * Double-clicking a row now only zooms to the line - focus is button-only.
 */
async function enterFocus(page: Page, lineId: number): Promise<void> {
  const row = page.locator(`[data-testid="line-row-${lineId}"]`);
  await expect(row).toBeVisible();
  await row.hover();
  await page.getByTestId(`line-focus-${lineId}`).click();
  await expect(page.getByTestId('status-focused-line')).toBeVisible();
}

test.describe('Line focus mode', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Line Focus Test');
    await seedProjectWithLines(page, lineSeeds);
    await waitForNoDialogs(page);
    await waitForMapIdle(page);
  });

  test('focus button enters focus mode and the fit-to-view button exits', async ({ page }) => {
    const lineId = await getLineIdByName(page, LINE_NAME);

    // Baseline: project name shown, sections panel open, data table closed
    // (its closed-state opener tab is shown, the table body is unmounted).
    await expect(page.getByTestId('status-focused-line')).toHaveCount(0);
    await expect(page.getByTestId('sections-panel')).toBeVisible();
    await expect(page.getByTestId('panel-tab-data')).toBeVisible();
    await expect(page.getByTestId('data-table')).toHaveCount(0);

    // Enter focus mode via the dedicated per-row focus button.
    await enterFocus(page, lineId);

    // Toolbar swaps the project name for the focused line name.
    const focusName = page.getByTestId('status-focused-line');
    await expect(focusName).toContainText(LINE_NAME);
    await expect(page.getByTestId('mode-carousel')).toBeVisible();

    // Both side panels collapse (their closed-state opener tabs appear and the
    // workspace remains mounted but hidden); the data table opens.
    await expect(page.getByTestId('panel-tab-sections')).toBeVisible();
    await expect(page.getByTestId('sections-panel')).toBeHidden();
    await expect(page.getByTestId('panel-tab-detail')).toBeVisible();
    await expect(page.getByTestId('detail-panel')).toHaveCount(0);
    await expect(page.getByTestId('data-table')).toBeVisible();
    await expect(page.getByTestId('panel-tab-data')).toHaveCount(0);

    // The fit-to-view button gains an "Exit focus" label while focused.
    const fitToView = page.getByTestId('fit-to-view-button');
    await expect(fitToView).toContainText('Exit focus');

    // Exit via that button (doubles as exit-focus).
    await fitToView.click();

    // Layout restored: project name back, sections panel open, data table closed.
    await expect(page.getByTestId('status-focused-line')).toHaveCount(0);
    await expect(page.getByTestId('status-focused-line')).toHaveCount(0);
    await expect(page.getByTestId('sections-panel')).toBeVisible();
    await expect(page.getByTestId('panel-tab-data')).toBeVisible();
    await expect(page.getByTestId('data-table')).toHaveCount(0);
    // Label is gone once out of focus mode.
    await expect(fitToView).not.toContainText('Exit focus');
  });

  test('double-clicking a line zooms to it WITHOUT entering focus mode', async ({ page }) => {
    const lineId = await getLineIdByName(page, LINE_NAME);

    const lineRow = page.locator(`[data-testid="line-row-${lineId}"]`);
    await expect(lineRow).toBeVisible();
    // The row tooltip must teach the current gesture (double-click zooms; focus
    // lives on the per-row button) - pins the #319 stale-copy fix.
    await expect(lineRow).toHaveAttribute('title', /double-click to zoom/i);
    await lineRow.dblclick();

    // The line is selected, but focus mode is NOT entered: project name stays,
    // the sections panel remains open, and the data table stays collapsed (its
    // closed-state opener tab is shown, the table body stays unmounted).
    await expect(page.getByTestId('status-focused-line')).toHaveCount(0);
    await expect(page.getByTestId('status-focused-line')).toHaveCount(0);
    await expect(page.getByTestId('sections-panel')).toBeVisible();
    await expect(page.getByTestId('panel-tab-data')).toBeVisible();
    await expect(page.getByTestId('data-table')).toHaveCount(0);
    await expect(page.getByTestId('status-focused-line')).toHaveCount(0);
  });

  test('the per-row focus button enters focus mode', async ({ page }) => {
    const lineId = await getLineIdByName(page, LINE_NAME);

    await expect(page.getByTestId('status-focused-line')).toHaveCount(0);

    // The focus button is revealed on row hover (opacity-0 otherwise).
    const lineRow = page.locator(`[data-testid="line-row-${lineId}"]`);
    await lineRow.hover();
    const focusButton = page.getByTestId(`line-focus-${lineId}`);
    await expect(focusButton).toBeVisible();
    await focusButton.click();

    // A single click on the button enters focus mode, just like double-clicking
    // the row: toolbar shows the line name, side panels collapse, table opens.
    const focusName = page.getByTestId('status-focused-line');
    await expect(focusName).toBeVisible();
    await expect(focusName).toContainText(LINE_NAME);
    await expect(page.getByTestId('mode-carousel')).toBeVisible();
    // Side panels collapse to their opener tabs; the workspace stays mounted but hidden.
    await expect(page.getByTestId('panel-tab-sections')).toBeVisible();
    await expect(page.getByTestId('sections-panel')).toBeHidden();
    await expect(page.getByTestId('panel-tab-detail')).toBeVisible();
    await expect(page.getByTestId('detail-panel')).toHaveCount(0);
    await expect(page.getByTestId('data-table')).toBeVisible();
    await expect(page.getByTestId('panel-tab-data')).toHaveCount(0);
  });

  test('focus mode shows the focused line in the status bar; fit-to-view exits', async ({ page }) => {
    const lineId = await getLineIdByName(page, LINE_NAME);

    // Status entry is absent outside focus mode.
    await expect(page.getByTestId('status-focused-line')).toHaveCount(0);

    await enterFocus(page, lineId);

    // Focused line appears in the bottom-left status bar.
    const statusFocused = page.getByTestId('status-focused-line');
    await expect(statusFocused).toBeVisible();
    await expect(statusFocused).toContainText(LINE_NAME);

    // The fit-to-view button doubles as exit-focus; clicking it clears the status.
    await page.getByTestId('fit-to-view-button').click();
    await expect(page.getByTestId('status-focused-line')).toHaveCount(0);
  });

  test('deleting the focused line exits focus mode', async ({ page }) => {
    const lineId = await getLineIdByName(page, LINE_NAME);

    // Enter focus mode.
    await enterFocus(page, lineId);

    // Delete the focused line out from under focus mode (e.g. via undo,
    // or a delete action). Focus must not linger on a line that's gone.
    await page.evaluate((secId) => {
      const store = (window as unknown as {
        __E2E_TEST__?: { projectStore?: { getState: () => { deleteLine: (id: number) => Promise<unknown> } } };
      }).__E2E_TEST__?.projectStore;
      return store?.getState().deleteLine(secId);
    }, lineId);

    // Focus mode exits: project name returns, side panels restored.
    await expect(page.getByTestId('status-focused-line')).toHaveCount(0);
    await expect(page.getByTestId('status-focused-line')).toHaveCount(0);
    await expect(page.getByTestId('sections-panel')).toBeVisible();
  });

  test('focused heading follows line selection (e.g. clicking another line on the map)', async ({ page }) => {
    const firstId = await getLineIdByName(page, LINE_NAME);

    // Enter focus on the first line.
    await enterFocus(page, firstId);
    const focusName = page.getByTestId('status-focused-line');
    await expect(focusName).toContainText(LINE_NAME);

    // Select another line while focused (simulating a map click into it). The
    // panel is collapsed, so drive the selection through the store as the map does.
    await page.evaluate(() => {
      const ui = (window as unknown as {
        __E2E_TEST__?: {
          projectStore?: { getState: () => { project: { lines: Array<{ id: number; name: string }> } | null } };
          uiStore?: { getState: () => { setSelection: (s: { type: string; lineId: number }) => void } };
        };
      }).__E2E_TEST__;
      const other = ui?.projectStore?.getState().project?.lines.find((s) => s.name === 'Other Gallery');
      if (other) ui?.uiStore?.getState().setSelection({ type: 'line', lineId: other.id });
    });

    // The heading must update to the newly selected line.
    await expect(focusName).toContainText('Other Gallery');
    await expect(focusName).not.toContainText(LINE_NAME);
  });
});
