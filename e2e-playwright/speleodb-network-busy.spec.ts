import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage } from './helpers/app';
import type { useSpeleoStore } from '../src/store/speleoStore';
import type { useProjectStore } from '../src/store/projectStore';
import type { useUIStore } from '../src/store/uiStore';

type BusyWindow = Window & {
  __E2E_TEST__: { speleoStore: typeof useSpeleoStore; projectStore: typeof useProjectStore; uiStore: typeof useUIStore };
  __MOCK_IPC_OVERRIDES__: Record<string, unknown>;
  __MOCK_SET_PROJECT_IS_REMOTE__: (remote: boolean) => void;
  /** Answer a held command: `__answer[cmd](value)`. */
  __answer: Record<string, (value: unknown) => void>;
  /** Commands whose next call is held until answered. */
  __held: Set<string>;
};

/** The controls that would start a second lock, upload or download. */
const CONFLICTING = [
  'speleodb-workspace-upload',
  'speleodb-workspace-editing',
  'speleodb-workspace-latest',
  'speleodb-workspace-close',
];

/**
 * Open a synthetic remote project that is being edited, with the SpeleoDB
 * workspace showing. Any command added to `__held` answers only when the test
 * calls `__answer[command](value)`, standing in for a slow server.
 */
async function openEditingRemote(page: Page) {
  await setupMockedPage(page);
  await page.evaluate(async () => {
    const w = window as unknown as BusyWindow;
    w.__MOCK_SET_PROJECT_IS_REMOTE__(true);
    w.__answer = {};
    w.__held = new Set();
    const overrides: Record<string, unknown> = {
      speleodb_session: { logged_in: true, user_email: 'me@example.com', endpoint: 'https://speleodb.example' },
      speleodb_get_revisions: [],
      speleodb_project_access_at: { release_context: { project_id: 'busy-remote', endpoint: 'https://speleodb.example' } },
    };
    const answers: Record<string, unknown> = {
      speleodb_start_editing: { kind: 'editing' },
      speleodb_update: { kind: 'up_to_date' },
      speleodb_release_lock: null,
      speleodb_link_status: {
        is_remote: true, edit_enabled: true, permission: 'READ_AND_WRITE', lock_holder: 'me@example.com',
        has_unpushed_edits: false, unwound: false, remote_state: 'up_to_date', access_denied: false,
        server_error: null, undo_mode: 'full', project_id: 'busy-remote',
      },
    };
    for (const [command, answer] of Object.entries(answers)) {
      Object.defineProperty(overrides, command, {
        enumerable: true,
        get: () => {
          if (!w.__held.has(command)) return answer;
          w.__held.delete(command);
          return new Promise((resolve) => { w.__answer[command] = resolve; });
        },
      });
    }
    w.__MOCK_IPC_OVERRIDES__ = overrides;
    await w.__E2E_TEST__.speleoStore.getState().refreshSession();
    await w.__E2E_TEST__.projectStore.getState().loadProject('/projects/cave-alpha.stalx');
  });
  // The store action every SpeleoDB entry point uses (opens the dock on the tab),
  // rather than one particular button, which the toolbar no longer has to carry.
  await page.evaluate(() => (window as unknown as BusyWindow).__E2E_TEST__.uiStore.getState().openSpeleoWorkspace());
  await expect(page.getByTestId('workspace-tab-speleodb')).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByTestId('speleodb-workspace-editing')).toHaveText('Switch to Read Only Mode');
  await expect(page.getByTestId('speleodb-network-busy')).toHaveCount(0);
}

async function hold(page: Page, command: string) {
  await page.evaluate((cmd) => (window as unknown as BusyWindow).__held.add(cmd), command);
}

async function answer(page: Page, command: string, value: unknown) {
  await page.waitForFunction((cmd) => typeof (window as unknown as BusyWindow).__answer[cmd] === 'function', command);
  await page.evaluate(([cmd, v]) => (window as unknown as BusyWindow).__answer[cmd as string]!(v), [command, value] as const);
}

