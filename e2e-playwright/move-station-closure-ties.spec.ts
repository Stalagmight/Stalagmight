import { test, expect, type Page } from './fixtures/coverage';
import {
  createProject,
  getDragGhostState,
  seedProjectWithLines,
  setupMockedPage,
  waitForActiveTool,
  waitForDrawnStations,
  waitForMapIdle,
  waitForMapReady,
  waitForNoDialogs,
  type LineSeed,
  type StationScreenPos,
} from './helpers/app';

/**
 * Plan D15: Move Station moves a station with what an ALIGNED loop closure
 * ties to it, and nothing a closure-chain entry merely names.
 *
 * X has an unaligned closure to Y and an aligned one to Z. The wire gives Y a
 * closure-chain entry "Z, via X" under the X-Z closure's id (the mock builds
 * `closures_with` by the backend's rule). A walk that follows that entry as a
 * tie moves Z with Y, off X, which tears the aligned X-Z tie apart. Dragging Y
 * must move Y alone.
 *
 * The mock lays every line out on one row, station i at 10 i metres, so the
 * lines differ in length to keep Y (the last station of the longest line) on
 * a point of its own.
 */

const shot = { azimuth_in: 90, azimuth_out: 270, length: 10, depth_in: 1, depth_out: 1 };
const lineSeeds: LineSeed[] = [
  { name: 'Tie X', direction: 'in', shots: [shot] },
  { name: 'Tie Y', direction: 'in', shots: [shot, shot, shot, shot] },
  { name: 'Tie Z', direction: 'in', shots: [shot, shot] },
];

type Ref = { line_id: number; station_index: number };

async function lineIdsByName(page: Page): Promise<Record<string, number>> {
  return page.evaluate(() => {
    const store = (window as unknown as {
      __E2E_TEST__: { projectStore: { getState: () => { project: { lines: Array<{ id: number; name: string }> } | null } } };
    }).__E2E_TEST__.projectStore;
    return Object.fromEntries((store.getState().project?.lines ?? []).map((l) => [l.name, l.id]));
  });
}

/** Every station ref a committed Move Station sent, through either path
 *  (a lone `move_station`, or one `execute_batch` of them). */
async function movedStations(page: Page): Promise<Ref[]> {
  return page.evaluate(() => {
    const w = window as unknown as {
      __MOCK_MOVE_CALLS__?: Array<{ command: string; args: { stationRef?: Ref } | null }>;
      __MOCK_EXECUTE_BATCH_CALLS__?: Array<Array<{ type?: string; station_ref?: Ref }> | null>;
    };
    const single = (w.__MOCK_MOVE_CALLS__ ?? [])
      .filter((c) => c.command === 'move_station' && c.args?.stationRef)
      .map((c) => c.args!.stationRef!);
    const batched = (w.__MOCK_EXECUTE_BATCH_CALLS__ ?? [])
      .flatMap((cmds) => cmds ?? [])
      .filter((c) => c.type === 'move_station' && c.station_ref)
      .map((c) => c.station_ref!);
    return [...single, ...batched].map((r) => ({ line_id: r.line_id, station_index: r.station_index }));
  });
}

function drawn(stations: StationScreenPos[], ref: Ref): StationScreenPos {
  const found = stations.find((s) => s.lineId === ref.line_id && s.stationIndex === ref.station_index);
  expect(found, `station ${ref.line_id}:${ref.station_index} is drawn`).toBeDefined();
  return found!;
}

