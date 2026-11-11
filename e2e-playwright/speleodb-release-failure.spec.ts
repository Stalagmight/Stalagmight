import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage, createProject, openAppMenu, waitForNoPendingQuit } from './helpers/app';
import { clearIpcOverrideKeys, setIpcOverrides } from './helpers/ipcOverrides';

/**
 * "Couldn't hand back the lock", end to end on the mocked IPC. Synthetic projects
 * only.
 *
 * A clean SpeleoDB project we hold the lock on is left by Close, a project switch
 * or a quit only once its lock is settled. When the backend reports the release
 * as failed or uncertain (`lock_release: 'failed'`), nothing leaves silently: the
 * in-app dialog offers Retry, Close and keep the lock, or Cancel (stay, read-only).
 * The backend calls are read back from `__MOCK_EXIT_CALLS__` (mocks/tauri-ipc.ts).
 */

const ME = 'diver@example.com';
const FAILED = {
  success: false,
  warnings: [],
  error: 'The SpeleoDB lock could not be handed back: Network error talking to SpeleoDB: synthetic timeout',
  section_ids: [],
  lock_release: 'failed',
};
const RELEASED = { success: true, warnings: [], error: null, section_ids: [], lock_release: 'released' };

type Call = { cmd: string; args: Record<string, unknown> | null };

async function calls(page: Page, cmd: string): Promise<Call[]> {
  const all = await page.evaluate(
    () => ((window as unknown as { __MOCK_EXIT_CALLS__?: Call[] }).__MOCK_EXIT_CALLS__ ?? []),
  );
  return all.filter((c) => c.cmd === cmd);
}

/** Seed "we hold the lock" on a clean SpeleoDB project: no exit prompt applies. */
async function seedCleanLock(page: Page): Promise<void> {
  await page.evaluate((me) => {
    const w = window as unknown as {
      __speleoLinkStatusOverrides?: Record<string, unknown>;
      __MOCK_EXIT_CALLS__?: unknown[];
      __E2E_TEST__: { speleoStore: { setState: (s: unknown) => void } };
    };
    w.__speleoLinkStatusOverrides = {
      is_remote: true, edit_enabled: true, has_unpushed_edits: false,
      permission: 'READ_AND_WRITE', lock_holder: me, remote_state: 'up_to_date',
    };
    w.__E2E_TEST__.speleoStore.setState({
      session: { loggedIn: true, userEmail: me, endpoint: 'https://synthetic.example' },
      remoteProjectId: 'synthetic-open',
      projectLinkKind: 'remote',
      busy: false,
      link: {
        permission: 'READ_AND_WRITE', editState: 'editing', lockHolder: me, lockedByOther: false,
        hasUnpushedEdits: false, unwound: false, undoMode: 'full', remoteState: 'up_to_date',
        noAccess: false, serverError: null,
      },
    });
    w.__MOCK_EXIT_CALLS__ = [];
  }, ME);
}

async function closeFromMenu(page: Page): Promise<void> {
  await openAppMenu(page);
  await page.getByTestId('back-button').click();
}

/** The OS window X: what the real backend emits after preventing the close. */
async function pressWindowCloseButton(page: Page): Promise<void> {
  await page.evaluate(async () => {
    await (window as unknown as {
      __TAURI_INTERNALS__?: { invoke: (cmd: string, args: unknown) => Promise<unknown> };
    }).__TAURI_INTERNALS__?.invoke('plugin:event|emit', { event: 'tauri://close-requested', payload: null });
  });
}

/**
 * The app's dialog layout: the body says what each answer does, and every button
 * is in the one right-aligned footer row, Cancel first and Retry (primary) last.
 */
