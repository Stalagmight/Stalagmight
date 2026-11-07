import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  waitForMapReady,
  waitForNoDialogs,
  seedProjectWithSections,
  waitForDrawnStations,
  type SectionSeed,
} from './helpers/app';

/**
 * The survey map's canvas cursor has ONE owner (#576 D3, src/components/Map/mapCursor.ts).
 * It used to be written from six places - the tool effect, the Ctrl/middle pan,
 * the placement effect and SurveyLayer's hover - and the cursor on screen was
 * whichever wrote last. These pin the precedence cases that raced: a placement's
 * crosshair is the cursor for as long as placement is on, whatever is hovered and
 * whatever a modifier pan did in between.
 */

const seeds: SectionSeed[] = [
  {
    name: 'CaveA Main',
    direction: 'in',
    shots: [
      { azimuth_in: 90, azimuth_out: 270, length: 10, depth_in: -1, depth_out: -3 },
      { azimuth_in: 90, azimuth_out: 270, length: 10, depth_in: -3, depth_out: -6 },
    ],
  },
];

async function canvasCursor(page: Page): Promise<string> {
  return page.evaluate(() => {
    const map = (window as unknown as { __stalagmight_map__?: { getCanvas: () => HTMLCanvasElement } }).__stalagmight_map__;
    return map?.getCanvas().style.cursor ?? '';
  });
}

async function hoveredStation(page: Page): Promise<unknown> {
  return page.evaluate(
    () =>
      (window as unknown as { __E2E_TEST__: { uiStore: { getState: () => { hoveredStation: unknown } } } }).__E2E_TEST__.uiStore.getState()
        .hoveredStation,
  );
}

async function startPlacement(page: Page): Promise<void> {
  await page.evaluate(() => {
    (window as unknown as { __E2E_TEST__: { uiStore: { getState: () => { startPlacementMode: (...a: unknown[]) => void } } } }).__E2E_TEST__.uiStore
      .getState()
      .startPlacementMode(
        {
          name: 'CaveA Placed',
          surveyor: null,
          date: null,
          color: '#ff6b6b',
          direction: 'in',
          quality: 1,
          shots: [
            {
              azimuth_in: 0, azimuth_out: 180, length: 6, depth_in: -1, depth_out: -2,
              pitch_in: null, pitch_out: null, left: null, right: null, up: null, down: null,
              comment: null, kind: 'Normal', marker_index: null, temperature: null,
            },
          ],
        },
        0,
        undefined,
        () => {},
      );
  });
  await expect(page.getByTestId('placement-mode-indicator')).toBeVisible();
}

async function openSeeded(page: Page, name: string): Promise<{ x: number; y: number; width: number; height: number }> {
  await setupMockedPage(page);
  await createProject(page, name);
  await waitForMapReady(page);
  await waitForNoDialogs(page);
  await seedProjectWithSections(page, seeds);
  await waitForDrawnStations(page, 3);
  const box = await page.getByTestId('map-canvas').boundingBox();
  if (!box) throw new Error('Map canvas not measurable');
  return box;
}

test.describe('Map cursor owner', () => {
  test('placement keeps its crosshair over a survey station and after leaving it', async ({ page }) => {
    const box = await openSeeded(page, 'Cursor Placement Hover');
    const [station] = await waitForDrawnStations(page, 3);
    await startPlacement(page);
    await expect.poll(() => canvasCursor(page)).toBe('crosshair');

    // Onto a station: SurveyLayer's hover still runs during placement, and its
    // pointer used to replace the crosshair.
    await page.mouse.move(box.x + station!.x, box.y + station!.y);
    await expect.poll(() => hoveredStation(page)).not.toBeNull();
    expect(await canvasCursor(page)).toBe('crosshair');

    // And off it: the hover fallback used to land on the TOOL's cursor (the
    // Select arrow), losing the crosshair until placement ended.
    await page.mouse.move(box.x + 8, box.y + box.height - 8);
    await expect.poll(() => hoveredStation(page)).toBeNull();
    expect(await canvasCursor(page)).toBe('crosshair');
  });

  test('a Ctrl+drag pan during placement ends back on the crosshair', async ({ page }) => {
    const box = await openSeeded(page, 'Cursor Placement Pan');
    await startPlacement(page);
    const cx = box.x + box.width * 0.75;
    const cy = box.y + box.height * 0.25;
    await page.mouse.move(cx, cy);
    await expect.poll(() => canvasCursor(page)).toBe('crosshair');

    await page.keyboard.down('Control');
    await page.mouse.down();
    await page.mouse.move(cx - 40, cy + 30, { steps: 4 });
    await expect.poll(() => canvasCursor(page)).toBe('grabbing');
    await page.mouse.up();
    await page.keyboard.up('Control');

    // Placement is still on (the pan never confirmed it), so its crosshair is the
    // cursor again. It used to stay the pan's `grab`: the release handed the pan's
    // resting cursor to the hover owner, and the Ctrl keyup re-ran only the tool
    // effect, which steps aside during placement.
    await expect(page.getByTestId('placement-mode-indicator')).toBeVisible();
    await expect.poll(() => canvasCursor(page)).toBe('crosshair');
  });
});
