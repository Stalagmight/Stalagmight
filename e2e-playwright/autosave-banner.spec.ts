import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage, createProject, createSectionViaStore } from './helpers/app';
import { setIpcOverrides, clearIpcOverrideKeys } from './helpers/ipcOverrides';
import type { CommandError } from '../src/types/ipc/CommandError';

/**
 * E2E for the autosave-failure banner (docs/notifications.md: a failing
 * background autosave is an ongoing condition - banner-class, not a modal and
 * not a toast). The debounced autosave hook is just `saveProject(path?, true)`,
 * so the specs drive that store call directly and assert the surface.
 */

/**
 * PR #460 gates the banner on dirtiness (`saveProject`'s `isAutosave &&
 * isDirty` gate in src/store/projectStore.ts), so every autosave-failure test
 * needs a real edit first - a freshly created project is clean and the banner
 * correctly stays hidden. Note: dirtying the project arms the real debounced
 * autosave (30s in the mock config); each test finishes well inside that, so
 * the timer never fires mid-test.
 */
async function makeDirty(page: Page): Promise<void> {
  await createSectionViaStore(page, {
    name: 'Autosave Dirty Section',
    shots: [{ azimuth_in: 0, azimuth_out: 0, length: 5, depth_in: 0, depth_out: 0 }],
  });
  // The edit must have landed as dirtiness, or the failure tests would fail
  // for the wrong reason (banner missing because the project stayed clean).
  await page.waitForFunction(
    () =>
      (window as unknown as {
        __E2E_TEST__: { projectStore: { getState: () => { isDirty: boolean } } };
      }).__E2E_TEST__.projectStore.getState().isDirty === true,
  );
}

async function runAutosave(page: Page): Promise<void> {
  await page.evaluate(async () => {
    await (window as unknown as {
      __E2E_TEST__: {
        projectStore: {
          getState: () => { saveProject: (path?: string, isAutosave?: boolean) => Promise<unknown> };
        };
      };
    }).__E2E_TEST__.projectStore.getState().saveProject(undefined, true);
  });
}

test.describe('Autosave-failure banner', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Autosave Banner Test');
  });

  test('a failing autosave raises the banner; a manual save is not required to see it', async ({ page }) => {
    await setIpcOverrides(page, {
      save_project: { __reject: { code: 'FAILED', message: 'Disk full: no space left on device' } satisfies CommandError },
    });
    await makeDirty(page);
    await runAutosave(page);

    const banner = page.getByTestId('autosave-banner');
    await expect(banner).toBeVisible();
    await expect(banner).toContainText('Autosave is failing');
    await expect(banner).toContainText('Disk full');
    // Banner-class, not modal: no error dialog for a background failure.
    await expect(page.getByTestId('error-dialog')).toHaveCount(0);
  });

  test('Dismiss hides the banner until the next failing autosave', async ({ page }) => {
    await setIpcOverrides(page, {
      save_project: { __reject: { code: 'FAILED', message: 'Disk full: no space left on device' } satisfies CommandError },
    });
    await makeDirty(page);
    await runAutosave(page);
    await expect(page.getByTestId('autosave-banner')).toBeVisible();

    await page.getByTestId('autosave-dismiss-button').click();
    await expect(page.getByTestId('autosave-banner')).toHaveCount(0);

    await runAutosave(page);
    await expect(page.getByTestId('autosave-banner')).toBeVisible();
  });

  test('Save now retries; a successful save clears the banner', async ({ page }) => {
    await setIpcOverrides(page, {
      save_project: { __reject: { code: 'FAILED', message: 'Disk full: no space left on device' } satisfies CommandError },
    });
    await makeDirty(page);
    await runAutosave(page);
    await expect(page.getByTestId('autosave-banner')).toBeVisible();

    // Space freed up: the retry succeeds and the condition clears.
    await clearIpcOverrideKeys(page, ['save_project']);
    await page.getByTestId('autosave-save-button').click();
    await expect(page.getByTestId('autosave-banner')).toHaveCount(0);
  });

  test('the raw FS_PERMISSION_DENIED marker never renders; the friendly line does', async ({ page }) => {
    await setIpcOverrides(page, {
      save_project: { __reject: { code: 'FS_PERMISSION_DENIED', message: 'operation not permitted' } satisfies CommandError },
    });
    await makeDirty(page);
    await runAutosave(page);

    const banner = page.getByTestId('autosave-banner');
    await expect(banner).toBeVisible();
    await expect(banner).toContainText('Folder access denied');
    await expect(banner).not.toContainText('FS_PERMISSION_DENIED');
    // The save-permission dialog stays user-initiated-only - not on autosave.
    await expect(page.getByTestId('save-permission-dialog')).toHaveCount(0);
  });

  test('a failing autosave on a clean project does not raise the banner (#429)', async ({ page }) => {
    await setIpcOverrides(page, {
      save_project: { __reject: { code: 'FAILED', message: 'Disk full: no space left on device' } satisfies CommandError },
    });
    // Project is still clean here: no edit was made after createProject.
    await runAutosave(page);

    // Store-level check first: runAutosave awaited saveProject, so the state
    // is settled - a null autosaveFailure proves the gate held, independent of
    // React flush timing on the DOM assertion below.
    const failure = await page.evaluate(
      () =>
        (window as unknown as {
          __E2E_TEST__: {
            projectStore: { getState: () => { autosaveFailure: string | null } };
          };
        }).__E2E_TEST__.projectStore.getState().autosaveFailure,
    );
    expect(failure).toBeNull();
    await expect(page.getByTestId('autosave-banner')).toHaveCount(0);
  });
});
