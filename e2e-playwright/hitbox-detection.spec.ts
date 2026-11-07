import { test, expect, Page } from './fixtures/coverage';
import { setupTauriMocks } from './mocks/tauri-ipc';
import {
  seedProjectWithSections,
  waitForMapReady,
  waitForMapIdle,
  getDrawnGeometry,
  waitForDrawnStations,
  countCanvasInk,
  readCanvasPixel,
  type SectionSeed,
  type ScreenPoint,
  type StationScreenPos,
  type ShotScreenPos,
} from './helpers/app';

/**
 * E2E tests for hitbox detection functionality.
 *
 * These tests verify that stations and shots are detected at their configured
 * hitArea boundaries, using deterministic screen coordinates via map.project().
 *
 * NOTE (#mem budget): the dedicated survey-shots-hitbox / survey-stations-hitbox
 * layers were removed (they were full duplicate vertex buckets). Hit-testing
 * reproduces the same tolerance itself (shot reach = hit_area/2, station reach =
 * hit_area), reading config.map.*.hit_area LIVE per query (no paint property, no
 * async settle). The boundary/nearest tests below cover that behavior directly.
 *
 * NOTE (#customRender): the survey is now drawn by a custom WebGL layer, so the
 * stock survey-shots / survey-stations layers are gone too. There is nothing left
 * for queryRenderedFeatures to return, and hit-test candidates come from the SoA
 * geometry index instead (SurveyLayer.hitCandidateSections -> geometryIndex).
 * Screen positions therefore come from the app's drawn-geometry hook (helpers/app
 * getDrawnGeometry), which reads that same SoA index, so a test's coordinates are
 * exactly what the renderer painted and what the hit test will resolve. As a
 * bonus the hook carries identity ({sectionId, stationIndex} / {sectionId,
 * shotId}), so the assertions below name the exact expected feature instead of
 * discovering it behaviorally.
 *
 * The old layer-inspection tests map onto the current model like this:
 *   - "stations render above shots" is now asserted as the BEHAVIOR that z-order
 *     existed to produce: a click where a station and a shot are both within hit
 *     reach selects the STATION (SurveyLayer.handleMapClick resolves stations
 *     first),
 *   - "both hit layers are queryable" is now "both types are in the hit-test
 *     geometry index, painted, and clickable",
 *   - the debug show_hitboxes toggle and hit_area sizes drive ONLY the debug-only
 *     ghost layers survey-ghost-stations-hitbox (circle) and
 *     survey-ghost-shots-hitbox (line), which are still stock MapLibre layers on
 *     the tiny ghost sources, so those tests keep asserting paint properties,
 *   - the hit_area-behavior test stays behavioral: the same screen point flips
 *     from hit to miss purely from a live config change.
 *
 * Every test frames the survey first (see zoomToWorkableScale): tolerance is a
 * screen-space quantity, so a +/-1px boundary probe needs the geometry drawn larger
 * than the tolerance itself.
 */

/**
 * Helper to wait for the app to be ready (canvas visible and map loaded).
 */
async function waitForAppReady(page: Page): Promise<void> {
  await page.waitForSelector('canvas.maplibregl-canvas', { timeout: 10000 });
  await waitForMapReady(page);
}

/**
 * Helper to create a new project with the given name.
 */
async function createProject(page: Page, name: string, seeds = sectionSeeds): Promise<void> {
  const newProjectBtn = page.locator('button:has-text("New Project"), h3:has-text("New Project")').first();
  await newProjectBtn.click();
  await page.waitForSelector('.fixed.inset-0');
  await page.locator('.fixed.inset-0 input').first().fill(name);
  await page.click('.fixed.inset-0 button:has-text("Create")');
  await waitForAppReady(page);
  await seedProjectWithSections(page, seeds);
  // The drawn geometry is what hit-testing resolves against, so waiting for it
  // replaces the old "survey-stations returns rendered features" poll.
  await waitForDrawnStations(page, 1);
  await zoomToWorkableScale(page);
}

const mainSection: SectionSeed = {
  name: 'Hitbox Main',
  direction: 'in',
  shots: [
    { azimuth_in: 0, azimuth_out: 180, length: 10, depth_in: 0, depth_out: -2 },
    { azimuth_in: 90, azimuth_out: 270, length: 8, depth_in: -2, depth_out: -4 },
  ],
};

const sectionSeeds: SectionSeed[] = [
  mainSection,
  {
    name: 'Hitbox Branch',
    direction: 'out',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 6, depth_in: -1, depth_out: -3 },
    ],
  },
];

// The IPC mock lays every section's stations out at `x = stationIndex * 10, y = 0`
// (see mocks/tauri-ipc.ts get_project_state), so with more than one section the
// stations of section N sit exactly on top of the stations of section 1. Tests that
// must name ONE expected station out of a nearby pair seed a single section.
const singleSectionSeed: SectionSeed[] = [mainSection];

// ---------------------------------------------------------------------------
// Screen-space geometry helpers (local: only this spec needs them)
// ---------------------------------------------------------------------------

function dist(a: ScreenPoint, b: ScreenPoint): number {
  return Math.hypot(a.x - b.x, a.y - b.y);
}

/** Distance from `p` to the segment `a`-`b`, the same measure the SoA shot
 *  resolver uses (see surveyLayer/hitTest.ts distanceToLineSegment). */
function distToSegment(p: ScreenPoint, a: ScreenPoint, b: ScreenPoint): number {
  const cx = b.x - a.x;
  const cy = b.y - a.y;
  const lenSq = cx * cx + cy * cy;
  if (lenSq === 0) return dist(p, a);
  let t = ((p.x - a.x) * cx + (p.y - a.y) * cy) / lenSq;
  t = t < 0 ? 0 : t > 1 ? 1 : t;
  return Math.hypot(p.x - (a.x + t * cx), p.y - (a.y + t * cy));
}

