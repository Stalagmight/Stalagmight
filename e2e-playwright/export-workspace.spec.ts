import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  createSectionViaStore,
  enterExportWorkspace,
  closeExportWorkspace,
  openAppMenu,
} from './helpers/app';

/**
 * The Export workspace replaces the retired modal Export dialog. It is not in the
 * mode switcher: the project menu's Export row and Ctrl+E enter it, and "Close
 * export" (toolbar, panel header, the same menu row, Ctrl+E, Escape) returns to
 * the mode it was entered from. Nothing here depends on a `mode-export` button.
 */

type ExportCall = { cmd: string; args: Record<string, unknown> | undefined };

interface TestWindow {
  __E2E_TEST__: {
    uiStore: { getState: () => { appMode: string; setAppMode: (m: string) => void; exportReturnMode: string | null } };
    projectStore: {
      getState: () => {
        setZeroDatum: (
          stationRef: { section_id: number; station_index: number },
          coordinates: { latitude: number; longitude: number; depth_reference: number },
        ) => Promise<unknown>;
      };
    };
  };
  __TAURI_INTERNALS__: { invoke: (cmd: string, args?: Record<string, unknown>, opts?: unknown) => Promise<unknown> };
  __MOCK_IPC_OVERRIDES__?: Record<string, unknown>;
  __EXPORT_CALLS__?: ExportCall[];
  __E2E_MOCKS__?: {
    getLastExportImageArgs: () => { options?: Record<string, unknown> } | null;
    clearLastExportImageArgs: () => void;
  };
}

const EXPORT_COMMANDS = ['export_csv', 'export_json', 'export_tml', 'export_image', 'plugin:dialog|save'];

async function appMode(page: Page): Promise<string> {
  return page.evaluate(() => (window as unknown as TestWindow).__E2E_TEST__.uiStore.getState().appMode);
}

async function setAppMode(page: Page, mode: string): Promise<void> {
  await page.evaluate((m) => (window as unknown as TestWindow).__E2E_TEST__.uiStore.getState().setAppMode(m), mode);
}

/** Record every export-related IPC call (and the save picker) from now on. */
async function recordExportCalls(page: Page): Promise<void> {
  await page.evaluate((names) => {
    const w = window as unknown as TestWindow;
    w.__EXPORT_CALLS__ = [];
    const inner = w.__TAURI_INTERNALS__.invoke;
    w.__TAURI_INTERNALS__.invoke = (cmd, args, opts) => {
      if (names.includes(cmd)) w.__EXPORT_CALLS__?.push({ cmd, args });
      return inner(cmd, args, opts);
    };
  }, EXPORT_COMMANDS);
}

async function exportCalls(page: Page): Promise<ExportCall[]> {
  return page.evaluate(() => (window as unknown as TestWindow).__EXPORT_CALLS__ ?? []);
}

/** What the OS save picker answers: a path, or null for a cancel. */
async function answerSavePicker(page: Page, path: string | null): Promise<void> {
  await page.evaluate((p) => {
    const w = window as unknown as TestWindow;
    w.__MOCK_IPC_OVERRIDES__ = { ...(w.__MOCK_IPC_OVERRIDES__ ?? {}), 'plugin:dialog|save': p };
  }, path);
}

async function pickOption(page: Page, selectTestId: string, label: string): Promise<void> {
  await page.getByTestId(selectTestId).click();
  await page.getByRole('listbox').locator(`[role="option"]:has-text("${label}")`).click();
  await expect(page.getByTestId(selectTestId)).toHaveText(label);
}

async function seedTwoSectionsOneAnchored(page: Page, secondName: string): Promise<void> {
  await createSectionViaStore(page, {
    name: 'Anchored Section',
    shots: [{ azimuth_in: 45, azimuth_out: 45, length: 10, depth_in: 0, depth_out: 0 }],
  });
  await createSectionViaStore(page, {
    name: secondName,
    shots: [{ azimuth_in: 120, azimuth_out: 120, length: 8, depth_in: 1, depth_out: 1 }],
  });
  await page.evaluate(async () => {
    await (window as unknown as TestWindow).__E2E_TEST__.projectStore.getState().setZeroDatum(
      { section_id: 1, station_index: 0 },
      { latitude: 24.55, longitude: -81.78, depth_reference: 0 },
    );
  });
}

