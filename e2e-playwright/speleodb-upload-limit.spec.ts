/**
 * E2E: SpeleoDB upload size and revision times (review items sdk#3, sdk#7).
 *
 * - An upload over the server's size limit that fits once its undo history is left
 *   out lands, and the success toast becomes a warning saying so.
 * - An upload too large even without history is refused before sending, and the
 *   Save dialog shows the clear "too large" reason.
 * - Revision times (a UTC instant, or an older server's New York wall time with
 *   no zone) show as the same local time with the GMT offset stated.
 *
 * Parallel-safe: all state is per-page (mocked IPC + `window.__E2E_TEST__` stores).
 */
import { test, expect, type Page } from './fixtures/coverage';
import { createProject, setupMockedPage } from './helpers/app';
import type { useSpeleoStore } from '../src/store/speleoStore';

type TestWindow = Window & {
  __E2E_TEST__: Record<string, { getState: () => Record<string, unknown>; setState: (s: unknown) => void }> & {
    speleoStore: typeof useSpeleoStore;
  };
  __MOCK_IPC_OVERRIDES__: Record<string, unknown>;
  __MOCK_SET_PROJECT_IS_REMOTE__: (remote: boolean) => void;
};

const LINK_STATUS = {
  is_remote: true, project_id: 'synthetic-upload', edit_enabled: true, permission: 'READ_AND_WRITE',
  has_unpushed_edits: true, unwound: false, remote_state: 'up_to_date', lock_holder: 'test@example.com',
  access_denied: false, server_error: null, exclude_geojson: false, undo_mode: 'full',
};

/** A remote project held for editing with un-uploaded work; `push` answers the Save. */
async function editingRemoteProject(page: Page, push: 'without_history' | 'too_large'): Promise<void> {
  await setupMockedPage(page);
  await createProject(page, 'Synthetic large upload');
  await page.evaluate(({ push, linkStatus }) => {
    const w = window as unknown as TestWindow;
    w.__MOCK_SET_PROJECT_IS_REMOTE__(true);
    const overrides: Record<string, unknown> = { speleodb_link_status: linkStatus };
    Object.defineProperty(overrides, 'speleodb_push', {
      get: () => push === 'without_history'
        ? Promise.resolve({ kind: 'pushed', undo_history_omitted: true })
        : Promise.reject({
          message: 'this project is too large to upload to SpeleoDB: it is 31.2 MB and the server accepts at most 24.7 MB, even without the undo history. Nothing was uploaded; your changes are still on this computer',
          action: 'Fatal', status: null, transport: null, unpushed_local: null, newer_schema: null,
        }),
    });
    w.__MOCK_IPC_OVERRIDES__ = overrides;
    w.__E2E_TEST__.speleoStore.setState({
      session: { loggedIn: true, userEmail: 'test@example.com', endpoint: 'https://speleodb.example' },
      projectLinkKind: 'remote', remoteProjectId: 'synthetic-upload',
      link: { permission: 'READ_AND_WRITE', editState: 'editing', hasUnpushedEdits: true,
        lockHolder: 'test@example.com', lockedByOther: false, unwound: false, undoMode: 'full',
        remoteState: 'up_to_date', noAccess: false, serverError: null },
    });
  }, { push, linkStatus: LINK_STATUS });
}

async function saveToSpeleoDb(page: Page): Promise<void> {
  await page.keyboard.press('Control+s');
  await expect(page.getByTestId('speleodb-done-dialog')).toBeVisible();
  await page.getByTestId('speleodb-done-message').fill('Synthetic survey update');
  await page.getByTestId('speleodb-done-confirm').click();
}

test('an upload that left its undo history out warns instead of plain success', async ({ page }) => {
  await editingRemoteProject(page, 'without_history');
  await saveToSpeleoDb(page);
  await expect(page.getByTestId('speleodb-done-dialog')).toHaveCount(0);
  const toast = page.locator('[data-testid="toast"][data-toast-key="speleodb-pushed"]');
  await expect(toast).toHaveAttribute('data-toast-kind', 'warning');
  await expect(toast).toContainText('without undo history');
});