/** Distance from `p` to the nearest station other than `exclude`. */
function nearestOtherStationDist(
  p: ScreenPoint,
  stations: StationScreenPos[],
  exclude: StationScreenPos
): number {
  let best = Infinity;
  for (const s of stations) {
    if (s === exclude) continue;
    best = Math.min(best, dist(p, s));
  }
  return best;
}

/**
 * Stations that are the only station drawn at their spot. Sections seeded without
 * a zero datum all start at the map origin, so the first station of each section
 * lands on the first station of every other section: a click there is genuinely
 * ambiguous (two stations at distance 0 from the cursor), so any assertion that
 * names ONE expected station has to avoid that cluster.
 */
function unambiguousStations(stations: StationScreenPos[], eps = 1): StationScreenPos[] {
  return stations.filter((s) => nearestOtherStationDist(s, stations, s) > eps);
}

/**
 * Hit tolerance is a SCREEN-space property (station reach = hit_area px, shot
 * reach = hit_area/2 px), but the seeded survey spans only ~5px on the camera a
 * freshly created project starts on: every probe point would then sit inside
 * several features at once and no +/-1px boundary click could be attributed to one
 * of them. Jump the camera (centered on the survey, so nothing leaves the canvas)
 * until the longest shot is at least `minShotLength` px. This scales the GEOMETRY,
 * never the tolerance, so the boundary assertions still exercise the real reach.
 */
async function zoomToWorkableScale(page: Page, minShotLength = 120): Promise<void> {
  const { stations, shots } = await getDrawnGeometry(page);
  if (stations.length === 0 || shots.length === 0) throw new Error('No drawn survey to frame');
  const longest = Math.max(...shots.map((s) => dist(s.from, s.to)));
  const centroid = stations.reduce(
    (acc, s) => ({ x: acc.x + s.x / stations.length, y: acc.y + s.y / stations.length }),
    { x: 0, y: 0 }
  );
  const zoomDelta = longest >= minShotLength ? 0 : Math.ceil(Math.log2(minShotLength / longest));
  await page.evaluate(
    ({ cx, cy, zoomDelta }) => {
      const map = (window as unknown as {
        __stalagmight_map__?: {
          unproject: (p: [number, number]) => unknown;
          getZoom: () => number;
          jumpTo: (opts: { center: unknown; zoom: number }) => void;
        };
      }).__stalagmight_map__;
      if (!map) throw new Error('Map not available');
      map.jumpTo({ center: map.unproject([cx, cy]), zoom: map.getZoom() + zoomDelta });
    },
    { cx: centroid.x, cy: centroid.y, zoomDelta }
  );
  await waitForMapIdle(page);
}

const OFFSET_DIRECTIONS: ScreenPoint[] = [
  { x: 0, y: -1 }, { x: 1, y: -1 }, { x: 1, y: 0 }, { x: 1, y: 1 },
  { x: 0, y: 1 }, { x: -1, y: 1 }, { x: -1, y: 0 }, { x: -1, y: -1 },
];

interface OffsetPick {
  station: StationScreenPos;
  /** Unit vector from the station toward the click point. */
  dir: ScreenPoint;
  /** Distance from the offset click point to the nearest OTHER station. */
  otherMinDist: number;
}

/**
 * The (station, direction) pair whose `offset`-px click point is farthest from
 * every OTHER station, so the chosen station is unambiguously the nearest one and
 * a hit/miss at that point is attributable to it alone. Distances are measured
 * against ALL drawn stations, but only stations without a coincident twin are
 * eligible targets (see {@link unambiguousStations}).
 */
function pickSafeOffsetTarget(stations: StationScreenPos[], offset: number): OffsetPick {
  let best: OffsetPick | null = null;
  for (const station of unambiguousStations(stations)) {
    for (const raw of OFFSET_DIRECTIONS) {
      const norm = Math.hypot(raw.x, raw.y) || 1;
      const dir = { x: raw.x / norm, y: raw.y / norm };
      const probe = { x: station.x + dir.x * offset, y: station.y + dir.y * offset };
      const otherMinDist = nearestOtherStationDist(probe, stations, station);
      if (!best || otherMinDist > best.otherMinDist) best = { station, dir, otherMinDist };
    }
  }
  if (!best) throw new Error('No drawn stations to pick a hit target from');
  return best;
}

/** The station farthest from every other station: a click on its center resolves
 *  to it and to nothing else. */
function mostIsolatedStation(stations: StationScreenPos[]): StationScreenPos {
  let best = stations[0]!;
  let bestMinOther = -Infinity;
  for (const s of unambiguousStations(stations)) {
    const minOther = nearestOtherStationDist(s, stations, s);
    if (minOther > bestMinOther) {
      bestMinOther = minOther;
      best = s;
    }
  }
  return best;
}

/**
 * A canvas point far from every drawn station and shot: the background sample the
 * pixel probe compares against ("nothing drawn here").
 */
function emptyCanvasPoint(
  canvasBox: { width: number; height: number },
  stations: StationScreenPos[],
  shots: ShotScreenPos[]
): ScreenPoint {
  const margin = 20;
  let best: ScreenPoint = { x: margin, y: margin };
  let bestClearance = -Infinity;
  for (let x = margin; x <= canvasBox.width - margin; x += 20) {
    for (let y = margin; y <= canvasBox.height - margin; y += 20) {
      const p = { x, y };
      let clearance = Infinity;
      for (const s of stations) clearance = Math.min(clearance, dist(p, s));
      for (const sh of shots) clearance = Math.min(clearance, distToSegment(p, sh.from, sh.to));
      if (clearance > bestClearance) {
        bestClearance = clearance;
        best = p;
      }
    }
  }
  return best;
}

