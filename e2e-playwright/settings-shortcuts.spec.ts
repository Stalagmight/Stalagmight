import { test, expect, Page } from './fixtures/coverage';
import { setupTauriMocks } from './mocks/tauri-ipc';
import { openSettingsFromMenu, closeProjectViaMenu } from './helpers/app';

/**
 * E2E tests for keyboard shortcuts configuration in Settings dialog.
 * Tests recording, conflict detection, reset functionality, search filtering,
 * save/cancel behavior, and shortcut functionality after save.
 */

/** Platform-aware key display names (macOS uses symbols, Linux/Windows uses text) */
const IS_MAC = process.platform === 'darwin';
const KEY = {
  Ctrl: IS_MAC ? '\u2318' : 'Ctrl',       // ⌘ or Ctrl
  Alt: IS_MAC ? '\u2325' : 'Alt',          // ⌥ or Alt
  Shift: IS_MAC ? '\u21E7' : 'Shift',      // ⇧ or Shift
  Escape: IS_MAC ? '\u238B' : 'Escape',    // ⎋ or Escape
  Delete: IS_MAC ? '\u2326' : 'Delete',    // ⌦ or Delete
  Backspace: IS_MAC ? '\u232B' : 'Backspace', // ⌫ or Backspace
  Tab: IS_MAC ? '\u21E5' : 'Tab',          // ⇥ or Tab
  Enter: IS_MAC ? '\u21A9' : 'Enter',      // ↩ or Enter
};

/**
 * Helper to create a new project with mock sections/stations
 */
async function createProject(page: Page, name: string): Promise<void> {
  const newProjectBtn = page.locator('button:has-text("New Project"), h3:has-text("New Project")').first();
  await newProjectBtn.click();
  await page.waitForSelector('.fixed.inset-0');
  await page.locator('.fixed.inset-0 input').first().fill(name);
  await page.click('.fixed.inset-0 button:has-text("Create")');
  await page.waitForSelector('canvas', { timeout: 10000 });

  // Wait for map to be fully initialized (per memory pitfall)
  await page.waitForFunction(() => (window as unknown as { __stalagmight_map__?: unknown }).__stalagmight_map__ !== undefined, { timeout: 10000 });
}

/**
 * Helper to go back to GlobalView from a project view
 */
async function goBackToGlobalView(page: Page): Promise<void> {
  // Close Project is now a hamburger-menu item; closeProjectViaMenu opens the
  // menu, clicks the back-button item, and waits for global-view.
  if (await page.getByTestId('global-view').isVisible().catch(() => false)) return;
  await closeProjectViaMenu(page);
}

/**
 * Helper to open Settings dialog
 * Settings is only accessible from GlobalView, so we need to navigate there first
 */
async function openSettingsDialog(page: Page): Promise<void> {
  // Go back to GlobalView if we're in a project
  await goBackToGlobalView(page);

  // Settings now lives behind the hamburger menu in the GlobalView header.
  await openSettingsFromMenu(page);
}

/**
 * Helper to navigate to the Shortcuts category via sidebar
 * Updated for new sidebar layout (was tab-based, now sidebar-based)
 */
async function navigateToShortcutsCategory(page: Page): Promise<void> {
  // Click on the Shortcuts item in the settings sidebar
  await page.locator('[data-testid="settings-tab-shortcuts"]').click();
  // Wait for the shortcuts panel to become visible
  await expect(page.locator('[data-testid="settings-panel-shortcuts"]')).toBeVisible({ timeout: 2000 });
}

/**
 * Helper to wait for ShortcutRecorder recording state
 */
async function waitForRecording(page: Page, testId: string, isRecording: boolean): Promise<void> {
  const recorder = page.locator(`[data-testid="${testId}"]`);
  await expect(recorder).toHaveAttribute('data-recording', String(isRecording), { timeout: 2000 });
}

