import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithLines,
  waitForMapIdle,
  waitForNoDialogs,
  type LineSeed,
} from './helpers/app';

/**
 * Station identity in the UI (SPEC 4.1/4.2): a station is shown by its name
 * (unique within its line) and its read-only Station ID; a shot by its two
 * station names (`4 → 5`). Positional `line:index` handles are never shown.
 *
 * Renaming goes through Edit Station -> set_station_name: an empty name or one
 * another station of the line holds is refused inline (Save disabled), a
 * backend refusal lands on the same field, and the rename is one undo step.
 * The mock derives Station IDs as `lineId * 100000 + index`.
 */

const lineSeeds: LineSeed[] = [
  {
    name: 'Main Line',
    direction: 'in',
    shots: [
      { azimuth_in: 0, azimuth_out: 0, length: 10, depth_in: 0, depth_out: -1 },
      { azimuth_in: 90, azimuth_out: 90, length: 10, depth_in: -1, depth_out: -2 },
    ],
  },
];

async function lineId(page: Page): Promise<number> {
  return page.evaluate(() => (window as any).__E2E_TEST__.projectStore.getState().project.lines[0].id as number);
}

async function selectStation(page: Page, line: number, index: number): Promise<void> {
  await page.evaluate(
    ({ line, index }) => {
      (window as any).__E2E_TEST__.uiStore.getState().setSelection({
        type: 'station',
        stationRef: { line_id: line, station_index: index },
      });
    },
    { line, index },
  );
  await expect(page.getByTestId('station-detail')).toBeVisible();
}

async function storedName(page: Page, line: number, index: number): Promise<string | null> {
  return page.evaluate(
    ({ line, index }) => {
      const station = (window as any).__E2E_TEST__.projectStore
        .getState()
        .stations.find((s: any) => s.station_ref.line_id === line && s.station_ref.station_index === index);
      return (station?.name as string | undefined) ?? null;
    },
    { line, index },
  );
}

test.describe('Station identity', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Station Identity Test');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapIdle(page);
    await waitForNoDialogs(page);
  });

  test('station detail shows the name and the read-only Station ID, never line:index', async ({ page }) => {
    const line = await lineId(page);
    await selectStation(page, line, 1);

    const detail = page.getByTestId('station-detail');
    await expect(page.getByTestId('station-detail-name')).toHaveText('Name:1');
    await expect(page.getByTestId('station-detail-id')).toHaveText(`Station ID:${line * 100_000 + 1}`);
    await expect(page.getByTestId('station-detail-line')).toHaveText('Line:Main Line');
    // The neighbouring shots are named by their stations.
    await expect(page.getByTestId(`shot-${line}-1-link`)).toHaveText('0 → 1');
    await expect(page.getByTestId(`shot-${line}-1-link`)).toHaveAttribute(
      'aria-label',
      'Select shot Main Line: 0 → 1',
    );
    await expect(detail).not.toContainText(`${line}:1`);
  });

  test('Edit Station renames through set_station_name; empty and taken names are refused inline', async ({ page }) => {
    const line = await lineId(page);
    await selectStation(page, line, 1);
    await page.getByTestId('station-edit-button').click();

    const dialog = page.getByTestId('edit-station-dialog');
    await expect(dialog).toBeVisible();
    await expect(page.getByTestId('edit-station-id')).toHaveText(String(line * 100_000 + 1));
    const input = page.getByTestId('station-name-input');
    await expect(input).toHaveValue('1');
    const save = page.getByTestId('edit-station-save');

    // Empty: refused inline, Save disabled.
    await input.fill('   ');
    await expect(page.getByTestId('station-name-error')).toHaveText('A station needs a name.');
    await expect(save).toBeDisabled();

    // Taken by another station of the line: refused inline, Save disabled.
    await input.fill('2');
    await expect(page.getByTestId('station-name-error')).toContainText('Main Line already has a station named "2"');
    await expect(save).toBeDisabled();

    // A free name saves (trimmed) and the detail panel follows.
    await input.fill(' 1a ');
    await expect(page.getByTestId('station-name-error')).toHaveCount(0);
    await save.click();
    await expect(dialog).toHaveCount(0);
    await expect(page.getByTestId('station-detail-name')).toHaveText('Name:1a');
    expect(await storedName(page, line, 1)).toBe('1a');
    // The Station ID does not change with the name.
    await expect(page.getByTestId('station-detail-id')).toHaveText(`Station ID:${line * 100_000 + 1}`);

    // One undo step: undo restores the old name, redo the new one.
    await page.evaluate(async () => {
      await (window as any).__E2E_TEST__.projectStore.getState().undo();
    });
    await expect(page.getByTestId('station-detail-name')).toHaveText('Name:1');
    await page.evaluate(async () => {
      await (window as any).__E2E_TEST__.projectStore.getState().redo();
    });
    await expect(page.getByTestId('station-detail-name')).toHaveText('Name:1a');
  });

  test('a backend refusal is shown at the name field', async ({ page }) => {
    const line = await lineId(page);
    await selectStation(page, line, 0);
    await page.getByTestId('station-edit-button').click();
    const input = page.getByTestId('station-name-input');
    await expect(input).toHaveValue('0');

    // Another window (or an undo) takes the name after the dialog read the
    // line's names: the client check passes, the backend refuses.
    await input.fill('Z9');
    await page.evaluate((line) => {
      (window as any).__MOCK_SET_STATION_NAME__(line, 2, 'Z9');
    }, line);
    await page.getByTestId('edit-station-save').click();

    // The refusal re-syncs the store, so the field now knows the name is taken.
    await expect(page.getByTestId('station-name-error')).toContainText('already has a station named "Z9"');
    await expect(page.getByTestId('edit-station-save')).toBeDisabled();
    await expect(page.getByTestId('edit-station-dialog')).toBeVisible();
    // What was typed stays, so it can be fixed.
    await expect(input).toHaveValue('Z9');
    expect(await storedName(page, line, 0)).toBe('0');
  });
});
