import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithLines,
  waitForMapReady,
  waitForNoDialogs,
} from './helpers/app';

type LineView = { id: number; name: string; shots: Array<{ id: number }> };

async function seedFourShotLine(page: Page): Promise<LineView> {
  await setupMockedPage(page);
  await createProject(page, 'Split Line Here');
  await seedProjectWithLines(page, [
    {
      name: 'Main',
      direction: 'in',
      shots: [1, 2, 3, 4].map((n) => ({
        azimuth_in: 10 * n,
        azimuth_out: 180 + 10 * n,
        length: 10,
        depth_in: -n,
        depth_out: -n - 1,
      })),
    },
  ]);
  await waitForMapReady(page);
  await waitForNoDialogs(page);
  const lines = await readLines(page);
  const line = lines[0];
  if (!line) throw new Error('seeded line missing');
  return line;
}

async function readLines(page: Page): Promise<LineView[]> {
  return page.evaluate(() =>
    ((window as any).__E2E_TEST__.projectStore.getState().project?.lines ?? []).map((s: any) => ({
      id: s.id,
      name: s.name,
      shots: s.shots.map((sh: any) => ({ id: sh.id })),
    }))
  );
}

async function openContextMenu(page: Page, target: Record<string, unknown>) {
  await page.evaluate(
    ({ target }) => {
      (window as any).__E2E_TEST__.uiStore.getState().openDialog('context-menu', {
        target,
        position: { x: 140, y: 140 },
      });
    },
    { target }
  );
}

async function confirmSplit(page: Page) {
  await page.getByRole('button', { name: 'Split Line Here' }).click();
  const dialog = page.getByTestId('split-line-here-dialog');
  await expect(dialog).toBeVisible();
  await expect(page.getByTestId('split-here-name-a')).toHaveValue('Main-A');
  await expect(page.getByTestId('split-here-name-b')).toHaveValue('Main-B');
  await page.getByTestId('split-here-confirm').click();
  await expect(dialog).not.toBeVisible();
}

