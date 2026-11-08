import { test, expect } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  createSectionViaStore,
  openSettingsFromMenu,
  seedProjectWithSections,
  waitForMapReady,
  waitForNoDialogs,
} from './helpers/app';
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

/** The page-side handle `holdInstall` leaves for settling the install. */
interface InstallHoldWindow {
  __MOCK_IPC_OVERRIDES__?: Record<string, unknown>;
  __E2E_INSTALL__?: { resolve: (value: unknown) => void; reject: (reason: unknown) => void };
}

/**
 * Keep `install_app_update_and_restart` running until the spec settles it through
 * `window.__E2E_INSTALL__`: a real install takes seconds to write the new bundle,
 * and the mock otherwise answers at once.
 */
async function holdInstall(page: import('@playwright/test').Page): Promise<void> {
  await page.evaluate(() => {
    const w = window as unknown as InstallHoldWindow;
    const running = new Promise((resolve, reject) => {
      w.__E2E_INSTALL__ = { resolve, reject };
    });
    w.__MOCK_IPC_OVERRIDES__ = { ...(w.__MOCK_IPC_OVERRIDES__ ?? {}), install_app_update_and_restart: running };
  });
}

/** The dialog slot, as the specs reach it through the exposed UI store. */
interface UiStoreWindow {
  __E2E_TEST__: {
    uiStore: {
      getState: () => {
        activeDialog: string | null;
        openDialog: (dialog: string, data?: Record<string, unknown>) => void;
      };
    };
  };
}

/** The page-side handle `holdShotEdit` leaves for releasing the held edit. */
interface EditHoldWindow {
  __TAURI_INTERNALS__: { invoke: (cmd: string, args?: unknown, opts?: unknown) => Promise<unknown> };
  __E2E_HELD_EDIT__?: { release: () => void };
  __E2E_TEST__: {
    projectStore: {
      getState: () => { isDirty: boolean; project: { sections: Array<{ shots: Array<{ length: number }> }> } };
      setState: (s: unknown) => void;
    };
  };
}

/**
 * Hold the next `update_shot` until the spec releases it through
 * `window.__E2E_HELD_EDIT__`, then hand it to the mock as issued: a real edit's
 * IPC and the refresh after it take a moment, and the mock otherwise answers at
 * once, which hides the window an exit can fall into.
 */
