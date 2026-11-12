import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapReady,
  waitForNoDialogs,
} from './helpers/app';

type SectionView = { id: number; name: string; shots: Array<{ id: number }> };

const shots = (count: number) =>
  Array.from({ length: count }, (_, i) => ({
    azimuth_in: 10 * (i + 1),
    azimuth_out: 180 + 10 * (i + 1),
    length: 10,
    depth_in: -i,
    depth_out: -i - 1,
  }));

async function seedTwoSections(page: Page): Promise<SectionView[]> {
  await setupMockedPage(page);
  await createProject(page, 'Combine Sections');
  await seedProjectWithSections(page, [
    { name: 'Main-A', direction: 'in', shots: shots(3) },
    { name: 'Main-B', direction: 'in', shots: shots(2) },
  ]);
  await waitForMapReady(page);
  await waitForNoDialogs(page);
  const sections = await readSections(page);
  if (sections.length !== 2) throw new Error('seeded sections missing');
  return sections;
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

async function openStationMenu(page: Page, sectionId: number, stationIndex: number) {
  await page.evaluate(
    ({ target }) => {
      (window as any).__E2E_TEST__.uiStore.getState().openDialog('context-menu', {
        target,
        position: { x: 140, y: 140 },
      });
    },
    { target: { type: 'station', ref: { section_id: sectionId, station_index: stationIndex } } }
  );
  await expect(page.getByTestId('context-menu')).toBeVisible();
}

test.describe('Combine Sections (#707)', () => {
  test('combines two sections from the joined end station', async ({ page }) => {
    const [first] = await seedTwoSections(page);
    await openStationMenu(page, first!.id, 3);
    await page.getByRole('button', { name: 'Combine Sections...' }).click();

    const dialog = page.getByTestId('combine-sections-dialog');
    await expect(dialog).toBeVisible();
    await expect(page.getByTestId('combine-plan')).toContainText('Main-B will be added after Main-A');
    await expect(page.getByTestId('combine-name')).toHaveValue('Main');
    await page.getByTestId('combine-name').fill('Main Line');
    await page.getByTestId('combine-confirm').click();
    await expect(dialog).not.toBeVisible();

    await expect.poll(async () => (await readSections(page)).map((s) => [s.name, s.shots.length])).toEqual([
      ['Main Line', 5],
    ]);
  });

  test('is absent on a mid-section station', async ({ page }) => {
    const [first] = await seedTwoSections(page);
    await openStationMenu(page, first!.id, 1);
    await expect(page.getByRole('button', { name: 'Split Section Here' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Combine Sections...' })).toHaveCount(0);
  });
});