interface ShotProbePick {
  shot: ShotScreenPos;
  /** Point on the shot centerline the perpendicular offsets are measured from. */
  base: ScreenPoint;
  /** Unit vector perpendicular to the shot. */
  perp: ScreenPoint;
}

/**
 * A shot plus a point on it that is clear of every station's hit reach and
 * strictly closer to that shot than to any other, so a perpendicular offset
 * click can only hit (or miss) that one shot.
 */
function pickIsolatedShotProbe(
  shots: ShotScreenPos[],
  stations: StationScreenPos[],
  stationReach: number,
  shotReach: number
): ShotProbePick {
  // Clearance: the offset click sits <= shotReach off the centerline, so keep the
  // base farther than stationReach + shotReach from every station (plus slack) to
  // guarantee no station is ever in reach (stations would win the click).
  const stationClearance = stationReach + shotReach + 5;
  for (const shot of shots) {
    const len = dist(shot.from, shot.to);
    if (len < 4 * shotReach + 10) continue;
    for (const t of [0.5, 0.35, 0.65, 0.2, 0.8]) {
      const base = {
        x: shot.from.x + (shot.to.x - shot.from.x) * t,
        y: shot.from.y + (shot.to.y - shot.from.y) * t,
      };
      let minStationDist = Infinity;
      for (const s of stations) minStationDist = Math.min(minStationDist, dist(base, s));
      if (minStationDist <= stationClearance) continue;
      // Other shots must stay clear of the whole offset band, so the nearest shot
      // at either offset click is unambiguously this one.
      let minOtherShotDist = Infinity;
      for (const other of shots) {
        if (other === shot) continue;
        minOtherShotDist = Math.min(minOtherShotDist, distToSegment(base, other.from, other.to));
      }
      if (minOtherShotDist <= 2 * shotReach + 5) continue;
      const perp = { x: -(shot.to.y - shot.from.y) / len, y: (shot.to.x - shot.from.x) / len };
      return { shot, base, perp };
    }
  }
  const lens = shots.map((s) => Math.round(dist(s.from, s.to))).join(',');
  throw new Error(
    `No shot with a station-clear, unambiguous probe point (need length >= ${4 * shotReach + 10}px ` +
      `and >= ${stationClearance}px station clearance; shot lengths on screen: ${lens})`
  );
}

/**
 * Helper to get the current selection state using expect.poll for stability.
 */
async function getSelectionState(page: Page): Promise<{
  type: string;
  stationRef?: { section_id: number; station_index: number };
  sectionId?: number;
  shotId?: number;
} | null> {
  return page.evaluate(() => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        uiStore: { getState: () => { selection: unknown } };
      };
    }).__E2E_TEST__;

    if (!stores) return null;
    return stores.uiStore.getState().selection as {
      type: string;
      stationRef?: { section_id: number; station_index: number };
      sectionId?: number;
      shotId?: number;
    } | null;
  });
}

/**
 * Helper to poll for selection state changes (more stable than waitForTimeout).
 */
async function pollSelectionState(
  page: Page,
  predicate: (sel: Awaited<ReturnType<typeof getSelectionState>>) => boolean,
  timeout = 5000
): Promise<Awaited<ReturnType<typeof getSelectionState>>> {
  let result: Awaited<ReturnType<typeof getSelectionState>> = null;
  await expect.poll(
    async () => {
      const sel = await getSelectionState(page);
      if (predicate(sel)) {
        result = sel;
        return true;
      }
      return false;
    },
    { timeout }
  ).toBe(true);
  return result;
}

/** True when the selection is exactly the given station. */
function isStationSelected(
  sel: Awaited<ReturnType<typeof getSelectionState>>,
  station: { sectionId: number; stationIndex: number }
): boolean {
  return (
    sel?.type === 'station' &&
    sel.stationRef?.section_id === station.sectionId &&
    sel.stationRef?.station_index === station.stationIndex
  );
}

/** True when the selection is exactly the given shot. */
function isShotSelected(
  sel: Awaited<ReturnType<typeof getSelectionState>>,
  shot: { sectionId: number; shotId: number }
): boolean {
  return sel?.type === 'shot' && sel.sectionId === shot.sectionId && sel.shotId === shot.shotId;
}

/**
 * Helper to get the canvas bounding box for click coordinate calculation.
 */
async function getCanvasBoundingBox(page: Page): Promise<{
  x: number;
  y: number;
  width: number;
  height: number;
}> {
  const canvas = page.locator('canvas.maplibregl-canvas');
  const box = await canvas.boundingBox();
  if (!box) throw new Error('Canvas not found');
  return box;
}

/**
 * Helper to get the hit_area value from config.
 */
async function getStationHitArea(page: Page): Promise<number> {
  return page.evaluate(() => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: { getState: () => { config: { map: { station: { hit_area: number } } } | null } };
      };
    }).__E2E_TEST__;

    if (!stores) return 10; // Default fallback
    const config = stores.projectStore.getState().config;
    return config?.map?.station?.hit_area ?? 10;
  });
}

/** Station / shot hit reach in px. Mirrors SurveyLayer.getStationHitReach() and
 *  getShotHitReach(): a station reaches its full hit_area, a shot half of it. */
async function getHitReaches(page: Page): Promise<{ station: number; shot: number }> {
  return page.evaluate(() => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: {
          getState: () => {
            config: { map: { station: { hit_area: number }; shot: { hit_area: number } } } | null;
          };
        };
      };
    }).__E2E_TEST__;
    const c = stores?.projectStore.getState().config;
    return {
      station: c?.map?.station?.hit_area ?? 10,
      shot: (c?.map?.shot?.hit_area ?? 8) / 2,
    };
  });
}

