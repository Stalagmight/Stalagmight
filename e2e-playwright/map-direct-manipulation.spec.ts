import { test, expect, type Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  getMapCenter,
  getSelection,
  seedProjectWithSections,
  waitForMapReady,
  waitForMapIdle,
  waitForNoDialogs,
  waitForActiveTool,
  waitForDrawnStations,
  getDragGhostState,
  getDrawnGeometry,
  type MapCenter,
  type ScreenPoint,
  type SectionSeed,
} from './helpers/app';

/**
 * #409 - the map is manipulated directly, the way every other map is: a plain
 * left-drag pans it under EVERY tool, and the Pan/Zoom tools are off the toolbar
 * unless the user opts back in.
 *
 * NO `seedNavigationTools` anywhere in this file, deliberately: this is the
 * default install, which is the configuration the whole feature is about.
 *
 * The two gestures that take the left button back are covered elsewhere (the
 * zoom tool in zoom-tool.spec.ts, placement in placement-overlay.spec.ts), and
 * neither is reachable without turning a setting on or entering a wizard. What
 * IS reachable from the default install is carrying a move ghost, and that is
 * asserted here precisely because it is NOT an exception: the drag pans there
 * too, and the drop is the still click that ends it.
 */

const sectionSeeds: SectionSeed[] = [
  {
    name: 'Direct Manipulation Main',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 10, depth_in: -1, depth_out: -2 },
      { azimuth_in: 90, azimuth_out: 270, length: 15, depth_in: -2, depth_out: -3 },
      { azimuth_in: 135, azimuth_out: 315, length: 12, depth_in: -3, depth_out: -4 },
    ],
  },
];

async function getMapZoom(page: Page): Promise<number> {
  const zoom = await page.evaluate(() => {
    const map = (window as unknown as { __stalagmight_map__?: { getZoom: () => number } })
      .__stalagmight_map__;
    return map?.getZoom() ?? null;
  });
  expect(zoom, 'map instance is available').not.toBeNull();
  return zoom!;
}

/** Manhattan distance between two centers - "did the camera move at all". */
function centerDelta(a: MapCenter, b: MapCenter): number {
  return Math.abs(a.lng - b.lng) + Math.abs(a.lat - b.lat);
}

/**
 * How many move commands have reached the backend. The mocked `move_station` /
 * `move_segment` / `move_section` handlers record every call in
 * `__MOCK_MOVE_CALLS__` and deliberately do NOT mutate the project (see
 * mocks/tauri-ipc.ts), so this - not a coordinate read-back - is the oracle for
 * "was a move committed".
 */
async function getCommittedMoves(page: Page): Promise<number> {
  return page.evaluate(
    () => ((window as unknown as { __MOCK_MOVE_CALLS__?: unknown[] }).__MOCK_MOVE_CALLS__ ?? []).length
  );
}

/** The ruler's first endpoint and the merge tool's first pick, in one read. */
async function getToolPicks(page: Page): Promise<{ measureStart: unknown; mergeStationA: unknown }> {
  return page.evaluate(() => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        uiStore: { getState: () => { measureStart: unknown; mergeStationA: unknown } };
      };
    }).__E2E_TEST__;
    const state = stores?.uiStore.getState();
    return {
      measureStart: state?.measureStart ?? null,
      mergeStationA: state?.mergeStationA ?? null,
    };
  });
}

/** The single dialog slot in uiStore - read synchronously, so it is current the moment a handler has run. */
async function getActiveDialog(page: Page): Promise<string | null> {
  return page.evaluate(() => {
    const stores = (window as unknown as {
      __E2E_TEST__?: { uiStore: { getState: () => { activeDialog: string | null } } };
    }).__E2E_TEST__;
    return stores?.uiStore.getState().activeDialog ?? null;
  });
}

/** What `window` saw of the right-click a {@link armRightClickWitness} call armed. */
interface RightClickWitness {
  released: boolean;
  /** `defaultPrevented` of the `contextmenu` event: true means no native browser menu. */
  contextMenuPrevented: boolean | null;
}

