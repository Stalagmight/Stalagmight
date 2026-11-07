/**
 * #576 RV7b-5: the legacy `SurveyData` builder agrees with the binary decoder.
 *
 * The load and the state refresh build `SurveyData` straight from the binary
 * envelope (`buildSurveyData` in `utils/loadProjectBinary.ts`), but
 * `surveyDataFromLegacy` still builds it for two fallbacks - the
 * closure-adjusted geometry fetch with no measured SoA in the store, and the map
 * layer's flush with no active geometry - and for every test fixture built from
 * legacy shapes. Two builders that must agree on ~20 columns, with nothing
 * pinning them: the legacy one already dropped each section's
 * `azimuth_reference` (the north reference the map and exports rotate by).
 *
 * One synthetic project goes through both: packed into the envelope the backend
 * sends (`test/projectStateEnvelope.ts`, which follows `core/load_binary.rs`, not
 * the decoder) and decoded by the real `buildResult`, and handed to
 * `surveyDataFromLegacy` - once as the JSON shapes the fixtures use, once as the
 * decoded project and stations the two production fallbacks pass. Every column
 * must come out identical, and each failure names its column.
 *
 * All data is synthetic.
 */
import { describe, it, expect } from 'vitest';
import { decode } from '@msgpack/msgpack';

import { surveyDataFromLegacy, type SurveyData } from '../survey-data';
import type { Section } from '../ipc/Section';
import type { Shot } from '../ipc/Shot';
import type { Station } from '../ipc/Station';
import type { AzimuthReference } from '../ipc/AzimuthReference';
import { buildResult, type BinaryEnvelope } from '../../utils/loadProjectBinary';
import { projectStateEnvelope } from '../../test/projectStateEnvelope';

const IGRF: AzimuthReference = { north: 'magnetic', correction: 10.5, source: { igrf_auto: { epoch_year: 2024 } } };
const MANUAL_GRID: AzimuthReference = { north: 'grid', correction: -1.75, source: 'manual' };
const TRUE_UNKNOWN: AzimuthReference = { north: 'true', correction: 0, source: 'unknown' };

const shot = (id: number, overrides: Partial<Shot>): Shot => ({
  id,
  azimuth_in: 12.3,
  azimuth_out: 191.7,
  length: 4.56,
  depth_in: 1.1,
  depth_out: 2.35,
  pitch_in: null,
  pitch_out: null,
  left: 0.4,
  right: 0.7,
  up: 1.9,
  down: 0.25,
  comment: null,
  kind: 'Normal',
  marker_index: null,
  temperature: 24.6,
  ...overrides,
});

const section = (id: number, overrides: Partial<Section>): Section => ({
  id,
  name: `CaveA section ${id}`,
  surveyor: 'SurveyorA',
  date: '2026-03-14',
  color: '#3366cc',
  direction: 'in',
  quality: 0.8,
  shots: [],
  next_shot_id: id * 100_000 + 10,
  reverse_parity: false,
  gauge_shot_uuid: null,
  gauge_neighbour_uuid: null,
  ...overrides,
});

/**
 * Four sections: one per kind of north reference (declared IGRF, manual grid,
 * an empty section declaring true north, and one inheriting with none), every
 * shot kind, comments, markers, nulls next to values, readings that are not
 * f32-exact, and a section id high enough that its shot ids pass 2^31 (#555).
 */
