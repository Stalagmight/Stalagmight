import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage } from './helpers/app';
import { getIpcCalls, recordIpcCalls, waitForIpcCall } from './helpers/ipcRecorder';

/**
 * Mocked-tier coverage for the Compass (.mak/.dat) import journey (#576
 * NF-W3CI-2), the way a user drives it: unified picker (Ctrl+I) ->
 * routeOpenedFile -> parse_compass -> new-project-for-import ->
 * CompassImportDialog (preview + options) -> execute_compass_import -> receipt.
 *
 * The file is the synthetic one mocks/ipc/compassImport.ts serves (no Compass
 * file is read): cave CaveA, surveys A1 (4 shots, one excluded) and A2 (3).
 */

const MAK_PATH = '/mock/synthetic-cave.mak';

async function openCompassImportDialog(page: Page): Promise<void> {
  await page.evaluate((selectedPath) => {
    const w = window as unknown as Record<string, unknown>;
    w.__MOCK_IPC_OVERRIDES__ = { ...((w.__MOCK_IPC_OVERRIDES__ as Record<string, unknown>) ?? {}), 'plugin:dialog|open': selectedPath };
  }, MAK_PATH);
  await page.keyboard.press('Control+i');

  const newProject = page.getByTestId('new-project-for-import-dialog');
  await expect(newProject).toBeVisible({ timeout: 10000 });
  // The default name comes from the parsed cave name.
  await expect(page.getByTestId('project-name-input')).toHaveValue('CaveA');
  await page.getByTestId('create-and-continue-button').click();
  await expect(page.getByTestId('compass-import-dialog')).toBeVisible({ timeout: 10000 });
}

async function importedSections(page: Page): Promise<Array<{ name: string; shots: number }>> {
  return page.evaluate(() => {
    const state = (window as unknown as {
      __E2E_TEST__: {
        projectStore: { getState: () => { project: { sections: Array<{ name: string; shots: unknown[] }> } | null } };
      };
    }).__E2E_TEST__.projectStore.getState();
    return (state.project?.sections ?? []).map((s) => ({ name: s.name, shots: s.shots.length }));
  });
}

test.describe('Compass import journey', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('imports a Compass MAK from the unified picker: preview, options, import, receipt @smoke', async ({ page }) => {
    await recordIpcCalls(page, ['parse_compass', 'execute_compass_import']);
    await openCompassImportDialog(page);

    // Preview: what parse_compass returned for the picked file.
    expect((await waitForIpcCall(page, 'parse_compass'))[0].args.path).toBe(MAK_PATH);
    const dialog = page.getByTestId('compass-import-dialog');
    await expect(dialog).toContainText('CaveA');
    await expect(dialog).toContainText('Lengths kept as-is (survey is in meters)');
    // 7 shots, 1 excluded: excluded shots do not land by default.
    await expect(page.getByTestId('compass-shot-count')).toHaveText('6 (1 excluded)');
    await expect(page.getByTestId('compass-survey-shots-0')).toHaveText('3');
    await expect(page.getByTestId('compass-survey-shots-1')).toHaveText('3');

    // Options: including the excluded shot changes what lands, live.
    await expect(page.getByTestId('compass-apply-corrections')).toBeChecked();
    await page.getByTestId('compass-include-excluded').check();
    await expect(page.getByTestId('compass-shot-count')).toHaveText('7');
    await expect(page.getByTestId('compass-survey-shots-0')).toHaveText('4');
    await page.getByTestId('compass-comments-on-stations').uncheck();

    // Import: the options reach the backend exactly as set.
    const confirm = page.getByTestId('compass-import-confirm');
    await expect(confirm).toBeEnabled();
    await confirm.click();
    const [call] = await waitForIpcCall(page, 'execute_compass_import');
    expect(call.args).toMatchObject({
      path: MAK_PATH,
      fileHash: 'mock_hash_synthetic-cave.mak',
      includeExcluded: true,
      applyCorrections: true,
      commentsOnStations: false,
    });
    await expect(dialog).toBeHidden({ timeout: 10000 });

    // Receipt: a warning-free import is a toast (not the modal) with the counts the
    // backend answered. `.filter` because the toast testid is per toast. [#576 E3] The
    // mock's import is canned (the default options' landing, whatever was sent): which
    // legs an option lands, and the receipt's counts, are pinned in Rust
    // (`the_compass_command_honours_include_excluded_and_records_the_file`).
    const receipt = page.getByTestId('toast').filter({ hasText: 'Compass import:' });
    await expect(receipt).toContainText('2 sections', { timeout: 10000 });
    await expect(receipt).toContainText('6 shots');
    await expect(page.getByTestId('import-receipt-dialog')).toBeHidden();

    // And what the backend landed shows in the new project.
    await expect.poll(() => importedSections(page)).toEqual([
      { name: 'A1', shots: 3 },
      { name: 'A2', shots: 3 },
    ]);
  });

  test('Cancel in the Compass dialog imports nothing', async ({ page }) => {
    await recordIpcCalls(page, ['execute_compass_import']);
    await openCompassImportDialog(page);
    await page.getByTestId('compass-import-cancel').click();
    await expect(page.getByTestId('compass-import-dialog')).toBeHidden();
    expect(await importedSections(page)).toEqual([]);
    // [#576 RV4-FE-FEB-9] One honest read once the dialog is gone: a poll for 0
    // passed on its first read, so it waited for nothing. Cancel issues no import.
    expect(await getIpcCalls(page, 'execute_compass_import')).toEqual([]);
  });
});
