import { test, expect, Page } from './fixtures/coverage';
import { setupTauriMocks } from './mocks/tauri-ipc';
import { openSettingsFromMenu, closeProjectViaMenu } from './helpers/app';

/**
 * E2E tests for theme switching: data-theme attribute, CSS variable resolution,
 * and Settings Appearance tab functionality.
 */

/**
 * Helper to create a new project using stable data-testid selectors.
 */
async function createProject(page: Page, name: string): Promise<void> {
  const newProjectBtn = page.locator('[data-testid="new-project-button"]');
  await expect(newProjectBtn).toBeVisible({ timeout: 5000 });
  await newProjectBtn.click();

  // Wait for the New Project dialog to appear
  await expect(
    page.locator('[data-testid="new-project-dialog"]')
  ).toBeVisible({ timeout: 5000 });

  // Fill in the project name via its stable test ID
  await page.locator('[data-testid="new-project-name-input"]').fill(name);

  // Click Create via its stable test ID
  await page.locator('[data-testid="new-project-create-button"]').click();
  await page.waitForSelector('canvas', { timeout: 10000 });

  // Wait for map to be fully initialized (per memory pitfall)
  await page.waitForFunction(
    () =>
      (window as unknown as { __stalagmight_map__?: unknown }).__stalagmight_map__ !==
      undefined,
    { timeout: 10000 }
  );
}

/**
 * Helper to go back to GlobalView from a project view
 */
async function goBackToGlobalView(page: Page): Promise<void> {
  if (
    await page
      .getByTestId('global-view')
      .isVisible()
      .catch(() => false)
  ) {
    return;
  }
  await closeProjectViaMenu(page);
}

/**
 * Helper to open Settings dialog from GlobalView
 */
async function openSettingsDialog(page: Page): Promise<void> {
  await goBackToGlobalView(page);

  // Settings now lives inside the hamburger menu: open the menu, click
  // settings-button, and wait for the dialog (shared helper).
  await openSettingsFromMenu(page);
}

/**
 * Helper to navigate to the Appearance tab in Settings
 */
async function navigateToAppearance(page: Page): Promise<void> {
  const appearanceTab = page.locator(
    '[data-testid="settings-tab-appearance"]'
  );
  await expect(appearanceTab).toBeVisible();
  await appearanceTab.click();
  await expect(
    page.locator('[data-testid="settings-panel-appearance"]')
  ).toBeVisible({ timeout: 3000 });
}

/**
 * Helper to select a theme mode via the custom Select dropdown.
 * Clicks the combobox trigger, then clicks the matching option.
 */
async function selectThemeMode(page: Page, label: string): Promise<void> {
  const trigger = page.locator('[data-testid="theme-mode-select"]');
  await trigger.click();

  // The dropdown is portaled to document.body - find the option there
  const option = page.locator(`[role="option"]:has-text("${label}")`);
  await expect(option).toBeVisible({ timeout: 3000 });
  await option.click();
}

/**
 * Parse a CSS color string into { r, g, b } components.
 * Handles both comma-separated (CSS Color 3) and space-separated (CSS Color 4)
 * rgb/rgba forms, as well as hex #rrggbb notation.
 */
function parseColor(color: string): { r: number; g: number; b: number } {
  // Handle comma-separated rgb(r, g, b) and rgba(r, g, b, a)
  const commaMatch = color.match(
    /rgba?\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)/
  );
  if (commaMatch) {
    return {
      r: parseInt(commaMatch[1]!, 10),
      g: parseInt(commaMatch[2]!, 10),
      b: parseInt(commaMatch[3]!, 10),
    };
  }
  // Handle space-separated rgb(r g b) and rgb(r g b / a) (CSS Color 4)
  const spaceMatch = color.match(
    /rgba?\(\s*(\d+)\s+(\d+)\s+(\d+)/
  );
  if (spaceMatch) {
    return {
      r: parseInt(spaceMatch[1]!, 10),
      g: parseInt(spaceMatch[2]!, 10),
      b: parseInt(spaceMatch[3]!, 10),
    };
  }
  // Handle hex #rrggbb
  const hexMatch = color.match(
    /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i
  );
  if (hexMatch) {
    return {
      r: parseInt(hexMatch[1]!, 16),
      g: parseInt(hexMatch[2]!, 16),
      b: parseInt(hexMatch[3]!, 16),
    };
  }
  throw new Error(`Cannot parse color: ${color}`);
}

