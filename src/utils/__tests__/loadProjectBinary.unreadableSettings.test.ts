/**
 * [#576 B15] The binary load envelope carries the stored project settings the
 * backend could not read, and an envelope from a backend that predates the field
 * reads as "none". `buildResult` is the pure half of the decoder, so this drives
 * it directly.
 */
import { describe, it, expect } from 'vitest';

import { buildResult, type BinaryEnvelope } from '../loadProjectBinary';

function envelope(unreadable?: string[]): BinaryEnvelope {
  return {
    version: 5,
    loading_id: 1,
    is_remote: false,
    ...(unreadable ? { unreadable_project_settings: unreadable } : {}),
    project_header: {
      name: 'CaveB',
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

describe('loadProjectBinary decodes the unread project settings (#576 B15)', () => {
  it('carries the list the backend sent', () => {
    expect(buildResult(envelope(['surface_elevation']), 0).unreadableProjectSettings).toEqual([
      'surface_elevation',
    ]);
  });

  it('reads an envelope without the field as none', () => {
    expect(buildResult(envelope(), 0).unreadableProjectSettings).toEqual([]);
  });
});
