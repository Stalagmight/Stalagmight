import { test, expect, Page } from './fixtures/coverage';
import { setupMockedPage } from './helpers/app';

/**
 * E2E coverage for the Walls import dialog's per-survey selection gating -
 * the derived state that silently imports the wrong surveys if it drifts:
 * the select-all → `null` vs explicit-sorted-index payload, the
 * selection-scoped shot/splay Import gate, and the splay-only enable branch.
 *
 * The dialog is driven directly via uiStore.openDialog with a synthetic
 * WallsFile (no parse_walls needed); execute_walls_import is mocked and its
 * args are captured via __MOCK_GET_WALLS_IMPORT_CALLS__.
 */

const FILE = '/test/Cave.wpj';

function makeWallsFile() {
  // Alpha: 10 shots, 1 fix. Beta: 5 shots. Splays: 0 shots, 4 splays.
  const surveys = [
    { name: 'Alpha', title: 'Alpha', date: '2020-01-01', segment: '', shot_count: 10, splay_count: 0, fixed_station_count: 1, resolved: true },
    { name: 'Beta', title: 'Beta', date: '', segment: '', shot_count: 5, splay_count: 0, fixed_station_count: 0, resolved: true },
    { name: 'Splays', title: 'Splays', date: '', segment: '', shot_count: 0, splay_count: 4, fixed_station_count: 0, resolved: true },
  ];
  return {
    project_name: 'Cave',
    book_count: 1,
    survey_count: 3,
    resolved_survey_count: 3,
    shot_count: 15,
    splay_count: 4,
    has_georeference: true,
    datum: 'WGS 1984',
    utm_zone: 34,
    fixed_station_count: 1,
    missing_file_count: 0,
    skipped_attachment_count: 0,
    surveys,
    warnings: [],
  };
}

async function openWallsDialog(page: Page) {
  await page.evaluate(
    ({ wallsFile, filePath }) => {
      const e2e = (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { openDialog: (k: string, d: unknown) => void } } } }).__E2E_TEST__;
      e2e?.uiStore.getState().openDialog('walls-import', { wallsFile, filePath });
    },
    { wallsFile: makeWallsFile(), filePath: FILE }
  );
  await expect(page.getByTestId('walls-import-dialog')).toBeVisible();
}

async function getWallsImportCalls(page: Page): Promise<Array<Record<string, unknown>>> {
  return page.evaluate(
    () => (window as unknown as { __MOCK_GET_WALLS_IMPORT_CALLS__?: () => Array<Record<string, unknown>> }).__MOCK_GET_WALLS_IMPORT_CALLS__?.() ?? []
  );
}

test.describe('Walls import dialog - survey selection gating', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await page.evaluate(() => (window as unknown as { __MOCK_RESET_WALLS_IMPORT_CALLS__?: () => void }).__MOCK_RESET_WALLS_IMPORT_CALLS__?.());
  });

  test('all surveys selected imports with selectedSurveys = null @smoke', async ({ page }) => {
    await openWallsDialog(page);
    const confirm = page.getByTestId('walls-import-confirm');
    // hashChecked resolves async (calculate_file_hash + check_duplicate_import).
    await expect(confirm).toBeEnabled();
    await expect(page.getByTestId('walls-selected-count')).toContainText('3 of 3');

    await confirm.click();
    await expect(page.getByTestId('walls-import-dialog')).toBeHidden();

    // Warning-free receipt: #411/#420 downgrade it to a toast, not the modal.
    // Assert the toast FIRST (it waits for the receipt to resolve) so the
    // modal check after it isn't a same-instant false pass that would let a
    // regression opening BOTH surfaces through green.
    // `.filter` because the toast testid is per-toast and strict mode errors
    // on a second concurrent toast.
    await expect(page.getByTestId('toast').filter({ hasText: 'Walls' })).toContainText(
      'Walls import:',
      { timeout: 10000 }
    );
    await expect(page.getByTestId('import-receipt-dialog')).toBeHidden();

    const calls = await getWallsImportCalls(page);
    expect(calls).toHaveLength(1);
    // select-all ⇒ omit the filter (null), so a full import stays unfiltered.
    expect(calls[0]!.selectedSurveys).toBeNull();
  });

  test('a subset sends explicit sorted survey indices', async ({ page }) => {
    await openWallsDialog(page);
    await expect(page.getByTestId('walls-import-confirm')).toBeEnabled();

    // Keep only Alpha (index 0): deselect Beta (1) and Splays (2).
    await page.getByTestId('walls-survey-1').uncheck();
    await page.getByTestId('walls-survey-2').uncheck();
    await expect(page.getByTestId('walls-selected-count')).toContainText('1 of 3');

    await page.getByTestId('walls-import-confirm').click();
    const calls = await getWallsImportCalls(page);
    expect(calls).toHaveLength(1);
    expect(calls[0]!.selectedSurveys).toEqual([0]);
  });

  test('splay-only selection is disabled until Include-splays is on', async ({ page }) => {
    await openWallsDialog(page);
    const confirm = page.getByTestId('walls-import-confirm');
    await expect(confirm).toBeEnabled();

    // Select only the splay-only survey (deselect the two shot-bearing ones).
    await page.getByTestId('walls-survey-0').uncheck();
    await page.getByTestId('walls-survey-1').uncheck();
    // 0 regular shots in the selection + splays not included ⇒ nothing to import.
    await expect(confirm).toBeDisabled();

    await page.getByTestId('walls-include-splays').check();
    // Now the selected survey's splays are importable ⇒ enabled.
    await expect(confirm).toBeEnabled();
  });

  test('deselecting every survey disables Import', async ({ page }) => {
    await openWallsDialog(page);
    const confirm = page.getByTestId('walls-import-confirm');
    await expect(confirm).toBeEnabled();

    await page.getByTestId('walls-select-all').uncheck();
    await expect(page.getByTestId('walls-selected-count')).toContainText('0 of 3');
    await expect(confirm).toBeDisabled();
  });
});
