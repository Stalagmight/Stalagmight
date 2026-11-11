import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage, createProject } from './helpers/app';
import { setIpcOverrides } from './helpers/ipcOverrides';

/**
 * The SpeleoDB exit prompt and the sign-out guard, end to end on the mocked IPC.
 * Synthetic projects only.
 *
 * Holding the write lock over work SpeleoDB has not seen, every way out of the
 * project asks ONE in-app question - Cancel / Discard (second confirm) / Keep the
 * lock / Upload, all in the dialog's footer row - and carries the answer out
 * before the exit continues. The backend calls each answer makes are read back from
 * `__MOCK_EXIT_CALLS__` (mocks/tauri-ipc.ts).
 */

const ME = 'diver@example.com';
const ENDPOINT = 'https://synthetic.example';

type Call = { cmd: string; args: Record<string, unknown> | null };

async function calls(page: Page, cmd?: string): Promise<Call[]> {
  const all = await page.evaluate(
    () => ((window as unknown as { __MOCK_EXIT_CALLS__?: Call[] }).__MOCK_EXIT_CALLS__ ?? []),
  );
  return cmd ? all.filter((c) => c.cmd === cmd) : all;
}

async function resetCalls(page: Page): Promise<void> {
  await page.evaluate(() => { (window as unknown as { __MOCK_EXIT_CALLS__?: unknown[] }).__MOCK_EXIT_CALLS__ = []; });
}

/** Seed "we hold the lock" with un-uploaded work, saved (and optionally unsaved). */
async function seedLockedWithChanges(page: Page, opts: { unsaved?: boolean } = {}): Promise<void> {
  await page.evaluate(({ unsaved, me, endpoint }) => {
    const w = window as unknown as {
      __speleoLinkStatusOverrides?: Record<string, unknown>;
      __E2E_TEST__: {
        projectStore: { setState: (s: unknown) => void };
        speleoStore: { setState: (s: unknown) => void };
      };
    };
    // A late status refresh must not quietly turn the project back into a local one.
    w.__speleoLinkStatusOverrides = {
      is_remote: true, edit_enabled: true, has_unpushed_edits: true,
      permission: 'READ_AND_WRITE', lock_holder: me, remote_state: 'up_to_date',
    };
    w.__E2E_TEST__.speleoStore.setState({
      session: { loggedIn: true, userEmail: me, endpoint },
      remoteProjectId: 'synthetic-open',
      projectLinkKind: 'remote',
      busy: false,
      link: {
        permission: 'READ_AND_WRITE', editState: 'editing', lockHolder: me, lockedByOther: false,
        hasUnpushedEdits: true, unwound: false, undoMode: 'full', remoteState: 'up_to_date',
        noAccess: false, serverError: null,
      },
    });
    w.__E2E_TEST__.projectStore.setState({ isDirty: unsaved === true });
  }, { unsaved: opts.unsaved, me: ME, endpoint: ENDPOINT });
  await resetCalls(page);
}

async function openMenuAndClose(page: Page): Promise<void> {
  await page.locator('[data-testid="app-menu-trigger"]').click();
  await page.waitForSelector('[data-testid="app-menu"]', { timeout: 5000 });
  await page.click('[data-testid="back-button"]');
}

/** The OS window X: what the real backend emits after preventing the close. */
async function pressWindowCloseButton(page: Page): Promise<void> {
  await page.evaluate(async () => {
    await (window as unknown as {
      __TAURI_INTERNALS__?: { invoke: (cmd: string, args: unknown) => Promise<unknown> };
    }).__TAURI_INTERNALS__?.invoke('plugin:event|emit', { event: 'tauri://close-requested', payload: null });
  });
}

async function windowTeardownCounts(page: Page): Promise<{ destroy: number; close: number }> {
  return page.evaluate(() => (window as unknown as {
    __MOCK_GET_WINDOW_TEARDOWN_COUNTS__?: () => { destroy: number; close: number };
  }).__MOCK_GET_WINDOW_TEARDOWN_COUNTS__?.() ?? { destroy: -1, close: -1 });
}