/** Whether a stock MapLibre layer exists in the live style. */
async function hasLayer(page: Page, layerId: string): Promise<boolean> {
  return page.evaluate((id) => {
    const map = (window as unknown as {
      __stalagmight_map__?: { getLayer: (l: string) => unknown };
    }).__stalagmight_map__;
    if (!map) return false;
    return Boolean(map.getLayer(id));
  }, layerId);
}

/**
 * Helper to check if debug.map.show_hitboxes changes layer opacity.
 */
async function getHitboxLayerOpacity(page: Page, layerId: string): Promise<number> {
  return page.evaluate(
    ({ layerId }) => {
      const map = (window as unknown as {
        __stalagmight_map__?: {
          getPaintProperty: (layer: string, prop: string) => number;
        };
      }).__stalagmight_map__;

      if (!map) return 0;
      return map.getPaintProperty(layerId, layerId.includes('shots') ? 'line-opacity' : 'circle-opacity') ?? 0;
    },
    { layerId }
  );
}

/**
 * Read a numeric paint property off a layer (returns 0 if unavailable).
 * Used to inspect the debug-only ghost hitbox layers' circle-radius / line-width.
 */
async function getPaintNumber(page: Page, layerId: string, prop: string): Promise<number> {
  return page.evaluate(
    ({ layerId, prop }) => {
      const map = (window as unknown as {
        __stalagmight_map__?: { getPaintProperty: (l: string, p: string) => unknown };
      }).__stalagmight_map__;
      if (!map) return 0;
      const value = map.getPaintProperty(layerId, prop);
      if (typeof value === 'number') return value;
      if (typeof value === 'string') return Number(value);
      return 0;
    },
    { layerId, prop }
  );
}

/**
 * Toggle debug.map.show_hitboxes via the project store, mirroring how the real
 * config-reload path updates SurveyLayer (setConfig -> updateDebugVisualization,
 * which drives the ghost hitbox layer opacity).
 */
async function setShowHitboxes(page: Page, value: boolean): Promise<void> {
  await page.evaluate((show) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: {
          getState: () => { config: { debug: { map: { show_hitboxes: boolean } } } | null };
          setState: (update: { config: unknown }) => void;
        };
      };
    }).__E2E_TEST__;

    if (!stores) return;
    const state = stores.projectStore.getState();
    if (!state.config) return;
    stores.projectStore.setState({
      config: {
        ...state.config,
        debug: {
          ...state.config.debug,
          map: { ...state.config.debug.map, show_hitboxes: show },
        },
      },
    });
  }, value);
}

/**
 * Set the station and/or shot hit_area via the project store. SurveyLayer reads
 * hit_area LIVE per query for actual hit-testing, and setConfig -> updateHitboxLayers
 * resizes the debug ghost layers to match, so the ghost radius/width is a reliable
 * signal that the config change has propagated to the layer.
 */
async function setHitAreas(
  page: Page,
  values: { station?: number; shot?: number }
): Promise<void> {
  await page.evaluate((v) => {
    const stores = (window as unknown as {
      __E2E_TEST__?: {
        projectStore: {
          getState: () => {
            config: { map: { station: { hit_area: number }; shot: { hit_area: number } } } | null;
          };
          setState: (update: { config: unknown }) => void;
        };
      };
    }).__E2E_TEST__;

    if (!stores) return;
    const state = stores.projectStore.getState();
    if (!state.config) return;
    stores.projectStore.setState({
      config: {
        ...state.config,
        map: {
          ...state.config.map,
          station: {
            ...state.config.map.station,
            ...(v.station !== undefined ? { hit_area: v.station } : {}),
          },
          shot: {
            ...state.config.map.shot,
            ...(v.shot !== undefined ? { hit_area: v.shot } : {}),
          },
        },
      },
    });
  }, values);
}

/** Wait for a station hit_area change to reach SurveyLayer. hit_area is read live
 *  per query, but setConfig runs from a React effect, so the ghost hitbox radius
 *  (sized by updateHitboxLayers) is the propagation signal. */
async function waitForStationHitAreaApplied(page: Page, value: number): Promise<void> {
  await expect.poll(
    async () => getPaintNumber(page, 'survey-ghost-stations-hitbox', 'circle-radius'),
    { timeout: 5000 }
  ).toBe(value);
}

