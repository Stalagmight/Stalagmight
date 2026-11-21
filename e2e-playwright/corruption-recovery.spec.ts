import { test, expect, Page } from './fixtures/coverage';
import { setupMockedPage } from './helpers/app';
import { recoveryBackupItems } from './helpers/testIds';

/**
 * E2E tests for the Corruption Recovery Dialog.
 *
 * Tests the recovery flow when a project file fails integrity checks:
 * - Dialog renders with three recovery options
 * - "Attempt Repair" triggers repair IPC and shows result
 * - "Restore from Backup" lists backups in words; a row only selects, Restore restores
 * - "Export Salvageable Data" triggers salvage export
 * - Cancel closes the dialog
 * - Error states in each recovery path
 */

async function openRecoveryDialog(page: Page, errorMessage?: string): Promise<void> {
  await page.evaluate(
    ({ errorMsg }) => {
      const e2e = (window as any).__E2E_TEST__;
      if (!e2e) return;
      e2e.uiStore.getState().openDialog('corruption-recovery', {
        corruptPath: '/mock/corrupt-project.stalx',
        errorMessage:
          errorMsg ?? 'The project breaks an invariant and cannot be opened normally: shot 3 of line Main ends at an unknown station.',
      });
    },
    { errorMsg: errorMessage },
  );

  await expect(page.getByTestId('corruption-recovery-dialog')).toBeVisible({ timeout: 5000 });
}

function setupRecoveryMocks(page: Page): Promise<void> {
  return page.evaluate(() => {
    const overrides = (window as any).__MOCK_IPC_OVERRIDES__ ?? {};

    overrides.attempt_repair = {
      success: true,
      message:
        'Repair succeeded: 1 fix(es): dropped a closure to an unknown station. The damaged original is kept as corrupt-project.stalx.backup.',
    };

    overrides.list_backups = [
      {
        path: '/mock/corrupt-project.stalx.backup.1',
        modifiedAt: '2026-02-05T14:30:00.000Z',
        sizeBytes: 51200,
        kind: 'backup',
      },
      {
        path: '/mock/app-data/backups/corrupt-project.0123456789ab.20260204T101500.123456789Z.pre-v2.stalx',
        modifiedAt: '2026-02-04T10:15:00.000Z',
        sizeBytes: 48000,
        kind: 'pre_upgrade',
      },
    ];

    overrides.restore_from_backup = 'File restored successfully from backup.';

    overrides.export_salvageable_data = {
      lineCount: 3,
      shotCount: 42,
      mergeCount: 2,
      fullProjectJson: '{"lines": []}',
      linesJson: '[]',
      errors: [],
    };

    (window as any).__MOCK_IPC_OVERRIDES__ = overrides;
  });
}

function clearRecoveryMocks(page: Page): Promise<void> {
  return page.evaluate(() => {
    const overrides = (window as any).__MOCK_IPC_OVERRIDES__ ?? {};
    delete overrides.attempt_repair;
    delete overrides.list_backups;
    delete overrides.restore_from_backup;
    delete overrides.export_salvageable_data;
    (window as any).__MOCK_IPC_OVERRIDES__ = overrides;
  });
}

