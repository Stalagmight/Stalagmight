import { test, expect } from './fixtures/coverage';
import { setupMockedPage, createProject, createSectionViaStore } from './helpers/app';
import { recordIpcCalls, getIpcCalls, waitForIpcCall } from './helpers/ipcRecorder';

test.describe('Export Dialog', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('should open export dialog with Ctrl+E @smoke', async ({ page }) => {
    // Create a project first (export requires open project)
    await createProject(page, 'Export Dialog Test Project');

    // Open export dialog with keyboard shortcut
    await page.keyboard.press('Control+e');

    // Verify export dialog appears with proper ARIA attributes
    const exportDialog = page.locator('[data-testid="export-dialog"]');
    await expect(exportDialog).toBeVisible({ timeout: 5000 });

    // Verify dialog has correct role and aria attributes
    const dialogWrapper = page.locator('[role="dialog"]');
    await expect(dialogWrapper).toBeVisible();
    await expect(dialogWrapper).toHaveAttribute('aria-modal', 'true');

    // Verify dialog title is present (use specific h2 within dialog)
    const dialogTitle = exportDialog.locator('h2');
    await expect(dialogTitle).toContainText('Export');
  });

  test('should show CSV format options and preview', async ({ page }) => {
    await createProject(page, 'Export Dialog Test Project');

    // Open export dialog
    await page.keyboard.press('Control+e');
    const exportDialog = page.locator('[data-testid="export-dialog"]');
    await expect(exportDialog).toBeVisible({ timeout: 5000 });

    // By default, data export type should be selected
    const dataRadio = page.locator('[data-testid="export-type-data"]');
    await expect(dataRadio).toBeChecked();

    // Format dropdown (custom Select component) should be visible
    const formatSelect = page.locator('[data-testid="data-format-select"]');
    await expect(formatSelect).toBeVisible();

    // CSV should be the default selection (check button text)
    await expect(formatSelect).toHaveText('CSV');

    // Click to open dropdown and verify CSV option is available
    await formatSelect.click();
    // The dropdown renders via portal to document.body - locate by role
    const dropdown = page.getByRole('listbox');
    await expect(dropdown.locator('[role="option"]:has-text("CSV")')).toBeVisible();
    // Click CSV to close dropdown (selecting default value)
    await dropdown.locator('[role="option"]:has-text("CSV")').click();

    // Preview should not be visible for data export (only for image export)
    const preview = page.locator('[data-testid="export-preview"]');
    await expect(preview).not.toBeVisible();
  });

  test('exports JSON: picks the format and sends export_json with a .json path @smoke', async ({ page }) => {
    await createProject(page, 'Export Dialog Test Project');
    await recordIpcCalls(page, ['export_json', 'export_csv']);

    // Open export dialog
    await page.keyboard.press('Control+e');
    const exportDialog = page.locator('[data-testid="export-dialog"]');
    await expect(exportDialog).toBeVisible({ timeout: 5000 });

    // Select data export type (should already be selected)
    const dataRadio = page.locator('[data-testid="export-type-data"]');
    await expect(dataRadio).toBeChecked();

    // Change format to JSON using custom Select component
    const formatSelect = page.locator('[data-testid="data-format-select"]');
    // Click to open dropdown
    await formatSelect.click();
    // Click JSON option (portaled listbox - locate globally by role)
    await page.getByRole('listbox').locator('[role="option"]:has-text("JSON")').click();

    // Verify JSON is now selected (check button text)
    await expect(formatSelect).toHaveText('JSON');

    // Preview should not be visible for data export
    const preview = page.locator('[data-testid="export-preview"]');
    await expect(preview).not.toBeVisible();

    // Export: the mocked picker answers /mock/saved-project.stalx, and the known
    // .stalx extension is replaced by the chosen format's (#576 D35).
    await page.getByTestId('export-button').click();
    const [call] = await waitForIpcCall(page, 'export_json');
    expect(call.args.path).toBe('/mock/saved-project.json');
    await expect(exportDialog).not.toBeVisible();
    expect(await getIpcCalls(page, 'export_csv')).toHaveLength(0);
  });

  test('should show image export options', async ({ page }) => {
    await createProject(page, 'Export Dialog Test Project');

    // Open export dialog
    await page.keyboard.press('Control+e');
    const exportDialog = page.locator('[data-testid="export-dialog"]');
    await expect(exportDialog).toBeVisible({ timeout: 5000 });

    // Switch to image export type
    await page.click('[data-testid="export-type-image"]');

    // Verify image export is now selected
    const imageRadio = page.locator('[data-testid="export-type-image"]');
    await expect(imageRadio).toBeChecked();

    // Image format select (custom Select component) should be visible
    const imageFormatSelect = page.locator('[data-testid="image-format-select"]');
    await expect(imageFormatSelect).toBeVisible();

    // Verify available image formats by opening dropdown
    await imageFormatSelect.click();
    // The dropdown renders via portal to document.body - locate by role
    const imageDropdown = page.getByRole('listbox');
    await expect(imageDropdown.locator('[role="option"]:has-text("PNG")')).toBeVisible();
    await expect(imageDropdown.locator('[role="option"]:has-text("SVG")')).toBeVisible();
    await expect(imageDropdown.locator('[role="option"]:has-text("PDF")')).toBeVisible();
    // Click PNG to close dropdown (selecting default value)
    await imageDropdown.locator('[role="option"]:has-text("PNG")').click();

    // Resolution select (custom Select component) should be visible for PNG format
    const resolutionSelect = page.locator('[data-testid="resolution-select"]');
    await expect(resolutionSelect).toBeVisible();

    // Branching threshold slider should be visible with default value 2
    const slider = page.locator('[data-testid="branching-threshold-slider"]');
    await expect(slider).toBeVisible();
    await expect(slider).toHaveValue('2');
    await expect(page.locator('[data-testid="branching-threshold-value"]')).toHaveText('2');

    // Preview should be visible for image export
    const preview = page.locator('[data-testid="export-preview"]');
    await expect(preview).toBeVisible({ timeout: 5000 });
  });

  test('should hide DPI select for non-PNG formats', async ({ page }) => {
    await createProject(page, 'Export Dialog Test Project');

    await page.keyboard.press('Control+e');
    await expect(page.locator('[data-testid="export-dialog"]')).toBeVisible({ timeout: 5000 });

    await page.click('[data-testid="export-type-image"]');

    // PNG (default): resolution visible
    await expect(page.locator('[data-testid="resolution-select"]')).toBeVisible();

    // Switch to SVG → resolution should disappear
    const imageFormatSelect = page.locator('[data-testid="image-format-select"]');
    await imageFormatSelect.click();
    await page.getByRole('listbox').locator('[role="option"]:has-text("SVG")').click();
    await expect(page.locator('[data-testid="resolution-select"]')).not.toBeVisible();

    // Switch to PDF → still hidden
    await imageFormatSelect.click();
    await page.getByRole('listbox').locator('[role="option"]:has-text("PDF")').click();
    await expect(page.locator('[data-testid="resolution-select"]')).not.toBeVisible();
  });

  // The SVG/PDF export canary (#576 RV3-FE-7): each format reaches export_image
  // with its own format option and a path carrying its own extension.
  test('exports SVG and PDF through export_image with the matching format and path @smoke', async ({ page }) => {
    await createProject(page, 'Export Dialog Vector Project');
    await recordIpcCalls(page, ['export_image']);
    const exportDialog = page.getByTestId('export-dialog');

    for (const [label, format, n] of [['SVG', 'svg', 1], ['PDF', 'pdf', 2]] as const) {
      await page.keyboard.press('Control+e');
      await expect(exportDialog).toBeVisible({ timeout: 5000 });
      await page.click('[data-testid="export-type-image"]');
      await page.getByTestId('image-format-select').click();
      await page.getByRole('listbox').locator(`[role="option"]:has-text("${label}")`).click();
      await expect(page.getByTestId('resolution-select')).not.toBeVisible();

      await page.getByTestId('export-button').click();
      const calls = await waitForIpcCall(page, 'export_image', n);
      const args = calls[n - 1]!.args as { path?: string; options?: { format?: string } }; // waited for n calls
      expect(args.options?.format).toBe(format);
      expect(args.path).toBe(`/mock/saved-project.${format}`);
      await expect(exportDialog).not.toBeVisible({ timeout: 10000 });
    }
  });

  test('threshold slider updates the displayed value and is passed to export @smoke', async ({ page }) => {
    await createProject(page, 'Export Dialog Threshold Project');

    await page.keyboard.press('Control+e');
    await expect(page.locator('[data-testid="export-dialog"]')).toBeVisible({ timeout: 5000 });
    await page.click('[data-testid="export-type-image"]');

    const slider = page.locator('[data-testid="branching-threshold-slider"]');
    await slider.fill('5');
    await expect(page.locator('[data-testid="branching-threshold-value"]')).toHaveText('5');

    // Clear captured args, then export and confirm the threshold lands in backend args.
    await page.evaluate(() => {
      const mocks = (window as unknown as { __E2E_MOCKS__?: { clearLastExportImageArgs?: () => void } }).__E2E_MOCKS__;
      mocks?.clearLastExportImageArgs?.();
    });

    await page.getByTestId('export-button').click();
    await expect(page.locator('[data-testid="export-dialog"]')).not.toBeVisible({ timeout: 10000 });

    const exportArgs = await page.evaluate(() => {
      const mocks = (window as unknown as {
        __E2E_MOCKS__?: { getLastExportImageArgs: () => { options?: Record<string, unknown> } | null };
      }).__E2E_MOCKS__;
      return mocks?.getLastExportImageArgs?.() ?? null;
    });
    expect(exportArgs).not.toBeNull();
    expect(exportArgs?.options?.branching_threshold).toBe(5);
    expect(exportArgs?.options?.format).toBe('png');
  });

  test('should execute export and show success @smoke', async ({ page }) => {
    await createProject(page, 'Export Dialog Test Project');
    await recordIpcCalls(page, ['export_csv']);

    // Open export dialog
    await page.keyboard.press('Control+e');
    const exportDialog = page.locator('[data-testid="export-dialog"]');
    await expect(exportDialog).toBeVisible({ timeout: 5000 });

    // Data export is selected by default (CSV)
    const exportButton = page.locator('[data-testid="export-button"]');
    await expect(exportButton).toBeVisible();
    await expect(exportButton).toHaveText('Export');

    // Click export button - this triggers the save dialog (mocked)
    // The mock returns a path, triggering the export flow
    await exportButton.click();

    // The CSV export reached the backend with the picked path, its known .stalx
    // extension replaced by .csv (#576 D35)...
    const [call] = await waitForIpcCall(page, 'export_csv');
    expect(call.args.path).toBe('/mock/saved-project.csv');
    // ...and after a successful export the dialog closes, with no failure toast.
    await expect(exportDialog).not.toBeVisible({ timeout: 5000 });
    await expect(page.getByTestId('toast').filter({ hasText: /export/i })).toHaveCount(0);
  });

  test('should warn for TML export when sections are not anchored and allow cancel', async ({ page }) => {
    await createProject(page, 'TML Warning Cancel Project');

    await createSectionViaStore(page, {
      name: 'Anchored Section',
      shots: [{
        azimuth_in: 45,
        azimuth_out: 45,
        length: 10,
        depth_in: 0,
        depth_out: 0,
      }],
    });
    await createSectionViaStore(page, {
      name: 'Floating Section',
      shots: [{
        azimuth_in: 120,
        azimuth_out: 120,
        length: 8,
        depth_in: 1,
        depth_out: 1,
      }],
    });

    await page.evaluate(async () => {
      const e2e = (window as unknown as {
        __E2E_TEST__?: {
          projectStore: {
            getState: () => {
              setZeroDatum: (
                stationRef: { section_id: number; station_index: number },
                coordinates: { latitude: number; longitude: number; depth_reference: number }
              ) => Promise<unknown>;
            };
          };
        };
      }).__E2E_TEST__;
      if (!e2e) throw new Error('E2E stores not available on window');
      await e2e.projectStore.getState().setZeroDatum(
        { section_id: 1, station_index: 0 },
        { latitude: 24.55, longitude: -81.78, depth_reference: 0 }
      );
    });

    await page.evaluate(() => {
      const win = window as unknown as Record<string, unknown>;
      win.__E2E_SAVE_CALLS__ = 0;
      const dialogMock = win.__TAURI_DIALOG_MOCK__ as { save?: () => Promise<string | null> } | undefined;
      if (dialogMock) {
        dialogMock.save = async () => {
          const previous = (win.__E2E_SAVE_CALLS__ as number) ?? 0;
          win.__E2E_SAVE_CALLS__ = previous + 1;
          return '/mock/export-warning.tml';
        };
      }
    });

    await page.keyboard.press('Control+e');
    const exportDialog = page.locator('[data-testid="export-dialog"]');
    await expect(exportDialog).toBeVisible({ timeout: 5000 });

    const formatSelect = page.locator('[data-testid="data-format-select"]');
    await formatSelect.click();
    await page.getByRole('listbox').locator('[role="option"]:has-text("TML (Ariane)")').click();
    await expect(formatSelect).toHaveText('TML (Ariane)');

    await page.getByTestId('export-button').click();

    const warningDialog = page.getByTestId('tml-unanchored-warning-dialog');
    await expect(warningDialog).toBeVisible();
    // The claims a7d12ad0 settled this copy on: the consequence is that the
    // export is not GEOREFERENCED, nothing is discarded, and the fix is named.
    // It must NOT promise entrance topology - that is merge-derived in the
    // exporter while this dialog classifies by zero-datum reachability, and the
    // two predicates disagree in both directions (see the comment above the
    // AlertDialog in ExportControls.tsx, and ExportControls.test.tsx).
    await expect(warningDialog).toContainText('1 of 2 sections is not connected to a zero datum');
    await expect(warningDialog).toContainText('will export without georeferencing');
    await expect(warningDialog).toContainText('Nothing is discarded');
    await expect(warningDialog).toContainText('Set Zero Datum');
    await expect(warningDialog).not.toContainText('entrance');
    await expect(warningDialog).not.toContainText('only include sections');
    await expect(warningDialog).toContainText('Floating Section (#2)');
    await expect(warningDialog).not.toContainText('Anchored Section');
    await expect(page.getByTestId('tml-unanchored-warning-continue')).toHaveAttribute('data-primary-action', '');

    await page.getByTestId('tml-unanchored-warning-cancel').click();
    await expect(warningDialog).not.toBeVisible({ timeout: 3000 });
    await expect(exportDialog).toBeVisible();

    const saveCalls = await page.evaluate(() => ((window as unknown as Record<string, unknown>).__E2E_SAVE_CALLS__ as number) ?? 0);
    expect(saveCalls).toBe(0);
  });

  test('should continue TML export after unanchored warning confirmation @smoke', async ({ page }) => {
    await createProject(page, 'TML Warning Continue Project');

    await createSectionViaStore(page, {
      name: 'Anchored Section',
      shots: [{
        azimuth_in: 30,
        azimuth_out: 30,
        length: 12,
        depth_in: 0,
        depth_out: 0,
      }],
    });
    await createSectionViaStore(page, {
      name: 'Unanchored Section',
      shots: [{
        azimuth_in: 210,
        azimuth_out: 210,
        length: 5,
        depth_in: 2,
        depth_out: 2,
      }],
    });

    await page.evaluate(async () => {
      const e2e = (window as unknown as {
        __E2E_TEST__?: {
          projectStore: {
            getState: () => {
              setZeroDatum: (
                stationRef: { section_id: number; station_index: number },
                coordinates: { latitude: number; longitude: number; depth_reference: number }
              ) => Promise<unknown>;
            };
          };
        };
      }).__E2E_TEST__;
      if (!e2e) throw new Error('E2E stores not available on window');
      await e2e.projectStore.getState().setZeroDatum(
        { section_id: 1, station_index: 0 },
        { latitude: 24.55, longitude: -81.78, depth_reference: 0 }
      );
    });

    await recordIpcCalls(page, ['export_tml']);
    await page.keyboard.press('Control+e');
    const exportDialog = page.locator('[data-testid="export-dialog"]');
    await expect(exportDialog).toBeVisible({ timeout: 5000 });

    const formatSelect = page.locator('[data-testid="data-format-select"]');
    await formatSelect.click();
    await page.getByRole('listbox').locator('[role="option"]:has-text("TML (Ariane)")').click();

    await page.getByTestId('export-button').click();
    await expect(page.getByTestId('tml-unanchored-warning-dialog')).toBeVisible();
    // Nothing is written while the warning is up.
    expect(await getIpcCalls(page, 'export_tml')).toHaveLength(0);

    await page.getByTestId('tml-unanchored-warning-continue').click();
    // [#576 E3] The export ran, once, with the path the picker chose: the mocked picker
    // answers /mock/saved-project.stalx and its known extension becomes .tml (#576 D35).
    // A closed dialog alone also holds when the export never ran.
    const [call] = await waitForIpcCall(page, 'export_tml');
    expect(call.args.path).toBe('/mock/saved-project.tml');
    await expect(exportDialog).not.toBeVisible({ timeout: 5000 });
    expect(await getIpcCalls(page, 'export_tml')).toHaveLength(1);
  });

  test('should close dialog on Cancel', async ({ page }) => {
    await createProject(page, 'Export Dialog Test Project');

    // Open export dialog
    await page.keyboard.press('Control+e');
    const exportDialog = page.locator('[data-testid="export-dialog"]');
    await expect(exportDialog).toBeVisible({ timeout: 5000 });

    // Click Cancel button
    const cancelButton = page.locator('button:has-text("Cancel")');
    await cancelButton.click();

    // Dialog should be closed
    await expect(exportDialog).not.toBeVisible({ timeout: 3000 });
  });

  test('should include 600 DPI option in resolution selector', async ({ page }) => {
    await createProject(page, 'Export Dialog Test Project');

    // Open export dialog
    await page.keyboard.press('Control+e');
    const exportDialog = page.locator('[data-testid="export-dialog"]');
    await expect(exportDialog).toBeVisible({ timeout: 5000 });

    // Switch to image export (PNG is default, which shows resolution selector)
    await page.click('[data-testid="export-type-image"]');

    // Resolution select should be visible
    const resolutionSelect = page.locator('[data-testid="resolution-select"]');
    await expect(resolutionSelect).toBeVisible();

    // Open the dropdown
    await resolutionSelect.click();

    // Verify 600 DPI option is available (portaled listbox)
    const dropdown = page.getByRole('listbox');
    await expect(dropdown.locator('[role="option"]:has-text("600")')).toBeVisible();
  });

  test('should show progress for image export and disable cancel', async ({ page }) => {
    await createProject(page, 'Export Dialog Progress Project');

    await page.keyboard.press('Control+e');
    const exportDialog = page.locator('[data-testid="export-dialog"]');
    await expect(exportDialog).toBeVisible({ timeout: 5000 });

    await page.click('[data-testid="export-type-image"]');

    const exportButton = page.getByTestId('export-button');
    await exportButton.click();

    const progress = page.getByTestId('loading-progress');
    await expect(progress).toBeVisible({ timeout: 5000 });

    const cancelButton = exportDialog.getByRole('button', { name: 'Cancel' });
    await expect(cancelButton).toBeDisabled();

    await expect(exportDialog).not.toBeVisible({ timeout: 10000 });
  });

  test('should close dialog when save is canceled', async ({ page }) => {
    await createProject(page, 'Export Dialog Save Cancel');

    await page.keyboard.press('Control+e');
    const exportDialog = page.locator('[data-testid="export-dialog"]');
    await expect(exportDialog).toBeVisible({ timeout: 5000 });

    await page.evaluate(() => {
      const dialogMock = (window as unknown as Record<string, unknown>).__TAURI_DIALOG_MOCK__ as {
        save?: () => Promise<string | null>;
      } | undefined;
      if (dialogMock) {
        dialogMock.save = async () => null;
      }
    });

    await page.getByTestId('export-button').click();
    await expect(exportDialog).not.toBeVisible({ timeout: 5000 });
  });
});
