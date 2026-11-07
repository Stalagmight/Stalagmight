import { test, expect, type Page } from './fixtures/coverage';
import type { CDPSession } from '@playwright/test';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapReady,
  waitForNoDialogs,
  type SectionSeed,
} from './helpers/app';

/**
 * The shot grid's context menu, reached by a finger (#338).
 *
 * The claim this tier exists for is the one jsdom cannot make: a real browser
 * synthesises compatibility mouse events from the LIFT that ends a long press,
 * and the grid closes its menu on any window mousedown. So a menu that opens on
 * the hold and is gone a frame later looks, to every unit test, exactly like a
 * menu that opened and stayed. Only a real touch stack tells them apart.
 *
 * Playwright's API cannot express a bare touch hold, so the gestures go through
 * CDP `Input.dispatchTouchEvent` - the same path Chromium's own touch emulation
 * uses - and `test.use({ hasTouch: true })` is what makes the context accept
 * them.
 */

test.use({ hasTouch: true });

const sectionSeeds: SectionSeed[] = [
  {
    name: 'Touch Grid Section',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 45, length: 10, depth_in: 0, depth_out: -1 },
      { azimuth_in: 90, azimuth_out: 90, length: 20, depth_in: -1, depth_out: -2 },
      { azimuth_in: 135, azimuth_out: 135, length: 30, depth_in: -2, depth_out: -3 },
    ],
  },
];

interface TouchPoint {
  x: number;
  y: number;
  id: number;
}

/**
 * One CDP touch event. `touchEnd` / `touchCancel` take NO points - Chromium
 * treats an empty list as "every live point released", which is the shape
 * Puppeteer's own touchscreen uses.
 *
 * A local copy of the helper in map-touch-gestures.spec.ts: two specs sharing
 * six lines is cheaper than a helper module both must keep in step.
 */
async function touch(
  cdp: CDPSession,
  type: 'touchStart' | 'touchMove' | 'touchEnd' | 'touchCancel',
  points: TouchPoint[] = []
): Promise<void> {
  await cdp.send('Input.dispatchTouchEvent', {
    type,
    touchPoints: type === 'touchEnd' || type === 'touchCancel' ? [] : points,
  });
}

/** Where to put a finger: the middle of an element, in viewport coordinates. */
async function centerOf(page: Page, testId: string): Promise<TouchPoint> {
  const box = await page.getByTestId(testId).boundingBox();
  expect(box, `${testId} has a bounding box`).not.toBeNull();
  return { x: box!.x + box!.width / 2, y: box!.y + box!.height / 2, id: 1 };
}

/**
 * Watch the lift from the document, where a bubble listener sees the same event
 * object the grid's handler already ran on - `defaultPrevented` and all. That
 * flag IS the mechanism: it is what stops the browser synthesising the
 * compatibility mouse sequence.
 */
async function watchTouchEnd(page: Page): Promise<void> {
  await page.evaluate(() => {
    const w = window as unknown as { __touchEndDefaultPrevented__?: boolean | null };
    w.__touchEndDefaultPrevented__ = null;
    document.addEventListener(
      'touchend',
      (e) => {
        w.__touchEndDefaultPrevented__ = e.defaultPrevented;
      },
      false
    );
  });
}

async function touchEndPrevented(page: Page): Promise<boolean | null | undefined> {
  return page.evaluate(
    () =>
      (window as unknown as { __touchEndDefaultPrevented__?: boolean | null })
        .__touchEndDefaultPrevented__
  );
}

async function openDataTable(page: Page): Promise<void> {
  await page.keyboard.press('Control+3');
  await expect(page.getByTestId('data-table')).toBeVisible({ timeout: 3000 });
  // 3 shots + the end draft row (auto-selects first section)
  await expect(page.getByTestId('grid-row-3')).toHaveClass(/sg-row-draft/, { timeout: 5000 });
}

test.describe('Grid touch context menu', () => {
  let cdp: CDPSession;

  test.beforeEach(async ({ context, page }) => {
    await context.grantPermissions(['clipboard-read', 'clipboard-write']);
    cdp = await page.context().newCDPSession(page);
    await setupMockedPage(page);
    await createProject(page, 'Grid Touch Test');
    await seedProjectWithSections(page, sectionSeeds);
    await waitForMapReady(page);
    await waitForNoDialogs(page);
    await openDataTable(page);
  });

  test('a long press opens the menu, and the lift it ends on does not dismiss it (#338)', async ({
    page,
  }) => {
    await watchTouchEnd(page);
    const cell = await centerOf(page, 'grid-cell-1-length');

    await touch(cdp, 'touchStart', [cell]);

    // The hold is 500ms; waiting for the menu is both the hold and the
    // assertion that the press was recognised as one.
    const menu = page.getByTestId('grid-context-menu');
    await expect(menu).toBeVisible({ timeout: 5000 });
    // Right-click parity: the press took the selection with it.
    await expect(page.getByTestId('grid-cell-1-length')).toHaveAttribute(
      'data-active-cell',
      'true'
    );

    await touch(cdp, 'touchEnd');

    expect(
      await touchEndPrevented(page),
      'the lift ending a press that opened a menu is preventDefault-ed'
    ).toBe(true);

    // settle: the compat mousedown this suppresses would arrive within a frame
    // or two of the lift, and the grid closes its menu on any of them.
    await page.waitForTimeout(200);
    await expect(menu).toBeVisible();
    await expect(page.getByTestId('grid-cell-1-length')).toHaveAttribute(
      'data-active-cell',
      'true'
    );
  });

  test('a tap on a menu entry runs it and closes the menu', async ({ page }) => {
    await touch(cdp, 'touchStart', [await centerOf(page, 'grid-cell-1-length')]);
    const menu = page.getByTestId('grid-context-menu');
    await expect(menu).toBeVisible({ timeout: 5000 });
    await touch(cdp, 'touchEnd');

    // A tap, not a hold: this lift is nobody's long press, so the browser's
    // compatibility click is exactly what should reach the entry.
    const copy = await centerOf(page, 'grid-context-copy');
    await touch(cdp, 'touchStart', [copy]);
    await touch(cdp, 'touchEnd');

    await expect(menu).toHaveCount(0);
    // Copy ran: the marquee round the copied range is set synchronously, so it
    // is there whether or not the OS clipboard write succeeded.
    await expect(page.getByTestId('grid-copy-marquee')).toBeVisible();
  });

  test('a moving finger scrolls the grid instead of opening a menu', async ({ page }) => {
    // The boundary the grid deliberately keeps: touch drags belong to
    // scrolling, so a moving press neither fires nor range-selects.
    const start = await centerOf(page, 'grid-cell-1-length');

    await touch(cdp, 'touchStart', [start]);
    for (let step = 1; step <= 6; step += 1) {
      await touch(cdp, 'touchMove', [{ x: start.x, y: start.y - step * 12, id: 1 }]);
    }
    // Comfortably past the 500 ms hold (`LONG_PRESS_MS` in
    // src/utils/longPressTouch.ts - not imported, so this tier's typecheck
    // stays clear of src/), so a press that survived the movement would fire.
    // settle: holding past the timer is the input under test, not a wait for it.
    await page.waitForTimeout(700);
    await touch(cdp, 'touchEnd');
    // settle: proves no menu opens after the lift; its absence holds on the first
    // read either way.
    await page.waitForTimeout(200);

    await expect(page.getByTestId('grid-context-menu')).toHaveCount(0);
    expect(
      await page.locator('[data-in-selection="true"]').count(),
      'a touch drag does not extend the selection the way a mouse drag does'
    ).toBeLessThanOrEqual(1);
  });
});