async function expectFooterRow(page: Page): Promise<void> {
  const footer = page.getByTestId('speleodb-release-failed-footer');
  await expect(footer.getByRole('button')).toHaveText(['Cancel', 'Close and keep the lock', 'Retry']);
  await expect(page.getByTestId('speleodb-release-failed-dialog').getByRole('button'))
    .toHaveCount(4); // the three above and the header's X
  await expect(page.getByTestId('speleodb-release-failed-retry')).toHaveAttribute('data-primary-action', '');
  await expect(page.getByTestId('speleodb-release-failed-choices')).toContainText('Close and keep the lock leaves');
  // One row: every button sits on the same line.
  const tops = await footer.getByRole('button').evaluateAll((els) => els.map((el) => el.getBoundingClientRect().top));
  expect(new Set(tops).size).toBe(1);
}

async function teardown(page: Page): Promise<{ destroy: number; close: number }> {
  return page.evaluate(() => (window as unknown as {
    __MOCK_GET_WINDOW_TEARDOWN_COUNTS__: () => { destroy: number; close: number };
  }).__MOCK_GET_WINDOW_TEARDOWN_COUNTS__());
}

test.describe('Close Project with a failing lock release', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Synthetic release close');
    await seedCleanLock(page);
    await setIpcOverrides(page, { close_project: FAILED });
  });

  test('asks instead of closing; Cancel stays in the project, read-only', async ({ page }) => {
    // What the backend reports once the failed release has shut its edit gate (the
    // claim is kept unbound): the status refresh the failure triggers reads this.
    await page.evaluate(() => {
      const w = window as unknown as { __speleoLinkStatusOverrides: Record<string, unknown> };
      w.__speleoLinkStatusOverrides = { ...w.__speleoLinkStatusOverrides, edit_enabled: false };
    });
    await closeFromMenu(page);
    const dialog = page.getByTestId('speleodb-release-failed-dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog).toContainText("Couldn't hand back the lock");
    await expect(page.getByTestId('speleodb-release-failed-body')).toContainText('you close the project');
    await expect(page.getByTestId('speleodb-release-failed-reason')).toContainText('synthetic timeout');
    await expectFooterRow(page);
    // #668's error dialog no longer stands in for this question.
    await expect(page.getByTestId('error-dialog')).toHaveCount(0);
    await page.getByTestId('speleodb-release-failed-cancel').click();
    await expect(dialog).toHaveCount(0);
    await expect(page.getByTestId('single-system-view')).toBeVisible();
    await expect(page.getByTestId('error-dialog')).toHaveCount(0);
    expect(await page.evaluate(() => (window as unknown as {
      __E2E_TEST__: { speleoStore: { getState: () => { link: { editState: string } | null } } };
    }).__E2E_TEST__.speleoStore.getState().link?.editState)).toBe('read-only');
    expect((await calls(page, 'close_project')).map((c) => c.args)).toEqual([{ releaseLock: true, discardLocal: false }]);
  });

  test('Retry closes once the release goes through', async ({ page }) => {
    await closeFromMenu(page);
    await expect(page.getByTestId('speleodb-release-failed-dialog')).toBeVisible();
    // Still failing: the question stays, with the reason.
    await page.getByTestId('speleodb-release-failed-retry').click();
    await expect(page.getByTestId('speleodb-release-failed-retry')).toBeEnabled();
    await expect(page.getByTestId('speleodb-release-failed-dialog')).toBeVisible();
    await setIpcOverrides(page, { close_project: RELEASED });
    await page.getByTestId('speleodb-release-failed-retry').click();
    await expect(page.getByTestId('speleodb-release-failed-dialog')).toHaveCount(0);
    await expect(page.getByTestId('global-view')).toBeVisible();
    expect((await calls(page, 'close_project')).map((c) => c.args)).toEqual([
      { releaseLock: true, discardLocal: false },
      { releaseLock: true, discardLocal: false },
      { releaseLock: true, discardLocal: false },
    ]);
  });

  test('Close and keep the lock closes without releasing', async ({ page }) => {
    await closeFromMenu(page);
    await expect(page.getByTestId('speleodb-release-failed-dialog')).toBeVisible();
    await clearIpcOverrideKeys(page, ['close_project']);
    await page.getByTestId('speleodb-release-failed-keep').click();
    await expect(page.getByTestId('global-view')).toBeVisible();
    expect((await calls(page, 'close_project')).map((c) => c.args)).toEqual([
      { releaseLock: true, discardLocal: false },
      { releaseLock: false, discardLocal: false },
    ]);
  });
});

