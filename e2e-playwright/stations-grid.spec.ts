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
 * E2E tests for the data table's stations view (SpreadsheetGrid migration).
 *
 * Stations are derived from shots, so X/Y/Depth and the Station ID are
 * read-only; the name (set_station_name, unique on every line through the
 * station) and the comment (set_station_comment) are editable, one command
 * per commit - no batch. The station's name pill lives in the row gutter ("0"); never a
 * positional `line:index` id (SPEC 4.1). The GPS cell opens the
 * Edit Station dialog: add-zero-datum mode (GPS input enabled + focused) for
 * plain stations, plain edit mode when the station already is a zero datum.
 * The eye menu and the header right-click menu show plain per-column hide
 * entries (no angle modes).
 *
 * Rows are grid-row-{rowIndex}, cells grid-cell-{rowIndex}-{colKey}; the GPS
 * cell's click target is station-gps-{lineId}-{stationIndex}. The mock's
 * set_station_comment stores comments that get_project_state threads back
 * into its derived station array.
 */

const lineSeeds: LineSeed[] = [
  {
    name: 'Stations Test Line',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 45, length: 10, depth_in: 0, depth_out: -1 },
      { azimuth_in: 90, azimuth_out: 90, length: 20, depth_in: -1, depth_out: -2 },
      { azimuth_in: 135, azimuth_out: 135, length: 30, depth_in: -2, depth_out: -3 },
    ],
  },
];

async function getLineId(page: Page): Promise<number> {
  return page.evaluate(() => {
    const e2e = (window as any).__E2E_TEST__;
    return e2e.projectStore.getState().project.lines[0].id as number;
  });
}

async function getStationName(page: Page, lineId: number, stationIndex: number): Promise<string | null> {
  return page.evaluate(
    ({ lineId, stationIndex }) => {
      const e2e = (window as any).__E2E_TEST__;
      const station = e2e.projectStore
        .getState()
        .stations.find(
          (s: any) =>
            s.station_ref.line_id === lineId && s.station_ref.station_index === stationIndex
        );
      return (station?.name as string | undefined) ?? null;
    },
    { lineId, stationIndex }
  );
}

async function getStationComment(page: Page, lineId: number, stationIndex: number): Promise<string | null> {
  return page.evaluate(
    ({ lineId, stationIndex }) => {
      const e2e = (window as any).__E2E_TEST__;
      const station = e2e.projectStore
        .getState()
        .stations.find(
          (s: any) =>
            s.station_ref.line_id === lineId && s.station_ref.station_index === stationIndex
        );
      return (station?.comment as string | undefined) ?? null;
    },
    { lineId, stationIndex }
  );
}

