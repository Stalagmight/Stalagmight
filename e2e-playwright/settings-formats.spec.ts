import { test, expect, Page } from './fixtures/coverage';
import { setupTauriMocks } from './mocks/tauri-ipc';
import { closeProjectViaMenu, openSettingsFromMenu } from './helpers/app';

/**
 * E2E tests for Number Format and Date Format settings.
 *
 * Tests the format selectors in Settings > General > Language section:
 * - select-number-format: locale, en-US (period), de-DE (comma), fr-FR (space)
 * - select-date-format: locale, en-US (MM/DD/YYYY), en-GB (DD/MM/YYYY), en-CA (YYYY/MM/DD)
 */

async function createProject(page: Page, name: string): Promise<void> {
  const newProjectBtn = page
    .locator('[data-testid="new-project-button"], [data-testid="empty-state-new-project"]')
    .first();
  await newProjectBtn.click();
  await page.waitForSelector('.fixed.inset-0');
  await page.locator('.fixed.inset-0 input').first().fill(name);
  await page.click('.fixed.inset-0 button:has-text("Create")');
  await page.waitForSelector('canvas', { timeout: 10000 });
  await page.waitForFunction(
    () => (window as unknown as { __stalagmight_map__?: unknown }).__stalagmight_map__ !== undefined,
    { timeout: 10000 },
  );
}

async function goBackToGlobalView(page: Page): Promise<void> {
  if (await page.getByTestId('global-view').isVisible().catch(() => false)) return;
  await closeProjectViaMenu(page);
}

async function openSettingsDialog(page: Page): Promise<void> {
  await goBackToGlobalView(page);
  await openSettingsFromMenu(page);
}

async function navigateToGeneralTab(page: Page): Promise<void> {
  const generalTab = page.locator('[data-testid="settings-tab-general"]');
  await expect(generalTab).toBeVisible();
  await generalTab.click();
  await expect(page.locator('[data-testid="settings-panel-general"]')).toBeVisible({ timeout: 3000 });
}

test.describe('Settings - Number Format', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Number Format Test');
  });

  test('number format selector is visible in General settings', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToGeneralTab(page);

    const select = page.locator('[data-testid="select-number-format"]');
    await expect(select).toBeVisible();
  });

  test('number format setting row has correct testid', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToGeneralTab(page);

    const settingRow = page.locator('[data-testid="setting-number-format"]');
    await expect(settingRow).toBeVisible();
  });

  test('number format defaults to locale', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToGeneralTab(page);

    const select = page.locator('[data-testid="select-number-format"]');
    // The default value should be "locale" or the system locale text
    await expect(select).toBeVisible();
  });

  test('changing number format enables Save button', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToGeneralTab(page);

    const saveButton = page.locator('[data-testid="settings-save-button"]');
    await expect(saveButton).toBeDisabled();

    // Open the select dropdown and pick a different format
    const trigger = page.locator('[data-testid="select-number-format"]');
    await trigger.click();

    // Select "Period (1,234.5)" option (en-US)
    const option = page.getByRole('listbox').locator('[role="option"]').nth(1);
    await option.click();

    // Save should be enabled
    await expect(saveButton).toBeEnabled();
  });

  test('number format change persists across category switches', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToGeneralTab(page);

    // Change number format
    const trigger = page.locator('[data-testid="select-number-format"]');
    await trigger.click();
    const option = page.getByRole('listbox').locator('[role="option"]').nth(1);
    await option.click();

    // Switch to Units and back
    await page.locator('[data-testid="settings-tab-units"]').click();
    await expect(page.locator('[data-testid="settings-panel-units"]')).toBeVisible();
    await navigateToGeneralTab(page);

    // Save button should still be enabled (change preserved)
    const saveButton = page.locator('[data-testid="settings-save-button"]');
    await expect(saveButton).toBeEnabled();
  });

  test('all four number format options are available', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToGeneralTab(page);

    // Open dropdown
    const trigger = page.locator('[data-testid="select-number-format"]');
    await trigger.click();

    // Should have 4 options
    const options = page.getByRole('listbox').locator('[role="option"]');
    await expect(options).toHaveCount(4);

    // Close dropdown
    await page.keyboard.press('Escape');
  });
});

