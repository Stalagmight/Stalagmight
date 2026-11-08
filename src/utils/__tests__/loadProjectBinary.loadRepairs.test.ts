/**
 * [#576 B35(h)] The binary load envelope carries what the backend reset or dropped
 * on open (`load_repairs`, per kind). An envelope without it, or with nothing
 * reset, reads as `null` so no notice shows. `buildResult` is the pure half of the
 * decoder, so this drives it directly.
 */
import { describe, it, expect } from 'vitest';

import { buildResult, type BinaryEnvelope } from '../loadProjectBinary';

function envelope(loadRepairs?: BinaryEnvelope['load_repairs']): BinaryEnvelope {
  return {
    version: 5,
    loading_id: 1,
    is_remote: false,
    ...(loadRepairs ? { load_repairs: loadRepairs } : {}),
    project_header: {
      name: 'CaveA',
      location: null,
      created_at: '2026-01-01T00:00:00Z',
      modified_at: '2026-01-01T00:00:00Z',
      file_path: null,
      next_section_id: 1,
      next_closure_id: 1,
      sections: [],
      station_merges: [],
      zero_datums: [],
      loop_closures: [],
      section_offsets: [],
      station_offsets: [],
      station_comments_map: [],
      imported_file_hashes: [],
      imported_section_hashes: [],
      validation: {} as never,
      surface_elevation: null,
    },
    shot_count: 0,
    shots_floats: new Uint8Array(0),
    shots_uints: new Uint8Array(0),
    shots_flags: new Uint8Array(0),
    shot_comments: [],
    station_count: 0,
    stations_floats: new Uint8Array(0),
    stations_uints: new Uint8Array(0),
    stations_flags: new Uint8Array(0),
    station_gps: [],
    station_merged_with: [],
    station_closures_with: [],
    station_comments: [],
    stats: {} as never,
    warnings: [],
    display_settings: {} as never,
    frozen_shots: [],
  } as unknown as BinaryEnvelope;
}

describe('loadProjectBinary decodes what the load reset or dropped (#576 B35(h))', () => {
  it('carries the per-kind counts the backend sent', () => {
    const result = buildResult(
      envelope({ shots: 2, section_offsets: 0, station_offsets: 1, gps_points: 3 }),
      0,
    );
    expect(result.loadRepairs).toEqual({
      shots: 2,
      sectionOffsets: 0,
      stationOffsets: 1,
      gpsPoints: 3,
      stationComments: 0,
    });
  });

  // [#576 Q-W30-3-D1] Orphaned station comments the open dropped.
  it('carries the station comments the open dropped as orphans', () => {
    const result = buildResult(
      envelope({ shots: 0, section_offsets: 0, station_offsets: 0, gps_points: 0, station_comments: 2n }),
      0,
    );
    expect(result.loadRepairs).toEqual({
      shots: 0,
      sectionOffsets: 0,
      stationOffsets: 0,
      gpsPoints: 0,
      stationComments: 2,
    });
  });

  it('reads msgpack u64 counts that arrive as bigint', () => {
    const result = buildResult(
      envelope({ shots: 0n, section_offsets: 1n, station_offsets: 0n, gps_points: 0n }),
      0,
    );
    expect(result.loadRepairs?.sectionOffsets).toBe(1);
  });

  it('reads nothing reset, or an envelope that predates the field, as null', () => {
    expect(
      buildResult(envelope({ shots: 0, section_offsets: 0, station_offsets: 0, gps_points: 0 }), 0)
        .loadRepairs,
    ).toBeNull();
    expect(buildResult(envelope(), 0).loadRepairs).toBeNull();
  });
});