/**
 * The tab's size and where its label sits inside it, to the sub-pixel, plus the
 * neighbouring tab's width and whether the tab's content overflows it. Relative
 * to the tab, so a reflow of the whole dock (the reload's toast and refresh can
 * nudge it) is not mistaken for the spinner moving the header.
 */
async function tabGeometry(page: Page) {
  return page.getByTestId('workspace-tab-speleodb').evaluate((tab) => {
    const box = tab.getBoundingClientRect();
    const walker = document.createTreeWalker(tab, NodeFilter.SHOW_TEXT);
    let label: Node | null = null;
    while (walker.nextNode()) {
      if (walker.currentNode.textContent?.trim() === 'SpeleoDB') { label = walker.currentNode; break; }
    }
    const range = document.createRange();
    if (label) range.selectNodeContents(label);
    const text = range.getBoundingClientRect();
    const sibling = tab.parentElement?.firstElementChild?.getBoundingClientRect();
    return {
      width: box.width,
      height: box.height,
      labelLeft: text.left - box.left,
      labelRight: text.right - box.left,
      tablistWidth: tab.parentElement?.getBoundingClientRect().width,
      firstTabWidth: sibling?.width,
      overflows: tab.scrollWidth > tab.clientWidth,
    };
  });
}

type Box = { x: number; y: number; width: number; height: number };
const intersects = (a: Box, b: Box) =>
  a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;

/**
 * Pin the left workspace to `width` px, the panel's narrowest floor, so the tab
 * header is measured where it has the least room. Applied to the workspace root
 * (React renders it with no style of its own), which is what the tab strip sizes
 * against.
 */
async function pinWorkspaceWidth(page: Page, width: number) {
  await page.getByRole('tablist', { name: 'Project workspace' }).evaluate((tablist, px) => {
    const root = tablist.parentElement as HTMLElement;
    root.style.width = `${px}px`;
    root.style.minWidth = `${px}px`;
    root.style.maxWidth = `${px}px`;
  }, width);
  await expect.poll(async () => Math.round((await page.getByRole('tablist', { name: 'Project workspace' }).boundingBox())!.width)).toBe(width);
}

const LAYOUTS = [
  { width: null, readOnly: false, name: 'default width' },
  { width: 230, readOnly: false, name: '230px panel' },
  // Read-only puts the lock badge on the tab icon's bottom-right corner.
  { width: 230, readOnly: true, name: '230px panel, read-only badge' },
] as const;

