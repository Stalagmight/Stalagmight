/**
 * Struct-of-Arrays representation of a survey's bulk numeric data.
 *
 * Background: the legacy in-memory shape is `Project.sections[].shots[]`
 * (75k+ JS `Shot` objects across 14k+ `Section` arrays) plus a parallel
 * `Station[]` (89k+ `Station` objects). Each object has 5-16 named
 * fields and JSC's IsoHeap commits a per-class slab that doesn't
 * decommit on close. After loading 92.stalx once the WebProcess ends
 * up with ~2 GB of committed but-mostly-empty slabs that won't return
 * to the OS without external pressure.
 *
 * This module defines a flat typed-array layout (`SurveyData`) and
 * thin accessor helpers (`getShot`, `getStation`) that materialize a
 * legacy-shape object on demand. The intent:
 *
 *   - In-store representation: ~6 typed arrays + small sparse Maps.
 *     One bmalloc large-block per array. No per-class slabs.
 *   - Consumer representation: still `Shot` / `Station` objects, but
 *     created on access. DataTable/DetailPanel only materialize the
 *     ~100 rows/items they actually display.
 *
 * Field correspondence between SurveyData and the existing IPC types
 * (`Shot`, `Station`) is intentionally one-to-one so the migration is
 * mechanical: anywhere a consumer reads `shot.azimuth_in`, after the
 * migration it reads `data.shotAzimuthIn[i]` directly, or - if it
 * needs an object - calls `getShot(data, i)`.
 */

import type { Shot } from './ipc/Shot';
import type { ShotKind } from './ipc/ShotKind';
import type { Station } from './ipc/Station';
import type { StationRef } from './ipc/StationRef';
import type { ClosureRef } from './ipc/ClosureRef';
import type { GpsCoordinates } from './ipc/GpsCoordinates';
import type { AzimuthReference } from './ipc/AzimuthReference';
import type { SurveyDirection } from './ipc/SurveyDirection';

/** Section metadata kept as small structured objects. ~14k items for
 *  92.stalx; total memory is tiny compared to the shot payload. The
 *  `shots[]` array is *gone* - shots live in `SurveyData` keyed by
 *  global index and looked up via `shotOffset` + `shotCount`.
 *
 *  Field names match the IPC `Section` shape minus `shots`. */
export interface SectionMetadata {
  id: number;
  name: string | null;
  surveyor: string | null;
  /** ISO 8601 date string (matches IPC), or null. */
  date: string | null;
  color: string;
  direction: SurveyDirection;
  quality: number;
  next_shot_id: number;
  /** This section's north reference (#438); `null` inherits the project's. */
  azimuth_reference?: AzimuthReference | null;
  /** Offset into the flat shot arrays where this section's shots start. */
  shotOffset: number;
  /** Number of shots in this section (== section.shots.length pre-SoA). */
  shotCount: number;
}

/** Flat struct-of-arrays layout for shots + stations. All numeric
 *  fields live in typed arrays. Sparse / variable fields live in
 *  `Map`s keyed by global index. */
export interface SurveyData {
  // ─── Sections ──────────────────────────────────────────────────────
  /** Sections in declaration order. Each entry's `shotOffset` /
   *  `shotCount` index into the flat shot arrays below. */
  sections: SectionMetadata[];

