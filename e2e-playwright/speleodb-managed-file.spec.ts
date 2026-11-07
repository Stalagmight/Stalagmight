import { test, expect, Page } from './fixtures/coverage';
import { setupMockedPage, createProject, openAppMenu } from './helpers/app';
import { getIpcCalls, recordIpcCalls, waitForIpcCall } from './helpers/ipcRecorder';

/**
 * Canonical SpeleoDB storage, frontend half:
 *
 * - The managed-file prompt (SpeleoManagedFileDialog): raised when the backend
 *   refuses to open a non-canonical copy of a managed project. The refusal
 *   itself cannot be driven end-to-end here - the frontend loads through
 *   `load_project_full_binary`, and the mock's `__throw` override stringifies
 *   into a plain Error, which cannot express the structured SPELEODB_MANAGED
 *   LoadError - so the prompt is raised directly through the store, the same
 *   idiom corruption-recovery.spec.ts uses.
 * - Save a Copy: an explicit destination on a managed project writes a
 *   detached unlinked copy; the session stays put and a toast names the copy.
 */

const MANAGED = {
  path: '/mock/foreign/copy.stalx',
  projectId: '11111111-1111-1111-1111-111111111111',
  projectName: 'Cenote Azul',
  endpoint: 'http://sdb.example/',
  canOpenRemote: true,
};

async function openManagedFileDialog(page: Page): Promise<void> {
  await page.evaluate((managed) => {
    const e2e = (window as unknown as {
      __E2E_TEST__?: {
        uiStore?: {
          getState: () => { openDialog: (d: string, data?: Record<string, unknown>) => void };
        };
      };
    }).__E2E_TEST__;
    e2e?.uiStore?.getState().openDialog('speleodb-managed-file', { managed });
  }, MANAGED);
  await expect(page.getByTestId('speleodb-managed-file-dialog')).toBeVisible({ timeout: 5000 });
}

test.describe('SpeleoDB managed-file prompt', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('names the file and project, and disables the remote action while signed out', async ({
    page,
  }) => {
    await openManagedFileDialog(page);

    await expect(page.getByTestId('speleodb-managed-file-dialog')).toContainText('copy.stalx');
    await expect(page.getByTestId('speleodb-managed-file-dialog')).toContainText('Cenote Azul');
    // The mock's speleodb_session is signed out, so the remote action is
    // disabled with the reason on screen while the strip action stays live.
    await expect(page.getByTestId('speleodb-managed-file-open-remote')).toBeDisabled();
    await expect(page.getByTestId('speleodb-managed-file-remote-blocked')).toBeVisible();
    await expect(page.getByTestId('speleodb-managed-file-remove-link')).toBeEnabled();

    // Cancel is a plain close: no project opened, still on the library view.
    await page.getByTestId('speleodb-managed-file-cancel').click();
    await expect(page.getByTestId('speleodb-managed-file-dialog')).not.toBeVisible();
    await expect(page.getByTestId('global-view')).toBeVisible();
  });

  test('remove link strips the file and opens it as a local project @smoke', async ({ page }) => {
    await openManagedFileDialog(page);
    await recordIpcCalls(page, ['speleodb_remove_link_at', 'load_project_full_binary']);
    await page.getByTestId('speleodb-managed-file-remove-link').click();

    // The strip succeeded (mock returns null) and the SAME path was re-opened
    // as a plain local project - the load lands on the survey view.
    await expect(page.getByTestId('speleodb-managed-file-dialog')).not.toBeVisible({
      timeout: 5000,
    });
    await expect(page.getByTestId('map-canvas')).toBeVisible({ timeout: 10000 });

    // The link was stripped from THAT file, for THAT managed project (the
    // backend refuses a strip whose `expected` does not match the row)...
    const [strip] = await waitForIpcCall(page, 'speleodb_remove_link_at');
    expect(strip.args.path).toBe(MANAGED.path);
    expect((strip.args.expected as { projectId?: string }).projectId).toBe(MANAGED.projectId);
    // ...and only then was the same path loaded, now as the open project.
    const loads = await waitForIpcCall(page, 'load_project_full_binary');
    expect(loads.map((c) => c.args.path)).toEqual([MANAGED.path]);
    // [#576 RV4-FE-FEB-8] "Only then": the recorder keeps call order, so a load
    // issued before the strip finished being sent fails here.
    expect((await getIpcCalls(page)).map((c) => c.cmd)).toEqual([
      'speleodb_remove_link_at',
      'load_project_full_binary',
    ]);
    const opened = await page.evaluate(
      () =>
        (window as unknown as {
          __E2E_TEST__: { projectStore: { getState: () => { project: { file_path: string | null } | null } } };
        }).__E2E_TEST__.projectStore.getState().project?.file_path ?? null,
    );
    expect(opened).toBe(MANAGED.path);
  });

  test('a failed strip keeps the prompt open with the reason', async ({ page }) => {
    await page.evaluate(() => {
      const overrides =
        ((window as unknown as Record<string, unknown>).__MOCK_IPC_OVERRIDES__ as Record<
          string,
          unknown
        >) ?? {};
      (window as unknown as Record<string, unknown>).__MOCK_IPC_OVERRIDES__ = {
        ...overrides,
        speleodb_remove_link_at: { __throw: 'file is locked by another process' },
      };
    });
    await openManagedFileDialog(page);
    await page.getByTestId('speleodb-managed-file-remove-link').click();

    // The prompt (and its error) stay on screen; nothing opened.
    await expect(page.getByTestId('speleodb-managed-file-error')).toBeVisible({ timeout: 5000 });
    await expect(page.getByTestId('speleodb-managed-file-dialog')).toBeVisible();
    await expect(page.getByTestId('global-view')).toBeVisible();
  });
});

