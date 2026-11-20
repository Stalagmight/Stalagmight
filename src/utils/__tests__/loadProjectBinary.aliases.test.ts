/**
 * H-frontend-wire-5: the binary load path carries a merged station's aliases
 * (the Station IDs and names merged into it) on every slot of it, as the JSON
 * refresh does, so "Also known as" shows from the first open.
 */
import { describe, it, expect } from 'vitest';

import { buildResult, type BinaryEnvelope } from '../loadProjectBinary';

/** One line of two stations with no shots: the station extras are the subject. */
function envelope(aliases?: Array<[number, Array<{ id: number; name: string | null }>]>): BinaryEnvelope {
  const floats = new Float64Array(6);
  const uints = new Uint32Array([1, 0, 1, 1]);
  const ids = new Uint32Array([5, 6]);
  return {
    version: 9,
    loading_id: 1,
    is_remote: false,
    project_header: {
      name: 'aliases', location: null, created_at: '2026-01-01T00:00:00Z', modified_at: '2026-01-01T00:00:00Z',
      file_path: null, next_line_id: 2, next_closure_id: 1,
      lines: [{ id: 1, name: 'Main', color: '#ff0000', direction: 'in', quality: 1, shot_offset: 0, shot_count: 0, next_shot_id: 0, credits: [] }],
      station_merges: [], zero_datums: [], loop_closures: [], station_offsets: [], station_comments_map: [],
      imported_file_hashes: [], imported_line_hashes: [], validation: {} as never, surface_elevation: null, credit_people: [],
    },
    shot_count: 0,
    shots_floats: new Uint8Array(0),
    shots_uints: new Uint8Array(0),
    shots_flags: new Uint8Array(0),
    shot_comments: [],
    shots_credit: new Uint8Array(0),
    station_count: 2,
    stations_floats: new Uint8Array(floats.buffer),
    stations_uints: new Uint8Array(uints.buffer),
    stations_flags: new Uint8Array(2),
    station_gps: [],
    station_merged_with: [],
    station_closures_with: [],
    station_comments: [],
    station_names: [[0, '0'], [1, '1']],
    stations_ids: new Uint8Array(ids.buffer),
    ...(aliases ? { station_aliases: aliases } : {}),
    stats: {} as never,
    warnings: [],
    display_settings: {} as never,
    frozen_shots: [],
  } as unknown as BinaryEnvelope;
}

describe('loadProjectBinary decodes station aliases', () => {
  it('puts them on the station they belong to, and nowhere else', () => {
    const result = buildResult(envelope([[1, [{ id: 40, name: 'B3' }]]]), 0);
    expect(result.stations[1]!.aliases).toEqual([{ id: 40, name: 'B3' }]);
    expect(result.stations[0]!.aliases).toBeUndefined();
    expect(result.surveyData.stationAliases.get(1)).toEqual([{ id: 40, name: 'B3' }]);
  });

  it('an envelope without the key decodes as no aliases', () => {
    const result = buildResult(envelope(), 0);
    expect(result.stations[1]!.aliases).toBeUndefined();
    expect(result.surveyData.stationAliases.size).toBe(0);
  });
});
