/**
 * Builders for the survey data the backend sends (#576 E15), typed against the
 * generated IPC types so a field the backend adds is a compile error here, not a
 * silently missing key in thirty hand-written literals.
 *
 * Each builder returns a complete object: every required field at a neutral
 * default, overridden by `overrides` (a shallow merge, so a nested override such
 * as `coordinates` replaces the whole value). An optional field (`uuid`,
 * `closures_with`, `comment`, `azimuth_reference`, ...) is absent unless given.
 * Keys come out in the generated type's order. Synthetic data only.
 */
import type { Project } from '../../types/ipc/Project';
import type { Section } from '../../types/ipc/Section';
import type { Shot } from '../../types/ipc/Shot';
import type { Station } from '../../types/ipc/Station';
import type { StationRef } from '../../types/ipc/StationRef';
import type { ValidationConfig } from '../../types/ipc/ValidationConfig';

/** A Normal shot of 10 m at azimuth 0, level, with no LRUD, comment or marker. */
export function makeShot(overrides: Partial<Shot> = {}): Shot {
  return {
    id: 1,
    azimuth_in: 0,
    azimuth_out: 0,
    length: 10,
    depth_in: 0,
    depth_out: 0,
    pitch_in: null,
    pitch_out: null,
    left: null,
    right: null,
    up: null,
    down: null,
    comment: null,
    kind: 'Normal',
    marker_index: null,
    temperature: null,
    ...overrides,
  };
}

/**
 * Section 1, unnamed, surveyed "in", with no shots unless given. `next_shot_id`
 * defaults to one past the highest shot id (1 for an empty section), as the
 * backend keeps it.
 */
export function makeSection(overrides: Partial<Section> = {}): Section {
  const shots = overrides.shots ?? [];
  return {
    id: 1,
    name: null,
    surveyor: null,
    date: null,
    color: '#3388ff',
    direction: 'in',
    quality: 0,
    shots,
    next_shot_id: shots.reduce((max, s) => Math.max(max, s.id + 1), 1),
    reverse_parity: false,
    gauge_shot_uuid: null,
    gauge_neighbour_uuid: null,
    ...overrides,
  };
}

/** Validation thresholds loose enough that a builder's shots raise no warning. */
export const TEST_VALIDATION: ValidationConfig = {
  warn_shot_length_min: 0,
  warn_shot_length_max: 100,
  warn_depth_change_per_shot_max: 100,
  warn_azimuth_deviation_max: 45,
  azimuth_diff_threshold: 5,
  pitch_diff_threshold: 5,
  inclination_depth_diff_threshold: 5,
};

/**
 * An unsaved project "CaveA" with no sections unless given. `next_section_id`
 * defaults to one past the highest section id (1 for none).
 */
export function makeProject(overrides: Partial<Project> = {}): Project {
  const sections = overrides.sections ?? [];
  return {
    name: 'CaveA',
    location: null,
    created_at: '2026-01-01T00:00:00Z',
    modified_at: '2026-01-01T00:00:00Z',
    sections,
    station_merges: [],
    zero_datums: [],
    loop_closures: [],
    file_path: null,
    next_section_id: sections.reduce((max, s) => Math.max(max, s.id + 1), 1),
    section_offsets: {},
    imported_file_hashes: [],
    imported_section_hashes: [],
    next_closure_id: 1,
    validation: { ...TEST_VALIDATION },
    surface_elevation: null,
    ...overrides,
  };
}

/** `{ section_id, station_index }`. */
export function stationRef(section_id: number, station_index: number): StationRef {
  return { section_id, station_index };
}

/** Station 1:0 at the origin: not a zero datum, no GPS fix, merged with nothing. */
export function makeStation(overrides: Partial<Station> = {}): Station {
  return {
    station_ref: { section_id: 1, station_index: 0 },
    coordinates: { x: 0, y: 0, z: 0 },
    is_zero_datum: false,
    gps_coordinates: null,
    merged_with: [],
    ...overrides,
  };
}
