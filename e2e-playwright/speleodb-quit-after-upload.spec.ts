import { test, expect, type Page } from './fixtures/coverage';
import { createProject, openAppMenu, setupMockedPage, waitForNoPendingQuit } from './helpers/app';
import { openSpeleoTab } from './helpers/speleoWorkspace';
import type { useSpeleoStore } from '../src/store/speleoStore';

/**
 * lock#6 / frontend#4: quitting while a SpeleoDB upload is running never cuts it.
 *
 * The window X (like menu Quit, Ctrl+Q and an OS close) reaches the app as
 * `tauri://close-requested`. While the upload is in flight the app keeps the
 * window, toasts that it will close when the upload finishes, shows a waiting
 * prompt above the upload progress, and closes once the upload has landed.
 * "Quit now" is an explicit choice behind a warning.
 */
type QuitWindow = Window & {
  __E2E_TEST__: { speleoStore: typeof useSpeleoStore };
  __MOCK_IPC_OVERRIDES__: Record<string, unknown>;
  __MOCK_SET_PROJECT_IS_REMOTE__: (remote: boolean) => void;
  __MOCK_GET_WINDOW_TEARDOWN_COUNTS__: () => { destroy: number; close: number };
  __finishUpload: () => void;
  __forcedExitArmed?: number;
  __TAURI_INTERNALS__: { invoke: (cmd: string, args: unknown) => Promise<unknown> };
};

async function startSlowUpload(page: Page): Promise<void> {
  await setupMockedPage(page);
  await createProject(page, 'Synthetic quit during upload');
  await page.evaluate(() => {
    const w = window as unknown as QuitWindow;
    w.__MOCK_SET_PROJECT_IS_REMOTE__(true);
    const pending = new Promise((resolve) => { w.__finishUpload = () => resolve({ kind: 'pushed' }); });
    const overrides: Record<string, unknown> = {
      speleodb_link_status: { is_remote: true, project_id: 'quit-upload', edit_enabled: true,
        permission: 'READ_AND_WRITE', has_unpushed_edits: false, unwound: false,
        remote_state: 'up_to_date', lock_holder: 'test@example.com', access_denied: false,
        server_error: null, exclude_geojson: false, undo_mode: 'full' },
    };
    Object.defineProperty(overrides, 'speleodb_push', { get: () => pending });
    w.__MOCK_IPC_OVERRIDES__ = overrides;
    w.__E2E_TEST__.speleoStore.setState({
      session: { loggedIn: true, userEmail: 'test@example.com', endpoint: 'https://speleodb.example' },
      projectLinkKind: 'remote', remoteProjectId: 'quit-upload',
      link: { permission: 'READ_AND_WRITE', editState: 'editing', hasUnpushedEdits: true,
        lockHolder: 'test@example.com', lockedByOther: false, unwound: false, undoMode: 'full',
        remoteState: 'up_to_date', noAccess: false, serverError: null },
    });
  });
  await page.keyboard.press('Control+s');
  await page.getByTestId('speleodb-done-message').fill('Synthetic survey update');
  await page.getByTestId('speleodb-done-confirm').click();
  await expect(page.getByTestId('speleodb-upload-progress')).toBeVisible();
}

/** What the native backend does after preventing a window close. */
async function deliverCloseRequested(page: Page): Promise<void> {
  await page.evaluate(async () => {
    await (window as unknown as QuitWindow).__TAURI_INTERNALS__.invoke('plugin:event|emit', {
      event: 'tauri://close-requested', payload: null,
    });
  });
}

const teardown = (page: Page) => page.evaluate(() => (window as unknown as QuitWindow).__MOCK_GET_WINDOW_TEARDOWN_COUNTS__());