test.describe('SpeleoDB exit prompt - close', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Synthetic exit');
    await setIpcOverrides(page, { speleodb_done: { kind: 'released' } });
  });

  test('saved but un-uploaded work under our lock prompts; Cancel keeps everything', async ({ page }) => {
    await seedLockedWithChanges(page);
    await openMenuAndClose(page);
    const dialog = page.getByTestId('speleodb-exit-dialog');
    await expect(dialog).toBeVisible();
    await expect(page.getByTestId('speleodb-exit-body')).toContainText('close the project');
    // The app's dialog layout: every button in the one footer row, Cancel first and
    // the primary last, with what each does said once in the body.
    await expect(page.getByTestId('speleodb-exit-footer').getByRole('button')).toHaveText(['Cancel', 'Discard', 'Keep the lock', 'Upload']);
    await expect(dialog.getByRole('button')).toHaveCount(5); // the four, and the header's X
    await expect(page.getByTestId('speleodb-exit-choices')).toContainText('Keep the lock leaves them on this computer');
    const row = await page.getByTestId('speleodb-exit-footer').getByRole('button').evaluateAll(
      (buttons) => buttons.map((b) => Math.round(b.getBoundingClientRect().top)));
    expect(new Set(row).size, 'all four buttons on one line').toBe(1);
    await page.getByTestId('speleodb-exit-cancel').click();
    await expect(dialog).toBeHidden();
    await expect(page.getByTestId('single-system-view')).toBeVisible();
    expect(await calls(page, 'close_project')).toEqual([]);
  });

  test('Keep the lock saves unsaved edits, then closes without releasing', async ({ page }) => {
    await seedLockedWithChanges(page, { unsaved: true });
    await openMenuAndClose(page);
    await page.getByTestId('speleodb-exit-keep').click();
    await expect(page.getByTestId('speleodb-exit-dialog')).toBeHidden();
    await expect(page.getByTestId('single-system-view')).toBeHidden();
    const log = await calls(page);
    const save = log.findIndex((c) => c.cmd === 'save_project');
    const close = log.findIndex((c) => c.cmd === 'close_project');
    expect(save).toBeGreaterThanOrEqual(0);
    expect(close).toBeGreaterThan(save);
    expect(log[close]?.args).toEqual({ releaseLock: false, discardLocal: false });
    expect(log.some((c) => c.cmd === 'speleodb_done')).toBe(false);
  });

  test('Discard needs the "lost forever" confirmation, then discards the local copy', async ({ page }) => {
    await seedLockedWithChanges(page);
    await openMenuAndClose(page);
    await page.getByTestId('speleodb-exit-discard').click();
    await expect(page.getByTestId('speleodb-exit-discard-body')).toHaveText('Your changes will be lost forever.');
    expect(await calls(page, 'close_project')).toEqual([]);
    await page.getByTestId('speleodb-exit-discard-confirm').click();
    await expect(page.getByTestId('speleodb-exit-dialog')).toBeHidden();
    await expect(page.getByTestId('single-system-view')).toBeHidden();
    expect((await calls(page, 'close_project')).map((c) => c.args)).toEqual([{ releaseLock: true, discardLocal: true }]);
  });

  test('Upload runs the normal upload with the description, then closes', async ({ page }) => {
    await seedLockedWithChanges(page);
    await openMenuAndClose(page);
    await page.getByTestId('speleodb-exit-upload').click();
    await expect(page.getByTestId('speleodb-exit-upload-confirm')).toBeDisabled();
    await page.getByTestId('speleodb-exit-message').fill('Synthetic north passage');
    await page.getByTestId('speleodb-exit-upload-confirm').click();
    await expect(page.getByTestId('speleodb-exit-dialog')).toBeHidden();
    await expect(page.getByTestId('single-system-view')).toBeHidden();
    expect((await calls(page, 'speleodb_done')).map((c) => c.args?.message)).toEqual(['Synthetic north passage']);
    expect((await calls(page, 'close_project')).map((c) => c.args)).toEqual([{ releaseLock: true, discardLocal: false }]);
  });

  test('a failed upload stays in the project with the error and does not close', async ({ page }) => {
    await setIpcOverrides(page, { speleodb_done: { __throw: 'Synthetic network failure' } });
    await seedLockedWithChanges(page);
    await openMenuAndClose(page);
    await page.getByTestId('speleodb-exit-upload').click();
    await page.getByTestId('speleodb-exit-message').fill('m');
    await page.getByTestId('speleodb-exit-upload-confirm').click();
    await expect(page.getByTestId('speleodb-push-failure-inline')).toBeVisible();
    await expect(page.getByTestId('speleodb-exit-dialog')).toBeVisible();
    // Shown once: the shell banner behind the prompt waits its turn.
    await expect(page.getByTestId('speleo-banner')).toHaveCount(0);
    expect(await calls(page, 'close_project')).toEqual([]);
    await page.getByTestId('speleodb-exit-back').click();
    await page.getByTestId('speleodb-exit-cancel').click();
    await expect(page.getByTestId('single-system-view')).toBeVisible();
    // Left unresolved, the failure is back on the banner.
    await expect(page.getByTestId('speleo-banner')).toHaveAttribute('data-kind', 'push_failed');
  });

  test('a clean held lock closes silently (and releases as before)', async ({ page }) => {
    await seedLockedWithChanges(page);
    await page.evaluate(() => {
      const s = (window as unknown as { __E2E_TEST__: { speleoStore: { getState: () => { link: object }; setState: (s: unknown) => void } } }).__E2E_TEST__.speleoStore;
      s.setState({ link: { ...s.getState().link, hasUnpushedEdits: false } });
    });
    await openMenuAndClose(page);
    await expect(page.getByTestId('single-system-view')).toBeHidden();
    await expect(page.getByTestId('speleodb-exit-dialog')).toHaveCount(0);
    expect((await calls(page, 'close_project')).map((c) => c.args)).toEqual([{ releaseLock: true, discardLocal: false }]);
  });
});

