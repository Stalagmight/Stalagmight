import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithLines,
  waitForMapReady,
  waitForNoDialogs,
  type LineSeed,
} from './helpers/app';

/**
 * Insert Station dialog: the paired metres/percent inputs and the
 * right-click-position prefill.
 *
 * The dialog's own logic is unit-tested in InsertStationDialog.test.tsx; what
 * needs a real map here is the WIRING - that a genuine right-click on a shot
 * carries the pointer's position along that shot all the way into the dialog.
 * A synthetic `alongT` cannot prove that.
 *
 * Deliberately position-independent: the test finds the shot by scanning rather
 * than hardcoding pixels, so a change to fit-to-bounds framing does not turn
 * this into a false failure. It asserts the fraction is real and ordered, not a
 * specific pixel-derived number.
 */

const lineSeeds: LineSeed[] = [
  {
    name: 'Long Shot',
    direction: 'in',
    // One long, purely horizontal, due-east shot: it renders as a wide straight
    // line, so a click at a given fraction across it is a click at that
    // fraction along it.
    shots: [{ azimuth_in: 90, azimuth_out: 90, length: 100, depth_in: 0, depth_out: 0 }],
  },
];

/** `alongT` recorded on the open context menu, or null if it is not a shot menu. */
async function menuAlongT(page: Page): Promise<number | null> {
  return page.evaluate(() => {
    const ui = (
      window as unknown as {
        __E2E_TEST__: { uiStore: { getState: () => { dialogData: Record<string, unknown> } } };
      }
    ).__E2E_TEST__.uiStore.getState();
    const target = ui.dialogData.target as { type?: string; alongT?: number } | undefined;
    return target?.type === 'shot' ? (target.alongT ?? null) : null;
  });
}

/** Short-timeout poll for the shot menu's alongT. Returns null if it never
 *  opens, which is the expected outcome for a click that misses the shot. */
async function pollAlongT(page: Page): Promise<number | null> {
  const deadline = Date.now() + 2000;
  for (;;) {
    const t = await menuAlongT(page);
    if (t !== null) return t;
    if (Date.now() >= deadline) return null;
    await page.waitForFunction(() => true, undefined, { polling: 50, timeout: 1000 });
  }
}

/**
 * Right-click along the map's horizontal midline until a shot context menu
 * opens, then return the click's x and the recorded fraction. `null` when the
 * scan never lands on the shot.
 */
async function rightClickShot(
  page: Page,
  xFractions: number[],
): Promise<Array<{ x: number; alongT: number }>> {
  const box = (await page.locator('canvas').first().boundingBox())!;
  const y = box.y + box.height / 2;
  const found: Array<{ x: number; alongT: number }> = [];
  for (const f of xFractions) {
    const x = box.x + box.width * f;
    await page.mouse.click(x, y, { button: 'right' });
    // Poll for the menu instead of sleeping: a right-click that MISSES the shot
    // legitimately opens no shot menu, so this has to tolerate "never arrives"
    // rather than wait for it. expect.poll's timeout is the miss case; a hit
    // resolves on the first tick.
    const t = await pollAlongT(page);
    if (t !== null) found.push({ x, alongT: t });
    await page.keyboard.press('Escape');
    await waitForNoDialogs(page);
  }
  return found;
}

