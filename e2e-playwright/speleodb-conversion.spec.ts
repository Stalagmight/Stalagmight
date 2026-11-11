import { test, expect } from './fixtures/coverage';
import { createProject, setupMockedPage } from './helpers/app';
import { openSpeleoTab, revealSpeleoTab } from './helpers/speleoWorkspace';
import type { useProjectStore } from '../src/store/projectStore';
import type { useSpeleoStore } from '../src/store/speleoStore';

type ConversionWindow = Window & {
  __E2E_TEST__: { projectStore: typeof useProjectStore; speleoStore: typeof useSpeleoStore };
  __MOCK_IPC_OVERRIDES__: Record<string, unknown>;
  __MOCK_SET_PROJECT_IS_REMOTE__: (remote: boolean) => void;
  __conversionCalls: string[];
};

for (const viewport of [{ width: 1280, height: 800 }, { width: 540, height: 720 }]) {
  test(`local conversion prompt is centered with versioning and team copy at ${viewport.width}px`, async ({ page }, testInfo) => {
    await page.setViewportSize(viewport);
    await setupMockedPage(page);
    await createProject(page, 'Synthetic team survey');
    // At 540px the side-panel squeeze can fold the left panel away: reveal it with
    // its opener first. The wide window uses the tab itself.
    if (viewport.width < 800) await revealSpeleoTab(page); else await openSpeleoTab(page);
    const panel = page.getByTestId('speleodb-workspace-local');
    const prompt = page.getByTestId('speleodb-workspace-conversion-prompt');
    const button = page.getByTestId('speleodb-workspace-convert');
    await expect(button).toBeVisible();
    await expect(panel.getByRole('button')).toHaveCount(1);
    await expect(panel).toContainText('revisit earlier versions');
    await expect(panel).toContainText("your team's work together");
    await expect(panel).not.toContainText(/\b(git|lock)\b/i);
    await expect.poll(async () => {
      const container = await panel.boundingBox();
      const body = await prompt.boundingBox();
      const action = await button.boundingBox();
      if (!container || !body || !action) return 999;
      return Math.max(
        Math.abs(container.x + container.width / 2 - action.x - action.width / 2),
        Math.abs(container.y + container.height / 2 - body.y - body.height / 2),
      );
    }).toBeLessThan(3);
    await panel.screenshot({ path: testInfo.outputPath('local-conversion-prompt.png') });
    await button.click();
    await expect(page.getByTestId('publish-to-speleodb-dialog')).toBeVisible();
    await expect(page.getByTestId('publish-to-speleodb-dialog-name')).toHaveValue('Synthetic team survey');
  });
}

