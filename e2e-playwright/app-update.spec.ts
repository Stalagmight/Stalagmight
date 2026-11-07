import { test, expect } from './fixtures/coverage';
import { setupMockedPage, createProject, createSectionViaStore, openSettingsFromMenu } from './helpers/app';
import { setIpcOverrides } from './helpers/ipcOverrides';
import { getIpcCalls, recordIpcCalls } from './helpers/ipcRecorder';

/**
 * E2E for the in-app updater (#579): the banner's three states, and the
 * restart button threading through the unsaved-changes exit guard.
 *
 * The automatic launch check fires ~10s after mount, which is too slow for a
 * unit-of-behavior test - these specs drive `updateStore` directly (the same
 * additive `__E2E_TEST__` pattern every other store-seeded spec uses), so the
 * banner/download/restart wiring is exercised without waiting on the timer.
 */

interface UpdateStoreApi {
  getState: () => {
    beginCheck: () => number;
    setAvailable: (seq: number, info: { version: string; notesUrl: string }) => void;
    startDownloading: (version: string) => void;
    setReady: (version: string) => void;
  };
}

async function getUpdateStore(page: import('@playwright/test').Page): Promise<void> {
  await page.waitForFunction(
    () => (window as unknown as { __E2E_TEST__?: { updateStore?: unknown } }).__E2E_TEST__?.updateStore != null,
  );
}

async function seedAvailable(page: import('@playwright/test').Page, version = '9.9.9'): Promise<void> {
  await getUpdateStore(page);
  await page.evaluate((v) => {
    const store = (window as unknown as { __E2E_TEST__: { updateStore: UpdateStoreApi } }).__E2E_TEST__.updateStore;
    // Through `beginCheck` rather than around it: every reply carries the
    // sequence of the check it answers, and one seeded outside that contract
    // would be dropped as stale.
    const seq = store.getState().beginCheck();
    store.getState().setAvailable(seq, { version: v, notesUrl: 'https://example.com/notes' });
  }, version);
}

/**
 * The mocked `plugin:window|close` command only counts the call - unlike the
 * real backend's `on_window_event` handler, it does not itself prevent the
 * close and re-emit `tauri://close-requested`. Every close-guard spec
 * (window-close-protection.spec.ts) simulates that re-emission explicitly;
 * `requestUpdateRestart`'s `getCurrentWindow().close()` needs the same nudge
 * before `useBeforeUnload`'s guard chain runs.
 */
async function simulateCloseRequested(page: import('@playwright/test').Page): Promise<void> {
  await page.evaluate(async () => {
    await (
      window as unknown as {
        __TAURI_INTERNALS__?: { invoke: (cmd: string, args: unknown) => Promise<unknown> };
      }
    ).__TAURI_INTERNALS__?.invoke('plugin:event|emit', {
      event: 'tauri://close-requested',
      payload: null,
    });
  });
}

/** Make the open project dirty, and put a downloaded update on the banner. */
async function dirtyProjectWithReadyUpdate(page: import('@playwright/test').Page): Promise<void> {
  await createSectionViaStore(page, {
    name: 'Update Restart Dirty Section',
    shots: [{ azimuth_in: 0, azimuth_out: 0, length: 5, depth_in: 0, depth_out: 0 }],
  });
  await page.waitForFunction(
    () =>
      (window as unknown as {
        __E2E_TEST__: { projectStore: { getState: () => { isDirty: boolean } } };
      }).__E2E_TEST__.projectStore.getState().isDirty === true,
  );
  await seedAvailable(page, '9.9.9');
  await page.evaluate(() => {
    const store = (window as unknown as { __E2E_TEST__: { updateStore: UpdateStoreApi } }).__E2E_TEST__.updateStore;
    store.getState().startDownloading('9.9.9');
    store.getState().setReady('9.9.9');
  });
  await expect(page.getByTestId('update-banner-restart')).toBeVisible();
}