test('an upload too large even without history names the reason', async ({ page }) => {
  await editingRemoteProject(page, 'too_large');
  await saveToSpeleoDb(page);
  const dialog = page.getByTestId('speleodb-done-dialog');
  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText('too large to upload');
  await expect(dialog).toContainText('Nothing was uploaded');
  // The work is still here, unuploaded.
  expect(await page.evaluate(() => (window as unknown as TestWindow).__E2E_TEST__.speleoStore.getState().link?.hasUnpushedEdits)).toBe(true);
});

test.describe('revision times', () => {
  // A half-hour zone, so a time read in the wrong zone cannot match by accident.
  test.use({ timezoneId: 'Asia/Kolkata', locale: 'en-US' });

  test('revision times show the local time with the GMT offset', async ({ page }) => {
    await page.setViewportSize({ width: 1400, height: 900 });
    await setupMockedPage(page);
    await page.evaluate(async () => {
      const w = window as unknown as TestWindow;
      w.__MOCK_SET_PROJECT_IS_REMOTE__(true);
      w.__MOCK_IPC_OVERRIDES__ = {
        speleodb_session: { logged_in: true, user_email: 'me@example.com', endpoint: 'https://example.com' },
        speleodb_get_project_metadata: { name: 'Synthetic cave', description: '', country: 'US', visibility: 'PRIVATE', latitude: null, longitude: null, exclude_geojson: false },
        // The UTC instant the backend hands on, and an older server's New York wall
        // time with no zone, for the same minute.
        speleodb_get_revisions: [
          { id: '0aaaaaaabbbbbbbb', parent_ids: ['1cccccccdddddddd'], author_email: 'surveyor@example.org',
            authored_date: '2026-11-09T09:15:42.123Z', message: 'Synthetic survey update' },
          { id: '1cccccccdddddddd', parent_ids: [], author_email: 'surveyor@example.org',
            authored_date: '2026/11/09 04:15', message: 'Synthetic survey' },
        ],
        speleodb_project_access_at: { release_context: { project_id: 'remote', endpoint: 'https://example.com' } },
        speleodb_start_editing: { kind: 'editing' },
        speleodb_link_status: { is_remote: true, edit_enabled: true, permission: 'READ_AND_WRITE', lock_holder: 'me@example.com',
          has_unpushed_edits: false, unwound: false, remote_state: 'up_to_date', access_denied: false, server_error: null,
          exclude_geojson: false, undo_mode: 'full', project_id: 'remote' },
      };
      await (w.__E2E_TEST__.speleoStore.getState().refreshSession as () => Promise<void>)();
      await (w.__E2E_TEST__.projectStore.getState().loadProject as (p: string) => Promise<unknown>)('/projects/cave-alpha.stalx');
      w.__E2E_TEST__.uiStore.setState({ sectionsPanelMode: 'pin', sectionsPanelOpen: true, speleoWorkspaceSection: 'history' });
    });
    await page.getByTestId('workspace-tab-speleodb').click();
    const iso = page.getByTestId('speleodb-revision-date-0aaaaaaabbbbbbbb');
    const wall = page.getByTestId('speleodb-revision-date-1cccccccdddddddd');
    await expect(iso).toBeVisible();
    await expect(iso).toHaveAttribute('datetime', '2026-11-09T09:15:42.123Z');
    // 04:15 in New York (EST) is 09:15 UTC, whatever zone the browser runs in.
    await expect(wall).toHaveAttribute('datetime', '2026-11-09T09:15:00.000Z');
    // 09:15 UTC is 14:45 in Kolkata (GMT+5:30), for both forms.
    for (const when of [iso, wall]) {
      await expect(when).toHaveText(/^Nov 9, 2026,? 02:45\sPM GMT\+5:30$/);
    }
  });
});