test.describe('Hitbox Detection', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await page.waitForSelector('[data-testid="global-view"]');
  });

  test('station click at hitArea-1 hits, hitArea+1 misses with detail panel visible', async ({ page }) => {
    await createProject(page, 'Hitbox Test Project');

    const stations = await waitForDrawnStations(page, 2);
    const hitArea = await getStationHitArea(page);

    // Station reach == hit_area (inclusive), so hitArea-1 is inside and hitArea+1
    // outside. Pick the (station, direction) whose offset click is farthest from
    // every other station so both outcomes are attributable to that station.
    const inside = hitArea - 1;
    const outside = hitArea + 1;
    const pick = pickSafeOffsetTarget(stations, inside);
    expect(pick.otherMinDist).toBeGreaterThan(hitArea);

    const target = pick.station;
    const canvasBox = await getCanvasBoundingBox(page);

    // Ensure select tool is active
    await page.keyboard.press('s');

    // Click just INSIDE the station hit reach -> hits that exact station.
    await page.mouse.click(
      canvasBox.x + target.x + pick.dir.x * inside,
      canvasBox.y + target.y + pick.dir.y * inside
    );

    const selectionAfterHit = await pollSelectionState(page, (sel) => sel?.type === 'station');
    expect(isStationSelected(selectionAfterHit, target)).toBe(true);

    // Verify the Detail Panel shows station details
    const detailPanel = page.locator('[data-testid="detail-panel"]');
    await expect(detailPanel).toBeVisible();

    // Verify station detail panel is shown
    const stationDetail = page.locator('[data-testid="station-detail"]');
    await expect(stationDetail).toBeVisible();

    // Clear selection with Escape and verify
    await page.keyboard.press('Escape');
    await pollSelectionState(page, (sel) => sel === null);

    // Click just OUTSIDE the station hit reach -> must NOT select the target
    // station (a shot may still be under the cursor, which is fine).
    await page.mouse.click(
      canvasBox.x + target.x + pick.dir.x * outside,
      canvasBox.y + target.y + pick.dir.y * outside
    );

    // The click handler resolves selection synchronously, so read it directly.
    const selectionAfterMiss = await getSelectionState(page);
    expect(isStationSelected(selectionAfterMiss, target)).toBe(false);
  });

  test('overlapping hitboxes select nearest to click', async ({ page }) => {
    // Single section: the mock stacks every section's stations on the same
    // coordinates, and "nearest wins" cannot name one of two stations drawn at the
    // exact same point.
    await createProject(page, 'Overlap Test Project', singleSectionSeed);

    const stations = await waitForDrawnStations(page, 2);

    // Take the closest pair of (unambiguous) stations and force their hit circles
    // to overlap by raising station hit_area to their screen separation: circles of
    // radius hit_area centered `d` apart overlap while d < 2*hit_area. Driving
    // hit_area makes "overlapping hitboxes" deterministic instead of dependent on
    // the survey's on-screen scale.
    const candidates = unambiguousStations(stations);
    expect(candidates.length).toBeGreaterThanOrEqual(2);
    let pair: { a: StationScreenPos; b: StationScreenPos; d: number } | null = null;
    for (let i = 0; i < candidates.length; i++) {
      for (let j = i + 1; j < candidates.length; j++) {
        const d = dist(candidates[i]!, candidates[j]!);
        if (d <= 0) continue;
        if (!pair || d < pair.d) pair = { a: candidates[i]!, b: candidates[j]!, d };
      }
    }
    expect(pair).not.toBeNull();
    const { a, b, d } = pair!;

    const hitArea = Math.max(await getStationHitArea(page), Math.ceil(d));
    await setHitAreas(page, { station: hitArea });
    await waitForStationHitAreaApplied(page, hitArea);
    // Their hit circles now genuinely overlap.
    expect(d).toBeLessThan(2 * hitArea);

    // Click a point a quarter of the way from A to B: both stations are well
    // inside reach, A is nearer, so nearest-to-click must resolve to A.
    const clickPoint = { x: a.x + (b.x - a.x) / 4, y: a.y + (b.y - a.y) / 4 };

    expect(dist(clickPoint, a)).toBeLessThan(dist(clickPoint, b));
    expect(dist(clickPoint, a)).toBeLessThanOrEqual(hitArea);
    // No third station may be nearer than A, or "nearest wins" would not name A.
    expect(nearestOtherStationDist(clickPoint, stations, a)).toBeGreaterThan(dist(clickPoint, a));

    const canvasBox = await getCanvasBoundingBox(page);
    await page.keyboard.press('s');
    await page.mouse.click(canvasBox.x + clickPoint.x, canvasBox.y + clickPoint.y);

    const selection = await pollSelectionState(page, (sel) => sel?.type === 'station');
    // The nearer of the two overlapping stations wins, not merely "one of them".
    expect(isStationSelected(selection, a)).toBe(true);
    expect(isStationSelected(selection, b)).toBe(false);
  });

  // Ported (#customRender): the stock survey-stations / survey-shots layers are
  // gone, so "stations render above shots" can no longer be expressed as a layer
  // index. What that z-order existed to produce IS still observable: at a point
  // where a station and a shot are both within hit reach, the station wins
  // (SurveyLayer.handleMapClick resolves the nearest station before the nearest
  // shot). This asserts that behavior directly.
  test('a station wins a click it shares with a shot', async ({ page }) => {
    await createProject(page, 'Station Hit Priority');

    const stations = await waitForDrawnStations(page, 2);
    const { shots } = await getDrawnGeometry(page);
    expect(shots.length).toBeGreaterThan(0);
    const reach = await getHitReaches(page);

    // A shot endpoint coincides with a station, so a click there is 0px from both
    // the station center and the shot centerline: both are inside their reach and
    // only the priority rule decides the outcome.
    let overlap: { station: StationScreenPos; shot: ShotScreenPos } | null = null;
    for (const shot of shots) {
      for (const endpoint of [shot.from, shot.to]) {
        const station = stations.find((s) => dist(s, endpoint) < 0.5);
        if (!station) continue;
        // The station must be the only one in reach, so the click is unambiguous.
        if (nearestOtherStationDist(endpoint, stations, station) <= reach.station) continue;
        overlap = { station, shot };
        break;
      }
      if (overlap) break;
    }
    expect(overlap, 'no shot endpoint coincides with a station').not.toBeNull();
    const { station, shot } = overlap!;

    // The shot is within its own reach at the station's center, so the click is a
    // genuine hit for BOTH types and only the priority rule decides.
    expect(distToSegment(station, shot.from, shot.to)).toBeLessThanOrEqual(reach.shot);

    const canvasBox = await getCanvasBoundingBox(page);
    await page.keyboard.press('s');
    await page.mouse.click(canvasBox.x + station.x, canvasBox.y + station.y);

    const selection = await pollSelectionState(page, (sel) => sel !== null);
    expect(selection?.type).toBe('station');
    expect(isStationSelected(selection, station)).toBe(true);
  });

  // Ported (#customRender): the old "both survey layers return rendered features"
  // check guarded that the hit-test candidate gate can see BOTH types at once.
  // Candidates now come from the SoA geometry index, and the visible mark is
  // painted by the custom WebGL layer, so this asserts both types are in that
  // index, painted on the canvas, and clickable.
  test('stations and shots are both painted and hit-testable', async ({ page }) => {
    await createProject(page, 'Hit Source Test');

    const stations = await waitForDrawnStations(page, 2);
    const { shots } = await getDrawnGeometry(page);
    expect(stations.length).toBeGreaterThan(0);
    expect(shots.length).toBeGreaterThan(0);

    const reach = await getHitReaches(page);
    const shotProbe = pickIsolatedShotProbe(shots, stations, reach.station, reach.shot);
    const station = mostIsolatedStation(stations);

    // Both marks are actually painted. Station/shot colors live in packed GPU
    // vertex attributes now, so the painted pixel is the only honest oracle: take
    // an empty-canvas sample as the background and count pixels that differ.
    const canvasBox = await getCanvasBoundingBox(page);
    const empty = emptyCanvasPoint(canvasBox, stations, shots);
    const bg = await readCanvasPixel(page, empty.x, empty.y);
    // The reference block is uniform, so a nonzero ink count below really is the
    // station/shot mark and not a textured basemap.
    expect(await countCanvasInk(page, empty.x, empty.y, bg)).toBe(0);
    expect(await countCanvasInk(page, station.x, station.y, bg)).toBeGreaterThan(0);
    expect(await countCanvasInk(page, shotProbe.base.x, shotProbe.base.y, bg)).toBeGreaterThan(0);

    // And both are hit-testable: a click on each resolves to that type.
    await page.keyboard.press('s');
    await page.mouse.click(canvasBox.x + station.x, canvasBox.y + station.y);
    const stationSel = await pollSelectionState(page, (sel) => sel?.type === 'station');
    expect(isStationSelected(stationSel, station)).toBe(true);

    await page.keyboard.press('Escape');
    await pollSelectionState(page, (sel) => sel === null);

    await page.mouse.click(canvasBox.x + shotProbe.base.x, canvasBox.y + shotProbe.base.y);
    const shotSel = await pollSelectionState(page, (sel) => sel?.type === 'shot');
    expect(isShotSelected(shotSel, shotProbe.shot)).toBe(true);
  });

  test('shot click detection at hitArea boundaries', async ({ page }) => {
    await createProject(page, 'Shot Hitbox Test');

    const stations = await waitForDrawnStations(page, 2);
    const { shots } = await getDrawnGeometry(page);
    expect(shots.length).toBeGreaterThan(0);

    const reach = await getHitReaches(page);
    // Shot reach == hit_area/2 (the old hitbox line was drawn at width hit_area).
    expect(reach.shot).toBeCloseTo(4, 5);

    const { shot, base, perp } = pickIsolatedShotProbe(shots, stations, reach.station, reach.shot);
    const canvasBox = await getCanvasBoundingBox(page);

    // Ensure select tool is active
    await page.keyboard.press('s');

    // Click just INSIDE the shot reach (perpendicular offset) -> hits that shot.
    const hitOffset = reach.shot - 1;
    await page.mouse.click(
      canvasBox.x + base.x + perp.x * hitOffset,
      canvasBox.y + base.y + perp.y * hitOffset
    );

    const selectionAfterHit = await pollSelectionState(page, (sel) => sel?.type === 'shot');
    expect(isShotSelected(selectionAfterHit, shot)).toBe(true);

    // Clear selection
    await page.keyboard.press('Escape');
    await pollSelectionState(page, (sel) => sel === null);

    // Click just OUTSIDE the shot reach -> must NOT select that shot. The probe
    // point is clear of every station and every other shot, so nothing should be
    // selected at all.
    const missOffset = reach.shot + 1;
    await page.mouse.click(
      canvasBox.x + base.x + perp.x * missOffset,
      canvasBox.y + base.y + perp.y * missOffset
    );

    const selectionAfterMiss = await getSelectionState(page);
    expect(isShotSelected(selectionAfterMiss, shot)).toBe(false);
    expect(selectionAfterMiss).toBeNull();
  });
});

