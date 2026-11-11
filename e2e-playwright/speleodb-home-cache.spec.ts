/**
 * E2E: cached SpeleoDB projects on the home screen.
 *
 * - Signed out, every cached copy is listed with "Read-only copy - connect to SpeleoDB to edit"
 *   and opens read-only without taking the lock; an expired session does the same
 *   and signs the app out.
 * - Signed in, only this account's copies (and pre-upgrade ones with no recorded
 *   owner) are listed.
 * - Right-click on a SpeleoDB card or row offers removing the local copy, through an
 *   in-app dialog that warns before un-uploaded work goes.
 * - Settings' "Purge all cached files" keeps copies with un-uploaded changes unless
 *   deleting them is confirmed a second time.
 *
 * The backend's owner classification is mocked (`speleodb_cached_projects`); the
 * Rust tests cover the classification itself. All data is synthetic and per-page.
 */
import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage } from './helpers/app';
import { openSpeleoTab } from './helpers/speleoWorkspace';

const MINE = '11111111-1111-1111-1111-111111111111';
const THEIRS = '22222222-2222-2222-2222-222222222222';
const LEGACY = '33333333-3333-3333-3333-333333333333';
const PROJECT_PATH = '/projects/cave-alpha.stalx';
const SIGNED_IN = { logged_in: true, user_email: 'diver@example.com', endpoint: 'https://synthetic.example' };
const SIGNED_OUT = { logged_in: false, user_email: null, endpoint: null };

type Ownership = 'signed_out' | 'mine' | 'unknown' | 'other_account';
type Call = { cmd: string; args: Record<string, unknown> | null };

function copy(id: string, name: string, ownership: Ownership, dirty = false) {
  return {
    project_id: id,
    project_name: name,
    local_path: `/cache/speleodb/${id}/project.stalx`,
    ownership,
    has_unpushed_edits: dirty,
    state_unknown: false,
    is_open: false,
  };
}

interface Seed {
  signedIn: boolean;
  copies: ReturnType<typeof copy>[];
  /** What `speleodb_open_project` answers; the session ends with it when `read_only` says so. */
  open?: Record<string, unknown>;
  purge?: Record<string, unknown>;
  /** Recents rows (the map is built from them). */
  recents?: Record<string, unknown>[];
}

async function seed(page: Page, { signedIn, copies, open, purge, recents }: Seed): Promise<string[]> {
  await page.addInitScript(({ signedIn, copies, open, purge, recents, inSession, outSession, openedId }) => {
    const w = window as unknown as Record<string, unknown>;
    let loggedIn = signedIn;
    const overrides: Record<string, unknown> = {
      speleodb_list_projects: [],
      speleodb_cached_projects: copies,
      ...(purge ? { speleodb_purge_cached_copies: purge } : {}),
      ...(recents ? { get_recent_projects: recents, list_recent_projects: recents } : {}),
    };
    Object.defineProperty(overrides, 'speleodb_session', {
      enumerable: true,
      get: () => (loggedIn ? inSession : outSession),
    });
    if (open) {
      Object.defineProperty(overrides, 'speleodb_open_project', {
        enumerable: true,
        get: () => {
          // The backend drops a token the server rejected during the open.
          if (open.read_only === 'session_expired') loggedIn = false;
          return open;
        },
      });
    }
    w.__MOCK_IPC_OVERRIDES__ = overrides;
    // The opened copy is a SpeleoDB project, held read-only.
    w.__speleoLinkStatusOverrides = { is_remote: true, edit_enabled: false, project_id: openedId };
  }, {
    signedIn, copies, open: open ?? null, purge: purge ?? null, recents: recents ?? null,
    inSession: SIGNED_IN, outSession: SIGNED_OUT, openedId: MINE,
  });
  const commands: string[] = [];
  page.on('console', (msg) => {
    if (msg.text().startsWith('[Mock IPC] Command:')) commands.push(msg.text());
  });
  await setupMockedPage(page);
  await page.evaluate(() => {
    (window as unknown as { __MOCK_SET_PROJECT_IS_REMOTE__: (r: boolean) => void }).__MOCK_SET_PROJECT_IS_REMOTE__(true);
  });
  return commands;
}

async function calls(page: Page, cmd: string): Promise<Call[]> {
  const all = await page.evaluate(
    () => ((window as unknown as { __MOCK_EXIT_CALLS__?: Call[] }).__MOCK_EXIT_CALLS__ ?? []),
  );
  return all.filter((c) => c.cmd === cmd);
}

