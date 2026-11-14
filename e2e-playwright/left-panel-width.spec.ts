/**
 * E2E: the left panel's width with its two tabs (Sections, SpeleoDB).
 *
 * - Both tabs share one width: switching tabs never resizes the panel, at a normal
 *   width and at the minimum (#576 UI polish, item 6).
 * - At the minimum (`LEFT_WORKSPACE_MIN_WIDTH`) both tab headers, icons included,
 *   fit on one row, and the SpeleoDB tab's content stays inside the panel (item 5).
 *
 * Parallel-safe: all state is per-page (mocked IPC + `window.__E2E_TEST__` stores).
 */
import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage } from './helpers/app';
import { LEFT_WORKSPACE_MIN_WIDTH } from '../src/components/SingleSystemView/layoutConstants';

type TestWindow = Window & {
  __E2E_TEST__: Record<string, { getState: () => Record<string, unknown>; setState: (s: unknown) => void }>;
  __MOCK_IPC_OVERRIDES__: Record<string, unknown>;
  __MOCK_SET_PROJECT_IS_REMOTE__: (remote: boolean) => void;
};

/** A remote project, held for editing with work not yet uploaded, and some history. */
async function openRemoteProject(page: Page): Promise<void> {
  await page.setViewportSize({ width: 1400, height: 900 });
  await setupMockedPage(page);
  await page.evaluate(async () => {
    const w = window as unknown as TestWindow;
    w.__MOCK_SET_PROJECT_IS_REMOTE__(true);
    w.__MOCK_IPC_OVERRIDES__ = {
      speleodb_session: { logged_in: true, user_email: 'me@example.com', endpoint: 'https://example.com' },
      speleodb_get_revisions: [0, 1].map((i) => ({ id: `${i}aaaaaaabbbbbbbb`, parent_ids: [], author_email: 'surveyor.with.a.long.address@example.org',
        authored_date: '2026-10-01T10:00:00Z', message: 'Synthetic survey update with a long description' })),
      speleodb_project_access_at: { release_context: { project_id: 'remote', endpoint: 'https://example.com' } },
      speleodb_start_editing: { kind: 'editing' },
      speleodb_link_status: { is_remote: true, edit_enabled: true, permission: 'READ_AND_WRITE', lock_holder: 'me@example.com',
        has_unpushed_edits: true, unwound: false, remote_state: 'up_to_date', access_denied: false, server_error: null,
        undo_mode: 'full', project_id: 'remote' },
    };
    await (w.__E2E_TEST__.speleoStore.getState().refreshSession as () => Promise<void>)();
    await (w.__E2E_TEST__.projectStore.getState().loadProject as (p: string) => Promise<unknown>)('/projects/cave-alpha.stalx');
    w.__E2E_TEST__.uiStore.setState({ sectionsPanelMode: 'pin', sectionsPanelOpen: true, speleoWorkspaceSection: 'history' });
  });
  await expect(page.getByTestId('workspace-tab-speleodb')).toHaveAttribute('data-state', 'unuploaded');
}

const panelWidth = async (page: Page) => (await page.getByTestId('sections-panel-bar').boundingBox())!.width;

/**
 * The panel width once it holds still: polled until two samples in a row agree,
 * so a resize (if any) has landed before the width is read, with no fixed sleep.
 */
async function settledPanelWidth(page: Page): Promise<number> {
  let last = Number.NaN;
  await expect.poll(async () => {
    const width = await panelWidth(page);
    const settled = width === last;
    last = width;
    return settled;
  }, { intervals: [100] }).toBe(true);
  return last;
}

/** Switch Sections -> SpeleoDB -> Sections and return the panel width at each step. */
async function widthsAcrossSwitches(page: Page): Promise<number[]> {
  const widths: number[] = [];
  for (const tab of ['sections', 'speleodb', 'sections', 'speleodb'] as const) {
    await page.getByTestId(`workspace-tab-${tab}`).click();
    await expect(page.getByTestId(`workspace-tab-${tab}`)).toHaveAttribute('aria-selected', 'true');
    await expect(page.locator(`#workspace-panel-${tab}`)).toBeVisible();
    widths.push(await settledPanelWidth(page));
  }
  return widths;
}