test.describe('Settings Dialog - Shortcuts Tab', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Shortcuts Test Project');
  });

  test.describe('Navigation', () => {
    test('opens Settings dialog and navigates to Shortcuts tab', async ({ page }) => {
      await openSettingsDialog(page);
      await navigateToShortcutsCategory(page);

      // Verify shortcuts panel content is visible
      await expect(page.locator('[data-testid="settings-panel-shortcuts"]')).toBeVisible();

      // Verify search input exists
      await expect(page.locator('[data-testid="shortcuts-search"]')).toBeVisible();

      // Verify reserved shortcuts section exists
      await expect(page.locator('[data-testid="reserved-shortcuts"]')).toBeVisible();

      // Verify categories are visible (File is now the first category)
      await expect(page.locator('[data-testid="shortcut-category-file"]')).toBeVisible();
      await expect(page.locator('[data-testid="shortcut-category-tools"]')).toBeVisible();
    });
  });

  test.describe('Search Filtering', () => {
    test('search filters actions by name', async ({ page }) => {
      await openSettingsDialog(page);
      await navigateToShortcutsCategory(page);

      // Verify Select action is visible initially
      await expect(page.locator('[data-testid="shortcut-row-select"]')).toBeVisible();

      // Search for "zoom"
      await page.locator('[data-testid="shortcuts-search"]').fill('zoom');

      // Zoom-related actions should be visible
      await expect(page.locator('[data-testid="shortcut-row-zoom"]')).toBeVisible();
      await expect(page.locator('[data-testid="shortcut-row-zoom_in"]')).toBeVisible();
      await expect(page.locator('[data-testid="shortcut-row-zoom_out"]')).toBeVisible();

      // Non-matching actions should be hidden
      await expect(page.locator('[data-testid="shortcut-row-select"]')).not.toBeVisible();
      await expect(page.locator('[data-testid="shortcut-row-pan"]')).not.toBeVisible();
    });

    test('search filters by key binding', async ({ page }) => {
      await openSettingsDialog(page);
      await navigateToShortcutsCategory(page);

      // Search for "Ctrl+Z" (undo binding)
      await page.locator('[data-testid="shortcuts-search"]').fill('Ctrl+Z');

      // Undo action (which has Ctrl+Z) should be visible
      await expect(page.locator('[data-testid="shortcut-row-undo"]')).toBeVisible();

      // Unrelated actions should be hidden
      await expect(page.locator('[data-testid="shortcut-row-select"]')).not.toBeVisible();
    });

    test('clearing search shows all actions', async ({ page }) => {
      await openSettingsDialog(page);
      await navigateToShortcutsCategory(page);

      // Search for something specific
      await page.locator('[data-testid="shortcuts-search"]').fill('undo');
      // The filter applied (so the clear below has something to undo).
      await expect(page.locator('[data-testid="shortcut-row-select"]')).not.toBeVisible();

      // Clear search
      await page.locator('[data-testid="shortcuts-search"]').fill('');

      // All categories and actions should be visible again
      await expect(page.locator('[data-testid="shortcut-category-tools"]')).toBeVisible();
      await expect(page.locator('[data-testid="shortcut-row-select"]')).toBeVisible();
      await expect(page.locator('[data-testid="shortcut-row-undo"]')).toBeVisible();
    });
  });

  test.describe('Shortcut Recording', () => {
    test('records new shortcut via keyboard', async ({ page }) => {
      await openSettingsDialog(page);
      await navigateToShortcutsCategory(page);

      // Click on the primary shortcut recorder for "select" action
      const recorder = page.locator('[data-testid="shortcut-primary-select"]');

      // Verify original shortcut is S
      await expect(recorder).toContainText('S');

      await recorder.click();

      // Verify recording state is active
      await waitForRecording(page, 'shortcut-primary-select', true);

      // Press a new key combination
      await page.keyboard.press('Q');

      // Recording should complete and show the new shortcut
      await waitForRecording(page, 'shortcut-primary-select', false);

      // Verify the shortcut was recorded (should show Q in key badge)
      // Note: The ARIA status says "Shortcut set to Q" so we check for the key badge specifically
      await expect(recorder.locator('span').filter({ hasText: 'Q' }).first()).toBeVisible();
    });

    test('Escape cancels recording', async ({ page }) => {
      await openSettingsDialog(page);
      await navigateToShortcutsCategory(page);

      const recorder = page.locator('[data-testid="shortcut-primary-select"]');

      // Verify original shortcut is S
      await expect(recorder).toContainText('S');

      // Start recording
      await recorder.click();
      await waitForRecording(page, 'shortcut-primary-select', true);

      // Cancel with Escape
      await page.keyboard.press('Escape');

      // Recording should be canceled
      await waitForRecording(page, 'shortcut-primary-select', false);

      // Original value should be preserved (S)
      await expect(recorder).toContainText('S');
    });

    test('records modifier combinations', async ({ page }) => {
      await openSettingsDialog(page);
      await navigateToShortcutsCategory(page);

      // Record a modifier combination for the secondary slot of "select"
      const recorder = page.locator('[data-testid="shortcut-secondary-select"]');
      await recorder.click();
      await waitForRecording(page, 'shortcut-secondary-select', true);

      // Press Ctrl+Shift+W
      await page.keyboard.press('Control+Shift+W');

      await waitForRecording(page, 'shortcut-secondary-select', false);

      // Verify the modifier combination is shown
      await expect(recorder).toContainText(KEY.Ctrl);
      await expect(recorder).toContainText(KEY.Shift);
      await expect(recorder).toContainText('W');
    });
  });

  test.describe('Conflict Detection', () => {
    test('shows error when recording a reserved shortcut', async ({ page }) => {
      await openSettingsDialog(page);
      await navigateToShortcutsCategory(page);

      // Records on MEASURE, not pan (#409): the Pan/Zoom rows are rendered
      // disabled while `map.controls.show_navigation_tools` is off, so their
      // recorders refuse to start recording at all - a reserved-key probe there
      // would test the disabled gate, not the reserved-key check.
      // Try to assign a reserved shortcut (Escape)
      const recorder = page.locator('[data-testid="shortcut-primary-measure"]');
      await recorder.click();
      await waitForRecording(page, 'shortcut-primary-measure', true);

      // Press Escape - this should be blocked (reserved key)
      // Note: Escape cancels recording, so we try a different reserved one
      // Let's use Delete
      await page.keyboard.press('Delete');

      // Check for error message about reserved shortcut
      await expect(page.locator('[data-testid="shortcut-error"]')).toBeVisible({ timeout: 2000 });
      await expect(page.locator('[data-testid="shortcut-error"]')).toContainText('cannot be assigned');
    });

    test('shows error when recording a duplicate shortcut', async ({ page }) => {
      await openSettingsDialog(page);
      await navigateToShortcutsCategory(page);

      // Try to assign "P" to select (P is already used by pan)
      const recorder = page.locator('[data-testid="shortcut-primary-select"]');
      await recorder.click();
      await waitForRecording(page, 'shortcut-primary-select', true);

      // Press P (which is already assigned to pan)
      await page.keyboard.press('P');
      await waitForRecording(page, 'shortcut-primary-select', false);

      // Check for conflict error message
      await expect(page.locator('[data-testid="shortcut-error"]')).toBeVisible({ timeout: 2000 });
      await expect(page.locator('[data-testid="shortcut-error"]')).toContainText('already used by');
    });

    test('shows error when assigning same shortcut to both slots of same action', async ({ page }) => {
      await openSettingsDialog(page);
      await navigateToShortcutsCategory(page);

      // First, set primary slot of merge to 'K'
      const primaryRecorder = page.locator('[data-testid="shortcut-primary-merge"]');
      await primaryRecorder.click();
      await waitForRecording(page, 'shortcut-primary-merge', true);
      await page.keyboard.press('K');
      await waitForRecording(page, 'shortcut-primary-merge', false);

      // Wait for the first assignment to commit (merge's key badge reads exactly
      // K, its default is M; an exact match, since `hasText: 'K'` would match the
      // recorder's "Press keys..." prompt); only then does "no error" prove the
      // first assignment raised none, so the one asserted below comes from the second.
      await expect(primaryRecorder.locator('span').filter({ hasText: /^K$/ }).first()).toBeVisible();
      await expect(page.locator('[data-testid="shortcut-error"]')).not.toBeVisible();

      // Try to assign same key to secondary slot
      const secondaryRecorder = page.locator('[data-testid="shortcut-secondary-merge"]');
      await secondaryRecorder.click();
      await waitForRecording(page, 'shortcut-secondary-merge', true);
      await page.keyboard.press('K');
      await waitForRecording(page, 'shortcut-secondary-merge', false);

      // Should show error about intra-action duplicate
      await expect(page.locator('[data-testid="shortcut-error"]')).toBeVisible({ timeout: 2000 });
      await expect(page.locator('[data-testid="shortcut-error"]')).toContainText('already assigned');
    });
  });

  test.describe('Reset Functionality', () => {
    test('reset single shortcut restores default', async ({ page }) => {
      await openSettingsDialog(page);
      await navigateToShortcutsCategory(page);

      // Change the select shortcut
      const recorder = page.locator('[data-testid="shortcut-primary-select"]');
      await recorder.click();
      await waitForRecording(page, 'shortcut-primary-select', true);
      await page.keyboard.press('Q');
      await waitForRecording(page, 'shortcut-primary-select', false);

      // Verify shortcut changed
      await expect(recorder).toContainText('Q');

      // Click reset button for select action
      const resetButton = page.locator('[data-testid="shortcut-reset-select"]');
      await expect(resetButton).not.toBeDisabled(); // Should be enabled since we changed it
      await resetButton.click();

      // Verify shortcut is back to default (S)
      await expect(recorder).toContainText('S');
    });

    test('reset button is disabled when no changes', async ({ page }) => {
      await openSettingsDialog(page);
      await navigateToShortcutsCategory(page);

      // Reset button should be disabled for unchanged shortcut
      const resetButton = page.locator('[data-testid="shortcut-reset-select"]');
      await expect(resetButton).toBeDisabled();
    });

    test('reset cancels active recording and restores default', async ({ page }) => {
      await openSettingsDialog(page);
      await navigateToShortcutsCategory(page);

      // First change the select shortcut so reset button is enabled
      const recorder = page.locator('[data-testid="shortcut-primary-select"]');
      await recorder.click();
      await waitForRecording(page, 'shortcut-primary-select', true);
      await page.keyboard.press('Q');
      await waitForRecording(page, 'shortcut-primary-select', false);
      await expect(recorder).toContainText('Q');

      // Start recording again on the same recorder
      await recorder.click();
      await waitForRecording(page, 'shortcut-primary-select', true);

      // Click reset while recording is active
      const resetButton = page.locator('[data-testid="shortcut-reset-select"]');
      await resetButton.click();

      // Recording should be terminated
      await waitForRecording(page, 'shortcut-primary-select', false);

      // Shortcut should be back to default (S)
      await expect(recorder).toContainText('S');
    });

    test('Reset All restores all shortcuts to defaults', async ({ page }) => {
      await openSettingsDialog(page);
      await navigateToShortcutsCategory(page);

      // Change multiple shortcuts
      const selectRecorder = page.locator('[data-testid="shortcut-primary-select"]');
      await selectRecorder.click();
      await page.keyboard.press('Q');
      await waitForRecording(page, 'shortcut-primary-select', false);

      // Second row is MEASURE, not pan (#409): the pan row is disabled while
      // the navigation tools are hidden, so it cannot be customized here.
      const measureRecorder = page.locator('[data-testid="shortcut-primary-measure"]');
      await measureRecorder.click();
      await page.keyboard.press('W');
      await waitForRecording(page, 'shortcut-primary-measure', false);

      // Click Reset All button
      const resetAllButton = page.locator('[data-testid="shortcuts-reset-all"]');
      await resetAllButton.click();

      // Verify all shortcuts are back to defaults
      await expect(selectRecorder).toContainText('S');
      await expect(measureRecorder).toContainText('R');
    });
  });

  test.describe('Save and Cancel', () => {
    test('Save persists changes', async ({ page }) => {
      await openSettingsDialog(page);
      await navigateToShortcutsCategory(page);

      // Change a shortcut
      const recorder = page.locator('[data-testid="shortcut-primary-select"]');
      await recorder.click();
      await page.keyboard.press('Q');
      await waitForRecording(page, 'shortcut-primary-select', false);

      // Verify change is shown
      await expect(recorder).toContainText('Q');

      // Save changes
      await page.locator('[data-testid="settings-save-button"]').click();

      // Dialog should close
      await expect(page.locator('[data-testid="settings-dialog"]')).not.toBeVisible({ timeout: 5000 });

      // Reopen settings and verify change persisted
      await openSettingsDialog(page);
      await navigateToShortcutsCategory(page);

      // Re-find the recorder after dialog reopened
      const recorderAfter = page.locator('[data-testid="shortcut-primary-select"]');
      await expect(recorderAfter).toContainText('Q');
    });

    test('Cancel discards changes', async ({ page }) => {
      await openSettingsDialog(page);
      await navigateToShortcutsCategory(page);

      // Change a shortcut
      const recorder = page.locator('[data-testid="shortcut-primary-select"]');
      await recorder.click();
      await page.keyboard.press('Q');
      await waitForRecording(page, 'shortcut-primary-select', false);

      // Verify change is shown locally
      await expect(recorder).toContainText('Q');

      // Cancel using testid
      await page.locator('[data-testid="settings-cancel-button"]').click();

      // Handle potential confirmation dialog for dirty state using testids
      const confirmDialog = page.locator('[data-testid="unsaved-changes-dialog"]');
      if (await confirmDialog.isVisible({ timeout: 1000 }).catch(() => false)) {
        await page.locator('[data-testid="unsaved-discard"]').click();
      }

      // Dialog should close
      await expect(page.locator('[data-testid="settings-dialog"]')).not.toBeVisible({ timeout: 5000 });

      // Reopen settings and verify change was NOT persisted
      await openSettingsDialog(page);
      await navigateToShortcutsCategory(page);

      // Re-find the recorder after dialog reopened
      const recorderAfter = page.locator('[data-testid="shortcut-primary-select"]');
      // Should show original shortcut (S), not changed one (Q)
      await expect(recorderAfter).toContainText('S');
    });
  });

  test.describe('Shortcut Functionality After Save', () => {
    // Rebinds MERGE, not pan (#409): the pan tool is off the toolbar unless
    // `map.controls.show_navigation_tools` is on, and while it is hidden the
    // shortcut handler ignores the pan action entirely - so a rebind-to-Q test
    // on pan would prove nothing about rebinding, only about the gate. The pan
    // ROW still exists in Settings (the config is unconditional), which is why
    // the conflict cases elsewhere in this file still key on it.
    test('customized shortcut works after save', async ({ page }) => {
      // beforeEach has already created a project and we're in project view
      // Open settings from GlobalView (need to go back first)
      await openSettingsDialog(page);
      await navigateToShortcutsCategory(page);

      // Change the merge tool shortcut from M to Q
      const recorder = page.locator('[data-testid="shortcut-primary-merge"]');
      await recorder.click();
      await page.keyboard.press('Q');
      await waitForRecording(page, 'shortcut-primary-merge', false);

      // Save
      await page.locator('[data-testid="settings-save-button"]').click();
      await expect(page.locator('[data-testid="settings-dialog"]')).not.toBeVisible({ timeout: 3000 });

      // Create a new project to test shortcut functionality
      // (we're now in GlobalView after Settings closed)
      await createProject(page, 'Shortcut Test Project');

      const activeTool = () => page.evaluate(() => {
        const stores = (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { activeTool: string } } } }).__E2E_TEST__;
        return stores?.uiStore?.getState()?.activeTool ?? 'unknown';
      });

      // Press Q (new merge shortcut)
      await page.keyboard.press('Q');

      // Verify merge tool is now active (poll the store rather than sleeping)
      await expect.poll(activeTool).toBe('merge');

      // Verify old shortcut (M) no longer activates merge
      // Switch to select first
      await page.keyboard.press('S');
      await expect.poll(activeTool).toBe('select');

      await page.keyboard.press('M');
      // settle: proves the tool does NOT change, and a poll would accept the
      // pre-update value.
      await page.waitForTimeout(100);
      expect(await activeTool()).toBe('select'); // Should stay as select
    });
  });

  test.describe('Category Collapse/Expand', () => {
    test('categories can be collapsed and expanded', async ({ page }) => {
      await openSettingsDialog(page);
      await navigateToShortcutsCategory(page);

      // Verify Tools category is expanded by default
      const toolsToggle = page.locator('[data-testid="shortcut-category-toggle-tools"]');
      await expect(page.locator('[data-testid="shortcut-row-select"]')).toBeVisible();

      // Collapse Tools category
      await toolsToggle.click();

      // Actions should be hidden
      await expect(page.locator('[data-testid="shortcut-row-select"]')).not.toBeVisible();

      // Expand again
      await toolsToggle.click();

      // Actions should be visible again
      await expect(page.locator('[data-testid="shortcut-row-select"]')).toBeVisible();
    });
  });

  test.describe('Reserved Shortcuts Display', () => {
    test('reserved shortcuts are displayed correctly', async ({ page }) => {
      await openSettingsDialog(page);
      await navigateToShortcutsCategory(page);

      const reserved = page.locator('[data-testid="reserved-shortcuts"]');
      await expect(reserved).toBeVisible();

      // Verify reserved shortcuts are shown (platform-aware display)
      await expect(reserved).toContainText(KEY.Escape);
      await expect(reserved).toContainText(KEY.Delete);
      await expect(reserved).toContainText(KEY.Backspace);
      await expect(reserved).toContainText(KEY.Tab);
      await expect(reserved).toContainText(KEY.Enter);
      await expect(reserved).toContainText(IS_MAC ? `${KEY.Ctrl}A` : 'Ctrl+A');

      // Verify grouped descriptions
      await expect(reserved).toContainText('Delete selection');
      await expect(reserved).toContainText('UI Navigation');

      // Ctrl+I and Ctrl+E should NOT be in reserved section (they are now configurable)
      await expect(reserved).not.toContainText(IS_MAC ? `${KEY.Ctrl}I` : 'Ctrl+I');
      await expect(reserved).not.toContainText(IS_MAC ? `${KEY.Ctrl}E` : 'Ctrl+E');
    });

    test('reserved shortcuts section is positioned above search bar', async ({ page }) => {
      await openSettingsDialog(page);
      await navigateToShortcutsCategory(page);

      const reserved = page.locator('[data-testid="reserved-shortcuts"]');
      const search = page.locator('[data-testid="shortcuts-search"]');

      await expect(reserved).toBeVisible();
      await expect(search).toBeVisible();

      // Verify reserved section precedes search bar in DOM order
      // compareDocumentPosition returns bitmask; bit 4 (DOCUMENT_POSITION_FOLLOWING) means search follows reserved
      const reservedPrecedesSearch = await page.evaluate(() => {
        const reservedEl = document.querySelector('[data-testid="reserved-shortcuts"]');
        const searchEl = document.querySelector('[data-testid="shortcuts-search"]');
        if (!reservedEl || !searchEl) return false;
        // Bit 4 = DOCUMENT_POSITION_FOLLOWING: searchEl follows reservedEl
        return (reservedEl.compareDocumentPosition(searchEl) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0;
      });
      expect(reservedPrecedesSearch).toBe(true);
    });
  });

  test.describe('File Category Shortcuts', () => {
    test('File category with Import DMP and Export shortcuts is visible', async ({ page }) => {
      await openSettingsDialog(page);
      await navigateToShortcutsCategory(page);

      // Verify File category exists
      await expect(page.locator('[data-testid="shortcut-category-file"]')).toBeVisible();

      // Verify Import DMP and Export rows exist
      await expect(page.locator('[data-testid="shortcut-row-import_dmp"]')).toBeVisible();
      await expect(page.locator('[data-testid="shortcut-row-export"]')).toBeVisible();

      // Verify default bindings
      const importRecorder = page.locator('[data-testid="shortcut-primary-import_dmp"]');
      await expect(importRecorder).toContainText(KEY.Ctrl);
      await expect(importRecorder).toContainText('I');

      const exportRecorder = page.locator('[data-testid="shortcut-primary-export"]');
      await expect(exportRecorder).toContainText(KEY.Ctrl);
      await expect(exportRecorder).toContainText('E');
    });

    test('Ctrl+I and Ctrl+E can be reassigned without conflict error', async ({ page }) => {
      await openSettingsDialog(page);
      await navigateToShortcutsCategory(page);

      // Reassign Import DMP from Ctrl+I to Ctrl+Shift+K (avoid Ctrl+Shift+I which opens browser DevTools)
      const importRecorder = page.locator('[data-testid="shortcut-primary-import_dmp"]');
      await importRecorder.click();
      await waitForRecording(page, 'shortcut-primary-import_dmp', true);
      await page.keyboard.press('Control+Shift+K');
      await waitForRecording(page, 'shortcut-primary-import_dmp', false);

      // Verify new binding is shown
      await expect(importRecorder).toContainText(KEY.Ctrl);
      await expect(importRecorder).toContainText(KEY.Shift);
      await expect(importRecorder).toContainText('K');

      // No error should appear
      await expect(page.locator('[data-testid="shortcut-error"]')).not.toBeVisible();

      // Now assign Ctrl+I to a different action (select) - should NOT conflict since it was freed
      const selectRecorder = page.locator('[data-testid="shortcut-primary-select"]');
      await selectRecorder.click();
      await waitForRecording(page, 'shortcut-primary-select', true);
      await page.keyboard.press('Control+I');
      await waitForRecording(page, 'shortcut-primary-select', false);

      // Should show Ctrl+I without error (it's no longer reserved or taken)
      await expect(selectRecorder).toContainText(KEY.Ctrl);
      await expect(selectRecorder).toContainText('I');
      await expect(page.locator('[data-testid="shortcut-error"]')).not.toBeVisible();

      // Also reassign Export from Ctrl+E to Ctrl+Alt+E (avoid Ctrl+Shift+J which opens browser Console)
      const exportRecorder = page.locator('[data-testid="shortcut-primary-export"]');
      await exportRecorder.click();
      await waitForRecording(page, 'shortcut-primary-export', true);
      await page.keyboard.press('Control+Alt+E');
      await waitForRecording(page, 'shortcut-primary-export', false);

      // Verify new binding is shown
      await expect(exportRecorder).toContainText(KEY.Ctrl);
      await expect(exportRecorder).toContainText(KEY.Alt);
      await expect(exportRecorder).toContainText('E');

      // No error should appear
      await expect(page.locator('[data-testid="shortcut-error"]')).not.toBeVisible();

      // Assign freed Ctrl+E to MEASURE - should NOT conflict since it was freed.
      // (Measure, not pan: the pan row is disabled while the navigation tools
      // are hidden (#409) and its recorder will not accept a rebind.)
      const measureRecorder = page.locator('[data-testid="shortcut-primary-measure"]');
      await measureRecorder.click();
      await waitForRecording(page, 'shortcut-primary-measure', true);
      await page.keyboard.press('Control+E');
      await waitForRecording(page, 'shortcut-primary-measure', false);

      // Should show Ctrl+E without error
      await expect(measureRecorder).toContainText(KEY.Ctrl);
      await expect(measureRecorder).toContainText('E');
      await expect(page.locator('[data-testid="shortcut-error"]')).not.toBeVisible();
    });

    test('customized Import DMP shortcut triggers import dialog', async ({ page }) => {
      // We start in project view from beforeEach
      await openSettingsDialog(page);
      await navigateToShortcutsCategory(page);

      // Change Import DMP shortcut from Ctrl+I to Ctrl+Shift+M
      const importRecorder = page.locator('[data-testid="shortcut-primary-import_dmp"]');
      await importRecorder.click();
      await waitForRecording(page, 'shortcut-primary-import_dmp', true);
      await page.keyboard.press('Control+Shift+M');
      await waitForRecording(page, 'shortcut-primary-import_dmp', false);

      // Save settings
      await page.locator('[data-testid="settings-save-button"]').click();
      await expect(page.locator('[data-testid="settings-dialog"]')).not.toBeVisible({ timeout: 3000 });

      // Navigate to a project so import shortcut is handled
      await createProject(page, 'Import Shortcut Test');

      // Press the customized shortcut (Ctrl+Shift+M)
      await page.keyboard.press('Control+Shift+M');

      // The import flow should trigger - in mock context, the file dialog returns
      // a mock path and the dmp-selection dialog should appear
      await expect(page.locator('[data-testid="dmp-selection-dialog"]')).toBeVisible({ timeout: 5000 });
    });
  });

  test.describe('Clear Shortcut', () => {
    test('clear button removes shortcut binding', async ({ page }) => {
      await openSettingsDialog(page);
      await navigateToShortcutsCategory(page);

      const recorder = page.locator('[data-testid="shortcut-primary-select"]');

      // Verify shortcut exists
      await expect(recorder).toContainText('S');

      // Click clear button
      const clearButton = page.locator('[data-testid="shortcut-primary-select-clear"]');
      await clearButton.click();

      // Shortcut should be cleared (placeholder text visible)
      await expect(recorder.locator('text=Click to record')).toBeVisible();
    });
  });
});

