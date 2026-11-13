import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapReady,
  waitForNoDialogs,
} from './helpers/app';

type SectionView = { id: number; name: string; shots: number };

function shots(count: number, base: number) {
  return Array.from({ length: count }, (_, i) => ({
    azimuth_in: base + 10 * i,
    azimuth_out: base + 10 * i,
    length: 10,
    depth_in: -i,
    depth_out: -i - 1,
  }));
}

async function seed(page: Page, sections: Array<{ name: string; count: number }>): Promise<SectionView[]> {
  await setupMockedPage(page);
  await createProject(page, 'Combine Sections');
  await seedProjectWithSections(
    page,
    sections.map((s, i) => ({ name: s.name, direction: 'in', shots: shots(s.count, 30 * i) }))
  );
  await waitForMapReady(page);
  await waitForNoDialogs(page);
  return readSections(page);
}

async function readSections(page: Page): Promise<SectionView[]> {
  return page.evaluate(() =>
    ((window as any).__E2E_TEST__.projectStore.getState().project?.sections ?? []).map((s: any) => ({
      id: s.id,
      name: s.name,
      shots: s.shots.length,
    }))
  );
}

async function openContextMenu(page: Page, target: Record<string, unknown>) {
  await page.evaluate(
    ({ target }) => {
      (window as any).__E2E_TEST__.uiStore.getState().openDialog('context-menu', {
        target,
        position: { x: 140, y: 140 },
        survivesScroll: true,
      });
    },
    { target }
  );
  await expect(page.getByTestId('context-menu')).toBeVisible();
}

async function merge(page: Page, a: { section_id: number; station_index: number }, b: { section_id: number; station_index: number }) {
  await page.evaluate(
    ({ a, b }) => (window as any).__E2E_TEST__.projectStore.getState().mergeStations(a, b),
    { a, b }
  );
}

async function undo(page: Page) {
  await page.evaluate(() => (window as any).__E2E_TEST__.projectStore.getState().undo());
}

/** The station menu's single combine row, when one other section ends there. */
async function combineWith(page: Page, partner: string) {
  const item = page.getByRole('button', { name: `Combine Section With ${partner}...` });
  await expect(item).toBeEnabled();
  await item.click();
}

/** Open the dialog's folded Details. */
async function openDetails(page: Page) {
  await page.getByTestId('combine-details-toggle').click();
  await expect(page.getByTestId('combine-details')).toHaveAttribute('open', '');
}

const sectionShape = async (page: Page) => (await readSections(page)).map((s) => [s.name, s.shots]);

