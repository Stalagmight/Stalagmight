import { test, expect } from './fixtures/coverage';
import { setupMockedPage, createProject, createSectionViaStore, openSettingsFromMenu } from './helpers/app';
import { setIpcOverrides } from './helpers/ipcOverrides';

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
    setAvailable: (
      seq: number,
      info: { version: string; notesUrl: string; installsOnQuit: boolean },
    ) => void;
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
    store
      .getState()
      .setAvailable(seq, { version: v, notesUrl: 'https://example.com/notes', installsOnQuit: true });
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
   * One quit attempt asks at a time. Pressing the window X again while the
   * unsent-input question of a restart is on screen used to start a second
   * attempt whose own question answered the first with "no": the first then
   * dropped the restart, and the "yes" that followed only quit, without the
   * update. The second X now joins the attempt that is asking.
   */
  test('Restart survives a second window X while the unsent-input question is open', async ({ page }) => {
    await setIpcOverrides(page, { install_app_update_and_restart: null });
    await seedAvailable(page, '9.9.9');
    await page.evaluate(() => {
      const store = (window as unknown as { __E2E_TEST__: { updateStore: UpdateStoreApi } }).__E2E_TEST__.updateStore;
      store.getState().startDownloading('9.9.9');
      store.getState().setReady('9.9.9');
    });
    await page.getByTestId('update-banner-restart').click();

    // The SpeleoDB project form holds unsent input, so the restart asks about it.
    await page.evaluate(() => {
      (window as unknown as {
        __E2E_TEST__: { uiStore: { getState: () => { openDialog: (name: string) => void } } };
      }).__E2E_TEST__.uiStore.getState().openDialog('publish-to-speleodb');
    });
    await page.getByTestId('publish-to-speleodb-dialog-description').fill('Synthetic system description');

    await simulateCloseRequested(page);
    await expect(page.getByTestId('exit-confirm-message')).toContainText('the SpeleoDB project form');
    // The user presses the window X again while the question is open.
    await simulateCloseRequested(page);
    await expect(page.getByTestId('exit-confirm-dialog')).toBeVisible();
    await page.getByTestId('exit-confirm-ok').click();

    const installs = () =>
      page.evaluate(
        () =>
          ((window as unknown as { __MOCK_EXIT_CALLS__?: Array<{ cmd: string }> }).__MOCK_EXIT_CALLS__ ?? []).filter(
            (call) => call.cmd === 'install_app_update_and_restart',
          ).length,
      );
    await expect.poll(installs).toBe(1);
    const destroyed = await page.evaluate(
      () =>
        (window as unknown as { __MOCK_GET_WINDOW_TEARDOWN_COUNTS__?: () => { destroy: number } })
          .__MOCK_GET_WINDOW_TEARDOWN_COUNTS__?.().destroy ?? -1,
    );
    expect(destroyed, 'the install takes the destroy\'s place').toBe(0);
  });

  /**
   * [review] About's "Check for updates" used to keep its own local result, so
   * its "see the update banner" line pointed at a banner the store had never
   * been told about. It now runs the SAME check path the launch hook does, and
   * the banner is the observable proof of that.
   *
   * [#631] That path also downloads in the background, so the banner arrives
   * already "ready", promising the install on quit.
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
        installs_on_quit: true,
      },
      download_app_update: null,
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
    await expect(banner).toContainText('will be installed when you quit');
    await expect(page.getByTestId('update-banner-restart')).toBeVisible();
    await expect(page.getByTestId('update-banner-download')).toHaveCount(0);
  });

  /**
   * [#645] The setting used to gate only whether a download STARTED. Turning it
   * off now takes back the download a check started - here About's, which runs
   * in view - and the banner offers Download again instead of an install.
   */
  test('turning automatic updates off takes back the automatic download', async ({ page }) => {
    await setIpcOverrides(page, {
      check_for_app_update: {
        status: { kind: 'available', version: '9.9.9', notes_url: 'https://example.com/notes' },
        current_version: '0.5.0',
        installs_on_quit: true,
      },
      download_app_update: null,
    });
    await openSettingsFromMenu(page);
    await page.getByTestId('settings-about-button').click();
    await page.getByTestId('about-check-updates').click();
    await expect(page.getByTestId('about-check-updates-result')).toContainText('9.9.9');
    await page.getByTestId('about-close').click();

    await page.getByTestId('toggle-update-check').click();
    await page.getByTestId('settings-save-button').click();
    await expect(page.getByTestId('settings-dialog')).toHaveCount(0);

    await expect(page.getByTestId('update-banner-download')).toBeVisible();
    await expect(page.getByTestId('update-banner-restart')).toHaveCount(0);
    await expect(page.getByTestId('update-banner')).not.toContainText('will be installed when you quit');
    expect(
      await page.evaluate(
        () => (window as unknown as { __MOCK_DISCARD_UPDATE_CALLS__?: number }).__MOCK_DISCARD_UPDATE_CALLS__,
      ),
    ).toBe(1);
  });

  /**
   * [#650] Beta installs release candidates, so switching to it is confirmed
   * first; Cancel keeps the track, and Stable needs no confirmation.
   */
  test('switching the update track to Beta asks first', async ({ page }) => {
    await openSettingsFromMenu(page);
    const track = page.getByTestId('select-update-track');
    await expect(track).toContainText('Stable');

    await track.click();
    await page.getByTestId('select-update-track-option-beta').click();
    const warning = page.getByTestId('update-track-beta-dialog');
    await expect(warning).toBeVisible();
    await expect(warning).toContainText('release candidates');
    await page.getByTestId('update-track-beta-cancel').click();
    await expect(warning).toHaveCount(0);
    await expect(track).toContainText('Stable');

    await track.click();
    await page.getByTestId('select-update-track-option-beta').click();
    await page.getByTestId('update-track-beta-confirm').click();
    await expect(track).toContainText('Beta');
    // The settings dialog itself is still open: the warning closed alone.
    await expect(page.getByTestId('settings-dialog')).toBeVisible();

    await track.click();
    await page.getByTestId('select-update-track-option-stable').click();
    await expect(page.getByTestId('update-track-beta-dialog')).toHaveCount(0);
    await expect(track).toContainText('Stable');
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
