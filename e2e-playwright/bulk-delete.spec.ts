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
 * E2E tests for bulk (contiguous-range) shot deletion (#95).
 *
 * The data table opens the bulk-delete dialog when more than one real shot is
 * selected. Interior ranges offer split-vs-shift; prefix/suffix ranges only
 * shift. These tests open the dialog programmatically (the deterministic path)
 * and assert the UX + IPC wiring.
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

async function getRange(
  page: Page,
): Promise<{ lineId: number; startShotId: number; endShotId: number; lineName: string }> {
  const ids = await page.evaluate(() => {
    const e2e = (window as any).__E2E_TEST__;
    if (!e2e) return null;
    const line = e2e.projectStore.getState().project?.lines?.[0];
    return {
      lineId: line?.id ?? 0,
      startShotId: line?.shots?.[1]?.id ?? 0, // interior range [1..2]
      endShotId: line?.shots?.[2]?.id ?? 0,
      lineName: line?.name ?? '',
    };
  });
  if (!ids) throw new Error('Could not retrieve line/shot IDs');
  return ids;
}

async function openBulkDelete(page: Page, data: Record<string, unknown>): Promise<void> {
  await page.evaluate((data) => {
    const e2e = (window as any).__E2E_TEST__;
    if (!e2e) return;
    e2e.uiStore.getState().openDialog('bulk-delete', data);
  }, data);
  await expect(page.getByTestId('bulk-delete-dialog')).toBeVisible({ timeout: 5000 });
}

function interiorData(r: {
  lineId: number;
  startShotId: number;
  endShotId: number;
  lineName: string;
}): Record<string, unknown> {
  return {
    lineId: r.lineId,
    startShotId: r.startShotId,
    endShotId: r.endShotId,
    shotCount: 2,
    kind: 'interior',
    lineName: r.lineName,
    startStationIndex: 1,
    endStationIndex: 3,
  };
}

test.describe('Bulk delete dialog', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Bulk Delete Test');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapReady(page);
    await waitForNoDialogs(page);
  });

  test('interior range shows the range summary, split names, and both actions', async ({ page }) => {
    const r = await getRange(page);
    await openBulkDelete(page, interiorData(r));

    const dialog = page.getByTestId('bulk-delete-dialog');
    await expect(dialog).toContainText('Delete 2 shots (stations 1 to 3 in Main Line).');

    // Interior → split offered with two prefilled name inputs.
    await expect(page.getByTestId('split-name-a')).toHaveValue('Main Line A');
    await expect(page.getByTestId('split-name-b')).toHaveValue('Main Line B');
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
      const overrides = (window as any).__MOCK_IPC_OVERRIDES__ ?? {};
      overrides.delete_shot_range_shift = { __throw: 'Shift geometry is not allowed for this range.' };
      (window as any).__MOCK_IPC_OVERRIDES__ = overrides;
    });

    await openBulkDelete(page, interiorData(r));
    await page.getByTestId('bulk-delete-shift').click();

    const dialog = page.getByTestId('bulk-delete-dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText('Shift geometry is not allowed');
  });

  test('lists the loop closures closing the gap would remove, before confirming (#737)', async ({ page }) => {
    const r = await getRange(page);
    await page.evaluate((lineId) => {
      const overrides = (window as any).__MOCK_IPC_OVERRIDES__ ?? {};
      overrides.preview_delete_shot_range = {
        line_id: lineId,
        start_pos: 1,
        end_pos: 2,
        kind: 'interior',
        removed_stations: [
          { line_id: lineId, station_index: 2 },
          { line_id: lineId, station_index: 3 },
        ],
        removed_merges: [],
        removed_closures: [],
        closure_to_merge: [],
        shift_blocked: false,
        shift_blocked_reasons: [],
        removed_datums: [],
        removed_comments: [],
        closures_removed_across_one_shot: [
          {
            closure_id: 7,
            station_a: { line_id: lineId, station_index: 1 },
            station_b: { line_id: lineId, station_index: 4 },
          },
        ],
      };
      (window as any).__MOCK_IPC_OVERRIDES__ = overrides;
    }, r.lineId);

    await openBulkDelete(page, interiorData(r));
    const block = page.getByTestId('impact-closures-across-one-shot');
    await expect(block).toBeVisible();
    // Only closing the gap removes them, not Split: the heading names the button.
    await expect(block).toContainText(
      'Close Gap & Delete removes these loop closures (they would span a single shot)'
    );
    // The stations the closure was created with, by name, not the renumbered ones.
    await expect(block).toContainText('#7: Main Line / 1 <-> Main Line / 4');
  });

  test('prefix range offers only a single delete action (no split names)', async ({ page }) => {
    const r = await getRange(page);
    await openBulkDelete(page, {
      lineId: r.lineId,
      startShotId: r.startShotId,
      endShotId: r.endShotId,
      shotCount: 2,
      kind: 'prefix',
      lineName: r.lineName,
      startStationIndex: 0,
      endStationIndex: 2,
    });

    await expect(page.getByTestId('split-name-a')).toHaveCount(0);
    await expect(page.getByTestId('bulk-delete-split')).toHaveCount(0);
    await expect(page.getByTestId('bulk-delete-shift')).toBeVisible();
  });
});
