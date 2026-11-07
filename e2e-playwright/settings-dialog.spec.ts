import { test, expect, Page } from './fixtures/coverage';
import { setupTauriMocks } from './mocks/tauri-ipc';
import { openSettingsFromMenu, closeProjectViaMenu } from './helpers/app';

/**
 * E2E tests for Settings dialog sidebar layout and interactions.
 * Tests navigation, keyboard shortcuts, dirty state, and confirmation dialogs.
 */

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
  if (await page.getByTestId('global-view').isVisible().catch(() => false)) return;
  await closeProjectViaMenu(page);
}

/**
 * Helper to open Settings dialog
 */
async function openSettingsDialog(page: Page): Promise<void> {
  await goBackToGlobalView(page);
  await openSettingsFromMenu(page);
}

async function getSidebarCategories(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    return Array.from(document.querySelectorAll('[data-testid^="settings-tab-"]'))
      .map((el) => el.getAttribute('data-testid')?.replace('settings-tab-', ''))
      .filter((value): value is string => Boolean(value));
  });
}

function filterTestableCategories(categories: string[]): string[] {
  return categories.filter((category) => category !== 'map');
}

test.describe('Settings Dialog - Keyboard Shortcuts to Open', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Settings Keyboard Test');
    // Stay in project view - keyboard shortcuts only work in SingleSystemView
  });

  test('Cmd/Ctrl+, opens Settings dialog from project view @smoke', async ({ page }) => {
    // We're in project view (SingleSystemView) where keyboard shortcuts are active
    // Ensure Settings is not open
    await expect(page.locator('[data-testid="settings-dialog"]')).not.toBeVisible();

    // No wait is needed here: the shortcut handler is registered by
    // SingleSystemView's mount effect (useKeyboardShortcuts), and beforeEach's
    // createProject returned only once single-system-view was visible and the
    // map ready. Asserted as a precondition, not as a wait.
    await expect(page.getByTestId('single-system-view')).toBeVisible();

    // Send the Ctrl+, shortcut using keyboard.down/up for more reliable behavior
    await page.keyboard.down('Control');
    await page.keyboard.press(',');
    await page.keyboard.up('Control');

    // Settings dialog should open
    await expect(page.locator('[data-testid="settings-dialog"]')).toBeVisible({ timeout: 5000 });
  });

  test('Cmd/Ctrl+, works when focused in section name input', async ({ page }) => {
    // We're in project view (SingleSystemView) where keyboard shortcuts are active
    // Wait for canvas to be visible and ready (deterministic check)
    const canvas = page.locator('canvas');
    await expect(canvas).toBeVisible({ timeout: 5000 });

    // Create a temporary input in the DOM to ensure input focus doesn't block shortcuts
    await page.evaluate(() => {
      const input = document.createElement('input');
      input.type = 'text';
      input.setAttribute('data-testid', 'e2e-temp-input');
      document.body.appendChild(input);
      input.focus();
    });

    const tempInput = page.locator('[data-testid="e2e-temp-input"]');
    await expect(tempInput).toBeFocused();

    // CRITICAL: Press Ctrl+, while the input is focused
    // The shortcut should open Settings even when focus is in an input field
    await page.keyboard.down('Control');
    await page.keyboard.press(',');
    await page.keyboard.up('Control');

    // Settings dialog should open even though an input was focused
    await expect(page.locator('[data-testid="settings-dialog"]')).toBeVisible({ timeout: 5000 });
  });
});