test.describe('Export workspace: entry and exit', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Export Workspace Project');
  });

  test('the menu Export row enters the workspace and its Close export row leaves it @smoke', async ({ page }) => {
    await enterExportWorkspace(page);
    expect(await appMode(page)).toBe('export');
    // A workspace, not a modal: no dialog, and the panel and live preview are on screen.
    await expect(page.locator('[role="dialog"]')).toHaveCount(0);
    await expect(page.getByTestId('export-mode-preview')).toBeVisible();
    // Import and Undo/Redo give way to the way out.
    await expect(page.getByTestId('export-close-button')).toHaveText('Close export');
    await expect(page.getByTestId('toolbar-import-button')).toHaveCount(0);

    await openAppMenu(page);
    const row = page.getByTestId('toolbar-export-button');
    await expect(row).toContainText('Close export');
    await row.click();
    expect(await appMode(page)).toBe('survey');
    await expect(page.getByTestId('export-panel')).toHaveCount(0);
    await expect(page.getByTestId('toolbar-import-button')).toBeVisible();

    await openAppMenu(page);
    await expect(page.getByTestId('toolbar-export-button')).toContainText('Export');
    await expect(page.getByTestId('toolbar-export-button')).not.toContainText('Close');
  });

  test('Ctrl+E enters the workspace and Ctrl+E leaves it @smoke', async ({ page }) => {
    await page.keyboard.press('Control+e');
    await expect(page.getByTestId('export-panel')).toBeVisible({ timeout: 5000 });
    expect(await appMode(page)).toBe('export');
    await expect(page.locator('[role="dialog"]')).toHaveCount(0);

    await page.keyboard.press('Control+e');
    await expect(page.getByTestId('export-panel')).toHaveCount(0);
    expect(await appMode(page)).toBe('survey');
  });

  test('the toolbar Close export returns to Plan when entered from Plan', async ({ page }) => {
    await page.getByTestId('mode-plan').click();
    expect(await appMode(page)).toBe('plan');

    await enterExportWorkspace(page);
    expect(await appMode(page)).toBe('export');

    await closeExportWorkspace(page);
    expect(await appMode(page)).toBe('plan');
  });

  test('the panel close button returns to the mode it was entered from', async ({ page }) => {
    await page.getByTestId('mode-plan').click();
    await page.keyboard.press('Control+e');
    await expect(page.getByTestId('export-panel')).toBeVisible({ timeout: 5000 });

    await page.getByTestId('export-panel-close').click();
    await expect(page.getByTestId('export-panel')).toHaveCount(0);
    expect(await appMode(page)).toBe('plan');
  });

  test('Ctrl+E from Plan toggles back to Plan', async ({ page }) => {
    await page.getByTestId('mode-plan').click();
    await page.keyboard.press('Control+e');
    await expect(page.getByTestId('export-panel')).toBeVisible({ timeout: 5000 });
    await page.keyboard.press('Control+e');
    await expect(page.getByTestId('export-panel')).toHaveCount(0);
    expect(await appMode(page)).toBe('plan');
  });

  test('Close export returns to Data when entered from Data', async ({ page }) => {
    await setAppMode(page, 'data');
    expect(await appMode(page)).toBe('data');

    await enterExportWorkspace(page);
    await closeExportWorkspace(page);
    expect(await appMode(page)).toBe('data');
  });

  test('Escape closes the workspace when nothing else is open', async ({ page }) => {
    await enterExportWorkspace(page);
    await page.keyboard.press('Escape');
    await expect(page.getByTestId('export-panel')).toHaveCount(0);
    expect(await appMode(page)).toBe('survey');
  });

  test('Escape closes an open format list first, then the workspace', async ({ page }) => {
    await enterExportWorkspace(page);
    await page.getByTestId('image-format-select').click();
    await expect(page.getByRole('listbox')).toBeVisible();
    await page.keyboard.press('Escape');
    await expect(page.getByRole('listbox')).toHaveCount(0);
    expect(await appMode(page)).toBe('export');

    await page.keyboard.press('Escape');
    await expect(page.getByTestId('export-panel')).toHaveCount(0);
    expect(await appMode(page)).toBe('survey');
  });

  test('Ctrl+E stays inert behind an open dialog', async ({ page }) => {
    await page.keyboard.press('?');
    await expect(page.getByTestId('keyboard-shortcuts-dialog')).toBeVisible({ timeout: 5000 });
    await page.keyboard.press('Control+e');
    expect(await appMode(page)).toBe('survey');
    await expect(page.getByTestId('export-panel')).toHaveCount(0);
  });
});

