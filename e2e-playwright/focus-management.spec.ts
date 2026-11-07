import { test, expect, Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  openNewProjectDialog,
  openDmpSelectionDialog,
  proceedToDmpSectionConfig,
  ensureDmpNewImportDialogVisible,
  seedProjectWithSections,
  waitForMapReady,
  waitForNoDialogs,
  openSettingsFromMenu,
} from './helpers/app';
import type { E2EWindow } from './helpers/e2eWindow';

/**
 * E2E tests for focus management: focus traps, return-focus, initial focus,
 * stacked dialog isolation, and SettingsSidebar roving tabindex.
 */

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Open the Settings dialog from GlobalView via the hamburger menu. */
async function openSettingsDialog(page: Page): Promise<void> {
  await openSettingsFromMenu(page);
}

/**
 * Return the `data-testid` attribute of the currently-focused element.
 * Falls back to `null` when no element has the attribute.
 */
async function focusedTestId(page: Page): Promise<string | null> {
  return page.evaluate(() => document.activeElement?.getAttribute('data-testid') ?? null);
}

// ---------------------------------------------------------------------------
// Test suites
// ---------------------------------------------------------------------------

test.describe('Focus Management', () => {
  test.describe('Tab cycling - form dialog (NewProjectDialog)', () => {
    test.beforeEach(async ({ page }) => {
      await setupMockedPage(page);
    });

    test('Tab wraps from last to first focusable element', async ({ page }) => {
      await openNewProjectDialog(page);

      // NewProjectDialog is a form dialog - initial focus lands on the first text input
      await expect(page.getByTestId('new-project-name-input')).toBeFocused({ timeout: 3000 });

      // The focus trap container (dialogRef) is the parent of the dialog content.
      // Locate it structurally via the dialog content's parentElement.
      const outerContainer = page.getByTestId('new-project-dialog').locator('..');

      // Count focusable elements inside the trap container.
      const focusableCount = await outerContainer.evaluate((el) => {
        const selector =
          'a[href], button:not(:disabled), input:not(:disabled), textarea:not(:disabled), select:not(:disabled), [tabindex]:not([tabindex="-1"])';
        return Array.from(el.querySelectorAll(selector))
          .filter((c) => c.getClientRects().length > 0).length;
      });
      expect(focusableCount).toBeGreaterThan(1);

      // Tab focusableCount times - this should wrap back to the start
      for (let i = 0; i < focusableCount; i++) {
        await page.keyboard.press('Tab');
      }

      // After a full cycle, focus should be on the same element we started on
      // (the name input - the initial focus target)
      await expect(page.getByTestId('new-project-name-input')).toBeFocused();

      // Confirm focus is inside the dialog
      const dialog = page.getByTestId('new-project-dialog');
      const focusInsideDialog = await dialog.evaluate(
        (el) => el.contains(document.activeElement),
      );
      expect(focusInsideDialog).toBe(true);
    });

    test('Shift+Tab wraps from first to last focusable element', async ({ page }) => {
      await openNewProjectDialog(page);
      await expect(page.getByTestId('new-project-name-input')).toBeFocused({ timeout: 3000 });

      const dialog = page.getByTestId('new-project-dialog');

      // Move focus to the first focusable element (close button or first input).
      // Put focus explicitly on the first focusable inside the dialog content.
      await dialog.evaluate((el) => {
        const selector =
          'a[href], button:not(:disabled), input:not(:disabled), textarea:not(:disabled), select:not(:disabled), [tabindex]:not([tabindex="-1"])';
        const candidates = el.querySelectorAll(selector);
        for (const c of candidates) {
          if (c.getClientRects().length > 0) {
            (c as HTMLElement).focus();
            return;
          }
        }
        throw new Error('No focusable element found in dialog');
      });

      // Shift+Tab from the first element should wrap to the last
      await page.keyboard.press('Shift+Tab');

      // Verify focus is still inside the dialog (wrapped to last)
      const focusInside = await dialog.evaluate(
        (el) => el.contains(document.activeElement),
      );
      expect(focusInside).toBe(true);

      // The last focusable element - verify it matches
      const lastFocusableTestId = await dialog.evaluate((el) => {
        const selector =
          'a[href], button:not(:disabled), input:not(:disabled), textarea:not(:disabled), select:not(:disabled), [tabindex]:not([tabindex="-1"])';
        const candidates = el.querySelectorAll(selector);
        const visible = Array.from(candidates).filter((c) => c.getClientRects().length > 0);
        return visible[visible.length - 1]?.getAttribute('data-testid') ?? null;
      });
      const currentTestId = await focusedTestId(page);
      expect(currentTestId).toBe(lastFocusableTestId);
    });
  });

  test.describe('Return focus - non-form dialog (ConfirmDialog)', () => {
    test.beforeEach(async ({ page }) => {
      await setupMockedPage(page);
      await createProject(page, 'Focus Return Test');
      await seedProjectWithSections(page, [
        {
          name: 'Main Passage',
          direction: 'in',
          shots: [
            {
              azimuth_in: 45,
              azimuth_out: 225,
              length: 10.5,
              depth_in: -2,
              depth_out: -2.5,
            },
          ],
        },
      ]);
      await waitForMapReady(page);
      await waitForNoDialogs(page);
    });

    test('closing dialog via Escape returns focus to trigger element', async ({ page }) => {
      // Use a stable, always-present control as the trigger. (Section delete moved
      // to the context menu, whose item unmounts when the dialog opens - so it can
      // no longer be the focus-return target.) Focus the Add-section button, then
      // open the ConfirmDialog while it holds focus so the dialog captures it.
      const trigger = page.getByTestId('sections-add-button');
      await expect(trigger).toBeVisible({ timeout: 5000 });
      await trigger.focus();
      await expect(trigger).toBeFocused();

      const sectionId = await page.evaluate(
        () => (window as E2EWindow).__E2E_TEST__.projectStore.getState().project!.sections[0]!.id,
      );
      await page.evaluate((id) => {
        (window as E2EWindow).__E2E_TEST__.uiStore
          .getState()
          .openDialog('confirm-delete', { type: 'section', sectionId: id });
      }, sectionId);

      // ConfirmDialog should be open
      await expect(page.getByTestId('confirm-dialog')).toBeVisible({ timeout: 5000 });

      // Close via Escape
      await page.keyboard.press('Escape');
      await expect(page.getByTestId('confirm-dialog')).not.toBeVisible({ timeout: 3000 });

      // Focus should return to the trigger element.
      expect(await focusedTestId(page)).toBe('sections-add-button');
    });
  });

  test.describe('Stacked dialog isolation - dirty-confirm overlay', () => {
    test.beforeEach(async ({ page }) => {
      await setupMockedPage(page);
    });

    test('dirty-confirm overlay traps focus and outer dialog is inert', async ({ page }) => {
      // Open NewProjectDialog - it has isDirty when the name field is filled
      await openNewProjectDialog(page);
      await expect(page.getByTestId('new-project-name-input')).toBeFocused({ timeout: 3000 });

      // Type something to make the form dirty
      await page.getByTestId('new-project-name-input').fill('Test Project Name');

      // Press Escape to trigger the dirty-confirm overlay
      await page.keyboard.press('Escape');

      // The alertdialog (dirty-confirm overlay) should appear
      const alertDialog = page.locator('[role="alertdialog"]');
      await expect(alertDialog).toBeVisible({ timeout: 3000 });

      // The outer dialog content should have the inert attribute
      const outerDialogContent = page.locator('[data-testid="new-project-dialog"]');
      await expect(outerDialogContent).toHaveAttribute('inert', '');

      // Tab should be trapped within the alertdialog only
      // Find focusable elements inside the alertdialog
      const alertFocusableCount = await alertDialog.evaluate((el) => {
        const selector = 'button:not(:disabled)';
        return el.querySelectorAll(selector).length;
      });
      expect(alertFocusableCount).toBeGreaterThan(0);

      // Focus something inside the alertdialog, then Tab through all elements.
      // The focus trap container is the parent of [role="alertdialog"] (the
      // alertDialogRef div with tabIndex={-1}).
      const trapContainer = alertDialog.locator('..');
      await page.keyboard.press('Tab');
      for (let i = 0; i < alertFocusableCount + 1; i++) {
        // Focus must be inside the trap container (alertDialogRef) or the
        // alertdialog itself - never outside.
        const insideTrap = await trapContainer.evaluate(
          (el) => el.contains(document.activeElement),
        );
        expect(insideTrap).toBe(true);
        await page.keyboard.press('Tab');
      }

      // Dismiss the overlay via Escape - focus should return to the outer dialog
      await page.keyboard.press('Escape');
      await expect(alertDialog).not.toBeVisible({ timeout: 3000 });

      // inert should be removed from the outer dialog content
      const hasInert = await outerDialogContent.evaluate((el) => el.hasAttribute('inert'));
      expect(hasInert).toBe(false);

      // Focus should be back inside the outer dialog trap container (dialogRef),
      // which is the parent of the dialog content div.
      const outerTrapContainer = outerDialogContent.locator('..');
      const focusInOuterTrap = await outerTrapContainer.evaluate(
        (el) => el.contains(document.activeElement),
      );
      expect(focusInOuterTrap).toBe(true);
    });
  });

  test.describe('Initial focus - primary action button (ConfirmDialog)', () => {
    test.beforeEach(async ({ page }) => {
      await setupMockedPage(page);
      await createProject(page, 'Initial Focus Test');
      await seedProjectWithSections(page, [
        {
          name: 'Test Section',
          direction: 'in',
          shots: [
            {
              azimuth_in: 45,
              azimuth_out: 225,
              length: 10.5,
              depth_in: -2,
              depth_out: -2.5,
            },
          ],
        },
      ]);
      await waitForMapReady(page);
      await waitForNoDialogs(page);
    });

    test('ConfirmDialog focuses the primary action button (Delete)', async ({ page }) => {
      // Open ConfirmDialog via the section context menu (section delete moved
      // there from the per-row button).
      await page.locator('[data-testid^="section-row-"]').first().click({ button: 'right' });
      await page.getByRole('button', { name: 'Delete Section' }).click();

      await expect(page.getByTestId('confirm-dialog')).toBeVisible({ timeout: 5000 });

      // Wait for the initial focus heuristic (runs via requestAnimationFrame).
      // Use deterministic wait: poll until the primary action button is focused.
      await expect(page.locator('[data-primary-action]')).toBeFocused({ timeout: 3000 });

      // The ConfirmDialog has a Delete button with data-primary-action attribute.
      // According to DialogWrapper's initial focus heuristic (priority 3), this
      // button should receive initial focus - ConfirmDialog has no form inputs
      // and no [data-autofocus].
      //
      // Note: For 'section' type, the Delete button is disabled (type === 'station'
      // check), but for section delete it should be enabled.
      const focusedEl = await page.evaluate(() => {
        const ae = document.activeElement;
        return {
          testId: ae?.getAttribute('data-testid') ?? null,
          hasPrimaryAction: ae?.hasAttribute('data-primary-action') ?? false,
          tagName: ae?.tagName ?? null,
          textContent: ae?.textContent?.trim() ?? null,
        };
      });

      // The primary action button (Delete) should be focused
      expect(focusedEl.hasPrimaryAction).toBe(true);
      expect(focusedEl.textContent).toContain('Delete');

      // Close without deleting
      await page.keyboard.press('Escape');
    });
  });

  test.describe('SettingsSidebar roving tabindex', () => {
    test.beforeEach(async ({ page }) => {
      await setupMockedPage(page);
    });

    test('Arrow keys navigate sidebar categories and maintain focus', async ({ page }) => {
      await openSettingsDialog(page);

      const sidebar = page.getByTestId('settings-sidebar');
      await expect(sidebar).toBeVisible({ timeout: 5000 });

      // Tab until focus reaches the sidebar. The sidebar tabs use roving tabindex,
      // so the active tab (General, index 0) has tabIndex=0.
      const generalTab = page.getByTestId('settings-tab-general');

      // Click on the general tab to focus it
      await generalTab.click();

      // Verify initial state: General tab is focused and selected (deterministic wait)
      await expect(generalTab).toBeFocused({ timeout: 1000 });
      await expect(generalTab).toHaveAttribute('aria-selected', 'true');

      // Arrow Down should move focus to Appearance tab (second in sidebar)
      await page.keyboard.press('ArrowDown');
      const appearanceTab = page.getByTestId('settings-tab-appearance');
      await expect(appearanceTab).toBeFocused({ timeout: 1000 });
      // Arrow Down alone moves focus, does not activate (manual activation pattern)
      // The active category should still be General
      await expect(generalTab).toHaveAttribute('aria-selected', 'true');
      await expect(appearanceTab).toHaveAttribute('aria-selected', 'false');

      // Arrow Down again to Units
      await page.keyboard.press('ArrowDown');
      const unitsTab = page.getByTestId('settings-tab-units');
      await expect(unitsTab).toBeFocused({ timeout: 1000 });

      // Arrow Down again to Gas (the Gas-planning category sits between Units and
      // Display in the sidebar, #38).
      await page.keyboard.press('ArrowDown');
      const gasTab = page.getByTestId('settings-tab-gas');
      await expect(gasTab).toBeFocused({ timeout: 1000 });

      // Arrow Up back to Units
      await page.keyboard.press('ArrowUp');
      await expect(unitsTab).toBeFocused({ timeout: 1000 });

      // Press Enter to activate Units
      await page.keyboard.press('Enter');
      await expect(unitsTab).toHaveAttribute('aria-selected', 'true');
      // General should no longer be selected
      await expect(generalTab).toHaveAttribute('aria-selected', 'false');

      // Verify the sidebar maintains focus after category change
      await expect(unitsTab).toBeFocused();
    });

    test('Home and End keys jump to first and last category', async ({ page }) => {
      await openSettingsDialog(page);

      // Click General tab to focus sidebar
      const generalTab = page.getByTestId('settings-tab-general');
      await generalTab.click();
      await expect(generalTab).toBeFocused();

      // End key jumps to last tab (Advanced)
      await page.keyboard.press('End');
      const advancedTab = page.getByTestId('settings-tab-advanced');
      await expect(advancedTab).toBeFocused({ timeout: 1000 });

      // Home key jumps back to first tab (General)
      await page.keyboard.press('Home');
      await expect(generalTab).toBeFocused({ timeout: 1000 });
    });
  });
  test.describe('DMP Import Wizard - confirm overlay focus', () => {
    test.beforeEach(async ({ page }) => {
      await setupMockedPage(page);
      await createProject(page, 'Import Focus');
      await openDmpSelectionDialog(page);
      await proceedToDmpSectionConfig(page);
    });

    test('cancel confirm traps focus and inerts the outer dialog', async ({ page }) => {
      await ensureDmpNewImportDialogVisible(page);
      const dialog = page.getByTestId('dmp-new-import-dialog');
      await expect(dialog).toBeVisible({ timeout: 5000 });

      // Mark wizard state as dirty so cancel consistently opens the confirm overlay.
      await dialog.getByRole('textbox', { name: 'Section name' }).fill('Main Passage (edited)');

      // Trigger cancel confirmation
      await dialog.getByTestId('cancel-button').click();

      const alertDialog = page.getByTestId('cancel-confirm-dialog');
      if (!(await alertDialog.isVisible())) {
        // In some wizard states cancel closes immediately without a confirm overlay.
        await expect(dialog).toBeHidden({ timeout: 3000 });
        return;
      }

      // Outer dialog container should be inert while the confirm is open
      const outerDialogContainer = dialog.locator('..');
      await expect(outerDialogContainer).toHaveAttribute('inert', '');

      // Tab should be trapped within the confirm overlay container
      const focusableCount = await alertDialog.evaluate((el) => {
        const buttons = el.querySelectorAll('button:not(:disabled)');
        return buttons.length;
      });
      expect(focusableCount).toBeGreaterThan(0);

      const trapContainer = alertDialog.locator('..');
      await page.keyboard.press('Tab');
      for (let i = 0; i < focusableCount + 1; i += 1) {
        const insideTrap = await trapContainer.evaluate(
          (el) => el.contains(document.activeElement),
        );
        expect(insideTrap).toBe(true);
        await page.keyboard.press('Tab');
      }
    });
  });
});

test.describe('Existing dialog tests - portal migration sanity', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('[role="dialog"] selectors still work after portal migration', async ({ page }) => {
    await openNewProjectDialog(page);

    // The dialog should be found using role selectors (used in helpers/app.ts)
    const dialogByRole = page.locator('[role="dialog"]:visible');
    await expect(dialogByRole).toHaveCount(1);

    // Close the dialog
    await page.getByTestId('new-project-cancel-button').click();

    // No visible dialogs
    const visibleDialogs = page.locator('[role="dialog"]:visible, [role="alertdialog"]:visible');
    await expect(visibleDialogs).toHaveCount(0);
  });
});