test.describe('SpeleoDB exit prompt - quit (window X)', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Synthetic quit');
    await setIpcOverrides(page, { speleodb_done: { kind: 'released' } });
  });

  test('Cancel keeps the window and the project', async ({ page }) => {
    await seedLockedWithChanges(page);
    await pressWindowCloseButton(page);
    await expect(page.getByTestId('speleodb-exit-body')).toContainText('Stalagmight closes');
    await page.getByTestId('speleodb-exit-cancel').click();
    await expect(page.getByTestId('speleodb-exit-dialog')).toBeHidden();
    expect(await windowTeardownCounts(page)).toEqual({ destroy: 0, close: 0 });
    expect(await calls(page, 'close_project')).toEqual([]);
  });

  test('Keep the lock closes without releasing, then the quit goes on', async ({ page }) => {
    await seedLockedWithChanges(page);
    await pressWindowCloseButton(page);
    await page.getByTestId('speleodb-exit-keep').click();
    await expect.poll(() => windowTeardownCounts(page)).toMatchObject({ close: 1 });
    expect((await calls(page, 'close_project')).map((c) => c.args)).toEqual([{ releaseLock: false, discardLocal: false }]);
    // The mock records close(); deliver the event the native backend emits.
    await pressWindowCloseButton(page);
    await expect.poll(() => windowTeardownCounts(page)).toMatchObject({ destroy: 1 });
  });

  test('Discard + confirm discards the local copy, then quits', async ({ page }) => {
    await seedLockedWithChanges(page, { unsaved: true });
    await pressWindowCloseButton(page);
    await page.getByTestId('speleodb-exit-discard').click();
    await page.getByTestId('speleodb-exit-discard-confirm').click();
    await expect.poll(() => windowTeardownCounts(page)).toMatchObject({ close: 1 });
    expect((await calls(page, 'close_project')).map((c) => c.args)).toEqual([{ releaseLock: true, discardLocal: true }]);
    expect(await calls(page, 'save_project')).toEqual([]);
  });

  test('Upload uploads, closes, then quits', async ({ page }) => {
    await seedLockedWithChanges(page);
    await pressWindowCloseButton(page);
    await page.getByTestId('speleodb-exit-upload').click();
    await page.getByTestId('speleodb-exit-message').fill('Synthetic quit upload');
    await page.getByTestId('speleodb-exit-upload-confirm').click();
    await expect.poll(() => windowTeardownCounts(page)).toMatchObject({ close: 1 });
    expect((await calls(page, 'speleodb_done')).length).toBe(1);
    expect((await calls(page, 'close_project')).map((c) => c.args)).toEqual([{ releaseLock: true, discardLocal: false }]);
  });
});