test.describe('Debug Hitbox Visualization', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await page.waitForSelector('[data-testid="global-view"]');
  });

  // Ported (#mem budget): the debug show_hitboxes toggle now drives ONLY the
  // debug-only ghost hitbox layers (an accepted "ghost-only debug hitboxes"
  // decision). Those are still stock MapLibre layers on the small ghost GeoJSON
  // sources, so paint queries against them remain valid under #customRender.
  test('ghost hitbox layers exist and are hidden by default', async ({ page }) => {
    await createProject(page, 'Debug Hitbox Test');

    // The debug-only ghost hitbox layers exist regardless of ghost source data.
    await expect.poll(
      async () =>
        (await hasLayer(page, 'survey-ghost-stations-hitbox')) &&
        (await hasLayer(page, 'survey-ghost-shots-hitbox')),
      { timeout: 10000 }
    ).toBe(true);

    // With debug.map.show_hitboxes at its default (false) the ghost layers are
    // fully hidden (opacity 0), so they never render in normal operation.
    const stationOpacity = await getHitboxLayerOpacity(page, 'survey-ghost-stations-hitbox');
    const shotOpacity = await getHitboxLayerOpacity(page, 'survey-ghost-shots-hitbox');
    expect(stationOpacity).toBe(0);
    expect(shotOpacity).toBe(0);
  });

  // Ported (#mem budget): retargeted to the debug-only ghost hitbox layers, which
  // are what the show_hitboxes toggle now drives (updateDebugVisualization sets
  // their opacity to 1 when enabled, 0 when disabled).
  test('hitbox opacity changes when debug.map.show_hitboxes config changes', async ({ page }) => {
    await createProject(page, 'Hitbox Visibility Toggle');

    // Poll for the ghost hitbox layers to be ready.
    await expect.poll(
      async () => hasLayer(page, 'survey-ghost-stations-hitbox'),
      { timeout: 10000 }
    ).toBe(true);

    // Default (show_hitboxes false): ghost hitbox layers are hidden.
    expect(await getHitboxLayerOpacity(page, 'survey-ghost-stations-hitbox')).toBe(0);
    expect(await getHitboxLayerOpacity(page, 'survey-ghost-shots-hitbox')).toBe(0);

    // Enable the debug toggle via the store; SurveyLayer.setConfig re-runs
    // updateDebugVisualization, which raises the ghost layer opacity to 1.
    await setShowHitboxes(page, true);

    await expect.poll(
      async () => getHitboxLayerOpacity(page, 'survey-ghost-stations-hitbox'),
      { timeout: 5000 }
    ).toBe(1);
    await expect.poll(
      async () => getHitboxLayerOpacity(page, 'survey-ghost-shots-hitbox'),
      { timeout: 5000 }
    ).toBe(1);

    // Disable it again; opacity returns to 0.
    await setShowHitboxes(page, false);

    await expect.poll(
      async () => getHitboxLayerOpacity(page, 'survey-ghost-stations-hitbox'),
      { timeout: 5000 }
    ).toBe(0);
    await expect.poll(
      async () => getHitboxLayerOpacity(page, 'survey-ghost-shots-hitbox'),
      { timeout: 5000 }
    ).toBe(0);
  });
});

