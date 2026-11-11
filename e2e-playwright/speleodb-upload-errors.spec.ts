/**
 * E2E: the SpeleoDB upload dialog stops at survey errors.
 *
 * Save on a project held for editing opens the commit-message dialog. When the
 * survey has errors (not warnings) it shows them in red, holds the upload until
 * "Upload anyway, with the errors" is ticked, and offers "Resolve now", which
 * uploads nothing, opens the Warnings tab, and keeps the typed message for the
 * next Save.
 *
 * Parallel-safe: all state is per-page (mocked IPC + `window.__E2E_TEST__` stores).
 * The survey is synthetic, and so are its issues (injected through the mocked
 * `get_project_warnings`, so a refresh keeps them).
 */
import { test, expect, type Page } from './fixtures/coverage';
import { createProject, seedProjectWithSections, setupMockedPage, waitForMapIdle, type SectionSeed } from './helpers/app';
import { warningMessages } from './helpers/testIds';
import type { useSpeleoStore } from '../src/store/speleoStore';
import type { useProjectStore } from '../src/store/projectStore';

type TestWindow = Window & {
  __E2E_TEST__: {
    speleoStore: typeof useSpeleoStore;
    projectStore: typeof useProjectStore;
  };
  __MOCK_IPC_OVERRIDES__: Record<string, unknown>;
  __MOCK_SET_PROJECT_IS_REMOTE__: (remote: boolean) => void;
  /** How many times the mocked `speleodb_push` was called. */
  __PUSHES__?: number;
};

const SECTIONS: SectionSeed[] = [{
  name: 'Synthetic Sump',
  direction: 'in',
  shots: [
    { azimuth_in: 45, azimuth_out: 225, length: 10, depth_in: -1, depth_out: -3 },
    { azimuth_in: 90, azimuth_out: 270, length: 12, depth_in: -3, depth_out: -6 },
    { azimuth_in: 120, azimuth_out: 300, length: 8, depth_in: -6, depth_out: -8 },
  ],
}];

const LINK_STATUS = {
  is_remote: true, project_id: 'synthetic-errors', edit_enabled: true, permission: 'READ_AND_WRITE',
  has_unpushed_edits: true, unwound: false, remote_state: 'up_to_date', lock_holder: 'test@example.com',
  access_denied: false, server_error: null, exclude_geojson: false, undo_mode: 'full',
};

/** A remote project held for editing, with un-uploaded work and two errors plus one warning. */
async function editingProjectWithErrors(page: Page): Promise<void> {
  await setupMockedPage(page);
  await createProject(page, 'Synthetic errors');
  await seedProjectWithSections(page, SECTIONS);
  await waitForMapIdle(page);
  await page.evaluate((linkStatus) => {
    const w = window as unknown as TestWindow;
    const section = w.__E2E_TEST__.projectStore.getState().project!.sections[0]!;
    const issue = (index: number, code: string, message: string, severity: 'error' | 'warning') => ({
      sectionId: section.id, shotId: section.shots[index]!.id, shotIndex: index, code, message, severity, isResolved: false,
    });
    const warnings = [
      issue(0, 'NEGATIVE_LENGTH', `Shot ${section.id}:1 has negative length: -10.0m`, 'error'),
      issue(1, 'ZERO_LENGTH', `Shot ${section.id}:2 has zero length`, 'error'),
      issue(2, 'LONG_SHOT', `Shot ${section.id}:3 length 8.0m above maximum threshold 5.0m`, 'warning'),
    ];
    w.__MOCK_SET_PROJECT_IS_REMOTE__(true);
    const overrides: Record<string, unknown> = { speleodb_link_status: linkStatus, get_project_warnings: warnings };
    w.__PUSHES__ = 0;
    Object.defineProperty(overrides, 'speleodb_push', {
      get: () => { w.__PUSHES__ = (w.__PUSHES__ ?? 0) + 1; return Promise.resolve({ kind: 'pushed' }); },
    });
    w.__MOCK_IPC_OVERRIDES__ = overrides;
    w.__E2E_TEST__.speleoStore.setState({
      session: { loggedIn: true, userEmail: 'test@example.com', endpoint: 'https://speleodb.example' },
      projectLinkKind: 'remote', remoteProjectId: 'synthetic-errors',
      link: { permission: 'READ_AND_WRITE', editState: 'editing', hasUnpushedEdits: true,
        lockHolder: 'test@example.com', lockedByOther: false, unwound: false, undoMode: 'full',
        remoteState: 'up_to_date', noAccess: false, serverError: null },
    });
  }, LINK_STATUS);
  // Load the issues through the app's own refresh, now answered by the override.
  // A refresh the seeding started earlier may still be in flight and land the
  // mock's empty list, so wait until the store holds the three issues.
  await page.evaluate(() => (window as unknown as TestWindow).__E2E_TEST__.projectStore.getState().refreshWarnings());
  await expect.poll(() => page.evaluate(() =>
    (window as unknown as TestWindow).__E2E_TEST__.projectStore.getState().warnings.length)).toBe(3);
}

const pushes = (page: Page) => page.evaluate(() => (window as unknown as TestWindow).__PUSHES__ ?? 0);

test('the upload waits for "Upload anyway", then goes ahead', async ({ page }) => {
  await editingProjectWithErrors(page);
  await page.keyboard.press('Control+s');
  const dialog = page.getByTestId('speleodb-done-dialog');
  await expect(dialog).toBeVisible();
  await expect(page.getByTestId('speleodb-upload-errors')).toContainText('This survey has 2 errors.');

  await page.getByTestId('speleodb-done-message').fill('Synthetic survey update');
  const confirm = page.getByTestId('speleodb-done-confirm');
  await expect(confirm).toBeDisabled();

  await page.getByTestId('speleodb-upload-errors-accept').check();
  await expect(confirm).toBeEnabled();
  await confirm.click();
  await expect(dialog).toHaveCount(0);
  expect(await pushes(page)).toBe(1);
});

test('Resolve now opens the Warnings tab and the next Save keeps the message', async ({ page }) => {
  await editingProjectWithErrors(page);
  await page.getByTestId('detail-tab-details').click();
  await page.keyboard.press('Control+s');
  await page.getByTestId('speleodb-done-message').fill('Fixed the sump survey');
  await page.getByTestId('speleodb-upload-errors-resolve').click();

  await expect(page.getByTestId('speleodb-done-dialog')).toHaveCount(0);
  await expect(page.getByTestId('detail-tab-warnings')).toHaveClass(/border-accent/);
  await expect(warningMessages(page)).toHaveCount(3);
  await expect(warningMessages(page).first()).toBeVisible();
  await page.keyboard.press('Control+s');
  await expect(page.getByTestId('speleodb-done-message')).toHaveValue('Fixed the sump survey');
  // Nothing went to SpeleoDB on the way.
  expect(await pushes(page)).toBe(0);
});
