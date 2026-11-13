/**
 * Load-path laziness regression (#mem SoA).
 *
 * The binary loader installs a per-section *lazy* `shots` getter so a freshly
 * loaded project holds only the struct-of-arrays typed buffers, not ~75k `Shot`
 * objects. The whole-project overlay/geometry walkers that run at load or when a
 * persisted display mode is restored (distance gradient, gas reachability,
 * closure-adjusted geometry) must therefore read the SoA arrays, never
 * `section.shots`. This test drives the real decode path (`buildResult`) and
 * asserts those walkers materialize *zero* sections.
 *
 * The guarding signal is `shotMaterialization.count` (also exposed to the E2E
 * harness as `window.__shotMaterializations`) plus the section descriptors: an
 * untouched section's `shots` is an accessor (`get` defined); reading it once
 * flips it to a data property. See utils/loadProjectBinary.ts.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { buildResult, type BinaryEnvelope } from '../loadProjectBinary';
import { buildDistanceFeatureState } from '../distanceGradient';
import { buildGasReachFeatureState } from '../gasReach';
import {
  shotMaterialization,
  surveyDataWithAdjustedStations,
  getStation,
} from '../../types/survey-data';
import type { DistanceField } from '../../types/project';
import type { GasReachField } from '../../types/ipc.generated';

const SHOT_FLOAT_FIELDS = 12;
const SHOT_UINT_FIELDS = 3;
const STATION_FLOAT_FIELDS = 3;
const STATION_UINT_FIELDS = 2;
const MARKER_NULL_WIRE = 0xffffffff;

/** Build a synthetic binary envelope: `nSections` sections of `shotsPer` shots
 *  each, with one more station than shots per section (station 0..=n). Values
 *  are arbitrary-but-deterministic; only the shape/ids matter here. */