for (const { width, readOnly, name } of LAYOUTS) {
  test(`a slow reload shows the spinner on the SpeleoDB tab without moving it, and locks conflicting actions (${name})`, async ({ page }, testInfo) => {
    await openEditingRemote(page);
    if (width) await pinWorkspaceWidth(page, width);
    if (readOnly) {
      await page.evaluate(() => {
        const store = (window as unknown as BusyWindow).__E2E_TEST__.speleoStore;
        const link = store.getState().link!;
        store.setState({ link: { ...link, editState: 'read-only', lockHolder: null } });
      });
      await expect(page.getByTestId('speleodb-workspace-editing')).toHaveText('Resume editing');
    }
    const before = await tabGeometry(page);
    expect(before.overflows, 'the tab fits its content before anything is in flight').toBe(false);

    await hold(page, 'speleodb_update');
    await page.getByTestId('speleodb-workspace-latest').click();

    const tabLocator = page.getByTestId('workspace-tab-speleodb');
    const spinner = tabLocator.getByTestId('speleodb-network-busy');
    await expect(spinner).toBeVisible();
    await expect(tabLocator).toContainText('Talking to SpeleoDB...');
    await expect(spinner).toHaveAttribute('title', 'Talking to SpeleoDB...');
    const during = await tabGeometry(page);
    expect(during, 'the spinner must not resize the tab or move its label').toEqual(before);

    const tab = (await tabLocator.boundingBox())!;
    const spin = (await spinner.boundingBox())!;
    // Inside the tab.
    expect(spin.x).toBeGreaterThanOrEqual(tab.x);
    expect(spin.x + spin.width).toBeLessThanOrEqual(tab.x + tab.width);
    expect(spin.y).toBeGreaterThanOrEqual(tab.y);
    expect(spin.y + spin.height).toBeLessThanOrEqual(tab.y + tab.height);
    const icon = tabLocator.getByTestId('workspace-tab-speleodb-icon');
    if (await icon.count()) {
      // With the tab icon present, the spinner overlays its top-left corner and
      // stays clear of the state badge on the bottom-right corner.
      await expect(spinner).toHaveAttribute('data-placement', 'icon');
      const iconBox = (await icon.boundingBox())!;
      expect(intersects(spin, iconBox), 'the spinner overlays the icon').toBe(true);
      expect(spin.x).toBeLessThanOrEqual(iconBox.x + 1);
      expect(spin.y).toBeLessThanOrEqual(iconBox.y + 1);
      const badge = tabLocator.getByTestId('workspace-tab-speleodb-icon-state-badge');
      if (readOnly) await expect(badge).toBeVisible();
      if (await badge.count()) {
        expect(intersects(spin, (await badge.boundingBox())!), 'the spinner never covers the state badge').toBe(false);
      }
    } else {
      // No tab icon to overlay: the spinner sits after the label, not on it.
      expect(spin.x - tab.x).toBeGreaterThanOrEqual(before.labelRight);
    }
    await tabLocator.screenshot({ path: testInfo.outputPath(`speleodb-tab-busy-${width ?? 'default'}${readOnly ? '-read-only' : ''}.png`) });

    for (const id of CONFLICTING) await expect(page.getByTestId(id), id).toBeDisabled();
    // Navigation and the rest of the app stay usable.
    await expect(page.getByTestId('speleodb-workspace-section-history')).toBeEnabled();
    await expect(page.getByTestId('workspace-tab-sections')).toBeEnabled();

    await answer(page, 'speleodb_update', { kind: 'up_to_date' });
    await expect(page.getByTestId('speleodb-network-busy')).toHaveCount(0);
    for (const id of CONFLICTING) await expect(page.getByTestId(id), id).toBeEnabled();
    expect(await tabGeometry(page)).toEqual(before);
  });
}

test('an exclusive call that holds no verb claim still locks the conflicting actions', async ({ page }) => {
  // Releasing another project's lock from the held-lock prompt runs outside the
  // hand-off verbs, so only the network count can gate the workspace.
  await openEditingRemote(page);
  await hold(page, 'speleodb_release_lock');
  await page.evaluate(() => {
    void (window as unknown as BusyWindow).__E2E_TEST__.speleoStore.getState().releaseLock('another-project');
  });
  await expect(page.getByTestId('speleodb-network-busy')).toBeVisible();
  expect(await page.evaluate(() => (window as unknown as BusyWindow).__E2E_TEST__.speleoStore.getState().busy)).toBe(false);
  for (const id of CONFLICTING) await expect(page.getByTestId(id), id).toBeDisabled();

  await answer(page, 'speleodb_release_lock', null);
  await expect(page.getByTestId('speleodb-network-busy')).toHaveCount(0);
  for (const id of CONFLICTING) await expect(page.getByTestId(id), id).toBeEnabled();
});

test('a slow status refresh shows the spinner but locks nothing', async ({ page }) => {
  await openEditingRemote(page);
  await hold(page, 'speleodb_link_status');
  await page.evaluate(() => {
    void (window as unknown as BusyWindow).__E2E_TEST__.speleoStore.getState().refreshLinkStatus();
  });
  await expect(page.getByTestId('speleodb-network-busy')).toBeVisible();
  for (const id of CONFLICTING) await expect(page.getByTestId(id), id).toBeEnabled();
  await answer(page, 'speleodb_link_status', {
    is_remote: true, edit_enabled: true, permission: 'READ_AND_WRITE', lock_holder: 'me@example.com',
    has_unpushed_edits: false, unwound: false, remote_state: 'up_to_date', access_denied: false,
    server_error: null, undo_mode: 'full', project_id: 'busy-remote',
  });
  await expect(page.getByTestId('speleodb-network-busy')).toHaveCount(0);
});
