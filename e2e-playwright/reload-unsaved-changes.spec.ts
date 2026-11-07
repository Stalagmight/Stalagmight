import { test, expect, type Page } from './fixtures/coverage';
import { openAppMenu, setupMockedPage, waitForAppReady, waitForMapReady } from './helpers/app';
import type { E2EWindow, MockCurrentProject } from './helpers/e2eWindow';
import { setIpcOverrides } from './helpers/ipcOverrides';
import { systemCardTestId } from './helpers/testIds';
import type { CommandError } from '../src/types/ipc/CommandError';

/**
 * [#576 W81-1] A webview reload (F5 / Ctrl+R) drops the page but not the backend:
 * the project stays open there, with every edit not yet saved. Opening it again
 * from the project list answers ALREADY_OPEN, and the frontend re-reads the
 * backend's project as a new session. That re-read started the session clean, so
 * closing the project (or quitting) asked nothing and the edits were lost.
 *
 * The reload is real (`page.reload()`); the mocked backend does not outlive the
 * page, so the spec plays its part after the reload: it holds the project the page
 * had open (`setCurrentProject`), answers the load ALREADY_OPEN, and says through
 * `speleodb_open_file_flags` whether its session holds unsaved changes.
 */

const ALPHA = '/projects/cave-alpha.stalx';

const ALREADY_OPEN = { code: 'ALREADY_OPEN', message: 'Project already open' } satisfies CommandError;

const isDirty = (page: Page) =>
  page.evaluate(() => (window as E2EWindow).__E2E_TEST__.projectStore.getState().isDirty);

async function openAlphaFromTheList(page: Page): Promise<void> {
  await page.getByTestId('globalview-view-toggle-list').click();
  await page.getByTestId(systemCardTestId(ALPHA)).dblclick();
  await expect(page.getByTestId('single-system-view')).toBeVisible();
  await waitForMapReady(page);
}

/**
 * Open CaveAlpha, edit it (unsaved), then reload the page with the mocked backend
 * still holding it open; `backendUnsaved` is what the backend says about its session.
 */
async function reloadWithAlphaStillOpen(page: Page, backendUnsaved: boolean): Promise<void> {
  await setupMockedPage(page);
  await openAlphaFromTheList(page);
  await page.evaluate(async () => {
    const st = (window as E2EWindow).__E2E_TEST__.projectStore.getState();
    const section = st.project!.sections[0]!;
    await st.updateShot(section.id, section.shots[0]!.id, { length: 12.5 });
  });
  await expect.poll(() => isDirty(page), { message: 'the edit marks the project unsaved' }).toBe(true);
  const held = await page.evaluate(() => (window as E2EWindow).__E2E_MOCKS__!.getCurrentProject());
  expect(held, 'the mocked backend holds the open project').not.toBeNull();

  await page.reload();
  await waitForAppReady(page);
  await expect(page.getByTestId('global-view')).toBeVisible();

  // The backend after the reload: still holding CaveAlpha, so a load of it answers ALREADY_OPEN.
  await page.evaluate(
    ({ project, unsaved }) => {
      const w = window as E2EWindow & { __openFileFlagsOverrides?: { hasUnsavedChanges: boolean } };
      w.__E2E_MOCKS__!.setCurrentProject(project as MockCurrentProject);
      w.__openFileFlagsOverrides = { hasUnsavedChanges: unsaved };
    },
    { project: held, unsaved: backendUnsaved },
  );
  await setIpcOverrides(page, { load_project_full_binary: { __reject: ALREADY_OPEN } });

  await openAlphaFromTheList(page);
  await expect(page.getByTestId('loading-progress')).toBeHidden();
}

test.describe('a project still open after a webview reload (#576 W81-1)', () => {
  test('keeps its unsaved changes guarded: closing it asks first', async ({ page }) => {
    await reloadWithAlphaStillOpen(page, true);

    await openAppMenu(page);
    await page.getByTestId('back-button').click();
    await expect(page.getByTestId('unsaved-changes-dialog'), 'closing asks about the unsaved changes').toBeVisible();
    await expect(page.getByTestId('single-system-view')).toBeVisible();
    expect(await isDirty(page), 'the re-read took the backend unsaved mark').toBe(true);
  });

  test('closes without asking when the backend holds no unsaved changes (the control)', async ({ page }) => {
    await reloadWithAlphaStillOpen(page, false);

    expect(await isDirty(page), 'the re-read starts clean').toBe(false);
    await openAppMenu(page);
    await page.getByTestId('back-button').click();
    await expect(page.getByTestId('global-view'), 'closing goes straight back to the project list').toBeVisible();
    await expect(page.getByTestId('unsaved-changes-dialog')).toHaveCount(0);
  });
});