test.describe('Visual Regression - Shortcuts', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Visual Test Project');
  });

  test('empty shortcut slot state', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToShortcutsCategory(page);

    // Take screenshot of shortcuts tab (secondary slots are often empty)
    await page.screenshot({
      path: 'e2e-playwright/screenshots/shortcuts-empty-state.png',
      fullPage: false,
    });
  });

  test('recording state', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToShortcutsCategory(page);

    // Start recording
    const recorder = page.locator('[data-testid="shortcut-primary-select"]');
    await recorder.click();
    await waitForRecording(page, 'shortcut-primary-select', true);

    // Take screenshot showing recording state
    await page.screenshot({
      path: 'e2e-playwright/screenshots/shortcuts-recording-state.png',
      fullPage: false,
    });
  });

  test('conflict error state', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToShortcutsCategory(page);

    // Create a conflict
    const recorder = page.locator('[data-testid="shortcut-primary-select"]');
    await recorder.click();
    await page.keyboard.press('P'); // Conflicts with pan
    await waitForRecording(page, 'shortcut-primary-select', false);

    // Wait for error to appear
    await expect(page.locator('[data-testid="shortcut-error"]')).toBeVisible();

    // Take screenshot showing error state
    await page.screenshot({
      path: 'e2e-playwright/screenshots/shortcuts-conflict-state.png',
      fullPage: false,
    });
  });

  test('customized shortcuts state', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToShortcutsCategory(page);

    // Customize multiple shortcuts
    const selectRecorder = page.locator('[data-testid="shortcut-primary-select"]');
    await selectRecorder.click();
    await page.keyboard.press('Q');
    await waitForRecording(page, 'shortcut-primary-select', false);

    // Add secondary binding. [#576 RV6b-F3] Not Ctrl+Q: with the whole default Config
    // that is quit's binding, so the recording was refused with the conflict error and
    // the screenshot below showed the error state, not a customized one. Ctrl+Shift+Q is
    // bound to nothing by default.
    const secondaryRecorder = page.locator('[data-testid="shortcut-secondary-select"]');
    await secondaryRecorder.click();
    await page.keyboard.press('Control+Shift+Q');
    await waitForRecording(page, 'shortcut-secondary-select', false);

    // Both bindings took, and nothing was refused - the state the screenshot is named for.
    await expect(page.locator('[data-testid="shortcut-error"]')).toHaveCount(0);
    await expect(selectRecorder).toContainText('Q');
    await expect(secondaryRecorder).toContainText(KEY.Ctrl);
    await expect(secondaryRecorder).toContainText(KEY.Shift);
    await expect(secondaryRecorder).toContainText('Q');

    // Take screenshot showing customized state
    await page.screenshot({
      path: 'e2e-playwright/screenshots/shortcuts-customized-state.png',
      fullPage: false,
    });
  });

  test.describe('Accessibility - live region announcements', () => {
    // Ported from the retired WDIO `shortcuts-accessibility` spec (#120): the
    // ShortcutRecorder's aria-live region must announce recording state changes
    // for screen-reader users. No other Playwright spec asserts these.
    test('announces recording start and cancellation', async ({ page }) => {
      await openSettingsDialog(page);
      await navigateToShortcutsCategory(page);

      const recorder = page.locator('[data-testid="shortcut-primary-select"]');
      const liveRegion = recorder.locator('[role="status"][aria-live="polite"]');

      await recorder.click();
      await waitForRecording(page, 'shortcut-primary-select', true);
      await expect(liveRegion).toContainText('Recording shortcut', { timeout: 2000 });

      await page.keyboard.press('Escape');
      await waitForRecording(page, 'shortcut-primary-select', false);
      await expect(liveRegion).toContainText('Recording cancelled', { timeout: 2000 });
    });

    test('announces the captured shortcut on completion', async ({ page }) => {
      await openSettingsDialog(page);
      await navigateToShortcutsCategory(page);

      const recorder = page.locator('[data-testid="shortcut-secondary-select"]');
      const liveRegion = recorder.locator('[role="status"][aria-live="polite"]');

      await recorder.click();
      await waitForRecording(page, 'shortcut-secondary-select', true);
      await page.keyboard.press('Control+Shift+W');
      await waitForRecording(page, 'shortcut-secondary-select', false);
      await expect(liveRegion).toContainText('Shortcut set to', { timeout: 2000 });
    });
  });
});
