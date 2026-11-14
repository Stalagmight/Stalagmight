import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage, openAppMenu } from './helpers/app';
import { openSpeleoTab, revealSpeleoTab } from './helpers/speleoWorkspace';
import { LEFT_WORKSPACE_MIN_WIDTH } from '../src/components/SingleSystemView/layoutConstants';
import { themeTokens } from '../src/theme/tokens';
import type { useUIStore } from '../src/store/uiStore';
import type { useSpeleoStore } from '../src/store/speleoStore';
import type { useProjectStore } from '../src/store/projectStore';
type TestWindow = Window & { __E2E_TEST__: { uiStore: typeof useUIStore; speleoStore: typeof useSpeleoStore; projectStore: typeof useProjectStore }; __MOCK_IPC_OVERRIDES__: Record<string, unknown> };


async function openRemote(page: Page, lockedByOther: boolean) {
  await setupMockedPage(page);
  await page.evaluate(async (contended) => {
    const w = window as unknown as {
      __MOCK_SET_PROJECT_IS_REMOTE__: (remote: boolean) => void;
      __MOCK_IPC_OVERRIDES__: Record<string, unknown>;
      __lockCalls: string[];
      __E2E_TEST__: {
        projectStore: { getState: () => { loadProject: (path: string) => Promise<unknown>; closeProject: () => Promise<unknown> } };
      };
    };
    w.__MOCK_SET_PROJECT_IS_REMOTE__(true);
    w.__lockCalls = [];
    let editing = false;
    const overrides: Record<string, unknown> = {
      speleodb_session: { logged_in: true, user_email: 'me@example.com', endpoint: 'https://example.com' },
      speleodb_get_revisions: [],
      speleodb_project_access_at: { release_context: { project_id: 'remote', endpoint: 'https://example.com' } },
    };
    Object.defineProperty(overrides, 'speleodb_start_editing', { get: () => {
      w.__lockCalls.push('acquire');
      editing = !contended;
      return contended ? { kind: 'locked_by', email: 'editor@example.com' } : { kind: 'editing' };
    } });
    Object.defineProperty(overrides, 'speleodb_release_current_lock', { get: () => {
      w.__lockCalls.push('release'); editing = false; return null;
    } });
    Object.defineProperty(overrides, 'speleodb_link_status', { get: () => ({
      is_remote: true, edit_enabled: editing,
      permission: 'READ_AND_WRITE', lock_holder: contended ? 'editor@example.com' : editing ? 'me@example.com' : null,
      has_unpushed_edits: false, unwound: false, remote_state: 'up_to_date',
      access_denied: false, server_error: null, undo_mode: 'full',
      project_id: 'remote',
    }) });
    w.__MOCK_IPC_OVERRIDES__ = overrides;
    await (window as unknown as TestWindow).__E2E_TEST__.speleoStore.getState().refreshSession();
    await w.__E2E_TEST__.projectStore.getState().loadProject('/projects/cave-alpha.stalx');
  }, lockedByOther);
}

test('opening acquires silently; workspace release stays open and does not reacquire', async ({ page }) => {
  await openRemote(page, false);
  await openSpeleoTab(page);
  await expect(page.getByTestId('speleodb-workspace-editing')).toHaveText('Switch to Read Only Mode');
  await expect(page.getByTestId('speleodb-read-only-dialog')).toHaveCount(0);
  await page.getByTestId('speleodb-workspace-editing').click();
  await expect(page.getByTestId('speleodb-workspace-editing')).toHaveText('Resume editing');
  await expect(page.getByTestId('single-system-view')).toBeVisible();
  expect(await page.evaluate(() => (window as unknown as { __lockCalls: string[] }).__lockCalls)).toEqual(['acquire', 'release']);
  await openSpeleoTab(page);
  await page.getByTestId('speleodb-workspace-editing').click();
  await expect(page.getByTestId('speleodb-workspace-editing')).toHaveText('Switch to Read Only Mode');
});

test('contended open shows one read-only modal naming the actual editor', async ({ page }) => {
  await openRemote(page, true);
  const notice = page.getByTestId('speleodb-read-only-dialog');
  await expect(notice).toBeVisible();
  await expect(notice).toContainText('editor@example.com is currently editing');
  await page.getByTestId('speleodb-read-only-continue').click();
  await expect(notice).toHaveCount(0);
  await openSpeleoTab(page);
  await expect(page.getByTestId('speleodb-workspace-project-header')).toContainText('editor@example.com');
  expect(await page.evaluate(() => (window as unknown as { __lockCalls: string[] }).__lockCalls)).toEqual(['acquire']);
});


