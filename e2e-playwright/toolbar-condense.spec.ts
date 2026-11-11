import { test, expect, type Page } from './fixtures/coverage';
import { setupMockedPage, createProject, waitForNoDialogs } from './helpers/app';

/**
 * E2E for the SingleSystemView Toolbar progressive condensation. As the left
 * (file/history) and right (tools/display) clusters run out of room to clear the
 * absolutely-centered mode carousel, labels drop in a fixed priority order:
 *   1. "Display" text  -> eye + caret only
 *   2. "Import" text   -> yellow import icon only
 *   3. Undo/Redo text  -> undo/redo icons only
 *   4. tighten padding around the icon buttons
 *   5. the carousel leaves absolute positioning and joins the flex flow (#260)
 *
 * The carousel is position:absolute (no CSS overflow), so this is measured by rects,
 * which jsdom can't do - hence an e2e. We assert the width-independent PRIORITY
 * INVARIANT (a later stage never precedes an earlier) across a width sweep, plus the
 * endpoints, plus the #260 NO-OVERLAP invariant: the ladder used to bottom out on a
 * "least, symmetric overlap" fallback that rendered "Export" underneath the
 * select-tool button.
 */

interface CondenseState {
  displayHidden: boolean;
  importHidden: boolean;
  historyHidden: boolean;
}

/** Painted overlap with each side cluster, including the narrow two-row layout. */
interface OverlapState {
  left: boolean;
  right: boolean;
}

async function readOverlap(page: Page): Promise<OverlapState | null> {
  return page.evaluate(() => {
    const box = (sel: string) => document.querySelector(sel)?.getBoundingClientRect() ?? null;
    const carousel = box('[data-testid="mode-carousel"]');
    const left = box('[data-testid="toolbar-file-controls"]');
    const right = box('[data-testid="toolbar-tools-group"]');
    if (!carousel || !left || !right) return null;
    const hits = (a: DOMRect, b: DOMRect) =>
      a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom;
    return { left: hits(carousel, left), right: hits(carousel, right) };
  });
}

async function readState(page: Page): Promise<CondenseState> {
  return page.evaluate(() => {
    const bar = document.querySelector('[data-testid="toolbar"]');
    const text = (sel: string) => bar?.querySelector(sel)?.textContent ?? '';
    return {
      displayHidden: !text('[data-testid="display-menu-button"]').includes('Display'),
      importHidden: !text('[data-testid="toolbar-import-button"]').includes('Import'),
      historyHidden: !text('[data-testid="toolbar-undo-button"]').includes('Undo'),
    };
  });
}

test.describe('SingleSystemView Toolbar - progressive condensation', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Toolbar Condense Test');
    await waitForNoDialogs(page);
    await expect(page.getByTestId('toolbar')).toBeVisible();
    await expect(page.getByTestId('display-menu-button')).toBeVisible();
  });

  test('drops labels in priority order and restores them when space returns', async ({ page }) => {
    // Wide: nothing condensed - all three labels present.
    await page.setViewportSize({ width: 1600, height: 800 });
    await expect
      .poll(async () => JSON.stringify(await readState(page)))
      .toBe(JSON.stringify({ displayHidden: false, importHidden: false, historyHidden: false }));

    // Sweep wide -> narrow; at EVERY width the priority invariant must hold:
    // historyHidden ⇒ importHidden ⇒ displayHidden.
    for (const width of [1600, 1100, 860, 720, 620, 540, 460]) {
      await page.setViewportSize({ width, height: 800 });
      const s = await readState(page);
      if (s.historyHidden) expect(s.importHidden, `w=${width}`).toBe(true);
      if (s.importHidden) expect(s.displayHidden, `w=${width}`).toBe(true);
    }

    // Narrow: the "Display" text is dropped first (stage 1 always fires under pressure).
    await page.setViewportSize({ width: 460, height: 800 });
    await expect.poll(async () => (await readState(page)).displayHidden).toBe(true);
    // The condensed Display button keeps an accessible name (eye + caret only).
    await expect(page.getByTestId('display-menu-button')).toHaveAttribute('aria-label', /.+/);

    // Restore width: everything expands back (reset-then-step-up).
    await page.setViewportSize({ width: 1600, height: 800 });
    await expect
      .poll(async () => JSON.stringify(await readState(page)))
      .toBe(JSON.stringify({ displayHidden: false, importHidden: false, historyHidden: false }));
  });

  // #260: the mode carousel used to be positioned for the "least, symmetric
  // overlap" once the condense ladder ran out, which rendered "Export" underneath
  // the select-tool button. The final stage now takes the carousel out of absolute
  // positioning, so flexbox lays it out BESIDE the clusters and overlap is not
  // expressible in the layout at any width.
  test('never overlaps the side clusters, at any width', async ({ page }) => {
    for (const width of [1600, 1100, 860, 720, 620, 540, 460, 400, 360]) {
      await page.setViewportSize({ width, height: 800 });
      // The reset-then-step-up fit pass runs across several frames after a
      // resize, so a single sample can read a mid-relayout frame where the
      // carousel is still sized for the previous width. Poll until the layout
      // SETTLES: a transient overlap during the pass is not the #260 regression
      // (which is a stable, painted overlap), and sampling once made this test
      // fail at a different width on every run.
      await expect
        .poll(async () => JSON.stringify(await readOverlap(page)), {
          message: `carousel overlaps a side cluster at w=${width}`,
        })
        .toBe(JSON.stringify({ left: false, right: false }));
    }
  });
});