test('local conversion publishes, fetches the server copy and opens the remote workspace', async ({ page }) => {
  await setupMockedPage(page);
  await createProject(page, 'Synthetic conversion');
  await page.evaluate(() => {
    const w = window as unknown as ConversionWindow;
    w.__conversionCalls = [];
    let editing = false;
    const overrides: Record<string, unknown> = {
      speleodb_session: { logged_in: true, user_email: 'test@example.com', endpoint: 'https://speleodb.example' },
    };
    Object.defineProperty(overrides, 'speleodb_publish', { get: () => {
      w.__conversionCalls.push('publish');
      w.__MOCK_SET_PROJECT_IS_REMOTE__(true);
      return { project_id: 'converted', endpoint: 'https://speleodb.example', local_path: '/cache/converted/project.stalx' };
    } });
    Object.defineProperty(overrides, 'speleodb_link_status', { get: () => ({
      is_remote: true, project_id: 'converted', edit_enabled: editing, permission: 'ADMIN',
      has_unpushed_edits: false, unwound: false, remote_state: 'up_to_date',
      access_denied: false, server_error: null, undo_mode: 'full',
      lock_holder: editing ? 'test@example.com' : null,
    }) });
    Object.defineProperty(overrides, 'speleodb_open_project', { get: () => {
      w.__conversionCalls.push('download');
      return { local_path: '/cache/converted/project.stalx', offline: false };
    } });
    Object.defineProperty(overrides, 'speleodb_start_editing', { get: () => {
      w.__conversionCalls.push('acquire'); editing = true; return { kind: 'editing' };
    } });
    w.__MOCK_IPC_OVERRIDES__ = overrides;
    w.__E2E_TEST__.speleoStore.setState({ session: { loggedIn: true, userEmail: 'test@example.com', endpoint: 'https://speleodb.example' } });
  });
  await openSpeleoTab(page);
  await page.getByTestId('speleodb-workspace-convert').click();
  await page.getByTestId('publish-to-speleodb-dialog-description').fill('A shared survey with revision history');
  await page.getByTestId('publish-to-speleodb-dialog-country').click();
  await page.getByTestId('publish-to-speleodb-dialog-country-option-US').click();
  await page.getByTestId('publish-to-speleodb-dialog-submit').click();
  await expect(page.getByTestId('publish-to-speleodb-dialog')).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => (window as unknown as ConversionWindow).__conversionCalls)).toEqual(['publish', 'download', 'acquire']);
  await openSpeleoTab(page);
  await expect(page.getByTestId('speleodb-workspace-upload')).toBeVisible();
  await expect(page.getByTestId('speleodb-workspace-convert')).toHaveCount(0);
  expect(await page.evaluate(() => (window as unknown as ConversionWindow).__E2E_TEST__.speleoStore.getState().projectLinkKind)).toBe('remote');
  expect(await page.evaluate(() => (window as unknown as ConversionWindow).__E2E_TEST__.projectStore.getState().project?.file_path)).toBe('/cache/converted/project.stalx');
});


test('publishing shows a centered app-blocking spinner until the conversion completes', async ({ page }, testInfo) => {
  await setupMockedPage(page);
  await createProject(page, 'Synthetic pending conversion');
  await page.evaluate(() => {
    const w = window as unknown as ConversionWindow & { __finishConversion: () => void };
    w.__E2E_TEST__.speleoStore.setState({ session: { loggedIn: true, userEmail: 'test@example.com', endpoint: 'https://speleodb.example' } });
    w.__MOCK_IPC_OVERRIDES__ = {
      speleodb_publish: new Promise((resolve) => {
        w.__finishConversion = () => resolve({ project_id: 'pending', endpoint: 'https://speleodb.example', local_path: '/cache/pending/project.stalx' });
      }),
      speleodb_link_status: { is_remote: true, project_id: 'pending', edit_enabled: false, has_unpushed_edits: true, permission: 'ADMIN', remote_state: 'up_to_date' },
    };
  });
  await openSpeleoTab(page);
  await page.getByTestId('speleodb-workspace-convert').click();
  await page.getByTestId('publish-to-speleodb-dialog-description').fill('Team survey');
  await page.getByTestId('publish-to-speleodb-dialog-country').click();
  await page.getByTestId('publish-to-speleodb-dialog-country-option-US').click();
  await page.getByTestId('publish-to-speleodb-dialog-submit').click();
  const overlay = page.getByTestId('speleodb-publish-progress');
  await expect(overlay).toBeVisible();
  await expect(overlay).toBeFocused();
  await expect(page.locator('#root')).toHaveAttribute('inert', '');
  const bounds = await overlay.boundingBox();
  expect(bounds?.x).toBe(0);
  expect(bounds?.y).toBe(0);
  expect(bounds?.width).toBe(page.viewportSize()!.width);
  expect(bounds?.height).toBe(page.viewportSize()!.height);
  await page.keyboard.press('Escape');
  await expect(overlay).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath('publish-progress-overlay.png') });
  await page.evaluate(() => (window as unknown as { __finishConversion: () => void }).__finishConversion());
  await expect(overlay).toHaveCount(0);
  await expect(page.getByTestId('publish-to-speleodb-dialog')).toHaveCount(0);
});