  // ─── Shots (flat, one slot per shot) ───────────────────────────────
  /** Number of shots == length of each shot* array divided by stride. */
  shotCount: number;
  /** Shot.id per global shot index.
   *
   *  UNSIGNED (#555). Rust mints shot ids as
   *  `section_id * 100_000 + sequence_order` in `u32`, so a project with a
   *  high section id reaches ids past 2^31 (section 21_474 already does).
   *  An `Int32Array` wrapped those to negative numbers here while every other
   *  representation of the same id - `section.shots[j].id` off the lazy
   *  materializer, `ProjectWarning.shotId`, the map's feature ids - stayed
   *  unsigned, so the two sides silently stopped matching. */
  shotIds: Uint32Array;
  /** Section id per global shot index. Redundant with the section
   *  metadata's shotOffset+shotCount but lets a consumer go from
   *  shot index → section id without a binary search.
   *
   *  Unsigned for the same reason as `shotIds`: section ids are `u32` on the
   *  wire and nothing here ever stores a negative sentinel. */
  shotSectionIds: Uint32Array;
  shotAzimuthIn: Float32Array;
  shotAzimuthOut: Float32Array;
  shotLength: Float32Array;
  shotDepthIn: Float32Array;
  shotDepthOut: Float32Array;
  /** `NaN` represents `null` (Shot.pitch_in is `number | null`). */
  shotPitchIn: Float32Array;
  shotPitchOut: Float32Array;
  shotLeft: Float32Array;
  shotRight: Float32Array;
  shotUp: Float32Array;
  shotDown: Float32Array;
  shotTemperature: Float32Array;
  /** Bits 0-1: `ShotKind` discriminant. Room to grow (e.g. has_comment cache bit). */
  shotFlags: Uint8Array;
  /** `-1` represents `null` for the IPC's optional marker_index. */
  shotMarkerIndex: Int32Array;
  /** Sparse map of shot global index → comment string. Only shots
   *  that actually have a comment are in the map (most don't). */
  shotComments: Map<number, string>;

  // ─── Stations (flat, one slot per station) ─────────────────────────
  stationCount: number;
  /** Section id per global station index. Unsigned (#555): `u32` on the wire,
   *  no negative sentinel is ever stored, and it is compared/keyed against
   *  section ids that come from unsigned sources. */
  stationSectionIds: Uint32Array;
  stationIndices: Uint32Array;
  /** Stride-3 flat coordinates: [x0,y0,z0, x1,y1,z1, …]. */
  stationCoords: Float64Array;
  /** Bit 0: is_zero_datum. */
  stationFlags: Uint8Array;
  /** Sparse extras keyed by global station index. */
  stationGps: Map<number, GpsCoordinates>;
  stationMergedWith: Map<number, StationRef[]>;
  stationClosuresWith: Map<number, ClosureRef[]>;
  stationComments: Map<number, string>;
  /** Station names (e.g. the mNemo station number of a DMP import). */
  stationNames: Map<number, string>;
}

/**
 * Global counter of lazy `section.shots` materializations.
 *
 * The binary loader installs a per-section lazy `shots` getter that
 * materializes ~250 B `Shot` objects on first read. The whole point of the
 * SoA layout is that the *load / first-render* path never triggers those
 * getters for the whole project (75k+ objects). This counter lets a test or
 * the E2E harness assert that: after a settled load it must stay 0 (a single
 * section opened by the user afterwards is fine and expected to be nonzero).
 *
 * Held in a mutable object so the value can be read live (a primitive export
 * would snapshot). `main.tsx` exposes it on `window.__shotMaterializations`
 * in E2E builds.
 */
export const shotMaterialization = { count: 0 };

/** Increment the lazy-materialization counter. Called by the binary loader's
 *  `section.shots` getter on first access. Cheap enough to run unconditionally. */
export function noteShotMaterialization(): void {
  shotMaterialization.count++;
}

/** Sentinel: `Int32Array`'s representation of "the marker_index is null". */
export const SHOT_MARKER_NULL = -1;
/**
 * Mask for the `ShotKind` discriminant packed into bits 0-1 of `shotFlags`
 * (#27). Values match Rust's `ShotKind::as_u8`: 0 Normal, 1 Jump, 2 Reference.
 * Before #27 bit 0 alone meant `is_reference`.
 */
export const SHOT_FLAG_KIND_MASK = 0b11;

/** Ordered so the index equals the wire discriminant. */
const SHOT_KINDS: readonly ShotKind[] = ['Normal', 'Jump', 'Reference'];

