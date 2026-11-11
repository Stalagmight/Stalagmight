import { test, expect, type Page } from './fixtures/coverage';
import { setIpcOverrides } from './helpers/ipcOverrides';
import { setupMockedPage } from './helpers/app';
import { systemCardTestId } from './helpers/testIds';

const CACHED = '11111111-1111-1111-1111-111111111111';
const REMOTE = '22222222-2222-2222-2222-222222222222';
const CACHE_PATH = `/cache/speleodb/${CACHED}/project.stalx`;

for (const density of ['cards', 'table'] as const) {
  test(`${density}: cached SpeleoDB projects use remote presentation and download before opening`, async ({ page }) => {
    await page.addInitScript(({ cached, remote, path }) => {
      const metadata = {
        name: 'Old cached name', location: 'Local metadata', file_path: path,
        modified_at: '2026-01-01T00:00:00Z', section_count: 123,
        station_count: 456, total_length: 789, gps_center: null, gps_bounds: null,
        is_remote: true, speleodb_project_id: cached,
      };
      const summaries = [
        { id: cached, name: 'Downloaded cave' },
        { id: remote, name: 'Undownloaded cave' },
      ].map((s) => ({ ...s, permission: 'READ_AND_WRITE', lock_holder: null, type_tag: 'stalx', latest_commit_sha: null }));
      (window as unknown as Record<string, unknown>).__MOCK_IPC_OVERRIDES__ = {
        speleodb_session: { logged_in: true, user_email: 'me@example.com', endpoint: 'https://example.com' },
        speleodb_list_projects: summaries,
        get_recent_projects: [metadata], list_recent_projects: [metadata],
        // Keep the download pending to inspect the home screen during the open.
        speleodb_open_project: new Promise(() => {}),
      };
    }, { cached: CACHED, remote: REMOTE, path: CACHE_PATH });
    const commands: string[] = [];
    page.on('console', (msg) => {
      if (msg.text().startsWith('[Mock IPC] Command:')) commands.push(msg.text());
    });
    await setupMockedPage(page);
    if (density === 'table') await page.getByTestId('recent-density-table').click();
    const prefix = density === 'cards' ? 'system-card' : 'system-row';
    const cached = page.getByTestId(`${prefix}-remote-${CACHED}`);
    const remote = page.getByTestId(`${prefix}-remote-${REMOTE}`);
    await expect(cached).toBeVisible();
    await expect(remote).toBeVisible();
    await expect(cached).toContainText('Downloaded cave');
    await expect(cached).toContainText('SpeleoDB');
    await expect(cached).toContainText('Permission: Read and write');
    await expect(cached).toContainText('Ready for editing');
    await expect(cached).not.toContainText('Old cached name');
    await expect(cached).not.toContainText(CACHE_PATH);
    expect(await cached.getAttribute('class')).toBe(await remote.getAttribute('class'));
    await expect(page.getByTestId(systemCardTestId(CACHE_PATH))).toHaveCount(0);
    await cached.click({ button: 'right' });
    await expect(page.getByTestId('metadata-panel')).toHaveCount(0);
    await expect(page.getByText('Delete Project', { exact: true })).toHaveCount(0);
    // The entry itself is the Open action (no separate Open button): a click pulls
    // the project, and the entry stays disabled while that pull is in flight.
    await expect(page.getByTestId(`${prefix}-remote-open-${CACHED}`)).toHaveCount(0);
    await cached.click();
    await expect(cached).toHaveAttribute('aria-disabled', 'true');
    // A second click while busy must not start a concurrent pull (forced: the
    // entry is aria-disabled, which Playwright would otherwise wait out).
    await cached.click({ force: true });
    expect(commands.filter((c) => c.includes('Command: speleodb_open_project'))).toHaveLength(1);
    expect(commands.filter((c) => /Command: load_project(?:_full(?:_binary)?)?\b/.test(c))).toHaveLength(0);
    await expect(page.getByTestId('metadata-panel')).toHaveCount(0);
  });
}

async function setupRemoteAndLocal(page: Page, pendingDownload = false, remoteOnMap = false): Promise<string[]> {
  await page.addInitScript(({ id, path, pending, onMap }) => {
    const local = { name: 'Local cave', file_path: '/projects/local.stalx', location: null,
      modified_at: '2026-01-01T00:00:00Z', section_count: 1, station_count: 2,
      total_length: 3, gps_center: null, gps_bounds: null, is_remote: false };
    (window as unknown as Record<string, unknown>).__MOCK_IPC_OVERRIDES__ = {
      speleodb_session: { logged_in: true, user_email: 'me@example.com', endpoint: 'https://example.com' },
      speleodb_list_projects: [{ id, name: 'Remote cave', permission: 'READ_AND_WRITE', lock_holder: null, type_tag: 'stalx', latest_commit_sha: null }],
      // The remote has never appeared in local recents, including after a failed first load.
      list_recent_projects: onMap ? [local, { ...local, name: 'Remote cave', file_path: path,
        is_remote: true, speleodb_project_id: id, gps_center: [-87.4631, 20.2137] }] : [local],
      speleodb_open_project: pending ? new Promise(() => {}) : { local_path: path },
    };
  }, { id: CACHED, path: CACHE_PATH, pending: pendingDownload, onMap: remoteOnMap });
  const commands: string[] = [];
  page.on('console', (msg) => {
    if (msg.text().startsWith('[Mock IPC] Command:')) commands.push(msg.text());
  });
  await setupMockedPage(page);
  return commands;
}

