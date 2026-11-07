import { test, expect, type Page, type Locator } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapReady,
  waitForNoDialogs,
  openAppMenu,
  type SectionSeed,
} from './helpers/app';
import type { E2EWindow } from './helpers/e2eWindow';

/**
 * E2E tests for the data table's fullscreen mode.
 *
 * The expand button in the table's sub-header turns the DataTable into a
 * window-covering overlay below the main toolbar. The toolbar swaps to a
 * reduced variant: Close/Save/Undo/Redo stay, Import/Export gray out (and
 * their shortcuts go dead), the tool buttons + display menu give way to a
 * table picker and a "Map view" shrink button, and the center shows the
 * table's title. Escape and the toggle_data_table shortcut (Ctrl+3 in the
 * mock config) exit the mode without closing the table; Escape closes an
 * open dialog first. While fullscreen only save/undo/redo shortcuts stay
 * alive - the Sections view itself remains editable.
 *
 * The table picker groups "Sections" (the editable table) and "Raw data" -
 * read-only grids over the raw sqlite tables (mocked get_raw_table_names /
 * get_raw_table; blobs arrive as "<blob N KB>" placeholders). The raw view
 * shows last-saved state, so an "As of last save" hint appears while the
 * project is dirty and a save clears it (and refetches the page).
 */

const sectionSeeds: SectionSeed[] = [
  {
    name: 'Fullscreen Test Section',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 45, length: 10, depth_in: 0, depth_out: -1 },
      { azimuth_in: 90, azimuth_out: 90, length: 20, depth_in: -1, depth_out: -2 },
      { azimuth_in: 135, azimuth_out: 135, length: 30, depth_in: -2, depth_out: -3 },
    ],
  },
];

async function getIsDirty(page: Page): Promise<boolean> {
  return page.evaluate(() => {
    const e2e = (window as E2EWindow).__E2E_TEST__;
    return e2e.projectStore.getState().isDirty as boolean;
  });
}

async function getActiveTool(page: Page): Promise<string> {
  return page.evaluate(() => {
    const e2e = (window as E2EWindow).__E2E_TEST__;
    return e2e.uiStore.getState().activeTool as string;
  });
}

/** Open the DataTable via keyboard shortcut (auto-selects the first section). */
async function openDataTable(page: Page): Promise<void> {
  await page.keyboard.press('Control+3');
  await expect(page.getByTestId('data-table')).toBeVisible({ timeout: 3000 });
  await expect(page.getByTestId('grid-row-0')).toBeVisible({ timeout: 5000 });
}

/** Click the sub-header expand button and wait for the fullscreen overlay. */
async function enterFullScreen(page: Page): Promise<void> {
  await page.getByTestId('data-table-expand-button').click();
  await expect(page.getByTestId('data-table')).toHaveAttribute('data-fullscreen', 'true');
  await expect(page.getByTestId('toolbar-data-table-title')).toBeVisible();
}

/** Assert the table is back to its windowed (non-fullscreen) state and open. */
async function expectWindowed(page: Page): Promise<void> {
  await expect(page.getByTestId('data-table')).toHaveAttribute('data-fullscreen', 'false');
  // The Dock only mounts the table when the panel is revealed, so its presence
  // == open (the table no longer carries a redundant data-open attribute).
  await expect(page.getByTestId('data-table')).toBeVisible();
  await expect(page.getByTestId('toolbar-data-table-title')).toHaveCount(0);
}

/** Double-click a cell, type a value, commit with Enter. */
async function fillCell(page: Page, row: number, colKey: string, value: string): Promise<void> {
  await page.getByTestId(`grid-cell-${row}-${colKey}`).dblclick();
  const editor = page.getByTestId('grid-cell-editor');
  await expect(editor).toBeVisible();
  await editor.fill(value);
  await page.keyboard.press('Enter');
  await expect(editor).toHaveCount(0);
}