/**
 * Assert two colors are approximately equal (within tolerance).
 * Uses numeric comparison for cross-engine stability per pitfall guidance.
 */
function assertColorsApproximatelyEqual(
  actual: string,
  expected: { r: number; g: number; b: number },
  tolerance = 2
) {
  const parsed = parseColor(actual);
  expect(Math.abs(parsed.r - expected.r)).toBeLessThanOrEqual(tolerance);
  expect(Math.abs(parsed.g - expected.g)).toBeLessThanOrEqual(tolerance);
  expect(Math.abs(parsed.b - expected.b)).toBeLessThanOrEqual(tolerance);
}

// Light theme --cc-background is #f5f0e8 -> rgb(245, 240, 232)
const LIGHT_BG = { r: 245, g: 240, b: 232 };
// Cave theme --cc-background is #0e0d0c -> rgb(14, 13, 12)
const CAVE_BG = { r: 14, g: 13, b: 12 };
// Light theme --cc-map-background is #faf7f2 -> rgb(250, 247, 242).
// This is the token themeColors.ts feeds to MapLibre's background layer, so it
// is what the user perceives as "the app background" (#332).
const LIGHT_MAP_BG = { r: 250, g: 247, b: 242 };
// Light theme --cc-stroke-color is #1a120b -> rgb(26, 18, 11)
const LIGHT_STROKE = { r: 26, g: 18, b: 11 };

test.describe('Theme Switching - Default Dark Theme', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    // Assert on GlobalView (app-ready signal) without creating a project.
    // This tests theme state truly "on app load" before any view transition.
    await expect(
      page.locator('[data-testid="global-view"]')
    ).toBeVisible({ timeout: 10000 });
    // GlobalView mounts before loadConfig() resolves, and useTheme() applies the
    // built-in 'dark' fallback until then. Gate on the config-derived theme so
    // these tests never observe the pre-config fallback.
    await expect
      .poll(
        () =>
          page.evaluate(() => document.documentElement.dataset.theme),
        { timeout: 10000 }
      )
      .toBe('cave');
  });

  test('default theme is dark on app load @smoke', async ({ page }) => {
    // Verify data-theme attribute on document root immediately after load
    // Config has theme:'dark' + night_color_theme:'cave', so data-theme resolves to 'cave'
    const dataTheme = await page.evaluate(
      () => document.documentElement.dataset.theme
    );
    expect(dataTheme).toBe('cave');
  });

  test('CSS variables resolve correctly in dark theme', async ({
    page,
  }) => {
    // Read --cc-background via getComputedStyle
    const bgColor = await page.evaluate(() => {
      return getComputedStyle(document.documentElement)
        .getPropertyValue('--cc-background')
        .trim();
    });

    // Parse numerically for cross-engine stability
    assertColorsApproximatelyEqual(bgColor, CAVE_BG);
  });
});

