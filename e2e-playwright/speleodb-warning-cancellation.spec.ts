import { test, expect } from './fixtures/coverage';
import { createProject, setupMockedPage } from './helpers/app';

// Synthetic projects only. The unsaved-edits question is the in-app exit confirm.
for (const dialog of ['open-from-speleodb', 'speleodb-managed-file'] as const) {
  for (const warning of ['read-only', 'locked'] as const) {
    test(`${dialog} preserves unsaved work when cancelling a switch to a ${warning} project`, async ({ page }) => {
      await setupMockedPage(page);
      await createProject(page, 'Synthetic unsaved source');
      await page.evaluate(({ dialog, warning }) => {
        const w = window as unknown as {
          __MOCK_IPC_OVERRIDES__: Record<string, unknown>;
          __E2E_TEST__: {
            projectStore: { setState: (state: unknown) => void };
            speleoStore: { setState: (state: unknown) => void };
            uiStore: { getState: () => { openDialog: (name: string, data?: unknown) => void } };
          };
        };
        w.__MOCK_IPC_OVERRIDES__ = {
          speleodb_session: { logged_in: true, endpoint: 'https://synthetic.example', user_email: 'test@example.com' },
          speleodb_list_projects: [{ id: 'synthetic', name: 'Synthetic cave', type_tag: 'stalx',
            latest_commit_sha: null, permission: warning === 'read-only' ? 'READ_ONLY' : 'READ_AND_WRITE',
            lock_holder: warning === 'locked' ? 'other@example.com' : null }],
          'plugin:dialog|confirm': false,
        };
        w.__E2E_TEST__.projectStore.setState({ isDirty: true });
        w.__E2E_TEST__.speleoStore.setState({ session: {
          loggedIn: true, endpoint: 'https://synthetic.example', userEmail: 'test@example.com',
        } });
        w.__E2E_TEST__.uiStore.getState().openDialog(dialog, dialog === 'speleodb-managed-file' ? {
          managed: { path: '/synthetic/copy.stalx', projectId: 'synthetic', projectName: 'Synthetic cave',
            endpoint: 'https://synthetic.example', canOpenRemote: true },
        } : {});
      }, { dialog, warning });
      const managed = dialog === 'speleodb-managed-file';
      const open = page.getByTestId(managed ? 'speleodb-managed-file-open-remote' : 'open-from-speleodb-open-synthetic');
      await expect(open).toBeEnabled();
      // The unsaved-edits question is in-app (never the native confirm); Cancel
      // keeps the work and the browser.
      for (let attempt = 0; attempt < 2; attempt += 1) {
        await open.click();
        await expect(page.getByTestId('exit-confirm-dialog')).toBeVisible();
        await page.getByTestId('exit-confirm-cancel').click();
        await expect(page.getByTestId('exit-confirm-dialog')).toHaveCount(0);
        await expect(open).toBeEnabled();
      }
      await page.getByTestId(managed ? 'speleodb-managed-file-cancel' : 'open-from-speleodb-close').click();
      await expect(page.getByTestId(managed ? 'speleodb-managed-file-dialog' : 'open-from-speleodb-dialog')).toBeHidden();
      await expect(page.getByTestId('single-system-view')).toBeVisible();
    });
  }
}


// Opening another file over un-uploaded work under our lock: the destination's disk
// preflight runs BEFORE the exit prompt, so a destination that cannot be opened
// fails with nothing given up - no prompt, no save, no upload, no close, no load.
test('a failing destination probe stops a switch before the exit prompt changes anything', async ({ page }) => {
  await setupMockedPage(page);
  await createProject(page, 'Synthetic close protection');
  await page.evaluate(() => {
    const w = window as unknown as {
      __MOCK_IPC_OVERRIDES__: Record<string, unknown>;
      __sideEffects: string[];
      __E2E_TEST__: {
        projectStore: { setState: (state: unknown) => void };
        speleoStore: { setState: (state: unknown) => void };
        routeOpenedFile: (path: string) => Promise<void>;
      };
    };
    w.__sideEffects = [];
    w.__MOCK_IPC_OVERRIDES__ = {
      speleodb_project_access_at: { __throw: 'Synthetic probe failure' },
    };
    for (const command of ['save_project', 'speleodb_done', 'speleodb_release_current_lock', 'close_project', 'load_project_full_binary']) {
      Object.defineProperty(w.__MOCK_IPC_OVERRIDES__, command, {
        get: () => { w.__sideEffects.push(command); return null; },
      });
    }
    w.__E2E_TEST__.projectStore.setState({ isDirty: true });
    w.__E2E_TEST__.speleoStore.setState({ busy: false, link: {
      permission: 'READ_AND_WRITE', editState: 'editing', lockedByOther: false,
      lockHolder: null, hasUnpushedEdits: true, unwound: false,
      remoteState: 'up_to_date', noAccess: false, serverError: null,
    } });
    void w.__E2E_TEST__.routeOpenedFile('/synthetic/destination.stalx');
  });
  await expect(page.getByTestId('error-dialog')).toContainText('Synthetic probe failure');
  await expect(page.getByTestId('speleodb-exit-dialog')).toHaveCount(0);
  expect(await page.evaluate(() => (window as unknown as { __sideEffects: string[] }).__sideEffects)).toEqual([]);
  await expect(page.getByTestId('single-system-view')).toBeVisible();
});