test('signed out: every cached copy is listed read-only and opens without taking the lock', async ({ page }) => {
  const commands = await seed(page, {
    signedIn: false,
    copies: [copy(MINE, 'Synthetic mine', 'signed_out', true), copy(THEIRS, 'Synthetic theirs', 'signed_out')],
    open: { local_path: PROJECT_PATH, read_only: 'signed_out', has_unpushed_edits: true },
  });
  const mine = page.getByTestId(`system-card-remote-${MINE}`);
  await expect(mine).toBeVisible();
  await expect(page.getByTestId(`system-card-remote-${THEIRS}`)).toBeVisible();
  await expect(mine).toContainText('Read-only copy - connect to SpeleoDB to edit');
  await expect(mine).not.toContainText('Permission:');

  await mine.click();
  await expect(page.getByTestId('single-system-view')).toBeVisible();
  await expect(page.getByTestId('toast')).toContainText('Opened a read-only copy');
  const pill = page.getByTestId('speleodb-readonly-banner');
  await expect(pill.getByTestId('speleodb-readonly-reason')).toHaveText('Read-only copy - connect to SpeleoDB to edit');
  await expect(pill.getByTestId('speleodb-readonly-login')).toHaveText('Connect');
  await expect(pill.getByTestId('speleodb-readonly-resume')).toHaveCount(0);
  expect(commands.filter((c) => c.includes('Command: speleodb_start_editing'))).toHaveLength(0);
  expect((await calls(page, 'speleodb_open_project')).map((c) => c.args?.projectId)).toEqual([MINE]);

  // The SpeleoDB tab: connecting again is the way forward, and what cannot run
  // says why. (Which button is filled follows the one-filled-button follow-up.)
  await openSpeleoTab(page);
  await expect(page.getByTestId('speleodb-workspace-editing')).toHaveText('Connect to resume editing');
  await expect(page.getByTestId('speleodb-workspace-editing')).toBeEnabled();
  await expect(page.getByTestId('speleodb-workspace-latest')).toBeDisabled();
  await expect(page.getByTestId('speleodb-workspace-latest')).toHaveAttribute('title', /Connect to SpeleoDB/);
  await expect(page.getByTestId('speleodb-workspace-open-web')).toHaveAttribute('title', /Connect to SpeleoDB/);

  // "Connect" goes to the SpeleoDB settings.
  await pill.getByTestId('speleodb-readonly-login').click();
  await expect(page.getByTestId('settings-panel-speleodb')).toBeVisible();
});

test('a copy with un-uploaded edits is marked alike on its card and its table row', async ({ page }) => {
  await seed(page, {
    signedIn: false,
    copies: [copy(MINE, 'Synthetic mine', 'signed_out', true), copy(THEIRS, 'Synthetic theirs', 'signed_out')],
  });
  await expect(page.getByTestId(`system-card-remote-${MINE}`)).toHaveAttribute('data-recent-accent', 'unpushed');
  await expect(page.getByTestId(`system-card-remote-${THEIRS}`)).toHaveAttribute('data-recent-accent', 'none');
  // SpeleoDB entries open on a single click, and the hint says so.
  await expect(page.getByTestId('globalview-list-status')).toContainText('A SpeleoDB project opens with a single click.');

  await page.getByTestId('recent-density-table').click();
  await expect(page.getByTestId(`system-row-remote-${MINE}`)).toHaveAttribute('data-recent-accent', 'unpushed');
  await expect(page.getByTestId(`system-row-remote-${THEIRS}`)).toHaveAttribute('data-recent-accent', 'none');
});

test('an expired session opens the cached copy read-only and signs the app out', async ({ page }) => {
  const commands = await seed(page, {
    signedIn: true,
    copies: [copy(MINE, 'Synthetic mine', 'mine')],
    open: { local_path: PROJECT_PATH, read_only: 'session_expired', has_unpushed_edits: false },
  });
  await page.getByTestId(`system-card-remote-${MINE}`).click();
  await expect(page.getByTestId('single-system-view')).toBeVisible();
  await expect(page.getByTestId('toast')).toContainText('Your SpeleoDB session has expired');
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as { __E2E_TEST__: { speleoStore: { getState: () => { session: { loggedIn: boolean } | null } } } })
      .__E2E_TEST__.speleoStore.getState().session?.loggedIn ?? false)).toBe(false);
  await expect(page.getByTestId('speleodb-readonly-reason')).toHaveText('Read-only copy - connect to SpeleoDB to edit');
  expect(commands.filter((c) => c.includes('Command: speleodb_start_editing'))).toHaveLength(0);
});

