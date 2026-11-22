/**
 * E2E: two sessions of one SpeleoDB account (fuzzer finding R3,
 * same-account-overwrite).
 *
 * SpeleoDB's write lock is the account's, so another session of the same
 * account holds it too. The backend refuses an upload when SpeleoDB's latest
 * revision is not the copy's base and not an upload of this session
 * (`divergence_needed`, nothing uploaded), and reports the race it cannot close
 * - a revision landing between that check and the upload - with the landed
 * result (`replaced_revisions`).
 *
 * - Save Changes refused that way opens the divergence prompt, and the work stays
 *   un-uploaded on this computer.
 * - An upload that replaced a revision pushed meanwhile names it on the banner,
 *   instead of the plain success toast.
 * - Save and Close whose upload replaced one stops there, so the banner is read;
 *   the next close asks nothing and closes.
 *
 * Parallel-safe: all state is per-page (mocked IPC + `window.__E2E_TEST__` stores).
 */
import { test, expect, type Page } from './fixtures/coverage';
import { createProject, openAppMenu, setupMockedPage } from './helpers/app';
import type { useSpeleoStore } from '../src/store/speleoStore';

type TestWindow = Window & {
  __E2E_TEST__: Record<string, { getState: () => Record<string, unknown>; setState: (s: unknown) => void }> & {
    speleoStore: typeof useSpeleoStore;
  };
  __MOCK_IPC_OVERRIDES__: Record<string, unknown>;
  __MOCK_SET_PROJECT_IS_REMOTE__: (remote: boolean) => void;
};

const LINK_STATUS = {
  is_remote: true, project_id: 'synthetic-same-account', edit_enabled: true, permission: 'READ_AND_WRITE',
  has_unpushed_edits: true, unwound: false, remote_state: 'up_to_date', lock_holder: 'test@example.com',
  access_denied: false, server_error: null, exclude_geojson: false, undo_mode: 'full',
};

/** A revision the account's other session uploaded (synthetic). */
const THEIRS = {
  id: '0123456789abcdef0123456789abcdef01234567', parent_ids: ['fedcba9876543210fedcba9876543210fedcba98'],
  author_email: 'test@example.com', authored_date: '2026-11-21T10:00:00.000Z', message: 'Surveyed the sump',
};

/** A remote project held for editing with un-uploaded work; `push` answers the Save. */
async function editingRemoteProject(page: Page, push: 'refused' | 'replaced'): Promise<void> {
  await setupMockedPage(page);
  await createProject(page, 'Synthetic shared account');
  await page.evaluate(({ push, linkStatus, theirs }) => {
    const w = window as unknown as TestWindow;
    w.__MOCK_SET_PROJECT_IS_REMOTE__(true);
    w.__MOCK_IPC_OVERRIDES__ = {
      speleodb_link_status: linkStatus,
      speleodb_push: push === 'refused'
        ? { kind: 'divergence_needed' }
        : { kind: 'pushed', replaced_revisions: [theirs] },
    };
    w.__E2E_TEST__.speleoStore.setState({
      session: { loggedIn: true, userEmail: 'test@example.com', endpoint: 'https://speleodb.example' },
      projectLinkKind: 'remote', remoteProjectId: 'synthetic-same-account',
      link: { permission: 'READ_AND_WRITE', editState: 'editing', hasUnpushedEdits: true,
        lockHolder: 'test@example.com', lockedByOther: false, unwound: false, undoMode: 'full',
        remoteState: 'up_to_date', noAccess: false, serverError: null },
    });
  }, { push, linkStatus: LINK_STATUS, theirs: THEIRS });
}

async function saveToSpeleoDb(page: Page): Promise<void> {
  await page.keyboard.press('Control+s');
  await expect(page.getByTestId('speleodb-done-dialog')).toBeVisible();
  await page.getByTestId('speleodb-done-message').fill('Synthetic survey update');
  await page.getByTestId('speleodb-done-confirm').click();
}

