import type { Page } from '../fixtures/coverage';
import type { GasReachField } from '../../src/types/ipc/GasReachField';
import type { StationRef } from '../../src/types/ipc/StationRef';

/**
 * The gas planning specs' view of `compute_gas_reach_field` (#576 E3): what the panel SENT
 * (the mock records every call's inputs in `__MOCK_GAS_REACH_CALLS__`) and what the engine
 * ANSWERS when a spec needs more than the mock's canned field (`seedGasField`, laid over it
 * through `__MOCK_SET_GAS_FIELD_PATCH__`). Split out of gas-planning.spec.ts.
 */

/** The `endpoint` argument the LAST compute_gas_reach_field call was invoked with (#310). */
export async function lastGasReachEndpoint(page: Page): Promise<StationRef | null> {
  return page.evaluate(() => {
    const calls =
      (
        window as unknown as {
          __MOCK_GAS_REACH_CALLS__?: Array<{ endpoint: StationRef | null }>;
        }
      ).__MOCK_GAS_REACH_CALLS__ ?? [];
    return calls.length ? calls[calls.length - 1]!.endpoint : null;
  });
}

/** The `stage_drop_overrides` argument the LAST compute_gas_reach_field call was invoked with. */
export async function lastGasReachOverrides(page: Page): Promise<Array<StationRef | null>> {
  return page.evaluate(() => {
    const calls =
      (
        window as unknown as {
          __MOCK_GAS_REACH_CALLS__?: Array<{ stageDropOverrides: Array<StationRef | null> }>;
        }
      ).__MOCK_GAS_REACH_CALLS__ ?? [];
    return calls.length ? calls[calls.length - 1]!.stageDropOverrides : [];
  });
}

export type PlanSegmentLite = { speed_m_per_min: number | null; on_scooter: boolean };

/** The `segments` argument the LAST compute_gas_reach_field call was invoked with (#317) - the
 *  wiring guard for the Segments table: a table that only re-rendered itself would leave this
 *  stale, since only `setSegments` (which nulls `reachField`) triggers a refetch. */
export async function lastGasReachSegments(page: Page): Promise<PlanSegmentLite[]> {
  return page.evaluate(() => {
    const calls =
      (
        window as unknown as {
          __MOCK_GAS_REACH_CALLS__?: Array<{ segments: PlanSegmentLite[] }>;
        }
      ).__MOCK_GAS_REACH_CALLS__ ?? [];
    return calls.length ? calls[calls.length - 1]!.segments : [];
  });
}

/** The roles of the cylinders the LAST compute_gas_reach_field call carried. */
export async function lastGasReachCylinderRoles(page: Page): Promise<string[]> {
  return page.evaluate(() => {
    const calls =
      (window as unknown as { __MOCK_GAS_REACH_CALLS__?: Array<{ cylinders?: Array<{ role: string }> }> })
        .__MOCK_GAS_REACH_CALLS__ ?? [];
    return calls.length ? (calls[calls.length - 1]!.cylinders ?? []).map((c) => c.role) : [];
  });
}

/** The `dpv` field the LAST compute_gas_reach_field call's `gasConfig` carried (#317) - proves a
 *  rig toggle reached the actual REQUEST, not just the client-side `hasDpvRig` flag the readout
 *  also gates on (that flag flips a render tick before any recompute round-trips). */
export async function lastGasReachDpv(page: Page): Promise<unknown> {
  return page.evaluate(() => {
    const calls =
      (
        window as unknown as {
          __MOCK_GAS_REACH_CALLS__?: Array<{ dpv: unknown }>;
        }
      ).__MOCK_GAS_REACH_CALLS__ ?? [];
    return calls.length ? calls[calls.length - 1]!.dpv : undefined;
  });
}

/**
 * What the gas engine answers on top of the mock's canned field, from now on (#576 E3); null
 * goes back to the canned field. The values are the backend's to compute - a spec seeds the
 * answer it needs, and asserts the panel renders it.
 */
export async function seedGasField(page: Page, patch: Partial<GasReachField> | null): Promise<void> {
  await page.evaluate((p) => {
    (window as unknown as { __MOCK_SET_GAS_FIELD_PATCH__: (patch: unknown) => void }).__MOCK_SET_GAS_FIELD_PATCH__(p);
  }, patch);
}

/** One stage drop at station 2 of the seed, 150 bar, 20 m in, not over its reserve. */
export const ONE_STAGE_DROP: Partial<GasReachField> = {
  stage_drop_refs: [{ section_id: 1, station_index: 2 }],
  stage_drop_bars: [150],
  stage_drop_over_reserve: [false],
  stage_drop_distances_m: [20],
  stage_drop_route_indices: [2],
};
