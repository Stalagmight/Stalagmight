import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage, createProject } from './helpers/app';
import { setIpcOverrides } from './helpers/ipcOverrides';

/**
 * SpeleoDB dialog guards, end to end on the mocked IPC. Synthetic projects only.
 *
 * - The Open from SpeleoDB browser cannot be dismissed while an open is in flight,
 *   and the open's 401 is shown (frontend#3).
 * - Settings' Connect saves the staged settings first, then signs in with the
 *   connection on screen, and reports a failure in an in-app dialog (security#4).
 *   The insecure-HTTP option exists only in a development build: these specs run
 *   against the dev server locally and a production build in CI, and assert it
 *   per mode (`__E2E_TEST__.devBuild`).
 * - Unsent SpeleoDB input is asked about in-app before the app closes (frontend#10).
 */

const ENDPOINT = 'https://synthetic.example';

type Call = { cmd: string; args: Record<string, unknown> | null };

/** Whether the bundle under test is a development build (main.tsx). */
async function devBuild(page: Page): Promise<boolean> {
  return page.evaluate(() => (window as unknown as { __E2E_TEST__: { devBuild?: boolean } }).__E2E_TEST__.devBuild === true);
}

async function calls(page: Page, cmd: string): Promise<Call[]> {
  const all = await page.evaluate(
    () => ((window as unknown as { __MOCK_EXIT_CALLS__?: Call[] }).__MOCK_EXIT_CALLS__ ?? []),
  );
  return all.filter((c) => c.cmd === cmd);
}

/** The OS window X: what the real backend emits after preventing the close. */
async function pressWindowCloseButton(page: Page): Promise<void> {
  await page.evaluate(async () => {
    await (window as unknown as {
      __TAURI_INTERNALS__?: { invoke: (cmd: string, args: unknown) => Promise<unknown> };
    }).__TAURI_INTERNALS__?.invoke('plugin:event|emit', { event: 'tauri://close-requested', payload: null });
  });
}

async function windowDestroyCount(page: Page): Promise<number> {
  return page.evaluate(() => (window as unknown as {
    __MOCK_GET_WINDOW_TEARDOWN_COUNTS__?: () => { destroy: number };
  }).__MOCK_GET_WINDOW_TEARDOWN_COUNTS__?.().destroy ?? -1);
}

test('the SpeleoDB browser stays open while an open is in flight, then shows its 401', async ({ page }) => {
  await setupMockedPage(page);
  await setIpcOverrides(page, {
    speleodb_session: { logged_in: true, endpoint: ENDPOINT, user_email: 'test@example.com' },
    speleodb_list_projects: [{ id: 'synthetic', name: 'Synthetic cave', type_tag: 'stalx',
      latest_commit_sha: null, permission: 'READ_AND_WRITE', lock_holder: null }],
  });
  await page.evaluate(() => {
    const w = window as unknown as {
      __MOCK_IPC_OVERRIDES__: Record<string, unknown>;
      __FAIL_OPEN__?: (error: unknown) => void;
      __E2E_TEST__: { uiStore: { getState: () => { openDialog: (name: string) => void } } };
    };
    w.__MOCK_IPC_OVERRIDES__.speleodb_open_project = new Promise((_resolve, reject) => { w.__FAIL_OPEN__ = reject; });
    w.__E2E_TEST__.uiStore.getState().openDialog('open-from-speleodb');
  });
  const dialog = page.getByTestId('open-from-speleodb-dialog');
  await page.getByTestId('open-from-speleodb-open-synthetic').click();
  await expect(page.getByTestId('open-from-speleodb-close')).toBeDisabled();

  await page.getByTestId('open-from-speleodb-filter').focus();
  await page.keyboard.press('Escape');
  await page.getByTestId('open-from-speleodb-dialog-close').click();
  await expect(dialog).toBeVisible();

  await page.evaluate(() => {
    const w = window as unknown as {
      __MOCK_IPC_OVERRIDES__: Record<string, unknown>;
      __FAIL_OPEN__: (error: unknown) => void;
    };
    // The server ended the session: the next session check says so.
    w.__MOCK_IPC_OVERRIDES__.speleodb_session = { logged_in: false };
    w.__FAIL_OPEN__({ message: 'Your SpeleoDB session expired', action: 'ReLogin', status: 401 });
  });
  await expect(page.getByTestId('open-from-speleodb-logged-out')).toBeVisible();
  await expect(page.getByTestId('open-from-speleodb-error')).toContainText('Your SpeleoDB session expired');
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
});