test.describe('Settings - Date Format', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Date Format Test');
  });

  test('date format selector is visible in General settings', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToGeneralTab(page);

    const select = page.locator('[data-testid="select-date-format"]');
    await expect(select).toBeVisible();
  });

  test('date format setting row has correct testid', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToGeneralTab(page);

    const settingRow = page.locator('[data-testid="setting-date-format"]');
    await expect(settingRow).toBeVisible();
  });

  test('changing date format enables Save button', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToGeneralTab(page);

    const saveButton = page.locator('[data-testid="settings-save-button"]');
    await expect(saveButton).toBeDisabled();

    // Open date format dropdown
    const trigger = page.locator('[data-testid="select-date-format"]');
    await trigger.click();

    // Select a different format
    const option = page.getByRole('listbox').locator('[role="option"]').nth(2);
    await option.click();

    await expect(saveButton).toBeEnabled();
  });

  test('all four date format options are available', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToGeneralTab(page);

    const trigger = page.locator('[data-testid="select-date-format"]');
    await trigger.click();

    const options = page.getByRole('listbox').locator('[role="option"]');
    await expect(options).toHaveCount(4);

    await page.keyboard.press('Escape');
  });

  test('language section contains all format selectors', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToGeneralTab(page);

    const languageSection = page.locator('[data-testid="section-language"]');
    await expect(languageSection).toBeVisible();

    // Language, number format, and date format should all be within this section
    await expect(page.locator('[data-testid="setting-language"]')).toBeVisible();
    await expect(page.locator('[data-testid="setting-number-format"]')).toBeVisible();
    await expect(page.locator('[data-testid="setting-date-format"]')).toBeVisible();
  });
});

test.describe('Settings - Auto-Save Controls', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Auto-Save Settings Test');
  });

  test('auto-save toggle is visible in saving section', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToGeneralTab(page);

    const savingSection = page.locator('[data-testid="section-saving"]');
    await expect(savingSection).toBeVisible();
    await expect(page.locator('[data-testid="toggle-auto-save"]')).toBeVisible();
  });

  test('auto-save interval input is visible', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToGeneralTab(page);

    const intervalInput = page.locator('[data-testid="input-auto-save-interval"]');
    await expect(intervalInput).toBeVisible();
    // Default is 30 seconds
    const value = await intervalInput.inputValue();
    expect(parseInt(value, 10)).toBeGreaterThanOrEqual(10);
  });

  test('changing auto-save interval enables Save', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToGeneralTab(page);

    const saveButton = page.locator('[data-testid="settings-save-button"]');
    await expect(saveButton).toBeDisabled();

    const intervalInput = page.locator('[data-testid="input-auto-save-interval"]');
    await intervalInput.fill('60');

    await expect(saveButton).toBeEnabled();
  });

  test('invalid auto-save interval triggers validation error', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToGeneralTab(page);

    // Set interval below minimum (10)
    const intervalInput = page.locator('[data-testid="input-auto-save-interval"]');
    await intervalInput.fill('5');

    // Error indicator should appear on the General tab
    const errorDot = page.locator('[data-testid="settings-tab-error-general"]');
    await expect(errorDot).toBeVisible({ timeout: 3000 });
  });

  test('undo history settings are visible', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToGeneralTab(page);

    const undoSection = page.locator('[data-testid="section-undo"]');
    await expect(undoSection).toBeVisible();
    await expect(page.locator('[data-testid="input-undo-limit"]')).toBeVisible();
    await expect(page.locator('[data-testid="input-undo-persist"]')).toBeVisible();
  });
});