/**
 * Decode the 2-bit discriminant in a `shotFlags` byte. An unrecognised value
 * (only `3` is reachable through the 2-bit mask) degrades to `'Reference'`
 * rather than throwing, mirroring Rust's `ShotKind::from_u8`: a corrupt byte
 * should not break a whole project load, but it must not produce a SWIMMABLE
 * leg either. `Normal` would be the unsafe reading - it enters gas, penetration
 * and the ruler at normal cost; `Reference` can only ever shrink a route (#27).
 */
export function shotKindFromFlags(flags: number): ShotKind {
  return SHOT_KINDS[flags & SHOT_FLAG_KIND_MASK] ?? 'Reference';
}

/** Encode a `ShotKind` into the low 2 bits of a `shotFlags` byte. */
export function shotKindToFlags(kind: ShotKind): number {
  const index = SHOT_KINDS.indexOf(kind);
  return index < 0 ? 0 : index;
}

/** True for the two virtual kinds. Mirrors Rust's `ShotKind::is_virtual`. */
export function isVirtualKind(kind: ShotKind): boolean {
  return kind !== 'Normal';
}
/** Bit position of the `is_zero_datum` flag in `stationFlags`. */
export const STATION_FLAG_IS_ZERO_DATUM = 0b01;

/** Convert a `Float32Array` slot back to `number | null`, using NaN as
 *  the null sentinel - matches the encoding in `loadProjectBinary.ts`. */
export function nanToNull(v: number): number | null {
  return Number.isNaN(v) ? null : v;
}

/**
 * Materialize a single `Shot` from a `SurveyData` at the given global
 * shot index (0-based across all sections). Cheap (one object alloc,
 * ~250 B) but call it sparingly - calling for every shot in a 75k
 * project undoes the point of the SoA layout. Use raw typed-array
 * field access (`data.shotAzimuthIn[i]`) in hot iteration paths.
 *
 * Throws if the index is out of range (defensive; would otherwise
 * silently return NaN'd / -1'd nonsense).
 */
export function getShot(data: SurveyData, globalIndex: number): Shot {
  if (globalIndex < 0 || globalIndex >= data.shotCount) {
    throw new RangeError(`shot index ${globalIndex} out of range (0..${data.shotCount})`);
  }
  return {
    id: data.shotIds[globalIndex]!,
    azimuth_in: data.shotAzimuthIn[globalIndex]!,
    azimuth_out: data.shotAzimuthOut[globalIndex]!,
    length: data.shotLength[globalIndex]!,
    depth_in: data.shotDepthIn[globalIndex]!,
    depth_out: data.shotDepthOut[globalIndex]!,
    pitch_in: nanToNull(data.shotPitchIn[globalIndex]!),
    pitch_out: nanToNull(data.shotPitchOut[globalIndex]!),
    left: nanToNull(data.shotLeft[globalIndex]!),
    right: nanToNull(data.shotRight[globalIndex]!),
    up: nanToNull(data.shotUp[globalIndex]!),
    down: nanToNull(data.shotDown[globalIndex]!),
    comment: data.shotComments.get(globalIndex) ?? null,
    kind: shotKindFromFlags(data.shotFlags[globalIndex]!),
    marker_index: data.shotMarkerIndex[globalIndex] === SHOT_MARKER_NULL
      ? null
      : data.shotMarkerIndex[globalIndex]!,
    temperature: nanToNull(data.shotTemperature[globalIndex]!),
  };
}

/**
 * Look up a shot by `(section_id, shot_id)` - the natural identity for
 * UI handlers that receive these from MapLibre feature properties.
 * Returns `null` if not found.
 *
 * Iterates the section's shot range. For sections of typical size
 * (~5-100 shots) this is cheap; for a 5000-shot section it's still
 * sub-millisecond. If lookups become a bottleneck, add an
 * `(sectionId, shotId) → globalIndex` Map at construction time.
 */
