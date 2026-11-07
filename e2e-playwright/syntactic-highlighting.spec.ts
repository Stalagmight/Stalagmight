import { test, expect, Page } from './fixtures/coverage';
import { setupTauriMocks } from './mocks/tauri-ipc';
import { seedProjectWithSections, waitForMapReady, createProject, type SectionSeed } from './helpers/app';

/**
 * E2E tests for Syntactic Highlighting: the colored duotone toolbar, the
 * removal of the draw/delete tools, the Full-mode entity blobs, and the
 * settings panel (level / preset / per-color overrides) with live preview.
 */

const seeds: SectionSeed[] = [
  {
    name: 'SH Main',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 10.5, depth_in: -2, depth_out: -2.5, left: 1.5, right: 2.3, up: 3.0, down: 0.8 },
      { azimuth_in: 90, azimuth_out: 270, length: 15.2, depth_in: -2.5, depth_out: -5.0, left: 0.5, right: 0.7, up: 1.2, down: 1.5 },
    ],
  },
];

function shLevel(page: Page): Promise<string | undefined> {
  return page.evaluate(() => document.documentElement.dataset.shLevel);
}

function shVar(page: Page, name: string): Promise<string> {
  return page.evaluate(
    (n) => getComputedStyle(document.documentElement).getPropertyValue(n).trim(),
    name,
  );
}

function activeTool(page: Page): Promise<string> {
  return page.evaluate(() => {
    const e2e = (window as unknown as { __E2E_TEST__: { uiStore: { getState: () => { activeTool: string } } } }).__E2E_TEST__;
    return e2e.uiStore.getState().activeTool;
  });
}

async function openSyntacticSettings(page: Page): Promise<void> {
  // The highlighting controls live in the Appearance settings tab.
  await page.evaluate(() => {
    const e2e = (window as unknown as { __E2E_TEST__: { uiStore: { getState: () => { openDialog: (d: string) => void } } } }).__E2E_TEST__;
    e2e.uiStore.getState().openDialog('settings');
  });
  await expect(page.getByTestId('settings-dialog')).toBeVisible({ timeout: 5000 });
  await page.getByTestId('settings-tab-appearance').click();
  await expect(page.getByTestId('settings-panel-appearance')).toBeVisible();
  await expect(page.getByTestId('settings-syntactic')).toBeVisible();
}

async function pickOption(page: Page, selectTestId: string, optionName: string): Promise<void> {
  const trigger = page.getByTestId(selectTestId);
  const option = page.getByRole('option', { name: optionName });
  // The Select opens in two phases (renders visibility:hidden, then a layout
  // effect repositions it), and a settings re-render can close the dropdown
  // between locating and clicking the option. Retry the open+click as a unit,
  // only (re)opening when it's actually closed so we never toggle it shut.
  // Selecting the same option twice is idempotent, so converging is safe.
  await expect(async () => {
    if ((await trigger.getAttribute('data-dropdown-open')) === null) {
      await trigger.click();
    }
    await expect(option).toBeVisible({ timeout: 1500 });
    await option.click({ timeout: 1500 });
  }).toPass({ timeout: 12000 });
}

