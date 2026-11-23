/**
 * H-frontend-wire-5, RS 6: the binary load path carries a node's records
 * (the stations merges archived in it) on every slot of it, and the shown
 * slot of a foreign slot or a loop end, as the JSON refresh does, so the
 * details and the dots show from the first open.
 */
import { describe, it, expect } from 'vitest';

import { buildResult, type BinaryEnvelope } from '../loadProjectBinary';

/** One line of two stations with no shots: the station extras are the subject. */
function envelope(
  archived?: Array<[number, Array<{ id: number; name: string; lines: number[] }>]>,
  shown?: Array<[number, { id: number; home_line?: number; foreign: boolean; loop_end: boolean }]>,
): BinaryEnvelope {
  const floats = new Float64Array(6);
  const uints = new Uint32Array([1, 0, 1, 1]);
  const ids = new Uint32Array([5, 6]);
  return {
    version: 9,
    loading_id: 1,
    is_remote: false,
    project_header: {
      name: 'records', location: null, created_at: '2026-01-01T00:00:00Z', modified_at: '2026-01-01T00:00:00Z',
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
    ...(archived ? { station_archived: archived } : {}),
    ...(shown ? { station_shown: shown } : {}),
    stats: {} as never,
    warnings: [],
    display_settings: {} as never,
    frozen_shots: [],
  } as unknown as BinaryEnvelope;
}

describe('loadProjectBinary decodes records and shown slots', () => {
  it('puts them on the station they belong to, and nowhere else', () => {
    const records = [{ id: 40, name: 'B3', lines: [1] }];
    const shown = { id: 5, home_line: 1, foreign: true, loop_end: true };
    const result = buildResult(envelope([[1, records]], [[1, shown]]), 0);
    expect(result.stations[1]!.archived).toEqual(records);
    expect(result.stations[1]!.shown).toEqual(shown);
    expect(result.stations[0]!.archived).toBeUndefined();
    expect(result.stations[0]!.shown).toBeUndefined();
    expect(result.surveyData.stationArchived.get(1)).toEqual(records);
    expect(result.surveyData.stationShown.get(1)).toEqual(shown);
  });

  it('an envelope without the keys decodes as none', () => {
    const result = buildResult(envelope(), 0);
    expect(result.stations[1]!.archived).toBeUndefined();
    expect(result.surveyData.stationArchived.size).toBe(0);
    expect(result.surveyData.stationShown.size).toBe(0);
  });
});
