import { test, expect, Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  openDmpSelectionDialog,
  proceedToDmpSectionConfig,
  createSectionViaStore,
  ensureDmpNewImportDialogVisible,
  openAppMenu,
} from './helpers/app';
import { recordIpcCalls, waitForIpcCall } from './helpers/ipcRecorder';
import type { E2EWindow } from './helpers/e2eWindow';

/**
 * DMP Import Dialog E2E Tests
 *
 * Tests the core DMP file import functionality including:
 * - Context-aware import triggering (new project vs existing project)
 * - Parsing and displaying DMP file data in Step 1 (selection) and Step 2 (config)
 * - Section selection and configuration
 * - Cancel without importing
 * - Drag-drop DMP file handling
 */

/**
 * Helper to create a project and open the import dialog via Ctrl+I.
 * With the new context-aware flow:
 * - From existing project: Opens DmpSelectionDialog (Step 1) directly with parsed DMP
 * Returns when the DMP file is loaded and sections are visible in Step 1.
 */
async function createProjectAndOpenImportDialog(page: Page): Promise<void> {
  await createProject(page, 'DMP Import Test');
  await openDmpSelectionDialog(page);

  const dialog = page.getByTestId('dmp-selection-dialog');
  await expect(dialog).toBeVisible();
  await expect(dialog.getByTestId('section-row-0')).toBeVisible({ timeout: 5000 });
  // Version is now shown in the filename line
  await expect(dialog.getByTestId('dmp-filename')).toContainText('(v5)');
}

/**
 * Helper to proceed from Step 1 (selection) to the new import wizard.
 */
async function proceedToStep2(page: Page): Promise<void> {
  await proceedToDmpSectionConfig(page);
}

/** How many sections the open project holds. */
async function countSections(page: Page): Promise<number> {
  return page.evaluate(() => {
    const e2e = (window as unknown as {
      __E2E_TEST__?: {
        projectStore?: { getState: () => { project: { sections: unknown[] } | null } };
      };
    }).__E2E_TEST__;
    if (!e2e?.projectStore) {
      throw new Error('E2E project store unavailable');
    }
    return e2e.projectStore.getState().project?.sections.length ?? 0;
  });
}

/** `uiStore.importContext`: which import flow, if any, the app thinks is running. */
async function readImportContext(page: Page): Promise<string | null> {
  return page.evaluate(() => {
    const e2e = (window as unknown as {
      __E2E_TEST__?: { uiStore?: { getState: () => { importContext: string | null } } };
    }).__E2E_TEST__;
    if (!e2e?.uiStore) {
      throw new Error('E2E UI store unavailable');
    }
    return e2e.uiStore.getState().importContext;
  });
}