function makeEnvelope(nSections: number, shotsPer: number): BinaryEnvelope {
  const sections = [];
  let shotCursor = 0;
  let stationCursor = 0;
  const stationRows: Array<{ sectionId: number; index: number }> = [];
  for (let s = 0; s < nSections; s++) {
    const sectionId = s + 1;
    sections.push({
      id: sectionId,
      name: `S${sectionId}`,
      color: '#ffffff',
      direction: 'AtoB',
      quality: 0,
      shot_offset: shotCursor,
      shot_count: shotsPer,
      next_shot_id: shotsPer + 1,
      credits: [],
    });
    shotCursor += shotsPer;
    // Stations 0..=shotsPer for this section.
    for (let st = 0; st <= shotsPer; st++) {
      stationRows.push({ sectionId, index: st });
      stationCursor++;
    }
  }
  const shotCount = shotCursor;
  const stationCount = stationCursor;

  const shotsF32 = new Float32Array(shotCount * SHOT_FLOAT_FIELDS);
  const shotsU32 = new Uint32Array(shotCount * SHOT_UINT_FIELDS);
  const shotsFlags = new Uint8Array(shotCount);
  let g = 0;
  for (let s = 0; s < nSections; s++) {
    const sectionId = s + 1;
    for (let j = 0; j < shotsPer; j++) {
      shotsF32[g * SHOT_FLOAT_FIELDS + 2] = j + 1; // length
      shotsU32[g * SHOT_UINT_FIELDS] = sectionId; // section_id
      shotsU32[g * SHOT_UINT_FIELDS + 1] = j + 1; // shot.id (1-based within section)
      shotsU32[g * SHOT_UINT_FIELDS + 2] = MARKER_NULL_WIRE;
      g++;
    }
  }

  const stationsF64 = new Float64Array(stationCount * STATION_FLOAT_FIELDS);
  const stationsU32 = new Uint32Array(stationCount * STATION_UINT_FIELDS);
  const stationsFlags = new Uint8Array(stationCount);
  stationRows.forEach((row, i) => {
    stationsF64[i * STATION_FLOAT_FIELDS] = i; // x
    stationsF64[i * STATION_FLOAT_FIELDS + 1] = i; // y
    stationsF64[i * STATION_FLOAT_FIELDS + 2] = -i; // z (depth)
    stationsU32[i * STATION_UINT_FIELDS] = row.sectionId;
    stationsU32[i * STATION_UINT_FIELDS + 1] = row.index;
  });

  const u8 = (a: Float32Array | Float64Array | Uint32Array): Uint8Array =>
    new Uint8Array(a.buffer, a.byteOffset, a.byteLength);

  return {
    // Keep in sync with BINARY_FORMAT_VERSION / EXPECTED_VERSION. This fixture
    // feeds buildResult() directly, bypassing the envelope-version gate, so a
    // stale value here rots silently rather than failing.
    version: 7,
    loading_id: 1,
    is_remote: false,
    project_header: {
      name: 'fixture',
      location: null,
      created_at: '2026-01-01T00:00:00Z',
      modified_at: '2026-01-01T00:00:00Z',
      file_path: null,
      next_section_id: nSections + 1,
      next_closure_id: 1,
      sections: sections as unknown as BinaryEnvelope['project_header']['sections'],
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
      credit_people: [],
    },
    shot_count: shotCount,
    shots_floats: u8(shotsF32),
    shots_uints: u8(shotsU32),
    shots_flags: shotsFlags,
    shot_comments: [],
    shots_credit: new Uint8Array(0),
    station_count: stationCount,
    stations_floats: u8(stationsF64),
    stations_uints: u8(stationsU32),
    stations_flags: stationsFlags,
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

/** A DistanceField covering every station in the fixture (all reachable). */
function distanceFieldFor(env: BinaryEnvelope): DistanceField {
  const station_refs = [];
  const distances: Array<number | null> = [];
  for (const s of env.project_header.sections) {
    for (let idx = 0; idx <= s.shot_count; idx++) {
      station_refs.push({ section_id: s.id, station_index: idx });
      distances.push(idx);
    }
  }
  return { station_refs, distances, max_reachable: 100 } as unknown as DistanceField;
}

function gasFieldFor(env: BinaryEnvelope): GasReachField {
  const station_refs = [];
  const gas_to_reach: Array<number | null> = [];
  const reachable: boolean[] = [];
  for (const s of env.project_header.sections) {
    for (let idx = 0; idx <= s.shot_count; idx++) {
      station_refs.push({ section_id: s.id, station_index: idx });
      gas_to_reach.push(idx * 10);
      reachable.push(true);
    }
  }
  return {
    station_refs,
    gas_to_reach,
    reachable,
    budget_litres: 1000,
    route: [],
  } as unknown as GasReachField;
}

describe('load-path laziness', () => {
  beforeEach(() => {
    shotMaterialization.count = 0;
  });

  it('decodes with every section lazy and zero materializations', () => {
    const { project, surveyData } = buildResult(makeEnvelope(50, 6), 0);
    expect(surveyData.shotCount).toBe(50 * 6);
    expect(shotMaterialization.count).toBe(0);
    let lazy = 0;
    for (const s of project.sections) {
      const d = Object.getOwnPropertyDescriptor(s, 'shots')!;
      expect(typeof d.get).toBe('function'); // accessor => not yet materialized
      lazy++;
    }
    expect(lazy).toBe(50);
  });

  it('distance + gas overlays and closure-adjusted geometry never materialize section.shots', () => {
    const env = makeEnvelope(50, 6);
    const { project, surveyData, stations } = buildResult(env, 0);
    shotMaterialization.count = 0;

    // Distance gradient (persisted-on at load) - used to walk section.shots.
    const dist = buildDistanceFeatureState(surveyData, distanceFieldFor(env));
    expect(dist.shots.size).toBe(50 * 6);

    // Gas reachability overlay (Plan mode) - same pattern.
    const gas = buildGasReachFeatureState(surveyData, gasFieldFor(env));
    expect(gas.shots.size).toBe(50 * 6);

    // Closure-adjusted view: rebuild from measured SoA + a fresh station set
    // (materialize the Station objects via getStation, which is SoA-only).
    const adjustedStations = Array.from({ length: surveyData.stationCount }, (_v, i) =>
      getStation(surveyData, i),
    );
    const adjusted = surveyDataWithAdjustedStations(surveyData, adjustedStations);
    expect(adjusted.shotCount).toBe(surveyData.shotCount);
    // Shot columns are shared with the measured set (no rebuild).
    expect(adjusted.shotIds).toBe(surveyData.shotIds);

    // The load-time walkers touched no lazy getter.
    expect(shotMaterialization.count).toBe(0);
    // Bonus: `stations` is the lazy proxy, and iterating it above materialized
    // no shots either.
    expect(stations.length).toBe(surveyData.stationCount);
    for (const s of project.sections) {
      expect(typeof Object.getOwnPropertyDescriptor(s, 'shots')!.get).toBe('function');
    }
  });

  it('negative control: reading section.shots once materializes exactly that section', () => {
    const { project } = buildResult(makeEnvelope(10, 4), 0);
    shotMaterialization.count = 0;
    const first = project.sections[0]!;
    expect(first.shots.length).toBe(4); // triggers the getter
    expect(shotMaterialization.count).toBe(1);
    // That section is now a data property; the rest stay lazy accessors.
    expect(Object.getOwnPropertyDescriptor(first, 'shots')!.get).toBeUndefined();
    expect(Array.isArray(Object.getOwnPropertyDescriptor(first, 'shots')!.value)).toBe(true);
    for (let i = 1; i < project.sections.length; i++) {
      expect(typeof Object.getOwnPropertyDescriptor(project.sections[i]!, 'shots')!.get).toBe(
        'function',
      );
    }
  });
});
