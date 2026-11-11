import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapReady,
  waitForNoDialogs,
  type SectionSeed,
} from './helpers/app';

/**
 * E2E tests for the data table's stations view (SpreadsheetGrid migration).
 *
 * Stations are derived from shots, so X/Y/Depth are read-only and only the
 * comment is editable (one set_station_comment per commit - no batch). The
 * station ID pill lives in the row gutter ("1:0"); the GPS cell opens the
 * Edit Station dialog: add-zero-datum mode (GPS input enabled + focused) for
 * plain stations, plain edit mode when the station already is a zero datum.
 * The eye menu and the header right-click menu show plain per-column hide
 * entries (no angle modes).
 *
 * Rows are grid-row-{rowIndex}, cells grid-cell-{rowIndex}-{colKey}; the GPS
 * cell's click target is station-gps-{sectionId}-{stationIndex}. The mock's
 * set_station_comment stores comments that get_project_state threads back
 * into its derived station array.
 */

const sectionSeeds: SectionSeed[] = [
  {
    name: 'Stations Test Section',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 45, length: 10, depth_in: 0, depth_out: -1 },
      { azimuth_in: 90, azimuth_out: 90, length: 20, depth_in: -1, depth_out: -2 },
      { azimuth_in: 135, azimuth_out: 135, length: 30, depth_in: -2, depth_out: -3 },
    ],
  },
];

async function getSectionId(page: Page): Promise<number> {
  return page.evaluate(() => {
    const e2e = (window as any).__E2E_TEST__;
    return e2e.projectStore.getState().project.sections[0].id as number;
  });
}

async function getStationComment(page: Page, sectionId: number, stationIndex: number): Promise<string | null> {
  return page.evaluate(
    ({ sectionId, stationIndex }) => {
      const e2e = (window as any).__E2E_TEST__;
      const station = e2e.projectStore
        .getState()
        .stations.find(
          (s: any) =>
            s.station_ref.section_id === sectionId && s.station_ref.station_index === stationIndex
        );
      return (station?.comment as string | undefined) ?? null;
    },
    { sectionId, stationIndex }
  );
}

async function setZeroDatum(page: Page, sectionId: number, stationIndex: number): Promise<void> {
  await page.evaluate(
    async ({ sectionId, stationIndex }) => {
      const e2e = (window as any).__E2E_TEST__;
      await e2e.projectStore.getState().setZeroDatum(
        { section_id: sectionId, station_index: stationIndex },
        { latitude: 24.55, longitude: -81.78, depth_reference: 0 },
        { fitToBounds: false }
      );
    },
    { sectionId, stationIndex }
  );
}

/** Open the DataTable and switch to the stations view. */
async function openStationsView(page: Page): Promise<void> {
  await page.keyboard.press('Control+3');
  await expect(page.getByTestId('data-table')).toBeVisible({ timeout: 3000 });
  await page.getByTestId('data-table-view-stations').click();
  await expect(page.getByTestId('data-table-view-stations')).toHaveAttribute('aria-checked', 'true');
  // 3 shots → 4 stations; the Depth column marks the stations grid (the
  // coordinate pairs start hidden).
  await expect(page.getByTestId('grid-header-_depth')).toBeVisible({ timeout: 5000 });
  await expect(page.getByTestId('grid-row-3')).toBeVisible();
}