test.describe('DMP Import Dialog', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  /**
   * Test 6.1: parse, display, and import through to the receipt (the DMP canary)
   *
   * Verifies the DmpSelectionDialog (Step 1) correctly displays parsed DMP data
   * including section names, shot counts, and file metadata - and then that the
   * selection actually imports (#576 RV3-FE-7: this canary used to stop at the
   * selection dialog): import_sections carries exactly the selected section,
   * the section lands in the project, and the receipt toast reports it.
   */
  test('parses a DMP, shows its sections, and imports the selection through to the receipt @smoke', async ({
    page,
  }) => {
    await createProject(page, 'DMP Import Test');
    await createSectionViaStore(page, {
      name: 'Anchor Section',
      shots: [{ azimuth_in: 45, azimuth_out: 45, length: 10, depth_in: -1, depth_out: -2 }],
    });
    // The wizard offers this connection to the existing section, so the import
    // finishes on Confirm (no manual placement).
    await page.evaluate(() => {
      const setSuggestions = (window as unknown as Record<string, unknown>).__MOCK_SET_IMPORT_PLACEMENT_SUGGESTIONS__;
      if (typeof setSuggestions !== 'function') {
        throw new Error('Mock suggestion setter not available');
      }
      (setSuggestions as (suggestions: unknown[]) => void)([
        {
          existingStationIndex: 0,
          importStationIndex: 0,
          score: 0.95,
          matchedShots: 0,
          direction: 'forward',
          isPartial: false,
          confidence: 'high',
          existingSectionName: 'Anchor Section',
          importSectionIndex: 0,
          importSectionName: 'Main Passage',
          targetType: 'existing',
          existingSectionId: 1,
          targetImportIndex: null,
        },
      ]);
    });
    await openDmpSelectionDialog(page);

    const dialog = page.locator('[data-testid="dmp-selection-dialog"]');
    await expect(dialog.getByTestId('section-row-0')).toBeVisible({ timeout: 5000 });

    // Verify dialog has correct ARIA role
    await expect(dialog).toHaveAttribute('role', 'dialog');
    await expect(dialog).toHaveAttribute('aria-modal', 'true');

    // Verify section names displayed in Step 1
    await expect(dialog.locator('text=Main Passage')).toBeVisible();
    await expect(dialog.locator('text=Side Tunnel')).toBeVisible();

    // Step 1 shows a table with checkboxes, section names, dates, and shot counts
    // Verify the section table has expected structure
    const sectionRow0 = dialog.locator('[data-testid="section-row-0"]');
    await expect(sectionRow0).toBeVisible();
    await expect(sectionRow0).toContainText('Main Passage');

    const sectionRow1 = dialog.locator('[data-testid="section-row-1"]');
    await expect(sectionRow1).toBeVisible();
    await expect(sectionRow1).toContainText('Side Tunnel');

    // Verify section checkboxes are present
    const checkbox0 = dialog.locator('[data-testid="section-checkbox-0"]');
    await expect(checkbox0).toBeVisible();
    await expect(checkbox0).toBeChecked(); // Non-empty sections are selected by default

    // Verify file metadata header (version is in filename, stats are in selection-stats)
    const filename = dialog.locator('[data-testid="dmp-filename"]');
    await expect(filename).toContainText('(v5)');

    // Selection stats show shot count, total length, and depth range
    const stats = dialog.locator('[data-testid="selection-stats"]');
    await expect(stats.locator('[data-testid="stats-shot-count"]')).toContainText('shots');
    await expect(stats.locator('[data-testid="stats-total-length"]')).toBeVisible();
    await expect(stats.locator('[data-testid="stats-depth-range"]')).toContainText('Depth:');

    // Import only Main Passage, through the wizard's suggested connection.
    await recordIpcCalls(page, ['import_sections']);
    await dialog.locator('[data-testid="select-none-button"]').click();
    await page.locator('[data-testid="section-checkbox-0"]').click();
    await proceedToDmpSectionConfig(page);
    await expect(page.getByTestId('wizard-suggestion')).toBeVisible({ timeout: 10000 });
    await page.getByTestId('confirm-button').click();

    const [call] = await waitForIpcCall(page, 'import_sections');
    const sent = call.args.sections as Array<{ name?: string; shots?: unknown[] }>;
    expect(sent.map((section) => section.name)).toEqual(['Main Passage']);
    expect(sent[0]!.shots?.length ?? 0).toBeGreaterThan(0);
    await expect(page.getByTestId('dmp-new-import-dialog')).toBeHidden({ timeout: 15000 });
    await expect.poll(() => countSections(page), { timeout: 10000 }).toBe(2);
    // A warning-free receipt is a toast (not the modal): one section landed.
    await expect(page.getByTestId('toast').filter({ hasText: 'import:' })).toContainText('1 section', {
      timeout: 10000,
    });
    // #576 NF-FIX3B-2: the backend mints a uuid for every shot it imports, and uuid
    // anchors (plan pins, the distance origin) resolve through it; the mock's
    // import_sections used to build its shots without one. Read off the mock backend's
    // project: the store refreshes through the binary envelope, which carries no uuid
    // (#576 A9-D2).
    const uuids = await page.evaluate(() => {
      const sections = (window as E2EWindow).__E2E_MOCKS__!.getCurrentProject()!.sections as Array<{
        shots: Array<{ uuid?: string | null }>;
      }>;
      return sections.map((section) => section.shots.map((shot) => shot.uuid ?? null));
    });
    expect(uuids[1]!.length).toBe(sent[0]!.shots?.length); // uuids[1]: two sections polled above
    expect(uuids[1]!.every((uuid) => typeof uuid === 'string'), `imported uuids: ${JSON.stringify(uuids[1])}`).toBe(true);
    expect(new Set(uuids.flat()).size).toBe(uuids.flat().length);
  });

  /**
   * Test 6.2: should proceed from Step 1 into the new workflow
   *
   * Verifies the import flow now proceeds from Step 1 (selection)
   * to zero-datum selection and then into the new import wizard.
   */
  test('should proceed from Step 1 to zero datum selection and wizard @smoke', async ({
    page,
  }) => {
    await createProjectAndOpenImportDialog(page);

    // Step 1 should show Continue button
    const continueButton = page.locator('[data-testid="continue-button"]');
    await expect(continueButton).toBeVisible();
    await expect(continueButton).toBeEnabled();

    // Click Continue to proceed to the zero-datum step
    await continueButton.click();
    await expect(page.getByTestId('zero-datum-selection-dialog')).toBeVisible({ timeout: 5000 });
    await page.getByTestId('zero-datum-selection-skip').click();

    // Flow may auto-enter map placement; helper reopens the wizard with Escape if needed.
    await ensureDmpNewImportDialogVisible(page);

    // In empty-project setup this lands in manual placing mode.
    await expect(page.getByTestId('wizard-manual-placing')).toBeVisible({ timeout: 10000 });
  });

  /**
   * Test 6.3: should allow toggling section selection in Step 1
   *
   * Verifies users can select/deselect sections in Step 1 (DmpSelectionDialog)
   * using checkboxes and bulk selection buttons.
   */
  test('should allow toggling section selection in Step 1', async ({ page }) => {
    await createProjectAndOpenImportDialog(page);

    // Find section checkboxes
    const checkbox0 = page.locator('[data-testid="section-checkbox-0"]');
    const checkbox1 = page.locator('[data-testid="section-checkbox-1"]');
    await expect(checkbox0).toBeVisible();
    await expect(checkbox1).toBeVisible();

    // Both non-empty sections should be selected by default
    await expect(checkbox0).toBeChecked();
    await expect(checkbox1).toBeChecked();

    // Test deselecting a section
    await checkbox0.click();
    await expect(checkbox0).not.toBeChecked();
    await expect(checkbox1).toBeChecked(); // Other section still selected

    // Test reselecting the section
    await checkbox0.click();
    await expect(checkbox0).toBeChecked();

    // Test Select None button
    const selectNoneBtn = page.locator('[data-testid="select-none-button"]');
    await selectNoneBtn.click();
    await expect(checkbox0).not.toBeChecked();
    await expect(checkbox1).not.toBeChecked();

    // Test Select All button
    const selectAllBtn = page.locator('[data-testid="select-all-button"]');
    await selectAllBtn.click();
    await expect(checkbox0).toBeChecked();
    await expect(checkbox1).toBeChecked();
  });

  /**
   * Test 6.4: should handle multi-section DMP files in Step 1
   *
   * Verifies the Step 1 dialog handles DMP files with multiple sections,
   * showing all sections with checkboxes for selection.
   */
  test('should handle multi-section DMP files', async ({ page }) => {
    await createProjectAndOpenImportDialog(page);

    // Verify multiple sections are listed (mock returns 6 sections)
    const dialog = page.locator('[data-testid="dmp-selection-dialog"]');
    await expect(dialog.locator('text=Main Passage')).toBeVisible();
    await expect(dialog.locator('text=Side Tunnel')).toBeVisible();
    await expect(dialog.locator('text=Mixed LRUD Section')).toBeVisible();
    await expect(dialog.locator('text=Undefined LRUD Section')).toBeVisible();
    await expect(dialog.locator('text=Empty Section')).toBeVisible();
    await expect(dialog.locator('text=Validation Test Section')).toBeVisible();

    // Step 1 shows section rows with checkboxes - no expand buttons in Step 1
    const sectionRow0 = dialog.locator('[data-testid="section-row-0"]');
    const sectionRow1 = dialog.locator('[data-testid="section-row-1"]');
    await expect(sectionRow0).toBeVisible();
    await expect(sectionRow1).toBeVisible();

    // Empty section should be displayed but with (empty) indicator and disabled checkbox
    const emptyRow = dialog.locator('[data-testid="section-row-4"]');
    await expect(emptyRow).toContainText('(empty)');
    const emptyCheckbox = dialog.locator('[data-testid="section-checkbox-4"]');
    await expect(emptyCheckbox).toBeDisabled();

    // Test section selection checkboxes
    // Click "Select None" to deselect all sections
    await dialog.locator('[data-testid="select-none-button"]').click();

    // The selection count should show 0 selected
    await expect(dialog.locator('text=/0 of \\d+ sections selected/')).toBeVisible();

    // Click "Select All" to select all non-empty sections
    await dialog.locator('[data-testid="select-all-button"]').click();

    // All non-empty sections should be selected
    await expect(dialog.locator('text=/5 of 5 sections selected/')).toBeVisible();

    // Select only specific sections
    await dialog.locator('[data-testid="select-none-button"]').click();

    // Select Main Passage and Side Tunnel only
    await dialog.locator('[data-testid="section-checkbox-0"]').click();
    await dialog.locator('[data-testid="section-checkbox-1"]').click();

    // Selection count should show 2 selected
    await expect(dialog.locator('text=/2 of 5 sections selected/')).toBeVisible();
  });

  /**
   * Test 6.5: should close Step 1 dialog on Cancel without importing
   *
   * Verifies Cancel button closes dialog without making changes.
   */
  test('should close dialog on Cancel without importing', async ({ page }) => {
    // Create a project first
    await createProject(page, 'Cancel Test Project');

    // Open import dialog with Ctrl+I
    // New flow opens Step 1 (DmpSelectionDialog) directly with parsed DMP
    await page.keyboard.press('Control+i');
    await page.waitForSelector('[data-testid="dmp-selection-dialog"]');
    const dialog = page.locator('[data-testid="dmp-selection-dialog"]');
    await expect(dialog).toBeVisible();

    // Dialog should show the parsed DMP sections
    await expect(page.locator('text=Main Passage')).toBeVisible();

    // Click Cancel button
    await dialog.locator('[data-testid="cancel-button"]').click();

    // Dialog should close
    await expect(dialog).not.toBeVisible({ timeout: 3000 });

    // Re-open - with hash-based duplicate detection, re-import goes directly
    // to DmpSelectionDialog (no session-based path tracking)
    await page.keyboard.press('Control+i');

    // Should go directly to the selection dialog again
    await page.waitForSelector('[data-testid="dmp-selection-dialog"]');
    const dialogAfter = page.locator('[data-testid="dmp-selection-dialog"]');
    await expect(dialogAfter).toBeVisible();

    // Dialog should show sections (file is parsed each time)
    await expect(page.locator('text=Main Passage')).toBeVisible();
  });

  /**
   * Test 6.6: should show manual placing controls in the new wizard
   */
  test('should show manual placing controls in wizard @smoke', async ({
    page,
  }) => {
    await createProjectAndOpenImportDialog(page);

    // Proceed to wizard
    await proceedToStep2(page);

    await expect(page.getByTestId('dmp-new-import-dialog')).toBeVisible();
    await expect(page.getByTestId('wizard-manual-placing')).toBeVisible();
    await expect(page.getByTestId('segment-placement-modes')).toBeVisible();
    await expect(page.getByTestId('segment-place-button')).toBeVisible();
  });

  /**
   * Test 6.7: Re-importing same file goes directly to selection dialog
   *
   * Verifies that re-importing the same DMP file no longer shows a
   * session-based duplicate warning dialog. Hash-based duplicate detection
   * (inline warning banner in DmpSelectionDialog) replaces the old
   * session-based path tracking.
   */
  test('should allow re-importing same file without session-based duplicate warning', async ({ page }) => {
    // Create a project first
    await createProject(page, 'Duplicate Import Test');

    // First import
    await page.keyboard.press('Control+i');
    await page.waitForSelector('[data-testid="dmp-selection-dialog"]');
    await expect(page.locator('[data-testid="dmp-selection-dialog"]')).toBeVisible();
    await expect(page.locator('text=Main Passage')).toBeVisible();

    // Cancel the first import
    await page.locator('[data-testid="cancel-button"]').click();
    await expect(page.locator('[data-testid="dmp-selection-dialog"]')).not.toBeVisible({ timeout: 3000 });

    // Try to import the same file again - should go directly to selection dialog
    await page.keyboard.press('Control+i');
    await page.waitForSelector('[data-testid="dmp-selection-dialog"]');
    await expect(page.locator('[data-testid="dmp-selection-dialog"]')).toBeVisible();
    await expect(page.locator('text=Main Passage')).toBeVisible();
  });

  /**
   * Test 6.8: Context-aware import - from home screen creates new project
   *
   * Verifies that Ctrl+I from home screen (no project) shows the
   * NewProjectForImportDialog before proceeding to import.
   */
  test('should prompt for project name when importing from home screen', async ({
    page,
  }) => {
    // We're on the home screen (GlobalView) with no project open
    await expect(page.getByTestId('global-view')).toBeVisible();

    // Start the import from GlobalView. With recent projects present (default
    // mock) there is no empty-state drop card, so the door is the hamburger's
    // "Open / Import" row, which routes a .dmp into the new-project chain.
    await openAppMenu(page);
    // ...and it is the ONLY door here: the separate Import row was removed in
    // #508 because it opened the identical picker. Re-adding it is the exact
    // regression that change exists to prevent, so fail if it comes back.
    await expect(page.getByTestId('global-import-menu-button')).toHaveCount(0);
    await page.getByTestId('global-open-project-button').click();

    // Should show NewProjectForImportDialog
    await page.waitForSelector('[data-testid="new-project-for-import-dialog"]', { timeout: 5000 });
    const projectDialog = page.locator('[data-testid="new-project-for-import-dialog"]');
    await expect(projectDialog).toBeVisible();

    // Should show DMP file info
    await expect(projectDialog.locator('text=Importing from DMP file')).toBeVisible();

    // Project name field should be pre-filled with filename
    const nameInput = projectDialog.locator('[data-testid="project-name-input"]');
    await expect(nameInput).toBeVisible();
    // Mock returns /mock./fixtures/coverage-survey.dmp, so default name should be 'test-survey'
    await expect(nameInput).toHaveValue('test-survey');

    // Fill in project name and create
    await nameInput.fill('My Import Test Project');
    await projectDialog.locator('[data-testid="create-and-continue-button"]').click();

    // Should transition to Step 1 (DmpSelectionDialog)
    await page.waitForSelector('[data-testid="dmp-selection-dialog"]', { timeout: 5000 });
    const selectionDialog = page.locator('[data-testid="dmp-selection-dialog"]');
    await expect(selectionDialog).toBeVisible();

    // Sections should be visible
    await expect(page.locator('text=Main Passage')).toBeVisible();
  });
});

