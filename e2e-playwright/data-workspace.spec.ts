/**
 * E2E: the Data workspace - one spreadsheet with every section's stations and
 * shots interleaved in survey order.
 *
 * Covers entering Data, the interleaved rows and their IDs, the section
 * headers in the gutter and the header corner, editing a shot length and a
 * station comment (and undoing both), a link chip jumping to the merge
 * partner, "Show on map" opening the map pane, "Open in Survey" with the
 * Back-to-Data chip, and "Open in Data" from Survey's docked table.
 *
 * Fully synthetic project: three sections, a merge (Main:2 = Branch B:0) and
 * a loop closure (Main:5 -> Sump:3).
 *
 * Parallel-safe: all state is per-page (mocked IPC + `window.__E2E_TEST__` stores).
 */
import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage, createProject, seedProjectWithSections, type SectionSeed } from './helpers/app';

type StationRef = { section_id: number; station_index: number };
type Stores = {
  __E2E_TEST__: {
    projectStore: {
      getState: () => {
        project: { sections: Array<{ id: number; shots: Array<{ id: number; length: number }> }> } | null;
        stations: Array<{ station_ref: StationRef; comment?: string }>;
        mergeStations: (a: StationRef, b: StationRef) => Promise<unknown>;
        createLoopClosure: (a: StationRef, b: StationRef, strategy: string) => Promise<unknown>;
        undo: () => Promise<unknown>;
      };
    };
    uiStore: { getState: () => { appMode: string; selection: unknown; setSelection: (s: unknown) => void } };
    dataWorkspaceStore: { getState: () => { activeRowKey: string | null; mapPaneOpen: boolean; filters: { sectionIds: number[] | null } } };
  };
};

const shot = (az: number, length: number, d1: number, d2: number, extra: Partial<SectionSeed['shots'][number]> = {}) => ({
  azimuth_in: az,
  azimuth_out: az,
  length,
  depth_in: d1,
  depth_out: d2,
  left: 0.5,
  right: 1,
  up: 1.5,
  down: 0.25,
  ...extra,
});

const SEEDS: SectionSeed[] = [
  { name: 'Main', shots: [shot(45, 8, 0, 1), shot(70, 6.5, 1, 2), shot(110, 11, 2, 2), shot(180, 7, 2, 2), shot(250, 9, 2, 1)] },
  { name: 'Branch B', shots: [shot(10, 4.5, 2, 1), shot(355, 6, 1, 0, { kind: 'Jump' })] },
  { name: 'Sump', shots: [shot(90, 5, 1, 2), shot(95, 3, 2, 3), shot(100, 9, 3, 4)] },
];

async function sectionIds(page: Page): Promise<number[]> {
  return page.evaluate(() => (window as unknown as Stores).__E2E_TEST__.projectStore.getState().project!.sections.map((s) => s.id));
}

async function seed(page: Page): Promise<number[]> {
  await setupMockedPage(page);
  await createProject(page, 'Synthetic data workspace');
  await seedProjectWithSections(page, SEEDS);
  const [main, branch, sump] = await sectionIds(page);
  await page.evaluate(
    async ({ main, branch, sump }) => {
      const st = (window as unknown as Stores).__E2E_TEST__.projectStore.getState();
      await st.mergeStations({ section_id: main!, station_index: 2 }, { section_id: branch!, station_index: 0 });
      await (window as unknown as Stores).__E2E_TEST__.projectStore
        .getState()
        .createLoopClosure({ section_id: main!, station_index: 5 }, { section_id: sump!, station_index: 3 }, 'batch');
    },
    { main, branch, sump },
  );
  return [main!, branch!, sump!];
}

async function enterData(page: Page): Promise<void> {
  await page.getByTestId('mode-data').click();
  await expect(page.getByTestId('data-workspace')).toBeVisible();
}

const activeRowKey = (page: Page) =>
  page.evaluate(() => (window as unknown as Stores).__E2E_TEST__.dataWorkspaceStore.getState().activeRowKey);
const appMode = (page: Page) => page.evaluate(() => (window as unknown as Stores).__E2E_TEST__.uiStore.getState().appMode);

