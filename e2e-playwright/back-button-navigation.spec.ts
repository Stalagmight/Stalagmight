import { test, expect } from './fixtures/coverage';
import { setupTauriMocks } from './mocks/tauri-ipc';

/**
 * E2E tests for Back button navigation.
 *
 * These tests verify that clicking the Back button in SingleSystemView:
 * 1. Returns to Global View without crashing
 * 2. Handles unsaved changes dialog correctly
 * 3. Handles rapid clicks gracefully
 * 4. Does not produce JavaScript errors (regression for app freeze bug)
 */

/**
 * Filter out known mock errors that are unrelated to the Back button bug.
 * The Back button crash produces "this.style.getLayer" error - we DO want to catch that.
 * Only 'unregisterListener' (Tauri mock cleanup) is filtered out.
 */
function filterMockErrors(errors: string[]): string[] {
  return errors.filter((error) => {
    // Keep the actual Back button crash error - this is what we're testing for
    if (error.includes('this.style.getLayer')) {
      return true;
    }
    // Filter out Tauri mock-environment errors (unrelated to Back button bug)
    if (error.includes('unregisterListener')) {
      return false;
    }
    // Resource-load failures from EXTERNAL hosts are network weather, not JS
    // errors in the app - this assertion exists for the getLayer crash class.
    // Scoped by the captured location URL (appended in beforeEach): a failed
    // SAME-ORIGIN load (a 404'd app chunk or asset) still fails the spec.
    // The app makes no external fetch any more (the maplibre CSS is bundled
    // since #576 C12 / #606, pinned by offline-map-assets.spec.ts); the former
    // offender, the dev agent bridge's /__agent/token probe, is gated off
    // under VITE_E2E since #417.
    if (error.startsWith('Failed to load resource') && !error.includes('[http://localhost')) {
      return false;
    }
    return true;
  });
}

/**
 * Helper to create a new project and wait for SingleSystemView
 */
async function createProject(page: import('./fixtures/coverage').Page, name: string) {
  const newProjectBtn = page.locator('button:has-text("New Project"), h3:has-text("New Project")').first();
  await newProjectBtn.click();
  await page.waitForSelector('.fixed.inset-0');
  await page.locator('.fixed.inset-0 input').first().fill(name);
  await page.click('.fixed.inset-0 button:has-text("Create")');
  await page.waitForSelector('canvas', { timeout: 10000 });
}

/** Open the in-project hamburger menu. */
async function openMenu(page: import('./fixtures/coverage').Page) {
  await page.locator('[data-testid="app-menu-trigger"]').click();
  await page.waitForSelector('[data-testid="app-menu"]', { timeout: 5000 });
}

/** Close the project via the hamburger "Close Project" item (now in the menu). */
async function clickBack(page: import('./fixtures/coverage').Page) {
  await openMenu(page);
  await page.click('[data-testid="back-button"]');
}

