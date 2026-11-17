import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithLines,
  waitForMapReady,
  waitForNoDialogs,
  type LineSeed,
} from './helpers/app';

/**
 * E2E tests for the line split workflow.
 *
 * Tests the Split Line dialog which opens when a mid-line shot is deleted,
 * allowing the user to name the two resulting lines.
 */

const lineSeeds: LineSeed[] = [
  {
    name: 'Main Line',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 10, depth_in: 0, depth_out: -2 },
      { azimuth_in: 90, azimuth_out: 270, length: 12, depth_in: -2, depth_out: -4 },
      { azimuth_in: 135, azimuth_out: 315, length: 8, depth_in: -4, depth_out: -6 },
      { azimuth_in: 180, azimuth_out: 0, length: 15, depth_in: -6, depth_out: -8 },
    ],
  },
];

/** Get line and shot IDs from the project store. */
async function getIds(page: Page): Promise<{ lineId: number; shotId: number; lineName: string }> {
  const ids = await page.evaluate(() => {
    const e2e = (window as any).__E2E_TEST__;
    if (!e2e) return null;
    const state = e2e.projectStore.getState();
    const line = state.project?.lines?.[0];
    // Pick a mid-line shot (index 1) for splitting
    const shot = line?.shots?.[1];
    return {
      lineId: line?.id ?? 0,
      shotId: shot?.id ?? 0,
      lineName: line?.name ?? '',
    };
  });
  if (!ids) throw new Error('Could not retrieve line/shot IDs');
  return ids;
}

/** Open split-line dialog programmatically via UI store. */
async function openSplitDialog(page: Page, lineId: number, shotId: number, lineName: string): Promise<void> {
  await page.evaluate(
    ({ lineId, shotId, lineName }) => {
      const e2e = (window as any).__E2E_TEST__;
      if (!e2e) return;
      e2e.uiStore.getState().openDialog('split-line', { lineId, shotId, lineName });
    },
    { lineId, shotId, lineName },
  );
  await expect(page.getByTestId('split-line-dialog')).toBeVisible({ timeout: 5000 });
}

test.describe('Line Split Dialog', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Split Test');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapReady(page);
    await waitForNoDialogs(page);
  });

  test('dialog renders with correct default names', async ({ page }) => {
    const { lineId, shotId, lineName } = await getIds(page);
    await openSplitDialog(page, lineId, shotId, lineName);

    const nameA = page.getByTestId('split-name-a');
    const nameB = page.getByTestId('split-name-b');

    await expect(nameA).toHaveValue('Main Line-A');
    await expect(nameB).toHaveValue('Main Line-B');
  });

  test('both name inputs are pre-filled from line name', async ({ page }) => {
    const { lineId, shotId, lineName } = await getIds(page);
    await openSplitDialog(page, lineId, shotId, lineName);

    const nameA = page.getByTestId('split-name-a');
    const nameB = page.getByTestId('split-name-b');

    // Both should contain the line name as prefix
    await expect(nameA).toHaveValue(/Main Line/);
    await expect(nameB).toHaveValue(/Main Line/);
  });

  test('empty name A disables confirm button', async ({ page }) => {
    const { lineId, shotId, lineName } = await getIds(page);
    await openSplitDialog(page, lineId, shotId, lineName);

    const nameA = page.getByTestId('split-name-a');
    const confirmBtn = page.getByTestId('split-confirm');

    await nameA.fill('');
    await expect(confirmBtn).toBeDisabled();
  });

  test('empty name B disables confirm button', async ({ page }) => {
    const { lineId, shotId, lineName } = await getIds(page);
    await openSplitDialog(page, lineId, shotId, lineName);

    const nameB = page.getByTestId('split-name-b');
    const confirmBtn = page.getByTestId('split-confirm');

    await nameB.fill('');
    await expect(confirmBtn).toBeDisabled();
  });

  test('custom names can be entered', async ({ page }) => {
    const { lineId, shotId, lineName } = await getIds(page);
    await openSplitDialog(page, lineId, shotId, lineName);

    const nameA = page.getByTestId('split-name-a');
    const nameB = page.getByTestId('split-name-b');

    await nameA.fill('North Passage');
    await nameB.fill('South Passage');

    await expect(nameA).toHaveValue('North Passage');
    await expect(nameB).toHaveValue('South Passage');

    // Confirm should be enabled with valid names
    await expect(page.getByTestId('split-confirm')).toBeEnabled();
  });

  test('confirm triggers IPC and closes dialog', async ({ page }) => {
    const { lineId, shotId, lineName } = await getIds(page);

    // Set up mock override for delete_shot_and_split
    await page.evaluate(() => {
      const overrides = (window as any).__MOCK_IPC_OVERRIDES__ ?? {};
      overrides.delete_shot_and_split = { success: true };
      (window as any).__MOCK_IPC_OVERRIDES__ = overrides;
    });

    await openSplitDialog(page, lineId, shotId, lineName);
    await page.getByTestId('split-confirm').click();

    // Dialog should close on success
    await expect(page.getByTestId('split-line-dialog')).toBeHidden({ timeout: 5000 });
  });

  test('IPC error shows error message in dialog', async ({ page }) => {
    const { lineId, shotId, lineName } = await getIds(page);

    // Set up mock override to throw error
    await page.evaluate(() => {
      const overrides = (window as any).__MOCK_IPC_OVERRIDES__ ?? {};
      overrides.delete_shot_and_split = { __throw: 'Line has too few shots to split' };
      (window as any).__MOCK_IPC_OVERRIDES__ = overrides;
    });

    await openSplitDialog(page, lineId, shotId, lineName);
    await page.getByTestId('split-confirm').click();

    // Error should display in the dialog
    const dialog = page.getByTestId('split-line-dialog');
    await expect(dialog).toContainText('too few shots', { timeout: 5000 });
  });

  test('cancel closes without action', async ({ page }) => {
    const { lineId, shotId, lineName } = await getIds(page);
    await openSplitDialog(page, lineId, shotId, lineName);

    // Click Cancel
    const dialog = page.getByTestId('split-line-dialog');
    await dialog.getByRole('button', { name: /Cancel/i }).click();

    await expect(dialog).toBeHidden({ timeout: 3000 });
  });

  test('dialog renders Delete & Split button text', async ({ page }) => {
    const { lineId, shotId, lineName } = await getIds(page);
    await openSplitDialog(page, lineId, shotId, lineName);

    const confirmBtn = page.getByTestId('split-confirm');
    await expect(confirmBtn).toContainText('Delete & Split');
  });
});
