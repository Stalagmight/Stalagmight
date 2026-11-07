import { test, expect, Page } from './fixtures/coverage';
import { setupMockedPage } from './helpers/app';
import { recoveryBackupItems } from './helpers/testIds';
import type { E2EWindow } from './helpers/e2eWindow';

/**
 * E2E tests for the Corruption Recovery Dialog.
 *
 * Tests the recovery flow when a project file fails integrity checks:
 * - Dialog renders with three recovery options
 * - "Attempt Repair" triggers repair IPC and shows result
 * - "Restore from Backup" lists backups and allows selection
 * - "Export Salvageable Data" triggers salvage export
 * - Cancel closes the dialog
 * - Error states in each recovery path
 */

async function openRecoveryDialog(page: Page, errorMessage?: string): Promise<void> {
  await page.evaluate(
    ({ errorMsg }) => {
      const e2e = (window as E2EWindow).__E2E_TEST__;
      if (!e2e) return;
      e2e.uiStore.getState().openDialog('corruption-recovery', {
        corruptPath: '/mock/corrupt-project.stalx',
        errorMessage: errorMsg ?? 'Database integrity check failed: index corruption detected.',
      });
    },
    { errorMsg: errorMessage },
  );

  await expect(page.getByTestId('corruption-recovery-dialog')).toBeVisible({ timeout: 5000 });
}

function setupRecoveryMocks(page: Page): Promise<void> {
  return page.evaluate(() => {
    const overrides = (window as E2EWindow).__MOCK_IPC_OVERRIDES__ ?? {};

    overrides.attempt_repair = {
      success: true,
      backupPath: '/mock/corrupt-project.stalx.bak.1',
      message: 'Repair complete. REINDEX and VACUUM applied successfully.',
    };

    overrides.list_backups = [
      {
        path: '/mock/corrupt-project.stalx.bak.1',
        modifiedAt: '2026-02-05 14:30:00',
        sizeBytes: 51200,
        // #576 C15: what the backend now says about each candidate.
        kind: 'backup',
        restoreNumber: null,
        usable: true,
        problem: null,
      },
      {
        path: '/mock/corrupt-project.stalx.bak.2',
        modifiedAt: '2026-02-04 10:15:00',
        sizeBytes: 48000,
        kind: 'backup',
        restoreNumber: null,
        usable: true,
        problem: null,
      },
    ];

    overrides.restore_from_backup = 'File restored successfully from backup.';

    overrides.export_salvageable_data = {
      projectName: 'CaveA',
      projectLocation: null,
      sectionCount: 3,
      shotCount: 42,
      mergeCount: 2,
      fullProjectJson: '{"sections": []}',
      sectionsJson: '[]',
      errors: [],
    };

    (window as E2EWindow).__MOCK_IPC_OVERRIDES__ = overrides;
  });
}

function clearRecoveryMocks(page: Page): Promise<void> {
  return page.evaluate(() => {
    const overrides = (window as E2EWindow).__MOCK_IPC_OVERRIDES__ ?? {};
    delete overrides.attempt_repair;
    delete overrides.list_backups;
    delete overrides.restore_from_backup;
    delete overrides.export_salvageable_data;
    (window as E2EWindow).__MOCK_IPC_OVERRIDES__ = overrides;
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
    await expect(dialog).toContainText('integrity check failed');
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
    await expect(repairBtn).toContainText('REINDEX');
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
    await expect(dialog).toContainText('REINDEX and VACUUM');

    // Should have "Open Project" button on success
    await expect(dialog.getByRole('button', { name: /Open Project/i })).toBeVisible();
  });

  test('Restore from Backup lists available backups', async ({ page }) => {
    await openRecoveryDialog(page);

    await page.getByTestId('recovery-restore-backup').click();

    // Should show backup list
    const dialog = page.getByTestId('corruption-recovery-dialog');
    await expect(dialog).toContainText('Select a file to restore', { timeout: 5000 });

    // Should show backup items
    const backupItems = recoveryBackupItems(page);
    await expect(backupItems).toHaveCount(2);

    // Should show filenames and dates
    await expect(dialog).toContainText('.bak.1');
    await expect(dialog).toContainText('.bak.2');
  });

  test('clicking a backup triggers restore and shows success', async ({ page }) => {
    await openRecoveryDialog(page);

    await page.getByTestId('recovery-restore-backup').click();

    // Wait for backup list
    const dialog = page.getByTestId('corruption-recovery-dialog');
    await expect(dialog).toContainText('Select a file to restore', { timeout: 5000 });

    // Click first backup
    const firstBackup = recoveryBackupItems(page).first();
    await firstBackup.click();

    // Should show restore success
    await expect(dialog).toContainText('Restore Succeeded', { timeout: 5000 });
    await expect(dialog.getByRole('button', { name: /Open Project/i })).toBeVisible();
  });

  test('backup list has Back button to return to choices', async ({ page }) => {
    await openRecoveryDialog(page);

    await page.getByTestId('recovery-restore-backup').click();

    const dialog = page.getByTestId('corruption-recovery-dialog');
    await expect(dialog).toContainText('Select a file to restore', { timeout: 5000 });

    // Click Back. By test id: the candidates are labelled ("Backup", #576 C15), so
    // a /Back/i role query matches them too.
    await dialog.getByTestId('recovery-backup-back').click();

    // Should return to choice screen
    await expect(page.getByTestId('recovery-attempt-repair')).toBeVisible();
    await expect(page.getByTestId('recovery-restore-backup')).toBeVisible();
    await expect(page.getByTestId('recovery-export-salvage')).toBeVisible();
  });

  test('no backups found shows message', async ({ page }) => {
    // Override to return empty backup list
    await page.evaluate(() => {
      const overrides = (window as E2EWindow).__MOCK_IPC_OVERRIDES__ ?? {};
      overrides.list_backups = [];
      (window as E2EWindow).__MOCK_IPC_OVERRIDES__ = overrides;
    });

    await openRecoveryDialog(page);
    await page.getByTestId('recovery-restore-backup').click();

    const dialog = page.getByTestId('corruption-recovery-dialog');
    await expect(dialog).toContainText('No Backups Found', { timeout: 5000 });
  });

  test('repair failure shows error and Try Another Option button', async ({ page }) => {
    // Override to return failure
    await page.evaluate(() => {
      const overrides = (window as E2EWindow).__MOCK_IPC_OVERRIDES__ ?? {};
      overrides.attempt_repair = {
        success: false,
        backupPath: '/mock/corrupt-project.stalx.bak.1',
        message: 'Database corruption too severe for automatic repair.',
      };
      (window as E2EWindow).__MOCK_IPC_OVERRIDES__ = overrides;
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
      const overrides = (window as E2EWindow).__MOCK_IPC_OVERRIDES__ ?? {};
      overrides.attempt_repair = {
        success: false,
        backupPath: '/mock/corrupt-project.stalx.bak.1',
        message: 'Repair failed.',
      };
      (window as E2EWindow).__MOCK_IPC_OVERRIDES__ = overrides;
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
      const overrides = (window as E2EWindow).__MOCK_IPC_OVERRIDES__ ?? {};
      overrides.attempt_repair = { __throw: 'Connection lost during repair' };
      (window as E2EWindow).__MOCK_IPC_OVERRIDES__ = overrides;
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
