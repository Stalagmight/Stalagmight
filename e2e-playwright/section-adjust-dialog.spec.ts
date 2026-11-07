import { test, expect } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapReady,
  waitForNoDialogs,
} from './helpers/app';
import type { CommandError } from '../src/types/ipc/CommandError';

test.describe('Section Adjust Dialog', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);

    // Create a project
    await createProject(page, 'Section Adjust Test');

    await seedProjectWithSections(page, [
      {
        name: 'Main Passage',
        direction: 'in',
        shots: [
          { azimuth_in: 45, azimuth_out: 225, length: 10.5, depth_in: -2, depth_out: -2.5, left: 1.5, right: 2.3, up: 3.0, down: 0.8 },
          { azimuth_in: 90, azimuth_out: 270, length: 15.2, depth_in: -2.5, depth_out: -5.0, left: 0.5, right: 0.7, up: 1.2, down: 1.5 },
        ],
      },
      {
        name: 'Side Tunnel',
        direction: 'out',
        shots: [
          { azimuth_in: 270, azimuth_out: 90, length: 5.0, depth_in: -3.0, depth_out: -4.0, left: 0, right: 0, up: 0, down: 0 },
        ],
      },
    ]);

    await waitForMapReady(page);
    await waitForNoDialogs(page);

    await expect(page.getByTestId('sections-panel')).toBeVisible({ timeout: 5000 });
  });

  test('should open section adjust dialog from context menu', async ({ page }) => {
    // Wait for the sections panel to be visible
    await expect(page.getByTestId('sections-panel')).toBeVisible({ timeout: 5000 });

    // Find a section item in the sections panel
    const sectionItem = page.locator('[data-testid^="section-item-"]').filter({ hasText: /Section|Main Passage|Side Tunnel/ }).first();
    await expect(sectionItem).toBeVisible({ timeout: 5000 });

    // Right-click on section to open context menu
    await sectionItem.click({ button: 'right' });

    // Check that "Adjust Position..." menu item is visible
    const adjustMenuItem = page.locator('button:has-text("Adjust Position...")');
    await expect(adjustMenuItem).toBeVisible({ timeout: 3000 });

    // Click on "Adjust Position..."
    await adjustMenuItem.click();

    // Verify dialog opens
    const dialog = page.locator('[data-testid="section-adjust-dialog"]');
    await expect(dialog).toBeVisible({ timeout: 3000 });
  });

  test('should have dx and dy input fields', async ({ page }) => {
    // Open the dialog via context menu
    const sectionItem = page.locator('[data-testid^="section-item-"]').filter({ hasText: /Section|Main Passage|Side Tunnel/ }).first();
    await sectionItem.click({ button: 'right' });
    await page.click('button:has-text("Adjust Position...")');

    // Verify dialog opens
    const dialog = page.locator('[data-testid="section-adjust-dialog"]');
    await expect(dialog).toBeVisible({ timeout: 3000 });

    // Check for dx and dy inputs
    const dxInput = page.locator('[data-testid="dx-input"]');
    const dyInput = page.locator('[data-testid="dy-input"]');

    await expect(dxInput).toBeVisible();
    await expect(dyInput).toBeVisible();

    // Fields start empty (placeholder shows "0") so a leading '-' isn't rejected
    // as '0-' - see the negative-input regression test below (#161).
    await expect(dxInput).toHaveValue('');
    await expect(dyInput).toHaveValue('');
    await expect(dxInput).toHaveAttribute('placeholder', '0');
  });

  test('should accept numeric input for dx and dy', async ({ page }) => {
    // Open the dialog
    const sectionItem = page.locator('[data-testid^="section-item-"]').filter({ hasText: /Section|Main Passage|Side Tunnel/ }).first();
    await sectionItem.click({ button: 'right' });
    await page.click('button:has-text("Adjust Position...")');

    const dialog = page.locator('[data-testid="section-adjust-dialog"]');
    await expect(dialog).toBeVisible({ timeout: 3000 });

    // Enter values in dx and dy
    const dxInput = page.locator('[data-testid="dx-input"]');
    const dyInput = page.locator('[data-testid="dy-input"]');

    await dxInput.fill('5.5');
    await dyInput.fill('-3.2');

    // Verify values
    await expect(dxInput).toHaveValue('5.5');
    await expect(dyInput).toHaveValue('-3.2');
  });

  test('should close dialog on Cancel', async ({ page }) => {
    // Open the dialog
    const sectionItem = page.locator('[data-testid^="section-item-"]').filter({ hasText: /Section|Main Passage|Side Tunnel/ }).first();
    await sectionItem.click({ button: 'right' });
    await page.click('button:has-text("Adjust Position...")');

    const dialog = page.locator('[data-testid="section-adjust-dialog"]');
    await expect(dialog).toBeVisible({ timeout: 3000 });

    // Click Cancel
    await page.click('button:has-text("Cancel")');

    // Verify dialog closes
    await expect(dialog).not.toBeVisible({ timeout: 3000 });
  });

  test('should close dialog on Apply', async ({ page }) => {
    // Open the dialog
    const sectionItem = page.locator('[data-testid^="section-item-"]').filter({ hasText: /Section|Main Passage|Side Tunnel/ }).first();
    await sectionItem.click({ button: 'right' });
    await page.click('button:has-text("Adjust Position...")');

    const dialog = page.locator('[data-testid="section-adjust-dialog"]');
    await expect(dialog).toBeVisible({ timeout: 3000 });

    // Enter offset values
    const dxInput = page.locator('[data-testid="dx-input"]');
    await dxInput.fill('10');

    // Click Apply
    await page.click('button:has-text("Apply")');

    // Verify dialog closes
    await expect(dialog).not.toBeVisible({ timeout: 3000 });
  });

  test('should display section name in dialog', async ({ page }) => {
    // Open the dialog
    const sectionItem = page.locator('[data-testid^="section-item-"]').filter({ hasText: /Section|Main Passage|Side Tunnel/ }).first();
    await sectionItem.click({ button: 'right' });
    await page.click('button:has-text("Adjust Position...")');

    const dialog = page.locator('[data-testid="section-adjust-dialog"]');
    await expect(dialog).toBeVisible({ timeout: 3000 });

    // Check that the dialog shows the section being adjusted
    const adjustingText = dialog.locator('text=Adjusting:');
    await expect(adjustingText).toBeVisible();
  });

  test('should close dialog on Escape key', async ({ page }) => {
    // Open the dialog
    const sectionItem = page.locator('[data-testid^="section-item-"]').filter({ hasText: /Section|Main Passage|Side Tunnel/ }).first();
    await sectionItem.click({ button: 'right' });
    await page.click('button:has-text("Adjust Position...")');

    const dialog = page.locator('[data-testid="section-adjust-dialog"]');
    await expect(dialog).toBeVisible({ timeout: 3000 });

    // Press Escape
    await page.keyboard.press('Escape');

    // Verify dialog closes
    await expect(dialog).not.toBeVisible({ timeout: 3000 });
  });

  test('should have Apply and Cancel buttons', async ({ page }) => {
    // Open the dialog
    const sectionItem = page.locator('[data-testid^="section-item-"]').filter({ hasText: /Section|Main Passage|Side Tunnel/ }).first();
    await sectionItem.click({ button: 'right' });
    await page.click('button:has-text("Adjust Position...")');

    const dialog = page.locator('[data-testid="section-adjust-dialog"]');
    await expect(dialog).toBeVisible({ timeout: 3000 });

    // Check for Apply and Cancel buttons
    const applyButton = dialog.locator('button:has-text("Apply")');
    const cancelButton = dialog.locator('button:has-text("Cancel")');

    await expect(applyButton).toBeVisible();
    await expect(cancelButton).toBeVisible();
  });

  test('applies preview when values change and reverts on cancel', async ({ page }) => {
    const sectionItem = page.locator('[data-testid^="section-item-"]').filter({ hasText: /Section|Main Passage|Side Tunnel/ }).first();
    await sectionItem.click({ button: 'right' });
    await page.click('button:has-text("Adjust Position...")');

    const dialog = page.locator('[data-testid="section-adjust-dialog"]');
    await expect(dialog).toBeVisible({ timeout: 3000 });

    const dxInput = dialog.locator('[data-testid="dx-input"]');
    await dxInput.fill('2');

    await expect(dialog.locator('text=Preview applied. Press Apply to confirm or Cancel to revert.')).toBeVisible({ timeout: 3000 });

    await dialog.getByRole('button', { name: 'Cancel' }).click();
    await expect(dialog).not.toBeVisible({ timeout: 3000 });
  });

  test('shows section not found when opened with invalid section id', async ({ page }) => {
    await page.evaluate(() => {
      (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { openDialog: (name: string, data: Record<string, unknown>) => void } } } }).__E2E_TEST__?.uiStore.getState().openDialog('section-adjust', { sectionId: 999999 });
    });

    const dialog = page.locator('[data-testid="section-adjust-dialog"]');
    await expect(dialog).toBeVisible({ timeout: 3000 });
    await expect(dialog.locator('text=Section not found')).toBeVisible();

    await page.evaluate(() => {
      (window as unknown as { __E2E_TEST__?: { uiStore: { getState: () => { closeDialog: () => void } } } }).__E2E_TEST__?.uiStore.getState().closeDialog();
    });
  });

  test('shows error when apply fails without preview', async ({ page }) => {
    await page.evaluate(() => {
      (window as unknown as Record<string, unknown>).__MOCK_IPC_OVERRIDES__ = {
        move_section: { __reject: { code: 'FAILED', message: 'Move failed' } satisfies CommandError },
      };
    });

    const sectionItem = page.locator('[data-testid^="section-item-"]').filter({ hasText: /Section|Main Passage|Side Tunnel/ }).first();
    await sectionItem.click({ button: 'right' });
    await page.click('button:has-text("Adjust Position...")');

    const dialog = page.locator('[data-testid="section-adjust-dialog"]');
    await expect(dialog).toBeVisible({ timeout: 3000 });

    const dxInput = dialog.locator('[data-testid="dx-input"]');
    await dxInput.fill('3');

    await dialog.getByRole('button', { name: 'Apply' }).click();
    await expect(dialog.locator('text=Move failed')).toBeVisible();
  });

  test('adjusts a section in the -X direction via typed negative input (#161)', async ({ page }) => {
    const sectionItem = page.locator('[data-testid^="section-item-"]').filter({ hasText: /Section|Main Passage|Side Tunnel/ }).first();
    await sectionItem.click({ button: 'right' });
    await page.click('button:has-text("Adjust Position...")');

    const dialog = page.locator('[data-testid="section-adjust-dialog"]');
    await expect(dialog).toBeVisible({ timeout: 3000 });

    // Type the negative value KEY BY KEY onto the empty field - no clear/select.
    // Unlike .fill() (used by the test above, which sets the value atomically and
    // passed even while the bug was live), this exercises the intermediate '-'
    // state the old numeric control snapped back to 0 - the actual #161 failure.
    const dxInput = dialog.locator('[data-testid="dx-input"]');
    await dxInput.click();
    await dxInput.pressSequentially('-4');
    await expect(dxInput).toHaveValue('-4');

    // Commit, then confirm the NET offset that reached the backend is -4. The
    // live preview applies the move incrementally and Apply commits the residual,
    // so we sum every move_section delta rather than match a single call (a lone
    // "some call == -4" would be satisfied by the preview alone).
    await dialog.getByRole('button', { name: 'Apply' }).click();
    await expect(dialog).not.toBeVisible({ timeout: 3000 });

    await expect
      .poll(async () =>
        page.evaluate(() => {
          const calls =
            (window as unknown as { __MOCK_MOVE_CALLS__?: Array<{ command: string; args?: { offset?: { x?: number } } }> })
              .__MOCK_MOVE_CALLS__ ?? [];
          return calls
            .filter((c) => c.command === 'move_section')
            .reduce((sum, c) => sum + (c.args?.offset?.x ?? 0), 0);
        }),
      )
      .toBe(-4);
  });

  test('applies delta when preview already applied', async ({ page }) => {
    const sectionItem = page.locator('[data-testid^="section-item-"]').filter({ hasText: /Section|Main Passage|Side Tunnel/ }).first();
    await sectionItem.click({ button: 'right' });
    await page.click('button:has-text("Adjust Position...")');

    const dialog = page.locator('[data-testid="section-adjust-dialog"]');
    await expect(dialog).toBeVisible({ timeout: 3000 });

    const dxInput = dialog.locator('[data-testid="dx-input"]');
    await dxInput.fill('2');
    await expect(dialog.locator('text=Preview applied')).toBeVisible({ timeout: 3000 });

    await dxInput.fill('3');
    await dialog.getByRole('button', { name: 'Apply' }).click();
    await expect(dialog).not.toBeVisible({ timeout: 3000 });
  });
});