test('workspace contains everyday controls and Revision History without history-step controls', async ({ page }) => {
  await openRemote(page, false);
  await openSpeleoTab(page);
  await expect(page.getByTestId('speleodb-workspace-upload')).toHaveText('Save Changes');
  await expect(page.getByTestId('speleodb-workspace-latest')).toHaveText('Reload Project');
  const history = page.getByTestId('speleodb-workspace-section-history');
  await expect(history).toContainText('Revision History');
  await history.click();
  await expect(page.getByTestId('speleodb-revisions').getByRole('heading')).toHaveCount(0);
  await expect(page.getByTestId('speleodb-history-back')).toHaveCount(0);
  await expect(page.getByTestId('speleodb-history-forward')).toHaveCount(0);
  const webHistory = page.getByTestId('speleodb-web-revisions');
  await expect(webHistory).toHaveText('Revision History on SpeleoDB');
  // An ordinary button (the standard padding and font, no fixed height: a long
  // label wraps), with the external-link glyph.
  await expect(webHistory).toHaveCSS('padding', '6px 12px');
  await expect(webHistory).toHaveCSS('font-size', await page.getByTestId('speleodb-workspace-latest').evaluate((b) => getComputedStyle(b).fontSize));
  await expect(webHistory).toHaveCSS('box-shadow', 'none');
  await expect(webHistory.locator('svg')).toBeVisible();
  await expect(page.getByTestId('speleodb-workspace-section-projects')).toHaveCount(0);
  await expect(page.getByTestId('speleodb-workspace-close')).toHaveText('Close Project');
  await expect(page.getByTestId('speleodb-workspace-section-settings')).toHaveCount(0);
  await expect(page.getByTestId('speleodb-workspace-section-account')).toHaveCount(0);
  await expect(page.getByTestId('speleodb-settings-name')).toHaveCount(0);
  expect(await page.evaluate(() => (window as unknown as { __lockCalls: string[] }).__lockCalls)).toEqual(['acquire']);
});

test('the SpeleoDB tab carries into Plan and Export, and the hamburger has no SpeleoDB row', async ({ page }) => {
  await openRemote(page, false);
  const setMode = (mode: string) => page.evaluate((m) => (window as unknown as TestWindow).__E2E_TEST__.uiStore.getState().setAppMode(m as never), mode);
  // Leaving Survey with the SpeleoDB tab on screen keeps it in Plan and Export.
  await openSpeleoTab(page);
  await setMode('plan');
  await expect(page.getByTestId('speleodb-workspace')).toBeVisible();
  await expect(page.getByTestId('workspace-tab-sections')).toHaveCount(0);
  await setMode('export');
  await expect(page.getByTestId('speleodb-workspace')).toBeVisible();
  await setMode('survey');
  await expect(page.getByTestId('workspace-tab-speleodb')).toHaveAttribute('aria-selected', 'true');
  // The hamburger's former "SpeleoDB" row (it only switched to this tab) is gone.
  await openAppMenu(page);
  await expect(page.getByTestId('back-button')).toBeVisible();
  await expect(page.getByTestId('project-menu-speleodb')).toHaveCount(0);
});

test('a narrow window reaches the SpeleoDB tab through the panel opener, and a narrow saved panel resizes immediately', async ({ page }) => {
  await page.setViewportSize({ width: 540, height: 720 });
  await openRemote(page, false);
  await page.evaluate(() => (window as unknown as TestWindow).__E2E_TEST__.uiStore.setState({ sectionsPanelWidth: 200 }));
  await expect(page.getByTestId('speleodb-status-button')).toHaveCount(0);
  await revealSpeleoTab(page);
  const panel = page.getByTestId('sections-panel-bar');
  const handle = page.getByTestId('sections-resize-handle');
  await expect(panel).toBeVisible();
  await expect.poll(async () => (await panel.boundingBox())?.width ?? 0).toBeGreaterThanOrEqual(LEFT_WORKSPACE_MIN_WIDTH - 0.5);
  const initial = await panel.boundingBox();
  const box = await handle.boundingBox();
  expect(box).not.toBeNull();
  await page.mouse.move(box!.x + box!.width / 2, box!.y + box!.height / 2);
  await page.mouse.down();
  await page.mouse.move(box!.x + box!.width / 2 + 30, box!.y + box!.height / 2, { steps: 5 });
  await page.mouse.up();
  await expect.poll(async () => (await panel.boundingBox())?.width ?? 0).toBeGreaterThan(initial!.width + 20);
});