export function findShotGlobalIndex(
  data: SurveyData,
  sectionId: number,
  shotId: number,
): number | null {
  // Find the section's range.
  const sec = data.sections.find((s) => s.id === sectionId);
  if (!sec) return null;
  for (let i = sec.shotOffset; i < sec.shotOffset + sec.shotCount; i++) {
    if (data.shotIds[i] === shotId) return i;
  }
  return null;
}

/**
 * Materialize a single `Station` from a `SurveyData` at the given
 * global station index.
 */
export function getStation(data: SurveyData, globalIndex: number): Station {
  if (globalIndex < 0 || globalIndex >= data.stationCount) {
    throw new RangeError(`station index ${globalIndex} out of range (0..${data.stationCount})`);
  }
  const sectionId = data.stationSectionIds[globalIndex]!;
  const stationIndex = data.stationIndices[globalIndex]!;
  const coordBase = globalIndex * 3;
  const station: Station = {
    station_ref: {
      section_id: sectionId,
      station_index: stationIndex,
    },
    coordinates: {
      x: data.stationCoords[coordBase]!,
      y: data.stationCoords[coordBase + 1]!,
      z: data.stationCoords[coordBase + 2]!,
    },
    is_zero_datum: (data.stationFlags[globalIndex]! & STATION_FLAG_IS_ZERO_DATUM) !== 0,
    gps_coordinates: data.stationGps.get(globalIndex) ?? null,
    merged_with: data.stationMergedWith.get(globalIndex) ?? [],
  };
  // `closures_with` is optional on the wire and the sparse map only holds
  // non-empty lists, so materialize the absence rather than an empty array -
  // same rule as `comment` below, and the same shape `get_project_state` sends
  // (#441). Every consumer already guards.
  const closures = data.stationClosuresWith.get(globalIndex);
  if (closures !== undefined) station.closures_with = closures;
  const cmt = data.stationComments.get(globalIndex);
  if (cmt !== undefined) station.comment = cmt;
  const name = data.stationNames.get(globalIndex);
  if (name !== undefined) station.name = name;
  return station;
}

/**
 * Whether any station has a name (e.g. the mNemo station numbers a DMP import
 * records). The Display menu and the image export offer the "Station names"
 * labels only when this holds, because with no names they would label nothing.
 */
export function hasStationNames(data: Pick<SurveyData, 'stationNames'> | null | undefined): boolean {
  return (data?.stationNames.size ?? 0) > 0;
}

/**
 * Find a station's global index by `(section_id, station_index)`.
 * Used by MapLibre click/hover handlers that get those two numbers
 * from feature properties.
 *
 * Linear scan; fine for the volumes we care about. Add a Map index if
 * a profile shows this in hot paths.
 */
export function findStationGlobalIndex(
  data: SurveyData,
  sectionId: number,
  stationIndex: number,
): number | null {
  for (let i = 0; i < data.stationCount; i++) {
    if (data.stationSectionIds[i] === sectionId &&
        data.stationIndices[i] === stationIndex) {
      return i;
    }
  }
  return null;
}

/**
 * Build a read-only, lazily-materialized `Station[]` view over a
 * `SurveyData`.
 *
 * Numeric index access (`view[i]`) calls `getStation(data, i)`, `.length`
 * reports `data.stationCount`, and the standard read-only `Array` methods
 * (`find`, `filter`, `map`, `some`, `forEach`, `slice`, `for…of`,
 * `entries`) work because they read `length` and indices back through the
 * proxy traps. `Array.isArray(view)` is `true` (the proxy target is a real
 * array), so array-producing methods (`map`/`filter`/`slice`) return real
 * arrays.
 *
 * Memory intent: no `Station` object is retained. Each index access
 * materializes a fresh object, so a full pass (e.g. fit-to-bounds)
 * produces garbage-collectable temporaries instead of a persistent
 * ~89 k-object array that duplicates `stationCoords` / `stationSectionIds`
 * / `stationIndices` (the eager `Station[]` the binary loader used to
 * build). The proxy is read-only - the app never mutates the store
 * `stations` array - and its `set` / `deleteProperty` traps throw so an
 * accidental write surfaces immediately instead of silently no-opping.
 */