test.describe('Config Reload Effects', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await page.waitForSelector('[data-testid="global-view"]');
  });

  // Ported (#mem budget): retargeted to the ghost station hitbox layer, whose
  // circle-radius updateHitboxLayers() sizes to config.map.station.hit_area
  // (independent of the show_hitboxes toggle).
  test('hitbox layer radius reflects config hit_area value', async ({ page }) => {
    await createProject(page, 'Config Hitbox Test');

    // Poll for the ghost station hitbox layer to be ready.
    await expect.poll(
      async () => hasLayer(page, 'survey-ghost-stations-hitbox'),
      { timeout: 10000 }
    ).toBe(true);

    // Get the current hitArea from config
    const hitArea = await getStationHitArea(page);
    expect(hitArea).toBe(10); // Mock config default

    // The ghost station hitbox circle-radius should equal the config hit_area.
    const layerRadius = await getPaintNumber(page, 'survey-ghost-stations-hitbox', 'circle-radius');
    expect(layerRadius).toBeCloseTo(hitArea, 2);
  });

  // Ported (#mem budget): retargeted to the ghost hitbox layers. A hit_area config
  // change flows through setConfig -> updateHitboxLayers, resizing the ghost
  // station circle-radius and ghost shot line-width.
  test('hitbox layer sizes update when config hit_area changes', async ({ page }) => {
    await createProject(page, 'Config Reload Hitbox Test');

    // Poll for the ghost hitbox layers to be ready.
    await expect.poll(
      async () => hasLayer(page, 'survey-ghost-stations-hitbox'),
      { timeout: 10000 }
    ).toBe(true);

    // Defaults from the mock config: station hit_area 10, shot hit_area 8.
    expect(await getPaintNumber(page, 'survey-ghost-stations-hitbox', 'circle-radius')).toBe(10);
    expect(await getPaintNumber(page, 'survey-ghost-shots-hitbox', 'line-width')).toBe(8);

    // Update both hit areas via the store (simulates a config reload).
    await setHitAreas(page, { station: 15, shot: 12 });

    // Ghost station radius follows the new station hit_area.
    await expect.poll(
      async () => getPaintNumber(page, 'survey-ghost-stations-hitbox', 'circle-radius'),
      { timeout: 5000 }
    ).toBe(15);

    // Ghost shot width follows the new shot hit_area.
    await expect.poll(
      async () => getPaintNumber(page, 'survey-ghost-shots-hitbox', 'line-width'),
      { timeout: 5000 }
    ).toBe(12);
  });

  // Ported (#mem budget): kept BEHAVIORAL and strictly discriminating. hit_area is
  // read LIVE per query (getStationHitReach; station reach == hit_area, inclusive
  // <=), so the SAME screen point must flip from HIT to MISS purely from a live
  // config change: we click a point offset just inside the original reach but
  // outside a reduced reach and prove the outcome flips. The removed
  // survey-stations-hitbox paint poll is replaced with a ghost-layer settle that
  // confirms setConfig propagated (config is read live, but setConfig runs from a
  // React effect, so we wait for the ghost radius to reflect the new hit_area).
  test('config hit_area change affects actual hit detection behavior', async ({ page }) => {
    await createProject(page, 'Config Hit Detection Behavior Test');

    // Reach model: station maxDistance == hit_area (inclusive). Pick an offset that
    // is inside the original reach (10) but outside the reduced reach (4), with a
    // comfortable margin on both sides.
    const ORIGINAL_HIT_AREA = 10; // mock config default
    const HIT_OFFSET = 7; // 7 <= 10 -> hits originally; 7 > 4 -> misses when reduced
    const REDUCED_HIT_AREA = 4;

    const stations = await waitForDrawnStations(page, 2);
    const hitArea = await getStationHitArea(page);
    expect(hitArea).toBe(ORIGINAL_HIT_AREA);

    // Choose the (station, direction) whose offset click is farthest from EVERY
    // other station, so the chosen station is unambiguously the nearest one.
    const pick = pickSafeOffsetTarget(stations, HIT_OFFSET);
    // The nearest OTHER station at the offset click must be farther than the
    // chosen one (HIT_OFFSET away).
    expect(pick.otherMinDist).toBeGreaterThan(HIT_OFFSET);

    const target = pick.station;
    const canvasBox = await getCanvasBoundingBox(page);
    const clickX = canvasBox.x + target.x + pick.dir.x * HIT_OFFSET;
    const clickY = canvasBox.y + target.y + pick.dir.y * HIT_OFFSET;

    // Ensure select tool is active.
    await page.keyboard.press('s');

    // 1) With the original hit_area (10), the offset-7 click hits the target.
    await page.mouse.click(clickX, clickY);
    const hitSel = await pollSelectionState(page, (sel) => sel?.type === 'station');
    expect(isStationSelected(hitSel, target)).toBe(true);

    // Clear selection.
    await page.keyboard.press('Escape');
    await pollSelectionState(page, (sel) => sel === null);

    // 2) Reduce the station hit_area and wait for the change to reach SurveyLayer.
    await setHitAreas(page, { station: REDUCED_HIT_AREA });
    await waitForStationHitAreaApplied(page, REDUCED_HIT_AREA);

    // Sanity: the click pipeline still selects after the config change (a click on
    // the station's center hits at any hit_area), confirming propagation end to end.
    await page.mouse.click(canvasBox.x + target.x, canvasBox.y + target.y);
    const stillHits = await pollSelectionState(page, (sel) => sel?.type === 'station');
    expect(isStationSelected(stillHits, target)).toBe(true);
    await page.keyboard.press('Escape');
    await pollSelectionState(page, (sel) => sel === null);

    // 3) The SAME offset-7 click now MISSES the target station (7 > reduced 4).
    // The click handler resolves selection synchronously, so read it directly
    // (a shot / different station / nothing may be selected, but NOT the target).
    await page.mouse.click(clickX, clickY);
    const afterMiss = await getSelectionState(page);
    expect(isStationSelected(afterMiss, target)).toBe(false);
  });
});