test.describe('Stations grid', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Stations Grid Test');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapReady(page);
    await waitForNoDialogs(page);
    await openStationsView(page);
  });

  test('gutter shows station ID pills; X/Y/Depth are read-only', async ({ page }) => {
    const sectionId = await getSectionId(page);

    // The index pill moved into the row gutter ("1:0" ... "1:3").
    await expect(page.getByTestId('grid-gutter-0')).toHaveText(`${sectionId}:0`);
    await expect(page.getByTestId('grid-gutter-3')).toHaveText(`${sectionId}:3`);

    // X/Y start hidden; the "Coordinates (m)" eye-menu entry shows the pair.
    await expect(page.getByTestId('grid-header-_x')).toHaveCount(0);
    await page.getByTestId('data-table-columns-menu').click();
    await page.getByTestId('data-table-columns-dropdown').getByTestId('col-vis-_x').check();
    await page.keyboard.press('Escape');

    // Coordinates from the mock (x = index * 10, 2 decimals; depth 1 decimal).
    await expect(page.getByTestId('grid-cell-1-_x')).toHaveText('10.00');
    await expect(page.getByTestId('grid-cell-2-_x')).toHaveText('20.00');
    await expect(page.getByTestId('grid-cell-0-_depth')).toHaveText('0.0');

    // X/Y/Depth never open the cell editor.
    for (const colKey of ['_x', '_y', '_depth']) {
      await page.getByTestId(`grid-cell-1-${colKey}`).dblclick();
      await expect(page.getByTestId('grid-cell-editor')).toHaveCount(0);
    }
    // The comment cell does (sanity check that editing works at all here).
    await page.getByTestId('grid-cell-1-comment').dblclick();
    await expect(page.getByTestId('grid-cell-editor')).toBeVisible();
    await page.keyboard.press('Escape');
  });

  test('the name column shows each station name and is read-only', async ({ page }) => {
    const sectionId = await getSectionId(page);
    // Names as a DMP import records them (mNemo station numbers); station 2 has none.
    await page.evaluate(async (sectionId) => {
      const setName = (window as any).__MOCK_SET_STATION_NAME__;
      setName(sectionId, 0, '7');
      setName(sectionId, 1, '8');
      setName(sectionId, 3, '10');
      await (window as any).__E2E_TEST__.projectStore.getState().refreshProjectState();
    }, sectionId);

    // Hidden by default; the eye menu shows it.
    await expect(page.getByTestId('grid-header-name')).toHaveCount(0);
    await page.getByTestId('data-table-columns-menu').click();
    await page.getByTestId('data-table-columns-dropdown').getByTestId('col-vis-name').check();
    await page.keyboard.press('Escape');
    await expect(page.getByTestId('grid-header-name')).toBeVisible();
    await expect(page.getByTestId('grid-cell-0-name')).toHaveText('7');
    await expect(page.getByTestId('grid-cell-1-name')).toHaveText('8');
    await expect(page.getByTestId('grid-cell-2-name')).toHaveText('');
    await expect(page.getByTestId('grid-cell-3-name')).toHaveText('10');
    // The positional id stays in the gutter beside it.
    await expect(page.getByTestId('grid-gutter-1')).toHaveText(`${sectionId}:1`);

    await page.getByTestId('grid-cell-1-name').dblclick();
    await expect(page.getByTestId('grid-cell-editor')).toHaveCount(0);
  });

  test('comment edits commit via set_station_comment and survive the refresh', async ({ page }) => {
    const sectionId = await getSectionId(page);

    await page.getByTestId('grid-cell-0-comment').dblclick();
    const editor = page.getByTestId('grid-cell-editor');
    await expect(editor).toBeVisible();
    await editor.fill('Drop weight here');
    await page.keyboard.press('Enter');
    await expect(editor).toHaveCount(0);

    // The store refreshed from get_project_state and still shows the comment.
    await expect(page.getByTestId('grid-cell-0-comment')).toHaveText('Drop weight here');
    expect(await getStationComment(page, sectionId, 0)).toBe('Drop weight here');

    // Delete clears the comment (text column → null, no red "missing" state).
    await page.getByTestId('grid-cell-0-comment').click();
    await page.keyboard.press('Delete');
    await expect(page.getByTestId('grid-cell-0-comment')).toHaveText('');
    expect(await getStationComment(page, sectionId, 0)).toBeNull();
  });

  test('GPS cell click on a plain station opens add-zero-datum mode', async ({ page }) => {
    const sectionId = await getSectionId(page);

    await page.getByTestId(`station-gps-${sectionId}-1`).click();
    const dialog = page.getByTestId('edit-station-dialog');
    await expect(dialog).toBeVisible();

    // Add mode: the zero-datum checkbox is forced on and the GPS input is
    // focused, ready to paste coordinates.
    await expect(dialog.locator('#zeroDatum')).toBeChecked();
    const gpsInput = page.getByTestId('station-gps-input');
    await expect(gpsInput).toHaveAttribute('data-autofocus', 'true');
    await expect(gpsInput).toBeFocused();

    await dialog.getByRole('button', { name: 'Cancel' }).click();
    await expect(dialog).toHaveCount(0);
  });

  test('GPS cell click on a zero-datum station opens plain edit mode', async ({ page }) => {
    const sectionId = await getSectionId(page);
    await setZeroDatum(page, sectionId, 0);

    await page.getByTestId(`station-gps-${sectionId}-0`).click();
    const dialog = page.getByTestId('edit-station-dialog');
    await expect(dialog).toBeVisible();

    // Edit mode: the checkbox reflects the existing datum, the GPS input is
    // pre-filled with its coordinates and NOT autofocused.
    await expect(dialog.locator('#zeroDatum')).toBeChecked();
    const gpsInput = page.getByTestId('station-gps-input');
    await expect(gpsInput).toHaveValue(/24\.55/);
    await expect(gpsInput).not.toHaveAttribute('data-autofocus', 'true');
    await expect(gpsInput).not.toBeFocused();

    await dialog.getByRole('button', { name: 'Cancel' }).click();
    await expect(dialog).toHaveCount(0);
  });

  test('eye menu lists station columns and toggles their visibility', async ({ page }) => {
    await page.getByTestId('data-table-columns-menu').click();
    const dropdown = page.getByTestId('data-table-columns-dropdown');
    await expect(dropdown).toBeVisible();

    // Station entries, not the shots ones.
    // The station name starts hidden, like the coordinate pairs.
    await expect(dropdown.getByTestId('col-vis-name')).not.toBeChecked();
    await expect(dropdown.getByTestId('col-vis-_x')).toBeVisible();
    await expect(dropdown.getByTestId('col-vis-_zeroDatum')).toBeVisible();
    await expect(dropdown.getByTestId('col-vis-length')).toHaveCount(0);

    // The metric coordinate pair starts hidden and toggles together.
    await expect(dropdown.getByTestId('col-vis-_x')).not.toBeChecked();
    await expect(page.getByTestId('grid-header-_x')).toHaveCount(0);
    await dropdown.getByTestId('col-vis-_x').check();
    await expect(page.getByTestId('grid-header-_x')).toBeVisible();
    await expect(page.getByTestId('grid-header-_y')).toBeVisible();
    await dropdown.getByTestId('col-vis-_x').uncheck();
    await expect(page.getByTestId('grid-header-_x')).toHaveCount(0);
    await expect(page.getByTestId('grid-header-_y')).toHaveCount(0);
  });

  test('header right-click offers a plain hide entry', async ({ page }) => {
    // Show the metric pair first (hidden by default), then hide via the header:
    // X and Y hide together as the Coordinates (m) group.
    await page.getByTestId('data-table-columns-menu').click();
    await page.getByTestId('data-table-columns-dropdown').getByTestId('col-vis-_x').check();
    await page.keyboard.press('Escape');
    await expect(page.getByTestId('grid-header-_y')).toBeVisible();

    await page.getByTestId('grid-header-_y').click({ button: 'right' });
    const menu = page.getByTestId('data-table-header-menu');
    await expect(menu).toBeVisible();

    // No angle-mode entries in the stations view - just the group hide.
    await expect(menu.getByRole('button')).toHaveCount(1);
    await menu.getByRole('button', { name: 'Hide Coordinates (m)' }).click();
    await expect(page.getByTestId('grid-header-_y')).toHaveCount(0);
    await expect(page.getByTestId('grid-header-_x')).toHaveCount(0);
    await expect(page.getByTestId('grid-header-_depth')).toBeVisible();
  });
});
