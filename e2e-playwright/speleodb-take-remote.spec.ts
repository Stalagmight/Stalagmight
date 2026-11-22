import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage } from './helpers/app';

/**
 * Take the SpeleoDB version keeps the lock (demo item 2), end to end on the
 * mocked IPC. Synthetic projects only.
 *
 * SpeleoDB's lock is the account's, so the same account signed in on another
 * computer edits under it too. Take remote used to hand it back, which left that
 * other session editing with no lock on the server. The backend now releases
 * nothing: an edit session carries over the reload and goes on editing the server
 * version, a read-only one stays read-only (data#3: only a bound claim carries).
 * The overrides below model that backend - a lock taken by Start Editing and kept
 * by Take remote, with the link status derived from it - and this pins that the
 * app follows it, and that the confirmation says what happens to the lock.
 */

const PATH = '/projects/synthetic-take-remote.stalx';

async function openRemote(page: Page, opts: { editing: boolean }): Promise<void> {
  await setupMockedPage(page);
  await page.evaluate(async ({ path, editing }) => {
    const w = window as unknown as {
      __MOCK_SET_PROJECT_IS_REMOTE__: (remote: boolean) => void;
      __MOCK_IPC_OVERRIDES__: Record<string, unknown>;
      __lockCalls: string[];
      __E2E_TEST__: {
        speleoStore: { getState: () => { refreshSession: () => Promise<unknown> } };
        projectStore: { getState: () => { loadProject: (path: string, opts?: unknown) => Promise<unknown> } };
      };
    };
    w.__MOCK_SET_PROJECT_IS_REMOTE__(true);
    w.__lockCalls = [];
    // The server's lock, as this account sees it: what the edit gate is made of.
    let held = false;
    let unpushed = true;
    let conflict = true;
    const overrides: Record<string, unknown> = {
      speleodb_session: { logged_in: true, user_email: 'me@example.com', endpoint: 'https://example.com' },
      speleodb_get_revisions: [],
      speleodb_project_access_at: { release_context: { project_id: 'remote', endpoint: 'https://example.com' } },
    };
    Object.defineProperty(overrides, 'speleodb_start_editing', { get: () => {
      w.__lockCalls.push('acquire');
      held = true;
      return { kind: 'editing' };
    } });
    Object.defineProperty(overrides, 'speleodb_take_remote', { get: () => {
      w.__lockCalls.push('take_remote');
      unpushed = false;
      conflict = false;
      return path;
    } });
    for (const release of ['speleodb_release_lock', 'speleodb_release_current_lock']) {
      Object.defineProperty(overrides, release, { get: () => {
        w.__lockCalls.push(release);
        held = false;
        return null;
      } });
    }
    Object.defineProperty(overrides, 'speleodb_link_status', { get: () => ({
      is_remote: true, edit_enabled: held,
      permission: 'READ_AND_WRITE', lock_holder: held ? 'me@example.com' : null,
      has_unpushed_edits: unpushed, unwound: false, remote_state: conflict ? 'conflict' : 'up_to_date',
      access_denied: false, server_error: null, exclude_geojson: false, undo_mode: 'full',
      project_id: 'remote',
    }) });
    w.__MOCK_IPC_OVERRIDES__ = overrides;
    await w.__E2E_TEST__.speleoStore.getState().refreshSession();
    await w.__E2E_TEST__.projectStore.getState().loadProject(path, editing ? undefined : { acquireLock: false });
  }, { path: PATH, editing: opts.editing });
}

async function editState(page: Page): Promise<string | undefined> {
  return page.evaluate(() => (window as unknown as {
    __E2E_TEST__: { speleoStore: { getState: () => { link: { editState?: string } | null } } };
  }).__E2E_TEST__.speleoStore.getState().link?.editState);
}

async function lockCalls(page: Page): Promise<string[]> {
  return page.evaluate(() => (window as unknown as { __lockCalls: string[] }).__lockCalls);
}

/** A hand-off verb reported a conflict: the resolution dialog opens; confirm Take remote. */
async function takeRemote(page: Page, lockLine: string): Promise<void> {
  await page.evaluate(() => (window as unknown as {
    __E2E_TEST__: { speleoStore: { setState: (s: unknown) => void } };
  }).__E2E_TEST__.speleoStore.setState({ divergence: 'done' }));
  const dialog = page.getByTestId('speleodb-divergence-dialog');
  await expect(dialog).toBeVisible();

  await page.getByTestId('speleodb-divergence-take-remote').click();
  await expect(page.getByTestId('speleodb-take-remote-warning')).toBeVisible();
  await expect(page.getByTestId('speleodb-take-remote-lock')).toContainText(lockLine);
  await page.getByTestId('speleodb-take-remote-destroy').click();
  await expect(dialog).toBeHidden();
}

test('Take remote keeps the lock and goes on editing the server version', async ({ page }) => {
  await openRemote(page, { editing: true });
  await expect.poll(() => editState(page)).toBe('editing');

  await takeRemote(page, 'You keep the write lock and go on editing');

  await expect.poll(() => editState(page)).toBe('editing');
  await expect(page.getByTestId('speleodb-readonly-resume')).toBeHidden();
  expect(await lockCalls(page)).toEqual(['acquire', 'take_remote']);
});

test('Take remote from a read-only project leaves the lock alone and stays read-only', async ({ page }) => {
  await openRemote(page, { editing: false });
  await expect.poll(() => editState(page)).toBe('read-only');

  await takeRemote(page, 'The write lock is left as it is');

  await expect.poll(() => editState(page)).toBe('read-only');
  await expect(page.getByTestId('speleodb-readonly-resume')).toBeVisible();
  expect(await lockCalls(page)).toEqual(['take_remote']);
});
