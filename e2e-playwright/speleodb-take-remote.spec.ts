import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage } from './helpers/app';

/**
 * Take remote ends the edit session (data#3), end to end on the mocked IPC.
 * Synthetic projects only.
 *
 * The backend's edit gate is the lock claim: Take remote releases the lock, so
 * the server version it reloads is read-only. It used to come back "editing"
 * with no lock held. The overrides below model that backend - a lock taken by
 * Start Editing and handed back by Take remote, with the link status derived
 * from it - and this pins that the app follows it: the conflict dialog closes,
 * the project is read-only, and Resume editing is offered.
 */

const PATH = '/projects/synthetic-take-remote.stalx';

async function openRemoteEditing(page: Page): Promise<void> {
  await setupMockedPage(page);
  await page.evaluate(async (path) => {
    const w = window as unknown as {
      __MOCK_SET_PROJECT_IS_REMOTE__: (remote: boolean) => void;
      __MOCK_IPC_OVERRIDES__: Record<string, unknown>;
      __lockCalls: string[];
      __E2E_TEST__: {
        speleoStore: { getState: () => { refreshSession: () => Promise<unknown> } };
        projectStore: { getState: () => { loadProject: (path: string) => Promise<unknown> } };
      };
    };
    w.__MOCK_SET_PROJECT_IS_REMOTE__(true);
    w.__lockCalls = [];
    // The server's lock, as this account sees it: what the edit gate is made of.
    let held = false;
    let unpushed = true;
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
      held = false;
      unpushed = false;
      return path;
    } });
    Object.defineProperty(overrides, 'speleodb_link_status', { get: () => ({
      is_remote: true, edit_enabled: held,
      permission: 'READ_AND_WRITE', lock_holder: held ? 'me@example.com' : null,
      has_unpushed_edits: unpushed, unwound: false, remote_state: held ? 'conflict' : 'up_to_date',
      access_denied: false, server_error: null, exclude_geojson: false, undo_mode: 'full',
      project_id: 'remote',
    }) });
    w.__MOCK_IPC_OVERRIDES__ = overrides;
    await w.__E2E_TEST__.speleoStore.getState().refreshSession();
    await w.__E2E_TEST__.projectStore.getState().loadProject(path);
  }, PATH);
}

async function editState(page: Page): Promise<string | undefined> {
  return page.evaluate(() => (window as unknown as {
    __E2E_TEST__: { speleoStore: { getState: () => { link: { editState?: string } | null } } };
  }).__E2E_TEST__.speleoStore.getState().link?.editState);
}

test('Take remote takes the server version and leaves the project read-only', async ({ page }) => {
  await openRemoteEditing(page);
  await expect.poll(() => editState(page)).toBe('editing');

  // A hand-off verb reported a conflict: the resolution dialog opens.
  await page.evaluate(() => (window as unknown as {
    __E2E_TEST__: { speleoStore: { setState: (s: unknown) => void } };
  }).__E2E_TEST__.speleoStore.setState({ divergence: 'done' }));
  const dialog = page.getByTestId('speleodb-divergence-dialog');
  await expect(dialog).toBeVisible();

  await page.getByTestId('speleodb-divergence-take-remote').click();
  await expect(page.getByTestId('speleodb-take-remote-warning')).toBeVisible();
  await page.getByTestId('speleodb-take-remote-destroy').click();

  await expect(dialog).toBeHidden();
  await expect.poll(() => editState(page)).toBe('read-only');
  await expect(page.getByTestId('speleodb-readonly-resume')).toBeVisible();
  expect(await page.evaluate(() => (window as unknown as { __lockCalls: string[] }).__lockCalls))
    .toEqual(['acquire', 'take_remote']);
});
