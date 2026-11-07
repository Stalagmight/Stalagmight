import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapReady,
  waitForMapIdle,
  waitForNoDialogs,
  waitForActiveTool,
  waitForDrawnStations,
  getDragGhostState,
  getDrawnGeometry,
  type ScreenPoint,
  type SectionSeed,
} from './helpers/app';

/**
 * GHOST-2 (#576) - integrity while a move ghost is carried (docs/move-tools.md,
 * "Integrity while carrying").
 *
 * Between the pickup click and the drop click the ghost and its eventual commit are
 * computed against the survey as it stood at the pickup, and station refs are
 * positional. So while a carry is in flight no other survey mutation may run: one
 * that is attempted cancels the carry instead and is not sent (R1); the carry is
 * also cancelled when the survey changes underneath it or editing becomes blocked
 * (R2), and when the tool leaves the move tools (R4).
 *
 * The oracle is what the mocked backend RECEIVES: every survey-mutating command that
 * reaches `__TAURI_INTERNALS__.invoke` is recorded (see `recordMutations`).
 */

/** The IPC commands whose backend handler runs `AppState::guard_mutation`. */
const SURVEY_MUTATIONS = [
  'create_section', 'update_section', 'delete_section', 'reverse_section', 'move_section', 'move_segment',
  'execute_batch', 'create_shot', 'update_shot', 'delete_shot', 'set_shot_kind', 'create_loop_closure',
  'remove_loop_closure', 'merge_stations', 'unmerge_stations', 'unmerge_station_pair', 'set_zero_datum',
  'remove_zero_datum', 'set_station_comment', 'move_comment', 'move_station', 'undo', 'redo', 'undo_to',
  'redo_to', 'update_project_settings', 'set_surface_elevation', 'repair_project_integrity',
  'delete_shot_and_split', 'delete_shot_range_split', 'delete_shot_range_shift', 'delete_last_shot_and_section',
  'split_shot_at_distance', 'create_section_and_merge', 'import_sections', 'execute_tml_import',
  'execute_compass_import', 'execute_walls_import', 'execute_gpx_import', 'set_project_name_for_path',
  'resolve_warning', 'unresolve_warning',
];

const sectionSeeds: SectionSeed[] = [
  {
    name: 'CaveA Main',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 10, depth_in: -1, depth_out: -2 },
      { azimuth_in: 90, azimuth_out: 270, length: 20, depth_in: -2, depth_out: -3 },
      { azimuth_in: 135, azimuth_out: 315, length: 12, depth_in: -3, depth_out: -4 },
    ],
  },
];

const MOVE_CANCELLED_TOAST = '[data-testid="toast"][data-toast-key="move-cancelled"]';

async function canvasBox(page: Page): Promise<{ x: number; y: number; width: number; height: number }> {
  const box = await page.locator('canvas.maplibregl-canvas').boundingBox();
  expect(box, 'map canvas has a bounding box').not.toBeNull();
  return box!;
}

/** Centre on the survey and zoom in so the stations are tens of pixels apart. */
async function zoomInOnSurvey(page: Page): Promise<void> {
  await page.evaluate(() => {
    const w = window as unknown as {
      __stalagmight_map__?: { jumpTo: (o: { center: [number, number]; zoom: number }) => void; getZoom: () => number };
      __stalagmight_geometry__?: () => { stations: Array<{ lngLat: [number, number] }> } | null;
    };
    const map = w.__stalagmight_map__;
    const geometry = w.__stalagmight_geometry__?.();
    if (!map || !geometry || geometry.stations.length === 0) throw new Error('map + geometry must be ready');
    const n = geometry.stations.length;
    const lng = geometry.stations.reduce((sum, s) => sum + s.lngLat[0], 0) / n;
    const lat = geometry.stations.reduce((sum, s) => sum + s.lngLat[1], 0) / n;
    map.jumpTo({ center: [lng, lat], zoom: map.getZoom() + 3 });
  });
}

function pointToSegment(p: ScreenPoint, a: ScreenPoint, b: ScreenPoint): number {
  const vx = b.x - a.x;
  const vy = b.y - a.y;
  const lengthSq = vx * vx + vy * vy;
  if (lengthSq === 0) return Math.hypot(p.x - a.x, p.y - a.y);
  const t = Math.max(0, Math.min(1, ((p.x - a.x) * vx + (p.y - a.y) * vy) / lengthSq));
  return Math.hypot(p.x - (a.x + t * vx), p.y - (a.y + t * vy));
}

