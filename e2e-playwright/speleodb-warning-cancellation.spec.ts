import { test, expect } from './fixtures/coverage';
import { createProject, setupMockedPage } from './helpers/app';

// Synthetic projects only. Exercise the real native-confirm IPC seam.
for (const dialog of ['open-from-speleodb', 'speleodb-managed-file'] as const) {
  for (const warning of ['read-only', 'locked'] as const) {
    test(`${dialog} recovers after cancelling the ${warning} warning`, async ({ page }) => {
      await setupMockedPage(page);
      await page.evaluate(({ dialog, warning }) => {
        const w = window as unknown as {
          __MOCK_IPC_OVERRIDES__: Record<string, unknown>;
          __E2E_TEST__: {
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
      await open.click();
      await expect(open).toBeEnabled();
      await open.click();
      await expect(open).toBeEnabled();
      await page.getByTestId(managed ? 'speleodb-managed-file-cancel' : 'open-from-speleodb-close').click();
      await expect(page.getByTestId(managed ? 'speleodb-managed-file-dialog' : 'open-from-speleodb-dialog')).toBeHidden();
      await expect(page.getByTestId('global-view')).toBeVisible();
    });
  }
}


for (const button of ['speleodb-close-release', 'speleodb-close-push-release']) {
  test(`close protects the destination probe for ${button}`, async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Synthetic close protection');
    await page.evaluate(() => {
      const w = window as unknown as {
        __MOCK_IPC_OVERRIDES__: Record<string, unknown>;
        __finishProbe: () => void;
        __sideEffects: string[];
        __E2E_TEST__: {
          projectStore: { setState: (state: unknown) => void };
          speleoStore: { setState: (state: unknown) => void };
          uiStore: { getState: () => { openDialog: (name: string, data?: unknown) => void } };
        };
      };
      w.__sideEffects = [];
      w.__MOCK_IPC_OVERRIDES__ = {
        'plugin:dialog|confirm': false,
        speleodb_project_access_at: new Promise((resolve) => {
          w.__finishProbe = () => resolve({ project: { name: 'Synthetic destination', permission: 'READ_ONLY', lock_holder: null }, endpoint: 'https://example.com', verification: 'verified' });
        }),
      };
      for (const command of ['save_project', 'speleodb_done', 'speleodb_release_current_lock', 'close_project', 'load_project_full_binary']) {
        Object.defineProperty(w.__MOCK_IPC_OVERRIDES__, command, {
          get: () => { w.__sideEffects.push(command); return null; },
        });
      }
      w.__E2E_TEST__.projectStore.setState({ isDirty: true });
      w.__E2E_TEST__.speleoStore.setState({ busy: false, link: {
        permission: 'READ_AND_WRITE', editState: 'editing', lockedByOther: false,
        lockHolder: null, hasUnpushedEdits: true, unwound: false, forceEditing: false,
        forceEditAllowed: false, remoteState: 'up_to_date', noAccess: false, serverError: null,
      } });
      w.__E2E_TEST__.uiStore.getState().openDialog('speleodb-close', { pendingOpenPath: '/synthetic/destination.stalx' });
    });
    await page.getByRole('textbox', { name: 'Commit message' }).fill('Synthetic update');
    await page.getByTestId(button).click();
    await expect(page.getByTestId('speleodb-close-cancel')).toBeDisabled();
    await expect(page.getByTestId('speleodb-close-save-locally')).toBeDisabled();
    await page.keyboard.press('Escape');
    await page.getByTestId('speleodb-close-dialog-close').click();
    await expect(page.getByTestId('speleodb-close-dialog')).toBeVisible();
    await page.evaluate(() => (window as unknown as { __finishProbe: () => void }).__finishProbe());
    await expect(page.getByTestId(button)).toBeEnabled();
    expect(await page.evaluate(() => (window as unknown as { __sideEffects: string[] }).__sideEffects)).toEqual([]);
    await page.getByTestId('speleodb-close-cancel').click();
    await expect(page.getByTestId('speleodb-close-dialog')).toBeHidden();
  });
}

for (const outcome of ['pushed', 'push_unconfirmed', 'disk_failure'] as const) {
  test(`Save + Push reports ${outcome} and retains the lock`, async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Synthetic Save + Push');
    await page.evaluate((outcome) => {
      const w = window as unknown as {
        __MOCK_IPC_OVERRIDES__: Record<string, unknown>;
        __savePushCalls: string[];
        __E2E_TEST__: {
          projectStore: { setState: (state: unknown) => void };
          speleoStore: { setState: (state: unknown) => void };
          uiStore: { getState: () => { openDialog: (name: string, data?: unknown) => void } };
        };
      };
      w.__savePushCalls = [];
      w.__MOCK_IPC_OVERRIDES__ = { speleodb_session: { logged_in: true, user_email: 'synthetic@example.com', endpoint: 'https://synthetic.example' } };
      for (const command of ['save_project', 'speleodb_push', 'speleodb_done', 'speleodb_release_current_lock']) {
        Object.defineProperty(w.__MOCK_IPC_OVERRIDES__, command, {
          get: () => {
            w.__savePushCalls.push(command);
            return command === 'save_project'
              ? { success: outcome !== 'disk_failure', error: outcome === 'disk_failure' ? 'Synthetic disk failure' : null }
              : { kind: outcome };
          },
        });
      }
      w.__E2E_TEST__.projectStore.setState({ isDirty: true });
      w.__E2E_TEST__.speleoStore.setState({ busy: false, remoteProjectId: 'synthetic', session: { loggedIn: true, userEmail: 'synthetic@example.com', endpoint: 'https://synthetic.example' }, link: {
        permission: 'READ_AND_WRITE', editState: 'editing', lockedByOther: false,
        lockHolder: null, hasUnpushedEdits: true, unwound: false, forceEditing: false,
        forceEditAllowed: false, remoteState: 'up_to_date', noAccess: false, serverError: null,
      } });
      w.__E2E_TEST__.uiStore.getState().openDialog('speleodb-done', { mode: 'push' });
    }, outcome);
    const confirm = page.getByTestId('speleodb-done-confirm');
    await expect(confirm).toBeDisabled();
    await page.getByLabel('Commit message').fill('Synthetic survey update');
    await confirm.focus();
    await page.keyboard.press('Enter');
    if (outcome === 'pushed') {
      await expect(page.getByTestId('speleodb-done-dialog')).toBeHidden();
    } else if (outcome === 'push_unconfirmed') {
      await expect(page.getByTestId('speleodb-save-upload-failure')).toContainText('upload could not be confirmed');
      await expect(page.getByLabel('Commit message')).toHaveValue('Synthetic survey update');
    } else {
      await expect(page.getByTestId('error-dialog')).toBeVisible();
    }
    const calls = await page.evaluate(() => (window as unknown as { __savePushCalls: string[] }).__savePushCalls);
    expect(calls.slice(0, 2)).toEqual(outcome === 'disk_failure' ? ['save_project'] : ['save_project', 'speleodb_push']);
    expect(calls).not.toContain('speleodb_done');
    expect(calls).not.toContain('speleodb_release_current_lock');
  });
}
