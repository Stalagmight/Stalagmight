import { test, expect, Page } from './fixtures/coverage';
import { setupTauriMocks } from './mocks/tauri-ipc';
import { openSettingsFromMenu, closeProjectViaMenu } from './helpers/app';

/**
 * E2E tests for Decimal Precision settings (per-category).
 *
 * Tests the precision input fields in Settings > Units > Precision section.
 * Each category (length, coordinates, depth, azimuth, inclination, gps) has
 * an input-precision-{key} field with range 0-6.
 */

const PRECISION_KEYS = ['length', 'coordinates', 'depth', 'azimuth', 'inclination', 'gps'] as const;

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

async function navigateToUnitsTab(page: Page): Promise<void> {
  const unitsTab = page.locator('[data-testid="settings-tab-units"]');
  await expect(unitsTab).toBeVisible();
  await unitsTab.click();
  await expect(page.locator('[data-testid="settings-panel-units"]')).toBeVisible({ timeout: 3000 });
}

test.describe('Settings - Decimal Precision', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Precision Settings Test');
  });

  test('precision section is visible in Units settings', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToUnitsTab(page);

    // The precision section should be visible
    const precisionSection = page.locator('[data-testid="section-precision"]');
    await expect(precisionSection).toBeVisible();
  });

  test('all six precision input fields are present', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToUnitsTab(page);

    for (const key of PRECISION_KEYS) {
      const input = page.locator(`[data-testid="input-precision-${key}"]`);
      await expect(input).toBeVisible();
    }
  });

  test('precision inputs have default values', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToUnitsTab(page);

    // All precision inputs should have a numeric value between 0 and 6
    for (const key of PRECISION_KEYS) {
      const input = page.locator(`[data-testid="input-precision-${key}"]`);
      const value = await input.inputValue();
      const numValue = parseInt(value, 10);
      expect(numValue).toBeGreaterThanOrEqual(0);
      expect(numValue).toBeLessThanOrEqual(6);
    }
  });

  test('changing precision value enables Save button', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToUnitsTab(page);

    const saveButton = page.locator('[data-testid="settings-save-button"]');
    await expect(saveButton).toBeDisabled();

    // Change length precision
    const lengthInput = page.locator('[data-testid="input-precision-length"]');
    const currentValue = await lengthInput.inputValue();
    const newValue = currentValue === '1' ? '2' : '1';
    await lengthInput.fill(newValue);

    // Save button should be enabled
    await expect(saveButton).toBeEnabled();
  });

  test('precision values preserved when switching categories', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToUnitsTab(page);

    // Change depth precision
    const depthInput = page.locator('[data-testid="input-precision-depth"]');
    await depthInput.fill('3');

    // Switch to General settings
    await page.locator('[data-testid="settings-tab-general"]').click();
    await expect(page.locator('[data-testid="settings-panel-general"]')).toBeVisible();

    // Switch back to Units
    await navigateToUnitsTab(page);

    // Depth precision should still be 3
    const updatedInput = page.locator('[data-testid="input-precision-depth"]');
    await expect(updatedInput).toHaveValue('3');
  });

  test('each precision field has a corresponding setting row', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToUnitsTab(page);

    for (const key of PRECISION_KEYS) {
      const settingRow = page.locator(`[data-testid="setting-precision-${key}"]`);
      await expect(settingRow).toBeVisible();
    }
  });

  test('invalid precision triggers validation error indicator on tab', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToUnitsTab(page);

    // Set an out-of-range value (7, which is > max of 6)
    const lengthInput = page.locator('[data-testid="input-precision-length"]');
    await lengthInput.fill('7');

    // The Units tab should show an error indicator
    const errorDot = page.locator('[data-testid="settings-tab-error-units"]');
    await expect(errorDot).toBeVisible({ timeout: 3000 });
  });

  test('valid precision clears error indicator', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToUnitsTab(page);

    // Set invalid then valid
    const lengthInput = page.locator('[data-testid="input-precision-length"]');
    await lengthInput.fill('7');

    const errorDot = page.locator('[data-testid="settings-tab-error-units"]');
    await expect(errorDot).toBeVisible({ timeout: 3000 });

    // Fix the value
    await lengthInput.fill('2');
    await expect(errorDot).not.toBeVisible({ timeout: 3000 });
  });

  test('measurement unit selectors coexist with precision section', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToUnitsTab(page);

    // Both sections should be visible
    await expect(page.locator('[data-testid="section-measurement"]')).toBeVisible();
    await expect(page.locator('[data-testid="section-precision"]')).toBeVisible();

    // Unit selectors should still work
    await expect(page.locator('[data-testid="select-length-unit"]')).toBeVisible();
    await expect(page.locator('[data-testid="select-depth-unit"]')).toBeVisible();
    await expect(page.locator('[data-testid="select-angle-unit"]')).toBeVisible();
  });
});