test.describe('Syntactic Highlighting', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await createProject(page, 'SH Test');
    await seedProjectWithSections(page, seeds);
    await waitForMapReady(page);
  });

  test('toolbar omits draw/delete and renders duotone icons; D/X are unbound @smoke', async ({ page }) => {
    await expect(page.getByTestId('tool-draw')).toHaveCount(0);
    await expect(page.getByTestId('tool-delete')).toHaveCount(0);
    await expect(page.getByTestId('tool-select')).toBeVisible();
    await expect(page.getByTestId('tool-merge')).toBeVisible();

    // Duotone glyphs are present in the toolbar.
    await expect(page.locator('[data-testid="toolbar-tools-group"] svg.duotone').first()).toBeVisible();

    // Tool buttons have a per-tool tinted background ("back-color"), not the
    // default transparent ghost background.
    const selectButton = page.getByTestId('tool-select');
    const selectBg = await selectButton.evaluate((el) => getComputedStyle(el).backgroundColor);
    expect(selectBg).not.toBe('rgba(0, 0, 0, 0)');
    expect(selectBg).not.toBe('transparent');

    // The active tool (select is active by default) is ringed with a tool-colored
    // border (not the theme accent), and the border is the same width on all four
    // sides so it stays pixel-even at any UI zoom.
    await expect(selectButton).toHaveClass(/sh-tool--active/);
    const border = await selectButton.evaluate((el) => {
      const s = getComputedStyle(el);
      return {
        color: s.borderTopColor,
        widths: [s.borderTopWidth, s.borderRightWidth, s.borderBottomWidth, s.borderLeftWidth],
      };
    });
    expect(border.color).not.toBe('rgba(0, 0, 0, 0)'); // ring present
    expect(new Set(border.widths).size).toBe(1); // all four sides equal width

    // Pressing the old draw/delete shortcuts does nothing.
    await page.getByTestId('tool-select').click();
    await page.keyboard.press('d');
    await page.keyboard.press('x');
    const tool = await activeTool(page);
    expect(['draw', 'delete']).not.toContain(tool);
  });

  test('default highlighting level is Full', async ({ page }) => {
    await expect.poll(() => shLevel(page)).toBe('full');
  });

  test('settings: changing Level live-previews data-sh-level', async ({ page }) => {
    await openSyntacticSettings(page);

    await pickOption(page, 'select-syntactic-level', 'Off');
    await expect.poll(() => shLevel(page)).toBe('off');

    await pickOption(page, 'select-syntactic-level', 'Toolbar only');
    await expect.poll(() => shLevel(page)).toBe('toolbar_only');

    await pickOption(page, 'select-syntactic-level', 'Full');
    await expect.poll(() => shLevel(page)).toBe('full');
  });

  test('settings: changing preset repaints role colors', async ({ page }) => {
    const before = await shVar(page, '--sh-station'); // default green
    await openSyntacticSettings(page);
    await pickOption(page, 'select-syntactic-preset', 'Deuteranopia');
    await expect.poll(() => shVar(page, '--sh-station')).not.toBe(before);
  });

  test('settings: per-color override applies and resets', async ({ page }) => {
    await openSyntacticSettings(page);

    const input = page.getByTestId('input-syntactic-color-shot');
    await input.evaluate((el) => {
      const i = el as HTMLInputElement;
      // Use React's native value setter so its onChange fires (a plain
      // assignment is swallowed by React's input value tracker).
      const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set;
      setter?.call(i, '#123456');
      i.dispatchEvent(new Event('input', { bubbles: true }));
    });
    await expect.poll(() => shVar(page, '--sh-shot')).toBe('#123456');

    // Per-row reset returns to the preset base color.
    await page.getByTestId('setting-syntactic-color-shot-reset').click();
    await expect.poll(() => shVar(page, '--sh-shot')).not.toBe('#123456');
  });

  test('Full mode shows entity blobs in the data table; Off hides them', async ({ page }) => {
    // Select the first shot (reading the real ids from the store) so the detail
    // panel shows its start/end station links, and open the data table.
    await page.evaluate(() => {
      const e2e = (window as unknown as {
        __E2E_TEST__: {
          projectStore: { getState: () => { project: { sections: { id: number; shots: { id: number }[] }[] } } };
          uiStore: { getState: () => { setSelection: (s: unknown) => void } };
        };
      }).__E2E_TEST__;
      const section = e2e.projectStore.getState().project.sections[0]!; // the seeded section
      e2e.uiStore.getState().setSelection({ type: 'shot', sectionId: section.id, shotId: section.shots[0]!.id });
    });
    await page.keyboard.press('Control+3');
    await expect(page.getByTestId('data-table')).toBeVisible();

    // Full (default): entity blobs render in the table, and detail-panel links
    // render the entity symbol (a duotone svg, not a plain dot).
    await expect(page.locator('[data-entity]').first()).toBeVisible({ timeout: 5000 });
    await expect(page.locator('[data-testid$="-link-symbol"]').first()).toBeVisible({ timeout: 5000 });
    // The link renders as an entity pill: inline-flex (symbol next to text),
    // rounded, with a tinted background.
    const linkPill = page.locator('[data-testid$="-link"]').first();
    const pill = await linkPill.evaluate((el) => {
      const s = getComputedStyle(el);
      return {
        display: s.display,
        radius: parseFloat(s.borderTopLeftRadius),
        bg: s.backgroundColor,
        borderWidth: parseFloat(s.borderTopWidth),
        borderStyle: s.borderTopStyle,
        borderColor: s.borderTopColor,
      };
    });
    // inline-flex blockifies to flex when the link sits in a flex parent.
    expect(pill.display).toMatch(/flex/);
    expect(pill.radius).toBeGreaterThan(8); // rounded-full
    expect(pill.bg).not.toBe('rgba(0, 0, 0, 0)'); // tinted background
    // The dim entity-tinted border actually renders (solid, non-zero, visible).
    expect(pill.borderStyle).toBe('solid');
    expect(pill.borderWidth).toBeGreaterThan(0);
    expect(pill.borderColor).not.toBe('rgba(0, 0, 0, 0)');
    // The fully-qualified id (e.g. "1:1"), monospace, after a short ::before divider.
    const linkId = page.locator('[data-testid$="-link-id"]').first();
    await expect(linkId).toBeVisible();
    expect((await linkId.textContent())?.trim()).toMatch(/^\d+:\d+$/);
    const idStyle = await linkId.evaluate((el) => ({
      font: getComputedStyle(el).fontFamily,
      dividerW: getComputedStyle(el, '::before').width,
    }));
    expect(idStyle.font.toLowerCase()).toMatch(/mono/);
    expect(parseFloat(idStyle.dividerW)).toBeGreaterThan(0);

    // Switch to Off via the store preview: blobs and symbols disappear.
    await page.evaluate(() => {
      const e2e = (window as unknown as {
        __E2E_TEST__: { uiStore: { getState: () => { setSyntacticHighlightPreview: (c: unknown) => void } } };
      }).__E2E_TEST__;
      e2e.uiStore.getState().setSyntacticHighlightPreview({ level: 'off', preset: 'default', overrides: {} });
    });
    await expect(page.locator('[data-entity]')).toHaveCount(0);
    await expect(page.locator('[data-testid$="-link-symbol"]')).toHaveCount(0);
    await expect(page.locator('[data-testid$="-link-id"]')).toHaveCount(0);
  });
});