test('upload saves locally, retains the lock; after a release Save saves locally and takes the lock back to upload', async ({ page }) => {
  await openRemote(page, false);
  await page.evaluate(() => {
    const w = window as unknown as TestWindow;
    w.__MOCK_IPC_OVERRIDES__.speleodb_push = { kind: 'pushed' };
    const store = w.__E2E_TEST__.speleoStore;
    store.setState({ link: { ...store.getState().link!, hasUnpushedEdits: true } });
  });
  await openSpeleoTab(page);
  await page.getByTestId('speleodb-workspace-upload').click();
  await page.getByTestId('speleodb-done-message').fill('Synthetic survey update');
  await page.getByTestId('speleodb-done-confirm').click();
  await expect(page.getByTestId('speleodb-done-dialog')).toHaveCount(0);
  await expect(page.getByTestId('speleodb-workspace-editing')).toHaveText('Switch to Read Only Mode');
  await page.getByTestId('speleodb-workspace-editing').click();
  await expect(page.getByTestId('speleodb-workspace-editing')).toHaveText('Resume editing');
  // frontend#6: new work without the lock. Save puts it on disk first, then takes
  // the lock back so the upload can go ahead.
  await page.evaluate(() => {
    const w = window as unknown as TestWindow;
    w.__E2E_TEST__.projectStore.setState({ isDirty: true });
    const store = w.__E2E_TEST__.speleoStore;
    store.setState({ link: { ...store.getState().link!, hasUnpushedEdits: true } });
  });
  await page.getByTestId('speleodb-workspace-upload').click();
  await expect(page.getByTestId('speleodb-done-dialog')).toBeVisible();
  await page.getByTestId('speleodb-done-message').fill('Synthetic second update');
  await expect(page.getByTestId('speleodb-done-confirm')).toBeEnabled();
  expect(await page.evaluate(() => (window as unknown as { __lockCalls: string[] }).__lockCalls)).toEqual(['acquire', 'release', 'acquire']);
  expect(await page.evaluate(() => (window as unknown as TestWindow).__E2E_TEST__.projectStore.getState().isDirty)).toBe(false);
});

test('Save while someone else holds the lock saves on this computer and says why it was not uploaded', async ({ page }) => {
  await openRemote(page, true);
  await page.getByTestId('speleodb-read-only-continue').click();
  await page.evaluate(() => {
    const w = window as unknown as TestWindow;
    w.__E2E_TEST__.projectStore.setState({ isDirty: true });
    const store = w.__E2E_TEST__.speleoStore;
    store.setState({ link: { ...store.getState().link!, hasUnpushedEdits: true } });
  });
  await openSpeleoTab(page);
  await page.getByTestId('speleodb-workspace-upload').click();
  await expect(page.getByTestId('toast').filter({ hasText: 'Saved on this computer' })).toContainText('editor@example.com');
  await expect(page.getByTestId('speleodb-done-dialog')).toHaveCount(0);
  expect(await page.evaluate(() => (window as unknown as TestWindow).__E2E_TEST__.projectStore.getState().isDirty)).toBe(false);
  expect(await page.evaluate(() => (window as unknown as { __lockCalls: string[] }).__lockCalls)).toEqual(['acquire']);
});

test('read-only switching refuses unuploaded work and sign-in opens application Settings', async ({ page }) => {
  await openRemote(page, false);
  await page.evaluate(() => {
    const store = (window as unknown as TestWindow).__E2E_TEST__.speleoStore;
    store.setState({ link: { ...store.getState().link!, hasUnpushedEdits: true } });
  });
  await openSpeleoTab(page);
  await page.getByTestId('speleodb-workspace-editing').click();
  await expect(page.getByTestId('error-dialog')).toContainText('Save your changes using Save Changes');
  expect(await page.evaluate(() => (window as unknown as { __lockCalls: string[] }).__lockCalls)).toEqual(['acquire']);
  await page.evaluate(() => {
    const w = window as unknown as TestWindow;
    w.__E2E_TEST__.uiStore.getState().closeDialog();
    w.__MOCK_IPC_OVERRIDES__.speleodb_session = { logged_in: false, user_email: null, endpoint: 'https://example.com' };
    w.__E2E_TEST__.speleoStore.setState({ session: null, link: { ...w.__E2E_TEST__.speleoStore.getState().link!, editState: 'read-only' } });
  });
  await page.getByTestId('speleodb-workspace-editing').click();
  await expect(page.getByTestId('settings-panel-speleodb')).toBeVisible();
  await expect(page.getByTestId('speleodb-account-login-form')).toBeVisible();
});


