import type { MockCommandFamily, MockReply } from './types';

/**
 * The read-only survey queries of the Playwright IPC mock: the closure-adjusted stations,
 * the distance-gradient field and the ruler's along-line distance, each derived from the
 * core's project (`ctx.invoke('get_project_state')`, `ctx.currentProject()`).
 *
 * Split out of `../tauri-ipc.ts` unchanged (#576 AFK-D3). Runs IN THE PAGE: `setupTauriMocks`
 * serializes this factory with `Function.prototype.toString` into its init script, so it
 * may use only `ctx` and browser globals - never an import (type-only imports are erased).
 */
export const surveyQueryCommands: MockCommandFamily = (ctx) => {
  return (cmd, args) => {
    switch (cmd) {
      case 'get_adjusted_stations': {
        // Closure-adjusted geometry view. The mock has no real least-squares
        // solver, so the adjusted geometry is identical to measured: reuse the
        // station array get_project_state builds.
        // shape not checked: mockHandler returns unknown; the get_project_state case checks this value
        const state = ctx.invoke('get_project_state', args) as MockReply<'get_project_state'>;
        return (state?.stations ?? []) satisfies MockReply<'get_adjusted_stations'>;
      }

      case 'get_station_distances_from': {
        // Distance-gradient field (#34). No real graph in the mock - synthesize
        // distances from each station's mock x-coordinate (origin at x=0) so the
        // overlay + legend have finite values to render. `from` is ignored.
        const state = ctx.invoke('get_project_state', args) as
          | {
              stations?: Array<{
                station_ref: { section_id: number; station_index: number };
                coordinates: { x: number; y: number; z: number };
              }>;
            }
          | null;
        const stations = state?.stations ?? [];
        let max = 0;
        const distances = stations.map((s) => {
          const d = Math.abs(s.coordinates.x);
          if (d > max) max = d;
          return d;
        });
        return {
          distances,
          station_refs: stations.map((s) => s.station_ref),
          max_reachable: max,
        } satisfies MockReply<'get_station_distances_from'>;
      }

      case 'get_along_line_distance': {
        // Ruler along-line distance (#26). No real survey graph in the mock -
        // return a deterministic distance and tint the first couple of shots
        // of endpoint `a`'s section so the readout + tint have something to show.
        const endpointA = (args?.a ?? null) as { section_id?: number } | null;
        const sections = (ctx.currentProject()?.sections ?? []) as Array<{ id: number; shots: Array<{ id: number }> }>;
        const sec = sections.find((s) => s.id === endpointA?.section_id) ?? sections[0];
        const path_shots = sec
          ? (sec.shots ?? []).slice(0, 2).map((sh) => ({ section_id: sec.id, shot_id: sh.id }))
          : [];
        // Stations bounding those shots (one more than the shot count).
        const path_stations = sec
          ? Array.from({ length: path_shots.length + 1 }, (_, i) => ({ section_id: sec.id, station_index: i }))
          : [];
        return { along_line: 12.34, path_shots, path_stations } satisfies MockReply<'get_along_line_distance'>;
      }

      default:
        return ctx.unhandled;
    }
  };
};