test.describe('SpeleoDB exit prompt - switch', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Synthetic switch');
  });

  test('opening another file asks first; Keep then opens it', async ({ page }) => {
    await seedLockedWithChanges(page);
    const opening = page.evaluate(() => (window as unknown as {
      __E2E_TEST__: { routeOpenedFile: (p: string) => Promise<void> };
    }).__E2E_TEST__.routeOpenedFile('/synthetic/other.stalx'));
    await expect(page.getByTestId('speleodb-exit-body')).toContainText('open another project');
    await page.getByTestId('speleodb-exit-keep').click();
    await opening;
    expect((await calls(page, 'close_project')).map((c) => c.args)).toEqual([{ releaseLock: false, discardLocal: false }]);
  });

  test('opening another file: Cancel opens nothing', async ({ page }) => {
    await seedLockedWithChanges(page);
    const opening = page.evaluate(() => (window as unknown as {
      __E2E_TEST__: { routeOpenedFile: (p: string) => Promise<void> };
    }).__E2E_TEST__.routeOpenedFile('/synthetic/other.stalx'));
    await page.getByTestId('speleodb-exit-cancel').click();
    await opening;
    expect(await calls(page, 'close_project')).toEqual([]);
    await expect(page.getByTestId('single-system-view')).toBeVisible();
  });
});

/** Start a staged sign-out or account switch without awaiting it (the dialog is on screen). */
async function startAuthChange(page: Page, pending: Record<string, unknown>): Promise<void> {
  await page.evaluate(({ pending, endpoint }) => {
    const w = window as unknown as {
      __authResult?: unknown;
      __E2E_TEST__: { speleoStore: { getState: () => {
        setPendingAuth: (p: unknown) => void;
        applyPendingAuth: (e: string) => Promise<unknown>;
      } } };
    };
    w.__authResult = undefined;
    const store = w.__E2E_TEST__.speleoStore.getState();
    store.setPendingAuth(pending);
    void store.applyPendingAuth(endpoint).then((r) => { w.__authResult = r; });
  }, { pending, endpoint: ENDPOINT });
}

async function authResult(page: Page): Promise<unknown> {
  return page.evaluate(() => (window as unknown as { __authResult?: unknown }).__authResult);
}

const summary = (id: string, lockHolder: string | null) => ({
  id, name: `Synthetic ${id}`, permission: 'READ_AND_WRITE', lock_holder: lockHolder, type_tag: 'stalx', latest_commit_sha: null,
});
const localState = (unpushed: boolean) => ({
  has_local_copy: true, has_unpushed_edits: unpushed, last_edit_at: null, is_open: false, state_unknown: false,
});