test('workspace Close Project asks about unsaved work, cancels safely and closes to the listing', async ({ page }) => {
  await openRemote(page, false);
  await page.evaluate(() => (window as unknown as TestWindow).__E2E_TEST__.projectStore.setState({ isDirty: true }));
  await openSpeleoTab(page);
  await page.getByTestId('speleodb-workspace-close').click();
  await expect(page.getByTestId('speleodb-exit-dialog')).toBeVisible();
  await page.getByTestId('speleodb-exit-cancel').click();
  await expect(page.getByTestId('single-system-view')).toBeVisible();
  expect(await page.evaluate(() => (window as unknown as TestWindow).__E2E_TEST__.projectStore.getState().isDirty)).toBe(true);
  await page.getByTestId('speleodb-workspace-close').click();
  await page.getByTestId('speleodb-exit-discard').click();
  await page.getByTestId('speleodb-exit-discard-confirm').click();
  await expect(page.getByTestId('global-view')).toBeVisible();
  expect(await page.evaluate(() => (window as unknown as TestWindow).__E2E_TEST__.projectStore.getState().project)).toBeNull();
});


test('Settings authentication returns the Save description and close intent without uploading', async ({ page }) => {
  await openRemote(page, false);
  await page.evaluate(() => {
    const w = window as unknown as TestWindow;
    w.__MOCK_IPC_OVERRIDES__.speleodb_session = { logged_in: false, user_email: null, endpoint: 'https://example.com' };
    w.__MOCK_IPC_OVERRIDES__.speleodb_login = { user_email: 'me@example.com', endpoint: 'https://example.com' };
    for (const cmd of ['speleodb_push', 'speleodb_done', 'save_project']) {
      Object.defineProperty(w.__MOCK_IPC_OVERRIDES__, cmd, { get: () => { throw new Error('Authentication must not save or upload'); } });
    }
    w.__E2E_TEST__.speleoStore.setState({ session: null, link: { ...w.__E2E_TEST__.speleoStore.getState().link!, editState: 'read-only', hasUnpushedEdits: true } });
    w.__E2E_TEST__.uiStore.getState().openDialog('speleodb-done', { mode: 'push', closeAfterSave: true });
  });
  await page.getByTestId('speleodb-done-message').fill('Validated synthetic survey update');
  await page.getByTestId('speleodb-upload-sign-in').click();
  await expect(page.getByTestId('settings-panel-speleodb')).toBeVisible();
  await page.getByTestId('speleodb-settings-email').fill('me@example.com');
  await page.getByTestId('speleodb-settings-password').fill('synthetic-password');
  await page.getByTestId('speleodb-settings-try-login').click();
  await expect(page.getByTestId('speleodb-done-dialog')).toBeVisible();
  await expect(page.getByTestId('speleodb-done-message')).toHaveValue('Validated synthetic survey update');
  expect(await page.evaluate(() => (window as unknown as TestWindow).__E2E_TEST__.uiStore.getState().dialogData.closeAfterSave)).toBe(true);
  await expect(page.getByTestId('single-system-view')).toBeVisible();
});

