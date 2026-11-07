import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapReady,
  waitForNoDialogs,
  type SectionSeed,
} from './helpers/app';
import type { E2EWindow } from './helpers/e2eWindow';

/**
 * E2E tests for bulk (contiguous-range) shot deletion (#95).
 *
 * The data table opens the bulk-delete dialog when more than one real shot is
 * selected. Interior ranges offer split-vs-shift; prefix/suffix ranges only
 * shift. These tests open the dialog programmatically (the deterministic path)
 * and assert the UX + IPC wiring.
 */

const sectionSeeds: SectionSeed[] = [
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

async function getRange(
  page: Page,
): Promise<{ sectionId: number; startShotId: number; endShotId: number; sectionName: string }> {
  const ids = await page.evaluate(() => {
    const e2e = (window as E2EWindow).__E2E_TEST__;
    if (!e2e) return null;
    const section = e2e.projectStore.getState().project?.sections?.[0];
    return {
      sectionId: section?.id ?? 0,
      startShotId: section?.shots?.[1]?.id ?? 0, // interior range [1..2]
      endShotId: section?.shots?.[2]?.id ?? 0,
      sectionName: section?.name ?? '',
    };
  });
  if (!ids) throw new Error('Could not retrieve section/shot IDs');
  return ids;
}

async function openBulkDelete(page: Page, data: Record<string, unknown>): Promise<void> {
  await page.evaluate((data) => {
    const e2e = (window as E2EWindow).__E2E_TEST__;
    if (!e2e) return;
    e2e.uiStore.getState().openDialog('bulk-delete', data);
  }, data);
  await expect(page.getByTestId('bulk-delete-dialog')).toBeVisible({ timeout: 5000 });
}

function interiorData(r: {
  sectionId: number;
  startShotId: number;
  endShotId: number;
  sectionName: string;
}): Record<string, unknown> {
  return {
    sectionId: r.sectionId,
    startShotId: r.startShotId,
    endShotId: r.endShotId,
    shotCount: 2,
    kind: 'interior',
    sectionName: r.sectionName,
    startStationIndex: 1,
    endStationIndex: 3,
  };
}

test.describe('Bulk delete dialog', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Bulk Delete Test');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapReady(page);
    await waitForNoDialogs(page);
  });

  test('interior range shows the range summary, split names, and both actions', async ({ page }) => {
    const r = await getRange(page);
    await openBulkDelete(page, interiorData(r));

    const dialog = page.getByTestId('bulk-delete-dialog');
    await expect(dialog).toContainText('Delete 2 shots');
    await expect(dialog).toContainText('Main Line');

    // Interior → split offered with two prefilled name inputs.
    await expect(page.getByTestId('split-name-a')).toHaveValue('Main Line-A');
    await expect(page.getByTestId('split-name-b')).toHaveValue('Main Line-B');
    await expect(page.getByTestId('bulk-delete-split')).toBeVisible();
    await expect(page.getByTestId('bulk-delete-shift')).toBeVisible();
  });

  test('split action invokes delete_shot_range_split and closes', async ({ page }) => {
    const r = await getRange(page);
    await openBulkDelete(page, interiorData(r));

    await page.getByTestId('bulk-delete-split').click();
    await expect(page.getByTestId('bulk-delete-dialog')).toBeHidden({ timeout: 5000 });
  });

  test('shift action invokes delete_shot_range_shift and closes', async ({ page }) => {
    const r = await getRange(page);
    await openBulkDelete(page, interiorData(r));

    await page.getByTestId('bulk-delete-shift').click();
    await expect(page.getByTestId('bulk-delete-dialog')).toBeHidden({ timeout: 5000 });
  });

  test('shows an error when the delete IPC fails', async ({ page }) => {
    const r = await getRange(page);
    await page.evaluate(() => {
      const overrides = (window as E2EWindow).__MOCK_IPC_OVERRIDES__ ?? {};
      overrides.delete_shot_range_shift = { __throw: 'Shift geometry is not allowed for this range.' };
      (window as E2EWindow).__MOCK_IPC_OVERRIDES__ = overrides;
    });

    await openBulkDelete(page, interiorData(r));
    await page.getByTestId('bulk-delete-shift').click();

    const dialog = page.getByTestId('bulk-delete-dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText('Shift geometry is not allowed');
  });

  test('prefix range offers only a single delete action (no split names)', async ({ page }) => {
    const r = await getRange(page);
    await openBulkDelete(page, {
      sectionId: r.sectionId,
      startShotId: r.startShotId,
      endShotId: r.endShotId,
      shotCount: 2,
      kind: 'prefix',
      sectionName: r.sectionName,
      startStationIndex: 0,
      endStationIndex: 2,
    });

    await expect(page.getByTestId('split-name-a')).toHaveCount(0);
    await expect(page.getByTestId('bulk-delete-split')).toHaveCount(0);
    await expect(page.getByTestId('bulk-delete-shift')).toBeVisible();
  });
});