for (const density of ['cards', 'table'] as const) {
  test(`${density}: Enter on a remote project does not also open the selected local project`, async ({ page }) => {
    const commands = await setupRemoteAndLocal(page, true);
    // Select a local project first, leaving a target in the window shortcut handler.
    await page.getByTestId(systemCardTestId('/projects/local.stalx')).click();
    await expect(page.getByTestId('metadata-panel')).toBeVisible();
    if (density === 'table') await page.getByTestId('recent-density-table').click();
    const prefix = density === 'cards' ? 'system-card' : 'system-row';
    await page.getByTestId(`${prefix}-remote-${CACHED}`).focus();
    await page.keyboard.press('Enter');
    await expect(page.getByTestId(`${prefix}-remote-${CACHED}`)).toHaveAttribute('aria-disabled', 'true');
    expect(commands.filter((c) => c.includes('Command: speleodb_open_project'))).toHaveLength(1);
    expect(commands.filter((c) => /Command: load_project(?:_full(?:_binary)?)?\b/.test(c))).toHaveLength(0);
    await expect(page.getByTestId('metadata-panel')).toHaveCount(0);
  });
}

test('retry keeps a failed first remote open remote, then forgets it after dismissal', async ({ page }) => {
  const commands = await setupRemoteAndLocal(page);
  await setIpcOverrides(page, {
    load_project_full_binary: { __throw: 'Synthetic load failed' },
    load_project_full: { __throw: 'Synthetic load failed' },
    load_project: { __throw: 'Synthetic load failed' },
  });
  await page.getByTestId(`system-card-remote-${CACHED}`).click();
  await expect(page.getByTestId('loading-progress')).toContainText('Synthetic load failed');
  await page.getByTestId('loading-error-retry').click();
  await expect.poll(() => commands.filter((c) => c.includes('Command: speleodb_open_project')).length).toBe(2);
  await expect(page.getByTestId('loading-progress')).toContainText('Synthetic load failed');
  await page.getByTestId('loading-error-dismiss').click();
  await expect(page.getByTestId('loading-progress')).toHaveCount(0);

  await setIpcOverrides(page, { 'plugin:dialog|open': '/projects/unrelated.stalx' });
  await page.getByTestId('global-open-import-button').click();
  await expect(page.getByTestId('loading-progress')).toContainText('Synthetic load failed');
  const loadCalls = () => commands.filter((c) => /Command: load_project(?:_full(?:_binary)?)?\b/.test(c)).length;
  const beforeRetry = loadCalls();
  await page.getByTestId('loading-error-retry').click();
  await expect.poll(loadCalls).toBeGreaterThan(beforeRetry);
  expect(commands.filter((c) => c.includes('Command: speleodb_open_project'))).toHaveLength(2);
});

test('map keeps its empty panel but never exposes local details for a SpeleoDB selection', async ({ page }) => {
  await setupRemoteAndLocal(page, true, true);
  await page.getByTestId('globalview-view-toggle-map').click();
  await expect(page.getByTestId('metadata-panel-empty')).toBeVisible();
  await page.getByTestId('view-mode-toggle').click();
  const marker = page.getByTestId(`system-marker-${encodeURIComponent(CACHE_PATH)}`);
  await expect(marker).toBeVisible();
  await marker.click();
  await expect(page.getByTestId('metadata-panel')).toHaveCount(0);
  await expect(page.getByTestId('metadata-panel-empty')).toHaveCount(0);
  // Survey Ctrl-click changes only the map store, without onProjectSelect.
  await page.evaluate((path) => {
    const store = (window as unknown as {
      __E2E_TEST__: { globalViewStore: { getState: () => { toggleSystemSelection: (id: string) => void } } };
    }).__E2E_TEST__.globalViewStore;
    store.getState().toggleSystemSelection(path);
  }, CACHE_PATH);
  await expect(page.getByTestId('metadata-panel-empty')).toBeVisible();
  await marker.click();
  await expect(page.getByTestId('metadata-panel-empty')).toHaveCount(0);
  await page.keyboard.press('Escape');
  await expect(page.getByTestId('metadata-panel-empty')).toBeVisible();
});