test('un-uploaded changes turn the tab icon and text yellow, with no "!" badge', async ({ page }) => {
  await openRemote(page, false);
  await page.evaluate(() => (window as unknown as TestWindow).__E2E_TEST__.projectStore.setState({ isDirty: true }));
  const tab = page.getByTestId('workspace-tab-speleodb');
  await expect(tab).toHaveAttribute('data-state', 'unuploaded');
  await expect(tab.getByTestId('workspace-tab-speleodb-icon-state-badge')).toHaveCount(0);
  /** The painted colours of the mark and the label, and the warning text colours they should be. */
  const colours = () => tab.evaluate((el) => {
    const probe = (cls: string) => {
      const span = document.createElement('span');
      span.className = cls;
      el.appendChild(span);
      const colour = getComputedStyle(span).color;
      span.remove();
      return colour;
    };
    const logo = el.querySelector('[data-testid="workspace-tab-speleodb-icon-logo"]')!;
    const label = Array.from(el.querySelectorAll('span')).find((s) => s.textContent === 'SpeleoDB')!;
    return {
      icon: getComputedStyle(logo).backgroundColor,
      text: getComputedStyle(label).color,
      full: probe('text-warning-text'),
      dim: probe('text-warning-text-dim'),
    };
  });
  // Not selected: icon and text in the dimmed warning text, alike.
  await expect(tab).toHaveAttribute('aria-selected', 'false');
  const idle = await colours();
  expect(idle.icon).toBe(idle.text);
  expect(idle.text).toBe(idle.dim);
  // Selected: both at full strength.
  await openSpeleoTab(page);
  const selected = await colours();
  expect(selected.icon).toBe(selected.text);
  expect(selected.text).toBe(selected.full);
  expect(selected.full).not.toBe(selected.dim);
});

test('the read-only map pill offers Resume editing, which retakes the lock', async ({ page }) => {
  await openRemote(page, false);
  await openSpeleoTab(page);
  await page.getByTestId('speleodb-workspace-editing').click();
  const pill = page.getByTestId('speleodb-readonly-banner');
  await expect(pill).toBeVisible();
  // No yellow read-only bar above the map any more: the pill and the tab's lock say it.
  await expect(page.getByTestId('speleodb-access-notice')).toHaveCount(0);
  await expect(page.getByTestId('workspace-tab-speleodb')).toHaveAttribute('data-state', 'read-only');
  await pill.getByTestId('speleodb-readonly-resume').click();
  await expect(pill).toHaveCount(0);
  await expect(page.getByTestId('speleodb-workspace-editing')).toHaveText('Switch to Read Only Mode');
  expect(await page.evaluate(() => (window as unknown as { __lockCalls: string[] }).__lockCalls)).toEqual(['acquire', 'release', 'acquire']);
});

test('the read-only pill hides Resume editing when it cannot apply', async ({ page }) => {
  await openRemote(page, false);
  await openSpeleoTab(page);
  await page.getByTestId('speleodb-workspace-editing').click();
  const pill = page.getByTestId('speleodb-readonly-banner');
  await expect(pill.getByTestId('speleodb-readonly-resume')).toBeVisible();
  await page.evaluate(() => {
    const store = (window as unknown as TestWindow).__E2E_TEST__.speleoStore;
    store.setState({ link: { ...store.getState().link!, permission: 'READ_ONLY' } });
  });
  await expect(pill).toBeVisible();
  await expect(pill.getByTestId('speleodb-readonly-resume')).toHaveCount(0);
  await page.evaluate(() => {
    const store = (window as unknown as TestWindow).__E2E_TEST__.speleoStore;
    store.setState({ session: null, link: { ...store.getState().link!, permission: 'READ_AND_WRITE' } });
  });
  await expect(pill).toBeVisible();
  await expect(pill.getByTestId('speleodb-readonly-resume')).toHaveCount(0);
});

test('Reload Project asks in the in-app dialog and Cancel keeps the work', async ({ page }) => {
  await openRemote(page, false);
  await page.evaluate(() => {
    const w = window as unknown as TestWindow & { __updateCalls: number };
    w.__updateCalls = 0;
    Object.defineProperty(w.__MOCK_IPC_OVERRIDES__, 'speleodb_update', { get: () => { w.__updateCalls += 1; return { kind: 'up_to_date' }; } });
    w.__E2E_TEST__.projectStore.setState({ isDirty: true });
  });
  await openSpeleoTab(page);
  await page.getByTestId('speleodb-workspace-latest').click();
  const dialog = page.getByTestId('speleodb-reload-confirm-dialog');
  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText('Discard changes removes unsaved and unuploaded modifications');
  await page.getByTestId('speleodb-reload-confirm-cancel').click();
  await expect(dialog).toHaveCount(0);
  expect(await page.evaluate(() => (window as unknown as { __updateCalls: number }).__updateCalls)).toBe(0);
  expect(await page.evaluate(() => (window as unknown as TestWindow).__E2E_TEST__.projectStore.getState().isDirty)).toBe(true);
  await page.getByTestId('speleodb-workspace-latest').click();
  await page.getByTestId('speleodb-reload-confirm-discard').click();
  await expect(dialog).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => (window as unknown as { __updateCalls: number }).__updateCalls)).toBe(1);
});

