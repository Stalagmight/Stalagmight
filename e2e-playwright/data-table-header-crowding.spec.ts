/**
 * Regression test for #429 item 6: the data-table sub-header controls must not
 * overlap at small window sizes.
 *
 * The centered line pill used to be an absolutely positioned band spanning the
 * full header width, so it took no part in the header's flex layout. In a narrow
 * window it simply grew over the controls on its right: at 1024x640 the pill ran
 * to x=559 while the "Shots" button started at x=543, which rendered the label as
 * "hots" and left its leftmost 16px unclickable (the pill's own
 * `pointer-events-auto` wrapper ate the click).
 *
 * The header is now a three-column grid whose outer columns are equal `1fr` - so
 * the pill still centers across the FULL width - and whose middle column may
 * shrink to nothing, so the pill truncates instead of growing over its
 * neighbours. Both properties are pinned below: no overlap and a hittable button
 * edge at 1024x640, and true centering when there is room.
 */

import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithLines,
  waitForMapReady,
  waitForNoDialogs,
  type LineSeed,
} from './helpers/app';

// A line name of realistic length: the overlap is a function of how much room
// the centered pill asks for, and cave lines are not called "A".
const lineSeeds: LineSeed[] = [
  {
    name: 'Cenote Dos Ojos - Upstream Main Line',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 10, depth_in: -1, depth_out: -3 },
      { azimuth_in: 90, azimuth_out: 270, length: 12, depth_in: -3, depth_out: -5 },
    ],
  },
  {
    name: 'Cenote Dos Ojos - Downstream Bat Cave Line',
    direction: 'out',
    shots: [{ azimuth_in: 180, azimuth_out: 0, length: 7, depth_in: -2, depth_out: -4 }],
  },
];

async function openDataTable(page: Page): Promise<void> {
  await page.keyboard.press('Control+3');
  await expect(page.getByTestId('data-table')).toBeVisible({ timeout: 3000 });
  await expect(page.getByTestId('data-table-line-pill')).toBeVisible();
}

/** Horizontal overlap of two boxes in px (0 when they are clear of each other). */
function overlapX(a: { x: number; width: number }, b: { x: number; width: number }): number {
  return Math.max(0, Math.min(a.x + a.width, b.x + b.width) - Math.max(a.x, b.x));
}

/**
 * Hit-test points measured `dx` px in from an element's own left edge, at its
 * vertical middle: for each, the nearest `data-testid` above whatever is actually
 * topmost there.
 *
 * Measuring and probing inside ONE evaluate is deliberate. The Dock slides the
 * panel open, so a box read over the wire and hit-tested a round trip later can
 * point at the map the panel has since covered - a false failure that says
 * nothing about overlap.
 */
async function probeFromLeftEdge(
  page: Page,
  testId: string,
  offsets: number[]
): Promise<(string | null)[]> {
  return page.evaluate(
    ({ id, dxs }) => {
      const el = document.querySelector(`[data-testid="${id}"]`);
      if (!el) return dxs.map(() => null);
      const r = el.getBoundingClientRect();
      const y = r.y + r.height / 2;
      return dxs.map((dx) => {
        const hit = document.elementFromPoint(r.x + dx, y);
        return hit?.closest('[data-testid]')?.getAttribute('data-testid') ?? null;
      });
    },
    { id: testId, dxs: offsets }
  );
}

/** Wait out the Dock's open slide, so a measured box is still valid a moment
 *  later (two identical readings in a row). */
async function waitForHeaderSettled(page: Page): Promise<void> {
  const header = page.getByTestId('data-table-sub-header');
  let last = Number.NaN;
  await expect
    .poll(async () => {
      const y = (await header.boundingBox())?.y ?? Number.NaN;
      const settled = Number.isFinite(y) && y === last;
      last = y;
      return settled;
    })
    .toBe(true);
}

test.describe('#429 data-table sub-header at small window sizes', () => {
  test.use({ viewport: { width: 1024, height: 640 } });

  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Header Crowding');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapReady(page);
    await waitForNoDialogs(page);
    await openDataTable(page);
    await waitForHeaderSettled(page);
  });

  test('the line pill never overlaps the controls beside it', async ({ page }) => {
    const pill = (await page.getByTestId('data-table-line-pill').boundingBox())!;
    const viewSelect = (await page.getByTestId('data-table-view-select').boundingBox())!;
    const columnsMenu = (await page.getByTestId('data-table-columns-menu').boundingBox())!;
    const title = (await page.getByTestId('data-table-title').boundingBox())!;

    expect(overlapX(pill, viewSelect)).toBe(0);
    expect(overlapX(pill, columnsMenu)).toBe(0);
    expect(overlapX(pill, title)).toBe(0);
    // Squeezed, but not squeezed out of existence.
    expect(pill.width).toBeGreaterThan(24);
  });

  test('the Shots button is hittable all the way to its left edge', async ({ page }) => {
    // The reported symptom was the leftmost ~16px of the button being covered by
    // the pill, so probe exactly that strip.
    const offsets = [1, 3, 8, 16];
    const hits = await probeFromLeftEdge(page, 'data-table-view-shots', offsets);
    expect(Object.fromEntries(offsets.map((dx, i) => [dx, hits[i]]))).toEqual(
      Object.fromEntries(offsets.map((dx) => [dx, 'data-table-view-shots']))
    );
  });

  test('a click on the Shots button edge really switches the view', async ({ page }) => {
    // Functional counterpart to the hit test: start on Stations, then click the
    // left edge of Shots. A covered edge leaves the view on Stations.
    await page.getByTestId('data-table-view-stations').click();
    await expect(page.getByTestId('data-table-view-stations')).toHaveAttribute(
      'aria-checked',
      'true'
    );

    const b = (await page.getByTestId('data-table-view-shots').boundingBox())!;
    await page.mouse.click(b.x + 2, b.y + b.height / 2);

    await expect(page.getByTestId('data-table-view-shots')).toHaveAttribute('aria-checked', 'true');
  });

  test('the pill is still centered across the full header when there is room', async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await waitForHeaderSettled(page);
    const header = (await page.getByTestId('data-table-sub-header').boundingBox())!;
    const pill = (await page.getByTestId('data-table-line-pill').boundingBox())!;

    // Centering is what the absolute band bought and what the grid has to keep:
    // the pill sits on the header's midpoint, not on the midpoint of whatever
    // space the side controls left over.
    expect(Math.abs(pill.x + pill.width / 2 - (header.x + header.width / 2))).toBeLessThan(2);
  });
});
