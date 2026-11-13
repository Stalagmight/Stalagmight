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

  test('is absent on an end station', async ({ page }) => {
    const section = await seedFourShotSection(page);
    await openContextMenu(page, { type: 'station', ref: { section_id: section.id, station_index: 4 } });
    await expect(page.getByTestId('context-menu')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Split Section Here' })).toHaveCount(0);
  });
});