export function lazyStationArray(data: SurveyData): Station[] {
  // Empty target: holds no slots, so no per-station memory. `length` and
  // indices are served entirely by the traps below.
  const target: Station[] = [];

  /** Parse a property key as a canonical array index, else null. Mirrors
   *  the ECMAScript "array index" definition (non-negative integer with
   *  no leading zeros / other noise), so `'01'`, `'1.0'`, `'-1'`, `'x'`
   *  fall through to the ordinary property path. */
  const asIndex = (prop: string): number | null => {
    const n = Number(prop);
    if (!Number.isInteger(n) || n < 0) return null;
    if (String(n) !== prop) return null;
    return n;
  };

  const handler: ProxyHandler<Station[]> = {
    get(t, prop, receiver) {
      if (prop === 'length') return data.stationCount;
      if (typeof prop === 'string') {
        const idx = asIndex(prop);
        if (idx !== null) {
          return idx < data.stationCount ? getStation(data, idx) : undefined;
        }
      }
      return Reflect.get(t, prop, receiver);
    },
    has(t, prop) {
      if (prop === 'length') return true;
      if (typeof prop === 'string') {
        const idx = asIndex(prop);
        if (idx !== null) return idx < data.stationCount;
      }
      return Reflect.has(t, prop);
    },
    // This is a read-only lazy VIEW over `SurveyData`; the backing target is
    // empty, so a write (`stations[i] = x`, `.push(...)`, `.sort()`) would
    // silently no-op and desync from the SoA. Throw instead of swallowing it so
    // an accidental mutation surfaces at the call site rather than as a
    // mysteriously-empty array later. Rebuild SurveyData to change stations.
    set(_t, prop) {
      throw new TypeError(
        `lazyStationArray is a read-only view over SurveyData; cannot set "${String(prop)}". ` +
          'Rebuild SurveyData instead of mutating the station array.',
      );
    },
    deleteProperty(_t, prop) {
      throw new TypeError(
        `lazyStationArray is a read-only view over SurveyData; cannot delete "${String(prop)}".`,
      );
    },
  };

  return new Proxy(target, handler);
}

/**
 * Construct `SurveyData` from the legacy `Project.sections[].shots[]` +
 * `Station[]` shape that `refreshProjectState` still returns (it pulls
 * via JSON IPC, not the binary `load_project_full_binary` route).
 *
 * This is the migration-window converter - once the JSON IPC is also
 * binary-ized (out of scope for Phase A), this stops being needed.
 *
 * For the binary IPC path, see `buildSurveyData` inside
 * `utils/loadProjectBinary.ts` which builds `SurveyData` directly from
 * the wire bytes without going through legacy objects (faster, but
 * specific to that input shape).
 */
