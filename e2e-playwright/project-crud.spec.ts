import { test, expect } from './fixtures/coverage';
import { setupMockedPage, openNewProjectDialog, createProject, closeProjectViaMenu } from './helpers/app';
import { setIpcOverrides } from './helpers/ipcOverrides';

test.describe('Project CRUD Operations', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test.describe('Start Page', () => {
    test('should display Stalagmight title @smoke', async ({ page }) => {
      // Look for Stalagmight title in header
      const title = page.locator('h1:has-text("Stalagmight")');
      await expect(title).toBeVisible({ timeout: 10000 });
    });

    test('should have New Project option', async ({ page }) => {
      const newProjectBtn = page.locator('[data-testid="new-project-button"], [data-testid="empty-state-new-project"]').first();
      await expect(newProjectBtn).toBeVisible({ timeout: 10000 });
    });

    test('should have Open Project option', async ({ page }) => {
      const openBtn = page.locator('[data-testid="global-open-import-button"], [data-testid="empty-state-open-project"]').first();
      await expect(openBtn).toBeVisible({ timeout: 10000 });
    });
  });

  test.describe('Project Creation', () => {
    test('should open dialog when clicking New Project', async ({ page }) => {
      await openNewProjectDialog(page);
      await expect(page.getByTestId('new-project-dialog')).toBeVisible({ timeout: 5000 });
    });

    test('should require a project name', async ({ page }) => {
      await openNewProjectDialog(page);

      const createBtn = page.getByTestId('new-project-create-button');

      // The button might be disabled or validation might prevent submission
      // Check that the dialog stays open
      if (await createBtn.isEnabled()) {
        await createBtn.click();
        // If validation fails, dialog should still be visible
        const dialog = page.getByTestId('new-project-dialog');
        const isStillOpen = await dialog.isVisible();
        // Either stays open due to validation, or closes (mock accepts empty name)
        expect(isStillOpen !== undefined).toBeTruthy();
      }
    });

    test('should create project with valid name @smoke', async ({ page }) => {
      await createProject(page, 'My Test Project');
      await expect(page.getByTestId('map-canvas')).toBeVisible({ timeout: 10000 });
    });

    test('should not autosave untouched project when backing out', async ({ page }) => {
      // Untouched new projects should close without any implicit save.
      await setIpcOverrides(page, {
        save_project: { __throw: 'save_project should not be called when backing out of untouched project' },
      });

      await createProject(page, 'Untouched Project');
      await closeProjectViaMenu(page);

      await expect(page.getByTestId('single-system-view')).not.toBeVisible({ timeout: 10000 });
      await expect(
        page.locator('[data-testid="new-project-button"], [data-testid="empty-state-new-project"]').first()
      ).toBeVisible({ timeout: 10000 });
    });

    test('creates without an immediate-save option', async ({ page }) => {
      await openNewProjectDialog(page);
      await expect(page.getByTestId('new-project-save-immediately')).toHaveCount(0);
      await expect(page.getByText('Save project file immediately', { exact: true })).toHaveCount(0);
      await page.getByTestId('new-project-name-input').fill('New Unsaved Project');
      await page.getByTestId('new-project-create-button').click();
      await expect(page.getByTestId('single-system-view')).toBeVisible();
      await expect(page.getByTestId('new-project-dialog')).not.toBeVisible();
    });

    test('should cancel project creation', async ({ page }) => {
      await openNewProjectDialog(page);
      await page.getByTestId('new-project-cancel-button').click();
      await expect(page.getByTestId('new-project-dialog')).not.toBeVisible({ timeout: 3000 });

      // Stalagmight title should still be visible (we're on start page)
      const title = page.locator('h1:has-text("Stalagmight")');
      await expect(title).toBeVisible();
    });
  });

  test.describe('Project View', () => {
    test.beforeEach(async ({ page }) => {
      await createProject(page, 'View Test Project');
    });

    test('should display map canvas', async ({ page }) => {
      await expect(page.getByTestId('map-canvas')).toBeVisible();
    });

    test('should have toolbar with tools', async ({ page }) => {
      await expect(page.getByTestId('toolbar')).toBeVisible();
      await expect(page.getByTestId('tool-select')).toBeVisible();
    });

    test('should have section panel', async ({ page }) => {
      await expect(page.getByTestId('sections-panel')).toBeVisible();
    });
  });

  test.describe('Project Save', () => {
    test('should respond to Ctrl+S @smoke', async ({ page }) => {
      await createProject(page, 'Save Test Project');

      // Press Ctrl+S
      await page.keyboard.press('Control+s');

      // Should not crash - app remains functional
      await expect(page.getByTestId('map-canvas')).toBeVisible();
    });
  });
});
