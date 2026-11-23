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
 *
 * The edit session is the lock BOUND to the open project's session, and every
 * project switch moves that session on: `cancel_loading`, which `loadProject`
 * sends first, ends the edit session while the lock stays held on SpeleoDB
 * (`AppState::cancel_loading`). The model keeps that, because it is how the real
 * app lost the edit session after an in-place reload (sdbdemo2 issues 1 and 2:
 * the frontend reconciled the reload through `loadProject`).
 */

const PATH = '/projects/synthetic-take-remote.stalx';

async function openRemote(page: Page, opts: { editing: boolean; behind?: boolean }): Promise<void> {
  await setupMockedPage(page);
  await page.evaluate(async ({ path, editing, behind }) => {
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
    // The server's lock, as this account sees it, and whether this session's
    // edit gate is bound to it: the edit gate is both.
    let held = false;
    let bound = false;
    let unpushed = !behind;
    let conflict = !behind;
    let ahead = !!behind;
    const overrides: Record<string, unknown> = {
      speleodb_session: { logged_in: true, user_email: 'me@example.com', endpoint: 'https://example.com' },
      speleodb_get_revisions: [],
      speleodb_project_access_at: { release_context: { project_id: 'remote', endpoint: 'https://example.com' } },
    };
    Object.defineProperty(overrides, 'speleodb_start_editing', { get: () => {
      w.__lockCalls.push('acquire');
      held = true;
      bound = true;
      if (ahead) {
        // Resume editing on a copy behind SpeleoDB: the newer revision is
        // downloaded and reopened in place first, under the session it binds.
        ahead = false;
        return { kind: 'fast_forwarded', local_path: path };
      }
      return { kind: 'editing' };
    } });
    // A project switch moves the backend's session on: the edit session ends,
    // the lock stays held on SpeleoDB.
    Object.defineProperty(overrides, 'cancel_loading', { get: () => {
      bound = false;
      return null;
    } });
    Object.defineProperty(overrides, 'speleodb_take_remote', { get: () => {
      w.__lockCalls.push('take_remote');
      unpushed = false;
      conflict = false;
      // The backend reads the holder before the reload: an edit session goes on
      // only over a lock SpeleoDB still records (farm finding
      // P2-take-remote-editable).
      if (bound && !held) {
        bound = false;
        return { local_path: path, editing_stopped: { kind: 'released' } };
      }
      return { local_path: path };
    } });
    // Another session of the same account hands the lock back on the server (its
    // Close): this session is not told.
    (w as unknown as { __releaseElsewhere: () => void }).__releaseElsewhere = () => { held = false; };
    for (const release of ['speleodb_release_lock', 'speleodb_release_current_lock']) {
      Object.defineProperty(overrides, release, { get: () => {
        w.__lockCalls.push(release);
        held = false;
        bound = false;
        return null;
      } });
    }
    Object.defineProperty(overrides, 'speleodb_link_status', { get: () => ({
      is_remote: true, edit_enabled: held && bound,
      permission: 'READ_AND_WRITE', lock_holder: held ? 'me@example.com' : null,
      has_unpushed_edits: unpushed, unwound: false,
      remote_state: conflict ? 'conflict' : ahead ? 'remote_ahead' : 'up_to_date',
      access_denied: false, server_error: null, exclude_geojson: false, undo_mode: 'full',
      project_id: 'remote',
    }) });
    w.__MOCK_IPC_OVERRIDES__ = overrides;
    await w.__E2E_TEST__.speleoStore.getState().refreshSession();
    await w.__E2E_TEST__.projectStore.getState().loadProject(path, editing ? undefined : { acquireLock: false });
  }, { path: PATH, editing: opts.editing, behind: opts.behind ?? false });
}

async function editState(page: Page): Promise<string | undefined> {
  return page.evaluate(() => (window as unknown as {
    __E2E_TEST__: { speleoStore: { getState: () => { link: { editState?: string } | null } } };
  }).__E2E_TEST__.speleoStore.getState().link?.editState);
}

async function busy(page: Page): Promise<boolean> {
  return page.evaluate(() => (window as unknown as {
    __E2E_TEST__: { speleoStore: { getState: () => { busy: boolean } } };
  }).__E2E_TEST__.speleoStore.getState().busy);
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
  await expect(page.getByTestId('speleodb-read-only-dialog')).toBeHidden();
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

// sdbdemo2 issue 2: the tab promised "Resume editing downloads it first". One
// click downloaded the newer revision and took the lock, but the project stayed
// read-only with no message, and a second click started editing.
test('one Resume editing click on a copy behind SpeleoDB downloads it and starts editing', async ({ page }) => {
  await openRemote(page, { editing: false, behind: true });
  await expect.poll(() => editState(page)).toBe('read-only');

  await page.getByTestId('speleodb-readonly-resume').click();
  // Settled, not a passing moment: the result reads "editing" before the
  // reload's status re-read can say otherwise.
  await expect.poll(() => busy(page)).toBe(false);

  expect(await editState(page)).toBe('editing');
  await expect(page.getByTestId('speleodb-readonly-resume')).toBeHidden();
  await expect(page.getByTestId('speleodb-read-only-dialog')).toBeHidden();
  expect(await lockCalls(page)).toEqual(['acquire']);
});

// Farm finding P2-take-remote-editable (seed 950411): another session of the
// same account handed the lock back on the server, and Take the SpeleoDB version
// came back editable with no lock on SpeleoDB. It now opens read-only and says
// editing stopped, and why.
test('Take remote over a lock handed back elsewhere opens read-only and says editing stopped', async ({ page }) => {
  await openRemote(page, { editing: true });
  await expect.poll(() => editState(page)).toBe('editing');
  await page.evaluate(() => (window as unknown as { __releaseElsewhere: () => void }).__releaseElsewhere());

  await takeRemote(page, 'You keep the write lock and go on editing');

  const notice = page.getByTestId('speleodb-read-only-dialog');
  await expect(notice).toBeVisible();
  await expect(notice).toContainText('Editing stopped');
  await expect(notice).toContainText('handed back elsewhere');
  await expect(notice).toContainText('The SpeleoDB version is open read-only');
  await expect(notice).not.toContainText('kept on this computer');
  expect(await editState(page)).toBe('read-only');
  expect(await lockCalls(page)).toEqual(['acquire', 'take_remote']);
});