test.describe('Settings Connect', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await page.getByTestId('global-speleodb-button').click();
    await expect(page.getByTestId('settings-panel-speleodb')).toBeVisible();
  });

  async function connect(page: Page): Promise<void> {
    await page.getByTestId('speleodb-settings-email').fill('test@example.com');
    await page.getByTestId('speleodb-settings-password').fill('synthetic-secret');
    await page.getByTestId('speleodb-settings-try-login').click();
  }

  test('connects over the endpoint on screen, then closes', async ({ page }) => {
    await page.getByTestId('input-speleodb-endpoint').fill('https://lab.example/');
    await connect(page);
    await expect(page.getByTestId('settings-dialog')).toBeHidden();
    const logins = (await calls(page, 'speleodb_login')).map((c) => c.args);
    expect(logins).toEqual([expect.objectContaining({ endpoint: 'https://lab.example/' })]);
    // The box on screen in a development build; no choice at all in a release one.
    if (await devBuild(page)) expect(logins[0]).toMatchObject({ insecureHttp: false });
    else expect(logins[0]).not.toHaveProperty('insecureHttp');
  });

  test('the insecure-HTTP option exists only in a development build, and Connect uses the box on screen', async ({ page }) => {
    const toggle = page.getByTestId('toggle-speleodb-insecure-http');
    if (!await devBuild(page)) {
      await expect(page.getByTestId('setting-speleodb-insecure-http')).toHaveCount(0);
      await expect(toggle).toHaveCount(0);
      return;
    }
    await expect(toggle).toHaveAttribute('aria-checked', 'false');
    await toggle.click();
    await expect(page.getByTestId('speleodb-insecure-http-warning')).toBeVisible();
    await page.getByTestId('input-speleodb-endpoint').fill('http://lab.example/');
    await connect(page);
    await expect(page.getByTestId('settings-dialog')).toBeHidden();
    expect((await calls(page, 'speleodb_login')).map((c) => c.args)).toEqual([
      expect.objectContaining({ endpoint: 'http://lab.example/', insecureHttp: true }),
    ]);
  });

  test('saves another tab\'s staged edits before it connects, then closes', async ({ page }) => {
    await page.getByTestId('settings-tab-general').click();
    const autoSave = page.getByTestId('toggle-auto-save');
    const before = await autoSave.getAttribute('aria-checked');
    await autoSave.click();
    await page.getByTestId('settings-tab-speleodb').click();
    await connect(page);

    await expect(page.getByTestId('settings-dialog')).toBeHidden();
    const saved = await page.evaluate(async () => (window as unknown as {
      __TAURI_INTERNALS__: { invoke: (cmd: string) => Promise<{ general: { auto_save: boolean } }> };
    }).__TAURI_INTERNALS__.invoke('get_config'));
    expect(String(saved.general.auto_save)).not.toBe(before);
    expect(await calls(page, 'speleodb_login')).toHaveLength(1);
  });

  test('a failure shows in an in-app dialog naming the server on screen, and Settings stays open', async ({ page }) => {
    // The server on screen cannot be reached.
    await page.evaluate(() => {
      const w = window as unknown as { __MOCK_IPC_OVERRIDES__?: Record<string, unknown> };
      const overrides = w.__MOCK_IPC_OVERRIDES__ ?? {};
      Object.defineProperty(overrides, 'speleodb_login', {
        enumerable: true,
        configurable: true,
        get: () => Promise.reject({
          message: 'Network error talking to SpeleoDB: refused', action: 'Retry', status: null,
          transport: 'Refused', unpushed_local: null, newer_schema: null,
        }),
      });
      w.__MOCK_IPC_OVERRIDES__ = overrides;
    });
    await page.getByTestId('input-speleodb-endpoint').fill('https://unsaved.example/');
    await connect(page);

    const error = page.getByTestId('speleodb-connection-error');
    await expect(error).toBeVisible();
    await expect(error).toBeInViewport();
    // Named for the server that was tried: the one on screen.
    await expect(error).toContainText('Could not connect to SpeleoDB');
    await expect(error).toContainText('https://unsaved.example/');
    await expect(error).not.toContainText('configured server');
    await page.getByTestId('speleodb-connection-error-close').click();
    await expect(error).toBeHidden();
    await expect(page.getByTestId('settings-dialog')).toBeVisible();
    await expect(page.getByTestId('speleodb-account-login-form')).toBeVisible();
  });
});

test('closing the app asks in-app before dropping the SpeleoDB project form', async ({ page }) => {
  await setupMockedPage(page);
  await createProject(page, 'Synthetic draft');
  await page.evaluate(() => {
    (window as unknown as {
      __E2E_TEST__: { uiStore: { getState: () => { openDialog: (name: string) => void } } };
    }).__E2E_TEST__.uiStore.getState().openDialog('publish-to-speleodb');
  });
  const description = page.getByTestId('publish-to-speleodb-dialog-description');
  await description.fill('Synthetic system description');

  await pressWindowCloseButton(page);
  await expect(page.getByTestId('exit-confirm-dialog')).toBeVisible();
  await expect(page.getByTestId('exit-confirm-message')).toContainText('the SpeleoDB project form');
  await page.getByTestId('exit-confirm-cancel').click();
  await expect(page.getByTestId('exit-confirm-dialog')).toBeHidden();
  await expect(description).toHaveValue('Synthetic system description');
  expect(await windowDestroyCount(page)).toBe(0);

  await pressWindowCloseButton(page);
  await page.getByTestId('exit-confirm-ok').click();
  await expect(page.getByTestId('publish-to-speleodb-dialog')).toBeHidden();
});