/** A canvas point (canvas-relative) far from every drawn station and shot, clear of the edge chrome. */
async function findEmptyPoint(page: Page): Promise<ScreenPoint> {
  const box = await canvasBox(page);
  const { stations, shots } = await getDrawnGeometry(page);
  let best: ScreenPoint | null = null;
  let bestClearance = -Infinity;
  for (let fx = 0.15; fx <= 0.8501; fx += 0.05) {
    for (let fy = 0.2; fy <= 0.8001; fy += 0.05) {
      const candidate = { x: box.width * fx, y: box.height * fy };
      let nearest = Infinity;
      for (const s of stations) nearest = Math.min(nearest, Math.hypot(candidate.x - s.x, candidate.y - s.y));
      for (const sh of shots) nearest = Math.min(nearest, pointToSegment(candidate, sh.from, sh.to));
      if (nearest > bestClearance) {
        bestClearance = nearest;
        best = candidate;
      }
    }
  }
  expect(bestClearance, 'an empty map point exists').toBeGreaterThan(40);
  return best!;
}

/** The station farthest from every other station - a click on it is unambiguous. */
async function mostIsolatedStation(page: Page): Promise<{ x: number; y: number; sectionId: number; stationIndex: number }> {
  const stations = await waitForDrawnStations(page, 2);
  let best = stations[0]!;
  let bestMinOther = -Infinity;
  for (const candidate of stations) {
    let minOther = Infinity;
    for (const other of stations) {
      if (other !== candidate) minOther = Math.min(minOther, Math.hypot(candidate.x - other.x, candidate.y - other.y));
    }
    if (minOther > bestMinOther) {
      bestMinOther = minOther;
      best = candidate;
    }
  }
  return best;
}

/** Record every survey-mutating command the mocked backend receives, in order. */
async function recordMutations(page: Page): Promise<void> {
  await page.evaluate((mutations) => {
    type Invoke = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
    const w = window as unknown as { __TAURI_INTERNALS__: { invoke: Invoke }; __carryMutations__?: string[] };
    const log: string[] = [];
    w.__carryMutations__ = log;
    const originalInvoke = w.__TAURI_INTERNALS__.invoke;
    w.__TAURI_INTERNALS__.invoke = (cmd, args) => {
      if (mutations.includes(cmd)) log.push(cmd);
      return originalInvoke(cmd, args);
    };
  }, SURVEY_MUTATIONS);
}

async function receivedMutations(page: Page): Promise<string[]> {
  return page.evaluate(() => [...((window as unknown as { __carryMutations__?: string[] }).__carryMutations__ ?? [])]);
}

/**
 * Positive barrier for "this did NOT happen": the frame after the next one. Any
 * IPC a handler issues is queued on microtasks of the event's own task, which all
 * drain before a later animation frame runs.
 */
async function nextFrames(page: Page): Promise<void> {
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
}

/** A still left click on the canvas, returned once the browser dispatched its `click` and two frames passed. */
async function clickCanvasAndSettle(page: Page, at: ScreenPoint): Promise<void> {
  await page.evaluate(() => {
    const w = window as unknown as { __clickSeen__?: boolean };
    w.__clickSeen__ = false;
    window.addEventListener('click', () => { w.__clickSeen__ = true; }, { once: true });
  });
  const box = await canvasBox(page);
  await page.mouse.click(box.x + at.x, box.y + at.y);
  await expect
    .poll(() => page.evaluate(() => (window as unknown as { __clickSeen__?: boolean }).__clickSeen__ === true))
    .toBe(true);
  await nextFrames(page);
}

/**
 * Wait until the map has caught up with its container's layout: the container kept one
 * size across two frames, and the map's own canvas has that size.
 *
 * Opening the DataTable runs the Dock's CSS height transition on the map container, and
 * MapLibre follows it through a 50 ms-throttled resize. Until both have settled,
 * `map.project` (where the drawn station positions come from) and the click's hit test
 * disagree with the final layout, so a pickup click lands beside the station and no ghost
 * goes up - the flake under load, when the transition outlasts the test's own steps.
 */