async function holdShotEdit(page: import('@playwright/test').Page): Promise<void> {
  await page.evaluate(() => {
    const w = window as unknown as EditHoldWindow;
    const original = w.__TAURI_INTERNALS__.invoke;
    w.__TAURI_INTERNALS__.invoke = (cmd, args, opts) => {
      if (cmd !== 'update_shot' || w.__E2E_HELD_EDIT__) return original(cmd, args, opts);
      return new Promise((resolve, reject) => {
        w.__E2E_HELD_EDIT__ = { release: () => void original(cmd, args, opts).then(resolve, reject) };
      });
    };
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
   * [#579 x #576] The install runs off the main thread, so the window stays live
   * while it writes the new bundle - after the user already passed the
   * unsaved-changes prompt, so an edit made then would be dropped by the restart
   * without a word. From the moment the install starts the app is held still: a
   * shortcut does nothing (Ctrl+Z sends no undo), and a failed install gives the
   * app back with the existing failure toast.
   *
   * "Sends no undo" is read off the mutation queue, not a timer: undo and redo run
   * on one first-in first-out queue, and Ctrl+Z puts its undo on it inside the key
   * handler. So once the redo pressed after the install failed has reached the
   * backend, an undo the held Ctrl+Z had queued would have reached it first.
   */
  test('while the update installs, shortcuts do nothing; a failed install lifts the hold', async ({ page }) => {
    await dirtyProjectWithReadyUpdate(page);
    await holdInstall(page);
    await recordIpcCalls(page, ['install_app_update_and_restart', 'undo', 'redo']);
    const count = async (cmd: string) => (await getIpcCalls(page, cmd)).length;

    await page.getByTestId('update-banner-restart').click();
    await simulateCloseRequested(page);
    await page.getByTestId('unsaved-changes-discard').click();
    await simulateCloseRequested(page);
    await expect.poll(() => count('install_app_update_and_restart')).toBe(1);

    // The sentinel: Ctrl+Z sends an undo whenever the app is not held.
    await page.keyboard.press('Control+z');
    expect(await count('undo')).toBe(0);
    const overlay = page.getByTestId('update-install-overlay');
    await expect(overlay).toBeVisible();
    await expect(overlay).toBeFocused();
    await expect(page.getByTestId('update-install-panel')).toContainText('9.9.9');

    await page.evaluate(() => {
      (window as unknown as InstallHoldWindow).__E2E_INSTALL__?.reject({
        code: 'FAILED',
        message: 'Update install failed: disk full',
      });
    });
    await expect(overlay).toHaveCount(0);
    await expect(page.locator('[data-toast-key="app-update-install-failed"]')).toBeVisible();
    await expect(page.getByTestId('update-banner-download')).toBeVisible();

    // The control: a redo queued behind anything the sentinel queued. Once it is sent,
    // the sentinel's undo would have been sent before it.
    await page.keyboard.press('Control+y');
    await expect.poll(() => count('redo')).toBe(1);
    expect(await getIpcCalls(page), 'the held Ctrl+Z sent nothing before the redo').toEqual(
      [expect.objectContaining({ cmd: 'install_app_update_and_restart' }), expect.objectContaining({ cmd: 'redo' })],
    );

    await page.keyboard.press('Control+z');
    await expect.poll(() => count('undo')).toBe(1);
  });

  /**
   * [#576 W86] A dialog the app asks for while the update installs (a SpeleoDB verb
   * answering, a load's prompt, an async failure's error) needs no click or key press,
   * so the screen hold cannot stop it. It opened above the install window and took
   * input there; it now waits, and opens once a failed install gives the app back,
   * beside the install's own failure toast.
   */
  test('a dialog asked for during the install waits; a failed install shows it with the failure toast', async ({ page }) => {
    await dirtyProjectWithReadyUpdate(page);
    await holdInstall(page);
    await recordIpcCalls(page, ['install_app_update_and_restart']);

    await page.getByTestId('update-banner-restart').click();
    await simulateCloseRequested(page);
    await page.getByTestId('unsaved-changes-discard').click();
    await simulateCloseRequested(page);
    await expect.poll(async () => (await getIpcCalls(page, 'install_app_update_and_restart')).length).toBe(1);
    const overlay = page.getByTestId('update-install-overlay');
    await expect(overlay).toBeVisible();

    // `openDialog` is synchronous, so the slot read right after it is the answer.
    const slot = await page.evaluate(() => {
      const ui = (window as unknown as UiStoreWindow).__E2E_TEST__.uiStore;
      ui.getState().openDialog('error', { title: 'Open', message: 'Asked for during the install' });
      return ui.getState().activeDialog;
    });
    expect(slot, 'no dialog takes the slot above the install window').toBeNull();
    await expect(overlay).toBeFocused();
    await expect(page.getByTestId('error-dialog')).toHaveCount(0);

    await page.evaluate(() => {
      (window as unknown as InstallHoldWindow).__E2E_INSTALL__?.reject({
        code: 'FAILED',
        message: 'Update install failed: disk full',
      });
    });
    await expect(overlay).toHaveCount(0);
    await expect(page.getByTestId('error-dialog')).toContainText('Asked for during the install');
    await expect(page.locator('[data-toast-key="app-update-install-failed"]')).toBeVisible();
  });

  /**
   * [#576 W83] The click on Restart is also the blur that commits a grid cell
   * being edited, so on a clean project that edit is still in flight when the
   * close request reaches the exit guard. The guard read the clean flag and the
   * update installed with no prompt, dropping the edit; it now waits for the edit
   * to land and asks.
   */
  test('Restart pressed out of a grid cell edit prompts once that edit lands', async ({ page }) => {
    await seedProjectWithSections(page, [
      {
        name: 'Restart Edit Section',
        shots: [{ azimuth_in: 0, azimuth_out: 0, length: 5, depth_in: 0, depth_out: 0 }],
      },
    ]);
    await waitForMapReady(page);
    await waitForNoDialogs(page);
    await page.keyboard.press('Control+3');
    await expect(page.getByTestId('data-table')).toBeVisible();
    // Saved: nothing for the guard to protect yet.
    await page.evaluate(() => {
      (window as unknown as EditHoldWindow).__E2E_TEST__.projectStore.setState({ isDirty: false });
    });
    await seedAvailable(page, '9.9.9');
    await page.evaluate(() => {
      const store = (window as unknown as { __E2E_TEST__: { updateStore: UpdateStoreApi } }).__E2E_TEST__.updateStore;
      store.getState().startDownloading('9.9.9');
      store.getState().setReady('9.9.9');
    });
    await expect(page.getByTestId('update-banner-restart')).toBeVisible();
    await setIpcOverrides(page, { install_app_update_and_restart: null });
    await holdShotEdit(page);
    await recordIpcCalls(page, ['update_shot', 'install_app_update_and_restart', 'plugin:window|destroy']);
    const count = async (cmd: string) => (await getIpcCalls(page, cmd)).length;

    // Type a new length; the editor is still open when Restart is pressed.
    await page.getByTestId('grid-cell-0-length').click();
    await page.keyboard.type('42');
    await expect(page.getByTestId('grid-cell-editor')).toHaveValue('42');
    await page.getByTestId('update-banner-restart').click();
    await expect(page.getByTestId('grid-cell-editor')).toHaveCount(0);
    // The blur committed the cell: its edit is on its way, not answered yet.
    await expect.poll(() => count('update_shot')).toBe(1);
    await expect
      .poll(() => page.evaluate(() => (window as unknown as EditHoldWindow).__E2E_HELD_EDIT__ != null))
      .toBe(true);
    expect(await page.evaluate(() => (window as unknown as EditHoldWindow).__E2E_TEST__.projectStore.getState().isDirty)).toBe(false);

    await simulateCloseRequested(page);
    await page.evaluate(() => (window as unknown as EditHoldWindow).__E2E_HELD_EDIT__?.release());

    // The guard saw the edit: the prompt is up, and nothing was installed.
    await expect(page.getByTestId('unsaved-changes-dialog')).toBeVisible();
    expect(await count('install_app_update_and_restart')).toBe(0);
    expect(await count('plugin:window|destroy')).toBe(0);
    expect(
      await page.evaluate(
        () => (window as unknown as EditHoldWindow).__E2E_TEST__.projectStore.getState().project.sections[0]?.shots[0]?.length,
      ),
    ).toBe(42);
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