test.describe('Save a Copy on a managed project', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('labels the action, writes a detached copy, and the session stays put', async ({ page }) => {
    await createProject(page, 'Managed Cave');

    // Make the open project a managed (linked) one: the mock flag makes
    // save_project answer with the Save-a-Copy contract, and the seeded link is
    // what flips the menu label.
    await page.evaluate(() => {
      const w = window as unknown as {
        __MOCK_SET_PROJECT_IS_REMOTE__?: (r: boolean) => void;
        __E2E_TEST__?: {
          speleoStore?: {
            getState: () => { setLinkOnOpen: (a: { permission: string | null }) => void };
          };
        };
      };
      w.__MOCK_SET_PROJECT_IS_REMOTE__?.(true);
      w.__E2E_TEST__?.speleoStore?.getState().setLinkOnOpen({ permission: 'READ_AND_WRITE' });
    });

    const pathBefore = await page.evaluate(
      () =>
        (window as unknown as {
          __E2E_TEST__?: {
            projectStore?: { getState: () => { project: { file_path: string | null } | null } };
          };
        }).__E2E_TEST__?.projectStore?.getState().project?.file_path ?? null,
    );

    await openAppMenu(page);
    const saveAs = page.getByTestId('project-menu-save-as');
    await expect(saveAs).toContainText('Save a Copy');
    await saveAs.click();

    // The mocked native save dialog picks /mock/saved-project.stalx; the mocked
    // save_project answers with unlinked_copy_path. The toast names the copy...
    await expect(page.getByTestId('toast')).toContainText('/mock/saved-project.stalx', {
      timeout: 5000,
    });
    // ...and the session never moved.
    const pathAfter = await page.evaluate(
      () =>
        (window as unknown as {
          __E2E_TEST__?: {
            projectStore?: { getState: () => { project: { file_path: string | null } | null } };
          };
        }).__E2E_TEST__?.projectStore?.getState().project?.file_path ?? null,
    );
    expect(pathAfter).toBe(pathBefore);
    await expect(page.getByTestId('map-canvas')).toBeVisible();
  });
});
