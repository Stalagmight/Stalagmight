import { test, expect, Page } from './fixtures/coverage';
import {
  setupMockedPage,
  createProject,
  seedProjectWithSections,
  waitForMapReady,
  waitForMapIdle,
  waitForActiveTool,
  getDragGhostState,
  type SectionSeed,
} from './helpers/app';

/**
 * #79 / #20 - a move-tool drag must keep the grabbed point under the cursor across
 * a mid-drag ZOOM, and must grab the EXACT feature (not the imprecise click point).
 *
 * The drag preview keeps the grabbed feature's ORIGINAL position and carries the
 * live cursor delta as a screen-pixel offset, so the feature renders at
 * `project(original) + offset`. That used to be a MapLibre `*-translate` paint
 * property; it is now one shader uniform on the custom WebGL ghost layer
 * (`survey-drag-ghost`), read back through the app's `__stalagmight_drag_ghost__`
 * hook in `featureGap` below. Two independent bugs, each guarded here:
 *   - #20 (grab precision): the grab baselined on `unproject(click)`, which at low
 *     zoom lands meters off the feature. Each test clicks a few px OFF the feature
 *     and asserts the *feature* (snapped: a station, or the nearest point on a shot)
 *     - not the click point - sits under the cursor at the grab zoom (the `at grab
 *     zoom` checks; on un-fixed code these fail first).
 *   - #79 (zoom drift): the translate was a fixed offset from the grab's screen
 *     point, so on a mid-drag zoom the re-projected feature drifted. The fix
 *     re-projects the grab's world point each frame and re-applies on every map
 *     `move`. The `after mid-drag zoom` checks zoom+pan mid-drag and re-verify.
 *
 * Three grab paths, because they take different code: move_segment grabbed on a
 * station (`stationRef` → station coords, `updateDragPreview`), move_segment grabbed
 * on a shot line (`queryNearestShotPoint`, `updateDragPreview`), and move_station
 * (`updateStationDragPreview` - translate + per-shot geometry rebuild). There was no
 * prior E2E for move-tool dragging.
 *
 * Gap reads use `toPass` (not one-shot `evaluate`): the segment preview layer is
 * created after an async `getConnectedSectionIds` IPC, so a one-shot read can race
 * its creation on a slow CI runner.
 */

const SECTION_SEEDS: SectionSeed[] = [
  {
    name: 'Mover',
    direction: 'in',
    shots: [
      { azimuth_in: 45, azimuth_out: 225, length: 10, depth_in: 0, depth_out: -2 },
      { azimuth_in: 90, azimuth_out: 270, length: 15, depth_in: -2, depth_out: -5 },
    ],
  },
];

/**
 * Every drawn station + shot in MAP coords, read off the app's
 * `__stalagmight_geometry__` hook (the same SoA geometry index the renderer and
 * hit-testing use). The survey is drawn by a custom WebGL layer, so there is no
 * stock `survey-stations` / `survey-shots` layer for `queryRenderedFeatures`; the
 * shared helpers in helpers/app.ts project this hook to screen px, but these tests
 * need the map coords themselves because each one `setCenter`s on a feature before
 * grabbing it. Yields empty arrays until the geometry is indexed - callers poll via
 * pollGeometry.
 */
async function readDrawnGeometry(page: Page): Promise<{
  stations: Array<{ sectionId: number; stationIndex: number; lngLat: [number, number] }>;
  shots: Array<{ sectionId: number; shotId: number; from: [number, number]; to: [number, number] }>;
}> {
  return page.evaluate(() => {
    const w = window as unknown as {
      __stalagmight_geometry__?: () => {
        stations: Array<{ sectionId: number; stationIndex: number; lngLat: [number, number] }>;
        shots: Array<{ sectionId: number; shotId: number; from: [number, number]; to: [number, number] }>;
      } | null;
    };
    return w.__stalagmight_geometry__?.() ?? { stations: [], shots: [] };
  });
}