/**
 * Arm a witness for the next right-click: it records the right-button release and
 * the `contextmenu` event as each bubbles up to `window`. React delegates at the
 * app root and MapLibre listens on the canvas container, both below `window`, so
 * once the witness has seen both, the app's own handlers for them have run, along
 * with the microtasks they queued. That is the positive barrier the "this did NOT
 * happen" assertions after a right-click wait on, instead of a sleep.
 */
async function armRightClickWitness(page: Page): Promise<void> {
  await page.evaluate(() => {
    const witness = { released: false, contextMenuPrevented: null as boolean | null };
    (window as unknown as { __rightClickWitness__?: typeof witness }).__rightClickWitness__ = witness;
    window.addEventListener('pointerup', (e) => {
      if (e.button === 2) witness.released = true;
    });
    window.addEventListener('contextmenu', (e) => {
      witness.contextMenuPrevented = e.defaultPrevented;
    });
  });
}

async function waitForRightClickWitness(page: Page): Promise<RightClickWitness> {
  const read = () =>
    page.evaluate(
      () =>
        (window as unknown as { __rightClickWitness__?: RightClickWitness }).__rightClickWitness__ ?? {
          released: false,
          contextMenuPrevented: null,
        }
    );
  await expect
    .poll(async () => {
      const witness = await read();
      return witness.released && witness.contextMenuPrevented !== null;
    }, { message: 'the right-click reached window: its release and its contextmenu event', timeout: 5000 })
    .toBe(true);
  return read();
}

/** What {@link holdSegmentPickup} saw of the Move Segment round trips. */
interface SegmentPickupHold {
  /** `get_anchored_section_ids` answered, so a segment click can pick up. */
  anchoredLoaded: boolean;
  /** `get_rigid_connected_section_ids` was asked and is being held. */
  requested: boolean;
}

/**
 * Hold the Move Segment pickup's round trip open. A segment pickup asks the
 * backend which sections move with it (`get_rigid_connected_section_ids`) and
 * puts the ghost up only when the answer comes back, so until
 * {@link releaseSegmentPickup} the pickup is pending with no ghost on the map.
 * Wraps the mocked invoke the way undo-redo-dropdown.spec.ts delays `undo_to`,
 * but on a promise the test resolves rather than a timer.
 */
async function holdSegmentPickup(page: Page): Promise<void> {
  await page.evaluate(() => {
    type Invoke = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
    const w = window as unknown as {
      __TAURI_INTERNALS__: { invoke: Invoke };
      __segmentPickupHold__?: SegmentPickupHold & { release: () => void };
    };
    const originalInvoke = w.__TAURI_INTERNALS__.invoke;
    let release = (): void => {};
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    const hold = { anchoredLoaded: false, requested: false, release: () => release() };
    w.__segmentPickupHold__ = hold;
    w.__TAURI_INTERNALS__.invoke = async (cmd, args) => {
      if (cmd === 'get_rigid_connected_section_ids') {
        hold.requested = true;
        await held;
        // The shared mock has no handler for this command; one section moves.
        return [args?.sectionId];
      }
      const result = await originalInvoke(cmd, args);
      if (cmd === 'get_anchored_section_ids') hold.anchoredLoaded = true;
      return result;
    };
  });
}

async function getSegmentPickupHold(page: Page): Promise<SegmentPickupHold> {
  return page.evaluate(() => {
    const hold = (window as unknown as { __segmentPickupHold__?: SegmentPickupHold }).__segmentPickupHold__;
    return { anchoredLoaded: hold?.anchoredLoaded ?? false, requested: hold?.requested ?? false };
  });
}

async function releaseSegmentPickup(page: Page): Promise<void> {
  await page.evaluate(() => {
    (window as unknown as { __segmentPickupHold__?: { release: () => void } }).__segmentPickupHold__?.release();
  });
}

