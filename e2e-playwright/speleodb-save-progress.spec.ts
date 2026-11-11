import { test, expect } from './fixtures/coverage';
import { createProject, openAppMenu, setupMockedPage } from './helpers/app';
import type { useSpeleoStore } from '../src/store/speleoStore';

type UploadWindow = Window & {
  __E2E_TEST__: { speleoStore: typeof useSpeleoStore };
  __MOCK_IPC_OVERRIDES__: Record<string, unknown>;
  __MOCK_SET_PROJECT_IS_REMOTE__: (remote: boolean) => void;
  __finishUpload: () => void;
  __uploadCalls: number;
};

for (const entry of ['menu', 'Control+s', 'Meta+s']) {
  test(`Save upload blocks the interface during upload from ${entry}`, async ({ page }, testInfo) => {
    await setupMockedPage(page);
    await createProject(page, 'Synthetic shared upload');
    await page.evaluate(() => {
      const w = window as unknown as UploadWindow;
      w.__MOCK_SET_PROJECT_IS_REMOTE__(true);
      w.__uploadCalls = 0;
      const pending = new Promise((resolve) => { w.__finishUpload = () => resolve({ kind: 'pushed' }); });
      const overrides: Record<string, unknown> = {
        speleodb_link_status: { is_remote: true, project_id: 'shared-upload', edit_enabled: true,
          permission: 'READ_AND_WRITE', has_unpushed_edits: true, unwound: false,
          remote_state: 'up_to_date', lock_holder: 'test@example.com', access_denied: false,
          server_error: null, undo_mode: 'full' },
      };
      Object.defineProperty(overrides, 'speleodb_push', { get: () => { w.__uploadCalls += 1; return pending; } });
      w.__MOCK_IPC_OVERRIDES__ = overrides;
      w.__E2E_TEST__.speleoStore.setState({
        session: { loggedIn: true, userEmail: 'test@example.com', endpoint: 'https://speleodb.example' },
        projectLinkKind: 'remote', remoteProjectId: 'shared-upload',
        link: { permission: 'READ_AND_WRITE', editState: 'editing', hasUnpushedEdits: true,
          lockHolder: 'test@example.com', lockedByOther: false, unwound: false, undoMode: 'full',
          remoteState: 'up_to_date', noAccess: false, serverError: null },
      });
    });
    if (entry === 'menu') {
      await openAppMenu(page);
      await page.getByTestId('toolbar-save-button').click();
    } else await page.keyboard.press(entry);
    await expect(page.getByTestId('speleodb-done-dialog')).toBeVisible();
    await expect(page.getByTestId('speleodb-upload-progress')).toHaveCount(0);
    await page.getByTestId('speleodb-done-message').fill('Synthetic survey update');
    await page.getByTestId('speleodb-done-confirm').click();
    const overlay = page.getByTestId('speleodb-upload-progress');
    await expect(overlay).toBeVisible();
    await expect(overlay).toBeFocused();
    await expect(page.locator('#root')).toHaveAttribute('inert', '');
    await expect(overlay).toContainText('Saving changes to SpeleoDB');
    await page.keyboard.press('Escape');
    await expect(overlay).toBeVisible();
    const card = await overlay.getByRole('status').boundingBox();
    expect(Math.abs(card!.x + card!.width / 2 - page.viewportSize()!.width / 2)).toBeLessThan(2);
    expect(Math.abs(card!.y + card!.height / 2 - page.viewportSize()!.height / 2)).toBeLessThan(2);
    if (entry === 'menu') await page.screenshot({ path: testInfo.outputPath('save-upload-progress.png') });
    await page.evaluate(() => (window as unknown as UploadWindow).__finishUpload());
    await expect(overlay).toHaveCount(0);
    await expect(page.getByTestId('speleodb-done-dialog')).toHaveCount(0);
    expect(await page.evaluate(() => (window as unknown as UploadWindow).__uploadCalls)).toBe(1);
  });
}