test('signed in: only this account\'s copies and unknown-owner copies are listed', async ({ page }) => {
  await seed(page, {
    signedIn: true,
    copies: [
      copy(MINE, 'Synthetic mine', 'mine'),
      copy(LEGACY, 'Synthetic legacy', 'unknown'),
      copy(THEIRS, 'Synthetic theirs', 'other_account'),
    ],
  });
  await expect(page.getByTestId(`system-card-remote-${MINE}`)).toBeVisible();
  await expect(page.getByTestId(`system-card-remote-${LEGACY}`)).toBeVisible();
  await expect(page.getByTestId(`system-card-remote-${THEIRS}`)).toHaveCount(0);
  await expect(page.getByTestId(`system-card-remote-${MINE}`)).not.toContainText('connect to SpeleoDB to edit');
});

for (const signedIn of [false, true]) {
  test(`map: another account's cached copy is ${signedIn ? 'hidden while signed in' : 'shown while signed out'}`, async ({ page }) => {
    const path = copy(THEIRS, 'Synthetic theirs', 'other_account').local_path;
    await seed(page, {
      signedIn,
      copies: [copy(THEIRS, 'Synthetic theirs', signedIn ? 'other_account' : 'signed_out')],
      recents: [
        { name: 'Local cave', file_path: '/projects/local.stalx', location: null, modified_at: '2026-01-01T00:00:00Z',
          section_count: 1, station_count: 2, total_length: 3, gps_center: [-87.46, 20.21], gps_bounds: null, is_remote: false },
        { name: 'Synthetic theirs', file_path: path, location: null, modified_at: '2026-01-01T00:00:00Z',
          section_count: 1, station_count: 2, total_length: 3, gps_center: [-87.4631, 20.2137], gps_bounds: null,
          is_remote: true, speleodb_project_id: THEIRS },
      ],
    });
    await page.getByTestId('globalview-view-toggle-map').click();
    await page.getByTestId('view-mode-toggle').click();
    await expect(page.getByTestId(`system-marker-${encodeURIComponent('/projects/local.stalx')}`)).toBeVisible();
    await expect(page.getByTestId(`system-marker-${encodeURIComponent(path)}`)).toHaveCount(signedIn ? 0 : 1);
  });
}

for (const density of ['cards', 'table'] as const) {
  test(`${density}: right-click removes the local copy, warning before un-uploaded work goes`, async ({ page }) => {
    await seed(page, {
      signedIn: false,
      copies: [copy(MINE, 'Synthetic mine', 'signed_out', true), copy(THEIRS, 'Synthetic theirs', 'signed_out')],
    });
    if (density === 'table') await page.getByTestId('recent-density-table').click();
    const prefix = density === 'cards' ? 'system-card' : 'system-row';

    await page.getByTestId(`${prefix}-remote-${MINE}`).click({ button: 'right' });
    const menu = page.getByTestId('speleodb-project-context-menu');
    await expect(menu).toBeVisible();
    await expect(menu.getByTestId('speleodb-context-menu-open')).toBeVisible();
    await menu.getByTestId('speleodb-context-menu-remove-copy').click();
    const dialog = page.getByTestId('speleodb-remove-copy-dialog');
    await expect(dialog).toBeVisible();
    await expect(dialog.getByTestId('speleodb-remove-copy-warning')).toContainText('never uploaded');
    // Signed out, no lock is handed back, and the dialog says where it stays.
    await expect(dialog.getByTestId('speleodb-remove-copy-lock')).toContainText('You are not connected to SpeleoDB, so no editing lock is handed back');
    const confirm = dialog.getByTestId('speleodb-remove-copy-confirm');
    await expect(confirm).toHaveText('Remove copy and lose my changes');
    // Cancel removes nothing.
    await dialog.getByTestId('speleodb-remove-copy-cancel').click();
    await expect(dialog).toHaveCount(0);
    expect(await calls(page, 'speleodb_remove_cached_copy')).toEqual([]);

    await page.getByTestId(`${prefix}-remote-${MINE}`).click({ button: 'right' });
    await page.getByTestId('speleodb-context-menu-remove-copy').click();
    await page.getByTestId('speleodb-remove-copy-confirm').click();
    await expect(page.getByTestId('speleodb-remove-copy-dialog')).toHaveCount(0);
    await expect(page.getByTestId('toast')).toContainText('Removed the local copy of Synthetic mine');

    // A clean copy is removed without the un-uploaded consent.
    await page.getByTestId(`${prefix}-remote-${THEIRS}`).click({ button: 'right' });
    await page.getByTestId('speleodb-context-menu-remove-copy').click();
    await expect(page.getByTestId('speleodb-remove-copy-warning')).toHaveCount(0);
    await expect(page.getByTestId('speleodb-remove-copy-confirm')).toHaveText('Remove copy');
    await page.getByTestId('speleodb-remove-copy-confirm').click();
    await expect(page.getByTestId('speleodb-remove-copy-dialog')).toHaveCount(0);

    expect((await calls(page, 'speleodb_remove_cached_copy')).map((c) => c.args)).toEqual([
      { projectId: MINE, discardUnpushed: true },
      { projectId: THEIRS, discardUnpushed: false },
    ]);
  });
}

