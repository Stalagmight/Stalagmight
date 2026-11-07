import type { CommandError } from '../../../src/types/ipc/CommandError';
import type { MockCommandFamily, MockReply } from './types';

/**
 * Plan-mode commands of the Playwright IPC mock: dive-plan persistence (#244), the
 * plan anchors that keep a pin on its station across survey edits (#310), and the
 * gas-weighted reach field (`compute_gas_reach_field`).
 *
 * Split out of `../tauri-ipc.ts` unchanged (#576 E8). Runs IN THE PAGE: `setupTauriMocks`
 * serializes this factory with `Function.prototype.toString` into its init script, so it
 * may use only `ctx` and browser globals - never an import (type-only imports are erased).
 */
export const gasPlanCommands: MockCommandFamily = (ctx) => {
  // #310: live plan pins are kept as opaque ANCHORS beside their positional refs, so a survey
  // edit that renumbers stations does not silently retarget them. [#576 E3] Canned: the mock's
  // anchor is the ref it was taken from, and re-resolving one answers that same ref - unless
  // the spec has said what the backend answers for it after an edit
  // (`__MOCK_SET_ANCHOR_RESOLUTION__({ 'sectionId:stationIndex': ref | null })`). How the
  // backend names a station by its neighbouring shots and re-places it across a delete, an
  // insert or a reverse (`PlanAnchor` / `resolve_anchor`, core dive_plan.rs) is pinned by its
  // dive_plan tests, never here. The anchor shape is this mock's own and opaque to the app.
  interface MockPlanRefLite {
    section_id: number;
    station_index: number;
  }
  interface MockPlanAnchor {
    mock_anchor_of: MockPlanRefLite;
  }
  let anchorResolution: Record<string, MockPlanRefLite | null> = {};
  (window as unknown as Record<string, unknown>).__MOCK_SET_ANCHOR_RESOLUTION__ = (
    resolution: Record<string, MockPlanRefLite | null>,
  ) => {
    anchorResolution = { ...resolution };
  };
  const refToAnchor = (ref: MockPlanRefLite | null | undefined): MockPlanAnchor | null =>
    ref ? { mock_anchor_of: { section_id: ref.section_id, station_index: ref.station_index } } : null;
  const anchorToRef = (anchor: MockPlanAnchor | null | undefined): MockPlanRefLite | null => {
    const ref = anchor?.mock_anchor_of;
    if (!ref) return null;
    const key = `${ref.section_id}:${ref.station_index}`;
    return Object.prototype.hasOwnProperty.call(anchorResolution, key) ? anchorResolution[key]! : ref;
  };

  // Dive-plan (.plnx) mock store (#244): an in-memory list of saved plans for the
  // open project. Reset per test via __MOCK_RESET_DIVE_PLANS__.
  let mockDivePlans: Array<{
    id: number;
    name: string;
    created_at: string;
    updated_at: string;
    snapshot: unknown;
  }> = [];
  let mockDivePlanNextId = 1;
  let mockDivePlanClock = 0;
  const nextDivePlanStamp = () => {
    mockDivePlanClock += 1;
    return `2026-07-06T10:00:${String(mockDivePlanClock).padStart(2, '0')}Z`;
  };

  // [#576 E3] What a spec says the gas engine answers on top of the canned field (see
  // `compute_gas_reach_field`); null clears it.
  let gasFieldPatch: Partial<MockReply<'compute_gas_reach_field'> & object> = {};
  (window as unknown as Record<string, unknown>).__MOCK_SET_GAS_FIELD_PATCH__ = (
    patch: Partial<MockReply<'compute_gas_reach_field'> & object> | null,
  ) => {
    gasFieldPatch = patch ?? {};
  };

  (window as unknown as Record<string, unknown>).__MOCK_RESET_DIVE_PLANS__ = () => {
    mockDivePlans = [];
    mockDivePlanNextId = 1;
    mockDivePlanClock = 0;
  };
  (window as unknown as Record<string, unknown>).__MOCK_GET_DIVE_PLANS__ = () => [...mockDivePlans];

  return (cmd, args) => {
    switch (cmd) {
      // Dive-plan persistence (#244). An in-memory mirror of the .plnx store.
      case 'list_dive_plans':
        return mockDivePlans.map((p) => ({
          id: p.id,
          name: p.name,
          created_at: p.created_at,
          updated_at: p.updated_at,
        })) satisfies MockReply<'list_dive_plans'>;
      case 'load_dive_plan': {
        const found = mockDivePlans.find((p) => p.id === args?.id);
        if (!found) return Promise.reject({ code: 'FAILED', message: 'That dive plan no longer exists.' } satisfies CommandError);
        // #310: this mock keeps a plan's refs POSITIONAL (unlike the real `.plnx`, which
        // canonicalizes to shot-uuid anchors on save); a load hands back the canned anchor of
        // each stored ref.
        const snap = (found.snapshot ?? {}) as {
          origin?: MockPlanRefLite | null;
          endpoint?: MockPlanRefLite | null;
          waypoints?: MockPlanRefLite[];
          marked_points?: Array<{ station: MockPlanRefLite; label?: string | null }>;
          stage_drop_overrides?: Array<MockPlanRefLite | null>;
        };
        return {
          meta: {
            id: found.id,
            name: found.name,
            created_at: found.created_at,
            updated_at: found.updated_at,
          },
          // shape not checked: the DivePlanSnapshot the app last sent to save_dive_plan, stored as unknown
          snapshot: found.snapshot as MockReply<'load_dive_plan'>['snapshot'],
          stale_ref_count: 0,
          anchors: {
            origin: refToAnchor(snap.origin ?? null),
            endpoint: refToAnchor(snap.endpoint ?? null),
            waypoints: (snap.waypoints ?? []).map((w) => refToAnchor(w)),
            marked_points: (snap.marked_points ?? []).map((m) => refToAnchor(m.station)),
            stage_drop_overrides: (snap.stage_drop_overrides ?? []).map((r) =>
              refToAnchor(r ?? null),
            ),
          },
        } satisfies MockReply<'load_dive_plan'>;
      }
      case 'save_dive_plan': {
        const id = (args?.id ?? null) as number | null;
        const name = args?.name as string;
        const snapshot = args?.snapshot;
        const stamp = nextDivePlanStamp();
        if (id === null) {
          const plan = {
            id: mockDivePlanNextId,
            name,
            created_at: stamp,
            updated_at: stamp,
            snapshot,
          };
          mockDivePlanNextId += 1;
          mockDivePlans.push(plan);
          return {
            id: plan.id,
            name: plan.name,
            created_at: plan.created_at,
            updated_at: plan.updated_at,
          } satisfies MockReply<'save_dive_plan'>;
        }
        const existing = mockDivePlans.find((p) => p.id === id);
        if (!existing) return Promise.reject({ code: 'FAILED', message: 'That dive plan no longer exists.' } satisfies CommandError);
        existing.name = name;
        existing.updated_at = stamp;
        existing.snapshot = snapshot;
        return {
          id: existing.id,
          name: existing.name,
          created_at: existing.created_at,
          updated_at: existing.updated_at,
        } satisfies MockReply<'save_dive_plan'>;
      }
      case 'delete_dive_plan': {
        const before = mockDivePlans.length;
        mockDivePlans = mockDivePlans.filter((p) => p.id !== args?.id);
        if (mockDivePlans.length === before) {
          return Promise.reject({ code: 'FAILED', message: 'That dive plan no longer exists.' } satisfies CommandError);
        }
        return null satisfies MockReply<'delete_dive_plan'>;
      }

      // #310: anchor / re-resolve live plan pins across a survey edit. Recorded together in
      // one call log (both commands round-trip the same opaque anchor shape).
      case 'plan_anchors_for_refs': {
        const refs = (args?.refs ?? []) as Array<MockPlanRefLite | null>;
        const anchors = refs.map((ref) => refToAnchor(ref));
        const win = window as unknown as Record<string, unknown>;
        const calls = (win.__MOCK_ANCHOR_CALLS__ as unknown[]) ?? [];
        calls.push({ cmd: 'plan_anchors_for_refs', refs, anchors });
        win.__MOCK_ANCHOR_CALLS__ = calls;
        return anchors satisfies MockReply<'plan_anchors_for_refs'>;
      }
      case 'plan_refs_for_anchors': {
        const anchors = (args?.anchors ?? []) as Array<MockPlanAnchor | null>;
        const refs = anchors.map((a) => anchorToRef(a));
        const win = window as unknown as Record<string, unknown>;
        const calls = (win.__MOCK_ANCHOR_CALLS__ as unknown[]) ?? [];
        calls.push({ cmd: 'plan_refs_for_anchors', anchors, refs });
        win.__MOCK_ANCHOR_CALLS__ = calls;
        return refs satisfies MockReply<'plan_refs_for_anchors'>;
      }
      case 'compute_gas_reach_field': {
        // Record every call's inputs so a spec can assert what the plan sent: the drop
        // overrides (#246), the endpoint and origin, the ordered way points (#298), the
        // per-span speed + propulsion (#317) and the rig itself (#317), so disabling it can be
        // proven at the REQUEST. Keyed on the camelCase args the frontend invokes with.
        {
          const win = window as unknown as Record<string, unknown>;
          const calls = (win.__MOCK_GAS_REACH_CALLS__ as unknown[]) ?? [];
          calls.push({
            stageDropOverrides: args?.stageDropOverrides ?? [],
            endpoint: args?.endpoint ?? null,
            origin: args?.origin ?? null,
            waypoints: args?.waypoints ?? [],
            segments: args?.segments ?? [],
            cylinders: args?.cylinders ?? [],
            dpv: (args?.gasConfig as { dpv?: unknown } | undefined)?.dpv ?? null,
          });
          win.__MOCK_GAS_REACH_CALLS__ = calls;
        }
        // [#576 E3] Canned. The field is a fixed placeholder over the open project's stations,
        // whatever the plan sent: station i of a section (the mock's placeholder x = 10 i)
        // costs 200 i litres against a 1500 L budget, the last station of the list is
        // unconnected (null), and the route runs over the reachable stations to the last of
        // them. Nothing below re-runs the engine: the reserve strategies and their fallback,
        // the per-cylinder gauge rule, the stage drops and their over-reserve flag, the route
        // metrics, the out-and-back profile, the segment speeds and the scooter range are the
        // backend's (models project/gas tests, scooter.rs tests, pinned for #576 E3). A spec
        // that needs the backend to answer something else - a stage drop, an over-reserve
        // flag, a scooter range or park point - seeds it with
        // `__MOCK_SET_GAS_FIELD_PATCH__({ ... })`, laid over this field on every reply.
        const state = ctx.invoke('get_project_state', args) as
          | {
              stations?: Array<{
                station_ref: { section_id: number; station_index: number };
                coordinates: { x: number; y: number; z: number };
              }>;
            }
          | null;
        const stations = state?.stations ?? [];
        const refs = stations.map((s) => s.station_ref);
        const budget_litres = 1500;
        const gas_to_reach: (number | null)[] = stations.map((s, k) =>
          k === stations.length - 1 ? null : Math.abs(s.coordinates.x) * 20,
        );
        const reachable = gas_to_reach.map((g) => g !== null && g <= budget_litres);
        const reachableRefs = refs.filter((_r, k) => reachable[k]);
        const target = reachableRefs[reachableRefs.length - 1] ?? null;
        const route = reachableRefs
          .slice(0, -1)
          .map((from, i) => ({ from, to: reachableRefs[i + 1]!, is_jump: false }));
        const legs = reachableRefs.length;
        const routeIndexOf = (r: { section_id: number; station_index: number }) =>
          reachableRefs.findIndex(
            (x) => x.section_id === r.section_id && x.station_index === r.station_index,
          );
        const waypointArgs = (args?.waypoints ?? []) as Array<{ section_id: number; station_index: number }>;
        const outbound = reachableRefs.map((station_ref, i) => ({
          route_index: i,
          station_ref,
          distance_m: 10 * i,
          depth_m: 0,
          gas_l: 200 * i,
          elapsed_min: i,
          outbound: true,
        }));
        const back = outbound
          .slice(0, -1)
          .reverse()
          .map((point, j) => ({
            ...point,
            distance_m: 10 * (legs - 1) + 10 * (j + 1),
            gas_l: 200 * (legs - 1) + 100 * (j + 1),
            elapsed_min: legs - 1 + (j + 1),
            outbound: false,
          }));
        const field = {
          station_refs: refs,
          gas_to_reach,
          reachable,
          turn_frontier: target ? [target] : [],
          max_reachable_gas: legs > 0 ? 200 * (legs - 1) : 0,
          budget_litres,
          route,
          stage_drop_refs: [],
          final_turn_ref: target,
          turn_toward_endpoint: false,
          backgas_turn_bar: 140,
          stage_drop_bars: [],
          stage_drop_over_reserve: [],
          stage_drop_distances_m: [],
          cylinder_usable_bars: ((args?.cylinders ?? []) as unknown[]).map(() => 0),
          route_line_length_m: target ? 10 * (legs - 1) : 0,
          route_max_depth_m: target ? 0 : null,
          route_avg_depth_m: target ? 0 : null,
          waypoint_gas_litres: waypointArgs.map((w) => {
            const k = refs.findIndex((r) => r.section_id === w.section_id && r.station_index === w.station_index);
            return k >= 0 ? gas_to_reach[k]! : null;
          }),
          route_profile: [...outbound, ...back],
          stage_drop_route_indices: [],
          waypoint_route_indices: waypointArgs.map((w) => {
            const j = routeIndexOf(w);
            return j >= 0 ? j : null;
          }),
          scooter_distance_m: 0,
          scooter_range_limit_m: null,
          scooter_drop_ref: null,
          route_runtime_min: target ? legs - 1 : null,
          turn_on_scooter_range: false,
        } satisfies MockReply<'compute_gas_reach_field'>;
        return { ...field, ...gasFieldPatch } satisfies MockReply<'compute_gas_reach_field'>;
      }

      default:
        return ctx.unhandled;
    }
  };
};