test.describe('Combine Sections', () => {
  test('splits a section and combines it back from the join, then one undo restores the split', async ({ page }) => {
    const [main] = await seed(page, [{ name: 'Main', count: 4 }]);
    await openContextMenu(page, { type: 'station', ref: { section_id: main!.id, station_index: 2 } });
    await page.getByRole('button', { name: 'Split Section Here' }).click();
    await page.getByTestId('split-here-confirm').click();
    await waitForNoDialogs(page);
    await expect.poll(() => sectionShape(page)).toEqual([
      ['Main-A', 2],
      ['Main-B', 2],
    ]);

    // The first half's end is the join: the item is offered there.
    await openContextMenu(page, { type: 'station', ref: { section_id: main!.id, station_index: 2 } });
    await combineWith(page, 'Main-B');
    const dialog = page.getByTestId('combine-sections-dialog');
    await expect(dialog).toBeVisible();
    await expect(page.getByTestId('combine-summary')).toHaveText(/Combining Main-A \(2 shots\) and Main-B \(2 shots\) at station/);
    await expect(page.getByTestId('combine-name')).toHaveValue('Main');
    // The combined survey credit is summarised from both halves' shots, in
    // the folded Details.
    await openDetails(page);
    await expect(page.getByTestId('combine-credit')).toContainText('(4 shots)');
    await page.getByTestId('combine-confirm').click();
    await expect(dialog).not.toBeVisible();

    await expect.poll(() => sectionShape(page)).toEqual([['Main', 4]]);
    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_TEST__.uiStore.getState().selection))
      .toMatchObject({ type: 'section', sectionId: main!.id });

    await undo(page);
    await expect.poll(() => sectionShape(page)).toEqual([
      ['Main-A', 2],
      ['Main-B', 2],
    ]);
  });

  test('combines two selected sections from the Sections panel menu', async ({ page }) => {
    const [a, b] = await seed(page, [
      { name: 'East', count: 2 },
      { name: 'West', count: 3 },
    ]);
    await merge(page, { section_id: a!.id, station_index: 2 }, { section_id: b!.id, station_index: 3 });
    await page.evaluate((ids) => {
      const ui = (window as any).__E2E_TEST__.uiStore.getState();
      ui.setSelection({ type: 'section', sectionId: ids[0] });
      ui.addToSelection({ type: 'section', sectionId: ids[1] });
    }, [a!.id, b!.id]);

    await openContextMenu(page, { type: 'section', sectionId: a!.id });
    const item = page.getByRole('button', { name: 'Combine 2 Sections...' });
    await expect(item).toBeEnabled();
    await item.click();
    await expect(page.getByTestId('combine-sections-dialog')).toBeVisible();
    // End to end: the other section is reversed to line up.
    await expect(page.getByTestId('combine-plan')).toHaveText('West will be reversed and added after East.');
    await expect(page.getByTestId('combine-reverse-notice')).toBeVisible();
    await expect(page.getByTestId('combine-swap-lr')).toBeChecked();
    await page.getByTestId('combine-confirm').click();
    await waitForNoDialogs(page);

    await expect.poll(() => sectionShape(page)).toEqual([['East', 5]]);
    const call = await page.evaluate(() => {
      const calls = ((window as any).__MOCK_COMBINE_SECTIONS_CALLS__ ?? []) as Array<any>;
      return calls[calls.length - 1] ?? null;
    });
    expect(call).toMatchObject({ kept: a!.id, other: b!.id, options: { name: 'East', swap_lr: true } });
  });

  test('at a junction, the menu lists every section ending there and the dialog opens on the one picked', async ({ page }) => {
    const [a, b, c] = await seed(page, [
      { name: 'Trunk', count: 2 },
      { name: 'Left', count: 2 },
      { name: 'Right', count: 3 },
    ]);
    const join = { section_id: a!.id, station_index: 2 };
    await merge(page, join, { section_id: b!.id, station_index: 0 });
    await merge(page, join, { section_id: c!.id, station_index: 0 });

    await openContextMenu(page, { type: 'station', ref: join });
    const item = page.getByRole('button', { name: 'Combine Sections...' });
    await expect(item).toBeEnabled();
    await expect(item).toContainText('3 sections end here');
    await expect(page.getByRole('button', { name: 'Split Section Here' })).toHaveCount(0);
    // Hovering opens the submenu: every pair of the three sections ending here.
    await item.hover();
    const submenu = page.getByTestId('context-menu-submenu');
    await expect(submenu).toBeVisible();
    await expect(submenu.getByRole('button')).toHaveText([/^Trunk with Left/, /^Trunk with Right/, /^Left with Right/]);
    await expect(submenu.getByRole('button', { name: 'Trunk with Left' })).toContainText('Merged here');
    await submenu.getByRole('button', { name: 'Trunk with Right' }).click();

    // The dialog opens on that pair, with no picker of its own.
    await expect(page.getByTestId('combine-sections-dialog')).toBeVisible();
    await expect(page.getByTestId('combine-partner-picker')).toHaveCount(0);
    await openDetails(page);
    await expect(page.getByTestId('combine-branches-note')).toContainText('Left stays attached at the join');
    await expect(page.getByTestId('combine-summary')).toContainText('Combining Trunk (2 shots) and Right (3 shots)');
    await page.getByTestId('combine-confirm').click();
    await waitForNoDialogs(page);
    const call = await page.evaluate(() => {
      const calls = ((window as any).__MOCK_COMBINE_SECTIONS_CALLS__ ?? []) as Array<any>;
      return calls[calls.length - 1] ?? null;
    });
    expect(call).toMatchObject({ kept: a!.id, other: c!.id });
  });

  test('combines two sections in a loop at their loop closure, listing the merge that becomes a closure', async ({ page }) => {
    const [east, west] = await seed(page, [
      { name: 'East', count: 2 },
      { name: 'West', count: 2 },
    ]);
    // West's end is merged to East's start; East's end and West's start
    // close the loop with a loop closure.
    await merge(page, { section_id: east!.id, station_index: 0 }, { section_id: west!.id, station_index: 2 });
    const closed = await page.evaluate(
      ({ a, b }) => (window as any).__E2E_TEST__.projectStore.getState().createLoopClosure(a, b, 'Batch'),
      { a: { section_id: east!.id, station_index: 2 }, b: { section_id: west!.id, station_index: 0 } }
    );
    expect(closed).toMatchObject({ success: true });

    await openContextMenu(page, { type: 'station', ref: { section_id: east!.id, station_index: 2 } });
    await expect(page.getByRole('button', { name: 'Combine Section With West...' })).toContainText(
      'Across a loop closure'
    );
    await combineWith(page, 'West');

    await expect(page.getByTestId('combine-closure-join')).toContainText('East and West meet here across a loop closure');
    await openDetails(page);
    await expect(page.getByTestId('combine-note-torn')).toContainText('This connection becomes a loop closure:');
    await expect(page.locator('[data-testid^="combine-torn-tie-"]')).toHaveCount(1);
    await expect(page.locator('[data-testid^="combine-torn-tie-"]')).toContainText('The merge of');
    // Each end of the tie is a section pill and a station pill.
    const tie = page.locator('[data-testid^="combine-torn-tie-"]');
    await expect(tie.getByTestId(`combine-tie-section-${east!.id}`)).toBeVisible();
    await expect(tie.getByTestId(`combine-tie-station-${east!.id}-0`)).toBeVisible();
    await expect(tie.getByTestId(`combine-tie-section-${west!.id}`)).toBeVisible();
    await expect(tie.getByTestId(`combine-tie-station-${west!.id}-2`)).toBeVisible();
    await page.getByTestId('combine-confirm').click();
    await waitForNoDialogs(page);

    const call = await page.evaluate(() => {
      const calls = ((window as any).__MOCK_COMBINE_SECTIONS_CALLS__ ?? []) as Array<any>;
      return calls[calls.length - 1] ?? null;
    });
    expect(call).toMatchObject({ kept: east!.id, other: west!.id, options: { join: { kept: 'end', other: 'start' } } });
    expect(await sectionShape(page)).toEqual([['East', 4]]);
    const ties = await page.evaluate(() => {
      const project = (window as any).__E2E_TEST__.projectStore.getState().project;
      return { merges: project.station_merges.length, closures: project.loop_closures.length };
    });
    expect(ties).toEqual({ merges: 0, closures: 1 });
  });

  test('keeps its row, disabled, where no other section ends; none on a mid station', async ({ page }) => {
    const [main] = await seed(page, [{ name: 'Main', count: 3 }]);
    await openContextMenu(page, { type: 'station', ref: { section_id: main!.id, station_index: 3 } });
    const item = page.getByRole('button', { name: 'Combine Sections...' });
    await expect(item).toBeDisabled();
    await expect(item).toHaveAttribute('title', 'No other section ends at this station');
    await expect(item).toContainText('No other section ends at this station');

    await page.keyboard.press('Escape');
    await openContextMenu(page, { type: 'station', ref: { section_id: main!.id, station_index: 1 } });
    await expect(page.getByRole('button', { name: 'Edit Station' })).toBeVisible();
    await expect(page.getByRole('button', { name: /^Combine Section/ })).toHaveCount(0);
  });
});