test.describe('Sign-out never strands a lock or un-uploaded work', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Synthetic sign-out');
    await setIpcOverrides(page, { speleodb_done: { kind: 'released' } });
  });

  test('nothing held: signs out directly, no dialog', async ({ page }) => {
    await seedLockedWithChanges(page);
    await page.evaluate(() => {
      const s = (window as unknown as { __E2E_TEST__: { speleoStore: { setState: (s: unknown) => void } } }).__E2E_TEST__.speleoStore;
      s.setState({ link: null, remoteProjectId: null });
    });
    await setIpcOverrides(page, { speleodb_list_projects: [summary('a', null)], speleodb_local_project_states: { a: localState(false) } });
    await startAuthChange(page, { kind: 'logout' });
    await expect.poll(() => authResult(page)).toEqual({ ok: true });
    await expect(page.getByTestId('speleodb-signout-dialog')).toHaveCount(0);
    expect((await calls(page, 'speleodb_logout')).length).toBe(1);
  });

  test('open project with changes: Upload, then the other locks are released, then sign-out', async ({ page }) => {
    await seedLockedWithChanges(page);
    await setIpcOverrides(page, {
      speleodb_list_projects: [summary('synthetic-open', ME), summary('b', ME)],
      speleodb_local_project_states: { 'synthetic-open': { ...localState(true), is_open: true }, b: localState(false) },
    });
    await startAuthChange(page, { kind: 'logout' });
    const dialog = page.getByTestId('speleodb-signout-dialog');
    await expect(dialog).toBeVisible();
    await expect(page.getByTestId('speleodb-signout-releasable')).toContainText('Synthetic b');
    // Every answer is in the one footer row, Cancel first and the primary last.
    await expect(page.getByTestId('speleodb-signout-footer').getByRole('button')).toHaveText(
      ['Cancel', 'Discard', 'Keep on this computer', 'Upload and disconnect']);
    const row = await page.getByTestId('speleodb-signout-footer').getByRole('button').evaluateAll(
      (buttons) => buttons.map((b) => Math.round(b.getBoundingClientRect().top)));
    expect(new Set(row).size, 'all four buttons on one line').toBe(1);
    await page.getByTestId('speleodb-signout-open-upload').click();
    await expect(page.getByTestId('speleodb-signout-footer').getByRole('button')).toHaveText(['Back', 'Upload and disconnect']);
    await page.getByTestId('speleodb-signout-message').fill('Synthetic before sign-out');
    await page.getByTestId('speleodb-signout-open-upload-confirm').click();
    await expect.poll(() => authResult(page)).toEqual({ ok: true });
    await expect(dialog).toHaveCount(0);
    const log = await calls(page);
    const order = log.map((c) => c.cmd).filter((c) => ['speleodb_done', 'speleodb_release_lock', 'speleodb_logout'].includes(c));
    expect(order).toEqual(['speleodb_done', 'speleodb_release_lock', 'speleodb_logout']);
    expect(log.find((c) => c.cmd === 'speleodb_release_lock')?.args).toEqual({ projectId: 'b' });
  });

  // Decision 10: with the upload impossible, the edits are never discard-only.
  test('open project with changes: Keep on this computer hands the lock back and keeps the copy', async ({ page }) => {
    await seedLockedWithChanges(page);
    await setIpcOverrides(page, {
      speleodb_list_projects: [summary('synthetic-open', ME)],
      speleodb_local_project_states: { 'synthetic-open': { ...localState(true), is_open: true } },
    });
    await startAuthChange(page, { kind: 'logout' });
    await expect(page.getByTestId('speleodb-signout-dialog')).toBeVisible();
    await page.getByTestId('speleodb-signout-open-keep').click();
    await expect.poll(() => authResult(page)).toEqual({ ok: true });
    const log = await calls(page);
    const order = log.map((c) => c.cmd)
      .filter((c) => ['speleodb_done', 'close_project', 'speleodb_discard_cached_copy', 'speleodb_release_lock', 'speleodb_logout'].includes(c));
    // No upload, no close, no discard: the lock is handed back, then the sign-out.
    expect(order).toEqual(['speleodb_release_lock', 'speleodb_logout']);
    expect(log.find((c) => c.cmd === 'speleodb_release_lock')?.args).toEqual({ projectId: 'synthetic-open' });
    await expect(page.getByTestId('single-system-view')).toBeVisible();
  });

  test('another cached project with changes is kept on this computer, named only; its lock is handed back', async ({ page }) => {
    await seedLockedWithChanges(page);
    await page.evaluate(() => {
      const s = (window as unknown as { __E2E_TEST__: { speleoStore: { setState: (s: unknown) => void } } }).__E2E_TEST__.speleoStore;
      s.setState({ link: null, remoteProjectId: null });
    });
    await setIpcOverrides(page, {
      speleodb_list_projects: [summary('x', ME)],
      speleodb_local_project_states: { x: localState(true) },
    });
    await startAuthChange(page, { kind: 'logout' });
    const kept = page.getByTestId('speleodb-signout-unuploaded');
    await expect(kept).toContainText('open and upload them after you connect again');
    await expect(page.getByTestId('speleodb-signout-row-x')).toHaveText('Synthetic x');
    await expect(kept.getByRole('button')).toHaveCount(0);
    await expect(page.getByTestId('speleodb-signout-footer').getByRole('button')).toHaveText(['Cancel', 'Disconnect']);
    await page.getByTestId('speleodb-signout-go').click();
    await expect.poll(() => authResult(page)).toEqual({ ok: true });
    // Never discarded from the sign-out: only its lock is handed back.
    expect(await calls(page, 'speleodb_discard_cached_copy')).toEqual([]);
    expect((await calls(page, 'speleodb_release_lock')).map((c) => c.args)).toEqual([{ projectId: 'x' }]);
  });

  test('open project with changes: Discard (confirmed, the open project only) closes discarding it, then signs out', async ({ page }) => {
    await seedLockedWithChanges(page);
    await setIpcOverrides(page, {
      speleodb_list_projects: [summary('synthetic-open', ME), summary('x', ME)],
      speleodb_local_project_states: { 'synthetic-open': { ...localState(true), is_open: true }, x: localState(true) },
    });
    await startAuthChange(page, { kind: 'logout' });
    await page.getByTestId('speleodb-signout-open-discard').click();
    await expect(page.getByTestId('speleodb-signout-open-discard-body')).toContainText('lost forever');
    await expect(page.getByTestId('speleodb-signout-discard')).toContainText('Only this project is affected');
    await expect(page.getByTestId('speleodb-signout-footer').getByRole('button')).toHaveText(['Back', 'Discard and disconnect']);
    expect(await calls(page, 'close_project')).toEqual([]);
    await page.getByTestId('speleodb-signout-open-discard-confirm').click();
    await expect.poll(() => authResult(page)).toEqual({ ok: true });
    expect((await calls(page, 'close_project')).map((c) => c.args)).toEqual([{ releaseLock: true, discardLocal: true }]);
    expect(await calls(page, 'speleodb_discard_cached_copy')).toEqual([]);
    expect((await calls(page, 'speleodb_release_lock')).map((c) => c.args)).toEqual([{ projectId: 'x' }]);
  });

  test('a release that fails is reported; "Disconnect anyway" then signs out', async ({ page }) => {
    await seedLockedWithChanges(page);
    await page.evaluate(() => {
      const s = (window as unknown as { __E2E_TEST__: { speleoStore: { setState: (s: unknown) => void } } }).__E2E_TEST__.speleoStore;
      s.setState({ link: null, remoteProjectId: null });
    });
    await setIpcOverrides(page, {
      speleodb_list_projects: [summary('a', ME)],
      speleodb_local_project_states: { a: localState(false) },
      speleodb_release_lock: { __throw: 'Synthetic offline' },
    });
    await startAuthChange(page, { kind: 'logout' });
    await page.getByTestId('speleodb-signout-go').click();
    await expect(page.getByTestId('speleodb-signout-release-failed')).toContainText('Synthetic a');
    expect(await calls(page, 'speleodb_logout')).toEqual([]);
    await page.getByTestId('speleodb-signout-anyway').click();
    await expect.poll(() => authResult(page)).toEqual({ ok: true });
    expect((await calls(page, 'speleodb_logout')).length).toBe(1);
  });

  test('an account switch hands the previous account\'s clean lock back before logging in', async ({ page }) => {
    await seedLockedWithChanges(page);
    await page.evaluate(() => {
      const s = (window as unknown as { __E2E_TEST__: { speleoStore: { getState: () => { link: object }; setState: (s: unknown) => void } } }).__E2E_TEST__.speleoStore;
      s.setState({ link: { ...s.getState().link, hasUnpushedEdits: false } });
    });
    await setIpcOverrides(page, {
      speleodb_list_projects: [summary('synthetic-open', ME)],
      speleodb_local_project_states: { 'synthetic-open': { ...localState(false), is_open: true } },
    });
    await startAuthChange(page, { kind: 'login', email: 'someone.else@example.com', password: 'synthetic', token: '', useToken: false });
    await expect(page.getByTestId('speleodb-signout-go')).toHaveText('Switch account');
    await page.getByTestId('speleodb-signout-go').click();
    await expect.poll(() => authResult(page)).toEqual({ ok: true });
    const order = (await calls(page)).map((c) => c.cmd).filter((c) => ['speleodb_release_lock', 'speleodb_login'].includes(c));
    expect(order).toEqual(['speleodb_release_lock', 'speleodb_login']);
  });
});

