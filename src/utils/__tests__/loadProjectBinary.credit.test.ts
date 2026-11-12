/**
 * Binary envelope v7: per-shot survey credit rides as one project-wide people
 * table, each section's distinct credits as indices into it, and one u32 index
 * per shot (empty when no shot is credited). Drives the real `buildResult`.
 */
import { describe, it, expect } from 'vitest';

import { buildResult, type BinaryEnvelope } from '../loadProjectBinary';
import { shotMaterialization } from '../../types/survey-data';

const SHOT_FLOAT_FIELDS = 12;
const SHOT_UINT_FIELDS = 3;
const NONE = 0xffffffff;

function envelope(creditIdx: number[] | null): BinaryEnvelope {
  const n = 3;
  const shotsF32 = new Float32Array(n * SHOT_FLOAT_FIELDS);
  const shotsU32 = new Uint32Array(n * SHOT_UINT_FIELDS);
  for (let i = 0; i < n; i++) {
    shotsF32[i * SHOT_FLOAT_FIELDS + 2] = 1; // length
    shotsU32[i * SHOT_UINT_FIELDS] = 1;
    shotsU32[i * SHOT_UINT_FIELDS + 1] = 100_000 + i;
    shotsU32[i * SHOT_UINT_FIELDS + 2] = NONE;
  }
  const u8 = (a: Float32Array | Uint32Array): Uint8Array => new Uint8Array(a.buffer, a.byteOffset, a.byteLength);
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
      next_section_id: 2,
      next_closure_id: 1,
      sections: [
        {
          id: 1,
          name: 'S',
          color: '#ff0000',
          direction: 'in',
          quality: 1.0,
          shot_offset: 0,
          shot_count: n,
          next_shot_id: 100_000 + n,
          credits: [
            { surveyors: [0, 1], explorers: [2], date: '2024-02-03' },
            // An index past the people table is dropped, not a crash.
            { surveyors: [1, 9], explorers: [], date: null },
          ],
        },
      ] as unknown as BinaryEnvelope['project_header']['sections'],
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
      credit_people: [{ name: 'Ann' }, { name: 'Bo', email: 'bo@example.org' }, { name: 'Ex' }],
    },
    shot_count: n,
    shots_floats: u8(shotsF32),
    shots_uints: u8(shotsU32),
    shots_flags: new Uint8Array(n),
    shot_comments: [],
    shots_credit: creditIdx === null ? new Uint8Array(0) : u8(Uint32Array.from(creditIdx)),
    station_count: 0,
    stations_floats: new Uint8Array(0),
    stations_uints: new Uint8Array(0),
    stations_flags: new Uint8Array(0),
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

describe('loadProjectBinary decodes per-shot credit (v7)', () => {
  it('rebuilds each shot credit from the people table, absent for none', () => {
    const { project } = buildResult(envelope([0, NONE, 1]), 0);
    const shots = project.sections[0]!.shots;
    expect(shots[0]!.credit).toEqual({
      surveyors: [{ name: 'Ann' }, { name: 'Bo', email: 'bo@example.org' }],
      explorers: [{ name: 'Ex' }],
      date: '2024-02-03',
    });
    expect('credit' in shots[1]!).toBe(false);
    expect(shots[2]!.credit).toEqual({ surveyors: [{ name: 'Bo', email: 'bo@example.org' }], explorers: [], date: null });
  });

  it('shares one credit object per distinct section credit', () => {
    const { project } = buildResult(envelope([0, 0, 0]), 0);
    const shots = project.sections[0]!.shots;
    expect(shots[0]!.credit).toBe(shots[2]!.credit);
  });

  it('reads an empty buffer as an uncredited project, without materializing shots', () => {
    const before = shotMaterialization.count;
    const { project } = buildResult(envelope(null), 0);
    expect(shotMaterialization.count).toBe(before);
    expect(project.sections[0]!.shots.every((s) => s.credit === undefined)).toBe(true);
  });

  it('refuses a credit buffer of the wrong length', () => {
    expect(() => buildResult(envelope([0, 0]), 0)).toThrow(/shots_credit length mismatch/);
  });
});