test('window X during an upload waits for it, with a toast and progress, then closes', async ({ page }) => {
  await startSlowUpload(page);
  await deliverCloseRequested(page);
  await expect(page.getByTestId('toast').filter({ hasText: 'will close when it finishes' })).toBeVisible();
  const wait = page.getByTestId('quit-wait-dialog');
  await expect(wait).toBeVisible();
  // Layered over the upload progress, and the one that takes input.
  await expect(page.getByTestId('speleodb-upload-progress')).toBeVisible();
  await expect(wait.getByTestId('quit-wait-cancel')).toBeEnabled();
  expect(await teardown(page)).toEqual({ destroy: 0, close: 0 });

  await page.evaluate(() => (window as unknown as QuitWindow).__finishUpload());
  await expect(page.getByTestId('speleodb-upload-progress')).toHaveCount(0);
  // The quit is re-issued once the upload has landed...
  await expect.poll(async () => (await teardown(page)).close).toBe(1);
  await expect(wait).toHaveCount(0);
  // ...and, with the work uploaded, the clean project's lock is handed back
  // before the quit goes on (never strand a lock silently), then it closes.
  await deliverCloseRequested(page);
  await expect.poll(async () => (await teardown(page)).close).toBe(2);
  await deliverCloseRequested(page);
  await expect.poll(async () => (await teardown(page)).destroy).toBe(1);
  expect(await page.evaluate(() => (window as unknown as QuitWindow).__forcedExitArmed ?? 0)).toBe(0);
});

test('a quit that waited for the upload, then hits a failed lock release, asks instead of exiting', async ({ page }) => {
  await startSlowUpload(page);
  await page.evaluate(() => {
    (window as unknown as QuitWindow).__MOCK_IPC_OVERRIDES__.speleodb_settle_open_lock = {
      success: false, warnings: [], section_ids: [], lock_release: 'failed',
      error: 'The SpeleoDB lock could not be handed back: SpeleoDB returned HTTP 503: synthetic',
    };
  });
  await deliverCloseRequested(page);
  await expect(page.getByTestId('quit-wait-dialog')).toBeVisible();
  await page.evaluate(() => (window as unknown as QuitWindow).__finishUpload());
  // The re-issued quit reaches the lock release, which fails: the question, not an exit.
  await expect.poll(async () => (await teardown(page)).close).toBe(1);
  await deliverCloseRequested(page);
  const dialog = page.getByTestId('speleodb-release-failed-dialog');
  await expect(dialog).toBeVisible();
  await expect(page.getByTestId('speleodb-release-failed-body')).toContainText('Stalagmight closes');
  // The question is the release step's outcome, and the wait has handed over:
  // nothing can re-issue the quit behind it.
  await waitForNoPendingQuit(page);
  expect(await teardown(page)).toEqual({ destroy: 0, close: 1 });
  await page.getByTestId('speleodb-release-failed-cancel').click();
  await expect(dialog).toHaveCount(0);
  await waitForNoPendingQuit(page);
  expect(await teardown(page)).toEqual({ destroy: 0, close: 1 });
  await expect(page.getByTestId('single-system-view')).toBeVisible();
});

test('Keep Stalagmight open calls the quit off and lets the upload finish', async ({ page }) => {
  await startSlowUpload(page);
  await deliverCloseRequested(page);
  await page.getByTestId('quit-wait-cancel').click();
  await expect(page.getByTestId('quit-wait-dialog')).toHaveCount(0);
  await page.evaluate(() => (window as unknown as QuitWindow).__finishUpload());
  await expect(page.getByTestId('speleodb-done-dialog')).toHaveCount(0);
  await expect(page.getByTestId('speleodb-upload-progress')).toHaveCount(0);
  // The upload has landed; with the quit called off, nothing watches it to quit.
  await waitForNoPendingQuit(page);
  expect(await teardown(page)).toEqual({ destroy: 0, close: 0 });
});

test('Quit now cuts the upload only after a warning, and tells the backend so', async ({ page }) => {
  await startSlowUpload(page);
  await deliverCloseRequested(page);
  await page.getByTestId('quit-wait-force').click();
  await expect(page.getByTestId('quit-wait-force-warning')).toContainText('cut off');
  await page.getByTestId('quit-wait-force-confirm').click();
  await expect.poll(async () => (await teardown(page)).close).toBe(1);
  await deliverCloseRequested(page);
  await expect.poll(async () => (await teardown(page)).destroy).toBe(1);
  expect(await page.evaluate(() => (window as unknown as QuitWindow).__forcedExitArmed ?? 0)).toBe(1);
});

