import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapReady,
  waitForNoDialogs,
} from './helpers/app';

type SectionView = { id: number; name: string; shots: Array<{ id: number }> };

async function seedFourShotSection(page: Page): Promise<SectionView> {
  await setupMockedPage(page);
  await createProject(page, 'Split Section Here');
  await seedProjectWithSections(page, [
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
  const sections = await readSections(page);
  const section = sections[0];
  if (!section) throw new Error('seeded section missing');
  return section;
}

async function readSections(page: Page): Promise<SectionView[]> {
  return page.evaluate(() =>
    ((window as any).__E2E_TEST__.projectStore.getState().project?.sections ?? []).map((s: any) => ({
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
  await page.getByRole('button', { name: 'Split Section Here' }).click();
  const dialog = page.getByTestId('split-section-here-dialog');
  await expect(dialog).toBeVisible();
  await expect(page.getByTestId('split-here-name-a')).toHaveValue('Main-A');
  await expect(page.getByTestId('split-here-name-b')).toHaveValue('Main-B');
  await page.getByTestId('split-here-confirm').click();
  await expect(dialog).not.toBeVisible();
}

test.describe('Split Section Here (#335)', () => {
  test('splits a section at a mid station', async ({ page }) => {
    const section = await seedFourShotSection(page);
    await openContextMenu(page, { type: 'station', ref: { section_id: section.id, station_index: 2 } });
    await confirmSplit(page);

    await expect.poll(async () => (await readSections(page)).map((s) => [s.name, s.shots.length])).toEqual([
      ['Main-A', 2],
      ['Main-B', 2],
    ]);
  });

  test('splits a section partway along a shot', async ({ page }) => {
    const section = await seedFourShotSection(page);
    const shotId = section.shots[1]!.id;
    await openContextMenu(page, { type: 'shot', sectionId: section.id, shotId, alongT: 0.4 });
    await confirmSplit(page);

    await expect.poll(async () => (await readSections(page)).map((s) => [s.name, s.shots.length])).toEqual([
      ['Main-A', 2],
      ['Main-B', 3],
    ]);
    const call = await page.evaluate(() => {
      const calls = ((window as any).__MOCK_SPLIT_SECTION_CALLS__ ?? []) as Array<any>;
      return calls[calls.length - 1] ?? null;
    });
    expect(call.at).toEqual({ type: 'shot', shot_id: shotId, distance: 4 });
  });

  test('clears a ruler measurement across the split and its undo', async ({ page }) => {
    // A ruler endpoint names a station by index and carries its coordinates; a split
    // renumbers the stations past the cut into a new section. The ruler is cleared on
    // every modified_at change, which the split and its undo both make.
    const section = await seedFourShotSection(page);
    const measure = (id: number) =>
      page.evaluate((id) => {
        const ui = (window as any).__E2E_TEST__.uiStore.getState();
        ui.setMeasureStart({ kind: 'station', section_id: id, station_index: 1, coords: { x: 0, y: 0, z: 0 } });
        ui.setMeasureEnd({ kind: 'station', section_id: id, station_index: 3, coords: { x: 20, y: 0, z: 0 } });
      }, id);
    const ruler = () =>
      page.evaluate(() => {
        const ui = (window as any).__E2E_TEST__.uiStore.getState();
        return [ui.measureStart !== null, ui.measureEnd !== null];
      });

    await measure(section.id);
    expect(await ruler()).toEqual([true, true]);
    await openContextMenu(page, { type: 'station', ref: { section_id: section.id, station_index: 2 } });
    await confirmSplit(page);
    await expect.poll(ruler).toEqual([false, false]);

    await measure(section.id);
    expect(await ruler()).toEqual([true, true]);
    await page.evaluate(() => (window as any).__E2E_TEST__.projectStore.getState().undo());
    await expect.poll(async () => (await readSections(page)).length).toBe(1);
    await expect.poll(ruler).toEqual([false, false]);
  });

  test('names the loop closure the split removes before it splits (#738)', async ({ page }) => {
    // The preview (the split run on a copy in the backend) reports one closure that
    // becomes a merge and moves Main-B, and a second one to the same point that the
    // merge leaves tying that point to itself, named by the stations it was created with.
    const section = await seedFourShotSection(page);
    await page.evaluate((id) => {
      (window as any).__MOCK_SPLIT_PREVIEW__ = {
        closures_to_merges: 1,
        moved_side: 'b',
        moved_m: 0.1,
        removed_already_one_point: [],
        removed_closures: [
          {
            closure_id: 2,
            station_a: { section_id: id, station_index: 4 },
            station_b: { section_id: id, station_index: 1 },
          },
        ],
      };
    }, section.id);
    await openContextMenu(page, { type: 'station', ref: { section_id: section.id, station_index: 2 } });
    await page.getByRole('button', { name: 'Split Section Here' }).click();
    await expect(page.getByTestId('split-section-here-dialog')).toBeVisible();

    await expect(page.getByTestId('split-here-closure-move')).toHaveText(
      'Main-B is tied to Main-A only by loop closures, so the split turns one of them into a merge and moves ' +
        'Main-B 0.10m to close it.'
    );
    await expect(page.getByTestId('split-here-closure-removed-0')).toHaveText(
      `Loop closure ${section.id}:4 to ${section.id}:1 is removed: its two stations become one point.`
    );
    // Still the user's call.
    await page.getByTestId('split-here-confirm').click();
    await expect(page.getByTestId('split-section-here-dialog')).not.toBeVisible();
    await expect.poll(async () => (await readSections(page)).length).toBe(2);
  });

  test('a delete and split, or an unmerge, that removed a closure between merged stations says so (#738)', async ({
    page,
  }) => {
    // The backend names each closure its orphan sweep removed by the stations it had
    // before the edit; the store shows them in the one closure-removal toast.
    const section = await seedFourShotSection(page);
    const message =
      'Removed loop closure 1:0 to 0:3: another loop closure became a merge, which made its two stations one point.';
    await page.evaluate((message) => {
      const removed = {
        success: true,
        section_ids: [],
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
      { id: section.id, shot: section.shots[1]!.id }
    );
    expect(split.success).toBe(true);
    await expect(toast).toHaveCount(1);

    await page.evaluate(() => (window as any).__E2E_TEST__.uiStore.getState().clearToast());
    await expect(toast).toHaveCount(0);
    const unmerged = await page.evaluate(
      (id) =>
        (window as any).__E2E_TEST__.projectStore
          .getState()
          .unmergeStationPair({ section_id: id, station_index: 1 }, { section_id: id + 1, station_index: 0 }) as Promise<{
          success: boolean;
        }>,
      section.id
    );
    expect(unmerged.success).toBe(true);
    await expect(toast).toHaveCount(1);
  });

  test('is absent on an end station', async ({ page }) => {
    const section = await seedFourShotSection(page);
    await openContextMenu(page, { type: 'station', ref: { section_id: section.id, station_index: 4 } });
    await expect(page.getByTestId('context-menu')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Split Section Here' })).toHaveCount(0);
  });
});