/** `rgb(r, g, b)` / `rgba(...)` of a resolved `--cc-*` token, as getComputedStyle reports colours. */
async function tokenRgb(page: Page, token: string): Promise<string> {
  return page.evaluate((name) => {
    const probe = document.createElement('div');
    probe.style.color = `var(${name})`;
    document.body.appendChild(probe);
    const value = getComputedStyle(probe).color;
    probe.remove();
    return value;
  }, token);
}

for (const theme of Object.keys(themeTokens)) {
  test(`workspace buttons are the app's ordinary buttons, one primary per state, in ${theme} theme`, async ({ page }, testInfo) => {
    await openRemote(page, false);
    await page.evaluate((theme) => {
      const stores = (window as unknown as TestWindow).__E2E_TEST__;
      const appearance = stores.projectStore.getState().config!.appearance;
      // Mode `dark` shows the night colour theme: any of the 10 themes.
      stores.uiStore.getState().setThemePreview({ ...appearance, theme: 'dark', night_color_theme: theme } as typeof appearance);
    }, theme);
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    await openSpeleoTab(page);
    const controls = page.getByTestId('speleodb-workspace-everyday');
    await expect(controls).toBeVisible();
    await page.evaluate(() => Promise.all(document.getAnimations().map((animation) => animation.finished.catch(() => undefined))));
    const buttons = controls.getByRole('button');
    await expect(buttons).toHaveCount(5);
    await page.mouse.move(0, 0); // off the buttons: measure their rest colours, not a hover
    const styles = await buttons.evaluateAll((nodes) => nodes.map((node) => {
      const style = getComputedStyle(node);
      return { id: node.getAttribute('data-testid'), primary: node.hasAttribute('data-primary-action'),
        background: style.backgroundColor, color: style.color, padding: style.padding, radius: style.borderTopLeftRadius,
        weight: style.fontWeight, size: style.fontSize, shadow: style.boxShadow, height: node.getBoundingClientRect().height };
    }));
    // Editing with nothing to save: no primary, so every action is the standard
    // secondary - its own fill (the panel mixed toward the text colour, so it is
    // not the panel), primary text, the md padding and radius.
    expect(styles.filter((s) => s.primary).map((s) => s.id)).toEqual([]);
    const panel = await tokenRgb(page, '--cc-panel');
    const text = await tokenRgb(page, '--cc-text-primary');
    const fill = await page.evaluate(() => {
      const probe = document.createElement('div');
      probe.className = 'bg-button-secondary';
      document.body.appendChild(probe);
      const value = getComputedStyle(probe).backgroundColor;
      probe.remove();
      return value;
    });
    expect(fill).not.toBe(panel);
    for (const style of styles) {
      expect(style, style.id!).toMatchObject({ background: fill, color: text, padding: '6px 12px', radius: '4px', weight: '500', size: '14px', shadow: 'none' });
      expect(style.height).toBe(styles[0]!.height);
    }
    await expect(page.getByTestId('speleodb-workspace-open-web').locator('svg')).toBeVisible();
    await page.getByTestId('speleodb-workspace').screenshot({ path: testInfo.outputPath(`speleodb-buttons-${theme}.png`) });
    // Read-only, able to resume: Resume editing becomes the one primary, in the app's primary look.
    await page.getByTestId('speleodb-workspace-editing').click();
    const resume = page.getByTestId('speleodb-workspace-editing');
    await expect(resume).toHaveText('Resume editing');
    await expect(resume).toHaveAttribute('data-primary-action', '');
    await expect(controls.locator('[data-primary-action]')).toHaveCount(1);
    await page.mouse.move(0, 0);
    await expect.poll(() => resume.evaluate((node) => getComputedStyle(node).backgroundColor)).toBe(await tokenRgb(page, '--cc-accent'));
    await expect.poll(() => resume.evaluate((node) => getComputedStyle(node).color)).toBe(await tokenRgb(page, '--cc-accent-on'));
  });
}

/**
 * Record the menu close's `close_project` calls; `failFirst` makes the first one
 * fail - as a failed lock release (`lock_release: 'failed'`), or as any other
 * close failure.
 */
