import { test, expect, Page } from './fixtures/coverage';
import { setupMockedPage, createProject, seedProjectWithLines, waitForNoDialogs } from './helpers/app';

/**
 * Coverage ported from the retired WDIO `import-line-dialog` spec (#120),
 * extended when the dialog's hand-rolled shot table became a SpreadsheetGrid.
 *
 * The manual "Import / Create Line" dialog (`ImportDialog`, rendered for the
 * `import` dialog type - see App.tsx) has no other Playwright twin, so this is
 * its only mocked-tier coverage: the metadata fields and Create / Cancel
 * actions, plus the grid behaviours the old table could not do (paste a block
 * out of a spreadsheet, fill down) and the Create gate that replaced its silent
 * row-dropping.
 *
 * The dialog's rows are LOCAL until Create - there is no project state to
 * assert against, so the model is read off the rendered cells.
 */
async function openImportLineDialog(page: Page): Promise<void> {
  await page.evaluate(() => {
    const e2e = (window as unknown as {
      __E2E_TEST__?: { uiStore: { getState: () => { openDialog: (dialog: string) => void } } };
    }).__E2E_TEST__;
    if (!e2e) throw new Error('E2E stores not available on window');
    e2e.uiStore.getState().openDialog('import');
  });
  await expect(page.getByTestId('import-line-dialog')).toBeVisible({ timeout: 5000 });
  // The grid always carries a trailing blank row, so row 0 exists from the off.
  await expect(page.getByTestId('new-line-cell-0-length')).toBeVisible({ timeout: 5000 });
}

/** Type a value into a grid cell and commit it. */
async function fillCell(page: Page, row: number, colKey: string, value: string): Promise<void> {
  await page.getByTestId(`new-line-cell-${row}-${colKey}`).dblclick();
  await page.getByTestId('new-line-cell-editor').fill(value);
  await page.keyboard.press('Enter');
}

/** Fill one complete shot: length, azimuth and both depths. */
async function fillShotRow(page: Page, row: number, length: string): Promise<void> {
  await fillCell(page, row, 'length', length);
  await fillCell(page, row, '_azimuth_avg', '90');
  await fillCell(page, row, 'depth_in', '0');
  await fillCell(page, row, 'depth_out', '-1');
}

/** The `update_new_line_view` payloads the mock recorded so far. */
async function getViewCalls(page: Page): Promise<Record<string, unknown>[]> {
  return page.evaluate(
    () =>
      (((window as unknown as Record<string, unknown>).__MOCK_NEW_LINE_VIEW_CALLS__ ??
        []) as Record<string, unknown>[])
  );
}

/** Wait for the debounced layout save to settle, then return the last payload.
 *  Asserts the SETTLED state rather than sampling mid-debounce, which races the
 *  500ms timer under parallel load. */
async function waitForViewSave(page: Page): Promise<Record<string, unknown>> {
  await page.waitForFunction(
    () =>
      (((window as unknown as Record<string, unknown>).__MOCK_NEW_LINE_VIEW_CALLS__ ??
        []) as unknown[]).length >= 1,
    undefined,
    { timeout: 5000 }
  );
  const calls = await getViewCalls(page);
  return calls[calls.length - 1]!;
}