/** The map canvas's own bounding box - `map.project()` is canvas-relative. */
async function canvasBox(page: Page): Promise<{ x: number; y: number; width: number; height: number }> {
  const box = await page.locator('canvas.maplibregl-canvas').boundingBox();
  expect(box, 'map canvas has a bounding box').not.toBeNull();
  return box!;
}

/**
 * Centre the camera on the seeded survey and zoom in, so the stations are tens
 * of pixels apart rather than a smear (the mock lays them out 10 m apart, which
 * at the fit-to-bounds zoom is a handful of pixels). `jumpTo` is not animated, so
 * positions read afterwards already match the new view.
 */
async function zoomInOnSurvey(page: Page, zoomDelta = 3): Promise<void> {
  await page.evaluate((dz) => {
    const w = window as unknown as {
      __stalagmight_map__?: {
        jumpTo: (opts: { center: [number, number]; zoom: number }) => void;
        getZoom: () => number;
      };
      __stalagmight_geometry__?: () => { stations: Array<{ lngLat: [number, number] }> } | null;
    };
    const map = w.__stalagmight_map__;
    const geometry = w.__stalagmight_geometry__?.();
    if (!map || !geometry || geometry.stations.length === 0) {
      throw new Error('map + drawn geometry must be ready before zooming');
    }
    const count = geometry.stations.length;
    const lng = geometry.stations.reduce((sum, s) => sum + s.lngLat[0], 0) / count;
    const lat = geometry.stations.reduce((sum, s) => sum + s.lngLat[1], 0) / count;
    map.jumpTo({ center: [lng, lat], zoom: map.getZoom() + dz });
  }, zoomDelta);
}

/**
 * Keep-out band around the canvas edges. The map canvas runs edge to edge under
 * FLOATING chrome - the two panel-reveal handles, the +/- zoom buttons, the
 * scale bar, the "Data" tab - and a PRESS that lands on one of those never
 * reaches MapLibre, so it starts no drag at all. (The release half used to hang
 * the drag handler too; that is fixed - see "releasing a drag over floating
 * chrome" below - but the inset stays: these gestures want clear canvas at both
 * ends so they exercise the map and nothing else.)
 */
const EDGE_INSET = 70;

/** Distance from `p` to the segment `a`-`b`. */
function pointToSegment(p: ScreenPoint, a: ScreenPoint, b: ScreenPoint): number {
  const vx = b.x - a.x;
  const vy = b.y - a.y;
  const lengthSq = vx * vx + vy * vy;
  if (lengthSq === 0) return Math.hypot(p.x - a.x, p.y - a.y);
  const t = Math.max(0, Math.min(1, ((p.x - a.x) * vx + (p.y - a.y) * vy) / lengthSq));
  return Math.hypot(p.x - (a.x + t * vx), p.y - (a.y + t * vy));
}

/**
 * A canvas point at least `clearance` px from every drawn station and shot -
 * "empty map", so a click or a drag there cannot resolve to a feature.
 *
 * `delta`, when given, is the gesture that will start there: only candidates
 * whose END point is also clear of the edge chrome are considered. Scans a
 * coarse grid and takes the point FARTHEST from the survey, which keeps the
 * choice deterministic for a given camera.
 */