export function surveyDataFromLegacy(
  sections: import('./ipc/Section').Section[],
  stations: import('./ipc/Station').Station[],
): SurveyData {
  const totalShots = sections.reduce((s, sec) => s + sec.shots.length, 0);
  const data: SurveyData = {
    sections: [],
    shotCount: totalShots,
    shotIds: new Uint32Array(totalShots),
    shotSectionIds: new Uint32Array(totalShots),
    shotAzimuthIn: new Float32Array(totalShots),
    shotAzimuthOut: new Float32Array(totalShots),
    shotLength: new Float32Array(totalShots),
    shotDepthIn: new Float32Array(totalShots),
    shotDepthOut: new Float32Array(totalShots),
    shotPitchIn: new Float32Array(totalShots),
    shotPitchOut: new Float32Array(totalShots),
    shotLeft: new Float32Array(totalShots),
    shotRight: new Float32Array(totalShots),
    shotUp: new Float32Array(totalShots),
    shotDown: new Float32Array(totalShots),
    shotTemperature: new Float32Array(totalShots),
    shotFlags: new Uint8Array(totalShots),
    shotMarkerIndex: new Int32Array(totalShots),
    shotComments: new Map(),
    stationCount: stations.length,
    stationSectionIds: new Uint32Array(stations.length),
    stationIndices: new Uint32Array(stations.length),
    stationCoords: new Float64Array(stations.length * 3),
    stationFlags: new Uint8Array(stations.length),
    stationGps: new Map(),
    stationMergedWith: new Map(),
    stationClosuresWith: new Map(),
    stationComments: new Map(),
    stationNames: new Map(),
  };

  const nullToNaN = (v: number | null | undefined): number => (v == null ? NaN : v);

  let cursor = 0;
  for (const section of sections) {
    data.sections.push({
      id: section.id,
      name: section.name,
      surveyor: section.surveyor,
      date: section.date,
      color: section.color,
      direction: section.direction,
      quality: section.quality,
      next_shot_id: section.next_shot_id,
      shotOffset: cursor,
      shotCount: section.shots.length,
    });
    for (const shot of section.shots) {
      data.shotIds[cursor] = shot.id;
      data.shotSectionIds[cursor] = section.id;
      data.shotAzimuthIn[cursor] = shot.azimuth_in;
      data.shotAzimuthOut[cursor] = shot.azimuth_out;
      data.shotLength[cursor] = shot.length;
      data.shotDepthIn[cursor] = shot.depth_in;
      data.shotDepthOut[cursor] = shot.depth_out;
      data.shotPitchIn[cursor] = nullToNaN(shot.pitch_in);
      data.shotPitchOut[cursor] = nullToNaN(shot.pitch_out);
      data.shotLeft[cursor] = nullToNaN(shot.left);
      data.shotRight[cursor] = nullToNaN(shot.right);
      data.shotUp[cursor] = nullToNaN(shot.up);
      data.shotDown[cursor] = nullToNaN(shot.down);
      data.shotTemperature[cursor] = nullToNaN(shot.temperature);
      data.shotFlags[cursor] = shotKindToFlags(shot.kind);
      data.shotMarkerIndex[cursor] = shot.marker_index ?? SHOT_MARKER_NULL;
      if (shot.comment) data.shotComments.set(cursor, shot.comment);
      cursor++;
    }
  }

  stations.forEach((s, i) => {
    data.stationSectionIds[i] = s.station_ref.section_id;
    data.stationIndices[i] = s.station_ref.station_index;
    data.stationCoords[i * 3] = s.coordinates.x;
    data.stationCoords[i * 3 + 1] = s.coordinates.y;
    data.stationCoords[i * 3 + 2] = s.coordinates.z;
    data.stationFlags[i] = s.is_zero_datum ? STATION_FLAG_IS_ZERO_DATUM : 0;
    if (s.gps_coordinates) data.stationGps.set(i, s.gps_coordinates);
    // Rust's `Station` serializer skips `closures_with` when it is empty
    // (`skip_serializing_if = "Option::is_none"` in
    // src-tauri/crates/models/src/station.rs), so the JSON IPC path
    // (`get_project_state`) hands back stations with that field *missing* - not
    // an empty array. The generated type says so too (`closures_with?:`, #441).
    // `merged_with` is NOT skipped and is always present; the guard below is
    // kept anyway because it costs nothing and the two read alike. The binary
    // IPC path sets both via the sparse pair maps.
    const mergedWith = s.merged_with;
    if (mergedWith && mergedWith.length > 0) {
      data.stationMergedWith.set(i, mergedWith);
    }
    const closuresWith = s.closures_with;
    if (closuresWith && closuresWith.length > 0) {
      data.stationClosuresWith.set(i, closuresWith);
    }
    if (s.comment) data.stationComments.set(i, s.comment);
    if (s.name) data.stationNames.set(i, s.name);
  });

  return data;
}