async function editCell(page: Page, testId: string, value: string): Promise<void> {
  await page.getByTestId(testId).dblclick();
  const editor = page.getByTestId('data-grid-cell-editor');
  await editor.fill(value);
  await editor.press('Enter');
}

test.describe('Data workspace', () => {
  test('shows every section interleaved, with row names', async ({ page }) => {
    const [main, branch] = await seed(page);
    await enterData(page);
    const ids = await page
      .locator('[data-testid^="data-row-st-"], [data-testid^="data-row-sh-"]')
      .evaluateAll((els) => els.map((el) => el.getAttribute('data-testid')));
    expect(ids.slice(0, 5)).toEqual([
      `data-row-st-${main}-0`,
      `data-row-sh-${main}-1`,
      `data-row-st-${main}-1`,
      `data-row-sh-${main}-2`,
      `data-row-st-${main}-2`,
    ]);
    expect(ids).toContain(`data-row-st-${branch}-0`);
    // Row names are the canonical IDs only; the section is named by its header.
    await expect(page.getByTestId(`data-row-name-st-${main}-1`)).toHaveText(new RegExp(`^${main}:1`));
    await expect(page.getByTestId(`data-row-name-sh-${main}-2`)).toHaveText(new RegExp(`^${main}:2`));
    // The junction (Main:2) draws a ring on the rail; a Jump shot a dashed line.
    await expect(page.getByTestId(`data-row-st-${main}-2`).locator('.dw-rail-ring')).toHaveCount(1);
    await expect(page.getByTestId(`data-row-sh-${branch}-2`).locator('.dw-rail-line-dashed')).toHaveCount(2);
    await expect(page.getByTestId('data-status-counts')).toContainText('23 rows');
    // Station depth starts hidden; Links and Issues are no columns any more.
    for (const key of ['depth', 'links', 'issues']) {
      await expect(page.getByTestId(`data-grid-header-${key}`)).toHaveCount(0);
    }
    await expect(page.getByTestId('data-grid-header-length')).toBeVisible();
  });

  // Needs the grid's groupContentStart / headerCorner (SpreadsheetGrid).
  test('section headers start in the gutter; the header corner names a scrolled-away section', async ({ page }) => {
    const [main] = await seed(page);
    await enterData(page);
    const header = page.getByTestId(`data-section-header-${main}`);
    await expect(header).toContainText('Main');
    await expect(header).toContainText(`#${main}`);
    // The header's name starts in the gutter, left of the first column.
    const headerBox = await header.boundingBox();
    const firstColumn = await page.getByTestId('data-grid-header-length').boundingBox();
    expect(headerBox!.x).toBeLessThan(firstColumn!.x);
    // Its header on screen: nothing in the corner.
    await expect(page.getByTestId(`data-section-corner-${main}`)).toHaveCount(0);
    await page.locator('[data-testid="data-workspace"] [role="grid"]').evaluate((el) => {
      el.scrollTop = 33 * 4;
    });
    const corner = page.getByTestId(`data-section-corner-${main}`);
    await expect(corner).toBeVisible();
    await expect(corner).toContainText('Main');
    await expect(corner).toContainText(`#${main}`);
  });

  test('edits a shot length and a station comment, and undoes both', async ({ page }) => {
    // The mock records cell edits as undo steps only on request.
    await page.addInitScript(() => {
      (window as unknown as Record<string, unknown>).__E2E_MOCK_UNDO_EDITS__ = true;
    });
    const [main] = await seed(page);
    await enterData(page);
    const length = () =>
      page.evaluate(() => (window as unknown as Stores).__E2E_TEST__.projectStore.getState().project!.sections[0]!.shots[0]!.length);
    const comment = (idx: number) =>
      page.evaluate(
        ({ main, idx }) =>
          (window as unknown as Stores).__E2E_TEST__.projectStore
            .getState()
            .stations.find((s) => s.station_ref.section_id === main && s.station_ref.station_index === idx)?.comment ?? null,
        { main, idx },
      );

    await editCell(page, `data-cell-sh-${main}-1-length`, '12.5');
    await expect.poll(length).toBe(12.5);
    await expect(page.getByTestId(`data-cell-sh-${main}-1-length`)).toHaveText('12.5');

    await editCell(page, `data-cell-st-${main}-3-comment`, 'Big room');
    await expect.poll(() => comment(3)).toBe('Big room');

    await page.keyboard.press('Control+z');
    await expect.poll(() => comment(3)).toBeNull();
    await page.keyboard.press('Control+z');
    await expect.poll(length).toBe(8);
  });

  test('a merge chip jumps to the partner row', async ({ page }) => {
    const [main, branch] = await seed(page);
    await enterData(page);
    // Needs the grid's spanCells: the chips span the station's shot-only cells.
    const chip = page.getByTestId(`data-chip-merge-st-${main}-2-${branch}-0`);
    await expect(chip).toContainText('Branch B:0');
    // The station name is bold, the prose is not.
    await expect(chip.locator('.font-semibold')).toHaveText('Branch B:0');
    await chip.click();
    await expect.poll(() => activeRowKey(page)).toBe(`st:${branch}:0`);
    await expect(page.getByTestId(`data-row-st-${branch}-0`)).toBeInViewport();
    // The branch start names its parent; the closure carries its misclosure.
    await expect(page.getByTestId(`data-chip-branch-st-${branch}-0-${main}-2`)).toContainText('Main:2');
    await expect(page.locator(`[data-testid^="data-chip-misclosure-st-${main}-5-"]`)).toContainText('%');
  });

  test('"Show on map" opens the map pane on the row', async ({ page }) => {
    const [main] = await seed(page);
    await enterData(page);
    await page.getByTestId(`data-row-st-${main}-3`).hover();
    await page.getByTestId(`data-row-map-st-${main}-3`).click();
    await expect(page.getByTestId('data-map-pane-header')).toBeVisible();
    await expect
      .poll(() => page.evaluate(() => (window as unknown as Stores).__E2E_TEST__.uiStore.getState().selection))
      .toEqual({ type: 'station', stationRef: { section_id: main, station_index: 3 } });
    await page.getByTestId('data-map-pane-close').click();
    await expect(page.getByTestId('data-map-pane-header')).toHaveCount(0);
  });

  test('"Open in Survey" leaves a Back to Data chip that returns to the row', async ({ page }) => {
    const [main] = await seed(page);
    await enterData(page);
    await page.getByTestId(`data-cell-sh-${main}-3-length`).click();
    await page.keyboard.press('Alt+s');
    await expect.poll(() => appMode(page)).toBe('survey');
    const chip = page.getByTestId('back-to-data');
    // The row ID the gutter shows, with the section for context.
    await expect(chip).toContainText(`${main}:3 (Main)`);
    await chip.click();
    await expect.poll(() => appMode(page)).toBe('data');
    await expect.poll(() => activeRowKey(page)).toMatch(new RegExp(`^sh:${main}:`));
    await expect(page.getByTestId('back-to-data-chip')).toHaveCount(0);
  });

  test('"Open in Data" from Survey\'s table opens Data filtered to the section', async ({ page }) => {
    const [, branch] = await seed(page);
    await page.evaluate((branch) => {
      (window as unknown as Stores).__E2E_TEST__.uiStore.getState().setSelection({ type: 'station', stationRef: { section_id: branch, station_index: 1 } });
    }, branch);
    await page.keyboard.press('Control+3');
    await page.getByTestId('data-table-open-in-data').click();
    await expect(page.getByTestId('data-workspace')).toBeVisible();
    await expect
      .poll(() => page.evaluate(() => (window as unknown as Stores).__E2E_TEST__.dataWorkspaceStore.getState().filters.sectionIds))
      .toEqual([branch]);
    await expect.poll(() => activeRowKey(page)).toBe(`st:${branch}:1`);
    await expect(page.locator('[data-testid^="data-row-st-"]').first()).toHaveAttribute('data-testid', `data-row-st-${branch}-0`);
  });
});