test.describe('In-app update banner', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Update Banner Test');
  });

  test('an available update shows the banner with its version', async ({ page }) => {
    await seedAvailable(page, '9.9.9');
    const banner = page.getByTestId('update-banner');
    await expect(banner).toBeVisible();
    await expect(banner).toContainText('9.9.9');
    await expect(page.getByTestId('update-banner-download')).toBeVisible();
    await expect(page.getByTestId('update-banner-notes')).toBeVisible();
    await expect(page.getByTestId('update-banner-dismiss')).toBeVisible();
  });

  test('a download-progress event updates the progress bar while downloading', async ({ page }) => {
    await seedAvailable(page, '9.9.9');
    await page.evaluate(() => {
      const store = (window as unknown as { __E2E_TEST__: { updateStore: UpdateStoreApi } }).__E2E_TEST__.updateStore;
      store.getState().startDownloading('9.9.9');
    });
    await expect(page.getByTestId('update-banner-progress')).toBeVisible();

    await page.evaluate(() => {
      const mock = (window as unknown as { __tauriEventMock?: { emit: (event: string, payload: unknown) => void } }).__tauriEventMock;
      mock?.emit('app-update:download-progress', { downloaded: 50, total: 100 });
    });

    const bar = page.getByTestId('update-banner-progress').locator('[role="progressbar"]');
    await expect(bar).toHaveAttribute('aria-valuenow', '50');
  });

  test('Download invokes the download command, then Restart is offered', async ({ page }) => {
    await setIpcOverrides(page, { download_app_update: null });
    await seedAvailable(page, '9.9.9');
    await page.getByTestId('update-banner-download').click();

    await expect(page.getByTestId('update-banner-restart')).toBeVisible({ timeout: 10000 });
    await expect(page.getByTestId('update-banner-download')).toHaveCount(0);
  });

  test('Restart with a dirty project raises the unsaved-changes guard instead of restarting', async ({ page }) => {
    await createSectionViaStore(page, {
      name: 'Update Restart Dirty Section',
      shots: [{ azimuth_in: 0, azimuth_out: 0, length: 5, depth_in: 0, depth_out: 0 }],
    });
    await page.waitForFunction(
      () =>
        (window as unknown as {
          __E2E_TEST__: { projectStore: { getState: () => { isDirty: boolean } } };
        }).__E2E_TEST__.projectStore.getState().isDirty === true,
    );

    await seedAvailable(page, '9.9.9');
    await page.evaluate(() => {
      const store = (window as unknown as { __E2E_TEST__: { updateStore: UpdateStoreApi } }).__E2E_TEST__.updateStore;
      store.getState().startDownloading('9.9.9');
      store.getState().setReady('9.9.9');
    });
    await expect(page.getByTestId('update-banner-restart')).toBeVisible();

    await page.getByTestId('update-banner-restart').click();
    await simulateCloseRequested(page);
    await expect(page.getByTestId('unsaved-changes-dialog')).toBeVisible();

    // Cancelling leaves the app running with the banner intact - no restart happened.
    await page.getByTestId('unsaved-changes-cancel').click();
    await expect(page.getByTestId('unsaved-changes-dialog')).toHaveCount(0);
    await expect(page.getByTestId('update-banner-restart')).toBeVisible();
  });

  /**
   * [#579 x #576] The restart is a quit that installs: through the real
   * unsaved-changes prompt, Cancel gives up the restart on both sides, and
   * "Discard & Quit" re-issues the close, which installs the update instead of
   * destroying the window.
   */
  test('Restart through the unsaved-changes prompt: Cancel unarms, Discard & Quit installs', async ({ page }) => {
    await dirtyProjectWithReadyUpdate(page);
    // A real install ends the process; here it answers, and the app stays up.
    await setIpcOverrides(page, { install_app_update_and_restart: null });
    await recordIpcCalls(page, [
      'arm_app_update_restart',
      'unarm_app_update_restart',
      'install_app_update_and_restart',
      'plugin:window|destroy',
    ]);
    const count = async (cmd: string) => (await getIpcCalls(page, cmd)).length;

    await page.getByTestId('update-banner-restart').click();
    await simulateCloseRequested(page);
    await expect(page.getByTestId('unsaved-changes-dialog')).toBeVisible();
    await page.getByTestId('unsaved-changes-cancel').click();
    await expect.poll(() => count('unarm_app_update_restart')).toBe(1);
    expect(await count('install_app_update_and_restart')).toBe(0);

    await page.getByTestId('update-banner-restart').click();
    await simulateCloseRequested(page);
    await expect(page.getByTestId('unsaved-changes-dialog')).toBeVisible();
    await page.getByTestId('unsaved-changes-discard').click();
    // The confirmed quit re-issues the close; the backend would re-emit it.
    await simulateCloseRequested(page);

    await expect.poll(() => count('install_app_update_and_restart')).toBe(1);
    expect(await count('arm_app_update_restart')).toBe(2);
    expect(await count('plugin:window|destroy')).toBe(0);
  });

  /**
   * [review] About's "Check for updates" used to keep its own local result, so
   * its "see the update banner" line pointed at a banner the store had never
   * been told about. It now runs the SAME check path the launch hook does, and
   * the banner is the observable proof of that.
   */
  test('the About check raises the banner, not just its own line', async ({ page }) => {
    await setIpcOverrides(page, {
      check_for_app_update: {
        status: {
          kind: 'available',
          version: '9.9.9',
          notes_url: 'https://example.com/notes',
        },
        current_version: '0.5.0',
      },
    });
    await expect(page.getByTestId('update-banner')).toHaveCount(0);

    await openSettingsFromMenu(page);
    await page.getByTestId('settings-about-button').click();
    await expect(page.getByTestId('about-dialog')).toBeVisible();
    await page.getByTestId('about-check-updates').click();

    // The inline answer names both versions...
    const result = page.getByTestId('about-check-updates-result');
    await expect(result).toContainText('9.9.9');
    await expect(result).toContainText('0.5.0');

    // ...and the banner it points at really appears.
    await page.getByTestId('about-close').click();
    await page.getByTestId('settings-cancel-button').click();
    const banner = page.getByTestId('update-banner');
    await expect(banner).toBeVisible();
    await expect(banner).toContainText('9.9.9');
  });

  test('Dismiss hides the banner for the session, for that version only', async ({ page }) => {
    await seedAvailable(page, '9.9.9');
    await expect(page.getByTestId('update-banner')).toBeVisible();

    await page.getByTestId('update-banner-dismiss').click();
    await expect(page.getByTestId('update-banner')).toHaveCount(0);

    // A DIFFERENT version is not covered by the same dismissal.
    await seedAvailable(page, '10.0.0');
    await expect(page.getByTestId('update-banner')).toBeVisible();
    await expect(page.getByTestId('update-banner')).toContainText('10.0.0');
  });
});
