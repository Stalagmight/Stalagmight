import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithLines,
  waitForMapReady,
  waitForNoDialogs,
} from './helpers/app';

type LineView = { id: number; name: string; shots: number };

function shots(count: number, base: number) {
  return Array.from({ length: count }, (_, i) => ({
    azimuth_in: base + 10 * i,
    azimuth_out: base + 10 * i,
    length: 10,
    depth_in: -i,
    depth_out: -i - 1,
  }));
}

async function seed(page: Page, lines: Array<{ name: string; count: number }>): Promise<LineView[]> {
  await setupMockedPage(page);
  await createProject(page, 'Combine Lines');
  await seedProjectWithLines(
    page,
    lines.map((s, i) => ({ name: s.name, direction: 'in', shots: shots(s.count, 30 * i) }))
  );
  await waitForMapReady(page);
  await waitForNoDialogs(page);
  return readLines(page);
}

async function readLines(page: Page): Promise<LineView[]> {
  return page.evaluate(() =>
    ((window as any).__E2E_TEST__.projectStore.getState().project?.lines ?? []).map((s: any) => ({
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

async function merge(page: Page, a: { line_id: number; station_index: number }, b: { line_id: number; station_index: number }) {
  await page.evaluate(
    ({ a, b }) => (window as any).__E2E_TEST__.projectStore.getState().mergeStations(a, b),
    { a, b }
  );
}

async function undo(page: Page) {
  await page.evaluate(() => (window as any).__E2E_TEST__.projectStore.getState().undo());
}

/** The station menu's single combine row, when one other line ends there. */
async function combineWith(page: Page, partner: string) {
  const item = page.getByRole('button', { name: `Combine Line With ${partner}...` });
  await expect(item).toBeEnabled();
  await item.click();
}

/** Open the dialog's folded Details. */
async function openDetails(page: Page) {
  await page.getByTestId('combine-details-toggle').click();
  await expect(page.getByTestId('combine-details')).toHaveAttribute('open', '');
}

const lineShape = async (page: Page) => (await readLines(page)).map((s) => [s.name, s.shots]);

test.describe('Combine Lines', () => {
  test('splits a line and combines it back from the join, then one undo restores the split', async ({ page }) => {
    const [main] = await seed(page, [{ name: 'Main', count: 4 }]);
    await openContextMenu(page, { type: 'station', ref: { line_id: main!.id, station_index: 2 } });
    await page.getByRole('button', { name: 'Split Line Here' }).click();
    await page.getByTestId('split-here-confirm').click();
    await waitForNoDialogs(page);
    await expect.poll(() => lineShape(page)).toEqual([
      ['Main A', 2],
      ['Main B', 2],
    ]);

    // The first half's end is the join: the item is offered there.
    await openContextMenu(page, { type: 'station', ref: { line_id: main!.id, station_index: 2 } });
    await combineWith(page, 'Main B');
    const dialog = page.getByTestId('combine-lines-dialog');
    await expect(dialog).toBeVisible();
    await expect(page.getByTestId('combine-summary')).toHaveText(/Combining Main A \(2 shots\) and Main B \(2 shots\) at station Main A \/ 2$/);
    await expect(page.getByTestId('combine-name')).toHaveValue('Main');
    // The combined survey credit is summarised from both halves' shots, in
    // the folded Details.
    await openDetails(page);
    await expect(page.getByTestId('combine-credit')).toContainText('(4 shots)');
    await page.getByTestId('combine-confirm').click();
    await expect(dialog).not.toBeVisible();

    await expect.poll(() => lineShape(page)).toEqual([['Main', 4]]);
    await expect
      .poll(() => page.evaluate(() => (window as any).__E2E_TEST__.uiStore.getState().selection))
      .toMatchObject({ type: 'line', lineId: main!.id });

    await undo(page);
    await expect.poll(() => lineShape(page)).toEqual([
      ['Main A', 2],
      ['Main B', 2],
    ]);
  });

  test('combines two selected lines from the Sections panel menu', async ({ page }) => {
    const [a, b] = await seed(page, [
      { name: 'East', count: 2 },
      { name: 'West', count: 3 },
    ]);
    await merge(page, { line_id: a!.id, station_index: 2 }, { line_id: b!.id, station_index: 3 });
    await page.evaluate((ids) => {
      const ui = (window as any).__E2E_TEST__.uiStore.getState();
      ui.setSelection({ type: 'line', lineId: ids[0] });
      ui.addToSelection({ type: 'line', lineId: ids[1] });
    }, [a!.id, b!.id]);

    await openContextMenu(page, { type: 'line', lineId: a!.id });
    const item = page.getByRole('button', { name: 'Combine 2 Lines...' });
    await expect(item).toBeEnabled();
    await item.click();
    await expect(page.getByTestId('combine-lines-dialog')).toBeVisible();
    // End to end: the other line is reversed to line up.
    await expect(page.getByTestId('combine-plan')).toHaveText('West will be reversed and added after East.');
    await expect(page.getByTestId('combine-reverse-notice')).toBeVisible();
    await expect(page.getByTestId('combine-swap-lr')).toBeChecked();
    await page.getByTestId('combine-confirm').click();
    await waitForNoDialogs(page);

    await expect.poll(() => lineShape(page)).toEqual([['East', 5]]);
    const call = await page.evaluate(() => {
      const calls = ((window as any).__MOCK_COMBINE_LINES_CALLS__ ?? []) as Array<any>;
      return calls[calls.length - 1] ?? null;
    });
    expect(call).toMatchObject({ kept: a!.id, other: b!.id, options: { name: 'East', swap_lr: true } });
  });

  test('at a junction, the menu lists every line ending there and the dialog opens on the one picked', async ({ page }) => {
    const [a, b, c] = await seed(page, [
      { name: 'Trunk', count: 2 },
      { name: 'Left', count: 2 },
      { name: 'Right', count: 3 },
    ]);
    const join = { line_id: a!.id, station_index: 2 };
    await merge(page, join, { line_id: b!.id, station_index: 0 });
    await merge(page, join, { line_id: c!.id, station_index: 0 });

    await openContextMenu(page, { type: 'station', ref: join });
    const item = page.getByRole('button', { name: 'Combine Lines...' });
    await expect(item).toBeEnabled();
    await expect(item).toContainText('3 lines end here');
    await expect(page.getByRole('button', { name: 'Split Line Here' })).toHaveCount(0);
    // Hovering opens the submenu: every pair of the three lines ending here.
    await item.hover();
    const submenu = page.getByTestId('context-menu-submenu');
    await expect(submenu).toBeVisible();
    await expect(submenu.getByRole('button')).toHaveText([/^Trunk with Left/, /^Trunk with Right/, /^Left with Right/]);
    await expect(submenu.getByRole('button', { name: 'Trunk with Left' })).toContainText('Joined here');
    await submenu.getByRole('button', { name: 'Trunk with Right' }).click();

    // The dialog opens on that pair, with no picker of its own.
    await expect(page.getByTestId('combine-lines-dialog')).toBeVisible();
    await expect(page.getByTestId('combine-partner-picker')).toHaveCount(0);
    await openDetails(page);
    await expect(page.getByTestId('combine-branches-note')).toContainText('Left stays attached at the join');
    await expect(page.getByTestId('combine-summary')).toContainText('Combining Trunk (2 shots) and Right (3 shots)');
    await page.getByTestId('combine-confirm').click();
    await waitForNoDialogs(page);
    const call = await page.evaluate(() => {
      const calls = ((window as any).__MOCK_COMBINE_LINES_CALLS__ ?? []) as Array<any>;
      return calls[calls.length - 1] ?? null;
    });
    expect(call).toMatchObject({ kept: a!.id, other: c!.id });
  });

  test('combines two lines in a loop at their loop closure, listing the merge that becomes a closure', async ({ page }) => {
    const [east, west] = await seed(page, [
      { name: 'East', count: 2 },
      { name: 'West', count: 2 },
    ]);
    // West's end is merged to East's start; East's end and West's start
    // close the loop with a loop closure.
    await merge(page, { line_id: east!.id, station_index: 0 }, { line_id: west!.id, station_index: 2 });
    const closed = await page.evaluate(
      ({ a, b }) => (window as any).__E2E_TEST__.projectStore.getState().createLoopClosure(a, b, 'Batch'),
      { a: { line_id: east!.id, station_index: 2 }, b: { line_id: west!.id, station_index: 0 } }
    );
    expect(closed).toMatchObject({ success: true });

    await openContextMenu(page, { type: 'station', ref: { line_id: east!.id, station_index: 2 } });
    await expect(page.getByRole('button', { name: 'Combine Line With West...' })).toContainText(
      'Across a loop closure'
    );
    await combineWith(page, 'West');

    await expect(page.getByTestId('combine-closure-join')).toContainText('East and West meet here across a loop closure');
    // The torn merge stays as a closure: nothing is removed (#746).
    await expect(page.getByTestId('combine-removed-summary')).toHaveCount(0);
    await openDetails(page);
    await expect(page.getByTestId('combine-note-torn')).toContainText('This connection becomes a loop closure:');
    await expect(page.locator('[data-testid^="combine-torn-tie-"]')).toHaveCount(1);
    await expect(page.locator('[data-testid^="combine-torn-tie-"]')).toContainText('The merge of');
    // Each end of the tie is a line pill and a station pill.
    const tie = page.locator('[data-testid^="combine-torn-tie-"]');
    await expect(tie.getByTestId(`combine-tie-line-${east!.id}`)).toBeVisible();
    await expect(tie.getByTestId(`combine-tie-station-${east!.id}-0`)).toBeVisible();
    await expect(tie.getByTestId(`combine-tie-line-${west!.id}`)).toBeVisible();
    await expect(tie.getByTestId(`combine-tie-station-${west!.id}-2`)).toBeVisible();
    await page.getByTestId('combine-confirm').click();
    await waitForNoDialogs(page);

    const call = await page.evaluate(() => {
      const calls = ((window as any).__MOCK_COMBINE_LINES_CALLS__ ?? []) as Array<any>;
      return calls[calls.length - 1] ?? null;
    });
    expect(call).toMatchObject({ kept: east!.id, other: west!.id, options: { join: { kept: 'end', other: 'start' } } });
    expect(await lineShape(page)).toEqual([['East', 4]]);
    const ties = await page.evaluate(() => {
      const project = (window as any).__E2E_TEST__.projectStore.getState().project;
      return { merges: project.station_merges.length, closures: project.loop_closures.length };
    });
    expect(ties).toEqual({ merges: 0, closures: 1 });
  });

  test('lists the merge it removes across one shot before the confirm, then says so in a toast (#746)', async ({ page }) => {
    // Main's end and Beyond's start are tied by a loop closure (the join);
    // the one-shot Squeeze is merged to both. Combined, Main's end and
    // Beyond's start are one point, so the merge of Squeeze's end and
    // Beyond's start would be a loop closure across Squeeze's one shot.
    const [main, squeeze, beyond] = await seed(page, [
      { name: 'Main', count: 1 },
      { name: 'Squeeze', count: 1 },
      { name: 'Beyond', count: 1 },
    ]);
    const mainEnd = { line_id: main!.id, station_index: 1 };
    const beyondStart = { line_id: beyond!.id, station_index: 0 };
    await merge(page, mainEnd, { line_id: squeeze!.id, station_index: 0 });
    await merge(page, { line_id: squeeze!.id, station_index: 1 }, beyondStart);
    const closed = await page.evaluate(
      ({ a, b }) => (window as any).__E2E_TEST__.projectStore.getState().createLoopClosure(a, b, 'Batch'),
      { a: mainEnd, b: beyondStart }
    );
    expect(closed).toMatchObject({ success: true });

    await openContextMenu(page, { type: 'station', ref: mainEnd });
    await page.getByRole('button', { name: 'Combine Lines...' }).hover();
    await page.getByTestId('context-menu-submenu').getByRole('button', { name: 'Main with Beyond' }).click();
    await expect(page.getByTestId('combine-lines-dialog')).toBeVisible();

    // Said before the confirm with Details still folded.
    await expect(page.getByTestId('combine-details')).not.toHaveAttribute('open', '');
    await expect(page.getByTestId('combine-removed-summary')).toBeVisible();
    await expect(page.getByTestId('combine-removed-summary')).toHaveText(
      'Combining removes 1 connection that would join the two ends of one shot (see Details).'
    );
    await openDetails(page);
    const removed = page.locator('[data-testid^="combine-removed-tie-"]');
    await expect(removed).toHaveCount(1);
    await expect(removed).toContainText(': as a loop closure it would join the two ends of one shot.');
    await expect(page.getByTestId('combine-note-removed')).toContainText('This connection is removed:');
    await expect(removed.getByTestId(`combine-tie-station-${squeeze!.id}-1`)).toBeVisible();
    await expect(removed.getByTestId(`combine-tie-station-${beyond!.id}-0`)).toBeVisible();
    await expect(page.getByTestId('combine-removed-why')).toContainText(
      'Such a closure would squeeze that shot to zero length'
    );
    // Not also listed as a connection that becomes a loop closure.
    await expect(page.getByTestId('combine-note-torn')).toHaveCount(0);
    await page.getByTestId('combine-confirm').click();
    await waitForNoDialogs(page);

    const toast = page.getByTestId('toast').filter({
      hasText: `Removed the merge of Squeeze ${squeeze!.id}:1 and Beyond ${beyond!.id}:0: combining the lines left its two stations at the two ends of one shot.`,
    });
    await expect(toast).toHaveCount(1);
    expect(await lineShape(page)).toEqual([
      ['Main', 2],
      ['Squeeze', 1],
    ]);
    const merges = () =>
      page.evaluate(() => (window as any).__E2E_TEST__.projectStore.getState().project.station_merges.length as number);
    expect(await merges()).toBe(1);

    // One undo brings Beyond back with its merge.
    await undo(page);
    await expect.poll(() => lineShape(page)).toEqual([
      ['Main', 1],
      ['Squeeze', 1],
      ['Beyond', 1],
    ]);
    expect(await merges()).toBe(2);
  });

  test('keeps its row, disabled, where no other line ends; none on a mid station', async ({ page }) => {
    const [main] = await seed(page, [{ name: 'Main', count: 3 }]);
    await openContextMenu(page, { type: 'station', ref: { line_id: main!.id, station_index: 3 } });
    const item = page.getByRole('button', { name: 'Combine Lines...' });
    await expect(item).toBeDisabled();
    await expect(item).toHaveAttribute('title', 'No other line ends at this station');
    await expect(item).toContainText('No other line ends at this station');

    await page.keyboard.press('Escape');
    await openContextMenu(page, { type: 'station', ref: { line_id: main!.id, station_index: 1 } });
    await expect(page.getByRole('button', { name: 'Edit Station' })).toBeVisible();
    await expect(page.getByRole('button', { name: /^Combine Line/ })).toHaveCount(0);
  });
});