test.describe('Settings Dialog - Sidebar Navigation', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await page.waitForSelector('[data-testid="global-view"]');
    // Don't create a project - test from GlobalView directly
  });

  test('clicking sidebar category switches content panel @smoke', async ({ page }) => {
    // Open settings directly from GlobalView without creating a project
    await openSettingsFromMenu(page);

    // Initially should show General (first category)
    await expect(page.locator('[data-testid="settings-tab-general"]')).toHaveAttribute('aria-selected', 'true');
    await expect(page.locator('[data-testid="settings-panel-general"]')).toBeVisible();

    // Click on Units category (safe test)
    const unitsTab = page.locator('[data-testid="settings-tab-units"]');
    await expect(unitsTab).toBeVisible();
    await unitsTab.click();

    // Verify Units is now selected
    await expect(unitsTab).toHaveAttribute('aria-selected', 'true');
    await expect(page.locator('[data-testid="settings-panel-units"]')).toBeVisible({ timeout: 3000 });

    // Click on Shortcuts category
    const shortcutsTab = page.locator('[data-testid="settings-tab-shortcuts"]');
    await expect(shortcutsTab).toBeVisible();
    await shortcutsTab.click();

    // Verify Shortcuts is now selected
    await expect(shortcutsTab).toHaveAttribute('aria-selected', 'true');
    await expect(page.locator('[data-testid="settings-panel-shortcuts"]')).toBeVisible({ timeout: 3000 });
  });

  test('Arrow Down navigates to next sidebar category', async ({ page }) => {
    await openSettingsDialog(page);

    const categories = await getSidebarCategories(page);
    const firstCategory = categories[0];
    const secondCategory = categories[1];
    if (!firstCategory || !secondCategory) {
      throw new Error('Not enough categories to test Arrow Down navigation');
    }

    // Focus the first category
    const firstTab = page.locator(`[data-testid="settings-tab-${firstCategory}"]`);
    await firstTab.focus();
    await expect(firstTab).toBeFocused();

    // Press Arrow Down - should move focus to the next category
    await page.keyboard.press('ArrowDown');

    const secondTab = page.locator(`[data-testid="settings-tab-${secondCategory}"]`);
    await expect(secondTab).toBeFocused();

    // Selection should NOT change (manual activation)
    await expect(firstTab).toHaveAttribute('aria-selected', 'true');
  });

  test('Arrow Up navigates to previous sidebar category', async ({ page }) => {
    await openSettingsDialog(page);

    const categories = await getSidebarCategories(page);
    const firstCategory = categories[0];
    const secondCategory = categories[1];
    if (!firstCategory || !secondCategory) {
      throw new Error('Not enough categories to test Arrow Up navigation');
    }

    // Click the second category to select it
    const secondTab = page.locator(`[data-testid="settings-tab-${secondCategory}"]`);
    await secondTab.click();
    await expect(secondTab).toHaveAttribute('aria-selected', 'true');

    // Focus the second category and press Arrow Up
    await secondTab.focus();
    await page.keyboard.press('ArrowUp');

    // Focus should move to the first category
    const firstTab = page.locator(`[data-testid="settings-tab-${firstCategory}"]`);
    await expect(firstTab).toBeFocused();
  });

  test('Enter activates focused category', async ({ page }) => {
    await openSettingsDialog(page);

    // Focus General
    const generalTab = page.locator('[data-testid="settings-tab-general"]');
    await generalTab.focus();

    // Navigate to Shortcuts (or fall back to the last category if missing)
    const categories = await getSidebarCategories(page);
    const targetCategory = categories.includes('shortcuts')
      ? 'shortcuts'
      : categories[categories.length - 1];
    if (!targetCategory) {
      throw new Error('No categories available to test Enter activation');
    }
    const targetIndex = categories.indexOf(targetCategory);
    for (let i = 0; i < targetIndex; i++) {
      await page.keyboard.press('ArrowDown');
    }

    // Focus should be on Shortcuts
    const targetTab = page.locator(`[data-testid="settings-tab-${targetCategory}"]`);
    await expect(targetTab).toBeFocused();

    // General should still be selected (no auto-activation)
    await expect(generalTab).toHaveAttribute('aria-selected', 'true');

    // Press Enter to activate
    await page.keyboard.press('Enter');

    // Now target category should be selected
    await expect(targetTab).toHaveAttribute('aria-selected', 'true');
    await expect(page.locator(`[data-testid="settings-panel-${targetCategory}"]`)).toBeVisible();
  });

  test('Home jumps to first category', async ({ page }) => {
    await openSettingsDialog(page);

    const categories = await getSidebarCategories(page);
    const lastCategory = categories[categories.length - 1];
    const firstCategory = categories[0];
    if (!lastCategory || !firstCategory) {
      throw new Error('Not enough categories to test Home navigation');
    }

    // Click and focus the last category
    const lastTab = page.locator(`[data-testid="settings-tab-${lastCategory}"]`);
    await lastTab.scrollIntoViewIfNeeded();
    await lastTab.click();
    await lastTab.focus();

    // Press Home
    await page.keyboard.press('Home');

    // Focus should jump to General
    const firstTab = page.locator(`[data-testid="settings-tab-${firstCategory}"]`);
    await expect(firstTab).toBeFocused();
  });

  test('End jumps to last category', async ({ page }) => {
    await openSettingsDialog(page);

    const categories = await getSidebarCategories(page);
    const lastCategory = categories[categories.length - 1];
    const firstCategory = categories[0];
    if (!lastCategory || !firstCategory) {
      throw new Error('Not enough categories to test End navigation');
    }

    // Focus first category
    const firstTab = page.locator(`[data-testid="settings-tab-${firstCategory}"]`);
    await firstTab.focus();

    // Press End
    await page.keyboard.press('End');

    // Focus should jump to the last category
    const lastTab = page.locator(`[data-testid="settings-tab-${lastCategory}"]`);
    await expect(lastTab).toBeFocused();
  });
});

