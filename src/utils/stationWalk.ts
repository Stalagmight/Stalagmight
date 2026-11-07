/**
 * The station walk: where a section's stations land, computed from its shots
 * (#576 D15).
 *
 * The backend is the authority - `Project::compute_station_coordinates` in
 * `src-tauri/crates/models/src/project/coordinates.rs` - but the import and
 * placement previews draw sections that are not in the project yet, and redraw
 * on every edit, so they cannot ask it. This is the frontend's ONE copy of that
 * walk. Every preview used to carry its own, and none was the backend's: they
 * walked the SLOPE length (a 10 m shot dropping 6 m drew 10 m, not 8 m), some
 * took one reading where the backend averages two, and none applied the
 * section's north reference - so a station that should sit on an existing one
 * was drawn metres away from it.
 *
 * Kept in lockstep with Rust by `src/utils/__tests__/stationWalkVectors.json`,
 * one table asserted on both sides (`stationWalk.test.ts` here,
 * `station_walk_parity_tests.rs` there). Change the formula on either side and
 * the other side's assertion has to change with it.
 *
 * Per shot, exactly as `coordinates.rs`:
 *   - bearing: `averageAzimuth(in, out)` (= `average_azimuth_of`), plus the
 *     section's north-reference correction (= `AzimuthReference::to_true`);
 *   - plan length: the tape projected by the shot's depth change
 *     (= `horizontal_length_from_slope`), never the tape itself;
 *   - `x += h * sin(bearing)`, `y += h * cos(bearing)`, accumulated in order.
 * A station's z is the mean of the depths of the shots either side of it (the
 * incoming `depth_out`, the outgoing `depth_in`), as `compute_station_depth`
 * does for a station with no merge partners - a preview has none.
 */
import type { AzimuthReference } from '../types/ipc/AzimuthReference';
import { averageAzimuth, normalizeAzimuth } from './azimuth';

/** The fields of a shot the walk reads - `ShotData`'s names. */
export interface WalkShot {
  azimuth_in: number;
  azimuth_out: number;
  length: number;
  depth_in: number;
  depth_out: number;
}

export interface WalkedStation {
  x: number;
  y: number;
  z: number;
}

/**
 * The north reference in force for a section: its own, else the project's
 * default, else none (no correction). `Project::azimuth_reference_for`.
 */
export function azimuthReferenceFor(
  sectionReference: AzimuthReference | null | undefined,
  projectDefault: AzimuthReference | null | undefined
): AzimuthReference | null {
  return sectionReference ?? projectDefault ?? null;
}

/**
 * The degrees a reference adds to a measured bearing. `applied_correction`,
 * with the invariant every backend ingress enforces (`sanitised`): a true-north
 * reference carries no correction, and a non-finite one is none.
 */
function appliedCorrection(reference: AzimuthReference | null | undefined): number {
  if (!reference || reference.north === 'true') return 0;
  return Number.isFinite(reference.correction) ? reference.correction : 0;
}

/**
 * Plan length of a shot: `horizontal_length_from_slope`, including its guard
 * for a tape so long the squares overflow (it scales into [0, 1] first, so the
 * length stays finite). Exported for the warning-resolution ghost
 * (`resolutionPreview.ts`), which projects the same tape the same way.
 */
export function horizontalLength(length: number, dz: number): number {
  const lenSq = length * length - dz * dz;
  if (Number.isFinite(lenSq) || !(Number.isFinite(length) && Number.isFinite(dz))) {
    return lenSq <= 0 ? 0 : Math.sqrt(lenSq);
  }
  const scale = Math.max(Math.abs(length), Math.abs(dz));
  const l = Math.abs(length) / scale;
  const d = Math.abs(dz) / scale;
  const scaledSq = (l - d) * (l + d);
  return scaledSq <= 0 ? 0 : scale * Math.sqrt(scaledSq);
}

/**
 * The measured bearing of a shot. Two readings average across the short arc,
 * as the backend does. A preview can hold a row with one reading not yet
 * typed, which the backend never sees (both legs are required to commit); it
 * walks the lone leg, i.e. the bearing that leg would give on both legs.
 */
function measuredBearing(shot: WalkShot): number {
  const inOk = Number.isFinite(shot.azimuth_in);
  const outOk = Number.isFinite(shot.azimuth_out);
  if (inOk && outOk) return averageAzimuth(shot.azimuth_in, shot.azimuth_out);
  if (inOk) return averageAzimuth(shot.azimuth_in, shot.azimuth_in);
  if (outOk) return averageAzimuth(shot.azimuth_out, shot.azimuth_out);
  return Number.NaN;
}

/**
 * A component that is not a number moves nothing. The backend never walks a
 * non-finite shot (`ShotData::all_finite` refuses it at ingest), so on its
 * domain this changes nothing; a preview, though, draws half-typed rows, and
 * one NaN would otherwise carry into every station after it and leave the map
 * nothing to fit. The station is still emitted, so indices keep addressing
 * the rows they came from.
 */
const finiteStep = (value: number): number => (Number.isFinite(value) ? value : 0);

/** Mean of the finite depths either side of station `index`. */
function stationDepth(shots: readonly WalkShot[], index: number): number {
  let sum = 0;
  let count = 0;
  const incoming = shots[index - 1]?.depth_out;
  const outgoing = shots[index]?.depth_in;
  for (const depth of [incoming, outgoing]) {
    if (depth !== undefined && Number.isFinite(depth)) {
      sum += depth;
      count += 1;
    }
  }
  return count === 0 ? 0 : sum / count;
}

/**
 * Walk `shots` from (0, 0): one station per shot plus station 0.
 *
 * `reference` is the north reference the section is (or, for an import, WILL
 * be) recorded with - resolve it with {@link azimuthReferenceFor}. Omitted, the
 * readings are walked as measured, which is what a section with nothing
 * recorded gets.
 */
export function walkStations(
  shots: readonly WalkShot[],
  reference?: AzimuthReference | null
): WalkedStation[] {
  // Resolved once per section, as the backend does.
  const correction = appliedCorrection(reference);
  const degToRad = Math.PI / 180; // Rust's `to_radians` factor, same rounding.
  let x = 0;
  let y = 0;
  const stations: WalkedStation[] = [{ x, y, z: stationDepth(shots, 0) }];
  shots.forEach((shot, i) => {
    const measured = measuredBearing(shot);
    // `to_true`: the identity returns the measured mean untouched.
    const bearing = correction === 0 ? measured : normalizeAzimuth(measured + correction);
    const radians = bearing * degToRad;
    const horizontal = horizontalLength(shot.length, shot.depth_out - shot.depth_in);
    x += finiteStep(horizontal * Math.sin(radians));
    y += finiteStep(horizontal * Math.cos(radians));
    stations.push({ x, y, z: stationDepth(shots, i + 1) });
  });
  return stations;
}