test.describe('Back Button Navigation', () => {
  test.beforeEach(async ({ page }) => {
    // Capture page errors - test will fail if any occur
    const errors: string[] = [];
    page.on('pageerror', (error) => {
      errors.push(error.message);
    });
    page.on('console', (msg) => {
      if (msg.type() === 'error') {
        // Resource-failure console messages ("Failed to load resource...")
        // carry no URL in their text - append the source location so
        // filterMockErrors can tell an external host's weather from a broken
        // same-origin app asset (#417).
        const url = msg.location().url;
        errors.push(url ? `${msg.text()} [${url}]` : msg.text());
      }
    });

    // Store for assertions
    (page as unknown as { __capturedErrors: string[] }).__capturedErrors = errors;

    // Set up Tauri mocks and navigate to app
    await setupTauriMocks(page);
    await page.goto('/');
    await page.waitForSelector('[data-testid="global-view"]');
  });

  test.afterEach(async ({ page }) => {
    const errors = (page as unknown as { __capturedErrors: string[] }).__capturedErrors;
    const relevantErrors = filterMockErrors(errors);
    expect(relevantErrors, 'No JavaScript errors should occur').toHaveLength(0);
  });

  test('should return to Global View without crashing (clean state) @smoke', async ({ page }) => {
    // Create a project to get to SingleSystemView
    await createProject(page, 'Back Button Test Project');

    // Verify we're in SingleSystemView (canvas is visible)
    await expect(page.locator('canvas').first()).toBeVisible();

    // Close the project via the hamburger menu
    await clickBack(page);

    // Verify we're back at Global View
    await expect(page.locator('[data-testid="global-view"]')).toBeVisible({ timeout: 10000 });
  });

  test('should handle unsaved changes dialog correctly @smoke', async ({ page }) => {
    // Create a project
    await createProject(page, 'Unsaved Changes Test Project');

    // Wait for project to be fully loaded
    await expect(page.locator('canvas').first()).toBeVisible();

    // Set dirty state via exposed store helper
    await page.evaluate(() => {
      const e2e = (window as unknown as {
        __E2E_TEST__?: {
          projectStore: {
            setState: (state: { isDirty: boolean }) => void;
          };
        };
      }).__E2E_TEST__;
      if (e2e && e2e.projectStore) {
        e2e.projectStore.setState({ isDirty: true });
      }
    });

    // Close the project via the hamburger menu
    await clickBack(page);

    // Verify unsaved changes dialog appears
    await expect(page.locator('[role="dialog"]')).toBeVisible({ timeout: 5000 });

    // Verify dialog has expected content - use dialog heading specifically
    await expect(page.locator('[role="dialog"] h2:has-text("Unsaved Changes")')).toBeVisible();

    // Click "Discard Changes" button
    await page.click('button:has-text("Discard")');

    // Verify clean return to Global View
    await expect(page.locator('[data-testid="global-view"]')).toBeVisible({ timeout: 10000 });
  });

  test('should stay in view when canceling unsaved changes dialog', async ({ page }) => {
    // Create a project
    await createProject(page, 'Cancel Dialog Test Project');

    // Wait for project to be fully loaded
    await expect(page.locator('canvas').first()).toBeVisible();

    // Set dirty state via exposed store helper
    await page.evaluate(() => {
      const e2e = (window as unknown as {
        __E2E_TEST__?: {
          projectStore: {
            setState: (state: { isDirty: boolean }) => void;
          };
        };
      }).__E2E_TEST__;
      if (e2e && e2e.projectStore) {
        e2e.projectStore.setState({ isDirty: true });
      }
    });

    // Close the project via the hamburger menu
    await clickBack(page);

    // Verify unsaved changes dialog appears
    await expect(page.locator('[role="dialog"]')).toBeVisible({ timeout: 5000 });

    // Click "Cancel" button to stay
    await page.click('button:has-text("Cancel")');

    // Verify dialog closes
    await expect(page.locator('[role="dialog"]')).not.toBeVisible({ timeout: 5000 });

    // Verify we're still in SingleSystemView (canvas visible)
    await expect(page.locator('canvas').first()).toBeVisible();
  });

  test('should handle rapid Back clicks gracefully', async ({ page }) => {
    // Create a project
    await createProject(page, 'Rapid Click Test Project');

    // Wait for project to be fully loaded
    await expect(page.locator('canvas').first()).toBeVisible();

    // Open the menu and click "Close Project" (now a menu item) three times in a
    // row. A Playwright click per activation cannot do that: the first one closes
    // the menu, and the item is gone before the next can land (#576 NF-W10FE-1).
    // So the three clicks are dispatched in one task in the page, before React
    // re-renders the closed menu: each reaches the item's handler, and three
    // handleCloseProject calls overlap - the store's isClosing guard is what keeps
    // that to ONE close_project (#576 RV10b-5).
    await openMenu(page);
    const clicks = await page.evaluate(() => {
      const w = window as unknown as {
        __closeProjectCalls__: number;
        __TAURI_INTERNALS__: { invoke: (cmd: string, args?: unknown) => Promise<unknown> };
      };
      w.__closeProjectCalls__ = 0;
      const invoke = w.__TAURI_INTERNALS__.invoke;
      w.__TAURI_INTERNALS__.invoke = (cmd, args) => {
        if (cmd === 'close_project') w.__closeProjectCalls__ += 1;
        return invoke(cmd, args);
      };
      const item = document.querySelector<HTMLButtonElement>('[data-testid="back-button"]');
      let landed = 0;
      item?.addEventListener('click', () => { landed += 1; });
      item?.click();
      item?.click();
      item?.click();
      return { landed, stillInMenu: item?.isConnected === true };
    });
    expect(clicks).toEqual({ landed: 3, stillInMenu: true });

    // Should still end up at Global View, closed once, with no errors
    await expect(page.locator('[data-testid="global-view"]')).toBeVisible({ timeout: 10000 });
    await expect(page.locator('[data-testid="back-button"]')).toHaveCount(0);
    expect(
      await page.evaluate(() => (window as unknown as { __closeProjectCalls__: number }).__closeProjectCalls__),
    ).toBe(1);
  });

  test('should handle Back button immediately after opening project', async ({ page }) => {
    // Create a project
    await createProject(page, 'Immediate Back Test Project');

    // Close immediately (don't wait for map to fully load), via the menu
    await clickBack(page);

    // Should return to Global View without crashing
    await expect(page.locator('[data-testid="global-view"]')).toBeVisible({ timeout: 10000 });
  });

  test('should handle Back button with grid overlay visible', async ({ page }) => {
    // Create a project
    await createProject(page, 'Grid Visible Test Project');

    // Wait for project to be fully loaded
    await expect(page.locator('canvas').first()).toBeVisible();

    // Enable grid overlay via store
    await page.evaluate(() => {
      const e2e = (window as unknown as {
        __E2E_TEST__?: {
          uiStore: {
            setState: (state: { showGrid: boolean }) => void;
          };
        };
      }).__E2E_TEST__;
      if (e2e && e2e.uiStore) {
        e2e.uiStore.setState({ showGrid: true });
      }
    });

    // The grid overlay is actually on the map before the project is closed
    // (GridOverlay adds the 'grid-lines' layer once showGrid is set).
    await page.waitForFunction(() => {
      const map = (window as { __stalagmight_map__?: { getLayer?: (id: string) => unknown } }).__stalagmight_map__;
      return Boolean(map?.getLayer?.('grid-lines'));
    });

    // Close the project via the hamburger menu
    await clickBack(page);

    // Should return to Global View without crashing (grid cleanup should work)
    await expect(page.locator('[data-testid="global-view"]')).toBeVisible({ timeout: 10000 });
  });

  test('should handle Back button with grid overlay hidden', async ({ page }) => {
    // Create a project
    await createProject(page, 'Grid Hidden Test Project');

    // Wait for project to be fully loaded
    await expect(page.locator('canvas').first()).toBeVisible();

    // Ensure grid overlay is hidden (default state)
    await page.evaluate(() => {
      const e2e = (window as unknown as {
        __E2E_TEST__?: {
          uiStore: {
            setState: (state: { showGrid: boolean }) => void;
          };
        };
      }).__E2E_TEST__;
      if (e2e && e2e.uiStore) {
        e2e.uiStore.setState({ showGrid: false });
      }
    });

    // Close the project via the hamburger menu
    await clickBack(page);

    // Should return to Global View without crashing
    await expect(page.locator('[data-testid="global-view"]')).toBeVisible({ timeout: 10000 });
  });
});