test.describe('E2E Test Infrastructure', () => {
  test.beforeEach(async ({ page }) => {
    await setupTauriMocks(page);
    await page.goto('/');
    await page.waitForSelector('[data-testid="global-view"]');
  });

  test('__E2E_TEST__ stores are exposed in dev mode', async ({ page }) => {
    await createProject(page, 'E2E Test Infrastructure');

    // Verify __E2E_TEST__ is available with required stores
    const hasTestAPI = await page.evaluate(() => {
      const testAPI = (window as unknown as {
        __E2E_TEST__?: {
          uiStore: unknown;
          projectStore: unknown;
        };
      }).__E2E_TEST__;

      return {
        hasAPI: !!testAPI,
        hasUIStore: !!testAPI?.uiStore,
        hasProjectStore: !!testAPI?.projectStore,
      };
    });

    expect(hasTestAPI.hasAPI).toBe(true);
    expect(hasTestAPI.hasUIStore).toBe(true);
    expect(hasTestAPI.hasProjectStore).toBe(true);
  });

  test('__stalagmight_map__ is exposed after map loads', async ({ page }) => {
    await createProject(page, 'Map Exposure Test');

    // Ensure map has finished loading after project seeding
    await expect.poll(
      async () => {
        return page.evaluate(() => {
          const map = (window as unknown as {
            __stalagmight_map__?: { loaded?: () => boolean; isStyleLoaded?: () => boolean };
          }).__stalagmight_map__;
          return Boolean(map && ((map.isStyleLoaded && map.isStyleLoaded()) || (map.loaded && map.loaded())));
        });
      },
      { timeout: 10000 }
    ).toBe(true);

    // Verify __stalagmight_map__ is available with required methods
    const hasMapAPI = await page.evaluate(() => {
      const map = (window as unknown as {
        __stalagmight_map__?: {
          project: (coords: [number, number]) => { x: number; y: number };
          getStyle: () => { layers: unknown[] };
          getPaintProperty: (layer: string, prop: string) => unknown;
          queryRenderedFeatures: (options?: unknown) => unknown[];
        };
      }).__stalagmight_map__;

      return {
        hasMap: !!map,
        hasProject: typeof map?.project === 'function',
        hasGetStyle: typeof map?.getStyle === 'function',
        hasGetPaintProperty: typeof map?.getPaintProperty === 'function',
        hasQueryRenderedFeatures: typeof map?.queryRenderedFeatures === 'function',
      };
    });

    expect(hasMapAPI.hasMap).toBe(true);
    // [#576 RV4-FE-P-1, NF-W8E3-1] "Loaded" is a transient state: right after the
    // first loaded frame MapLibre's sources settle and loaded() / isStyleLoaded()
    // read false again for a few frames, so a second single read flaked. Poll it.
    await expect
      .poll(() =>
        page.evaluate(() => {
          const map = (window as unknown as {
            __stalagmight_map__?: { loaded?: () => boolean; isStyleLoaded?: () => boolean };
          }).__stalagmight_map__;
          return (map?.isStyleLoaded?.() ?? false) || (map?.loaded?.() ?? false);
        }),
      )
      .toBe(true);
    expect(hasMapAPI.hasProject).toBe(true);
    expect(hasMapAPI.hasGetStyle).toBe(true);
    expect(hasMapAPI.hasGetPaintProperty).toBe(true);
    expect(hasMapAPI.hasQueryRenderedFeatures).toBe(true);
  });
});