test.describe('Corruption Recovery Dialog', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await setupRecoveryMocks(page);
  });

  test.afterEach(async ({ page }) => {
    await clearRecoveryMocks(page);
  });

  test('dialog renders with title and error message @smoke', async ({ page }) => {
    await openRecoveryDialog(page);

    const dialog = page.getByTestId('corruption-recovery-dialog');
    await expect(dialog).toContainText('File Corruption Detected');
    await expect(dialog).toContainText('cannot be opened normally');
  });

  test('dialog shows three recovery action buttons', async ({ page }) => {
    await openRecoveryDialog(page);

    await expect(page.getByTestId('recovery-attempt-repair')).toBeVisible();
    await expect(page.getByTestId('recovery-restore-backup')).toBeVisible();
    await expect(page.getByTestId('recovery-export-salvage')).toBeVisible();
  });

  test('Attempt Repair button has description text', async ({ page }) => {
    await openRecoveryDialog(page);

    const repairBtn = page.getByTestId('recovery-attempt-repair');
    await expect(repairBtn).toContainText('Attempt Repair');
    await expect(repairBtn).toContainText('Keeps a copy of the damaged file');
    await expect(repairBtn).not.toContainText('REINDEX');
  });

  test('Restore from Backup button has description text', async ({ page }) => {
    await openRecoveryDialog(page);

    const backupBtn = page.getByTestId('recovery-restore-backup');
    await expect(backupBtn).toContainText('Restore from Backup');
    await expect(backupBtn).toContainText('Replace the corrupt file');
  });

  test('Export Salvageable Data button has description text', async ({ page }) => {
    await openRecoveryDialog(page);

    const salvageBtn = page.getByTestId('recovery-export-salvage');
    await expect(salvageBtn).toContainText('Export Salvageable Data');
    await expect(salvageBtn).toContainText('JSON');
  });

  test('Cancel closes the dialog', async ({ page }) => {
    await openRecoveryDialog(page);

    const dialog = page.getByTestId('corruption-recovery-dialog');
    const cancelBtn = dialog.getByRole('button', { name: /Cancel/i });
    await expect(cancelBtn).toBeVisible();
    await cancelBtn.click();

    await expect(dialog).not.toBeVisible({ timeout: 3000 });
  });

  test('Attempt Repair shows success result @smoke', async ({ page }) => {
    await openRecoveryDialog(page);

    await page.getByTestId('recovery-attempt-repair').click();

    // Should show result with success message
    const dialog = page.getByTestId('corruption-recovery-dialog');
    await expect(dialog).toContainText('Repair Succeeded', { timeout: 5000 });
    await expect(dialog).toContainText('The damaged original is kept');

    // Should have "Open Project" button on success
    await expect(dialog.getByRole('button', { name: /Open Project/i })).toBeVisible();
  });

  test('Restore from Backup lists available backups', async ({ page }) => {
    await openRecoveryDialog(page);

    await page.getByTestId('recovery-restore-backup').click();

    // Should show backup list
    const dialog = page.getByTestId('corruption-recovery-dialog');
    await expect(dialog).toContainText('Choose the copy to put back', { timeout: 5000 });

    // Should show backup items
    const backupItems = recoveryBackupItems(page);
    await expect(backupItems).toHaveCount(2);

    // Each copy in words, its file name in the row's tooltip
    await expect(page.getByTestId('recovery-backup-row-0')).toContainText('Backup');
    await expect(page.getByTestId('recovery-backup-row-0')).toHaveAttribute('title', 'corrupt-project.stalx.backup.1');
    await expect(page.getByTestId('recovery-backup-row-1')).toContainText('Original before the upgrade to the new format');
    await expect(page.getByTestId('recovery-backup-row-1')).toContainText('Feb 4, 2026');
  });

  test('picking a backup asks first, and Restore restores it', async ({ page }) => {
    await openRecoveryDialog(page);

    await page.getByTestId('recovery-restore-backup').click();

    // Wait for backup list
    const dialog = page.getByTestId('corruption-recovery-dialog');
    await expect(dialog).toContainText('Choose the copy to put back', { timeout: 5000 });
    await expect(page.getByTestId('recovery-backup-restore')).toBeDisabled();

    // Click first backup: it is selected, nothing is restored yet
    const firstBackup = recoveryBackupItems(page).first();
    await firstBackup.click();
    await expect(firstBackup).toBeChecked();
    await expect(page.getByTestId('recovery-backup-confirm')).toContainText(
      'Replace the damaged corrupt-project.stalx with this copy? The damaged file is kept as corrupt-project.stalx.pre-restore',
    );
    await expect(dialog).not.toContainText('Restore Succeeded');
    await page.getByTestId('recovery-backup-restore').click();

    // Should show restore success
    await expect(dialog).toContainText('Restore Succeeded', { timeout: 5000 });
    await expect(dialog.getByRole('button', { name: /Open Project/i })).toBeVisible();
  });

  test('backup list has Back button to return to choices', async ({ page }) => {
    await openRecoveryDialog(page);

    await page.getByTestId('recovery-restore-backup').click();

    const dialog = page.getByTestId('corruption-recovery-dialog');
    await expect(dialog).toContainText('Choose the copy to put back', { timeout: 5000 });

    // Click Back
    await dialog.getByRole('button', { name: /Back/i }).click();

    // Should return to choice screen
    await expect(page.getByTestId('recovery-attempt-repair')).toBeVisible();
    await expect(page.getByTestId('recovery-restore-backup')).toBeVisible();
    await expect(page.getByTestId('recovery-export-salvage')).toBeVisible();
  });

  test('no backups found shows message', async ({ page }) => {
    // Override to return empty backup list
    await page.evaluate(() => {
      const overrides = (window as any).__MOCK_IPC_OVERRIDES__ ?? {};
      overrides.list_backups = [];
      (window as any).__MOCK_IPC_OVERRIDES__ = overrides;
    });

    await openRecoveryDialog(page);
    await page.getByTestId('recovery-restore-backup').click();

    const dialog = page.getByTestId('corruption-recovery-dialog');
    await expect(dialog).toContainText('No Backups Found', { timeout: 5000 });
  });

  test('repair failure shows error and Try Another Option button', async ({ page }) => {
    // Override to return failure
    await page.evaluate(() => {
      const overrides = (window as any).__MOCK_IPC_OVERRIDES__ ?? {};
      overrides.attempt_repair = {
        success: false,
        message:
          'Repair could not rebuild the project (too severe: no line decodes). Nothing was changed. Try Restore from Backup or Export Salvageable Data instead.',
      };
      (window as any).__MOCK_IPC_OVERRIDES__ = overrides;
    });

    await openRecoveryDialog(page);
    await page.getByTestId('recovery-attempt-repair').click();

    const dialog = page.getByTestId('corruption-recovery-dialog');
    await expect(dialog).toContainText('Repair Incomplete', { timeout: 5000 });
    await expect(dialog).toContainText('too severe');

    // Should have "Try Another Option" button
    await expect(dialog.getByRole('button', { name: /Try Another Option/i })).toBeVisible();
  });

  test('Try Another Option returns to choice screen', async ({ page }) => {
    await page.evaluate(() => {
      const overrides = (window as any).__MOCK_IPC_OVERRIDES__ ?? {};
      overrides.attempt_repair = {
        success: false,
        message: 'Repair failed.',
      };
      (window as any).__MOCK_IPC_OVERRIDES__ = overrides;
    });

    await openRecoveryDialog(page);
    await page.getByTestId('recovery-attempt-repair').click();

    const dialog = page.getByTestId('corruption-recovery-dialog');
    await expect(dialog).toContainText('Repair Incomplete', { timeout: 5000 });

    await dialog.getByRole('button', { name: /Try Another Option/i }).click();

    // Should return to choice screen
    await expect(page.getByTestId('recovery-attempt-repair')).toBeVisible();
  });

  test('repair IPC error shows error result', async ({ page }) => {
    // Override to throw
    await page.evaluate(() => {
      const overrides = (window as any).__MOCK_IPC_OVERRIDES__ ?? {};
      overrides.attempt_repair = { __throw: 'Connection lost during repair' };
      (window as any).__MOCK_IPC_OVERRIDES__ = overrides;
    });

    await openRecoveryDialog(page);
    await page.getByTestId('recovery-attempt-repair').click();

    const dialog = page.getByTestId('corruption-recovery-dialog');
    await expect(dialog).toContainText('Repair Failed', { timeout: 5000 });
    await expect(dialog).toContainText('Connection lost');
  });

  test('custom error message is displayed in dialog', async ({ page }) => {
    await openRecoveryDialog(page, 'UNIQUE constraint violated on table "sections"');

    const dialog = page.getByTestId('corruption-recovery-dialog');
    await expect(dialog).toContainText('UNIQUE constraint violated');
  });
});