async function setZeroDatum(page: Page, lineId: number, stationIndex: number): Promise<void> {
  await page.evaluate(
    async ({ lineId, stationIndex }) => {
      const e2e = (window as any).__E2E_TEST__;
      await e2e.projectStore.getState().setZeroDatum(
        { line_id: lineId, station_index: stationIndex },
        { latitude: 24.55, longitude: -81.78, depth_reference: 0 },
        { fitToBounds: false }
      );
    },
    { lineId, stationIndex }
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
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapReady(page);
    await waitForNoDialogs(page);
    await openStationsView(page);
  });

  test('gutter shows station name pills and a read-only Station ID column; X/Y/Depth are read-only', async ({ page }) => {
    const lineId = await getLineId(page);

    // The gutter names each station (seeded from its index: "0" ... "3").
    await expect(page.getByTestId('grid-gutter-0')).toHaveText('0');
    await expect(page.getByTestId('grid-gutter-3')).toHaveText('3');

    // The Station ID column shows the project-unique id (the mock derives it
    // from the line and index) and never opens the editor.
    await expect(page.getByTestId('grid-header-_stationId')).toBeVisible();
    await expect(page.getByTestId('grid-cell-0-_stationId')).toHaveText(String(lineId * 100_000));
    await expect(page.getByTestId('grid-cell-3-_stationId')).toHaveText(String(lineId * 100_000 + 3));
    await page.getByTestId('grid-cell-1-_stationId').dblclick();
    await expect(page.getByTestId('grid-cell-editor')).toHaveCount(0);

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

  test('the name column shows each station name and renames through set_station_name', async ({ page }) => {
    const lineId = await getLineId(page);
    // Names as a DMP import records them (MNemo station numbers); station 2
    // keeps the name seeded from its index.
    await page.evaluate(async (lineId) => {
      const setName = (window as any).__MOCK_SET_STATION_NAME__;
      setName(lineId, 0, '7');
      setName(lineId, 1, '8');
      setName(lineId, 3, '10');
      await (window as any).__E2E_TEST__.projectStore.getState().refreshProjectState();
    }, lineId);

    // Hidden by default; the eye menu shows it.
    await expect(page.getByTestId('grid-header-name')).toHaveCount(0);
    await page.getByTestId('data-table-columns-menu').click();
    await page.getByTestId('data-table-columns-dropdown').getByTestId('col-vis-name').check();
    await page.keyboard.press('Escape');
    await expect(page.getByTestId('grid-header-name')).toBeVisible();
    await expect(page.getByTestId('grid-cell-0-name')).toHaveText('7');
    await expect(page.getByTestId('grid-cell-1-name')).toHaveText('8');
    await expect(page.getByTestId('grid-cell-2-name')).toHaveText('2');
    await expect(page.getByTestId('grid-cell-3-name')).toHaveText('10');
    // The gutter pill shows the same name.
    await expect(page.getByTestId('grid-gutter-1')).toHaveText('8');

    // A name another station of the line holds is refused inline: the editor
    // stays open and nothing is sent.
    await page.getByTestId('grid-cell-1-name').dblclick();
    const editor = page.getByTestId('grid-cell-editor');
    await expect(editor).toBeVisible();
    await editor.fill('10');
    await page.keyboard.press('Enter');
    await expect(editor).toBeVisible();
    expect(await getStationName(page, lineId, 1)).toBe('8');

    // A free name commits through set_station_name and survives the refresh.
    await editor.fill('8b');
    await page.keyboard.press('Enter');
    await expect(editor).toHaveCount(0);
    await expect(page.getByTestId('grid-cell-1-name')).toHaveText('8b');
    await expect(page.getByTestId('grid-gutter-1')).toHaveText('8b');
    expect(await getStationName(page, lineId, 1)).toBe('8b');

    // Undo gives the old name back; redo the new one.
    await page.evaluate(async () => {
      await (window as any).__E2E_TEST__.projectStore.getState().undo();
    });
    await expect(page.getByTestId('grid-cell-1-name')).toHaveText('8');
    await page.evaluate(async () => {
      await (window as any).__E2E_TEST__.projectStore.getState().redo();
    });
    await expect(page.getByTestId('grid-cell-1-name')).toHaveText('8b');
  });

  test('comment edits commit via set_station_comment and survive the refresh', async ({ page }) => {
    const lineId = await getLineId(page);

    await page.getByTestId('grid-cell-0-comment').dblclick();
    const editor = page.getByTestId('grid-cell-editor');
    await expect(editor).toBeVisible();
    await editor.fill('Drop weight here');
    await page.keyboard.press('Enter');
    await expect(editor).toHaveCount(0);

    // The store refreshed from get_project_state and still shows the comment.
    await expect(page.getByTestId('grid-cell-0-comment')).toHaveText('Drop weight here');
    expect(await getStationComment(page, lineId, 0)).toBe('Drop weight here');

    // Delete clears the comment (text column → null, no red "missing" state).
    await page.getByTestId('grid-cell-0-comment').click();
    await page.keyboard.press('Delete');
    await expect(page.getByTestId('grid-cell-0-comment')).toHaveText('');
    expect(await getStationComment(page, lineId, 0)).toBeNull();
  });

  test('GPS cell click on a plain station opens add-zero-datum mode', async ({ page }) => {
    const lineId = await getLineId(page);

    await page.getByTestId(`station-gps-${lineId}-1`).click();
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
    const lineId = await getLineId(page);
    await setZeroDatum(page, lineId, 0);

    await page.getByTestId(`station-gps-${lineId}-0`).click();
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

/**
 * A junction (plan 5.3): Main Line's last station is Side's first - ONE
 * station two lines pass, sharing its Station ID and name. A rename must be
 * free on every line through it, so a name only Side holds is refused inline
 * in Main Line's stations grid (the backend would refuse it too), and a free
 * name renames the station on both lines.
 */
test.describe('Stations grid at a junction', () => {
  const junctionSeeds: LineSeed[] = [
    lineSeeds[0]!,
    {
      name: 'Side Line',
      direction: 'in',
      shots: [
        { azimuth_in: 180, azimuth_out: 180, length: 5, depth_in: -3, depth_out: -4 },
        { azimuth_in: 200, azimuth_out: 200, length: 6, depth_in: -4, depth_out: -5 },
      ],
    },
  ];

  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Stations Grid Junction Test');
    await seedProjectWithLines(page, junctionSeeds);
    await waitForMapReady(page);
    await waitForNoDialogs(page);
  });

  test('a rename is checked on every line through the junction', async ({ page }) => {
    const [mainId, sideId] = await page.evaluate(() => {
      const lines = (window as any).__E2E_TEST__.projectStore.getState().project.lines as Array<{ id: number }>;
      return [lines[0]!.id, lines[1]!.id];
    });
    // Join Main's end (station 3) to Side's start, and give Side's far station
    // a name Main Line does not have.
    await page.evaluate(
      async ({ mainId, sideId }) => {
        const store = (window as any).__E2E_TEST__.projectStore;
        await store
          .getState()
          .mergeStations({ line_id: mainId, station_index: 3 }, { line_id: sideId, station_index: 0 });
        (window as any).__MOCK_SET_STATION_NAME__(sideId, 2, 'S2');
        await store.getState().refreshProjectState();
      },
      { mainId, sideId },
    );
    // Both slots are the one station: same Station ID, same name.
    expect(await getStationName(page, sideId, 0)).toBe('3');
    expect(await getStationName(page, sideId, 2)).toBe('S2');

    await openStationsView(page);
    await page.getByTestId('data-table-columns-menu').click();
    await page.getByTestId('data-table-columns-dropdown').getByTestId('col-vis-name').check();
    await page.keyboard.press('Escape');
    await expect(page.getByTestId('grid-cell-3-name')).toHaveText('3');

    // "S2" is free on Main Line but taken on Side Line, which also passes the
    // station: refused inline - the editor stays open and nothing is sent.
    await page.getByTestId('grid-cell-3-name').dblclick();
    const editor = page.getByTestId('grid-cell-editor');
    await expect(editor).toBeVisible();
    await editor.fill('S2');
    await page.keyboard.press('Enter');
    await expect(editor).toBeVisible();
    await expect(page.getByTestId('grid-invalid-flash')).toBeVisible();
    expect(await getStationName(page, mainId, 3)).toBe('3');

    // A name free on both lines renames the station on both.
    await editor.fill('J');
    await page.keyboard.press('Enter');
    await expect(editor).toHaveCount(0);
    await expect(page.getByTestId('grid-cell-3-name')).toHaveText('J');
    expect(await getStationName(page, mainId, 3)).toBe('J');
    expect(await getStationName(page, sideId, 0)).toBe('J');
  });
});
