/**
 * #707 - the decoder attaches a sparse `shot_credits` entry to the shot at that
 * GLOBAL index, so a credit on the second shot of the second section lands at
 * index 3, and every unlisted shot has no `credit` property at all.
 */
import { describe, it, expect } from 'vitest';

import { buildResult, type BinaryEnvelope } from '../loadProjectBinary';

const SHOT_FLOAT_FIELDS = 12;
const SHOT_UINT_FIELDS = 3;
const STATION_FLOAT_FIELDS = 3;
const STATION_UINT_FIELDS = 2;
const MARKER_NULL_WIRE = 0xffffffff;

const SHOT_COUNT = 4;
const STATION_COUNT = 6;

function makeEnvelope(): BinaryEnvelope {
  const shotsF32 = new Float32Array(SHOT_COUNT * SHOT_FLOAT_FIELDS);
  const shotsU32 = new Uint32Array(SHOT_COUNT * SHOT_UINT_FIELDS);
  for (let g = 0; g < SHOT_COUNT; g++) {
    const sectionId = g < 2 ? 1 : 2;
    shotsF32[g * SHOT_FLOAT_FIELDS + 2] = g + 1;
    shotsU32[g * SHOT_UINT_FIELDS] = sectionId;
    shotsU32[g * SHOT_UINT_FIELDS + 1] = sectionId * 100_000 + (g % 2);
    shotsU32[g * SHOT_UINT_FIELDS + 2] = MARKER_NULL_WIRE;
  }
  const stationsF64 = new Float64Array(STATION_COUNT * STATION_FLOAT_FIELDS);
  const stationsU32 = new Uint32Array(STATION_COUNT * STATION_UINT_FIELDS);
  for (let i = 0; i < STATION_COUNT; i++) {
    stationsU32[i * STATION_UINT_FIELDS] = i < 3 ? 1 : 2;
    stationsU32[i * STATION_UINT_FIELDS + 1] = i % 3;
  }
  const u8 = (a: Float32Array | Float64Array | Uint32Array): Uint8Array =>
    new Uint8Array(a.buffer, a.byteOffset, a.byteLength);
  const section = (id: number, offset: number) => ({
    id,
    name: `S${id}`,
    surveyor: null,
    date: null,
    color: '#0000ff',
    direction: 'in',
    quality: 1.0,
    shot_offset: offset,
    shot_count: 2,
    next_shot_id: id * 100_000 + 2,
  });

  return {
    version: 7,
    loading_id: 1,
    is_remote: false,
    project_header: {
      name: 'credit',
      location: null,
      created_at: '2026-01-01T00:00:00Z',
      modified_at: '2026-01-01T00:00:00Z',
      file_path: null,
      next_section_id: 3,
      next_closure_id: 1,
      sections: [section(1, 0), section(2, 2)] as unknown as BinaryEnvelope['project_header']['sections'],
      station_merges: [],
      zero_datums: [],
      loop_closures: [],
      section_offsets: [],
      station_offsets: [],
      station_comments_map: [],
      imported_file_hashes: [],
      imported_section_hashes: [],
      validation: {} as BinaryEnvelope['project_header']['validation'],
      surface_elevation: null,
    },
    shot_count: SHOT_COUNT,
    shots_floats: u8(shotsF32),
    shots_uints: u8(shotsU32),
    shots_flags: new Uint8Array(SHOT_COUNT),
    shot_comments: [],
    shot_credits: [[3, { surveyor: 'Ann', date: null }]],
    station_count: STATION_COUNT,
    stations_floats: u8(stationsF64),
    stations_uints: u8(stationsU32),
    stations_flags: new Uint8Array(STATION_COUNT),
    station_gps: [],
    station_merged_with: [],
    station_closures_with: [],
    station_comments: [],
    station_names: [],
    stats: {} as BinaryEnvelope['stats'],
    warnings: [],
    display_settings: {} as BinaryEnvelope['display_settings'],
    frozen_shots: [],
  };
}

describe('loadProjectBinary shot credit (#707)', () => {
  it('attaches the credit at its global shot index and nowhere else', () => {
    const { project } = buildResult(makeEnvelope(), 0);
    const [first, second] = project.sections;
    expect(second!.shots[1]!.credit).toEqual({ surveyor: 'Ann', date: null });
    for (const shot of [...first!.shots, second!.shots[0]!]) {
      expect('credit' in shot).toBe(false);
    }
  });
});
