import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage } from './helpers/app';
import { recoveryBackupItems, systemCardTestId } from './helpers/testIds';

/**
 * Recent Projects: right-click a project, "Restore an earlier copy" (real-app
 * demo I-2). Each copy is listed in words with a local date and size, the raw
 * file name only in a tooltip; a click on a row only selects it, the sentence
 * under the list says what Restore does, and only Restore restores. The copy a
 * restore keeps is listed next time, so the restore can be undone.
 *
 * Uses the shared IPC mock's list_backups / restore_from_backup (no
 * overrides): every project has the original its first save kept, and a
 * restore adds `<project>.stalx.pre-restore` to the next listing.
 */

const ALPHA = '/projects/cave-alpha.stalx';
const ORIGINAL = '/mock/app-data/backups/cave-alpha.0123456789ab.20261120T101112.123456789Z.pre-v2.stalx';

async function openRestore(page: Page): Promise<void> {
  await page.getByTestId(systemCardTestId(ALPHA)).click({ button: 'right' });
  await page.getByTestId('context-menu-restore-backup').click();
  await expect(page.getByTestId('corruption-recovery-dialog')).toContainText('Restore an Earlier Copy');
  await expect(recoveryBackupItems(page).first()).toBeVisible();
}

async function restoreCalls(page: Page): Promise<Array<{ projectPath: string; backupPath: string }>> {
  return page.evaluate(() =>
    (window as unknown as { __MOCK_GET_RESTORE_CALLS__: () => Array<{ projectPath: string; backupPath: string }> })
      .__MOCK_GET_RESTORE_CALLS__(),
  );
}

test.describe('Recent Projects - Restore an earlier copy', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await expect(page.getByTestId('global-view')).toBeVisible({ timeout: 10000 });
  });

  test('lists the original in words, asks before restoring, then restores it @smoke', async ({ page }) => {
    await openRestore(page);
    const dialog = page.getByTestId('corruption-recovery-dialog');
    await expect(dialog).toContainText('Choose the copy to put back in place of cave-alpha.stalx.');

    const row = page.getByTestId('recovery-backup-row-0');
    await expect(row).toContainText('Original before the upgrade to the new format');
    await expect(row).toContainText('Nov 20, 2026');
    await expect(row).toContainText('184.3 kB');
    await expect(row).not.toContainText('pre-v2');
    await expect(row).toHaveAttribute('title', 'cave-alpha.0123456789ab.20261120T101112.123456789Z.pre-v2.stalx');

    const restore = page.getByTestId('recovery-backup-restore');
    await expect(restore).toBeDisabled();

    // One click on the row selects it and restores nothing.
    await row.click();
    await expect(recoveryBackupItems(page).first()).toBeChecked();
    await expect(page.getByTestId('recovery-backup-confirm')).toHaveText(
      'Replace cave-alpha.stalx with this copy? The current file is kept as cave-alpha.stalx.pre-restore ' +
        'and is listed here, so you can switch back.',
    );
    expect(await restoreCalls(page)).toEqual([]);

    await restore.click();
    await expect(dialog).toContainText('Restore Succeeded');
    await expect(dialog).toContainText('Restored the original from before the upgrade to the new format.');
    await expect(dialog).not.toContainText('/mock/app-data');
    expect(await restoreCalls(page)).toEqual([{ projectPath: ALPHA, backupPath: ORIGINAL }]);

    await page.getByTestId('recovery-result-close').click();
    await expect(dialog).toBeHidden();
  });

  test('Cancel closes without restoring', async ({ page }) => {
    await openRestore(page);
    await page.getByTestId('recovery-backup-row-0').click();
    await page.getByTestId('recovery-backup-cancel').click();
    await expect(page.getByTestId('corruption-recovery-dialog')).toBeHidden();
    expect(await restoreCalls(page)).toEqual([]);
  });

  test('the copy a restore kept is listed next, and restoring it swaps back', async ({ page }) => {
    await openRestore(page);
    await page.getByTestId('recovery-backup-row-0').click();
    await page.getByTestId('recovery-backup-restore').click();
    await expect(page.getByTestId('corruption-recovery-dialog')).toContainText('Restore Succeeded');
    await page.getByTestId('recovery-result-close').click();

    await openRestore(page);
    await expect(recoveryBackupItems(page)).toHaveCount(2);
    await expect(page.getByTestId('recovery-backup-row-0')).toContainText('The file as it was before the last restore');
    await expect(page.getByTestId('recovery-backup-row-1')).toContainText('Original before the upgrade to the new format');

    // Picking the original again warns that the kept copy is replaced.
    await page.getByTestId('recovery-backup-row-1').click();
    await expect(page.getByTestId('recovery-backup-confirm')).toContainText(
      'It takes the place of the copy the last restore kept',
    );

    await page.getByTestId('recovery-backup-row-0').click();
    await expect(page.getByTestId('recovery-backup-confirm')).toHaveText(
      'Put cave-alpha.stalx back as it was before the last restore? The two files swap places: the current ' +
        'file is kept as cave-alpha.stalx.pre-restore, so you can switch back.',
    );
    await page.getByTestId('recovery-backup-restore').click();
    await expect(page.getByTestId('corruption-recovery-dialog')).toContainText('swapped places');
    expect((await restoreCalls(page)).map((c) => c.backupPath)).toEqual([
      ORIGINAL,
      '/projects/cave-alpha.stalx.pre-restore',
    ]);
  });
});
