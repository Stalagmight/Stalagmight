import { test, expect, Page } from './fixtures/coverage';
import { setupTauriMocks } from './mocks/tauri-ipc';
import { seedProjectWithLines, waitForMapReady, type LineSeed } from './helpers/app';

/**
 * E2E tests for the two-mode (hover/pin) panel reveal behavior and opener tabs
 * (Sections / Details / Data). See the panel-reveal plan.
 */

async function createProject(page: Page, name: string): Promise<void> {
  const newProjectBtn = page
    .locator('button:has-text("New Project"), h3:has-text("New Project")')
    .first();
  await newProjectBtn.click();
  await page.waitForSelector('.fixed.inset-0');
  await page.locator('.fixed.inset-0 input').first().fill(name);
  await page.click('.fixed.inset-0 button:has-text("Create")');
  await page.waitForSelector('canvas', { timeout: 10000 });
}

const lineSeeds: LineSeed[] = [
  {
    name: 'Panel Main',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 10.5, depth_in: -2, depth_out: -2.5, left: 1.5, right: 2.3, up: 3.0, down: 0.8 },
    ],
  },
];

interface PanelState {
  sectionsPanelMode: string;
  sectionsPanelOpen: boolean;
  detailPanelMode: string;
  detailPanelOpen: boolean;
  dataTableMode: string;
  dataTableOpen: boolean;
}

/** Read the panel slice of the UI store. */
async function panelState(page: Page): Promise<PanelState> {
  return page.evaluate(() => {
    const s = (window as unknown as {
      __E2E_TEST__: { uiStore: { getState: () => PanelState } };
    }).__E2E_TEST__.uiStore.getState();
    return {
      sectionsPanelMode: s.sectionsPanelMode,
      sectionsPanelOpen: s.sectionsPanelOpen,
      detailPanelMode: s.detailPanelMode,
      detailPanelOpen: s.detailPanelOpen,
      dataTableMode: s.dataTableMode,
      dataTableOpen: s.dataTableOpen,
    };
  });
}

/** Force a panel into a known mode/open state directly via the store. */
async function setMode(page: Page, panel: string, mode: 'pin' | 'hover'): Promise<void> {
  await page.evaluate(
    ({ panel, mode }) => {
      (window as unknown as {
        __E2E_TEST__: { uiStore: { getState: () => { setPanelMode: (p: string, m: string) => void } } };
      }).__E2E_TEST__.uiStore.getState().setPanelMode(panel, mode);
    },
    { panel, mode },
  );
}

test.describe('Panel reveal (hover/pin)', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'Panel Reveal Test');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapReady(page);
  });

  // "Fixture" not "defaults": the mock config pins this layout deliberately. A
  // fresh install opens the data table too (uiStore.initPanels.test.ts).
  test('fixture layout: side panels pinned open, data closed @smoke', async ({ page }) => {
    const s = await panelState(page);
    expect(s.sectionsPanelMode).toBe('pin');
    expect(s.sectionsPanelOpen).toBe(true);
    expect(s.detailPanelMode).toBe('pin');
    expect(s.detailPanelOpen).toBe(true);
    expect(s.dataTableOpen).toBe(false);
    // Pinned side panels are present; the data opener bar is shown (closed).
    await expect(page.locator('[data-testid="sections-panel"]')).toBeVisible();
    await expect(page.locator('[data-testid="data-panel-bar"]')).toBeVisible();
  });

  test('pin mode: close-arrow closes, bar reopens', async ({ page }) => {
    // Sections starts pinned-open: the tab shows pin (un-pin) + close-arrow.
    await page.locator('[data-testid="panel-close-sections"]').click();
    await expect.poll(async () => (await panelState(page)).sectionsPanelOpen).toBe(false);
    await expect(page.locator('[data-testid="sections-panel-bar"]')).toBeVisible();
    // Pin-mode bar requires a click to reopen (no hover-open).
    await page.locator('[data-testid="sections-panel-bar"]').click();
    await expect.poll(async () => (await panelState(page)).sectionsPanelOpen).toBe(true);
  });

  test('un-pin switches a pinned panel to hover mode', async ({ page }) => {
    await page.locator('[data-testid="panel-pin-sections"]').click();
    const s = await panelState(page);
    expect(s.sectionsPanelMode).toBe('hover');
    expect(s.sectionsPanelOpen).toBe(false);
  });

  test('hover mode: hovering the bar reveals the floating panel; leaving hides it', async ({ page }) => {
    await setMode(page, 'sections', 'hover');
    await expect(page.locator('[data-testid="sections-panel-bar"]')).toBeVisible();
    // Hovering the edge bar floats the panel in.
    await page.locator('[data-testid="sections-panel-bar"]').hover();
    await expect(page.locator('[data-testid="sections-panel"]')).toBeVisible({ timeout: 2000 });
    // Moving onto the map dismisses it (after the close grace).
    await page.locator('canvas').first().hover({ position: { x: 300, y: 300 } });
    await expect(page.locator('[data-testid="sections-panel"]')).toBeHidden({ timeout: 2000 });
  });

  test('keyboard shortcut pins/closes the detail panel', async ({ page }) => {
    // Detail starts pinned-open; the toggle shortcut (Ctrl+2) closes it.
    await page.keyboard.press('Control+2');
    await expect.poll(async () => (await panelState(page)).detailPanelOpen).toBe(false);
    expect((await panelState(page)).detailPanelMode).toBe('pin');
    // Pressing again pins it back open.
    await page.keyboard.press('Control+2');
    await expect.poll(async () => (await panelState(page)).detailPanelOpen).toBe(true);
  });
});