/** Settings on its SpeleoDB tab, as the account menu and every sign-in link open it. */
async function openSpeleoSettings(page: Page): Promise<void> {
  await page.evaluate(() => {
    (window as unknown as {
      __E2E_TEST__: { uiStore: { getState: () => { openDialog: (name: string, data: unknown) => void } } };
    }).__E2E_TEST__.uiStore.getState().openDialog('settings', { initialCategory: 'speleodb' });
  });
  await expect(page.getByTestId('settings-panel-speleodb')).toBeVisible();
}

test.describe('Disconnect in Settings runs the sign-out flow first', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Synthetic disconnect');
    await setIpcOverrides(page, { speleodb_done: { kind: 'released' } });
  });

  test('un-uploaded work: Cancel stays connected in Settings; Keep on this computer then disconnects', async ({ page }) => {
    await seedLockedWithChanges(page);
    await setIpcOverrides(page, {
      // Settings asks the backend who is signed in when it opens.
      speleodb_session: { logged_in: true, user_email: ME, endpoint: ENDPOINT },
      speleodb_list_projects: [summary('synthetic-open', ME)],
      speleodb_local_project_states: { 'synthetic-open': { ...localState(true), is_open: true } },
    });
    await openSpeleoSettings(page);
    const settings = page.getByTestId('settings-dialog');
    const signedIn = page.getByTestId('speleodb-account-signed-in');
    await expect(signedIn).toContainText(`Connected to ${ENDPOINT} as ${ME}`);

    // Never a direct logout: the sign-out dialog comes first, over Settings.
    await page.getByTestId('speleodb-settings-logout').click();
    const dialog = page.getByTestId('speleodb-signout-dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText('Before you disconnect');
    await expect(page.getByTestId('speleodb-signout-open')).toBeVisible();
    await expect(page.getByTestId('speleodb-signout-footer').getByRole('button')).toHaveText(
      ['Cancel', 'Discard', 'Keep on this computer', 'Upload and disconnect']);

    // Cancel: still connected, Settings still open on the SpeleoDB tab.
    await page.getByTestId('speleodb-signout-cancel').click();
    await expect(dialog).toHaveCount(0);
    await expect(settings).toBeVisible();
    await expect(signedIn).toBeVisible();
    await expect(page.getByTestId('speleodb-settings-logout')).toBeEnabled();
    expect(await calls(page, 'speleodb_logout')).toEqual([]);
    expect(await calls(page, 'speleodb_release_lock')).toEqual([]);

    // Again, keeping the changes on this computer: the lock is handed back, then
    // the session ends, and Settings stays open with the Connect form.
    await page.getByTestId('speleodb-settings-logout').click();
    await expect(dialog).toBeVisible();
    await setIpcOverrides(page, { speleodb_session: { logged_in: false, user_email: null, endpoint: null } });
    await page.getByTestId('speleodb-signout-open-keep').click();
    await expect(dialog).toHaveCount(0);
    await expect.poll(async () => (await calls(page, 'speleodb_logout')).length).toBe(1);
    const order = (await calls(page)).map((c) => c.cmd)
      .filter((c) => ['speleodb_done', 'speleodb_discard_cached_copy', 'speleodb_release_lock', 'speleodb_logout'].includes(c));
    expect(order).toEqual(['speleodb_release_lock', 'speleodb_logout']);
    await expect(settings).toBeVisible();
    await expect(page.getByTestId('speleodb-account-login-form')).toBeVisible();
    await expect(page.getByTestId('single-system-view')).toBeVisible();
  });

  test('a failed hand-back offers Try again / Disconnect anyway, still over Settings', async ({ page }) => {
    await seedLockedWithChanges(page);
    await page.evaluate(() => {
      const s = (window as unknown as { __E2E_TEST__: { speleoStore: { setState: (s: unknown) => void } } }).__E2E_TEST__.speleoStore;
      s.setState({ link: null, remoteProjectId: null });
    });
    await setIpcOverrides(page, {
      speleodb_session: { logged_in: true, user_email: ME, endpoint: ENDPOINT },
      speleodb_list_projects: [summary('a', ME)],
      speleodb_local_project_states: { a: localState(false) },
      speleodb_release_lock: { __throw: 'Synthetic offline' },
    });
    await openSpeleoSettings(page);
    await page.getByTestId('speleodb-settings-logout').click();
    await expect(page.getByTestId('speleodb-signout-releasable')).toContainText('Synthetic a');
    await page.getByTestId('speleodb-signout-go').click();
    await expect(page.getByTestId('speleodb-signout-release-failed')).toContainText('Synthetic a');
    await expect(page.getByTestId('speleodb-signout-footer').getByRole('button')).toHaveText(['Cancel', 'Disconnect anyway', 'Try again']);
    expect(await calls(page, 'speleodb_logout')).toEqual([]);
    await page.getByTestId('speleodb-signout-anyway').click();
    await expect.poll(async () => (await calls(page, 'speleodb_logout')).length).toBe(1);
    await expect(page.getByTestId('settings-dialog')).toBeVisible();
  });
});