test('window X during the Publishing overlay waits for the publish, then closes', async ({ page }) => {
  await setupMockedPage(page);
  await createProject(page, 'Synthetic quit during publish');
  await page.evaluate(() => {
    const w = window as unknown as QuitWindow & { __finishPublish: () => void };
    w.__E2E_TEST__.speleoStore.setState({ session: { loggedIn: true, userEmail: 'test@example.com', endpoint: 'https://speleodb.example' } });
    w.__MOCK_IPC_OVERRIDES__ = {
      speleodb_publish: new Promise((resolve) => {
        w.__finishPublish = () => resolve({ project_id: 'pending', endpoint: 'https://speleodb.example', local_path: '/cache/pending/project.stalx' });
      }),
      speleodb_link_status: { is_remote: true, project_id: 'pending', edit_enabled: false, has_unpushed_edits: false, permission: 'ADMIN', remote_state: 'up_to_date' },
    };
  });
  await openSpeleoTab(page);
  await page.getByTestId('speleodb-workspace-convert').click();
  await page.getByTestId('publish-to-speleodb-dialog-description').fill('Team survey');
  await page.getByTestId('publish-to-speleodb-dialog-country').click();
  await page.getByTestId('publish-to-speleodb-dialog-country-option-US').click();
  await page.getByTestId('publish-to-speleodb-dialog-submit').click();
  await expect(page.getByTestId('speleodb-publish-progress')).toBeVisible();

  await deliverCloseRequested(page);
  await expect(page.getByTestId('toast').filter({ hasText: 'will close when it finishes' })).toBeVisible();
  await expect(page.getByTestId('quit-wait-dialog')).toBeVisible();
  expect(await teardown(page)).toEqual({ destroy: 0, close: 0 });

  await page.evaluate(() => (window as unknown as { __finishPublish: () => void }).__finishPublish());
  await expect(page.getByTestId('speleodb-publish-progress')).toHaveCount(0);
  await expect.poll(async () => (await teardown(page)).close).toBe(1);
});

// The backend's exit waits for a project close (it can be handing the lock back)
// as for any other transfer, so the quit gate must too: before, the window was
// destroyed while the process lived on, windowless, until the close finished.
test('window X while Close Project is still handing the lock back waits for it, then closes', async ({ page }) => {
  await setupMockedPage(page);
  await createProject(page, 'Synthetic quit during close');
  await page.evaluate(() => {
    const w = window as unknown as QuitWindow & { __finishClose: () => void };
    const closing = new Promise((resolve) => {
      w.__finishClose = () => resolve({ success: true, warnings: [], section_ids: [], lock_release: 'released' });
    });
    const overrides: Record<string, unknown> = {};
    Object.defineProperty(overrides, 'close_project', { get: () => closing });
    w.__MOCK_IPC_OVERRIDES__ = overrides;
  });
  await openAppMenu(page);
  await page.getByTestId('back-button').click();
  await expect.poll(() => page.evaluate(() =>
    ((window as unknown as { __MOCK_EXIT_CALLS__?: Array<{ cmd: string }> }).__MOCK_EXIT_CALLS__ ?? [])
      .filter((c) => c.cmd === 'close_project').length)).toBe(1);

  await deliverCloseRequested(page);
  await expect(page.getByTestId('toast').filter({ hasText: 'will close when it finishes' })).toBeVisible();
  await expect(page.getByTestId('quit-wait-dialog')).toBeVisible();
  expect(await teardown(page)).toEqual({ destroy: 0, close: 0 });

  await page.evaluate(() => (window as unknown as { __finishClose: () => void }).__finishClose());
  await expect(page.getByTestId('global-view')).toBeVisible();
  // Re-issued once the close has settled; nothing is open any more, so it closes.
  await expect.poll(async () => (await teardown(page)).close).toBe(1);
  await deliverCloseRequested(page);
  await expect.poll(async () => (await teardown(page)).destroy).toBe(1);
});