test.describe('Insert Station dialog', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Insert Station');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapReady(page);
    await waitForNoDialogs(page);
    // Zoom to the survey so the 100 m shot is more than a few pixels wide.
    const zoomBefore = await page.evaluate(() => {
      const w = window as unknown as {
        __stalagmight_map__: { getZoom: () => number };
        __E2E_TEST__: { uiStore: { getState: () => { triggerFitToBounds: () => void } } };
      };
      const z = w.__stalagmight_map__.getZoom();
      w.__E2E_TEST__.uiStore.getState().triggerFitToBounds();
      return z;
    });
    // Wait for the camera to actually MOVE and then settle, rather than sleeping
    // through the animation (the classic flake source, docs/TESTING.md). A bare
    // waitForMapIdle is not enough: the fit is dispatched through a React effect,
    // so the map is still idle for a tick after the trigger and the wait would
    // return before anything moved.
    await page.waitForFunction(
      (z0) => {
        const map = (
          window as unknown as {
            __stalagmight_map__?: { isMoving?: () => boolean; getZoom: () => number };
          }
        ).__stalagmight_map__;
        return Boolean(map?.isMoving && !map.isMoving() && map.getZoom() !== z0);
      },
      zoomBefore,
    );
  });

  /** The x fractions the shot can plausibly fall under, widest first use. */
  const SCAN = [0.35, 0.4, 0.45, 0.5, 0.55, 0.6, 0.65];

  test('a right-click carries its position along the shot into the dialog', async ({ page }) => {
    // Scan across the whole width; the shot occupies the middle of it.
    const hits = await rightClickShot(page, SCAN);
    expect(hits.length, 'the scan must land on the shot at least twice').toBeGreaterThanOrEqual(2);

    for (const h of hits) {
      expect(h.alongT).toBeGreaterThanOrEqual(0);
      expect(h.alongT).toBeLessThanOrEqual(1);
    }
    // The shot heads due east, so clicking further right is further along it.
    // This is what proves the value tracks the pointer rather than being a
    // constant that happens to be in range.
    const first = hits[0]!;
    const last = hits[hits.length - 1]!;
    expect(last.x).toBeGreaterThan(first.x);
    expect(last.alongT).toBeGreaterThan(first.alongT);
  });

  test('the dialog opens prefilled at the clicked spot, in both units', async ({ page }) => {
    // Scan rather than assuming a single fraction lands on the shot: the framing
    // depends on the fit-to-bounds camera, so a hardcoded 0.5 makes the test
    // fail on a framing change instead of degrading.
    const hits = await rightClickShot(page, SCAN);
    expect(hits.length, 'the scan must land on the shot').toBeGreaterThanOrEqual(1);
    const alongT = hits[0]!.alongT;

    // Re-open at the same place and go through to the dialog.
    await page.mouse.click(hits[0]!.x, (await page.locator('canvas').first().boundingBox())!.y +
      (await page.locator('canvas').first().boundingBox())!.height / 2, { button: 'right' });
    await page.getByRole('button', { name: 'Insert Station...' }).click();
    await expect(page.getByTestId('insert-station-dialog')).toBeVisible();

    const metres = Number(await page.getByTestId('insert-station-distance').inputValue());
    const percent = Number(await page.getByTestId('insert-station-percent').inputValue());

    // 100 m shot, so metres and percent coincide numerically - which also pins
    // that the two fields agree with each other and with the click.
    expect(metres).toBeCloseTo(alongT * 100, 1);
    expect(percent).toBeCloseTo(alongT * 100, 1);
    // Prefilled and immediately actionable: no extra typing needed to split at
    // the spot the user clicked.
    await expect(page.getByTestId('insert-station-confirm')).toBeEnabled();
  });

  test('the two inputs stay in sync when edited', async ({ page }) => {
    const hits = await rightClickShot(page, SCAN);
    expect(hits.length, 'the scan must land on the shot').toBeGreaterThanOrEqual(1);
    await page.mouse.click(hits[0]!.x, (await page.locator('canvas').first().boundingBox())!.y +
      (await page.locator('canvas').first().boundingBox())!.height / 2, { button: 'right' });
    await page.getByRole('button', { name: 'Insert Station...' }).click();
    await expect(page.getByTestId('insert-station-dialog')).toBeVisible();

    const metres = page.getByTestId('insert-station-distance');
    const percent = page.getByTestId('insert-station-percent');

    await metres.fill('30');
    await expect(percent).toHaveValue('30');

    await percent.fill('80');
    await expect(metres).toHaveValue('80');

    // Clearing one clears the other and disables confirm.
    await metres.fill('');
    await expect(percent).toHaveValue('');
    await expect(page.getByTestId('insert-station-confirm')).toBeDisabled();
  });
});