test('Save Changes over another session\'s revision opens the divergence prompt and keeps the work', async ({ page }) => {
  await editingRemoteProject(page, 'refused');
  await saveToSpeleoDb(page);
  await expect(page.getByTestId('speleodb-divergence-dialog')).toBeVisible();
  await expect(page.getByTestId('speleodb-divergence-take-remote')).toBeVisible();
  await expect(page.getByTestId('speleodb-divergence-save-copy')).toBeVisible();
  await expect(page.locator('[data-testid="toast"][data-toast-key="speleodb-pushed"]')).toHaveCount(0);
  const link = await page.evaluate(() => (window as unknown as TestWindow).__E2E_TEST__.speleoStore.getState().link);
  expect(link?.hasUnpushedEdits).toBe(true);
  expect(link?.editState).toBe('editing');
});

test('an upload that replaced a revision pushed meanwhile names it on the banner', async ({ page }) => {
  await editingRemoteProject(page, 'replaced');
  await saveToSpeleoDb(page);
  await expect(page.getByTestId('speleodb-done-dialog')).toHaveCount(0);
  const banner = page.getByTestId('speleo-replaced-banner');
  await expect(banner).toHaveAttribute('data-kind', 'replaced_revision');
  await expect(banner).toContainText('Your upload replaced "Surveyed the sump" (01234567), uploaded by test@example.com');
  await expect(banner).toContainText('Revision History, where you can view or restore it.');
  await expect(page.locator('[data-testid="toast"][data-toast-key="speleodb-pushed"]')).toHaveCount(0);
  await page.getByTestId('speleo-replaced-banner-dismiss').click();
  await expect(banner).toHaveCount(0);
});

test('Save and Close stops on an upload that replaced a revision pushed meanwhile, and the next close proceeds', async ({ page }) => {
  await editingRemoteProject(page, 'replaced');
  await page.evaluate(({ linkStatus, theirs }) => {
    const w = window as unknown as TestWindow;
    // Finish editing lands on top of the other session's revision and hands the lock back.
    Object.defineProperty(w.__MOCK_IPC_OVERRIDES__, 'speleodb_done', {
      enumerable: true,
      configurable: true,
      get: () => {
        w.__MOCK_IPC_OVERRIDES__.speleodb_link_status = {
          ...linkStatus, edit_enabled: false, has_unpushed_edits: false, lock_holder: null,
        };
        return { kind: 'released', replaced_revisions: [theirs] };
      },
    });
  }, { linkStatus: LINK_STATUS, theirs: THEIRS });
  const closes = () => page.evaluate(() =>
    ((window as unknown as { __MOCK_EXIT_CALLS__?: Array<{ cmd: string }> }).__MOCK_EXIT_CALLS__ ?? [])
      .filter((call) => call.cmd === 'close_project').length);

  await openAppMenu(page);
  await page.getByTestId('project-menu-save-and-close').click();
  await expect(page.getByTestId('speleodb-done-dialog')).toBeVisible();
  await page.getByTestId('speleodb-done-message').fill('Synthetic survey update');
  await page.getByTestId('speleodb-done-confirm').click();

  // The upload landed, but the project stays open with the banner on screen.
  await expect(page.getByTestId('speleodb-done-dialog')).toHaveCount(0);
  const banner = page.getByTestId('speleo-replaced-banner');
  await expect(banner).toHaveAttribute('data-kind', 'replaced_revision');
  await expect(banner).toContainText('Your upload replaced "Surveyed the sump" (01234567), uploaded by test@example.com');
  await expect(page.getByTestId('single-system-view')).toBeVisible();
  expect(await closes()).toBe(0);

  // The lock is back and nothing is left to upload: the next close asks nothing.
  await openAppMenu(page);
  await page.getByTestId('back-button').click();
  await expect(page.getByTestId('global-view')).toBeVisible();
  await expect(page.getByTestId('speleodb-exit-dialog')).toHaveCount(0);
  expect(await closes()).toBe(1);
});