/** Open the fullscreen-toolbar table picker and return its portaled listbox. */
async function openTablePicker(page: Page): Promise<Locator> {
  const trigger = page.getByTestId('table-select-dropdown');
  await trigger.click();
  const listboxId = await trigger.getAttribute('aria-controls');
  expect(listboxId).toBeTruthy();
  const listbox = page.locator(`[id="${listboxId}"]`);
  await expect(listbox).toBeVisible();
  return listbox;
}

/** Pick a view in the fullscreen toolbar picker ('Sections' | 'Raw data'). */
async function pickView(page: Page, optionName: string): Promise<void> {
  const listbox = await openTablePicker(page);
  await listbox.getByRole('option', { name: optionName }).click();
  await expect(listbox).not.toBeVisible();
}

/** Pick a raw table via the sub-header menu next to the table-name pill. */
async function pickRawTable(page: Page, name: string): Promise<void> {
  await page.getByTestId('data-table-raw-table-menu').click();
  const list = page.getByTestId('data-table-raw-table-menu-list');
  await expect(list).toBeVisible();
  await list.getByRole('button', { name, exact: true }).click();
  await expect(list).not.toBeVisible();
}

test.describe('Data table fullscreen mode', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Fullscreen Test');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapReady(page);
    await waitForNoDialogs(page);
    await openDataTable(page);
  });

  test('expand button enters; Map view button and Escape exit', async ({ page }) => {
    await expect(page.getByTestId('data-table')).toHaveAttribute('data-fullscreen', 'false');

    await enterFullScreen(page);
    // The grid keeps rendering inside the overlay; the windowed-only chrome
    // (expand button, collapse chevron) is hidden while fullscreen.
    await expect(page.getByTestId('grid-row-0')).toBeVisible();
    await expect(page.getByTestId('data-table-expand-button')).toHaveCount(0);
    // Fullscreen renders no opener tab, so the windowed close/collapse control is gone.
    await expect(page.getByTestId('panel-close-data')).toHaveCount(0);

    // Shrink back via the toolbar's "Map view" button.
    await page.getByTestId('toolbar-map-view-button').click();
    await expectWindowed(page);
    await expect(page.getByTestId('data-table-expand-button')).toBeVisible();
    // Back in the windowed (pinned-open) table, the collapse control returns.
    await expect(page.getByTestId('panel-close-data')).toBeVisible();

    // Re-enter, exit via Escape.
    await enterFullScreen(page);
    await page.keyboard.press('Escape');
    await expectWindowed(page);
  });

  test('Escape closes an open dialog before exiting fullscreen', async ({ page }) => {
    await enterFullScreen(page);

    // Open a dialog (Debug Logs, via the hamburger menu, which stays in the
    // reduced toolbar) to verify Escape closes it before exiting fullscreen.
    await openAppMenu(page);
    await page.getByTestId('global-debug-button').click();
    await expect(page.getByTestId('debug-dialog')).toBeVisible();

    // First Escape closes the dialog and stays fullscreen.
    await page.keyboard.press('Escape');
    await expect(page.getByTestId('debug-dialog')).toHaveCount(0);
    await expect(page.getByTestId('data-table')).toHaveAttribute('data-fullscreen', 'true');

    // Second Escape exits the mode.
    await page.keyboard.press('Escape');
    await expectWindowed(page);
  });

  test('toggle_data_table shortcut exits fullscreen but keeps the table open', async ({ page }) => {
    await enterFullScreen(page);

    // The toggle action exits the mode instead of closing the table.
    await page.keyboard.press('Control+3');
    await expectWindowed(page);

    // Outside fullscreen the same shortcut toggles the table again - closing
    // unmounts the DataTable in favor of the closed-state opener tab.
    await page.keyboard.press('Control+3');
    await expect(page.getByTestId('panel-tab-data')).toBeVisible();
    await expect(page.getByTestId('data-table')).toHaveCount(0);
  });

  test('reduced toolbar: tools give way to the table picker, Import button drops out', async ({ page }) => {
    await enterFullScreen(page);

    // Center title instead of the workspace carousel.
    await expect(page.getByTestId('toolbar-data-table-title')).toHaveText('Data table');
    await expect(page.getByTestId('mode-carousel')).toHaveCount(0);

    // Map tools and the display menu are gone; only the table picker remains on the right.
    await expect(page.getByTestId('toolbar-tools-group')).toHaveCount(0);
    await expect(page.getByTestId('display-menu-button')).toHaveCount(0);
    await expect(page.getByTestId('table-select-dropdown')).toBeVisible();

    // The hamburger menu (Save/Export/Close/Debug live there) stays; the Import
    // button is replaced by the "Back to Map" button; Undo/Redo stay.
    await expect(page.getByTestId('app-menu-trigger')).toBeVisible();
    await expect(page.getByTestId('toolbar-undo-button')).toBeVisible();
    await expect(page.getByTestId('toolbar-redo-button')).toBeVisible();
    await expect(page.getByTestId('toolbar-import-button')).toHaveCount(0);
    // The "Back to Map" button lives on the left, in place of Import.
    await expect(page.getByTestId('toolbar-map-view-button')).toHaveText('Back to Map');
    await expect(
      page.getByTestId('toolbar-file-controls').getByTestId('toolbar-map-view-button'),
    ).toBeVisible();

    // Exiting restores the normal toolbar.
    await page.keyboard.press('Escape');
    await expectWindowed(page);
    await expect(page.getByTestId('toolbar-tools-group')).toBeVisible();
    await expect(page.getByTestId('display-menu-button')).toBeVisible();
    await expect(page.getByTestId('table-select-dropdown')).toHaveCount(0);
    await expect(page.getByTestId('toolbar-map-view-button')).toHaveCount(0);
    await expect(page.getByTestId('app-menu-trigger')).toBeVisible();
    await expect(page.getByTestId('toolbar-import-button')).toBeEnabled();
  });

  test('blocks tool and import/export shortcuts but keeps save/undo alive', async ({ page }) => {
    await enterFullScreen(page);

    // Tool shortcuts are dead: M (merge) must not switch the active tool.
    // Deliberately not P: the pan tool is hidden by default (#409) and its
    // shortcut is inert everywhere, so a P probe would pass vacuously whether
    // or not fullscreen blocked anything.
    expect(await getActiveTool(page)).toBe('select');
    await page.keyboard.press('m');
    // settle: proves the shortcut is dead (the tool does NOT change); a poll
    // would accept the pre-update value.
    await page.waitForTimeout(300);
    expect(await getActiveTool(page)).toBe('select');

    // Ctrl+I (import) and Ctrl+E (export) are dead: no dialog appears and
    // the mode stays active.
    await page.keyboard.press('Control+i');
    // settle: proves no import dialog opens; its absence is true on the first
    // read whether or not the shortcut fired.
    await page.waitForTimeout(300);
    await expect(page.getByTestId('dmp-selection-dialog')).toHaveCount(0);
    await page.keyboard.press('Control+e');
    // settle: proves no export dialog opens; its absence is true on the first
    // read whether or not the shortcut fired.
    await page.waitForTimeout(300);
    await expect(page.getByTestId('export-dialog')).toHaveCount(0);
    await expect(page.getByTestId('data-table')).toHaveAttribute('data-fullscreen', 'true');

    // Ctrl+S is alive: the save clears the dirty flag the seeding left. This clean
    // baseline is also what makes the edit below observable: from a dirty start the
    // dirty poll passed at once, before the edit had landed, so Ctrl+S could save
    // first and the late edit re-dirty the project (#576 NF-W12DMP-4).
    await page.keyboard.press('Control+s');
    await expect.poll(() => getIsDirty(page)).toBe(false);

    // The fullscreen Sections view stays editable: the flag turns true only once
    // the edit has landed (it is set after the edit's state refresh).
    await fillCell(page, 0, 'length', '42');
    await expect.poll(() => getIsDirty(page)).toBe(true);

    // Ctrl+S again, now saving the edit.
    await page.keyboard.press('Control+s');
    await expect.poll(() => getIsDirty(page)).toBe(false);

    // Ctrl+Z is alive: a successful undo re-dirties the project.
    await page.keyboard.press('Control+z');
    await expect.poll(() => getIsDirty(page)).toBe(true);

    // Sanity for the dead-key assertions above: outside fullscreen the same
    // tool shortcut works.
    await page.keyboard.press('Escape');
    await expectWindowed(page);
    // Click a neutral, non-input toolbar element to move focus off the table so the
    // 'm' shortcut fires. The Survey carousel button is a no-op while already in Survey.
    await page.getByTestId('mode-survey').click();
    await page.keyboard.press('m');
    await expect.poll(() => getActiveTool(page)).toBe('merge');
  });

  test('raw data: picker groups, read-only grid, dirty hint, back to Sections', async ({ page }) => {
    await enterFullScreen(page);

    // Clean baseline so the dirty hint assertions below are deterministic.
    await page.keyboard.press('Control+s');
    await expect.poll(() => getIsDirty(page)).toBe(false);

    // The picker groups "Sections" and "Raw data" (raw = the sqlite tables).
    // The toolbar picker offers just the two views; tables are chosen in the
    // sub-header.
    const listbox = await openTablePicker(page);
    // (the gap between label and qualifier is flex spacing, not a text space)
    await expect(listbox.getByRole('option')).toHaveText(['Sections', 'Raw data(read-only)']);

    // Entering raw mode auto-selects a table; the sub-header menu lists them
    // all and switches to the shots table. No Shots|Stations toggle in raw
    // mode; the read-only grid shows the saved rows.
    await listbox.getByRole('option', { name: 'Raw data' }).click();
    await expect(page.getByTestId('data-table-raw-name')).not.toHaveText('');
    await page.getByTestId('data-table-raw-table-menu').click();
    const tableList = page.getByTestId('data-table-raw-table-menu-list');
    await expect(tableList.getByRole('button')).toHaveText(['project', 'sections', 'shots', 'undo_stack']);
    await tableList.getByRole('button', { name: 'shots', exact: true }).click();
    await expect(page.getByTestId('raw-table-view')).toBeVisible();
    await expect(page.getByTestId('data-table-raw-name')).toHaveText('shots');
    await expect(page.getByTestId('data-table-view-select')).toHaveCount(0);
    await expect(page.getByTestId('data-table-raw-dirty-hint')).toHaveCount(0);
    await expect(page.getByTestId('grid-cell-0-length')).toHaveText('10');

    // Read-only: double-click must not open the cell editor.
    await page.getByTestId('grid-cell-0-length').dblclick();
    await expect(page.getByTestId('grid-cell-editor')).toHaveCount(0);

    // Blob columns arrive as size placeholders.
    await pickRawTable(page, 'undo_stack');
    await expect(page.getByTestId('data-table-raw-name')).toHaveText('undo_stack');
    await expect(page.getByTestId('grid-cell-0-command_json')).toHaveText('<blob 1.2 KB>');

    // Back to Sections: the editable table with its normal sub-header.
    await pickView(page, 'Sections');
    await expect(page.getByTestId('raw-table-view')).toHaveCount(0);
    await expect(page.getByTestId('data-table-raw-name')).toHaveCount(0);
    await expect(page.getByTestId('data-table-view-select')).toBeVisible();

    // Dirty the project, revisit the raw view: the "as of last save" hint
    // shows until a save clears it (the page refetches on dirty → clean).
    await fillCell(page, 0, 'length', '42');
    await expect.poll(() => getIsDirty(page)).toBe(true);
    await pickView(page, 'Raw data');  // matches despite the (read-only) suffix
    await pickRawTable(page, 'shots');
    await expect(page.getByTestId('data-table-raw-dirty-hint')).toHaveText('As of last save');
    await page.keyboard.press('Control+s');
    await expect(page.getByTestId('data-table-raw-dirty-hint')).toHaveCount(0);
    await expect(page.getByTestId('grid-cell-0-length')).toHaveText('42');
  });
});