test.describe('Export workspace: data export', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Export Data Project');
    await createSectionViaStore(page, {
      name: 'Main Passage',
      shots: [
        { azimuth_in: 10, azimuth_out: 10, length: 12, depth_in: 1.0, depth_out: 1.8 },
        { azimuth_in: 25, azimuth_out: 25, length: 9, depth_in: 1.8, depth_out: 2.4 },
      ],
    });
    await enterExportWorkspace(page);
  });

  test('a CSV export is one choice away and writes where the picker says @smoke', async ({ page }) => {
    await recordExportCalls(page);
    await answerSavePicker(page, '/mock/out/survey.csv');

    // The workspace opens on the image export with its live preview; Survey Data
    // switches to the data formats with CSV preselected and hides the preview.
    await expect(page.getByTestId('export-type-image')).toBeChecked();
    await page.getByTestId('export-type-data').click();
    await expect(page.getByTestId('export-type-data')).toBeChecked();
    await expect(page.getByTestId('data-format-select')).toHaveText('CSV');
    await expect(page.getByTestId('export-mode-preview')).toHaveCount(0);

    await page.getByTestId('export-button').click();
    await expect.poll(async () => (await exportCalls(page)).map((c) => c.cmd)).toEqual([
      'plugin:dialog|save',
      'export_csv',
    ]);
    const csv = (await exportCalls(page)).find((c) => c.cmd === 'export_csv');
    expect(csv?.args?.path).toBe('/mock/out/survey.csv');

    // The workspace stays open for the next export, and nothing failed.
    await expect(page.getByTestId('export-button')).toBeEnabled();
    await expect(page.getByTestId('export-button')).toHaveText('Export');
    expect(await appMode(page)).toBe('export');
    await expect(page.locator('[data-toast-kind="error"]')).toHaveCount(0);
  });

  test('the data formats offer CSV, JSON and TML; JSON exports through export_json', async ({ page }) => {
    await recordExportCalls(page);
    await answerSavePicker(page, '/mock/out/survey.json');
    await page.getByTestId('export-type-data').click();

    await page.getByTestId('data-format-select').click();
    const list = page.getByRole('listbox');
    for (const label of ['CSV', 'JSON', 'TML (Ariane)']) {
      await expect(list.locator(`[role="option"]:has-text("${label}")`)).toBeVisible();
    }
    await list.locator('[role="option"]:has-text("JSON")').click();
    await expect(page.getByTestId('data-format-select')).toHaveText('JSON');

    await page.getByTestId('export-button').click();
    await expect.poll(async () => (await exportCalls(page)).map((c) => c.cmd)).toContain('export_json');
    expect((await exportCalls(page)).find((c) => c.cmd === 'export_json')?.args?.path).toBe('/mock/out/survey.json');
  });

  test('a cancelled save picker writes nothing and leaves the workspace as it was', async ({ page }) => {
    await recordExportCalls(page);
    await answerSavePicker(page, null);
    await page.getByTestId('export-type-data').click();

    await page.getByTestId('export-button').click();
    await expect.poll(async () => (await exportCalls(page)).map((c) => c.cmd)).toEqual(['plugin:dialog|save']);
    await expect(page.getByTestId('export-button')).toBeEnabled();
    await expect(page.getByTestId('export-type-data')).toBeChecked();
    expect(await appMode(page)).toBe('export');
  });
});

test.describe('Export workspace: TML notice', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'TML Warning Project');
  });

  test('warns when sections are not anchored, and Cancel writes nothing', async ({ page }) => {
    await seedTwoSectionsOneAnchored(page, 'Floating Section');
    await enterExportWorkspace(page);
    await recordExportCalls(page);
    await answerSavePicker(page, '/mock/out/export-warning.tml');

    await page.getByTestId('export-type-data').click();
    await pickOption(page, 'data-format-select', 'TML (Ariane)');
    await page.getByTestId('export-button').click();

    const warningDialog = page.getByTestId('tml-unanchored-warning-dialog');
    await expect(warningDialog).toBeVisible();
    // The consequence is that the export is not GEOREFERENCED, nothing is
    // discarded, and the fix is named. It must NOT promise entrance topology
    // (see the comment above the AlertDialog in ExportControls.tsx).
    await expect(warningDialog).toContainText('1 of 2 sections is not connected to a zero datum');
    await expect(warningDialog).toContainText('will export without georeferencing');
    await expect(warningDialog).toContainText('Nothing is discarded');
    await expect(warningDialog).toContainText('Set Zero Datum');
    await expect(warningDialog).not.toContainText('entrance');
    await expect(warningDialog).toContainText('Floating Section (#2)');
    await expect(warningDialog).not.toContainText('Anchored Section');
    await expect(page.getByTestId('tml-unanchored-warning-continue')).toHaveAttribute('data-primary-action', '');

    await page.getByTestId('tml-unanchored-warning-cancel').click();
    await expect(warningDialog).toHaveCount(0);
    // Cancel closes the notice only: the workspace stays.
    expect(await appMode(page)).toBe('export');
    await expect(page.getByTestId('export-panel')).toBeVisible();
    expect(await exportCalls(page)).toEqual([]);
  });

  test('Continue exports through export_tml', async ({ page }) => {
    await seedTwoSectionsOneAnchored(page, 'Unanchored Section');
    await enterExportWorkspace(page);
    await recordExportCalls(page);
    await answerSavePicker(page, '/mock/out/cave.tml');

    await page.getByTestId('export-type-data').click();
    await pickOption(page, 'data-format-select', 'TML (Ariane)');
    await page.getByTestId('export-button').click();
    await expect(page.getByTestId('tml-unanchored-warning-dialog')).toBeVisible();

    await page.getByTestId('tml-unanchored-warning-continue').click();
    await expect(page.getByTestId('tml-unanchored-warning-dialog')).toHaveCount(0);
    await expect.poll(async () => (await exportCalls(page)).map((c) => c.cmd)).toEqual([
      'plugin:dialog|save',
      'export_tml',
    ]);
    expect((await exportCalls(page)).find((c) => c.cmd === 'export_tml')?.args?.path).toBe('/mock/out/cave.tml');
    expect(await appMode(page)).toBe('export');
  });
});