/** Every drawn station's lng/lat, in the geometry index's (stable) order. */
async function readStationLngLats(page: Page): Promise<Array<[number, number]>> {
  return (await readDrawnGeometry(page)).stations.map((s) => s.lngLat);
}

/** Every drawn shot's 2-point segment ([from, to] lng/lats). */
async function readShotSegments(page: Page): Promise<Array<[[number, number], [number, number]]>> {
  return (await readDrawnGeometry(page)).shots.map((s) => [s.from, s.to]);
}

/**
 * Poll a read-only geometry fetch until it yields >= `min` items, and return the
 * captured items. The geometry hook returns nothing until the map exists and the
 * seeded sections have been indexed, so a single one-shot read can race the seed.
 * Capturing the items on the successful poll iteration (rather than re-reading
 * afterward) makes the subsequent center/project independent of the live state.
 */
async function pollGeometry<T>(read: () => Promise<T[]>, min: number, label: string): Promise<T[]> {
  let items: T[] = [];
  await expect
    .poll(
      async () => {
        items = await read();
        return items.length;
      },
      { timeout: 10000 },
    )
    .toBeGreaterThanOrEqual(min);
  if (items.length < min) throw new Error(`${label}: need >=${min}, got ${items.length}`);
  return items;
}

/** Center on a station at `zoom`; return its canvas screen position + lng/lat. */
async function centerOnStation(page: Page, zoom: number) {
  // Capture the station coords on a successful poll, then center on the FIRST one -
  // one concrete station to grab. Centering uses the captured lng/lat, so it never
  // races the live render state.
  const pts = await pollGeometry(() => readStationLngLats(page), 1, 'centerOnStation');
  const coord = pts[0]!;
  const ll = await page.evaluate(
    ({ c, z }) => {
      const map = (window as unknown as { __stalagmight_map__?: import('maplibre-gl').Map }).__stalagmight_map__;
      if (!map) return null;
      map.setCenter(c);
      map.setZoom(z);
      return { lng: c[0], lat: c[1] };
    },
    { c: coord, z: zoom },
  );
  if (!ll) throw new Error('centerOnStation: no map');
  await waitForMapIdle(page);
  const screen = await page.evaluate((m) => {
    const map = (window as unknown as { __stalagmight_map__?: import('maplibre-gl').Map }).__stalagmight_map__;
    const p = map!.project([m.lng, m.lat]);
    return { x: p.x, y: p.y };
  }, ll);
  return { screen, lngLat: ll };
}

/**
 * Center on a shot's midpoint at `zoom`; return its screen position, lng/lat, and the
 * unit screen-vector PERPENDICULAR to the shot. Clicking `screen + perp*n` puts the
 * cursor n px off the line so the snap's nearest-point-on-shot lands back on the
 * midpoint - i.e. it proves the grab used `queryNearestShotPoint`, not the click.
 */