async function waitForMapFitsContainer(page: Page): Promise<void> {
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          new Promise<boolean>((resolve) => {
            const map = (window as unknown as {
              __stalagmight_map__?: { getContainer: () => HTMLElement; getCanvas: () => HTMLCanvasElement };
            }).__stalagmight_map__;
            if (!map) {
              resolve(false);
              return;
            }
            const container = map.getContainer();
            const size = () => `${container.clientWidth}x${container.clientHeight}`;
            const before = size();
            requestAnimationFrame(() =>
              requestAnimationFrame(() => {
                const canvas = map.getCanvas();
                resolve(
                  size() === before &&
                    canvas.style.width === `${container.clientWidth}px` &&
                    canvas.style.height === `${container.clientHeight}px`,
                );
              }),
            );
          }),
      ),
    )
    .toBe(true);
}

async function store<T>(page: Page, read: string): Promise<T> {
  return page.evaluate((path) => {
    const e2e = (window as unknown as { __E2E_TEST__: Record<string, { getState: () => Record<string, unknown> }> }).__E2E_TEST__;
    const [storeName, key] = path.split('.') as [string, string];
    return e2e[storeName]!.getState()[key] as unknown;
  }, read) as Promise<T>;
}

/** Pick up the most isolated station with Move Station and carry it to empty map. */
async function carryStation(page: Page): Promise<void> {
  await page.keyboard.press('t');
  await waitForActiveTool(page, 'move_station');
  const station = await mostIsolatedStation(page);
  const box = await canvasBox(page);
  await page.mouse.click(box.x + station.x, box.y + station.y);
  await expect.poll(() => getDragGhostState(page), { timeout: 5000 }).not.toBeNull();
  const empty = await findEmptyPoint(page);
  await page.mouse.move(box.x + empty.x, box.y + empty.y, { steps: 10 });
}

async function setUpSurvey(page: Page): Promise<void> {
  await setupMockedPage(page);
  await createProject(page, 'CaveA');
  await seedProjectWithSections(page, sectionSeeds);
  await waitForMapReady(page);
  await waitForNoDialogs(page);
  await waitForMapIdle(page);
  await waitForDrawnStations(page, 2);
  await zoomInOnSurvey(page);
  await waitForMapIdle(page);
  await waitForDrawnStations(page, 2);
}