function project(): { sections: Section[]; stations: Station[] } {
  const sections: Section[] = [
    section(7, {
      azimuth_reference: IGRF,
      shots: [
        shot(700_000, { comment: 'StationA1 lead', marker_index: 3 }),
        shot(700_001, { kind: 'Jump', pitch_in: -12.5, pitch_out: 13.25, left: null, temperature: null }),
        shot(700_002, { kind: 'Reference', length: 0.1, up: null, down: null, marker_index: 0 }),
      ],
    }),
    section(30_000, {
      name: null,
      surveyor: null,
      date: null,
      color: '#00aa55',
      direction: 'out',
      quality: 1.5,
      azimuth_reference: MANUAL_GRID,
      shots: [shot(3_000_000_000, { azimuth_in: 359.9, azimuth_out: 0.1, comment: 'past 2^31' })],
    }),
    section(8, { azimuth_reference: TRUE_UNKNOWN, shots: [], next_shot_id: 800_000 }),
    section(9, {
      direction: 'out',
      shots: [shot(900_000, { right: null, depth_in: 30.75, depth_out: 31.5 }), shot(900_001, { marker_index: 7 })],
    }),
  ];
  const ref = (section_id: number, station_index: number) => ({ section_id, station_index });
  const stations: Station[] = [
    {
      station_ref: ref(7, 0),
      coordinates: { x: 0.125, y: -3.5, z: -1.1 },
      is_zero_datum: true,
      gps_coordinates: { latitude: 20.5, longitude: -87.25, depth_reference: 0 },
      merged_with: [],
      comment: 'datum StationA1',
    },
    {
      station_ref: ref(7, 1),
      coordinates: { x: 4.2, y: 1.333, z: -2.2 },
      is_zero_datum: false,
      gps_coordinates: null,
      merged_with: [ref(9, 0)],
      closures_with: [{ closure_id: 1, station_ref: ref(9, 2), via_station: null, is_transitive: false }],
    },
    {
      station_ref: ref(30_000, 0),
      coordinates: { x: -10.01, y: 22.02, z: -5.5 },
      is_zero_datum: false,
      gps_coordinates: null,
      merged_with: [],
    },
    {
      station_ref: ref(9, 0),
      coordinates: { x: 4.2, y: 1.333, z: -2.2 },
      is_zero_datum: false,
      gps_coordinates: null,
      merged_with: [ref(7, 1)],
      closures_with: [
        { closure_id: 1, station_ref: ref(7, 1), via_station: ref(9, 1), is_transitive: true },
      ],
      comment: 'StationA9 junction',
    },
  ];
  return { sections, stations };
}

/** The decoder's answer for the project: the envelope bytes through msgpack and
 *  the real `buildResult`. */
function decoded(sections: Section[], stations: Station[]) {
  const bytes = new Uint8Array(projectStateEnvelope({ project: { sections }, stations }));
  return buildResult(decode(bytes) as BinaryEnvelope, bytes.byteLength);
}

/** Every column of `actual` equals `expected`'s, the failure naming the column. */
function expectSameSurveyData(actual: SurveyData, expected: SurveyData): void {
  expect(Object.keys(actual).sort(), 'SurveyData columns').toEqual(Object.keys(expected).sort());
  for (const column of Object.keys(expected) as Array<keyof SurveyData>) {
    const a = actual[column];
    const e = expected[column];
    if (ArrayBuffer.isView(e)) {
      expect(a?.constructor.name, `SurveyData.${column} array type`).toBe(e.constructor.name);
      expect(Array.from(a as ArrayLike<number>), `SurveyData.${column}`).toEqual(Array.from(e as ArrayLike<number>));
    } else if (column === 'sections') {
      const as = a as SurveyData['sections'];
      const es = e as SurveyData['sections'];
      expect(as.length, 'SurveyData.sections length').toBe(es.length);
      es.forEach((sec, i) => {
        expect(as[i], `SurveyData.sections[${i}] (id ${sec.id})`).toStrictEqual(sec);
      });
    } else {
      expect(a, `SurveyData.${column}`).toStrictEqual(e);
    }
  }
}

describe('surveyDataFromLegacy agrees with the binary decoder (#576 RV7b-5)', () => {
  it('builds the same SurveyData from the JSON shapes as the decoder builds from the envelope', () => {
    const { sections, stations } = project();
    const binary = decoded(sections, stations).surveyData;

    // Premises: the synthetic project reaches every branch it is meant to.
    expect(binary.sections.map((s) => s.azimuth_reference ?? null)).toEqual([IGRF, MANUAL_GRID, TRUE_UNKNOWN, null]);
    expect(binary.shotIds[3]).toBe(3_000_000_000);
    expect(binary.shotComments.size).toBe(2);

    expectSameSurveyData(surveyDataFromLegacy(sections, stations), binary);
  });

  it('builds the same SurveyData from the decoded project and stations the fallbacks pass', () => {
    const { sections, stations } = project();
    const result = decoded(sections, stations);

    // projectStore's adjusted-geometry fallback and SurveyLayer's flush hand the
    // store's project.sections (lazy shots getters) and stations (lazy view).
    expectSameSurveyData(surveyDataFromLegacy(result.project.sections, result.stations), result.surveyData);
  });
});