async function findEmptyPoint(
  page: Page,
  delta: { dx: number; dy: number } = { dx: 0, dy: 0 },
  clearance = 80
): Promise<ScreenPoint> {
  const box = await canvasBox(page);
  const { stations, shots } = await getDrawnGeometry(page);
  const inside = (p: ScreenPoint): boolean =>
    p.x >= EDGE_INSET &&
    p.y >= EDGE_INSET &&
    p.x <= box.width - EDGE_INSET &&
    p.y <= box.height - EDGE_INSET;

  let best: ScreenPoint | null = null;
  let bestClearance = -Infinity;
  for (let fx = 0.05; fx <= 0.9501; fx += 0.05) {
    for (let fy = 0.05; fy <= 0.9501; fy += 0.05) {
      const candidate = { x: box.width * fx, y: box.height * fy };
      if (!inside(candidate)) continue;
      if (!inside({ x: candidate.x + delta.dx, y: candidate.y + delta.dy })) continue;
      let nearest = Infinity;
      for (const s of stations) {
        nearest = Math.min(nearest, Math.hypot(candidate.x - s.x, candidate.y - s.y));
      }
      for (const sh of shots) {
        nearest = Math.min(nearest, pointToSegment(candidate, sh.from, sh.to));
      }
      if (nearest > bestClearance) {
        bestClearance = nearest;
        best = candidate;
      }
    }
  }
  expect(best, 'a candidate point clear of the edge chrome exists').not.toBeNull();
  expect(
    bestClearance,
    `an empty map point at least ${clearance}px from the survey exists`
  ).toBeGreaterThan(clearance);
  return best!;
}

/** The station farthest from every OTHER station - a click on it is unambiguous. */
async function mostIsolatedStation(page: Page): Promise<{
  x: number;
  y: number;
  sectionId: number;
  stationIndex: number;
}> {
  const stations = await waitForDrawnStations(page, 2);
  let best = stations[0]!;
  let bestMinOther = -Infinity;
  for (const candidate of stations) {
    let minOther = Infinity;
    for (const other of stations) {
      if (other === candidate) continue;
      minOther = Math.min(minOther, Math.hypot(candidate.x - other.x, candidate.y - other.y));
    }
    if (minOther > bestMinOther) {
      bestMinOther = minOther;
      best = candidate;
    }
  }
  return best;
}

/**
 * Left-drag from one canvas point to another (canvas-relative coordinates).
 * Both ends are checked against EDGE_INSET first: a gesture that strays onto the
 * floating chrome fails here, with a message, instead of hanging the next
 * `waitForMapIdle` on a drag MapLibre was never told had ended.
 */
async function dragOnCanvas(page: Page, from: ScreenPoint, to: ScreenPoint): Promise<void> {
  const box = await canvasBox(page);
  for (const [name, p] of [['start', from], ['end', to]] as const) {
    expect(p.x, `drag ${name} x is clear of the canvas chrome`).toBeGreaterThanOrEqual(EDGE_INSET);
    expect(p.y, `drag ${name} y is clear of the canvas chrome`).toBeGreaterThanOrEqual(EDGE_INSET);
    expect(p.x, `drag ${name} x is clear of the canvas chrome`).toBeLessThanOrEqual(
      box.width - EDGE_INSET
    );
    expect(p.y, `drag ${name} y is clear of the canvas chrome`).toBeLessThanOrEqual(
      box.height - EDGE_INSET
    );
  }
  await page.mouse.move(box.x + from.x, box.y + from.y);
  await page.mouse.down();
  await page.mouse.move(box.x + to.x, box.y + to.y, { steps: 10 });
  await page.mouse.up();
}

async function setUpSurvey(page: Page): Promise<void> {
  await createProject(page, 'Direct Manipulation');
  await seedProjectWithSections(page, sectionSeeds);
  await waitForMapReady(page);
  await waitForNoDialogs(page);
  await waitForMapIdle(page);
  await waitForDrawnStations(page, 2);
  await zoomInOnSurvey(page);
  await waitForMapIdle(page);
  await waitForDrawnStations(page, 2);
}

