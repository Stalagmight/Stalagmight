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
 * Prove-via-injection guard for the deep-clone refresh (#164, from #163's review).
 *
 * The e2e mock's `get_project_state` returns `structuredClone(currentProject)` so
 * every nested array (lines, zero_datums, loop_closures, …) gets a FRESH identity
 * each refresh - mirroring the real Tauri IPC (`project.clone()` → serialized). #163
 * added that deep clone, but only the `lines`/SectionsPanel tests exercised it;
 * nothing drove a `zero_datums` change through the refresh and asserted the
 * reference-keyed memo re-derived. This test does exactly that for the
 * `[project?.zero_datums]` → `gpsOrigin` memo (MapCanvas).
 *
 * Mechanism: the mock mutates `zero_datums` IN PLACE (`datums.push`), so the datum
 * data is present either way - but only a deep clone gives the array a new identity,
 * which is what makes the `useMemo([…, project?.zero_datums])` recompute. We add a
 * datum whose GPS is far from `config.map.satellite_gps_origin`, then assert the
 * app's own coordinate transform (`__stalagmight_to_meters__`, which closes over the
 * live `gpsOrigin`) observably MOVES for a fixed input.
 *
 * INJECTION CHECK (repo rule `prove-test-guards-via-injection`): this test MUST go
 * RED if `e2e-playwright/mocks/tauri-ipc.ts` is reverted to a shallow clone that does
 * not fresh-copy `zero_datums` (e.g. `{ ...currentProject, lines: [...currentProject.lines] }`)
 * and GREEN on `structuredClone`. Verified during development by temporarily
 * reverting that clone (`get_project_state`, ~L1184).
 */

const lineSeeds: LineSeed[] = [
  {
    name: 'Datum Reactivity Line',
    direction: 'in',
    shots: [
      { azimuth_in: 0, azimuth_out: 0, length: 10, depth_in: 0, depth_out: -1 },
      { azimuth_in: 0, azimuth_out: 0, length: 10, depth_in: -1, depth_out: -2 },
    ],
  },
];

async function toMeters(page: Page, lng: number, lat: number): Promise<{ x: number; y: number }> {
  return page.evaluate(
    ([l, a]) =>
      (
        window as unknown as {
          __stalagmight_to_meters__: (lng: number, lat: number) => { x: number; y: number };
        }
      ).__stalagmight_to_meters__(l, a),
    [lng, lat] as const,
  );
}

test.describe('zero-datum reactivity through the project refresh', () => {
  test('adding a zero datum re-derives the gpsOrigin memo (deep-clone guard)', async ({ page }) => {
    await setupMockedPage(page);
    await createProject(page, 'Datum Reactivity Test');
    await seedProjectWithLines(page, lineSeeds);
    await waitForMapReady(page);
    await waitForNoDialogs(page);

    // The meters-transform hook is published by an effect keyed on `gpsOrigin`.
    await page.waitForFunction(
      () => typeof (window as unknown as { __stalagmight_to_meters__?: unknown }).__stalagmight_to_meters__ === 'function',
      undefined,
      { timeout: 5000 },
    );

    // With no zero datum, gpsOrigin falls back to config.map.satellite_gps_origin
    // (24.55, -81.78). Transforming that exact point yields ~(0, 0); capture it.
    const FIXED = { lng: -81.78, lat: 24.55 };
    const before = await toMeters(page, FIXED.lng, FIXED.lat);

    // Add a zero datum whose GPS is FAR from the satellite origin. This drives
    // `set_zero_datum` (mutating zero_datums in place) and a refresh. The derived
    // gpsOrigin - and thus the transform of FIXED - only MOVES if the
    // `[project?.zero_datums]` memo recomputed, i.e. the refresh handed it a fresh
    // array identity (the deep clone), not the same mutated-in-place reference.
    const lineId = await page.evaluate(() => {
      const first = (
        window as unknown as {
          __E2E_TEST__: { projectStore: { getState: () => { project: { lines: Array<{ id: number }> } } } };
        }
      ).__E2E_TEST__.projectStore.getState().project.lines[0];
      if (!first) throw new Error('seed produced no line');
      return first.id;
    });
    const setOk = await page.evaluate(async (sid) => {
      const res = await (
        window as unknown as {
          __E2E_TEST__: {
            projectStore: {
              getState: () => {
                setZeroDatum: (
                  ref: { line_id: number; station_index: number },
                  gps: { latitude: number; longitude: number; depth_reference: number },
                  opts: { fitToBounds: boolean },
                ) => Promise<{ success: boolean }>;
              };
            };
          };
        }
      ).__E2E_TEST__.projectStore.getState().setZeroDatum(
        { line_id: sid, station_index: 0 },
        { latitude: 30.0, longitude: -85.0, depth_reference: 0 },
        { fitToBounds: false },
      );
      return res.success;
    }, lineId);
    // Fail loudly + distinctly on a broken command setup, so a future arg-shape
    // drift doesn't surface as a misleading "deep-clone regressed" poll timeout.
    expect(setOk).toBe(true);

    // Precondition the guard depends on: the refresh propagated the new datum into
    // the store. (The mock pushes it in place onto zero_datums; get_project_state
    // deep-clones it back out - see e2e-playwright/mocks/tauri-ipc.ts `set_zero_datum`.)
    await expect
      .poll(async () =>
        page.evaluate(
          () =>
            (
              (
                window as unknown as {
                  __E2E_TEST__: { projectStore: { getState: () => { project: { zero_datums: unknown[] } | null } } };
                }
              ).__E2E_TEST__.projectStore.getState().project?.zero_datums ?? []
            ).length,
        ),
      )
      .toBe(1);

    // The transform of FIXED must change once gpsOrigin re-derives. RED on a shallow
    // clone (zero_datums ref unchanged → memo frozen → transform never moves).
    await expect
      .poll(async () => toMeters(page, FIXED.lng, FIXED.lat), { timeout: 5000 })
      .not.toEqual(before);
  });
});