async function traceMenuClose(page: Page, failFirst: false | 'release' | 'other') {
  await page.evaluate((fail) => {
    const w = window as unknown as TestWindow & {
      __TAURI_INTERNALS__: { invoke: (command: string, args?: unknown) => Promise<unknown> };
      __menuCloseRequests: unknown[];
    };
    w.__menuCloseRequests = [];
    const invoke = w.__TAURI_INTERNALS__.invoke;
    w.__TAURI_INTERNALS__.invoke = async (command, args) => {
      if (command === 'close_project') {
        w.__menuCloseRequests.push(args);
        if (fail && w.__menuCloseRequests.length === 1) {
          return fail === 'release'
            ? { success: false, error: 'The SpeleoDB lock could not be handed back: synthetic 503', warnings: [], section_ids: [], lock_release: 'failed' }
            : { success: false, error: 'Synthetic close failure. The project is still open.', warnings: [], section_ids: [] };
        }
      }
      return invoke(command, args);
    };
  }, failFirst);
}

for (const dirty of [false, true]) {
  test(`hamburger Close Project requires lock release with unsaved changes ${dirty}`, async ({ page }) => {
    await openRemote(page, false);
    await traceMenuClose(page, false);
    await page.evaluate((isDirty) => (window as unknown as TestWindow).__E2E_TEST__.projectStore.setState({ isDirty }), dirty);
    await openAppMenu(page);
    await page.getByTestId('back-button').click();
    if (dirty) {
      await expect(page.getByTestId('speleodb-exit-dialog')).toBeVisible();
      await page.getByTestId('speleodb-exit-cancel').click();
      expect(await page.evaluate(() => (window as unknown as { __menuCloseRequests: unknown[] }).__menuCloseRequests)).toEqual([]);
      await openAppMenu(page);
      await page.getByTestId('back-button').click();
      await page.getByTestId('speleodb-exit-discard').click();
      await page.getByTestId('speleodb-exit-discard-confirm').click();
    }
    await expect(page.getByTestId('global-view')).toBeVisible();
    expect(await page.evaluate(() => (window as unknown as { __menuCloseRequests: unknown[] }).__menuCloseRequests))
      .toEqual([{ releaseLock: true, discardLocal: dirty }]);
  });
}

test('hamburger Close Project keeps the project open on lock release failure and supports retry', async ({ page }) => {
  await openRemote(page, false);
  await traceMenuClose(page, 'release');
  await openAppMenu(page);
  await page.getByTestId('back-button').click();
  // The release failure asks "Couldn't hand back the lock" instead of #668's error dialog.
  await expect(page.getByTestId('speleodb-release-failed-dialog')).toBeVisible();
  await expect(page.getByTestId('speleodb-release-failed-reason')).toContainText('synthetic 503');
  await expect(page.getByTestId('error-dialog')).toHaveCount(0);
  await expect(page.getByTestId('single-system-view')).toBeVisible();
  expect(await page.evaluate(() => (window as unknown as TestWindow).__E2E_TEST__.projectStore.getState().project?.file_path)).toBe('/projects/cave-alpha.stalx');
  await page.getByTestId('speleodb-release-failed-retry').click();
  await expect(page.getByTestId('global-view')).toBeVisible();
  expect(await page.evaluate(() => (window as unknown as { __menuCloseRequests: unknown[] }).__menuCloseRequests))
    .toEqual([{ releaseLock: true, discardLocal: false }, { releaseLock: true, discardLocal: false }]);
});

test('hamburger Close Project shows any other close failure in an error dialog', async ({ page }) => {
  await openRemote(page, false);
  await traceMenuClose(page, 'other');
  await openAppMenu(page);
  await page.getByTestId('back-button').click();
  await expect(page.getByTestId('error-dialog')).toContainText('Close Project failed');
  await expect(page.getByTestId('error-dialog')).toContainText('Synthetic close failure');
  await expect(page.getByTestId('speleodb-release-failed-dialog')).toHaveCount(0);
  await expect(page.getByTestId('single-system-view')).toBeVisible();
  await page.getByTestId('error-dialog').getByRole('button', { name: 'OK', exact: true }).click();
  await openAppMenu(page);
  await page.getByTestId('back-button').click();
  await expect(page.getByTestId('global-view')).toBeVisible();
});