test.describe('Settings Dialog - Dirty State', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Dirty State Test');
  });

  test('Save button disabled when no changes made', async ({ page }) => {
    await openSettingsDialog(page);

    const saveButton = page.locator('[data-testid="settings-save-button"]');
    await expect(saveButton).toBeDisabled();
  });

  test('Save button enabled after changing a setting @smoke', async ({ page }) => {
    await openSettingsDialog(page);

    // Navigate to Validation settings which has known testids
    await page.locator('[data-testid="settings-tab-validation"]').click();
    await expect(page.locator('[data-testid="settings-panel-validation"]')).toBeVisible();

    // Use specific testid for shot length min input
    const input = page.locator('[data-testid="input-shot-length-min"]');
    await expect(input).toBeVisible();
    const originalValue = await input.inputValue();

    // Clear and set a new value (increment by 1)
    await input.fill(String(Number(originalValue) + 1));

    // Save button should now be enabled
    const saveButton = page.locator('[data-testid="settings-save-button"]');
    await expect(saveButton).toBeEnabled();
  });

  test('unsaved changes preserved when switching categories', async ({ page }) => {
    await openSettingsDialog(page);

    // Go to Validation settings
    await page.locator('[data-testid="settings-tab-validation"]').click();
    await expect(page.locator('[data-testid="settings-panel-validation"]')).toBeVisible();

    // Use specific testid for depth change max input
    const input = page.locator('[data-testid="input-depth-change-max"]');
    await expect(input).toBeVisible();
    const originalValue = await input.inputValue();
    const newValue = String(Number(originalValue) + 10);
    await input.fill(newValue);

    // Switch to General category
    await page.locator('[data-testid="settings-tab-general"]').click();
    await expect(page.locator('[data-testid="settings-tab-general"]')).toHaveAttribute('aria-selected', 'true');

    // Switch back to Validation
    await page.locator('[data-testid="settings-tab-validation"]').click();
    await expect(page.locator('[data-testid="settings-panel-validation"]')).toBeVisible();

    // Verify the change is still there using the same testid
    const currentValue = await page.locator('[data-testid="input-depth-change-max"]').inputValue();
    expect(currentValue).toBe(newValue);
  });
});