test('purge keeps copies with un-uploaded changes unless deleting them is confirmed twice', async ({ page }) => {
  await seed(page, {
    signedIn: true,
    copies: [
      copy(MINE, 'Synthetic mine', 'mine', true),
      copy(LEGACY, 'Synthetic legacy', 'unknown'),
      // Another account's un-uploaded work: signed in, the purge never names or counts it.
      copy(THEIRS, 'Synthetic theirs', 'other_account', true),
    ],
    purge: { removed: 1, kept_unpushed: ['Synthetic mine'], kept_open: [], failed: [], locks_released: ['Synthetic legacy'], locks_unchecked: [] },
  });
  await page.evaluate(() => {
    (window as unknown as { __E2E_TEST__: { uiStore: { getState: () => { openDialog: (d: string, data?: unknown) => void } } } })
      .__E2E_TEST__.uiStore.getState().openDialog('settings', { initialCategory: 'speleodb' });
  });
  const section = page.getByTestId('section-speleodb-cache');
  await expect(section).toBeVisible();

  await section.getByTestId('speleodb-purge-cache').click();
  const dialog = page.getByTestId('speleodb-purge-dialog');
  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText('Remove 2 downloaded copies');
  await expect(dialog).not.toContainText('Synthetic theirs');
  await expect(dialog.getByTestId('speleodb-purge-at-risk-list')).toHaveText('Synthetic mine');
  await expect(dialog.getByTestId('speleodb-purge-locks')).toContainText('Your editing locks on these projects are handed back');
  await dialog.getByTestId('speleodb-purge-keep-at-risk').click();
  await expect(dialog).toHaveCount(0);
  await expect(section.getByTestId('speleodb-purge-result')).toContainText('Removed 1 downloaded copy.');
  await expect(section.getByTestId('speleodb-purge-result')).toContainText('Synthetic mine');
  await expect(section.getByTestId('speleodb-purge-result')).toContainText('Handed back the editing lock of: Synthetic legacy');
  // The last section of the panel: its result is brought into view, every line of it.
  await expect(section.getByTestId('speleodb-purge-result')).toBeInViewport({ ratio: 1 });

  await section.getByTestId('speleodb-purge-cache').click();
  await page.getByTestId('speleodb-purge-delete-all').click();
  const lost = page.getByTestId('speleodb-purge-lost-dialog');
  await expect(lost).toBeVisible();
  await expect(lost.getByTestId('speleodb-purge-at-risk-list')).toHaveText('Synthetic mine');
  // Back returns to the first question without purging.
  await lost.getByTestId('speleodb-purge-lost-back').click();
  await expect(page.getByTestId('speleodb-purge-dialog')).toBeVisible();
  await page.getByTestId('speleodb-purge-delete-all').click();
  await page.getByTestId('speleodb-purge-lost-confirm').click();
  await expect(page.getByTestId('speleodb-purge-lost-dialog')).toHaveCount(0);

  expect((await calls(page, 'speleodb_purge_cached_copies')).map((c) => c.args)).toEqual([
    { discardUnpushed: false },
    { discardUnpushed: true },
  ]);
});

test('purge is disabled, and says why, when nothing is downloaded', async ({ page }) => {
  await seed(page, { signedIn: false, copies: [] });
  await page.evaluate(() => {
    (window as unknown as { __E2E_TEST__: { uiStore: { getState: () => { openDialog: (d: string, data?: unknown) => void } } } })
      .__E2E_TEST__.uiStore.getState().openDialog('settings', { initialCategory: 'speleodb' });
  });
  const section = page.getByTestId('section-speleodb-cache');
  await expect(section.getByTestId('speleodb-purge-empty')).toHaveText('There are no downloaded copies to remove.');
  await expect(section.getByTestId('speleodb-purge-cache')).toBeDisabled();
  expect(await calls(page, 'speleodb_purge_cached_copies')).toEqual([]);
});