test.describe('Move carry integrity (GHOST-2)', () => {
  test.beforeEach(async ({ page }) => {
    await setUpSurvey(page);
  });

  test('Ctrl+Z while carrying cancels the move and sends no undo', async ({ page }) => {
    await recordMutations(page);
    await carryStation(page);

    await page.keyboard.press('Control+z');

    await expect(page.locator(MOVE_CANCELLED_TOAST)).toBeVisible({ timeout: 5000 });
    expect(await receivedMutations(page), 'nothing reached the backend').toEqual([]);
    expect(await getDragGhostState(page), 'the ghost is cancelled').toBeNull();

    // The carry is over, so the undo the user wanted goes through now.
    await page.keyboard.press('Control+z');
    await expect.poll(() => receivedMutations(page)).toEqual(['undo']);
  });

  test('the undo history dropdown while carrying: refused, and no error dialog on top of the notice', async ({ page }) => {
    // The shared mock reports an empty history; give the dropdown one entry to show.
    await page.evaluate(async () => {
      const w = window as unknown as {
        __MOCK_IPC_OVERRIDES__?: Record<string, unknown>;
        __E2E_TEST__: { projectStore: { getState: () => { refreshUndoState: () => Promise<void> } } };
      };
      w.__MOCK_IPC_OVERRIDES__ = {
        can_undo: true,
        get_undo_stack: [
          { index: 0, description: 'Update shot', timestamp: null, commandType: 'update_shot', previewData: null },
        ],
      };
      await w.__E2E_TEST__.projectStore.getState().refreshUndoState();
    });
    await recordMutations(page);
    await carryStation(page);

    await page.hover('[data-testid="toolbar-undo-button"]');
    await expect(page.getByTestId('toolbar-undo-button-dropdown')).toBeVisible({ timeout: 5000 });
    // A click on the trigger with the dropdown open runs through its action queue,
    // which opens an error dialog for an ordinary failed undo.
    await page.click('[data-testid="toolbar-undo-button"]');

    await expect(page.locator(MOVE_CANCELLED_TOAST)).toBeVisible({ timeout: 5000 });
    await expect(page.getByTestId('toolbar-undo-button-loading')).toHaveCount(0);
    await nextFrames(page);
    await expect(page.getByTestId('error-dialog')).toHaveCount(0);
    expect(await receivedMutations(page)).toEqual([]);
    expect(await getDragGhostState(page)).toBeNull();
  });

  test('a DataTable edit while carrying cancels the move and is not applied', async ({ page }) => {
    await page.keyboard.press('Control+3');
    await expect(page.getByTestId('data-table')).toBeVisible({ timeout: 5000 });
    // The table takes room from the map: re-read the drawn positions once the map has
    // followed the container to its final size (see waitForMapFitsContainer).
    await waitForMapFitsContainer(page);
    await waitForMapIdle(page);
    await waitForDrawnStations(page, 2);
    await recordMutations(page);
    await carryStation(page);

    // The pickup selected a station, which flips the table to its Stations view.
    await page.getByTestId('data-table-view-shots').click();
    const cell = page.getByTestId('grid-cell-1-length');
    await expect(cell).toHaveText('20.0', { timeout: 5000 });
    await cell.dblclick();
    await page.getByTestId('grid-cell-editor').fill('42');
    await page.keyboard.press('Enter');

    await expect(page.locator(MOVE_CANCELLED_TOAST)).toBeVisible({ timeout: 5000 });
    expect(await receivedMutations(page)).toEqual([]);
    expect(await getDragGhostState(page)).toBeNull();
    // The cell shows the stored value again, not the refused edit.
    await expect(cell).toHaveText('20.0');
    expect(await receivedMutations(page)).toEqual([]);
  });

  test('switching to Plan while carrying cancels the move; a click there sends nothing', async ({ page }) => {
    await recordMutations(page);
    await carryStation(page);

    await page.getByTestId('mode-plan').click();
    await expect.poll(() => store<string>(page, 'uiStore.appMode')).toBe('plan');

    expect(await getDragGhostState(page), 'the ghost does not survive into Plan').toBeNull();
    await expect(page.locator(MOVE_CANCELLED_TOAST)).toBeVisible({ timeout: 5000 });

    await clickCanvasAndSettle(page, await findEmptyPoint(page));
    expect(await receivedMutations(page), 'no move committed from Plan mode').toEqual([]);
  });

  test('switching to another tool while carrying cancels the move; its click runs only that tool', async ({ page }) => {
    await recordMutations(page);
    await carryStation(page);

    await page.keyboard.press('r');
    await waitForActiveTool(page, 'measure');
    expect(await getDragGhostState(page), 'the ghost does not follow into Measure').toBeNull();

    await clickCanvasAndSettle(page, await findEmptyPoint(page));

    expect(await store<unknown>(page, 'uiStore.measureStart'), 'the click started a measurement').not.toBeNull();
    expect(await receivedMutations(page), 'and dropped no ghost').toEqual([]);
    // The user switched tools themselves: no toast for that.
    await expect(page.locator(MOVE_CANCELLED_TOAST)).toHaveCount(0);
  });

  test('switching among the move tools keeps the carry', async ({ page }) => {
    await recordMutations(page);
    await carryStation(page);

    await page.keyboard.press('v');
    await waitForActiveTool(page, 'move_segment');
    await nextFrames(page);
    expect(await getDragGhostState(page)).not.toBeNull();

    await clickCanvasAndSettle(page, await findEmptyPoint(page));
    await expect.poll(() => receivedMutations(page)).toEqual(['move_station']);
  });

  test('a survey change underneath the ghost cancels the carry', async ({ page }) => {
    await recordMutations(page);
    await carryStation(page);

    // A refresh mints a new survey generation, exactly as a mutation that was
    // already in flight at the pickup, or a SpeleoDB tree replacement, would.
    await page.evaluate(() =>
      (window as unknown as { __E2E_TEST__: { projectStore: { getState: () => { refreshProjectState: () => Promise<void> } } } })
        .__E2E_TEST__.projectStore.getState()
        .refreshProjectState()
    );

    await expect.poll(() => getDragGhostState(page)).toBeNull();
    await expect(page.locator(MOVE_CANCELLED_TOAST)).toHaveText(/survey changed/);
    await clickCanvasAndSettle(page, await findEmptyPoint(page));
    expect(await receivedMutations(page)).toEqual([]);
  });

  test('Move Segment: a mutation while the pickup round trip is pending cancels it before the ghost goes up', async ({ page }) => {
    await page.evaluate(() => {
      type Invoke = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
      const w = window as unknown as {
        __TAURI_INTERNALS__: { invoke: Invoke };
        __segmentHold__?: { anchoredLoaded: boolean; requested: boolean; release: () => void };
      };
      const originalInvoke = w.__TAURI_INTERNALS__.invoke;
      let release = (): void => {};
      const held = new Promise<void>((resolve) => {
        release = resolve;
      });
      const hold = { anchoredLoaded: false, requested: false, release: () => release() };
      w.__segmentHold__ = hold;
      w.__TAURI_INTERNALS__.invoke = async (cmd, args) => {
        if (cmd === 'get_rigid_connected_section_ids') {
          hold.requested = true;
          await held;
          return [args?.sectionId];
        }
        const result = await originalInvoke(cmd, args);
        if (cmd === 'get_anchored_section_ids') hold.anchoredLoaded = true;
        return result;
      };
    });
    await recordMutations(page);
    await page.keyboard.press('v');
    await waitForActiveTool(page, 'move_segment');
    // A segment pickup is blocked until the anchored sections are known.
    const hold = (key: 'anchoredLoaded' | 'requested') =>
      page.evaluate(
        (k) => (window as unknown as { __segmentHold__?: Record<string, boolean> }).__segmentHold__?.[k] ?? false,
        key
      );
    await expect.poll(() => hold('anchoredLoaded'), { timeout: 5000 }).toBe(true);

    const station = await mostIsolatedStation(page);
    const box = await canvasBox(page);
    await page.mouse.click(box.x + station.x, box.y + station.y);
    await expect.poll(() => hold('requested'), { timeout: 5000 }).toBe(true);
    expect(await getDragGhostState(page), 'no ghost until the round trip returns').toBeNull();

    await page.keyboard.press('Control+z');
    await expect(page.locator(MOVE_CANCELLED_TOAST)).toBeVisible({ timeout: 5000 });
    expect(await receivedMutations(page)).toEqual([]);

    // The late answer must not put a ghost up for a carry that is already gone.
    await page.evaluate(() => (window as unknown as { __segmentHold__?: { release: () => void } }).__segmentHold__?.release());
    await nextFrames(page);
    expect(await getDragGhostState(page)).toBeNull();
  });

  test('Move Segment: Escape while the pickup round trip is pending ends the carry', async ({ page }) => {
    // Same hold as the test above: the carry is in flight from the click, but
    // `moveDrag` stays null until `get_rigid_connected_section_ids` answers.
    await page.evaluate(() => {
      type Invoke = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
      const w = window as unknown as {
        __TAURI_INTERNALS__: { invoke: Invoke };
        __segmentHold__?: { anchoredLoaded: boolean; requested: boolean; release: () => void };
      };
      const originalInvoke = w.__TAURI_INTERNALS__.invoke;
      let release = (): void => {};
      const held = new Promise<void>((resolve) => {
        release = resolve;
      });
      const hold = { anchoredLoaded: false, requested: false, release: () => release() };
      w.__segmentHold__ = hold;
      w.__TAURI_INTERNALS__.invoke = async (cmd, args) => {
        if (cmd === 'get_rigid_connected_section_ids') {
          hold.requested = true;
          await held;
          return [args?.sectionId];
        }
        const result = await originalInvoke(cmd, args);
        if (cmd === 'get_anchored_section_ids') hold.anchoredLoaded = true;
        return result;
      };
    });
    await recordMutations(page);
    await page.keyboard.press('v');
    await waitForActiveTool(page, 'move_segment');
    const hold = (key: 'anchoredLoaded' | 'requested') =>
      page.evaluate(
        (k) => (window as unknown as { __segmentHold__?: Record<string, boolean> }).__segmentHold__?.[k] ?? false,
        key
      );
    await expect.poll(() => hold('anchoredLoaded'), { timeout: 5000 }).toBe(true);

    const station = await mostIsolatedStation(page);
    const box = await canvasBox(page);
    await page.mouse.click(box.x + station.x, box.y + station.y);
    await expect.poll(() => hold('requested'), { timeout: 5000 }).toBe(true);
    expect(await getDragGhostState(page), 'no ghost until the round trip returns').toBeNull();

    await page.keyboard.press('Escape');

    // The late answer puts no ghost up for the carry Escape ended.
    await page.evaluate(() => (window as unknown as { __segmentHold__?: { release: () => void } }).__segmentHold__?.release());
    await nextFrames(page);
    expect(await getDragGhostState(page), 'no ghost after Escape').toBeNull();

    // So a click on empty map drops nothing.
    await clickCanvasAndSettle(page, await findEmptyPoint(page));
    expect(await receivedMutations(page), 'nothing sent').toEqual([]);

    // And no carry is left to refuse the next edit: the undo goes straight through.
    await page.keyboard.press('Control+z');
    await expect.poll(() => receivedMutations(page)).toEqual(['undo']);
    // Escape is the user's own gesture: no cancel notice.
    await expect(page.locator(MOVE_CANCELLED_TOAST)).toHaveCount(0);
  });

  test('the snap-merge dialog is part of the carry: a survey change closes it and nothing is sent', async ({ page }) => {
    await recordMutations(page);
    await page.keyboard.press('t');
    await waitForActiveTool(page, 'move_station');
    const stations = await waitForDrawnStations(page, 2);
    const carried = await mostIsolatedStation(page);
    const target = stations.find((s) => s !== carried && Math.hypot(s.x - carried.x, s.y - carried.y) > 20)!;
    const box = await canvasBox(page);
    await page.mouse.click(box.x + carried.x, box.y + carried.y);
    await expect.poll(() => getDragGhostState(page), { timeout: 5000 }).not.toBeNull();
    // Carry the station onto another one (it snaps - MapCanvas holds the target in a
    // ref, the store's snappedStation is cleared under Move Station) and drop it there.
    await page.mouse.move(box.x + target.x, box.y + target.y, { steps: 12 });
    await page.mouse.click(box.x + target.x, box.y + target.y);
    await expect(page.getByTestId('merge-dialog')).toBeVisible({ timeout: 5000 });

    await page.evaluate(() =>
      (window as unknown as { __E2E_TEST__: { projectStore: { getState: () => { refreshProjectState: () => Promise<void> } } } })
        .__E2E_TEST__.projectStore.getState()
        .refreshProjectState()
    );

    await expect(page.getByTestId('merge-dialog')).toHaveCount(0);
    await expect.poll(() => getDragGhostState(page)).toBeNull();
    await expect(page.locator(MOVE_CANCELLED_TOAST)).toBeVisible();
    expect(await receivedMutations(page)).toEqual([]);
  });

  test("a snap-merge confirm is the carry's own commit: one batch, no cancel", async ({ page }) => {
    await recordMutations(page);
    await page.keyboard.press('t');
    await waitForActiveTool(page, 'move_station');
    const stations = await waitForDrawnStations(page, 2);
    const carried = await mostIsolatedStation(page);
    const target = stations.find((s) => s !== carried && Math.hypot(s.x - carried.x, s.y - carried.y) > 20)!;
    const box = await canvasBox(page);
    await page.mouse.click(box.x + carried.x, box.y + carried.y);
    await expect.poll(() => getDragGhostState(page), { timeout: 5000 }).not.toBeNull();
    await page.mouse.move(box.x + target.x, box.y + target.y, { steps: 12 });
    await page.mouse.click(box.x + target.x, box.y + target.y);
    await expect(page.getByTestId('merge-dialog')).toBeVisible({ timeout: 5000 });

    await page.getByTestId('merge-confirm').click();

    await expect(page.getByTestId('merge-dialog')).toHaveCount(0);
    await expect.poll(() => receivedMutations(page)).toEqual(['execute_batch']);
    await expect(page.locator(MOVE_CANCELLED_TOAST)).toHaveCount(0);
  });

  test('a normal drop still commits exactly one move @smoke', async ({ page }) => {
    await recordMutations(page);
    await carryStation(page);

    await clickCanvasAndSettle(page, await findEmptyPoint(page));

    await expect.poll(() => receivedMutations(page)).toEqual(['move_station']);
    await expect.poll(() => getDragGhostState(page)).toBeNull();
    expect(
      await page.evaluate(() => ((window as unknown as { __MOCK_MOVE_CALLS__?: unknown[] }).__MOCK_MOVE_CALLS__ ?? []).length)
    ).toBe(1);
    await expect(page.locator(MOVE_CANCELLED_TOAST)).toHaveCount(0);
  });
});