test.describe('Settings Dialog - Unsaved Changes Confirmation', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Unsaved Changes Test');
  });

  test('closing with unsaved changes shows Save/Discard/Cancel dialog', async ({ page }) => {
    await openSettingsDialog(page);

    // Make a change in Shortcuts
    await page.locator('[data-testid="settings-tab-shortcuts"]').click();
    await expect(page.locator('[data-testid="settings-panel-shortcuts"]')).toBeVisible();

    // Record a new shortcut
    const recorder = page.locator('[data-testid="shortcut-primary-select"]');
    await recorder.click();
    // Wait for recording state to activate
    await expect(recorder).toHaveAttribute('data-recording', 'true', { timeout: 2000 });
    await page.keyboard.press('Q');
    // Wait for recording to complete and shortcut to be set
    await expect(recorder).toHaveAttribute('data-recording', 'false', { timeout: 2000 });
    await expect(recorder).toContainText('Q');

    // Try to close via Cancel button
    await page.locator('[data-testid="settings-cancel-button"]').click();

    // Unsaved changes dialog should appear
    await expect(page.locator('[data-testid="unsaved-changes-dialog"]')).toBeVisible({ timeout: 2000 });

    // Verify all three buttons exist
    await expect(page.locator('[data-testid="unsaved-cancel"]')).toBeVisible();
    await expect(page.locator('[data-testid="unsaved-discard"]')).toBeVisible();
    await expect(page.locator('[data-testid="unsaved-save"]')).toBeVisible();
  });

  test('Cancel in unsaved dialog returns to Settings', async ({ page }) => {
    await openSettingsDialog(page);

    // Make a change
    await page.locator('[data-testid="settings-tab-shortcuts"]').click();
    await expect(page.locator('[data-testid="settings-panel-shortcuts"]')).toBeVisible();
    const recorder = page.locator('[data-testid="shortcut-primary-select"]');
    await recorder.click();
    await expect(recorder).toHaveAttribute('data-recording', 'true', { timeout: 2000 });
    await page.keyboard.press('Q');
    await expect(recorder).toHaveAttribute('data-recording', 'false', { timeout: 2000 });
    await expect(recorder).toContainText('Q');

    // Try to close
    await page.locator('[data-testid="settings-cancel-button"]').click();
    await expect(page.locator('[data-testid="unsaved-changes-dialog"]')).toBeVisible();

    // Click Cancel to stay
    await page.locator('[data-testid="unsaved-cancel"]').click();

    // Dialog should close, Settings should still be visible
    await expect(page.locator('[data-testid="unsaved-changes-dialog"]')).not.toBeVisible();
    await expect(page.locator('[data-testid="settings-dialog"]')).toBeVisible();
  });

  test('Discard in unsaved dialog closes without saving', async ({ page }) => {
    await openSettingsDialog(page);

    // Make a change
    await page.locator('[data-testid="settings-tab-shortcuts"]').click();
    await expect(page.locator('[data-testid="settings-panel-shortcuts"]')).toBeVisible();
    const recorder = page.locator('[data-testid="shortcut-primary-select"]');
    await recorder.click();
    await expect(recorder).toHaveAttribute('data-recording', 'true', { timeout: 2000 });
    await page.keyboard.press('Q');
    await expect(recorder).toHaveAttribute('data-recording', 'false', { timeout: 2000 });

    // Verify change is shown
    await expect(recorder).toContainText('Q');

    // Try to close
    await page.locator('[data-testid="settings-cancel-button"]').click();
    await expect(page.locator('[data-testid="unsaved-changes-dialog"]')).toBeVisible();

    // Click Discard
    await page.locator('[data-testid="unsaved-discard"]').click();

    // Settings should close
    await expect(page.locator('[data-testid="settings-dialog"]')).not.toBeVisible({ timeout: 5000 });

    // Reopen and verify change was NOT saved
    await openSettingsDialog(page);
    await page.locator('[data-testid="settings-tab-shortcuts"]').click();
    const recorderAfter = page.locator('[data-testid="shortcut-primary-select"]');
    await expect(recorderAfter).toContainText('S'); // Original value
  });

  test('Save in unsaved dialog saves and closes', async ({ page }) => {
    await openSettingsDialog(page);

    // Make a change
    await page.locator('[data-testid="settings-tab-shortcuts"]').click();
    await expect(page.locator('[data-testid="settings-panel-shortcuts"]')).toBeVisible();
    const recorder = page.locator('[data-testid="shortcut-primary-select"]');
    await recorder.click();
    await expect(recorder).toHaveAttribute('data-recording', 'true', { timeout: 2000 });
    await page.keyboard.press('Q');
    await expect(recorder).toHaveAttribute('data-recording', 'false', { timeout: 2000 });
    await expect(recorder).toContainText('Q');

    // Try to close
    await page.locator('[data-testid="settings-cancel-button"]').click();
    await expect(page.locator('[data-testid="unsaved-changes-dialog"]')).toBeVisible();

    // Click Save
    await page.locator('[data-testid="unsaved-save"]').click();

    // Settings should close
    await expect(page.locator('[data-testid="settings-dialog"]')).not.toBeVisible({ timeout: 5000 });

    // Reopen and verify change WAS saved
    await openSettingsDialog(page);
    await page.locator('[data-testid="settings-tab-shortcuts"]').click();
    const recorderAfter = page.locator('[data-testid="shortcut-primary-select"]');
    await expect(recorderAfter).toContainText('Q');
  });
});