async function centerOnShotMidpoint(page: Page, zoom: number) {
  // Capture the first shot's segment on a successful poll, then center on its
  // midpoint using the captured lng/lats (no live-render race).
  const segs = await pollGeometry(() => readShotSegments(page), 1, 'centerOnShotMidpoint');
  const [segA, segB] = segs[0]!;
  const data = await page.evaluate(
    ({ a, b, z }) => {
      const map = (window as unknown as { __stalagmight_map__?: import('maplibre-gl').Map }).__stalagmight_map__;
      if (!map) return null;
      const mid: [number, number] = [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
      map.setCenter(mid);
      map.setZoom(z);
      return { lng: mid[0], lat: mid[1], a, b };
    },
    { a: segA, b: segB, z: zoom },
  );
  if (!data) throw new Error('centerOnShotMidpoint: no map');
  await waitForMapIdle(page);
  const proj = await page.evaluate((d) => {
    const map = (window as unknown as { __stalagmight_map__?: import('maplibre-gl').Map }).__stalagmight_map__;
    const mid = map!.project([d.lng, d.lat]);
    const pa = map!.project(d.a);
    const pb = map!.project(d.b);
    const dx = pb.x - pa.x;
    const dy = pb.y - pa.y;
    const len = Math.hypot(dx, dy) || 1;
    return { mid: { x: mid.x, y: mid.y }, perp: { x: -dy / len, y: dx / len } };
  }, data);
  return { screen: proj.mid, perp: proj.perp, lngLat: { lng: data.lng, lat: data.lat } };
}

/**
 * Where the grabbed feature renders now (project + the live drag offset) vs `cursor`,
 * plus which drag-preview path is driving it.
 *
 * The preview is the custom WebGL ghost layer (`survey-drag-ghost`,
 * surveyLayer/customRender/surveyGhostLayer.ts): its vertices are the captured
 * ORIGINAL positions and the live cursor delta is a single screen-pixel shader
 * uniform, so the grabbed feature paints at `project(original) + offset` - the same
 * arithmetic the old `getPaintProperty('survey-drag-stations','circle-translate')`
 * read fed. The offset and the projection come from `getDragGhostState` in ONE
 * evaluate (so both describe the same frame), which reads the app's
 * `__stalagmight_drag_ghost__` hook rather than the ghost layer's private fields.
 *
 * `mode` names the live path so a fallback cannot masquerade as a pass: the stock
 * GeoJSON preview (`survey-drag-preview-*`, used only when the custom ghost fails to
 * initialize) re-bakes its geometry AT the offset and leaves just the residual in
 * `*-translate`, so `project(original) + translate` would not describe it.
 */
async function featureGap(page: Page, lngLat: { lng: number; lat: number }, cursor: { x: number; y: number }) {
  const ghost = await getDragGhostState(page, lngLat);
  if (!ghost || !ghost.screen) return { mode: 'none', gx: 0, gy: 0 };
  return {
    mode: ghost.mode,
    gx: ghost.screen.x + ghost.dx - cursor.x,
    gy: ghost.screen.y + ghost.dy - cursor.y,
  };
}

/**
 * Poll until the grabbed feature (rendered at `lngLat` + the live drag offset) sits
 * within `maxMag` px of `cursor`. `toPass` retries to ride out the async preview
 * creation + the per-frame re-apply settling after a zoom.
 */
async function expectTracksCursor(
  page: Page,
  lngLat: { lng: number; lat: number },
  cursor: { x: number; y: number },
  label: string,
  maxMag = 3,
) {
  await expect(async () => {
    const g = await featureGap(page, lngLat, cursor);
    expect(g.mode, `${label}: custom drag ghost driving the preview`).toBe('custom');
    expect(Math.hypot(g.gx, g.gy), `${label}: gap from cursor (px)`).toBeLessThanOrEqual(maxMag);
  }).toPass({ timeout: 5000, intervals: [50, 100, 200, 400, 800] });
}

/** Zoom in +3 and pan (so the feature is off the zoom anchor) WITHOUT moving the mouse. */
async function zoomInAndPan(page: Page) {
  await page.evaluate(() => {
    const map = (window as unknown as { __stalagmight_map__?: import('maplibre-gl').Map }).__stalagmight_map__;
    if (!map) return;
    map.setZoom(map.getZoom() + 3);
    map.panBy([120, 90], { duration: 0 });
  });
  await waitForMapIdle(page);
}

/** Two well-separated stations of the section, centered to fit both at `zoom`. */
async function firstAndLastStation(page: Page, zoom: number) {
  // Capture the station coords on a successful poll, then take the farthest-apart
  // pair as origin/target - two well-separated stations, independent of coordinate
  // order (was: first + last Point feature). Centering uses the captured lng/lats.
  const pts = await pollGeometry(() => readStationLngLats(page), 2, 'firstAndLastStation');
  let a = pts[0]!;
  let b = pts[1]!;
  let best = -Infinity;
  for (let i = 0; i < pts.length; i++) {
    for (let j = i + 1; j < pts.length; j++) {
      const d = Math.hypot(pts[i]![0] - pts[j]![0], pts[i]![1] - pts[j]![1]);
      if (d > best) {
        best = d;
        a = pts[i]!;
        b = pts[j]!;
      }
    }
  }
  const data = await page.evaluate(
    ({ pa, pb, z }) => {
      const map = (window as unknown as { __stalagmight_map__?: import('maplibre-gl').Map }).__stalagmight_map__;
      if (!map) return null;
      map.setCenter([(pa[0] + pb[0]) / 2, (pa[1] + pb[1]) / 2]);
      map.setZoom(z);
      return { a: pa, b: pb };
    },
    { pa: a, pb: b, z: zoom },
  );
  if (!data) throw new Error('firstAndLastStation: no map');
  await waitForMapIdle(page);
  const screens = await page.evaluate((d) => {
    const map = (window as unknown as { __stalagmight_map__?: import('maplibre-gl').Map }).__stalagmight_map__;
    const pa = map!.project(d.a);
    const pb = map!.project(d.b);
    return { a: { x: pa.x, y: pa.y }, b: { x: pb.x, y: pb.y } };
  }, data);
  return {
    a: { lngLat: { lng: data.a[0], lat: data.a[1] }, screen: screens.a },
    b: { lngLat: { lng: data.b[0], lat: data.b[1] }, screen: screens.b },
  };
}

/** Current screen position of a lng/lat (e.g. to recompute a target after a zoom). */
async function projectLngLat(page: Page, lngLat: { lng: number; lat: number }) {
  return page.evaluate((ll) => {
    const map = (window as unknown as { __stalagmight_map__?: import('maplibre-gl').Map }).__stalagmight_map__;
    const p = map!.project([ll.lng, ll.lat]);
    return { x: p.x, y: p.y };
  }, lngLat);
}

/**
 * After a click-to-drop, assert the COMMITTED move offset lands the grabbed feature at
 * the cursor (#20): offset ≈ (cursorWorld - snappedFeatureWorld) in metres. Predicts with
 * the app's OWN transform via `__stalagmight_to_meters__` (mapCoordsToMeters + the live
 * gpsOrigin) - these mocked specs run with a non-null gpsOrigin (config.satellite_gps_origin),
 * so the ENU transform applies, NOT the linear fallback. An un-snapped grab (the #20 bug)
 * would be off by the grab imprecision (metres) - well outside the 1 m bound.
 */
async function expectCommitLandsAtCursor(
  page: Page,
  startLngLat: { lng: number; lat: number },
  cursorCanvas: { x: number; y: number },
  label: string,
) {
  const r = await page.evaluate(
    ({ start, c }) => {
      const w = window as unknown as {
        __stalagmight_map__?: import('maplibre-gl').Map;
        __stalagmight_to_meters__?: (lng: number, lat: number) => { x: number; y: number };
        __MOCK_MOVE_CALLS__?: Array<{ args: { offset?: { x: number; y: number } } }>;
      };
      const end = w.__stalagmight_map__!.unproject([c.x, c.y]);
      const startM = w.__stalagmight_to_meters__!(start.lng, start.lat);
      const endM = w.__stalagmight_to_meters__!(end.lng, end.lat);
      const calls = w.__MOCK_MOVE_CALLS__ ?? [];
      return {
        count: calls.length,
        offset: calls.length ? calls[calls.length - 1]!.args.offset : undefined,
        expX: endM.x - startM.x,
        expY: endM.y - startM.y,
      };
    },
    { start: startLngLat, c: cursorCanvas },
  );
  expect(r.count, `${label}: a move was committed on drop`).toBeGreaterThan(0);
  expect(r.offset, `${label}: committed offset present`).toBeTruthy();
  expect(Math.abs(r.offset!.x - r.expX), `${label}: offset.x lands at cursor`).toBeLessThan(1.0);
  expect(Math.abs(r.offset!.y - r.expY), `${label}: offset.y lands at cursor`).toBeLessThan(1.0);
}

async function activateTool(page: Page, key: string, tool: string) {
  await page.getByTestId('map-canvas').click({ position: { x: 4, y: 4 } });
  await page.keyboard.press(key);
  await waitForActiveTool(page, tool);
}

test('move_segment grabbed on a STATION snaps + tracks across a mid-drag zoom (#79, #20)', async ({ page }) => {
  await setupMockedPage(page);
  await createProject(page, 'Station Grab Move Test');
  await waitForMapReady(page);
  await seedProjectWithSections(page, SECTION_SEEDS);
  await waitForMapIdle(page);
  await activateTool(page, 'v', 'move_segment');

  const box = await page.getByTestId('map-canvas').boundingBox();
  if (!box) throw new Error('no canvas box');

  const { screen: s, lngLat } = await centerOnStation(page, 18);

  // Click-to-grab (the intentional click → move → click-to-drop UX) a few px OFF the
  // station center but inside its hit target - the baseline must snap to the EXACT
  // station, so the station (not the click point) tracks the cursor. Then move.
  await page.mouse.click(box.x + s.x + 6, box.y + s.y);
  const cursor = { x: s.x + 60, y: s.y - 40 };
  await page.mouse.move(box.x + cursor.x, box.y + cursor.y, { steps: 6 });

  await expectTracksCursor(page, lngLat, cursor, '#20 station snap at grab zoom');
  await zoomInAndPan(page);
  await expectTracksCursor(page, lngLat, cursor, '#79 station after mid-drag zoom');

  // Drop with a click at the current cursor (click-to-grab → move → click-to-drop;
  // there's no mouse.down() to pair an up() with - a click is the real drop).
  await page.mouse.click(box.x + cursor.x, box.y + cursor.y);
  await waitForMapIdle(page);
  await expectCommitLandsAtCursor(page, lngLat, cursor, '#20 station-grab commit');
});

test('move_segment grabbed on a SHOT line snaps + tracks across a mid-drag zoom (#79, #20)', async ({ page }) => {
  await setupMockedPage(page);
  await createProject(page, 'Shot Grab Move Test');
  await waitForMapReady(page);
  await seedProjectWithSections(page, SECTION_SEEDS);
  await waitForMapIdle(page);
  await activateTool(page, 'v', 'move_segment');

  const box = await page.getByTestId('map-canvas').boundingBox();
  if (!box) throw new Error('no canvas box');

  // Grab the segment by clicking a SHOT line (no station under the cursor) - this
  // takes the queryNearestShotPoint snap branch, distinct from the station path.
  const { screen: s, perp, lngLat } = await centerOnShotMidpoint(page, 18);

  // Click 3px PERPENDICULAR off the midpoint - inside the 8px-wide shot hitbox, but
  // off the centerline. The snap must pull to the nearest point ON the shot (≈ the
  // midpoint), so the midpoint - not the off-line click - tracks the cursor. Asserting
  // gap MAGNITUDE ≤1.5 cleanly separates snap (~0) from no-snap (~3px = the offset).
  await page.mouse.click(box.x + s.x + perp.x * 3, box.y + s.y + perp.y * 3);
  const cursor = { x: s.x + 60, y: s.y - 40 };
  await page.mouse.move(box.x + cursor.x, box.y + cursor.y, { steps: 6 });

  await expectTracksCursor(page, lngLat, cursor, '#20 shot snap at grab zoom', 1.5);
  await zoomInAndPan(page);
  await expectTracksCursor(page, lngLat, cursor, '#79 shot after mid-drag zoom');

  await page.mouse.click(box.x + cursor.x, box.y + cursor.y);
  await waitForMapIdle(page);
  await expectCommitLandsAtCursor(page, lngLat, cursor, '#20 shot-grab commit');
});

test('move_station grabbed off-center snaps + tracks across a mid-drag zoom (#79, #20)', async ({ page }) => {
  // Distinct rendering path: move_station uses updateStationDragPreview (translate +
  // per-shot geometry rebuild), not the segment translate of the tests above.
  await setupMockedPage(page);
  await createProject(page, 'Station Move Test');
  await waitForMapReady(page);
  await seedProjectWithSections(page, SECTION_SEEDS);
  await waitForMapIdle(page);
  await activateTool(page, 't', 'move_station');

  const box = await page.getByTestId('map-canvas').boundingBox();
  if (!box) throw new Error('no canvas box');

  const { screen: s, lngLat } = await centerOnStation(page, 18);

  // Grab the station a few px off-center; the baseline must snap to the exact station.
  await page.mouse.click(box.x + s.x + 6, box.y + s.y);
  const cursor = { x: s.x + 60, y: s.y - 40 };
  await page.mouse.move(box.x + cursor.x, box.y + cursor.y, { steps: 6 });

  await expectTracksCursor(page, lngLat, cursor, '#20 move_station snap at grab zoom');
  await zoomInAndPan(page);
  await expectTracksCursor(page, lngLat, cursor, '#79 move_station after mid-drag zoom');

  await page.mouse.click(box.x + cursor.x, box.y + cursor.y);
  await waitForMapIdle(page);
  await expectCommitLandsAtCursor(page, lngLat, cursor, '#20 move_station commit');
});

test('a press-drag while CARRYING pans the map and keeps the ghost (#409)', async ({ page }) => {
  // The drop is click-gated, so a press that travels is a pan like any other and
  // the ghost stays in flight. That makes the map move UNDER a ghost that is
  // glued to the cursor - the same per-frame re-apply as the mid-drag zoom above,
  // reached through MapLibre's own dragPan rather than a programmatic setZoom.
  await setupMockedPage(page);
  await createProject(page, 'Carry Pan Test');
  await waitForMapReady(page);
  await seedProjectWithSections(page, SECTION_SEEDS);
  await waitForMapIdle(page);
  await activateTool(page, 't', 'move_station');

  const box = await page.getByTestId('map-canvas').boundingBox();
  if (!box) throw new Error('no canvas box');

  const { screen: s, lngLat } = await centerOnStation(page, 18);

  await page.mouse.click(box.x + s.x, box.y + s.y);
  const cursor = { x: s.x + 60, y: s.y - 40 };
  await page.mouse.move(box.x + cursor.x, box.y + cursor.y, { steps: 6 });
  await expectTracksCursor(page, lngLat, cursor, '#409 carry-pan: ghost in flight before the pan');

  // Press-drag on the map. The camera must move, and the ghost must survive the
  // release - on the old modal behaviour this release WAS the drop.
  const centerBefore = await page.evaluate(() => {
    const map = (window as unknown as { __stalagmight_map__?: import('maplibre-gl').Map }).__stalagmight_map__;
    const c = map!.getCenter();
    return { lng: c.lng, lat: c.lat };
  });
  const panned = { x: cursor.x + 110, y: cursor.y + 70 };
  await page.mouse.down();
  await page.mouse.move(box.x + panned.x, box.y + panned.y, { steps: 10 });
  await page.mouse.up();
  await waitForMapIdle(page);

  const centerAfter = await page.evaluate(() => {
    const map = (window as unknown as { __stalagmight_map__?: import('maplibre-gl').Map }).__stalagmight_map__;
    const c = map!.getCenter();
    return { lng: c.lng, lat: c.lat };
  });
  expect(
    Math.abs(centerAfter.lng - centerBefore.lng) + Math.abs(centerAfter.lat - centerBefore.lat),
    '#409 carry-pan: the drag panned the camera',
  ).toBeGreaterThan(0.00001);
  expect(
    await page.evaluate(() => ((window as unknown as { __MOCK_MOVE_CALLS__?: unknown[] }).__MOCK_MOVE_CALLS__ ?? []).length),
    '#409 carry-pan: the release committed nothing',
  ).toBe(0);

  // Still glued to the cursor, on the camera the pan just moved.
  await expectTracksCursor(page, lngLat, panned, '#409 carry-pan: ghost tracks across the pan');

  // And the still click that follows is the drop, landing where the cursor is.
  await page.mouse.click(box.x + panned.x, box.y + panned.y);
  await waitForMapIdle(page);
  await expectCommitLandsAtCursor(page, lngLat, panned, '#409 carry-pan commit');
  await expect.poll(() => getDragGhostState(page), { timeout: 5000 }).toBeNull();
});

test('move_station snap-to-target holds across a mid-drag zoom (snap + #79)', async ({ page }) => {
  // Guards the M2 split: the per-frame zoom re-apply skips the O(all-stations) snap SCAN
  // but must still re-ALIGN a held snap at the new zoom. Grab one station, drag onto
  // another (snap), then zoom - the dragged station must stay locked on the target, not
  // jump back to the raw cursor.
  await setupMockedPage(page);
  await createProject(page, 'Station Snap Test');
  await waitForMapReady(page);
  await seedProjectWithSections(page, SECTION_SEEDS);
  await waitForMapIdle(page);
  await activateTool(page, 't', 'move_station');

  const box = await page.getByTestId('map-canvas').boundingBox();
  if (!box) throw new Error('no canvas box');

  const { a: origin, b: target } = await firstAndLastStation(page, 17);

  // Grab the origin station, then move to 5px OFF the target (inside the 10px snap
  // threshold). Snap must pull the origin ONTO the target, so it renders at the target
  // (gap ~0), NOT at the 5px-off cursor - that's how we know snapping fired.
  await page.mouse.click(box.x + origin.screen.x + 4, box.y + origin.screen.y);
  const near = { x: target.screen.x + 5, y: target.screen.y };
  await page.mouse.move(box.x + near.x, box.y + near.y, { steps: 8 });
  await expectTracksCursor(page, origin.lngLat, target.screen, 'snap: origin aligned onto target', 2);

  // Zoom mid-drag: the scan is skipped, but the held snap must re-align at the new zoom.
  // A naive skip would drop the origin back onto the raw cursor (~5px off the target).
  await zoomInAndPan(page);
  const targetAfter = await projectLngLat(page, target.lngLat);
  await expectTracksCursor(page, origin.lngLat, targetAfter, 'snap: origin still on target after zoom', 2);

  await page.keyboard.press('Escape');
});

test('move-tool hint banner names the phase: pick up -> drop -> gone (#409)', async ({ page }) => {
  // The move tools are click-to-pick-up -> ghost -> click-to-drop, and a press-drag pans
  // the camera instead (#409), so the banner is the only thing that explains the gesture.
  // The drop copy is asserted with a ghost PROVEN in flight (expectTracksCursor), not
  // merely after a click, so a pick-up that silently failed cannot pass this.
  await setupMockedPage(page);
  await createProject(page, 'Move Hint Banner Test');
  await waitForMapReady(page);
  await seedProjectWithSections(page, SECTION_SEEDS);
  await waitForMapIdle(page);

  const hint = page.getByTestId('move-tool-hint');

  // select (the default tool): no hint at all.
  await waitForActiveTool(page, 'select');
  await expect(hint).toHaveCount(0);

  await activateTool(page, 't', 'move_station');
  await expect(hint).toBeVisible();
  await expect(hint).toContainText('Click a station to pick it up');

  const box = await page.getByTestId('map-canvas').boundingBox();
  if (!box) throw new Error('no canvas box');
  const { screen: s, lngLat } = await centerOnStation(page, 18);

  await page.mouse.click(box.x + s.x, box.y + s.y);
  const cursor = { x: s.x + 60, y: s.y - 40 };
  await page.mouse.move(box.x + cursor.x, box.y + cursor.y, { steps: 6 });
  await expectTracksCursor(page, lngLat, cursor, '#409 hint: ghost really in flight');
  await expect(hint).toContainText('Click to drop it here');
  await expect(hint).not.toContainText('pick it up');

  // Escape cancels the move drag (MapCanvas' moveDrag Escape handler), so the banner
  // falls back to the pick-up copy - which is also what makes "Esc to cancel" honest.
  await page.keyboard.press('Escape');
  await expect(hint).toContainText('Click a station to pick it up');

  // Back to select: gone again.
  await page.keyboard.press('s');
  await waitForActiveTool(page, 'select');
  await expect(hint).toHaveCount(0);
});
