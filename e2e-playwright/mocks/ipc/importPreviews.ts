import type { TmlFile } from '../../../src/types/ipc/TmlFile';
import type { TmlShot } from '../../../src/types/ipc/TmlShot';
import type { MockCommandFamily, MockReply } from './types';

/**
 * The GPX and Ariane TML file previews of the Playwright IPC mock (`parse_gpx`, `parse_tml`),
 * with the synthetic files they serve. Their import halves (`execute_gpx_import`,
 * `execute_tml_import`) land sections in the core's open project, so they stay in the core.
 *
 * Split out of `../tauri-ipc.ts` unchanged (#576 AFK-D3). Runs IN THE PAGE: `setupTauriMocks`
 * serializes this factory with `Function.prototype.toString` into its init script, so it
 * may use only `ctx` and browser globals - never an import (type-only imports are erased).
 */
export const importPreviewCommands: MockCommandFamily = (ctx) => {
  const buildMockGpxFile = (path: string | null | undefined) => {
    const normalized = (path ?? '').toLowerCase();
    const now = new Date();
    const iso = now.toISOString();

    const waypointOnlyFile = {
      name: 'Waypoint Check Dive',
      waypoints: [
        { name: 'Entry Bell', comment: 'Main entry', latitude: 24.55, longitude: -81.78, elevation: 1.2, time: iso },
        { name: 'Jump Reel', comment: null, latitude: 24.5504, longitude: -81.7794, elevation: 3.4, time: iso },
      ],
      tracks: [],
      routes: [],
    };

    if (normalized.includes('waypoints-only')) {
      return waypointOnlyFile;
    }

    return {
      name: 'Reef Traverse',
      waypoints: waypointOnlyFile.waypoints,
      tracks: [
        {
          name: 'Main Line',
          comment: 'Primary track',
          segments: [
            {
              points: [
                { latitude: 24.55, longitude: -81.78, elevation: 2.0, time: iso },
                { latitude: 24.5502, longitude: -81.7798, elevation: 2.6, time: iso },
                { latitude: 24.55045, longitude: -81.7795, elevation: 3.1, time: iso },
              ],
            },
          ],
        },
      ],
      routes: [
        {
          name: 'Exit Route',
          comment: null,
          points: [
            { name: 'Route Start', latitude: 24.55045, longitude: -81.7795, elevation: 3.1, time: iso },
            { name: 'Route Mid', latitude: 24.55065, longitude: -81.7792, elevation: 2.8, time: iso },
            { name: 'Route End', latitude: 24.5508, longitude: -81.7789, elevation: 2.5, time: iso },
          ],
        },
      ],
    };
  };

  // Synthetic Ariane TML preview (#411). Fully in-memory - no .tml fixture
  // is read - carrying one of each shot flavour the import dialog's optional
  // controls key off: an excluded shot, a commented shot and a virtual shot.
  const buildMockTmlShot = (
    id: number,
    fromId: number,
    section: string,
    depth: number,
    overrides: Partial<TmlShot> = {}
  ): TmlShot => ({
    id,
    from_id: fromId,
    shot_type: 'Real',
    azimuth: (id * 30) % 360,
    length: 10,
    depth,
    depth_in: depth - 1,
    inclination: 0,
    left: 1,
    right: 1,
    up: 1,
    down: 1,
    comment: null as string | null,
    explorer: 'Synthetic Surveyor',
    date: '2026-01-02',
    color: '#3b82f6',
    excluded: false,
    latitude: 0,
    longitude: 0,
    closure_to_id: 0,
    section,
    ...overrides,
  });

  // A path containing "imperial" yields a file whose unit is Imperial, so a spec can
  // drive the "converted from feet" branch of the dialog. [#576 E3] Canned: the same
  // shots either way. The real parser converts ft->m before it returns the preview and
  // extracts the closures from the Closure rows (`apply_unit_conversion`,
  // `extract_closures`; parsers tml tests `test_imperial_conversion`,
  // `test_closure_mapping`); the closure below is written out, not derived.
  const buildMockTmlFile = (path: string | null | undefined): TmlFile => {
    const imperial = (path ?? '').toLowerCase().includes('imperial');
    const sections: TmlFile['sections'] = [
      {
        name: 'Main Line',
        shots: [
          buildMockTmlShot(1, 0, 'Main Line', 2),
          buildMockTmlShot(2, 1, 'Main Line', 4, { comment: 'Restriction at the T' }),
          buildMockTmlShot(3, 2, 'Main Line', 6, { excluded: true }),
        ],
        surveyor: 'Synthetic Surveyor',
        date: '2026-01-02',
        color: '#3b82f6',
        is_branch_split: false,
        start_ariane_id: 0,
        end_ariane_id: 3,
        start_depth: 0,
      },
      {
        name: 'Side Passage',
        shots: [
          buildMockTmlShot(4, 1, 'Side Passage', 3),
          buildMockTmlShot(5, 4, 'Side Passage', 5, { shot_type: 'Virtual' }),
          // The loop closure, as the real format stores it: a Closure-type
          // ROW near the end of the file. from_id=1 (Main Line station 1)
          // and closure_to_id=4 (Side Passage station 1) keep the previous
          // round's by-construction exclusion-safety: neither endpoint is
          // the excluded shot (id 3, Main Line's terminal) nor preceded by
          // it, so import-space indices agree under BOTH states of the
          // excluded-shots toggle.
          buildMockTmlShot(6, 1, 'Side Passage', 0, {
            shot_type: 'Closure',
            closure_to_id: 4,
            length: 0,
          }),
        ],
        surveyor: 'Synthetic Surveyor',
        date: '2026-01-03',
        color: '#22c55e',
        is_branch_split: true,
        start_ariane_id: 1,
        end_ariane_id: 5,
        start_depth: 2,
      },
    ];
    return {
      cave_name: 'Synthetic Cavern',
      unit: imperial ? 'Imperial' : 'Metric',
      use_magnetic_azimuth: true,
      first_start_absolute_elevation: 0,
      geo_coding: null,
      sections,
      // The Closure row (Side Passage raw row 2) ties back to shot 4 (its raw row 0).
      closures: [{ from_section_index: 1, from_station_index: 2, to_section_index: 1, to_station_index: 0 }],
    };
  };

  return (cmd, args) => {
    switch (cmd) {
      case 'parse_gpx': {
        return buildMockGpxFile(args?.path as string | undefined) satisfies MockReply<'parse_gpx'>;
      }

      case 'parse_tml':
        return buildMockTmlFile(args?.path as string | undefined) satisfies MockReply<'parse_tml'>;

      default:
        return ctx.unhandled;
    }
  };
};