test.describe('Import / Create Line dialog', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Import Line Test');
  });

  test('opens with the line-name field and Create/Cancel actions', async ({ page }) => {
    await openImportLineDialog(page);

    const dialog = page.getByTestId('import-line-dialog');
    await expect(dialog.getByTestId('line-name-input')).toBeVisible();
    await expect(dialog.getByRole('button', { name: /Create Line/i })).toBeVisible();
    await expect(dialog.getByRole('button', { name: /^Cancel$/i })).toBeVisible();

    // Cancel from a clean form closes the dialog.
    await dialog.getByRole('button', { name: /^Cancel$/i }).click();
    await expect(page.getByTestId('import-line-dialog')).toBeHidden({ timeout: 5000 });
  });

  test('starts on the first free "Line N" and refuses an empty or taken name', async ({ page }) => {
    // Line names are required and unique in the project (SPEC 4.1).
    await seedProjectWithLines(page, [
      {
        name: 'Line 1',
        direction: 'in',
        shots: [{ azimuth_in: 10, azimuth_out: 190, length: 10, depth_in: 0, depth_out: -1 }],
      },
    ]);
    await waitForNoDialogs(page);
    await openImportLineDialog(page);
    const nameInput = page.getByTestId('line-name-input');
    const create = page.getByTestId('import-line-create');
    await expect(nameInput).toHaveValue('Line 2');
    await fillShotRow(page, 0, '10');
    await expect(create).toBeEnabled();

    await nameInput.fill('  ');
    await expect(page.getByTestId('line-name-error')).toHaveText('A line needs a name.');
    await expect(create).toBeDisabled();
    await expect(create).toHaveAttribute('title', 'A line needs a name.');

    await nameInput.fill('Line 1');
    await expect(page.getByTestId('line-name-error')).toHaveText(
      'A line named "Line 1" already exists. Line names must be unique in the project.'
    );
    await expect(create).toBeDisabled();

    await nameInput.fill('Upstream');
    await expect(page.getByTestId('line-name-error')).toHaveCount(0);
    await expect(create).toBeEnabled();
  });

  test('accepts a line name in the name field', async ({ page }) => {
    await openImportLineDialog(page);

    const nameInput = page.getByTestId('import-line-dialog').getByTestId('line-name-input');
    await nameInput.fill('Main Line');
    await expect(nameInput).toHaveValue('Main Line');
  });

  test('opens on the columns the old table showed, with the rest one menu away', async ({ page }) => {
    await openImportLineDialog(page);

    // Visible by default.
    for (const col of ['length', '_azimuth_avg', 'depth_in', 'depth_out', 'kind', 'comment']) {
      await expect(page.getByTestId(`new-line-header-${col}`)).toBeVisible();
    }
    // Hidden by default: the in/out legs, pitch and LRUD.
    for (const col of ['azimuth_in', 'azimuth_out', '_pitch_avg', 'left', 'up']) {
      await expect(page.getByTestId(`new-line-header-${col}`)).toHaveCount(0);
    }
    // The warning/id columns have nothing to say about an uncommitted row and
    // are dropped entirely - not merely hidden, so they are absent from the menu.
    await page.getByTestId('new-line-columns-menu').click();
    await expect(page.getByTestId('new-line-col-vis-left')).toBeVisible();
    await expect(page.getByTestId('new-line-col-vis-_warnings')).toHaveCount(0);
    await expect(page.getByTestId('new-line-col-vis-shot_id')).toHaveCount(0);

    // Revealing LRUD replaces the old "Include LRUD" checkbox.
    await page.getByTestId('new-line-col-vis-left').click();
    await expect(page.getByTestId('new-line-header-left')).toBeVisible();
  });

  test('typing a complete row enables Create and appends a fresh blank row', async ({ page }) => {
    await openImportLineDialog(page);
    const create = page.getByTestId('import-line-create');

    // Nothing typed: Create is closed and says why.
    await expect(create).toBeDisabled();
    await expect(page.getByTestId('import-line-blocker')).toHaveText(
      /Enter at least one shot/i
    );

    // A half-filled row keeps Create closed and names the row.
    await fillCell(page, 0, 'length', '10');
    await expect(create).toBeDisabled();
    await expect(page.getByTestId('import-line-blocker')).toHaveText(
      /Row 1 is missing required values/i
    );

    // Completing it opens the gate, and a new trailing blank row appears.
    await fillCell(page, 0, '_azimuth_avg', '90');
    await fillCell(page, 0, 'depth_in', '0');
    await fillCell(page, 0, 'depth_out', '-1');
    await expect(create).toBeEnabled();
    await expect(page.getByTestId('import-line-blocker')).toHaveText('');
    await expect(page.getByTestId('new-line-cell-1-length')).toBeVisible();
  });

  test('pastes a block of shots out of a spreadsheet', async ({ context, page }) => {
    await context.grantPermissions(['clipboard-read', 'clipboard-write']);
    await openImportLineDialog(page);

    // Three shots x (length, azimuth, depth in, depth out) - the four visible
    // numeric columns, in order. The old hand-rolled table had no paste at all.
    await page.evaluate(() =>
      navigator.clipboard.writeText('10\t90\t0\t-1\n20\t95\t-1\t-2\n30\t100\t-2\t-3')
    );
    await page.getByTestId('new-line-cell-0-length').click();
    await page.keyboard.press('Control+v');

    await expect(page.getByTestId('new-line-cell-2-length')).toHaveText('30.0');
    await expect(page.getByTestId('new-line-cell-1-_azimuth_avg')).toHaveText('95.0');
    await expect(page.getByTestId('new-line-cell-2-depth_out')).toHaveText('-3.0');
    // Three filled rows plus the trailing blank the grid keeps.
    await expect(page.getByTestId('new-line-cell-3-length')).toHaveText('');
    await expect(page.getByTestId('import-line-create')).toBeEnabled();
  });

  test('Ctrl+D fills down and clearing a required cell re-closes the gate', async ({ page }) => {
    await openImportLineDialog(page);
    await fillShotRow(page, 0, '10');
    await fillShotRow(page, 1, '20');

    // Fill the row-0 azimuth down over row 1.
    await page.getByTestId('new-line-cell-0-_azimuth_avg').click();
    await page.getByTestId('new-line-cell-1-_azimuth_avg').click({ modifiers: ['Shift'] });
    await page.keyboard.press('Control+d');
    await expect(page.getByTestId('new-line-cell-1-_azimuth_avg')).toHaveText('90.0');

    // Clearing a required cell marks it and blocks Create - the old dialog let
    // a flagged cell through and silently dropped rows instead.
    const cell = page.getByTestId('new-line-cell-1-length');
    await cell.click();
    await page.keyboard.press('Delete');
    await expect(cell).toHaveClass(/sg-cell-missing/);
    await expect(page.getByTestId('import-line-create')).toBeDisabled();
    await expect(page.getByTestId('import-line-blocker')).toHaveText(
      /Row 2 is missing required values/i
    );

    // Refilling it re-opens the gate.
    await fillCell(page, 1, 'length', '20');
    await expect(page.getByTestId('import-line-create')).toBeEnabled();
  });

  test('the column layout survives closing and reopening the dialog', async ({ page }) => {
    // The round trip this change built a config field, an IPC command and a
    // persistence hook for. Nothing else covers it.
    await openImportLineDialog(page);

    await page.getByTestId('new-line-columns-menu').click();
    await page.getByTestId('new-line-col-vis-left').click();
    await page.getByTestId('new-line-col-vis-comment').click();
    await page.keyboard.press('Escape');

    // The grid follows at once; the save is debounced behind it.
    await expect(page.getByTestId('new-line-header-left')).toBeVisible();
    await expect(page.getByTestId('new-line-header-comment')).toHaveCount(0);

    const call = await waitForViewSave(page);
    expect(call.hiddenColumns).toContain('comment');
    expect(call.hiddenColumns).not.toContain('left');

    // It reaches the stored config, which is what a reopen seeds from.
    await page.waitForFunction(
      () => {
        const hidden = ((window as unknown as Record<string, unknown>).__E2E_TEST__ as
          | { projectStore: { getState: () => { config?: { panels?: Record<string, string[]> } } } }
          | undefined
        )?.projectStore.getState().config?.panels?.new_line_hidden_columns ?? [];
        return hidden.includes('comment') && !hidden.includes('left');
      },
      undefined,
      { timeout: 5000 }
    );

    // Close and reopen: the layout comes back rather than resetting to defaults.
    await page.getByTestId('import-line-cancel').click();
    await expect(page.getByTestId('import-line-dialog')).toBeHidden({ timeout: 5000 });
    await openImportLineDialog(page);
    await expect(page.getByTestId('new-line-header-left')).toBeVisible();
    await expect(page.getByTestId('new-line-header-comment')).toHaveCount(0);
  });

  test('Expand enlarges the table and Shrink puts it back', async ({ page }) => {
    await openImportLineDialog(page);
    const box = page.getByTestId('import-line-grid');
    const collapsed = (await box.boundingBox())!.height;

    await page.getByTestId('import-line-expand').click();
    await expect
      .poll(async () => (await box.boundingBox())!.height)
      .toBeGreaterThan(collapsed);

    // The metadata and the actions stay reachable while expanded - the point of
    // growing the dialog rather than letting the grid take over the window.
    await expect(page.getByTestId('line-name-input')).toBeVisible();
    await expect(page.getByTestId('import-line-create')).toBeVisible();

    await page.getByTestId('import-line-expand').click();
    await expect.poll(async () => (await box.boundingBox())!.height).toBe(collapsed);
  });

  test('reveals a hidden column that is holding up Create', async ({ page }) => {
    // A required field whose column is hidden would otherwise block Create with
    // the offending cell off-screen and no way to find it.
    await openImportLineDialog(page);

    await page.getByTestId('new-line-columns-menu').click();
    await page.getByTestId('new-line-col-vis-depth_in').click();
    await page.keyboard.press('Escape');
    await expect(page.getByTestId('new-line-header-depth_in')).toHaveCount(0);

    // Touching the row makes its missing Depth In matter; the column comes back.
    await fillCell(page, 0, 'length', '10');
    await expect(page.getByTestId('new-line-header-depth_in')).toBeVisible();
    await expect(page.getByTestId('import-line-create')).toBeDisabled();
  });

  test('Create hands off to map placement mode', async ({ page }) => {
    await openImportLineDialog(page);
    await page.getByTestId('line-name-input').fill('Placement Line');
    await fillShotRow(page, 0, '10');

    await page.getByTestId('import-line-create').click();

    // The dialog closes and the line preview follows the cursor until the
    // user clicks the map (placement is confirmed in MapCanvas, not here).
    await expect(page.getByTestId('import-line-dialog')).toBeHidden({ timeout: 5000 });
    await expect
      .poll(() =>
        page.evaluate(() => {
          const e2e = (window as unknown as {
            __E2E_TEST__?: { uiStore: { getState: () => { placementMode: unknown } } };
          }).__E2E_TEST__;
          return e2e?.uiStore.getState().placementMode !== null;
        })
      )
      .toBe(true);
  });
});