test.describe('left panel width', () => {
  test.beforeEach(async ({ page }) => {
    await openRemoteProject(page);
  });

  test('switching tabs keeps the width, at the default width', async ({ page }) => {
    const widths = await widthsAcrossSwitches(page);
    expect(widths[0]).toBeGreaterThan(LEFT_WORKSPACE_MIN_WIDTH);
    expect(new Set(widths).size, `widths ${widths.join(', ')}`).toBe(1);
  });

  test('switching tabs keeps the width, starting from the minimum reached by dragging', async ({ page }) => {
    await page.getByTestId('workspace-tab-sections').click();
    const handle = page.getByTestId('sections-resize-handle');
    const box = (await handle.boundingBox())!;
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(box.x - 400, box.y + box.height / 2, { steps: 8 });
    await page.mouse.up();
    await expect.poll(() => panelWidth(page)).toBeCloseTo(LEFT_WORKSPACE_MIN_WIDTH, 0);
    const widths = await widthsAcrossSwitches(page);
    expect(widths.every((w) => Math.abs(w - LEFT_WORKSPACE_MIN_WIDTH) < 0.5), `widths ${widths.join(', ')}`).toBe(true);
  });

  test('a stored width below the floor shows at the floor on both tabs', async ({ page }) => {
    await page.evaluate(() => (window as unknown as TestWindow).__E2E_TEST__.uiStore.setState({ sectionsPanelWidth: 150 }));
    const widths = await widthsAcrossSwitches(page);
    expect(widths.every((w) => Math.abs(w - LEFT_WORKSPACE_MIN_WIDTH) < 0.5), `widths ${widths.join(', ')}`).toBe(true);
  });

  test('at the minimum, both tab headers fit on one row and the SpeleoDB tab content stays inside', async ({ page }) => {
    await page.evaluate(() => (window as unknown as TestWindow).__E2E_TEST__.uiStore.setState({ sectionsPanelWidth: 150 }));
    await page.getByTestId('workspace-tab-speleodb').click();
    await expect(page.getByTestId('speleodb-revisions-list')).toBeVisible();
    expect(await panelWidth(page)).toBeCloseTo(LEFT_WORKSPACE_MIN_WIDTH, 0);
    const report = await page.evaluate(() => {
      const problems: string[] = [];
      const tabs = [...document.querySelectorAll<HTMLElement>('[role="tab"][id^="workspace-tab-"]')];
      const tops = new Set(tabs.map((t) => Math.round(t.getBoundingClientRect().top)));
      if (tabs.length !== 2 || tops.size !== 1) problems.push(`tabs not on one row: ${tabs.length} tabs, tops ${[...tops]}`);
      for (const tab of tabs) {
        // Icon + label + gap + padding must fit the tab's share of the width.
        const cs = getComputedStyle(tab);
        const kids = [...tab.children].map((c) => c.getBoundingClientRect().width);
        const need = kids.reduce((a, x) => a + x, 0) + parseFloat(cs.columnGap || '0') * (kids.length - 1)
          + parseFloat(cs.paddingLeft) + parseFloat(cs.paddingRight);
        const has = tab.getBoundingClientRect().width;
        if (need > has + 0.5) problems.push(`${tab.id} needs ${need.toFixed(1)}px, has ${has.toFixed(1)}px`);
        if (!tab.querySelector('svg, [style*="mask-image"]')) problems.push(`${tab.id} has no icon`);
      }
      const body = document.getElementById('workspace-panel-speleodb')!;
      const box = body.getBoundingClientRect();
      for (const el of body.querySelectorAll<HTMLElement>('*')) {
        const r = el.getBoundingClientRect();
        if (r.width === 0 || r.height === 0) continue;
        if (r.left < box.left - 0.5 || r.right > box.right + 0.5) problems.push(`overflows: ${el.tagName} ${el.dataset.testid ?? ''}`);
      }
      for (const button of body.querySelectorAll<HTMLElement>('button')) {
        if (button.scrollWidth > button.clientWidth + 0.5) problems.push(`clipped button: ${button.dataset.testid ?? button.textContent}`);
      }
      return problems;
    });
    expect(report).toEqual([]);
  });

  test('the un-uploaded SpeleoDB tab differs only by its text colour: dimmed idle, full selected', async ({ page }) => {
    const tabStyle = (id: 'sections' | 'speleodb') => page.getByTestId(`workspace-tab-${id}`).evaluate((tab) => {
      const cs = getComputedStyle(tab);
      return { background: cs.backgroundColor, shadow: cs.boxShadow, weight: cs.fontWeight, color: cs.color, borderBottom: cs.borderBottomStyle };
    });
    /** A colour expression resolved the way the browser paints it, under the tab list. */
    const resolve = (expr: string) => page.getByTestId('workspace-tab-sections').evaluate((tab, value) => {
      const probe = document.createElement('span');
      probe.style.color = value;
      tab.parentElement!.appendChild(probe);
      const color = getComputedStyle(probe).color;
      probe.remove();
      return color;
    }, expr);
    await page.getByTestId('workspace-tab-sections').click();
    await page.mouse.move(0, 0);
    const sectionsSelected = await tabStyle('sections');
    const idle = await tabStyle('speleodb');
    // Idle: no wash, no rule, the idle weight - only the dimmed warning text.
    expect(idle.background).toBe('rgba(0, 0, 0, 0)');
    expect(idle.shadow).toBe('none');
    expect(idle.borderBottom).toBe('none');
    expect(idle.weight).toBe('400');
    expect(idle.color).toBe(await resolve('color-mix(in srgb, var(--cc-warning-text) 75%, var(--cc-panel))'));
    expect(idle.color).not.toBe(await resolve('var(--cc-warning-text)'));

    await page.getByTestId('workspace-tab-speleodb').click();
    await expect(page.getByTestId('workspace-tab-speleodb')).toHaveAttribute('aria-selected', 'true');
    const selected = await tabStyle('speleodb');
    // Selected: the very indicator Sections uses, with the full-strength warning text.
    expect(selected.background).toBe(sectionsSelected.background);
    expect(selected.weight).toBe(sectionsSelected.weight);
    expect(selected.shadow).toBe('none');
    expect(selected.color).toBe(await resolve('var(--cc-warning-text)'));
    // Sections, now idle, looks like any idle tab.
    expect((await tabStyle('sections')).background).toBe('rgba(0, 0, 0, 0)');
  });
});