/**
 * Build a `SurveyData` for the closure-adjusted geometry view by reusing the
 * measured `base`'s shot columns + section metadata verbatim and rebuilding
 * only the station columns from `adjustedStations`.
 *
 * Closure adjustment moves station *coordinates* only; shot ids, angles,
 * lengths, section shape and offsets are identical to the measured set. So the
 * adjusted view can share the base's (large) shot typed-arrays and just swap the
 * station arrays - an O(stations) pass that never touches the lazy
 * `section.shots` getters. The previous `surveyDataFromLegacy(project.sections,
 * adjusted)` walked every section's shots to rebuild shot columns it already had,
 * force-materializing ~75k `Shot` objects the moment the closure-adjusted mode
 * was entered (or restored from persisted UI state) on load.
 */
export function surveyDataWithAdjustedStations(
  base: SurveyData,
  adjustedStations: import('./ipc/Station').Station[],
): SurveyData {
  const SN = adjustedStations.length;
  const stationSectionIds = new Uint32Array(SN);
  const stationIndices = new Uint32Array(SN);
  const stationCoords = new Float64Array(SN * 3);
  const stationFlags = new Uint8Array(SN);
  const stationGps = new Map<number, GpsCoordinates>();
  const stationMergedWith = new Map<number, StationRef[]>();
  const stationClosuresWith = new Map<number, ClosureRef[]>();
  const stationComments = new Map<number, string>();
  const stationNames = new Map<number, string>();

  adjustedStations.forEach((s, i) => {
    stationSectionIds[i] = s.station_ref.section_id;
    stationIndices[i] = s.station_ref.station_index;
    stationCoords[i * 3] = s.coordinates.x;
    stationCoords[i * 3 + 1] = s.coordinates.y;
    stationCoords[i * 3 + 2] = s.coordinates.z;
    stationFlags[i] = s.is_zero_datum ? STATION_FLAG_IS_ZERO_DATUM : 0;
    if (s.gps_coordinates) stationGps.set(i, s.gps_coordinates);
    const mergedWith = s.merged_with;
    if (mergedWith && mergedWith.length > 0) stationMergedWith.set(i, mergedWith);
    const closuresWith = s.closures_with;
    if (closuresWith && closuresWith.length > 0) stationClosuresWith.set(i, closuresWith);
    if (s.comment) stationComments.set(i, s.comment);
    if (s.name) stationNames.set(i, s.name);
  });

  return {
    ...base,
    stationCount: SN,
    stationSectionIds,
    stationIndices,
    stationCoords,
    stationFlags,
    stationGps,
    stationMergedWith,
    stationClosuresWith,
    stationComments,
    stationNames,
  };
}

/**
 * Build an empty `SurveyData` (zero shots, zero stations, zero
 * sections). Useful as the initial store value and for tests.
 */
export function emptySurveyData(): SurveyData {
  return {
    sections: [],
    shotCount: 0,
    shotIds: new Uint32Array(0),
    shotSectionIds: new Uint32Array(0),
    shotAzimuthIn: new Float32Array(0),
    shotAzimuthOut: new Float32Array(0),
    shotLength: new Float32Array(0),
    shotDepthIn: new Float32Array(0),
    shotDepthOut: new Float32Array(0),
    shotPitchIn: new Float32Array(0),
    shotPitchOut: new Float32Array(0),
    shotLeft: new Float32Array(0),
    shotRight: new Float32Array(0),
    shotUp: new Float32Array(0),
    shotDown: new Float32Array(0),
    shotTemperature: new Float32Array(0),
    shotFlags: new Uint8Array(0),
    shotMarkerIndex: new Int32Array(0),
    shotComments: new Map(),
    stationCount: 0,
    stationSectionIds: new Uint32Array(0),
    stationIndices: new Uint32Array(0),
    stationCoords: new Float64Array(0),
    stationFlags: new Uint8Array(0),
    stationGps: new Map(),
    stationMergedWith: new Map(),
    stationClosuresWith: new Map(),
    stationComments: new Map(),
    stationNames: new Map(),
  };
}