test.describe('DMP Import Dialog - Zero Datum + Wizard Flow', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('should route from selection to zero datum selection', async ({ page }) => {
    await createProject(page, 'Zero Datum Route Test');
    await openDmpSelectionDialog(page);

    await page.getByTestId('continue-button').click();
    await expect(page.getByTestId('zero-datum-selection-dialog')).toBeVisible({ timeout: 5000 });
  });

  test('should open zero datum config and continue to wizard after GPS confirm', async ({ page }) => {
    await createProject(page, 'Zero Datum Config Test');
    await openDmpSelectionDialog(page);

    await page.getByTestId('continue-button').click();
    await expect(page.getByTestId('zero-datum-selection-dialog')).toBeVisible({ timeout: 5000 });

    await page.getByTestId('zero-datum-section-card-0').click();
    await expect(page.getByTestId('zero-datum-config-dialog')).toBeVisible({ timeout: 5000 });
    await page.getByTestId('zero-datum-gps-input').fill('24.550123, -81.780456');
    await expect(page.getByTestId('zero-datum-gps-parsed')).toBeVisible({ timeout: 5000 });
    await page.getByTestId('zero-datum-config-confirm').click();

    // This flow auto-starts placement and hides the dialog; reopen via Esc.
    await expect(page.getByTestId('placement-mode-indicator')).toBeVisible({ timeout: 10000 });
    await page.keyboard.press('Escape');
    await expect(page.getByTestId('dmp-new-import-dialog')).toBeVisible({ timeout: 10000 });
    await expect(page.getByTestId('wizard-manual-placing')).toBeVisible({ timeout: 10000 });
  });

  /**
   * #534: the config dialog's OWN Back (not the wizard's) used to discard
   * everything typed - App.tsx relayed no carry through the hop back to
   * 'zero-datum-selection'. It now hands `onBack` the dialog's LIVE state
   * (`buildCurrentDraft`), and App.tsx re-seeds it on a forward pass ONLY
   * when the re-picked section matches (`carryZeroDatumConfig`,
   * `src/utils/zeroDatumCarry.ts`). This drives the REAL App.tsx wiring -
   * the vitest suite for this pins the shared decision function, not the
   * routing that calls it.
   */
  test('carries typed GPS back through Back-to-selection on a same-section re-pick, not a different one', async ({
    page,
  }) => {
    await createProject(page, 'Zero Datum Back Carry Test');
    await openDmpSelectionDialog(page);

    await page.getByTestId('continue-button').click();
    await expect(page.getByTestId('zero-datum-selection-dialog')).toBeVisible({ timeout: 5000 });

    await page.getByTestId('zero-datum-section-card-0').click();
    await expect(page.getByTestId('zero-datum-config-dialog')).toBeVisible({ timeout: 5000 });
    await page.getByTestId('zero-datum-gps-input').fill('24.550123, -81.780456');
    await expect(page.getByTestId('zero-datum-gps-parsed')).toBeVisible({ timeout: 5000 });

    await page.getByTestId('zero-datum-config-back').click();
    await page.getByTestId('zero-datum-config-back-confirm-discard').click();
    await expect(page.getByTestId('zero-datum-selection-dialog')).toBeVisible({ timeout: 5000 });

    // Same section re-picked: the typed GPS comes back.
    await page.getByTestId('zero-datum-section-card-0').click();
    await expect(page.getByTestId('zero-datum-config-dialog')).toBeVisible({ timeout: 5000 });
    await expect(page.getByTestId('zero-datum-gps-input')).toHaveValue('24.550123, -81.780456');

    // Back out again - this reopen holds exactly what it was carried with, so
    // it is CLEAN and Back leaves immediately with no discard confirm.
    await page.getByTestId('zero-datum-config-back').click();
    await expect(page.getByTestId('zero-datum-selection-dialog')).toBeVisible({ timeout: 5000 });

    await page.getByTestId('zero-datum-section-card-1').click();
    await expect(page.getByTestId('zero-datum-config-dialog')).toBeVisible({ timeout: 5000 });
    await expect(page.getByTestId('zero-datum-gps-input')).toHaveValue('');
  });

  /**
   * The section preview starts drawing Esri imagery the moment valid GPS
   * coordinates are entered, so it owes attribution from that moment too. It is
   * a few hundred pixels square, so the source names live behind the `i` button
   * Esri's guidance allows for exactly this case - and that panel has to escape
   * the preview's own `overflow-hidden`, which is why it is portalled.
   * https://developers.arcgis.com/documentation/esri-and-data-attribution/interactive-maps/
   */
  test('attributes the imagery the section preview draws, behind an i button', async ({ page }) => {
    await createProject(page, 'Zero Datum Attribution Test');
    await openDmpSelectionDialog(page);

    await page.getByTestId('continue-button').click();
    await expect(page.getByTestId('zero-datum-selection-dialog')).toBeVisible({ timeout: 5000 });
    await page.getByTestId('zero-datum-section-card-0').click();
    await expect(page.getByTestId('zero-datum-config-dialog')).toBeVisible({ timeout: 5000 });

    // Scoped to the preview: the survey map behind this dialog draws satellite
    // by default and carries its own attribution, so an unscoped locator would
    // match two elements and assert against the wrong one.
    const preview = page.getByTestId('section-preview');

    // No imagery before there is a GPS origin, so nothing is owed yet.
    await expect(preview.getByTestId('map-attribution')).toBeHidden();

    await page.getByTestId('zero-datum-gps-input').fill('24.550123, -81.780456');
    await expect(page.getByTestId('zero-datum-gps-parsed')).toBeVisible({ timeout: 5000 });

    const toggle = preview.getByTestId('map-attribution-toggle');
    await expect(toggle).toBeVisible({ timeout: 5000 });
    // Reachable without hovering at all: the names are on the button itself.
    await expect(toggle).toHaveAttribute('title', /Esri/);

    await toggle.click();
    const panel = page.getByTestId('map-attribution-panel');
    await expect(panel).toBeVisible();
    await expect(panel).toContainText('Esri');

    // The preview clips its own children; the panel must not be one of them.
    const escapesPreview = await page.evaluate(() => {
      const preview = document.querySelector('[data-testid="section-preview"]');
      const el = document.querySelector('[data-testid="map-attribution-panel"]');
      if (!preview || !el) return 'missing';
      return { contained: preview.contains(el), inBody: el.closest('body') !== null };
    });
    expect(escapesPreview).toEqual({ contained: false, inBody: true });

    // Dismiss via the toggle, not Escape: with GPS text entered the dialog is
    // dirty, so DialogWrapper turns Escape into its discard confirm (#526) and
    // the dialog - and this panel - stay mounted.
    await toggle.click();
    // Park the pointer off the toggle first: hovering it re-opens the panel
    // after a 350ms real timer (HOVER_OPEN_MS), the same race the
    // MapAttribution unit tests unhover() around.
    await page.mouse.move(0, 0);
    await expect(panel).toBeHidden();
    await expect(toggle).toHaveAttribute('aria-expanded', 'false');
  });

  test('should skip zero datum selection and open wizard', async ({ page }) => {
    await createProject(page, 'Zero Datum Skip Test');
    await openDmpSelectionDialog(page);
    await proceedToStep2(page);

    await expect(page.getByTestId('dmp-new-import-dialog')).toBeVisible({ timeout: 10000 });
    await expect(page.getByTestId('wizard-manual-placing')).toBeVisible({ timeout: 10000 });
  });
});