test.describe('Quit with a failing lock release (window X)', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Synthetic release quit');
    await seedCleanLock(page);
    await setIpcOverrides(page, { speleodb_settle_open_lock: FAILED });
  });

  test('asks before quitting; Cancel keeps the window and the project', async ({ page }) => {
    await pressWindowCloseButton(page);
    await expect(page.getByTestId('speleodb-release-failed-dialog')).toBeVisible();
    await expect(page.getByTestId('speleodb-release-failed-body')).toContainText('Stalagmight closes');
    await expectFooterRow(page);
    await page.getByTestId('speleodb-release-failed-cancel').click();
    await expect(page.getByTestId('speleodb-release-failed-dialog')).toHaveCount(0);
    // Cancel has been carried out and nothing in the quit gate can re-issue the quit.
    await waitForNoPendingQuit(page);
    expect(await teardown(page)).toEqual({ destroy: 0, close: 0 });
    await expect(page.getByTestId('single-system-view')).toBeVisible();
    expect((await calls(page, 'speleodb_settle_open_lock')).map((c) => c.args)).toEqual([{ releaseLock: true }]);
  });

  test('Close and keep the lock keeps it on purpose, then quits', async ({ page }) => {
    await pressWindowCloseButton(page);
    await expect(page.getByTestId('speleodb-release-failed-dialog')).toBeVisible();
    await clearIpcOverrideKeys(page, ['speleodb_settle_open_lock']);
    await page.getByTestId('speleodb-release-failed-keep').click();
    await expect.poll(async () => (await teardown(page)).close).toBe(1);
    expect((await calls(page, 'speleodb_settle_open_lock')).map((c) => c.args)).toEqual([
      { releaseLock: true }, { releaseLock: false },
    ]);
    // The mock records close(); deliver the event the native backend emits.
    await pressWindowCloseButton(page);
    await expect.poll(async () => (await teardown(page)).destroy).toBe(1);
  });

  test('Retry quits once the release goes through', async ({ page }) => {
    await pressWindowCloseButton(page);
    await expect(page.getByTestId('speleodb-release-failed-dialog')).toBeVisible();
    expect(await teardown(page)).toEqual({ destroy: 0, close: 0 });
    await setIpcOverrides(page, { speleodb_settle_open_lock: RELEASED });
    await page.getByTestId('speleodb-release-failed-retry').click();
    await expect.poll(async () => (await teardown(page)).close).toBe(1);
    await pressWindowCloseButton(page);
    await expect.poll(async () => (await teardown(page)).destroy).toBe(1);
  });
});

test('switching to another file with a failing release asks first; Cancel opens nothing', async ({ page }) => {
  await setupMockedPage(page);
  await createProject(page, 'Synthetic release switch');
  await seedCleanLock(page);
  await setIpcOverrides(page, { speleodb_settle_open_lock: FAILED });
  const opening = page.evaluate(() => (window as unknown as {
    __E2E_TEST__: { projectStore: { getState: () => { loadProject: (p: string, o: unknown) => Promise<unknown> } } };
  }).__E2E_TEST__.projectStore.getState().loadProject('/synthetic/other.stalx', { accessConfirmed: true }));
  await expect(page.getByTestId('speleodb-release-failed-dialog')).toBeVisible();
  await expect(page.getByTestId('speleodb-release-failed-body')).toContainText('you open another project');
  await expectFooterRow(page);
  await page.getByTestId('speleodb-release-failed-cancel').click();
  expect(await opening).toMatchObject({ success: false });
  await expect(page.getByTestId('single-system-view')).toBeVisible();
});
