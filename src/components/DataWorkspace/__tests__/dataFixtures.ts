/**
 * Fully synthetic Data-workspace fixture: three sections with a branch merge,
 * a loop closure, a zero datum, a frozen shot, a Jump shot, station names,
 * comments and warnings. No real survey data.
 *
 *   1 "Main"                (4 shots) - station 0 zero datum; 1:2 is a junction
 *                                       (2:0 starts there); 1:4 closes to 3:3.
 *   2 "Branch B"            (2 shots) - continues from 1:2; shot 2 is a Jump.
 *   3 "Downstream sump 1A"  (3 shots) - shot 1 frozen; shot 2 warns, shot 3 errs.
 */
import type { ProjectWarning } from '../../../types/commands';
import type { LoopClosure, Project, Section, Shot, Station } from '../../../types/project';
import { surveyDataFromLegacy, type SurveyData } from '../../../types/survey-data';
import { buildColumnDefs } from '../model/columns';
import { buildSectionLabels } from '../model/sectionLabels';
import type { DataModelCtx } from '../types';

export const COLORS = { 1: '#22aa66', 2: '#8866dd', 3: '#dd8844' } as const;

function shot(id: number, length: number, az: number, extra: Partial<Shot> = {}): Shot {
  return {
    id,
    azimuth_in: az,
    azimuth_out: az + 2,
    length,
    depth_in: 1,
    depth_out: 2,
    pitch_in: null,
    pitch_out: null,
    left: 0.5,
    right: 1,
    up: null,
    down: 0.25,
    comment: null,
    kind: 'Normal',
    marker_index: null,
    temperature: null,
    ...extra,
  };
}

function section(id: number, name: string | null, shots: Shot[]): Section {
  return {
    id,
    name,
    color: COLORS[id as 1 | 2 | 3],
    direction: 'out',
    quality: 0,
    next_shot_id: id * 100 + shots.length + 1,
    shots,
  } as unknown as Section;
}

function station(sectionId: number, index: number, x: number, y: number, z: number, extra: Partial<Station> = {}): Station {
  return {
    station_ref: { section_id: sectionId, station_index: index },
    coordinates: { x, y, z },
    is_zero_datum: false,
    gps_coordinates: null,
    merged_with: [],
    ...extra,
  };
}

export const ref = (section_id: number, station_index: number) => ({ section_id, station_index });

export const SECTIONS: Section[] = [
  section(1, 'Main', [
    shot(101, 8.25, 45, { comment: 'Entrance squeeze' }),
    shot(102, 6.5, 70),
    shot(103, 11, 110, { pitch_in: 2, pitch_out: 4 }),
    shot(104, 7.25, 180),
  ]),
  section(2, 'Branch B', [shot(201, 4.5, 10), shot(202, 6, 355, { kind: 'Jump' })]),
  section(3, 'Downstream sump 1A', [shot(301, 5, 90), shot(302, 3, 95), shot(303, 9, 100)]),
];

export const CLOSURE: LoopClosure = {
  id: 7 as unknown as bigint,
  station_a: ref(1, 4),
  station_b: ref(3, 3),
  path_length: 40,
  misclosure_distance: 0.7,
  error_percentage: 1.25,
  horizontal_misclosure: 0.5,
  vertical_misclosure: 0.2,
  strategy: 'Proportional',
  is_aligned: false,
} as unknown as LoopClosure;

const closureRef = (to: { section_id: number; station_index: number }) => ({
  closure_id: 7 as unknown as bigint,
  station_ref: to,
  via_station: null,
  is_transitive: false,
});

export const STATIONS: Station[] = [
  station(1, 0, 0, 0, 0, { is_zero_datum: true, gps_coordinates: { latitude: 20.5, longitude: -87.25, depth_reference: 0 } }),
  station(1, 1, 5, 5, 1, { name: 'A1' }),
  station(1, 2, 10, 8, 1.5, { merged_with: [ref(2, 0)], comment: 'Junction' }),
  station(1, 3, 20, 8, 2),
  station(1, 4, 20, 1, 2, { closures_with: [closureRef(ref(3, 3))] }),
  station(2, 0, 10, 8, 1.5, { merged_with: [ref(1, 2)] }),
  station(2, 1, 11, 12, 2),
  station(2, 2, 10, 18, 2.5),
  station(3, 0, 30, 0, 3),
  station(3, 1, 35, 0, 3.5),
  station(3, 2, 38, 0, 4),
  station(3, 3, 47, 0, 4.5, { closures_with: [closureRef(ref(1, 4))] }),
];

export const WARNINGS: ProjectWarning[] = [
  { sectionId: 3, shotId: 302, shotIndex: 1, code: 'SHORT_SHOT', message: 'Very short shot', severity: 'warning', isResolved: false },
  { sectionId: 3, shotId: 303, shotIndex: 2, code: 'ZERO_DEPTH', message: 'Depth jump too large', severity: 'error', isResolved: false },
  { sectionId: 3, shotId: 303, shotIndex: 2, code: 'LONG_SHOT', message: 'Long shot', severity: 'warning', isResolved: true },
];

export function makeProject(sections: Section[] = SECTIONS): Project {
  return {
    name: 'Synthetic',
    location: null,
    created_at: '2026-01-01T00:00:00Z',
    modified_at: '2026-01-01T00:00:00Z',
    sections,
    station_merges: [{ station_a: ref(1, 2), station_b: ref(2, 0), merged_at: '2026-01-01T00:00:00Z' }],
    zero_datums: [{ station_ref: ref(1, 0), gps: { latitude: 20.5, longitude: -87.25, depth_reference: 0 } }],
    loop_closures: [CLOSURE],
    file_path: null,
    next_section_id: 4,
    section_offsets: {},
    imported_file_hashes: [],
    imported_section_hashes: [],
    next_closure_id: 8 as unknown as bigint,
    validation: { azimuth_diff_threshold: 5, pitch_diff_threshold: 5, inclination_depth_diff_threshold: 1 },
    surface_elevation: null,
  } as unknown as Project;
}

export function makeData(): SurveyData {
  return surveyDataFromLegacy(SECTIONS, STATIONS);
}

export function warningsByShot(warnings: readonly ProjectWarning[]): Map<string, ProjectWarning[]> {
  const m = new Map<string, ProjectWarning[]>();
  for (const w of warnings) {
    const k = `${w.sectionId}:${w.shotId}`;
    const list = m.get(k);
    if (list) list.push(w);
    else m.set(k, [w]);
  }
  return m;
}

export function makeCtx(overrides: Partial<DataModelCtx> = {}): DataModelCtx {
  const data = overrides.data ?? makeData();
  const project = overrides.project ?? makeProject();
  return {
    data,
    project,
    sectionsById: new Map(data.sections.map((s) => [s.id, s])),
    sectionLabels: buildSectionLabels(data.sections, (id) => `Section ${id}`),
    sectionNames: new Map(data.sections.map((s) => [s.id, s.name ?? `Section ${s.id}`])),
    warningsByShot: warningsByShot(WARNINGS),
    frozenShots: new Set(['3:0']),
    precision: 1,
    locale: 'en-US',
    editingBlocked: false,
    decimals: { length: 2, coordinates: 2, depth: 1, azimuth: 1, inclination: 1, gps: 6 },
    ...overrides,
  };
}

export const COLUMNS = buildColumnDefs();
export const col = (key: string) => COLUMNS.find((c) => c.key === key)!;