test.describe('DMP Import Dialog - Hash Tracking', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  /**
   * Test: duplicate-hash-warning banner NOT visible on first import
   *
   * When check_duplicate_import returns false (default), the warning
   * banner should not appear in the DmpSelectionDialog.
   */
  test('should not show duplicate-hash-warning banner on first import', async ({ page }) => {
    await createProject(page, 'Hash Test No Banner');
    await openDmpSelectionDialog(page);

    // Wait for hash computation to settle before asserting banner absence.
    // The Continue button being enabled indicates hash check has completed
    // and the dialog is fully ready (avoids false positive from checking too early).
    const dialog = page.locator('[data-testid="dmp-selection-dialog"]');
    await expect(dialog.getByTestId('section-row-0')).toBeVisible({ timeout: 5000 });
    await expect(page.locator('[data-testid="continue-button"]')).toBeEnabled({ timeout: 5000 });

    // The duplicate-hash-warning banner should NOT be visible
    // since check_duplicate_import returns false by default
    await expect(page.locator('[data-testid="duplicate-hash-warning"]')).not.toBeVisible();
  });

  /**
   * Test: duplicate-hash-warning banner IS visible when check returns true
   *
   * When check_duplicate_import returns true, the warning banner
   * should appear in the DmpSelectionDialog.
   */
  test('should show duplicate-hash-warning banner when file was previously imported', async ({ page }) => {
    // Set mock to return true for check_duplicate_import
    await page.evaluate(() => {
      const setter = (window as unknown as Record<string, unknown>).__MOCK_SET_CHECK_DUPLICATE_RESULT__ as
        | ((result: boolean) => void)
        | undefined;
      if (setter) setter(true);
    });

    await createProject(page, 'Hash Test With Banner');
    await openDmpSelectionDialog(page);

    // Wait for sections to load (hash check runs on mount)
    const dialog = page.locator('[data-testid="dmp-selection-dialog"]');
    await expect(dialog.getByTestId('section-row-0')).toBeVisible({ timeout: 5000 });

    // The duplicate-hash-warning banner should be visible
    const warning = page.locator('[data-testid="duplicate-hash-warning"]');
    await expect(warning).toBeVisible({ timeout: 5000 });
    await expect(warning).toContainText('File may have been imported before');
  });

  /**
   * Test: file hash passed to import_sections after successful import
   *
   * After a successful import (no errors), the file hash should be
   * included in the import_sections call for backend tracking.
   */
  test('should pass file hash to import_sections after successful import', async ({ page }) => {
    await createProject(page, 'Hash Track Success');
    await createSectionViaStore(page, {
      name: 'Anchor Section',
      shots: [{ azimuth_in: 45, azimuth_out: 45, length: 10, depth_in: -1, depth_out: -2 }],
    });

    await page.evaluate(() => {
      const setSuggestions = (window as unknown as Record<string, unknown>).__MOCK_SET_IMPORT_PLACEMENT_SUGGESTIONS__;
      if (typeof setSuggestions !== 'function') {
        throw new Error('Mock suggestion setter not available');
      }
      (setSuggestions as (suggestions: unknown[]) => void)([
        {
          existingStationIndex: 0,
          importStationIndex: 0,
          score: 0.95,
          matchedShots: 0,
          direction: 'forward',
          isPartial: false,
          confidence: 'high',
          existingSectionName: 'Anchor Section',
          importSectionIndex: 0,
          importSectionName: 'Main Passage',
          targetType: 'existing',
          existingSectionId: 1,
          targetImportIndex: null,
        },
      ]);
    });

    // Capture import_sections arguments for fileHash verification
    await page.evaluate(() => {
      const tauri = (window as unknown as { __TAURI_INTERNALS__: { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> } }).__TAURI_INTERNALS__;
      const originalInvoke = tauri.invoke;
      (window as unknown as { __CAPTURED_IMPORT_SECTIONS_ARGS__?: Array<Record<string, unknown>> }).__CAPTURED_IMPORT_SECTIONS_ARGS__ = [];
      tauri.invoke = async (cmd: string, args?: Record<string, unknown>) => {
        if (cmd === 'import_sections') {
          (window as unknown as { __CAPTURED_IMPORT_SECTIONS_ARGS__?: Array<Record<string, unknown>> }).__CAPTURED_IMPORT_SECTIONS_ARGS__?.push(args ?? {});
        }
        return originalInvoke(cmd, args);
      };
    });

    // Open import dialog and select only Main Passage (no errors)
    await openDmpSelectionDialog(page);
    const dialog = page.locator('[data-testid="dmp-selection-dialog"]');
    await expect(dialog.getByTestId('section-row-0')).toBeVisible({ timeout: 5000 });

    // Select only Main Passage
    await dialog.locator('[data-testid="select-none-button"]').click();
    await page.locator('[data-testid="section-checkbox-0"]').click();

    // Proceed to Step 2
    await proceedToDmpSectionConfig(page);

    // Confirm the suggested connection and finish import.
    await expect(page.getByTestId('wizard-suggestion')).toBeVisible({ timeout: 10000 });
    await page.getByTestId('confirm-button').click();

    // Wait for import_sections call and verify fileHash passed for tracking
    await expect.poll(async () => {
      return page.evaluate(() => {
        const calls = (window as unknown as { __CAPTURED_IMPORT_SECTIONS_ARGS__?: Array<Record<string, unknown>> }).__CAPTURED_IMPORT_SECTIONS_ARGS__ ?? [];
        return calls.length;
      });
    }).toBeGreaterThan(0);

    const lastArgs = await page.evaluate(() => {
      const calls = (window as unknown as { __CAPTURED_IMPORT_SECTIONS_ARGS__?: Array<Record<string, unknown>> }).__CAPTURED_IMPORT_SECTIONS_ARGS__ ?? [];
      return calls[calls.length - 1] ?? null;
    });
    expect(lastArgs).not.toBeNull();
    expect((lastArgs as { fileHash?: string | null }).fileHash ?? '').toContain('mock_hash_');
  });

  /**
   * Test: #567 item 2 - the suggestion phase's section-settings disclosure
   * starts collapsed and preserves a typed name across a collapse/re-open.
   */
  test('suggestion-phase section settings disclosure starts collapsed and preserves a typed name', async ({ page }) => {
    await createProject(page, 'Disclosure Preserve');
    await createSectionViaStore(page, {
      name: 'Anchor Section',
      shots: [{ azimuth_in: 45, azimuth_out: 45, length: 10, depth_in: -1, depth_out: -2 }],
    });

    await page.evaluate(() => {
      const setSuggestions = (window as unknown as Record<string, unknown>).__MOCK_SET_IMPORT_PLACEMENT_SUGGESTIONS__;
      if (typeof setSuggestions !== 'function') {
        throw new Error('Mock suggestion setter not available');
      }
      (setSuggestions as (suggestions: unknown[]) => void)([
        {
          existingStationIndex: 0,
          importStationIndex: 0,
          score: 0.95,
          matchedShots: 0,
          direction: 'forward',
          isPartial: false,
          confidence: 'high',
          existingSectionName: 'Anchor Section',
          importSectionIndex: 0,
          importSectionName: 'Main Passage',
          targetType: 'existing',
          existingSectionId: 1,
          targetImportIndex: null,
        },
      ]);
    });

    await openDmpSelectionDialog(page);
    const dialog = page.getByTestId('dmp-selection-dialog');
    await expect(dialog.getByTestId('section-row-0')).toBeVisible({ timeout: 5000 });
    await dialog.locator('[data-testid="select-none-button"]').click();
    await page.locator('[data-testid="section-checkbox-0"]').click();
    await proceedToDmpSectionConfig(page);

    await expect(page.getByTestId('wizard-suggestion')).toBeVisible({ timeout: 10000 });

    const header = page.getByTestId('wizard-suggestion-section-settings-header');
    const nameInput = page.getByTestId('section-name-input');

    // Collapsed by default.
    await expect(nameInput).not.toBeVisible();

    // Expand, type a name.
    await header.click();
    await expect(nameInput).toBeVisible();
    await nameInput.fill('Renamed Section');
    await expect(nameInput).toHaveValue('Renamed Section');

    // Collapse, then re-open - the typed name survives.
    await header.click();
    await expect(nameInput).not.toBeVisible();
    await header.click();
    await expect(nameInput).toHaveValue('Renamed Section');
  });

  /**
   * Test: a completed DMP import drops its import context (#477)
   *
   * `importContext` says an import flow is in progress, and it decides what
   * `saveAfterImport` does with the next import ('new-project' forces a real
   * save to disk, 'existing-project' only marks dirty). Every other importer
   * ends through `completeImportToSingleView`, which drops it; both failure
   * routes drop it via `surfaceImportFailure`. The DMP wizard's completion in
   * App.tsx was a hand-rolled copy that did four of those five things, so a
   * finished DMP import left the app believing one was still running.
   */
  test('should clear importContext after the wizard completes @smoke', async ({ page }) => {
    await createProject(page, 'Import Context Clear');
    await createSectionViaStore(page, {
      name: 'Anchor Section',
      shots: [{ azimuth_in: 45, azimuth_out: 45, length: 10, depth_in: -1, depth_out: -2 }],
    });

    await page.evaluate(() => {
      const setSuggestions = (window as unknown as Record<string, unknown>).__MOCK_SET_IMPORT_PLACEMENT_SUGGESTIONS__;
      if (typeof setSuggestions !== 'function') {
        throw new Error('Mock suggestion setter not available');
      }
      (setSuggestions as (suggestions: unknown[]) => void)([
        {
          existingStationIndex: 0,
          importStationIndex: 0,
          score: 0.95,
          matchedShots: 0,
          direction: 'forward',
          isPartial: false,
          confidence: 'high',
          existingSectionName: 'Anchor Section',
          importSectionIndex: 0,
          importSectionName: 'Main Passage',
          targetType: 'existing',
          existingSectionId: 1,
          targetImportIndex: null,
        },
      ]);
    });

    // Ctrl+I routes the mocked file by extension, and that router is what SETS
    // the context - so read it back before proceeding, or a wizard that never
    // set one would make the assertion below vacuous.
    await openDmpSelectionDialog(page);
    expect(await readImportContext(page)).toBe('existing-project');

    const dialog = page.getByTestId('dmp-selection-dialog');
    await expect(dialog.getByTestId('section-row-0')).toBeVisible({ timeout: 5000 });
    await dialog.locator('[data-testid="select-none-button"]').click();
    await page.locator('[data-testid="section-checkbox-0"]').click();

    await proceedToDmpSectionConfig(page);

    await expect(page.getByTestId('wizard-suggestion')).toBeVisible({ timeout: 10000 });
    await page.getByTestId('confirm-button').click();

    // Anchor on the import actually LANDING, not on the context going null:
    // a cancelled wizard also closes and also (correctly) clears the context,
    // so without this the test could pass having exercised nothing. A clean
    // receipt is surfaced as a toast rather than the modal, so the section
    // count is the honest signal that the shared exit ran.
    await expect(page.getByTestId('dmp-new-import-dialog')).toBeHidden({ timeout: 15000 });
    await expect.poll(() => countSections(page), { timeout: 10000 }).toBe(2);

    expect(await readImportContext(page)).toBeNull();
  });

  /**
   * Test: a section with validation errors does not import before an explicit
   * placement action.
   *
   * The file hash rides on `import_sections` (it is recorded inside that
   * command), so "nothing imported" is also "no hash recorded". This used to be
   * titled after `track_imported_file`, an IPC command the frontend never
   * called and #576 NF-TRACK-IMPORTED removed; the title now says what the
   * assertion checks.
   */
  test('does not import a section with validation errors before an explicit placement action', async ({ page }) => {
    await createProject(page, 'Hash Track Error');
    await page.evaluate(() => {
      const resetSuggestions = (window as unknown as Record<string, unknown>).__MOCK_RESET_IMPORT_PLACEMENT_SUGGESTIONS__;
      if (typeof resetSuggestions === 'function') {
        (resetSuggestions as () => void)();
      }
    });

    // Capture import_sections arguments to confirm no import occurs
    await page.evaluate(() => {
      const tauri = (window as unknown as { __TAURI_INTERNALS__: { invoke: (cmd: string, args?: Record<string, unknown>) => Promise<unknown> } }).__TAURI_INTERNALS__;
      const originalInvoke = tauri.invoke;
      (window as unknown as { __CAPTURED_IMPORT_SECTIONS_ARGS__?: Array<Record<string, unknown>> }).__CAPTURED_IMPORT_SECTIONS_ARGS__ = [];
      tauri.invoke = async (cmd: string, args?: Record<string, unknown>) => {
        if (cmd === 'import_sections') {
          (window as unknown as { __CAPTURED_IMPORT_SECTIONS_ARGS__?: Array<Record<string, unknown>> }).__CAPTURED_IMPORT_SECTIONS_ARGS__?.push(args ?? {});
        }
        return originalInvoke(cmd, args);
      };
    });

    // Open import dialog and select only the Validation Test Section (has errors)
    await openDmpSelectionDialog(page);
    const dialog = page.locator('[data-testid="dmp-selection-dialog"]');
    await expect(dialog.getByTestId('section-row-0')).toBeVisible({ timeout: 5000 });

    // Select only Validation Test Section (index 5)
    await dialog.locator('[data-testid="select-none-button"]').click();
    await page.locator('[data-testid="section-checkbox-5"]').click();

    // Proceed to Step 2
    await proceedToDmpSectionConfig(page);

    await expect(page.getByTestId('dmp-new-import-dialog')).toBeVisible({ timeout: 10000 });
    await expect(page.getByTestId('wizard-manual-placing')).toBeVisible({ timeout: 10000 });

    // Verify import_sections was NOT called without an explicit placement action.
    const calls = await page.evaluate(() => {
      return (window as unknown as { __CAPTURED_IMPORT_SECTIONS_ARGS__?: Array<Record<string, unknown>> }).__CAPTURED_IMPORT_SECTIONS_ARGS__ ?? [];
    });
    expect(calls.length).toBe(0);

    // Cleanly close wizard; confirmation is conditional depending on state.
    const wizard = page.getByTestId('dmp-new-import-dialog');
    if (await wizard.isVisible()) {
      await wizard.getByTestId('cancel-button').click();
      const cancelConfirmDialog = page.getByTestId('cancel-confirm-dialog');
      if (await cancelConfirmDialog.isVisible()) {
        await page.getByTestId('confirm-cancel-button').click();
      }
      await expect(wizard).toBeHidden({ timeout: 10000 });
    }
  });
});