test.describe('Split Line Here (#335)', () => {
  test('splits a line at a mid station', async ({ page }) => {
    const line = await seedFourShotLine(page);
    await openContextMenu(page, { type: 'station', ref: { line_id: line.id, station_index: 2 } });
    await confirmSplit(page);

    await expect.poll(async () => (await readLines(page)).map((s) => [s.name, s.shots.length])).toEqual([
      ['Main-A', 2],
      ['Main-B', 2],
    ]);
  });

  test('splits a line partway along a shot', async ({ page }) => {
    const line = await seedFourShotLine(page);
    const shotId = line.shots[1]!.id;
    await openContextMenu(page, { type: 'shot', lineId: line.id, shotId, alongT: 0.4 });
    await confirmSplit(page);

    await expect.poll(async () => (await readLines(page)).map((s) => [s.name, s.shots.length])).toEqual([
      ['Main-A', 2],
      ['Main-B', 3],
    ]);
    const call = await page.evaluate(() => {
      const calls = ((window as any).__MOCK_SPLIT_LINE_CALLS__ ?? []) as Array<any>;
      return calls[calls.length - 1] ?? null;
    });
    expect(call.at).toEqual({ type: 'shot', shot_id: shotId, distance: 4 });
  });

  test('clears a ruler measurement across the split and its undo', async ({ page }) => {
    // A ruler endpoint names a station by index and carries its coordinates; a split
    // renumbers the stations past the cut into a new line. The ruler is cleared on
    // every modified_at change, which the split and its undo both make.
    const line = await seedFourShotLine(page);
    const measure = (id: number) =>
      page.evaluate((id) => {
        const ui = (window as any).__E2E_TEST__.uiStore.getState();
        ui.setMeasureStart({ kind: 'station', line_id: id, station_index: 1, coords: { x: 0, y: 0, z: 0 } });
        ui.setMeasureEnd({ kind: 'station', line_id: id, station_index: 3, coords: { x: 20, y: 0, z: 0 } });
      }, id);
    const ruler = () =>
      page.evaluate(() => {
        const ui = (window as any).__E2E_TEST__.uiStore.getState();
        return [ui.measureStart !== null, ui.measureEnd !== null];
      });

    await measure(line.id);
    expect(await ruler()).toEqual([true, true]);
    await openContextMenu(page, { type: 'station', ref: { line_id: line.id, station_index: 2 } });
    await confirmSplit(page);
    await expect.poll(ruler).toEqual([false, false]);

    await measure(line.id);
    expect(await ruler()).toEqual([true, true]);
    await page.evaluate(() => (window as any).__E2E_TEST__.projectStore.getState().undo());
    await expect.poll(async () => (await readLines(page)).length).toBe(1);
    await expect.poll(ruler).toEqual([false, false]);
  });

  test('names the loop closure the split removes before it splits (#738)', async ({ page }) => {
    // The preview (the split run on a copy in the backend) reports one closure that
    // becomes a merge and moves Main-B, and a second one to the same point that the
    // merge leaves tying that point to itself, named by the stations it was created with.
    const line = await seedFourShotLine(page);
    await page.evaluate((id) => {
      (window as any).__MOCK_SPLIT_PREVIEW__ = {
        closures_to_merges: 1,
        moved_side: 'b',
        moved_m: 0.1,
        removed_already_one_point: [],
        removed_closures: [
          {
            closure_id: 2,
            station_a: { line_id: id, station_index: 4 },
            station_b: { line_id: id, station_index: 1 },
          },
        ],
      };
    }, line.id);
    await openContextMenu(page, { type: 'station', ref: { line_id: line.id, station_index: 2 } });
    await page.getByRole('button', { name: 'Split Line Here' }).click();
    await expect(page.getByTestId('split-line-here-dialog')).toBeVisible();

    await expect(page.getByTestId('split-here-closure-move')).toHaveText(
      'Main-B is tied to Main-A only by loop closures, so the split turns one of them into a merge and moves ' +
        'Main-B 0.10m to close it.'
    );
    await expect(page.getByTestId('split-here-closure-removed-0')).toHaveText(
      `Loop closure ${line.id}:4 to ${line.id}:1 is removed: its two stations become one point.`
    );
    // Still the user's call.
    await page.getByTestId('split-here-confirm').click();
    await expect(page.getByTestId('split-line-here-dialog')).not.toBeVisible();
    await expect.poll(async () => (await readLines(page)).length).toBe(2);
  });

  test('a delete and split, or an unmerge, that removed a closure between merged stations says so (#738)', async ({
    page,
  }) => {
    // The backend names each closure its orphan sweep removed by the stations it had
    // before the edit; the store shows them in the one closure-removal toast.
    const line = await seedFourShotLine(page);
    const message =
      'Removed loop closure 1:0 to 0:3: another loop closure became a merge, which made its two stations one point.';
    await page.evaluate((message) => {
      const removed = {
        success: true,
        line_ids: [],
        warnings: [{ code: 'CLOSURE_REMOVED_MERGED_ENDS', message, severity: 'warning' }],
      };
      const overrides = (window as any).__MOCK_IPC_OVERRIDES__ ?? {};
      overrides.delete_shot_and_split = removed;
      overrides.unmerge_station_pair = removed;
      (window as any).__MOCK_IPC_OVERRIDES__ = overrides;
    }, message);
    const toast = page.getByTestId('toast').filter({ hasText: message });

    const split = await page.evaluate(
      ({ id, shot }) =>
        (window as any).__E2E_TEST__.projectStore
          .getState()
          .deleteShotAndSplit(id, shot, 'Main-A', 'Main-B') as Promise<{ success: boolean }>,
      { id: line.id, shot: line.shots[1]!.id }
    );
    expect(split.success).toBe(true);
    await expect(toast).toHaveCount(1);

    await page.evaluate(() => (window as any).__E2E_TEST__.uiStore.getState().clearToast());
    await expect(toast).toHaveCount(0);
    const unmerged = await page.evaluate(
      (id) =>
        (window as any).__E2E_TEST__.projectStore
          .getState()
          .unmergeStationPair({ line_id: id, station_index: 1 }, { line_id: id + 1, station_index: 0 }) as Promise<{
          success: boolean;
        }>,
      line.id
    );
    expect(unmerged.success).toBe(true);
    await expect(toast).toHaveCount(1);
  });

  test('is absent on an end station', async ({ page }) => {
    const line = await seedFourShotLine(page);
    await openContextMenu(page, { type: 'station', ref: { line_id: line.id, station_index: 4 } });
    await expect(page.getByTestId('context-menu')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Split Line Here' })).toHaveCount(0);
  });
});
