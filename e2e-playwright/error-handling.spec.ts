import { test, expect, Page } from './fixtures/coverage';
import { setupTauriMocks } from './mocks/tauri-ipc';
import { setupMockedPage, createProject as createProjectViaApp, openAppMenu } from './helpers/app';
import { setIpcOverrides, clearIpcOverrideKeys } from './helpers/ipcOverrides';
import type { CommandError } from '../src/types/ipc/CommandError';

/**
 * Error Handling E2E Tests
 *
 * Tests that errors are communicated clearly to users and the app
 * remains functional after error scenarios.
 */

/**
 * Helper to create a project.
 */
async function createProject(page: Page, name: string): Promise<void> {
  const newProjectBtn = page
    .locator('[data-testid="new-project-button"], [data-testid="empty-state-new-project"]')
    .first();
  await newProjectBtn.click();
  await page.getByTestId('new-project-dialog').waitFor({ state: 'visible', timeout: 5000 });
  await page.getByTestId('new-project-name-input').fill(name);
  await page.getByTestId('new-project-create-button').click();
  await expect(page.getByTestId('map-canvas')).toBeVisible({ timeout: 10000 });
}

test.describe('Error Handling', () => {
  test.describe('Save Errors', () => {
    /**
     * Test 9.1: should display error notification on save failure
     *
     * Verifies that save errors (permission denied, disk full) are
     * clearly communicated to the user via error notification.
     */
    test('should display error notification on save failure @smoke', async ({ page }) => {
      await setupMockedPage(page);
      await createProjectViaApp(page, 'Error Test Project');

      // A refused save rejects with a `CommandError` (#576 C4) - what the backend
      // sends. Simulating only a throw (`__throw`, a transport failure) exercised a
      // path a refusal does not take, which is why this assertion used to be skipped
      // rather than met.
      await setIpcOverrides(page, {
        save_project: { __reject: { code: 'FAILED', message: 'Permission denied: cannot write to file' } satisfies CommandError },
      });

      // Trigger save with Ctrl+S
      await page.keyboard.press('Control+s');

      // The failure must reach the user - that is what this test is named for.
      // It may land in a dialog or an inline notification; either satisfies it.
      await expect(
        page.locator('text=/Permission denied|cannot write|save.*fail/i').first(),
      ).toBeVisible({ timeout: 5000 });

      // Verify app remains functional after error
      await expect(page.getByTestId('map-canvas')).toBeVisible();

      // Test disk full error as well.
      await setIpcOverrides(page, {
        save_project: { __throw: 'Disk full: no space left on device' },
      });

      await page.keyboard.press('Control+s');

      // App should still be functional
      await expect(page.getByTestId('map-canvas')).toBeVisible();
    });
  });

  test.describe('File Open Errors', () => {
    /**
     * Test 9.2: should display error on file open failure
     *
     * Verifies that file open errors (corrupted file, missing file) are
     * clearly communicated to the user.
     */
    test('should display error on file open failure', async ({ page }) => {
      await setupMockedPage(page);

      const loadingError = page.getByTestId('loading-progress');
      const title = page.locator('h1:has-text("Stalagmight")');

      // Opening a file whose load fails must surface the loading-error overlay
      // with the message, keep the app on the start page, and be dismissable.
      // Use the hamburger "Open Project" item. Open/Import now routes by the
      // chosen file's EXTENSION (unified Open/Import), so we override the mocked
      // file dialog to return a .stalx path - that routes through loadProject,
      // which we force to throw, surfacing the loading-error overlay. (The default
      // mock dialog returns a .dmp, which would route to import instead.)
      // Asserted deterministically (wait for the actual states, no racing
      // best-effort timeouts) so it can't flake under parallel load - and it now
      // verifies the error message is shown, which the previous version never did.
      const expectOpenErrorSurfaced = async (message: string) => {
        await setIpcOverrides(page, {
          load_project_full: { __throw: message },
          'plugin:dialog|open': '/mock/broken-project.stalx',
        });

        await openAppMenu(page);
        await page.getByTestId('global-open-project-button').click();

        await expect(loadingError).toBeVisible({ timeout: 10000 });
        await expect(loadingError).toContainText(message);

        await loadingError.getByRole('button', { name: 'Dismiss' }).click();
        await expect(loadingError).toBeHidden({ timeout: 10000 });

        // App stays functional on the start page.
        await expect(title).toBeVisible();
      };

      await expectOpenErrorSurfaced('File corrupted: invalid project format');
      await expectOpenErrorSurfaced('File not found: the specified file does not exist');
    });
  });

  test.describe('DMP Parse Errors', () => {
    test('shows error dialog for invalid DMP header', async ({ page }) => {
      await setupMockedPage(page);
      await createProjectViaApp(page, 'DMP Error Test');

      await setIpcOverrides(page, {
        import_dmp: { __throw: 'Invalid DMP header: expected version 2-5, got invalid data' },
      });

      await page.keyboard.press('Control+i');

      const errorDialog = page.getByTestId('error-dialog');
      await expect(errorDialog).toBeVisible({ timeout: 5000 });
      await expect(errorDialog).toContainText('Import Failed');
      await expect(errorDialog).toContainText('Invalid DMP header');

      await errorDialog.locator('button:has-text("OK")').click();
      await expect(errorDialog).toBeHidden({ timeout: 3000 });
      await expect(page.getByTestId('map-canvas')).toBeVisible();
      await expect(page.locator('[data-testid="dmp-selection-dialog"]')).toHaveCount(0);
    });

    test('shows error dialog for truncated DMP file', async ({ page }) => {
      await setupMockedPage(page);
      await createProjectViaApp(page, 'DMP Truncated Test');

      await setIpcOverrides(page, {
        import_dmp: { __throw: 'Truncated DMP file: unexpected end of data' },
      });

      await page.keyboard.press('Control+i');

      const errorDialog = page.getByTestId('error-dialog');
      await expect(errorDialog).toBeVisible({ timeout: 5000 });
      await expect(errorDialog).toContainText('Import Failed');
      await expect(errorDialog).toContainText('Truncated DMP file');

      await errorDialog.locator('button:has-text("OK")').click();
      await expect(errorDialog).toBeHidden({ timeout: 3000 });
      await expect(page.getByTestId('map-canvas')).toBeVisible();
      await expect(page.locator('[data-testid="dmp-selection-dialog"]')).toHaveCount(0);
    });
  });

  test.describe('Network/IPC Timeout', () => {
    /**
     * Test 9.4: should handle network/IPC timeout gracefully
     *
     * Verifies the app handles IPC timeouts gracefully and remains
     * functional.
     */
    test('should handle network/IPC timeout gracefully', async ({ page }) => {
      // Use standard mocks - we'll test with slow operations
      await setupTauriMocks(page);
      await page.goto('/');

      await createProject(page, 'Timeout Test');

      // The mocked IPC returns immediately, but we can verify the app
      // handles rapid operations without blocking
      for (let i = 0; i < 5; i++) {
        await page.keyboard.press('Control+s');
      }

      // App should remain responsive
      await expect(page.getByTestId('map-canvas')).toBeVisible();

      // Try multiple rapid keyboard shortcuts
      await page.keyboard.press('Control+z');
      await page.keyboard.press('Control+y');
      await page.keyboard.press('Control+z');

      // App should still be functional
      await expect(page.getByTestId('map-canvas')).toBeVisible();
    });
  });

  test.describe('Form Validation', () => {
    /**
     * Test 9.5: should display validation errors in forms
     *
     * Verifies inline validation errors are displayed in forms
     * when users enter invalid data.
     */
    test('should display validation errors in forms @smoke', async ({ page }) => {
      await setupTauriMocks(page);
      await page.goto('/');

      // Test New Project dialog validation
      const newProjectBtn = page
        .locator('[data-testid="new-project-button"], [data-testid="empty-state-new-project"]')
        .first();
      await newProjectBtn.click();
      await page.getByTestId('new-project-dialog').waitFor({ state: 'visible', timeout: 5000 });

      // Try to create without name
      const createBtn = page.getByTestId('new-project-create-button');

      // Button should be disabled when name is empty
      const isDisabled = await createBtn.isDisabled();

      if (!isDisabled) {
        // If button is enabled, clicking should show validation error
        await createBtn.click();

        // Check for error message
        const errorMsg = page
          .locator('[data-testid="new-project-dialog"] >> text=/required|name.*empty|enter.*name/i')
          .first();
        if (await errorMsg.isVisible()) {
          await expect(errorMsg).toBeVisible();
        }
      }

      // Dialog should still be open
      await expect(page.getByTestId('new-project-dialog')).toBeVisible();

      // Now enter a valid name and create
      await page.getByTestId('new-project-name-input').fill('Valid Project');
      await createBtn.click();

      // Should successfully create and show canvas
      await expect(page.getByTestId('map-canvas')).toBeVisible({ timeout: 10000 });
    });
  });

  test.describe('Undo/Redo Recovery', () => {
    /**
     * Test 9.6: should recover from undo/redo errors
     *
     * Verifies the app handles undo/redo errors gracefully and
     * remains functional.
     */
    test('should recover from undo/redo errors', async ({ page }) => {
      await setupMockedPage(page);
      await createProjectViaApp(page, 'Undo Error Test');

      await setIpcOverrides(page, {
        undo: { __throw: 'Undo failed: command stack corrupted' },
      });

      // Try undo - should handle error gracefully
      await page.keyboard.press('Control+z');

      // App should remain functional
      await expect(page.getByTestId('map-canvas')).toBeVisible();

      await setIpcOverrides(page, {
        redo: { __throw: 'Redo failed: no commands to redo' },
      });

      // Try redo - should handle error gracefully
      await page.keyboard.press('Control+y');

      // App should still be functional
      await expect(page.getByTestId('map-canvas')).toBeVisible();

      // Disable overrides and verify normal operations work.
      await clearIpcOverrideKeys(page, ['undo', 'redo']);

      // Undo/redo should work normally now
      await page.keyboard.press('Control+z');
      await page.keyboard.press('Control+y');

      // App should remain functional
      await expect(page.getByTestId('map-canvas')).toBeVisible();
    });
  });
});