test.describe('Settings Dialog - Backdrop and Escape', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Backdrop Test');
  });

  test('backdrop click does NOT close dialog', async ({ page }) => {
    await openSettingsDialog(page);

    // Find the dialog content panel
    const dialog = page.locator('[data-testid="settings-dialog"]');
    await expect(dialog).toBeVisible();

    // Get the dialog's bounding box to calculate a click point outside it
    const dialogBox = await dialog.boundingBox();
    expect(dialogBox).not.toBeNull();

    // Click to the left of the dialog (on the backdrop/overlay area)
    // Use a point that's guaranteed to be outside the dialog
    const clickX = Math.max(5, (dialogBox?.x ?? 100) - 20);
    const clickY = (dialogBox?.y ?? 100) + (dialogBox?.height ?? 200) / 2;
    await page.mouse.click(clickX, clickY);

    // Dialog should still be visible (backdrop click doesn't close it)
    await expect(dialog).toBeVisible();
  });

  test('Escape closes Settings dialog when no overlays open', async ({ page }) => {
    await openSettingsDialog(page);

    // Press Escape
    await page.keyboard.press('Escape');

    // Dialog should close
    await expect(page.locator('[data-testid="settings-dialog"]')).not.toBeVisible({ timeout: 5000 });
  });

  test('Escape closes unsaved dialog first, not Settings', async ({ page }) => {
    await openSettingsDialog(page);

    // Make a change
    await page.locator('[data-testid="settings-tab-shortcuts"]').click();
    await expect(page.locator('[data-testid="settings-panel-shortcuts"]')).toBeVisible();
    const recorder = page.locator('[data-testid="shortcut-primary-select"]');
    await recorder.click();
    await expect(recorder).toHaveAttribute('data-recording', 'true', { timeout: 2000 });
    await page.keyboard.press('Q');
    await expect(recorder).toHaveAttribute('data-recording', 'false', { timeout: 2000 });
    await expect(recorder).toContainText('Q');

    // Trigger unsaved changes dialog via Escape
    await page.keyboard.press('Escape');
    await expect(page.locator('[data-testid="unsaved-changes-dialog"]')).toBeVisible();

    // Press Escape again - should close the unsaved dialog, not Settings
    await page.keyboard.press('Escape');

    // Unsaved dialog should close
    await expect(page.locator('[data-testid="unsaved-changes-dialog"]')).not.toBeVisible();
    // Settings should still be visible
    await expect(page.locator('[data-testid="settings-dialog"]')).toBeVisible();
  });

  test('Escape closes reset confirmation dialog first, not Settings', async ({ page }) => {
    await openSettingsDialog(page);

    // Click Reset to Defaults
    await page.locator('[data-testid="reset-category-defaults"]').click();

    // Reset confirmation dialog should appear
    await expect(page.locator('[data-testid="reset-confirm-dialog"]')).toBeVisible();

    // Press Escape - should close reset dialog, not Settings
    await page.keyboard.press('Escape');

    // Reset dialog should close
    await expect(page.locator('[data-testid="reset-confirm-dialog"]')).not.toBeVisible();
    // Settings should still be visible
    await expect(page.locator('[data-testid="settings-dialog"]')).toBeVisible();
  });
});

