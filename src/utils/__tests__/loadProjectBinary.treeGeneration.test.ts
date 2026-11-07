/**
 * [#576 W75-NF7] The binary load and refresh envelopes carry the open file's tree
 * generation (`tree_generation`), which the saves name so the backend can refuse one
 * made for a tree an in-place SpeleoDB swap replaced. A u64 on the wire, so the
 * decoder may get a bigint; absent from an envelope that predates it. `buildResult`
 * is the pure half of the decoder, so this drives it directly.
 */
import { describe, it, expect } from 'vitest';

import { buildResult, type BinaryEnvelope } from '../loadProjectBinary';

function envelope(treeGeneration?: number | bigint): BinaryEnvelope {
  return {
    version: 5,
    loading_id: 1,
    is_remote: false,
    ...(treeGeneration === undefined ? {} : { tree_generation: treeGeneration }),
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

describe('loadProjectBinary decodes the tree generation (#576 W75-NF7)', () => {
  it('carries the generation the backend sent', () => {
    expect(buildResult(envelope(3), 0).treeGeneration).toBe(3);
  });

  it('reads a msgpack u64 that arrives as bigint', () => {
    expect(buildResult(envelope(5n), 0).treeGeneration).toBe(5);
  });

  it('reads an envelope that predates the field as null', () => {
    expect(buildResult(envelope(), 0).treeGeneration).toBeNull();
  });
});