test.describe('Move Station and loop-closure ties (plan D15)', () => {
  test.beforeEach(async ({ page }) => {
    await setupMockedPage(page);
  });

  test('dragging Y moves Y alone, not the station a partner is tied to', async ({ page }) => {
    await createProject(page, 'Closure Ties');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapReady(page);
    await waitForNoDialogs(page);

    const ids = await lineIdsByName(page);
    const x: Ref = { line_id: ids['Tie X']!, station_index: 1 };
    const y: Ref = { line_id: ids['Tie Y']!, station_index: 4 };
    const z: Ref = { line_id: ids['Tie Z']!, station_index: 2 };

    // X-Y unaligned, X-Z aligned (the mock creates closures unaligned; the
    // backend would call a snapped pair aligned).
    await page.evaluate(
      async ({ x, y, z }) => {
        const w = window as unknown as {
          __E2E_TEST__: {
            projectStore: {
              getState: () => {
                createLoopClosure: (a: Ref, b: Ref, s: string) => Promise<{ success: boolean }>;
                refreshProjectState: () => Promise<void>;
                project: { loop_closures: Array<{ id: bigint; station_a: Ref; station_b: Ref }> } | null;
              };
            };
          };
          __MOCK_SET_CLOSURE_ALIGNED__: (id: bigint, aligned: boolean) => boolean;
        };
        const store = w.__E2E_TEST__.projectStore;
        if (!(await store.getState().createLoopClosure(x, y, 'Batch')).success) throw new Error('X-Y closure');
        if (!(await store.getState().createLoopClosure(x, z, 'Batch')).success) throw new Error('X-Z closure');
        const xz = store
          .getState()
          .project!.loop_closures.find(
            (c) => c.station_b.line_id === z.line_id && c.station_b.station_index === z.station_index,
          );
        if (!xz || !w.__MOCK_SET_CLOSURE_ALIGNED__(xz.id, true)) throw new Error('X-Z not aligned');
        await store.getState().refreshProjectState();
      },
      { x, y, z },
    );
    await expect
      .poll(() =>
        page.evaluate(() => {
          const store = (window as unknown as {
            __E2E_TEST__: { projectStore: { getState: () => { project: { loop_closures: Array<{ is_aligned: boolean }> } | null } } };
          }).__E2E_TEST__.projectStore;
          return (store.getState().project?.loop_closures ?? []).map((c) => c.is_aligned);
        }),
      )
      .toEqual([false, true]);
    await waitForMapIdle(page);

    // Centre on Y and zoom in, so stations 10 m apart are well apart on screen.
    await waitForDrawnStations(page, 3);
    await page.evaluate((y) => {
      const w = window as unknown as {
        __stalagmight_map__: {
          getZoom: () => number;
          jumpTo: (o: { center: [number, number]; zoom: number }) => void;
        };
        __stalagmight_geometry__: () => {
          stations: Array<{ lineId: number; stationIndex: number; lngLat: [number, number] }>;
        } | null;
      };
      const at = w.__stalagmight_geometry__()?.stations.find(
        (s) => s.lineId === y.line_id && s.stationIndex === y.station_index,
      );
      if (!at) throw new Error('Y is not drawn');
      w.__stalagmight_map__.jumpTo({ center: at.lngLat, zoom: w.__stalagmight_map__.getZoom() + 3 });
    }, y);
    await waitForMapIdle(page);

    const stations = await waitForDrawnStations(page, 3);
    const yAt = drawn(stations, y);
    for (const other of stations) {
      if (other === yAt) continue;
      expect(Math.hypot(other.x - yAt.x, other.y - yAt.y), 'Y has a point of its own').toBeGreaterThan(20);
    }

    await page.keyboard.press('t');
    await waitForActiveTool(page, 'move_station');
    const box = await page.locator('canvas.maplibregl-canvas').boundingBox();
    expect(box).not.toBeNull();
    // Pick Y up (the move tools grab and drop on clicks), then drop it on a
    // still click a little further along.
    await page.mouse.click(box!.x + yAt.x, box!.y + yAt.y);
    await expect.poll(() => getDragGhostState(page), { timeout: 5000 }).not.toBeNull();
    await page.mouse.move(box!.x + yAt.x + 30, box!.y + yAt.y + 40, { steps: 5 });
    await page.mouse.click(box!.x + yAt.x + 30, box!.y + yAt.y + 40);
    await expect.poll(() => getDragGhostState(page), { timeout: 5000 }).toBeNull();

    await expect.poll(() => movedStations(page), { timeout: 5000 }).toEqual([y]);
  });
});
