import { test, expect } from './fixtures/coverage';
import { setupMockedPage } from './helpers/app';
import { recoveryBackupItems } from './helpers/testIds';

async function openRecoveryDialog(page: import('./fixtures/coverage').Page): Promise<void> {
  await page.evaluate(() => {
    const e2e = (window as unknown as {
      __E2E_TEST__?: {
        uiStore: {
          getState: () => {
            openDialog: (dialog: string, data?: Record<string, unknown>) => void;
          };
        };
      };
    }).__E2E_TEST__;

    if (!e2e?.uiStore) {
      throw new Error('E2E UI store unavailable');
    }

    e2e.uiStore.getState().openDialog('corruption-recovery', {
      corruptPath: '/mock/corrupt-project.stalx',
      errorMessage: 'Database integrity check failed: index corruption detected.',
    });
  });
}

test.describe('Recovery Integrity Workflow', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);

    await page.evaluate(() => {
      const overrides = (window as unknown as Record<string, unknown>).__MOCK_IPC_OVERRIDES__ as
        | Record<string, unknown>
        | undefined;
      const next = overrides ?? {};

      next.list_backups = [
        {
          path: '/mock/corrupt-project.stalx.bak.1',
          modifiedAt: '2026-02-11 14:30:00',
          sizeBytes: 51200,
          // #576 C15: what the backend now says about each candidate.
          kind: 'backup',
          restoreNumber: null,
          usable: true,
          problem: null,
        },
      ];
      next.restore_from_backup = 'File restored successfully from backup.';

      (window as unknown as Record<string, unknown>).__MOCK_IPC_OVERRIDES__ = next;
    });
  });

  test('restores from backup and reopens a valid project state', async ({ page }) => {
    await openRecoveryDialog(page);
    await expect(page.getByTestId('corruption-recovery-dialog')).toBeVisible({ timeout: 5000 });

    await page.getByTestId('recovery-restore-backup').click();
    await expect(recoveryBackupItems(page)).toHaveCount(1);

    await recoveryBackupItems(page).first().click();
    await expect(page.getByText('Restore Succeeded')).toBeVisible({ timeout: 5000 });

    await page.getByTestId('corruption-recovery-dialog').getByRole('button', { name: 'Open Project' }).click();
    await expect(page.getByTestId('single-system-view')).toBeVisible({ timeout: 10000 });

    const projectState = await page.evaluate(() => {
      const e2e = (window as unknown as {
        __E2E_TEST__?: {
          projectStore: {
            getState: () => {
              project: { file_path: string | null; sections: unknown[] } | null;
            };
          };
        };
      }).__E2E_TEST__;

      return {
        filePath: e2e?.projectStore.getState().project?.file_path ?? null,
        sectionCount: e2e?.projectStore.getState().project?.sections.length ?? 0,
      };
    });

    expect(projectState.filePath).toBe('/mock/corrupt-project.stalx');
    expect(projectState.sectionCount).toBeGreaterThan(0);
  });
});