test.describe('Theme Switching - Via Settings UI', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Theme Settings Test');
  });

  test('theme can be changed to light via Settings Appearance tab @smoke', async ({
    page,
  }) => {
    await openSettingsDialog(page);
    await navigateToAppearance(page);

    // Use the custom Select dropdown to switch to Light mode
    await selectThemeMode(page, 'Light');

    // Save settings
    const saveButton = page.locator(
      '[data-testid="settings-save-button"]'
    );
    await expect(saveButton).toBeEnabled();
    await saveButton.click();

    // Wait for dialog to close (save closes dialog)
    await expect(
      page.locator('[data-testid="settings-dialog"]')
    ).not.toBeVisible({ timeout: 5000 });

    // Verify data-theme attribute changed to light
    // (default day color theme is 'light')
    const dataTheme = await page.evaluate(
      () => document.documentElement.dataset.theme
    );
    expect(dataTheme).toBe('light');
  });

  test('CSS variables resolve to light values after switching theme', async ({
    page,
  }) => {
    await openSettingsDialog(page);
    await navigateToAppearance(page);

    // Use the custom Select dropdown to switch to Light mode
    await selectThemeMode(page, 'Light');

    // Save
    const saveButton = page.locator(
      '[data-testid="settings-save-button"]'
    );
    await expect(saveButton).toBeEnabled();
    await saveButton.click();

    // Wait for dialog to close
    await expect(
      page.locator('[data-testid="settings-dialog"]')
    ).not.toBeVisible({ timeout: 5000 });

    // Verify --cc-background resolves to the light value
    const bgColor = await page.evaluate(() => {
      return getComputedStyle(document.documentElement)
        .getPropertyValue('--cc-background')
        .trim();
    });

    assertColorsApproximatelyEqual(bgColor, LIGHT_BG);
  });

  test('map canvas tokens resolve to light values in light mode', async ({
    page,
  }) => {
    // Regression guard for #332: light mode used to theme only the UI chrome,
    // leaving the MapLibre background layer on the dark :root value, so the app
    // still looked dark. Chrome-only assertions above cannot catch that.
    await openSettingsDialog(page);
    await navigateToAppearance(page);

    await selectThemeMode(page, 'Light');

    const saveButton = page.locator(
      '[data-testid="settings-save-button"]'
    );
    await expect(saveButton).toBeEnabled();
    await saveButton.click();

    await expect(
      page.locator('[data-testid="settings-dialog"]')
    ).not.toBeVisible({ timeout: 5000 });

    const { mapBg, strokeColor } = await page.evaluate(() => {
      const styles = getComputedStyle(document.documentElement);
      return {
        mapBg: styles.getPropertyValue('--cc-map-background').trim(),
        strokeColor: styles.getPropertyValue('--cc-stroke-color').trim(),
      };
    });

    assertColorsApproximatelyEqual(mapBg, LIGHT_MAP_BG);
    // The outline colour has to invert too, or survey lines would be white on a
    // near-white canvas.
    assertColorsApproximatelyEqual(strokeColor, LIGHT_STROKE);
  });

  test('MapLibre paints the light canvas, not just the CSS token', async ({
    page,
  }) => {
    // #332 was a token that resolved correctly but never reached MapLibre, so
    // the test above cannot fail for the actual bug: it reads the same CSS
    // custom property the stylesheet declares. This one reads the paint
    // property off the live map. That needs a project open, which is why the
    // project is created AFTER saving - openSettingsDialog closes the one the
    // beforeEach made, and MapCanvas deletes __stalagmight_map__ on unmount.
    //
    // What this pins: the map's INITIAL paint, MapCanvas.tsx:1691. Not the
    // [resolvedTheme, mapLoaded] re-apply at :2317 - the theme is already light
    // by the time createProject() builds the map, so init alone turns this
    // assertion green and deleting the re-apply effect would not fail it.
    // The re-apply path is unreachable from Playwright as the app is shaped
    // today: openSettingsDialog() starts with goBackToGlobalView(), and the
    // hamburger that holds Settings only exists in global view, so a project
    // can never be open across a theme change. If Settings ever becomes
    // reachable in-project, add a second case that opens a project FIRST and
    // then switches - that one would pin :2317.
    await openSettingsDialog(page);
    await navigateToAppearance(page);
    await selectThemeMode(page, 'Light');

    const saveButton = page.locator('[data-testid="settings-save-button"]');
    await expect(saveButton).toBeEnabled();
    await saveButton.click();
    await expect(
      page.locator('[data-testid="settings-dialog"]')
    ).not.toBeVisible({ timeout: 5000 });

    await createProject(page, 'Theme Map Paint Test');

    const painted = await page.evaluate(() => {
      const map = (
        window as unknown as {
          __stalagmight_map__?: {
            getPaintProperty: (layer: string, prop: string) => unknown;
          };
        }
      ).__stalagmight_map__;
      return map
        ? String(map.getPaintProperty('background', 'background-color'))
        : null;
    });

    expect(painted).not.toBeNull();
    assertColorsApproximatelyEqual(painted as string, LIGHT_MAP_BG);
  });

  test('theme applies immediately on selection before saving', async ({
    page,
  }) => {
    await openSettingsDialog(page);
    await navigateToAppearance(page);

    // Verify night color theme is active before change (dark mode → night_color_theme 'cave')
    const dataThemeBefore = await page.evaluate(
      () => document.documentElement.dataset.theme
    );
    expect(dataThemeBefore).toBe('cave');

    // Use the custom Select dropdown to switch to Light (but do NOT save)
    await selectThemeMode(page, 'Light');

    // Theme should apply immediately (live preview)
    const dataThemeAfter = await page.evaluate(
      () => document.documentElement.dataset.theme
    );
    expect(dataThemeAfter).toBe('light');

    // CSS variables should also resolve to light values
    const bgColor = await page.evaluate(() => {
      return getComputedStyle(document.documentElement)
        .getPropertyValue('--cc-background')
        .trim();
    });
    assertColorsApproximatelyEqual(bgColor, LIGHT_BG);
  });

  test('theme reverts when canceling unsaved changes', async ({ page }) => {
    await openSettingsDialog(page);
    await navigateToAppearance(page);

    // Use the custom Select dropdown to switch to Light (do NOT save)
    await selectThemeMode(page, 'Light');

    // Verify live preview is active
    const dataThemeDuring = await page.evaluate(
      () => document.documentElement.dataset.theme
    );
    expect(dataThemeDuring).toBe('light');

    // Cancel the dialog (click Cancel, then Discard in unsaved changes dialog)
    const cancelButton = page.locator(
      '[data-testid="settings-cancel-button"]'
    );
    await cancelButton.click();

    // Unsaved changes dialog should appear
    await expect(
      page.locator('[data-testid="unsaved-changes-dialog"]')
    ).toBeVisible({ timeout: 3000 });

    // Discard changes
    const discardButton = page.locator('[data-testid="unsaved-discard"]');
    await discardButton.click();

    // Settings dialog should close
    await expect(
      page.locator('[data-testid="settings-dialog"]')
    ).not.toBeVisible({ timeout: 5000 });

    // Theme should revert to night color theme ('cave')
    const dataThemeAfter = await page.evaluate(
      () => document.documentElement.dataset.theme
    );
    expect(dataThemeAfter).toBe('cave');
  });

  test('theme persists across Settings dialog close and reopen', async ({
    page,
  }) => {
    // Switch to light mode
    await openSettingsDialog(page);
    await navigateToAppearance(page);

    await selectThemeMode(page, 'Light');

    // Save and close
    const saveButton = page.locator(
      '[data-testid="settings-save-button"]'
    );
    await expect(saveButton).toBeEnabled();
    await saveButton.click();
    await expect(
      page.locator('[data-testid="settings-dialog"]')
    ).not.toBeVisible({ timeout: 5000 });

    // Verify light theme is active
    const dataThemeBefore = await page.evaluate(
      () => document.documentElement.dataset.theme
    );
    expect(dataThemeBefore).toBe('light');

    // Reopen Settings and navigate to Appearance
    await openSettingsDialog(page);
    await navigateToAppearance(page);

    // In light (day) mode, the day section should show the Active badge
    await expect(
      page.locator('[data-testid="theme-active-badge-day"]')
    ).toBeVisible();

    // Night section should not have an active badge (night mode is not active)
    await expect(
      page.locator('[data-testid="theme-active-badge-night"]')
    ).not.toBeVisible();
  });

  test('color theme can be changed via circle selectors', async ({
    page,
  }) => {
    await openSettingsDialog(page);
    await navigateToAppearance(page);

    // Default: night mode active with dark color theme
    // Click the light color circle in the night section to change night color theme
    const nightLightCircle = page.locator(
      '[data-testid="theme-section-night"] [data-testid="color-theme-circle-light"]'
    );
    await nightLightCircle.click();

    // Should apply light color theme even in dark/night mode (live preview)
    const dataTheme = await page.evaluate(
      () => document.documentElement.dataset.theme
    );
    expect(dataTheme).toBe('light');

    // CSS variables should resolve to light values
    const bgColor = await page.evaluate(() => {
      return getComputedStyle(document.documentElement)
        .getPropertyValue('--cc-background')
        .trim();
    });
    assertColorsApproximatelyEqual(bgColor, LIGHT_BG);
  });

  test('cave color theme can be selected via circle selector', async ({
    page,
  }) => {
    await openSettingsDialog(page);
    await navigateToAppearance(page);

    // Click the cave color circle in the night section
    const nightCaveCircle = page.locator(
      '[data-testid="theme-section-night"] [data-testid="color-theme-circle-cave"]'
    );
    await nightCaveCircle.click();

    // Should apply cave color theme (live preview)
    const dataTheme = await page.evaluate(
      () => document.documentElement.dataset.theme
    );
    expect(dataTheme).toBe('cave');

    // CSS variables should resolve to cave values
    const bgColor = await page.evaluate(() => {
      return getComputedStyle(document.documentElement)
        .getPropertyValue('--cc-background')
        .trim();
    });
    assertColorsApproximatelyEqual(bgColor, CAVE_BG);
  });
});
