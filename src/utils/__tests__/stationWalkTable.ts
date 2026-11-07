/**
 * The shared station-walk table (#576 D15), typed for the TS tests.
 *
 * `stationWalkVectors.json` is read by BOTH sides: the Rust walker asserts it in
 * `src-tauri/crates/models/src/project/station_walk_parity_tests.rs`, and every
 * TS preview walker asserts it here. A row is one section walked from (0, 0);
 * its expected stations are what `Project::compute_station_coordinates` returns.
 */
import { expect } from 'vitest';
import table from './stationWalkVectors.json';
import type { AzimuthReference } from '../../types/ipc/AzimuthReference';

export interface VectorShot {
  azimuth_in: number;
  /** `null` = never read. Rust cannot hold one; its row carries the lone leg. */
  azimuth_out: number | null;
  length: number;
  depth_in: number;
  depth_out: number;
}

export interface StationWalkVector {
  name: string;
  reference: AzimuthReference | null;
  project_default: AzimuthReference | null;
  shots: VectorShot[];
  /** `[x, y, z]` per station, station 0 first. */
  stations: number[][];
}

export const STATION_WALK_TOLERANCE: number = table.tolerance;
export const STATION_WALK_VECTORS = table.vectors as unknown as StationWalkVector[];

/** The row's shots as a preview holds them: a missing leg is NaN, not null. */
export function vectorShots(vector: StationWalkVector) {
  return vector.shots.map((s) => ({ ...s, azimuth_out: s.azimuth_out ?? Number.NaN }));
}

/**
 * Within the table's tolerance: absolute for ordinary metres, relative for the
 * overflow-guard row (whose stations are ~1e154 m).
 */
export function near(want: number) {
  const magnitude = Math.max(1, Math.abs(want));
  const digits = -Math.log10((2 * STATION_WALK_TOLERANCE * magnitude));
  return expect.closeTo(want, digits);
}

/** The row's expected stations as matchers, on the axes a walker exposes. */
export function expectedStations(vector: StationWalkVector, axes: 2 | 3 = 3) {
  return vector.stations.map((s) => s.slice(0, axes).map(near));
}

/** `it.each` rows: `[name, vector]`. */
export function vectorCases(filter: (v: StationWalkVector) => boolean = () => true) {
  return STATION_WALK_VECTORS.filter(filter).map((v) => [v.name, v] as const);
}