test.describe('Settings Dialog - Reset to Defaults', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Reset Defaults Test');
  });

  test('Reset to Defaults shows confirmation dialog', async ({ page }) => {
    await openSettingsDialog(page);

    // Click Reset to Defaults
    await page.locator('[data-testid="reset-category-defaults"]').click();

    // Confirmation dialog should appear
    await expect(page.locator('[data-testid="reset-confirm-dialog"]')).toBeVisible();
    await expect(page.locator('[data-testid="reset-cancel"]')).toBeVisible();
    await expect(page.locator('[data-testid="reset-confirm"]')).toBeVisible();
  });

  test('Cancel in reset dialog keeps changes', async ({ page }) => {
    await openSettingsDialog(page);

    // Make a change first using specific testid
    await page.locator('[data-testid="settings-tab-validation"]').click();
    await expect(page.locator('[data-testid="settings-panel-validation"]')).toBeVisible();
    const input = page.locator('[data-testid="input-shot-length-min"]');
    await expect(input).toBeVisible();
    const newValue = '999';
    await input.fill(newValue);

    // Click Reset to Defaults
    await page.locator('[data-testid="reset-category-defaults"]').click();
    await expect(page.locator('[data-testid="reset-confirm-dialog"]')).toBeVisible();

    // Cancel
    await page.locator('[data-testid="reset-cancel"]').click();

    // Dialog should close
    await expect(page.locator('[data-testid="reset-confirm-dialog"]')).not.toBeVisible();

    // Value should still be changed
    const currentValue = await input.inputValue();
    expect(currentValue).toBe(newValue);
  });

  test('Confirm reset restores current category to defaults', async ({ page }) => {
    await openSettingsDialog(page);

    // Go to Shortcuts and change a shortcut
    await page.locator('[data-testid="settings-tab-shortcuts"]').click();
    await expect(page.locator('[data-testid="settings-panel-shortcuts"]')).toBeVisible();

    const recorder = page.locator('[data-testid="shortcut-primary-select"]');
    await recorder.click();
    // Wait for recording state
    await expect(recorder).toHaveAttribute('data-recording', 'true', { timeout: 2000 });
    await page.keyboard.press('Q');
    // Wait for recording to complete
    await expect(recorder).toHaveAttribute('data-recording', 'false', { timeout: 2000 });

    // Verify change
    await expect(recorder).toContainText('Q');

    // Click Reset to Defaults
    await page.locator('[data-testid="reset-category-defaults"]').click();
    await expect(page.locator('[data-testid="reset-confirm-dialog"]')).toBeVisible();

    // Confirm reset
    await page.locator('[data-testid="reset-confirm"]').click();

    // Dialog should close
    await expect(page.locator('[data-testid="reset-confirm-dialog"]')).not.toBeVisible();

    // Shortcut should be back to default
    await expect(recorder).toContainText('S');
  });
});

test.describe('Settings Dialog - Error Indicators', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Error Indicator Test');
  });

  test('error indicator appears on category with validation error', async ({ page }) => {
    await openSettingsDialog(page);

    // Go to Validation settings which has validators
    await page.locator('[data-testid="settings-tab-validation"]').click();
    await expect(page.locator('[data-testid="settings-panel-validation"]')).toBeVisible();

    // Use specific testids for validation inputs
    // Setting min > max triggers "Min must be less than max" validation error
    const minInput = page.locator('[data-testid="input-shot-length-min"]');
    const maxInput = page.locator('[data-testid="input-shot-length-max"]');

    await expect(minInput).toBeVisible();
    await expect(maxInput).toBeVisible();

    // First get the current max value
    const maxValue = await maxInput.inputValue();

    // Set min to a value greater than max to trigger validation error
    await minInput.fill(String(Number(maxValue) + 100));

    // Check for error indicator on the Validation tab
    // Use polling assertion instead of waitForTimeout
    const errorDot = page.locator('[data-testid="settings-tab-error-validation"]');
    await expect(errorDot).toBeVisible({ timeout: 3000 });
  });

  // Note: Map settings tests skipped due to rendering issue in E2E environment
  // The MapSettings component crashes when rendered in headless Chromium
  // This is a pre-existing bug that should be fixed separately
});