test.describe('Map direct manipulation (#409)', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
    await setUpSurvey(page);
  });

  test('a plain left-drag on empty map pans under the default tool @smoke', async ({ page }) => {
    await waitForActiveTool(page, 'select');

    const selectionBefore = await getSelection(page);
    const centerBefore = await getMapCenter(page);
    const empty = await findEmptyPoint(page, { dx: 120, dy: 90 });

    await dragOnCanvas(page, empty, { x: empty.x + 120, y: empty.y + 90 });
    await waitForMapIdle(page);

    expect(centerDelta(await getMapCenter(page), centerBefore)).toBeGreaterThan(0.00001);
    // Panning is navigation, not selection.
    expect(await getSelection(page)).toEqual(selectionBefore);
    await waitForActiveTool(page, 'select');
  });

  test('a drag that STARTS on a station pans, and does not select it', async ({ page }) => {
    await waitForActiveTool(page, 'select');

    const station = await mostIsolatedStation(page);
    const centerBefore = await getMapCenter(page);

    await dragOnCanvas(page, station, { x: station.x + 130, y: station.y + 60 });
    await waitForMapIdle(page);

    expect(centerDelta(await getMapCenter(page), centerBefore)).toBeGreaterThan(0.00001);
    // The press landed on a feature, but the gesture was a drag - MapLibre's
    // clickTolerance is what keeps it from also being a click.
    expect(await getSelection(page)).toBeNull();
  });

  test('a plain click after a drag still selects a station', async ({ page }) => {
    await waitForActiveTool(page, 'select');

    // Pan first, so the click below is exercised on a camera that just moved -
    // the clickTolerance path has to survive that, not only a pristine map.
    const empty = await findEmptyPoint(page, { dx: 100, dy: 70 });
    await dragOnCanvas(page, empty, { x: empty.x + 100, y: empty.y + 70 });
    await waitForMapIdle(page);
    expect(await getSelection(page)).toBeNull();

    // Positions moved with the camera, so re-read them.
    const station = await mostIsolatedStation(page);
    const box = await canvasBox(page);
    await page.mouse.click(box.x + station.x, box.y + station.y);

    await expect.poll(() => getSelection(page)).toEqual({
      type: 'station',
      stationRef: { section_id: station.sectionId, station_index: station.stationIndex },
    });
  });

  for (const tool of [
    { key: 'r', name: 'measure' },
    { key: 'm', name: 'merge' },
  ]) {
    test(`a drag under ${tool.name} pans and starts no ${tool.name} interaction`, async ({ page }) => {
      await page.keyboard.press(tool.key);
      await waitForActiveTool(page, tool.name);

      const centerBefore = await getMapCenter(page);
      // Start ON a station: the tools pick stations, so this is the drag that
      // could most plausibly be mistaken for the first half of a pick.
      const station = await mostIsolatedStation(page);

      await dragOnCanvas(page, station, { x: station.x + 140, y: station.y + 70 });
      await waitForMapIdle(page);

      expect(centerDelta(await getMapCenter(page), centerBefore)).toBeGreaterThan(0.00001);

      const picks = await getToolPicks(page);
      expect(picks.measureStart, 'no ruler endpoint was placed').toBeNull();
      expect(picks.mergeStationA, 'no merge station was picked').toBeNull();
      // The tool is still the tool - panning is not a mode switch.
      await waitForActiveTool(page, tool.name);
    });
  }

  test('P and Z are inert and the navigate group is not on the toolbar', async ({ page }) => {
    const mapCanvas = page.getByTestId('map-canvas');
    await waitForActiveTool(page, 'select');

    await expect(page.getByTestId('tool-group-navigate')).toHaveCount(0);

    await page.keyboard.press('p');
    await page.keyboard.press('z');
    // settle: this proves the tool does NOT change, and a poll would accept the
    // pre-update value.
    await page.waitForTimeout(200);
    await expect(mapCanvas).toHaveAttribute('data-active-tool', 'select');
  });

  test('double-click zooms in', async ({ page }) => {
    await waitForActiveTool(page, 'select');

    const zoomBefore = await getMapZoom(page);
    const empty = await findEmptyPoint(page);
    const box = await canvasBox(page);

    await page.mouse.dblclick(box.x + empty.x, box.y + empty.y);
    await page.waitForFunction((z) => {
      const map = (window as unknown as {
        __stalagmight_map__?: { getZoom: () => number; isMoving: () => boolean };
      }).__stalagmight_map__;
      return Boolean(map && !map.isMoving() && map.getZoom() > z);
    }, zoomBefore, { timeout: 5000 });

    expect(await getMapZoom(page)).toBeGreaterThan(zoomBefore);
  });

  test('a right-click after a drag still opens the feature context menu', async ({ page }) => {
    await waitForActiveTool(page, 'select');

    const empty = await findEmptyPoint(page, { dx: 110, dy: 80 });
    await dragOnCanvas(page, empty, { x: empty.x + 110, y: empty.y + 80 });
    await waitForMapIdle(page);

    // Re-read: the drag moved the camera under the features.
    const station = await mostIsolatedStation(page);
    const box = await canvasBox(page);
    await page.mouse.click(box.x + station.x, box.y + station.y, { button: 'right' });

    await expect(page.getByTestId('context-menu')).toBeVisible({ timeout: 5000 });
  });

  test('Move Station: a drag pans even with a ghost in flight; the still click drops it', async ({ page }) => {
    await page.keyboard.press('t');
    await waitForActiveTool(page, 'move_station');

    // Not carrying: the drag belongs to the map.
    const centerBeforePan = await getMapCenter(page);
    const empty = await findEmptyPoint(page, { dx: 120, dy: 80 });
    await dragOnCanvas(page, empty, { x: empty.x + 120, y: empty.y + 80 });
    await waitForMapIdle(page);
    expect(centerDelta(await getMapCenter(page), centerBeforePan)).toBeGreaterThan(0.00001);

    // Grab a station. The move tools grab and drop on CLICKS, so this picks the
    // ghost up rather than starting a button-held drag.
    const station = await mostIsolatedStation(page);
    const box = await canvasBox(page);
    await page.mouse.click(box.x + station.x, box.y + station.y);
    await expect.poll(() => getDragGhostState(page), { timeout: 5000 }).not.toBeNull();
    expect(await getCommittedMoves(page), 'picking up commits nothing').toBe(0);

    // Carrying does NOT take the drag back: the drop is gated on the press
    // STAYING PUT (the same 3px at which the map stops calling a press a click),
    // so a real drag is a pan like any other and the ghost simply stays up.
    const centerCarrying = await getMapCenter(page);
    const carryFrom = await findEmptyPoint(page, { dx: 120, dy: 80 });
    await dragOnCanvas(page, carryFrom, { x: carryFrom.x + 120, y: carryFrom.y + 80 });
    await waitForMapIdle(page);

    expect(centerDelta(await getMapCenter(page), centerCarrying)).toBeGreaterThan(0.00001);
    // The release at the end of that drag committed nothing and dropped nothing.
    expect(await getCommittedMoves(page), 'a drag-release is not a drop').toBe(0);
    expect(await getDragGhostState(page), 'the ghost is still in flight').not.toBeNull();

    // ...and a still click, on the camera the pan just moved, IS the drop. On
    // empty map: dropping onto another station opens the snap-merge dialog
    // instead of committing, which is a different path (move-tools.md).
    const centerBeforeDrop = await getMapCenter(page);
    const dropAt = await findEmptyPoint(page);
    await page.mouse.click(box.x + dropAt.x, box.y + dropAt.y);

    await expect.poll(() => getCommittedMoves(page), { timeout: 5000 }).toBe(1);
    await expect.poll(() => getDragGhostState(page), { timeout: 5000 }).toBeNull();
    // A click is not a pan: the drop moved the survey, not the camera.
    expect(centerDelta(await getMapCenter(page), centerBeforeDrop)).toBe(0);
  });

  test('Move Station: a still right-click with a ghost in flight is not a drop', async ({ page }) => {
    await page.keyboard.press('t');
    await waitForActiveTool(page, 'move_station');

    const station = await mostIsolatedStation(page);
    const box = await canvasBox(page);
    await page.mouse.click(box.x + station.x, box.y + station.y);
    await expect.poll(() => getDragGhostState(page), { timeout: 5000 }).not.toBeNull();

    // Carry the ghost to empty map, then press and release the RIGHT button
    // without travel: the press stays put exactly like a drop click, so only
    // the button tells the two apart.
    const dropAt = await findEmptyPoint(page);
    await page.mouse.move(box.x + dropAt.x, box.y + dropAt.y, { steps: 10 });
    await armRightClickWitness(page);
    await page.mouse.click(box.x + dropAt.x, box.y + dropAt.y, { button: 'right' });

    // Wait until the release has been handled, then read: a poll would accept
    // the pre-commit value, and this proves the commit does NOT happen.
    const witness = await waitForRightClickWitness(page);
    expect(witness.contextMenuPrevented, 'no native browser menu').toBe(true);
    expect(await getCommittedMoves(page), 'a right-click is not a drop').toBe(0);
    expect(await getDragGhostState(page), 'the ghost is still in flight').not.toBeNull();

    // The left still click on the same spot is still the drop.
    await page.mouse.click(box.x + dropAt.x, box.y + dropAt.y);
    await expect.poll(() => getCommittedMoves(page), { timeout: 5000 }).toBe(1);
    await expect.poll(() => getDragGhostState(page), { timeout: 5000 }).toBeNull();
  });

  test('Move Station: a right-click on a station with a ghost in flight opens no menu', async ({ page }) => {
    await page.keyboard.press('t');
    await waitForActiveTool(page, 'move_station');

    const carried = await mostIsolatedStation(page);
    const stations = await waitForDrawnStations(page, 2);
    // Right-click the station farthest from the one being carried.
    const distance = (s: { x: number; y: number }) => Math.hypot(s.x - carried.x, s.y - carried.y);
    const target = stations.reduce((far, s) => (distance(s) > distance(far) ? s : far));
    expect(distance(target), 'a second station to right-click').toBeGreaterThan(20);

    const box = await canvasBox(page);
    await page.mouse.click(box.x + carried.x, box.y + carried.y);
    await expect.poll(() => getDragGhostState(page), { timeout: 5000 }).not.toBeNull();

    // The station menu offers Delete / Merge / Add Shot, any of which would
    // change the survey under the ghost's captured station ref, so with a ghost
    // in flight the right-click opens nothing - like the touch long-press.
    await page.mouse.move(box.x + target.x, box.y + target.y, { steps: 10 });
    await armRightClickWitness(page);
    await page.mouse.click(box.x + target.x, box.y + target.y, { button: 'right' });

    const witness = await waitForRightClickWitness(page);
    expect(witness.contextMenuPrevented, 'no native browser menu').toBe(true);
    expect(await getActiveDialog(page), 'no context menu over the ghost').toBeNull();
    await expect(page.getByTestId('context-menu')).toHaveCount(0);
    expect(await getCommittedMoves(page), 'a right-click is not a drop').toBe(0);
    expect(await getDragGhostState(page), 'the ghost is still in flight').not.toBeNull();

    // A left still click on empty map is still the drop.
    const dropAt = await findEmptyPoint(page);
    await page.mouse.move(box.x + dropAt.x, box.y + dropAt.y, { steps: 10 });
    await page.mouse.click(box.x + dropAt.x, box.y + dropAt.y);
    await expect.poll(() => getCommittedMoves(page), { timeout: 5000 }).toBe(1);
    await expect.poll(() => getDragGhostState(page), { timeout: 5000 }).toBeNull();
  });

  test('Move Segment: a right-click while the pickup round trip is pending opens no menu', async ({ page }) => {
    await holdSegmentPickup(page);
    await page.keyboard.press('v');
    await waitForActiveTool(page, 'move_segment');
    await expect
      .poll(async () => (await getSegmentPickupHold(page)).anchoredLoaded, { timeout: 5000 })
      .toBe(true);

    const carried = await mostIsolatedStation(page);
    const stations = await waitForDrawnStations(page, 2);
    const distance = (s: { x: number; y: number }) => Math.hypot(s.x - carried.x, s.y - carried.y);
    const target = stations.reduce((far, s) => (distance(s) > distance(far) ? s : far));
    expect(distance(target), 'a second station to right-click').toBeGreaterThan(20);

    const box = await canvasBox(page);
    await page.mouse.click(box.x + carried.x, box.y + carried.y);
    await expect
      .poll(async () => (await getSegmentPickupHold(page)).requested, { timeout: 5000 })
      .toBe(true);
    expect(await getDragGhostState(page), 'no ghost until the round trip returns').toBeNull();

    // The pickup is pending: the ghost goes up when the backend answers, so a
    // menu opened now would sit under it, the same hazard as with the ghost up.
    await page.mouse.move(box.x + target.x, box.y + target.y, { steps: 10 });
    await armRightClickWitness(page);
    await page.mouse.click(box.x + target.x, box.y + target.y, { button: 'right' });

    const witness = await waitForRightClickWitness(page);
    expect(witness.contextMenuPrevented, 'no native browser menu').toBe(true);
    expect(await getActiveDialog(page), 'no context menu while the pickup is pending').toBeNull();
    await expect(page.getByTestId('context-menu')).toHaveCount(0);

    await releaseSegmentPickup(page);
    await expect.poll(() => getDragGhostState(page), { timeout: 5000 }).not.toBeNull();
    expect(await getActiveDialog(page), 'no context menu under the ghost').toBeNull();
    await expect(page.getByTestId('context-menu')).toHaveCount(0);

    // Escape cancels the ghost and commits nothing, and with the pickup over
    // the same right-click opens the station menu again.
    await page.keyboard.press('Escape');
    await expect.poll(() => getDragGhostState(page), { timeout: 5000 }).toBeNull();
    expect(await getCommittedMoves(page), 'nothing was committed').toBe(0);
    await page.mouse.click(box.x + target.x, box.y + target.y, { button: 'right' });
    await expect(page.getByTestId('context-menu')).toBeVisible({ timeout: 5000 });
  });

  test('releasing a drag over floating chrome still ends the drag', async ({ page }) => {
    await waitForActiveTool(page, 'select');

    const zoomIn = page.locator('.maplibregl-ctrl-zoom-in');
    await expect(zoomIn).toBeVisible();
    const button = await zoomIn.boundingBox();
    expect(button, 'the zoom-in control has a bounding box').not.toBeNull();
    const release = {
      x: button!.x + button!.width / 2,
      y: button!.y + button!.height / 2,
    };

    const box = await canvasBox(page);
    const centerBefore = await getMapCenter(page);
    const zoomBefore = await getMapZoom(page);
    // Two legs: the first stays on clear canvas, so the camera provably moves;
    // the second carries the still-pressed button off the canvas and onto the
    // zoom control, which is where the release happens.
    const start = await findEmptyPoint(page, { dx: 120, dy: 90 });

    await page.mouse.move(box.x + start.x, box.y + start.y);
    await page.mouse.down();
    await page.mouse.move(box.x + start.x + 120, box.y + start.y + 90, { steps: 10 });
    await page.mouse.move(release.x, release.y, { steps: 10 });
    await page.mouse.up();

    // LOAD-BEARING: nothing may move the mouse from here on. MapLibre's drag
    // handler listens for mousemove on the document, so ANY later movement ends
    // the wedged drag by itself and makes this test vacuous. Every assertion
    // below reads state through page.evaluate only.
    await expect
      .poll(
        () =>
          page.evaluate(() => {
            const map = (window as unknown as {
              __stalagmight_map__?: { isMoving: () => boolean };
            }).__stalagmight_map__;
            return map ? map.isMoving() : null;
          }),
        { message: 'the drag ended even though the release landed on the zoom control' }
      )
      .toBe(false);

    // ...and it really was a drag, not a gesture that trivially had nothing to end.
    expect(centerDelta(await getMapCenter(page), centerBefore)).toBeGreaterThan(0.00001);
    // The release must not press the button underneath it: the browser fires
    // `click` on the common ancestor of the press and release targets, which is
    // the map container, not the control.
    expect(await getMapZoom(page)).toBeCloseTo(zoomBefore, 6);
  });
});