test.describe('Export workspace: image export', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Export Image Project');
    await enterExportWorkspace(page);
  });

  test('offers the image formats, resolutions and the line-weight threshold', async ({ page }) => {
    await expect(page.getByTestId('export-type-image')).toBeChecked();
    await expect(page.getByTestId('image-format-select')).toHaveText('PNG');

    await page.getByTestId('image-format-select').click();
    const formats = page.getByRole('listbox');
    for (const label of ['PNG', 'SVG', 'PDF']) {
      await expect(formats.locator(`[role="option"]:has-text("${label}")`)).toBeVisible();
    }
    await formats.locator('[role="option"]:has-text("PNG")').click();

    // PNG has a resolution, including the 600 DPI print option.
    const resolution = page.getByTestId('resolution-select');
    await expect(resolution).toBeVisible();
    await resolution.click();
    await expect(page.getByRole('listbox').locator('[role="option"]:has-text("600")')).toBeVisible();
    await page.getByRole('listbox').locator('[role="option"]:has-text("300")').click();

    const slider = page.getByTestId('branching-threshold-slider');
    await expect(slider).toHaveValue('2');
    await expect(page.getByTestId('branching-threshold-value')).toHaveText('2');

    // SVG and PDF have no DPI.
    await pickOption(page, 'image-format-select', 'SVG');
    await expect(page.getByTestId('resolution-select')).toHaveCount(0);
    await pickOption(page, 'image-format-select', 'PDF');
    await expect(page.getByTestId('resolution-select')).toHaveCount(0);
  });

  test('the threshold reaches the live preview and the exported image', async ({ page }) => {
    await answerSavePicker(page, '/mock/out/map.png');
    await page.evaluate(() => (window as unknown as TestWindow).__E2E_MOCKS__?.clearLastExportImageArgs());

    await page.getByTestId('branching-threshold-slider').fill('5');
    await expect(page.getByTestId('branching-threshold-value')).toHaveText('5');
    await expect(page.locator('[data-testid="export-mode-preview"] [data-testid="export-preview"] svg'))
      .toHaveAttribute('data-threshold', '5', { timeout: 5000 });

    await page.getByTestId('export-button').click();
    await expect.poll(() =>
      page.evaluate(() => (window as unknown as TestWindow).__E2E_MOCKS__?.getLastExportImageArgs()?.options ?? null),
    ).not.toBeNull();
    const options = await page.evaluate(
      () => (window as unknown as TestWindow).__E2E_MOCKS__?.getLastExportImageArgs()?.options ?? null,
    );
    expect(options?.branching_threshold).toBe(5);
    expect(options?.format).toBe('png');
    // The workspace stays open after an export.
    await expect(page.getByTestId('export-button')).toBeEnabled({ timeout: 10000 });
    expect(await appMode(page)).toBe('export');
  });

  test('an image export shows progress and disables the Export button meanwhile', async ({ page }) => {
    await answerSavePicker(page, '/mock/out/map.png');
    await page.getByTestId('export-button').click();
    await expect(page.getByTestId('loading-progress')).toBeVisible({ timeout: 5000 });
    await expect(page.getByTestId('export-button')).toBeDisabled();
    await expect(page.getByTestId('export-button')).toBeEnabled({ timeout: 10000 });
    await expect(page.getByTestId('loading-progress')).toHaveCount(0);
  });
});