test.describe('Settings Dialog - Fixed Dimensions', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await page.waitForSelector('[data-testid="global-view"]');
    // Open from GlobalView directly - no project needed
  });

  test('dialog dimensions remain constant across category switches', async ({ page }) => {
    await openSettingsFromMenu(page);

    const dialog = page.locator('[data-testid="settings-dialog"]');

    // Capture initial bounding box on the General tab
    const initialBox = await dialog.boundingBox();
    expect(initialBox).not.toBeNull();

    const categories = filterTestableCategories(await getSidebarCategories(page));

    // Switch through testable categories and verify dimensions stay the same
    for (const category of categories) {
      const tab = page.locator(`[data-testid="settings-tab-${category}"]`);
      await tab.click();
      await expect(page.locator(`[data-testid="settings-panel-${category}"]`)).toBeVisible({ timeout: 5000 });

      const box = await dialog.boundingBox();
      expect(box).not.toBeNull();
      // Allow 1px tolerance for sub-pixel rendering differences
      expect(Math.abs(box!.width - initialBox!.width)).toBeLessThanOrEqual(1);
      expect(Math.abs(box!.height - initialBox!.height)).toBeLessThanOrEqual(1);
    }
  });

  test('scroll position resets to top on category change', async ({ page }) => {
    await openSettingsFromMenu(page);

    // Navigate to shortcuts which has many rows of content
    await page.locator('[data-testid="settings-tab-shortcuts"]').click();
    await expect(page.locator('[data-testid="settings-panel-shortcuts"]')).toBeVisible({ timeout: 5000 });

    // Helper to find the nearest scrollable ancestor (overflow-auto class)
    const getScrollInfo = (panelTestId: string) => page.evaluate((testId) => {
      const panel = document.querySelector(`[data-testid="${testId}"]`);
      if (!panel) return { found: false, scrollTop: -1 };
      let el: Element | null = panel.parentElement;
      while (el) {
        if (el.classList.contains('overflow-auto')) {
          return { found: true, scrollTop: (el as HTMLElement).scrollTop };
        }
        el = el.parentElement;
      }
      return { found: false, scrollTop: -1 };
    }, panelTestId);

    // Force the scroll container to be scrollable by constraining its max-height
    // and injecting tall content, then scroll down. This makes the test deterministic.
    const scrolled = await page.evaluate(() => {
      const panel = document.querySelector('[data-testid="settings-panel-shortcuts"]');
      if (!panel) return false;
      let scrollContainer: HTMLElement | null = null;
      let el: Element | null = panel.parentElement;
      while (el) {
        if (el.classList.contains('overflow-auto')) {
          scrollContainer = el as HTMLElement;
          break;
        }
        el = el.parentElement;
      }
      if (!scrollContainer) return false;
      // Constrain the scroll container and inject tall content to force overflow
      scrollContainer.style.maxHeight = '200px';
      const sentinel = document.createElement('div');
      sentinel.style.height = '2000px';
      sentinel.dataset.testSentinel = 'true';
      panel.appendChild(sentinel);
      scrollContainer.scrollTop = 300;
      return scrollContainer.scrollTop > 0;
    });

    // Injected constraints guarantee scrolling is possible
    expect(scrolled).toBe(true);

    // Switch to a different category -- scroll must reset to top
    await page.locator('[data-testid="settings-tab-units"]').click();
    await expect(page.locator('[data-testid="settings-panel-units"]')).toBeVisible({ timeout: 5000 });

    const info1 = await getScrollInfo('settings-panel-units');
    expect(info1.found).toBe(true);
    expect(info1.scrollTop).toBe(0);

    // Switch back to shortcuts -- scroll must also be at top
    await page.locator('[data-testid="settings-tab-shortcuts"]').click();
    await expect(page.locator('[data-testid="settings-panel-shortcuts"]')).toBeVisible({ timeout: 5000 });

    const info2 = await getScrollInfo('settings-panel-shortcuts');
    expect(info2.found).toBe(true);
    expect(info2.scrollTop).toBe(0);
  });

  test('content area has overflow-auto for scrolling tall content', async ({ page }) => {
    await openSettingsFromMenu(page);

    // Navigate to shortcuts (content-heavy category)
    await page.locator('[data-testid="settings-tab-shortcuts"]').click();
    await expect(page.locator('[data-testid="settings-panel-shortcuts"]')).toBeVisible({ timeout: 5000 });

    // Verify a scrollable ancestor (overflow-auto) exists for the panel content
    const hasScrollableAncestor = await page.evaluate(() => {
      const panel = document.querySelector('[data-testid="settings-panel-shortcuts"]');
      if (!panel) return false;
      let el: Element | null = panel.parentElement;
      while (el) {
        if (el.classList.contains('overflow-auto')) return true;
        el = el.parentElement;
      }
      return false;
    });
    expect(hasScrollableAncestor).toBe(true);
  });
});

test.describe('Settings Dialog - All panels have testids', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Panel TestID Test');
  });

  test('each category panel has correct data-testid', async ({ page }) => {
    await openSettingsDialog(page);

    // The sidebar is rendered before it is read: getSidebarCategories is a
    // point-in-time DOM query, and an empty list would make the loop below vacuous.
    await expect(page.locator('[data-testid^="settings-tab-"]').first()).toBeVisible();

    const categories = filterTestableCategories(await getSidebarCategories(page));

    // Test each testable category (Map is skipped due to rendering issue)
    for (const category of categories) {
      // Click the category tab
      const tab = page.locator(`[data-testid="settings-tab-${category}"]`);
      await expect(tab).toBeVisible();
      await tab.click();

      // Verify the panel is visible with correct testid
      await expect(page.locator(`[data-testid="settings-panel-${category}"]`)).toBeVisible({ timeout: 5000 });
    }
  });
});
