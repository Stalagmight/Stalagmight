/**
 * Decoder for the `load_project_full_binary` IPC response.
 *
 * The Rust side serializes `LoadProjectFullResult` with a layout that
 * keeps small/sparse metadata in an msgpack envelope and packs the bulk
 * numeric fields (shot/station per-row data) into typed-array byte
 * buffers. Reconstructing the same JS shape as the legacy `Project` /
 * `Station[]` is cheap because it's pure memory reads - no per-field
 * parser, no string lookups.
 *
 * Performance target: a 75k-shot / 89k-station project decodes in
 * ~150-300 ms (vs ~5 s for JSON.parse + materialization on the same
 * payload as JSON). Wire format documented in
 * src-tauri/src/core/load_binary.rs - keep field orders in sync.
 */
import { invoke } from '@tauri-apps/api/core';
import { decode as msgpackDecode } from '@msgpack/msgpack';
import type { LoadProjectFullResult } from '../types/ipc/LoadProjectFullResult';
import type { Project } from '../types/ipc/Project';
import type { AzimuthReference } from '../types/ipc/AzimuthReference';
import type { ValidationConfig } from '../types/ipc/ValidationConfig';
import type { Section } from '../types/ipc/Section';
import type { Shot } from '../types/ipc/Shot';
import type { Person } from '../types/ipc/Person';
import type { SurveyCredit } from '../types/ipc/SurveyCredit';
import type { StationRef } from '../types/ipc/StationRef';
import type { ClosureRef } from '../types/ipc/ClosureRef';
import type { GpsCoordinates } from '../types/ipc/GpsCoordinates';
import type { LoopClosure } from '../types/ipc/LoopClosure';
import type { StationMerge } from '../types/ipc/StationMerge';
import type { ZeroDatum } from '../types/ipc/ZeroDatum';
import type { ProjectStats } from '../types/ipc/ProjectStats';
import type { ProjectWarning } from '../types/ipc/ProjectWarning';
import type { FrozenShotRef } from '../types/ipc/FrozenShotRef';
import type { DisplaySettings } from '../types/ipc/DisplaySettings';
import type { SurveyDirection } from '../types/ipc/SurveyDirection';
import {
  type SurveyData,
  type SectionMetadata,
  lazyStationArray,
  noteShotMaterialization,
  SHOT_FLAG_KIND_MASK,
  shotKindFromFlags,
  SHOT_MARKER_NULL,
  STATION_FLAG_IS_ZERO_DATUM,
} from '../types/survey-data';
import { recordPerfTiming } from './memoryProbe';

/**
 * Extended result type that carries both the legacy LoadProjectFullResult
 * shape (Project + Station[]) and the new Struct-of-Arrays `surveyData`.
 * During Phase A migration both are populated from the same typed-array
 * buffers. Phase A.6 removes the legacy fields and consumers source
 * everything from `surveyData`.
 */
export interface LoadProjectFullResultWithSoa extends LoadProjectFullResult {
  /** Struct-of-Arrays bulk-data layout. Phase-A staging area. */
  surveyData: SurveyData;
  /** Frozen shots computed during load, shipped in the binary envelope (the
   *  JSON `LoadProjectFullResult` omits them). Lets the store seed `frozenShots`
   *  on a binary load instead of inheriting the previous project's set (#57). */
  frozen_shots: FrozenShotRef[];
}

/**
 * Wire format version expected from the Rust encoder. Must track
 * `BINARY_FORMAT_VERSION` in `src-tauri/src/core/load_binary.rs` exactly - the
 * version check below is the only thing standing between a format change and a
 * silently misdecoded project.
 *
 * 3 covered two changes that landed in the same unreleased version:
 * - #27: the per-shot flags byte carries a 2-bit `ShotKind` discriminant in
 *   bits 0-1 instead of a single `is_reference` bit, so a v2 reader would
 *   decode a Reference (2) as "not virtual".
 * - #270: the envelope carries `is_remote`, so the SpeleoDB read-only gate
 *   settles with the load.
 *
 * 4 drops the header's `collab` field with Ariane collaboration mode. The
 * envelope is map-encoded (`rmp_serde::to_vec_named`), so a stale reader would
 * find the key absent rather than misalign - the bump is a deliberate fence, not
 * a decode requirement, and it keeps this constant honest about the shape.
 */
const EXPECTED_VERSION = 7;
/** f32 fields per shot. Mirror of SHOT_FLOAT_FIELDS in Rust. */
const SHOT_FLOAT_FIELDS = 12;
/** u32 fields per shot. Mirror of SHOT_UINT_FIELDS in Rust. */
const SHOT_UINT_FIELDS = 3;
/** f64 fields per station. */
const STATION_FLOAT_FIELDS = 3;
/** u32 fields per station. */
const STATION_UINT_FIELDS = 2;

/** Shape of the msgpack-decoded envelope. Mirrors `LoadProjectFullBinaryResult`.
 *  Exported for the load-path laziness regression test's fixture builder. */
export interface BinaryEnvelope {
  version: number;
  loading_id: bigint | number;
  /** Is this a SpeleoDB project? Same value the backend set its edit gate
   *  from, so the frontend's read-only gate settles with the load (#270). */
  is_remote: boolean;
  project_header: ProjectHeaderBinary;
  shot_count: number;
  shots_floats: Uint8Array;
  shots_uints: Uint8Array;
  shots_flags: Uint8Array;
  shot_comments: Array<[number, string]>;
  /** Per-shot u32: index into the shot's SECTION `credits`, 0xFFFFFFFF for none.
   *  Empty (zero bytes) when no shot of the project carries a credit (v7). */
  shots_credit: Uint8Array;
  station_count: number;
  stations_floats: Uint8Array;
  stations_uints: Uint8Array;
  stations_flags: Uint8Array;
  station_gps: Array<[number, GpsCoordinates]>;
  station_merged_with: Array<[number, StationRef[]]>;
  station_closures_with: Array<[number, ClosureRef[]]>;
  station_comments: Array<[number, string]>;
  station_names: Array<[number, string]>;
  stats: ProjectStats;
  warnings: ProjectWarning[];
  display_settings: DisplaySettings;
  frozen_shots: unknown[];
}

interface ProjectHeaderBinary {
  name: string;
  location: string | null;
  created_at: string;
  modified_at: string;
  file_path: string | null;
  next_section_id: number;
  next_closure_id: bigint | number;
  sections: SectionHeaderBinary[];
  station_merges: StationMerge[];
  zero_datums: ZeroDatum[];
  loop_closures: LoopClosure[];
  section_offsets: Array<[number, [number, number]]>;
  station_offsets: Array<[StationRef, [number, number, number]]>;
  station_comments_map: Array<[StationRef, string]>;
  imported_file_hashes: string[];
  imported_section_hashes: string[];
  validation: ValidationConfig;
  surface_elevation: number | null;
  /**
   * The project-level north reference a section inherits when it declares none
   * (#438). Absent on an envelope from a backend that predates it; the decoder
   * folds that onto `null`, which is the same "nothing recorded" the JSON path
   * means.
   */
  default_azimuth_reference?: AzimuthReference | null;
  /** Every distinct person any shot credits, once (v7); `CreditBinary` lists
   *  index into it. */
  credit_people: Person[];
}

/** One distinct survey credit of a section, people as `credit_people` indices. */
interface CreditBinary {
  surveyors: number[];
  explorers: number[];
  date: string | null;
}

interface SectionHeaderBinary {
  id: number;
  name: string | null;
  color: string;
  direction: SurveyDirection;
  quality: number;
  shot_offset: number;
  shot_count: number;
  next_shot_id: number;
  /** This section's own north reference (#438); absent/null inherits. */
  azimuth_reference?: AzimuthReference | null;
  /** The section's distinct shot credits, indexed by `shots_credit` (v7). */
  credits: CreditBinary[];
}

const NO_CREDIT = 0xFFFFFFFF;

/**
 * `ALREADY_OPEN` is the backend saying the requested path is the project it
 * already has open (`LoadError::already_open`). Every caller reaching this
 * rejection treats it as control flow rather than failure - `loadProject` short
 * circuits on it and switches the view instead of loading - so it is the one
 * code that must not reach the console as an error.
 */
function isAlreadyOpen(e: unknown): boolean {
  return typeof e === 'object' && e !== null && (e as { code?: unknown }).code === 'ALREADY_OPEN';
}

/**
 * Invoke `load_project_full_binary` and decode the response into the same
 * shape `load_project_full` returned. The frontend can swap one call for
 * the other without touching consumers.
 */
export async function loadProjectFullBinary(path: string): Promise<LoadProjectFullResultWithSoa> {
  const tInvoke = performance.now();
  let raw: ArrayBuffer | Uint8Array | unknown;
  try {
    raw = await invoke<ArrayBuffer>('load_project_full_binary', { path });
  } catch (e) {
    // Still rethrown either way - only the level changes, so the expected
    // short-circuit stops masquerading as a load failure in the debug log.
    if (isAlreadyOpen(e)) {
      console.info('[loadProjectFullBinary] project already open - caller will switch views');
    } else {
      console.error('[loadProjectFullBinary] IPC failed:', e);
    }
    throw e;
  }
  const tInvokeDone = performance.now();
  recordPerfTiming('binary IPC roundtrip', tInvokeDone - tInvoke);

  // Tauri 2 on Linux delivers Raw responses as ArrayBuffer through the
  // URI scheme protocol fast path. Older configurations or platforms might
  // hand back Uint8Array or a JSON-array fallback - handle all three so
  // the load works even if the IPC path changes shape.
  let bytes: Uint8Array;
  if (raw instanceof Uint8Array) {
    bytes = raw;
  } else if (raw instanceof ArrayBuffer) {
    bytes = new Uint8Array(raw);
  } else if (Array.isArray(raw)) {
    // Slow fallback - Tauri's eval-based callback path. Should not happen
    // for `load_project_full_binary` on Linux/Windows but keep the
    // conversion correct in case the platform changes.
    bytes = new Uint8Array(raw as number[]);
  } else {
    throw new Error(`load_project_full_binary returned unexpected type: ${typeof raw}`);
  }

  const tDecodeStart = performance.now();
  const envelope = msgpackDecode(bytes) as BinaryEnvelope;
  const tDecodeDone = performance.now();
  recordPerfTiming('msgpack decode', tDecodeDone - tDecodeStart);

  if (envelope.version !== EXPECTED_VERSION) {
    throw new Error(
      `load_project_full_binary version mismatch: expected ${EXPECTED_VERSION}, got ${envelope.version}. ` +
      `Rebuild backend or frontend so the binary wire format matches.`
    );
  }

  const tBuildStart = performance.now();
  const result = buildResult(envelope, bytes.byteLength);
  const tBuildDone = performance.now();
  recordPerfTiming('binary->JS materialize', tBuildDone - tBuildStart);

  return result;
}

/**
 * Decode a (already msgpack-decoded) binary envelope into the SoA + legacy
 * result shape. Exported for the load-path laziness regression test, which
 * drives this exact function so it exercises the real lazy-getter installation
 * rather than a hand-rolled stand-in. Production callers go through
 * `loadProjectFullBinary`.
 */
export function buildResult(env: BinaryEnvelope, totalBytes: number): LoadProjectFullResultWithSoa {
  // Build typed-array views over the bulk byte buffers. These views don't
  // copy; they just point into the underlying ArrayBuffers.
  const shotsF32 = bytesToFloat32(env.shots_floats);
  const shotsU32 = bytesToUint32(env.shots_uints);
  const shotsFlags = env.shots_flags;

  if (shotsF32.length !== env.shot_count * SHOT_FLOAT_FIELDS) {
    throw new Error(`shots_floats length mismatch: ${shotsF32.length} vs ${env.shot_count * SHOT_FLOAT_FIELDS}`);
  }
  if (shotsU32.length !== env.shot_count * SHOT_UINT_FIELDS) {
    throw new Error(`shots_uints length mismatch: ${shotsU32.length} vs ${env.shot_count * SHOT_UINT_FIELDS}`);
  }
  if (shotsFlags.length !== env.shot_count) {
    throw new Error(`shots_flags length mismatch: ${shotsFlags.length} vs ${env.shot_count}`);
  }

  // Index shot comments by global shot index for O(1) lookup.
  const shotCommentByIdx = new Map<number, string>();
  for (const [idx, c] of env.shot_comments) shotCommentByIdx.set(idx, c);

  // Per-shot credit indices (v7); `null` when no shot of the project has one.
  const shotsCredit = env.shots_credit.length > 0 ? bytesToUint32(env.shots_credit) : null;
  if (shotsCredit !== null && shotsCredit.length !== env.shot_count) {
    throw new Error(`shots_credit length mismatch: ${shotsCredit.length} vs ${env.shot_count}`);
  }
  const people = env.project_header.credit_people;

  // Build Section[] with a *lazy* `shots` getter backed by the typed
  // arrays we already have. Materializing all 75k Shot objects up
  // front committed bmalloc slabs for the Shot class that never
  // decommitted (the residue we measured at ~2 GB after close).
  // Lazy materialization means the slab only commits if a consumer
  // actually iterates `section.shots`; sections nobody touches stay
  // typed-array-only.
  //
  // Each Section's shots getter caches the materialized array after
  // first access (consumer code expects identity stability - repeat
  // calls return the same array). The cache is per-section, so the
  // memory cost is bounded by which sections are accessed × shots in
  // those sections. For typical UI use (one section at a time in
  // DataTable, single shots in DetailPanel) this is ~100s of shots
  // materialized vs the legacy 75 000.
  const sections: Section[] = env.project_header.sections.map((sh) => {
    const materializeShots = (): Shot[] => {
      const shots: Shot[] = new Array(sh.shot_count);
      // The section's distinct credits, resolved once and SHARED by every shot
      // that carries them - treat a shot's `credit` as immutable.
      const credits: SurveyCredit[] = sh.credits.map((c) => decodeCredit(c, people));
      for (let j = 0; j < sh.shot_count; j++) {
        const globalIdx = sh.shot_offset + j;
        const fBase = globalIdx * SHOT_FLOAT_FIELDS;
        const uBase = globalIdx * SHOT_UINT_FIELDS;
        const shot_id = shotsU32[uBase + 1]!;
        const markerRaw = shotsU32[uBase + 2]!;
        shots[j] = {
          id: shot_id,
          azimuth_in: shotsF32[fBase]!,
          azimuth_out: shotsF32[fBase + 1]!,
          length: shotsF32[fBase + 2]!,
          depth_in: shotsF32[fBase + 3]!,
          depth_out: shotsF32[fBase + 4]!,
          pitch_in: nanToNull(shotsF32[fBase + 5]!),
          pitch_out: nanToNull(shotsF32[fBase + 6]!),
          left: nanToNull(shotsF32[fBase + 7]!),
          right: nanToNull(shotsF32[fBase + 8]!),
          up: nanToNull(shotsF32[fBase + 9]!),
          down: nanToNull(shotsF32[fBase + 10]!),
          temperature: nanToNull(shotsF32[fBase + 11]!),
          comment: shotCommentByIdx.get(globalIdx) ?? null,
          kind: shotKindFromFlags(shotsFlags[globalIdx]!),
          marker_index: markerRaw === 0xFFFFFFFF ? null : markerRaw,
        };
        const creditIdx = shotsCredit === null ? NO_CREDIT : shotsCredit[globalIdx]!;
        const credit = creditIdx === NO_CREDIT ? undefined : credits[creditIdx];
        if (credit !== undefined) shots[j]!.credit = credit;
      }
      return shots;
    };
    // `Object.defineProperty` for `shots` so we can install a getter
    // without breaking the `Section` interface (which declares it as
    // a plain `Shot[]`). Consumers read `section.shots` exactly as
    // before; the materialization happens transparently on first read.
    const section = {
      id: sh.id,
      name: sh.name,
      color: sh.color,
      direction: sh.direction,
      quality: sh.quality,
      next_shot_id: sh.next_shot_id,
      azimuth_reference: sh.azimuth_reference ?? null,
    } as Section;
    // Define-once lazy getter: on first read it materializes the Shot[] and
    // *replaces itself* with a plain data property holding that array. This
    // (a) keeps identity stable (later reads return the same array),
    // (b) drops the getter/closure overhead after materialization, and
    // (c) makes the state externally observable for the load-path lazy guard:
    //     an untouched section's descriptor is an accessor (`get` defined),
    //     a materialized one is a data property (`value` defined). The
    //     regression test and the E2E harness classify sections that way.
    Object.defineProperty(section, 'shots', {
      get() {
        const shots = materializeShots();
        noteShotMaterialization();
        Object.defineProperty(section, 'shots', {
          value: shots,
          writable: true,
          enumerable: true,
          configurable: true,
        });
        return shots;
      },
      enumerable: true,
      configurable: true,
    });
    return section;
  });

  // Reconstruct the section_offsets record: Vec<(SectionId, (f64,f64))> -> Record<number, [number, number]>
  const section_offsets: Record<number, [number, number]> = {};
  for (const [sid, [x, y]] of env.project_header.section_offsets) {
    section_offsets[sid] = [x, y];
  }

  // Note: station_offsets and station_comments_map are on Project but TS
  // type generator skips them (#[ts(skip)]). They're carried in the
  // binary but we don't surface them on the LoadProjectFullResult.project
  // here because the existing JSON path doesn't either.

  const project: Project = {
    name: env.project_header.name,
    location: env.project_header.location,
    created_at: env.project_header.created_at,
    modified_at: env.project_header.modified_at,
    sections,
    station_merges: env.project_header.station_merges,
    zero_datums: env.project_header.zero_datums,
    loop_closures: env.project_header.loop_closures,
    file_path: env.project_header.file_path,
    next_section_id: env.project_header.next_section_id,
    section_offsets,
    imported_file_hashes: env.project_header.imported_file_hashes,
    imported_section_hashes: env.project_header.imported_section_hashes,
    next_closure_id: BigInt(env.project_header.next_closure_id as unknown as number | bigint),
    validation: env.project_header.validation,
    surface_elevation: env.project_header.surface_elevation,
    // Spread rather than assigned: the generated `Project` declares this
    // optional and the tsconfig sets `exactOptionalPropertyTypes`, so an absent
    // envelope field has to stay ABSENT - writing `undefined` into it is a type
    // error, and writing `null` claims a value the type does not admit.
    ...(env.project_header.default_azimuth_reference
      ? { default_azimuth_reference: env.project_header.default_azimuth_reference }
      : {}),
  };

  // Stations: typed-array views over the binary buffers.
  const stationsF64 = bytesToFloat64(env.stations_floats);
  const stationsU32 = bytesToUint32(env.stations_uints);
  const stationsFlags = env.stations_flags;

  if (stationsF64.length !== env.station_count * STATION_FLOAT_FIELDS) {
    throw new Error(`stations_floats length mismatch: ${stationsF64.length} vs ${env.station_count * STATION_FLOAT_FIELDS}`);
  }
  if (stationsU32.length !== env.station_count * STATION_UINT_FIELDS) {
    throw new Error(`stations_uints length mismatch: ${stationsU32.length} vs ${env.station_count * STATION_UINT_FIELDS}`);
  }

  // Index sparse extras by station index for O(1) lookup.
  const gpsByIdx = new Map<number, GpsCoordinates>();
  for (const [i, g] of env.station_gps) gpsByIdx.set(i, g);
  const mergedByIdx = new Map<number, StationRef[]>();
  for (const [i, m] of env.station_merged_with) mergedByIdx.set(i, m);
  const closuresByIdx = new Map<number, ClosureRef[]>();
  for (const [i, c] of env.station_closures_with) closuresByIdx.set(i, c);
  const commentByIdx = new Map<number, string>();
  for (const [i, c] of env.station_comments) commentByIdx.set(i, c);
  const nameByIdx = new Map<number, string>();
  for (const [i, n] of env.station_names) nameByIdx.set(i, n);

  // Build the new SoA layout. This reuses the same typed-array views we
  // already created above; per-shot/station numeric fields aren't copied,
  // we just expose them under field-named arrays so consumers don't have
  // to know about the multiplexed `shots_floats`/`shots_uints` layout.
  // No additional allocations beyond:
  //   - 12 per-field Float32 views over the same `shots_floats` buffer
  //     (and similar for stations) - DataView-style, zero copy
  //   - 1 Uint32Array `shotSectionIds` (4 bytes/shot, derived from the
  //     section metadata + offsets - we have to materialize this because
  //     it's not in the wire format)
  //   - 1 Uint8Array `shotFlags` mapping the wire's u32 marker_or_max
  //     into the Int32Array `shotMarkerIndex` and bit-packed flags
  const surveyData = buildSurveyData(env, shotsF32, shotsU32, shotsFlags, stationsF64, stationsU32, stationsFlags, shotCommentByIdx, gpsByIdx, mergedByIdx, closuresByIdx, commentByIdx, nameByIdx);

  // Stations: a *lazy* `Station[]` view over `surveyData`, not an eager
  // ~89k-object array. The old eager loop (removed here) materialized one
  // `Station` (plus nested `station_ref` / `coordinates` objects) per
  // station and held them for the project's whole lifetime - a pure
  // duplicate of `surveyData.stationCoords` / `stationSectionIds` /
  // `stationIndices` / the sparse extras maps. `lazyStationArray`
  // reconstructs the exact same `Station` shape on index access via
  // `getStation`, so consumers that still take a `Station[]` are unchanged,
  // but nothing is retained unless a consumer actually reads an element.
  const stations = lazyStationArray(surveyData);

  // For visibility in the perf panel, also record the wire size.
  recordPerfTiming('binary payload size (KB)', totalBytes / 1024);

  return {
    loadingId: typeof env.loading_id === 'bigint' ? env.loading_id : BigInt(env.loading_id),
    project,
    stations,
    stats: env.stats,
    warnings: env.warnings,
    // The Rust encoder already ships the computed frozen shots in the envelope;
    // surface them so the store can populate `frozenShots` on a binary load
    // instead of leaving the previous project's stale set until the next refresh
    // (issue #57 / Codex). Shape mirrors Rust's Vec<FrozenShotRef>.
    frozen_shots: env.frozen_shots as FrozenShotRef[],
    displaySettings: env.display_settings,
    // #270: the SpeleoDB read-only gate is resolved from this, synchronously,
    // before the project view renders - never from the async link-status refresh.
    isRemote: env.is_remote,
    surveyData,
  };
}

/**
 * Project the wire-format typed-array buffers into the field-named
 * `SurveyData` layout. Zero-copy where the layout allows it; the
 * fields with stride 1 (one value per shot) become `subarray` views
 * over the multiplexed `shots_floats` buffer, while the per-field
 * unstrided ones are unpacked into their own typed array.
 *
 * Choosing `subarray` views vs unpacked arrays: `subarray` would be
 * stride-N (e.g. azimuth_in at indices 0, 12, 24, …) which JS doesn't
 * support directly. So we unpack to dense per-field arrays. Cost is
 * one `Float32Array(shot_count)` per field × 12 fields ≈ 3.5 MB for
 * 75k shots, which is still ~50× smaller than the legacy 75k Shot
 * objects we materialized above.
 */
function buildSurveyData(
  env: BinaryEnvelope,
  shotsF32: Float32Array,
  shotsU32: Uint32Array,
  shotsFlags: Uint8Array,
  stationsF64: Float64Array,
  stationsU32: Uint32Array,
  stationsFlags: Uint8Array,
  shotCommentByIdx: Map<number, string>,
  stationGps: Map<number, GpsCoordinates>,
  stationMerged: Map<number, StationRef[]>,
  stationClosures: Map<number, ClosureRef[]>,
  stationComments: Map<number, string>,
  stationNames: Map<number, string>,
): SurveyData {
  const N = env.shot_count;
  const shotIds = new Uint32Array(N);
  const shotSectionIds = new Uint32Array(N);
  const shotAzimuthIn = new Float32Array(N);
  const shotAzimuthOut = new Float32Array(N);
  const shotLength = new Float32Array(N);
  const shotDepthIn = new Float32Array(N);
  const shotDepthOut = new Float32Array(N);
  const shotPitchIn = new Float32Array(N);
  const shotPitchOut = new Float32Array(N);
  const shotLeft = new Float32Array(N);
  const shotRight = new Float32Array(N);
  const shotUp = new Float32Array(N);
  const shotDown = new Float32Array(N);
  const shotTemperature = new Float32Array(N);
  const shotFlags = new Uint8Array(N);
  const shotMarkerIndex = new Int32Array(N);

  // The wire format multiplexes f32 fields at SHOT_FLOAT_FIELDS stride;
  // unpack into field-aligned arrays. Single tight loop touches each
  // shot's row exactly once.
  for (let i = 0; i < N; i++) {
    const fBase = i * SHOT_FLOAT_FIELDS;
    shotAzimuthIn[i]   = shotsF32[fBase]!;
    shotAzimuthOut[i]  = shotsF32[fBase + 1]!;
    shotLength[i]      = shotsF32[fBase + 2]!;
    shotDepthIn[i]     = shotsF32[fBase + 3]!;
    shotDepthOut[i]    = shotsF32[fBase + 4]!;
    shotPitchIn[i]     = shotsF32[fBase + 5]!;
    shotPitchOut[i]    = shotsF32[fBase + 6]!;
    shotLeft[i]        = shotsF32[fBase + 7]!;
    shotRight[i]       = shotsF32[fBase + 8]!;
    shotUp[i]          = shotsF32[fBase + 9]!;
    shotDown[i]        = shotsF32[fBase + 10]!;
    shotTemperature[i] = shotsF32[fBase + 11]!;

    const uBase = i * SHOT_UINT_FIELDS;
    // Both columns are u32 on the wire and u32 in the SoA, so copy them
    // verbatim. They used to be coerced with `| 0` into Int32Arrays, which
    // wrapped every shot id past 2^31 (reachable from section 21_474 up, since
    // Rust mints `section_id * 100_000 + sequence_order`) to a negative number
    // while the lazy `section.shots` materializer below read the same slot
    // unsigned - the two views of one id stopped comparing equal (#555).
    shotSectionIds[i] = shotsU32[uBase]!;           // section_id
    shotIds[i]        = shotsU32[uBase + 1]!;       // shot.id
    const markerRaw   = shotsU32[uBase + 2]!;
    shotMarkerIndex[i] = markerRaw === 0xFFFFFFFF ? SHOT_MARKER_NULL : (markerRaw | 0);

    // Wire format v3 packs the ShotKind discriminant into bits 0-1 (#27), and
    // the SoA flags byte uses the same layout, so this is a straight copy of
    // the low two bits.
    shotFlags[i] = shotsFlags[i]! & SHOT_FLAG_KIND_MASK;
  }

  const sections: SectionMetadata[] = env.project_header.sections.map((sh) => ({
    id: sh.id,
    name: sh.name,
    color: sh.color,
    direction: sh.direction,
    quality: sh.quality,
    next_shot_id: sh.next_shot_id,
    azimuth_reference: sh.azimuth_reference ?? null,
    shotOffset: sh.shot_offset,
    shotCount: sh.shot_count,
  }));

  // ─── Stations ──────────────────────────────────────────────────
  const SN = env.station_count;
  const stationSectionIds = new Uint32Array(SN);
  const stationIndices = new Uint32Array(SN);
  // Reuse the existing stationsF64 view directly - it's already
  // [x0,y0,z0,x1,y1,z1,…] which matches our stationCoords stride-3.
  // Take a copy though, because the underlying buffer is borrowed from
  // msgpack's decoded Uint8Array which can be reused/recycled.
  const stationCoords = new Float64Array(stationsF64);
  const stationFlagsOut = new Uint8Array(SN);

  for (let i = 0; i < SN; i++) {
    const uBase = i * STATION_UINT_FIELDS;
    // u32 in, u32 out - see the shot columns above for why the old `| 0`
    // coercion was wrong (#555).
    stationSectionIds[i] = stationsU32[uBase]!;
    stationIndices[i] = stationsU32[uBase + 1]!;
    stationFlagsOut[i] = (stationsFlags[i]! & 1) !== 0 ? STATION_FLAG_IS_ZERO_DATUM : 0;
  }

  return {
    sections,
    shotCount: N,
    shotIds,
    shotSectionIds,
    shotAzimuthIn,
    shotAzimuthOut,
    shotLength,
    shotDepthIn,
    shotDepthOut,
    shotPitchIn,
    shotPitchOut,
    shotLeft,
    shotRight,
    shotUp,
    shotDown,
    shotTemperature,
    shotFlags,
    shotMarkerIndex,
    shotComments: shotCommentByIdx,
    stationCount: SN,
    stationSectionIds,
    stationIndices,
    stationCoords,
    stationFlags: stationFlagsOut,
    stationGps,
    stationMergedWith: stationMerged,
    stationClosuresWith: stationClosures,
    stationComments,
    stationNames,
  };
}

/** Convert a flat byte buffer to a Float32Array view. Aligns the buffer
 *  if needed (msgpack delivers Uint8Array with arbitrary byteOffset). */
/** A section credit with its people resolved; an index past the table is dropped. */
function decodeCredit(credit: CreditBinary, people: readonly Person[]): SurveyCredit {
  const resolve = (ids: readonly number[]): Person[] =>
    ids.map((id) => people[id]).filter((p): p is Person => p !== undefined);
  return { surveyors: resolve(credit.surveyors), explorers: resolve(credit.explorers), date: credit.date ?? null };
}

function bytesToFloat32(bytes: Uint8Array): Float32Array {
  if ((bytes.byteOffset & 3) === 0) {
    return new Float32Array(bytes.buffer, bytes.byteOffset, bytes.byteLength / 4);
  }
  // Misaligned: copy into a fresh buffer.
  const copy = new Uint8Array(bytes.byteLength);
  copy.set(bytes);
  return new Float32Array(copy.buffer, 0, bytes.byteLength / 4);
}

function bytesToFloat64(bytes: Uint8Array): Float64Array {
  if ((bytes.byteOffset & 7) === 0) {
    return new Float64Array(bytes.buffer, bytes.byteOffset, bytes.byteLength / 8);
  }
  const copy = new Uint8Array(bytes.byteLength);
  copy.set(bytes);
  return new Float64Array(copy.buffer, 0, bytes.byteLength / 8);
}

function bytesToUint32(bytes: Uint8Array): Uint32Array {
  if ((bytes.byteOffset & 3) === 0) {
    return new Uint32Array(bytes.buffer, bytes.byteOffset, bytes.byteLength / 4);
  }
  const copy = new Uint8Array(bytes.byteLength);
  copy.set(bytes);
  return new Uint32Array(copy.buffer, 0, bytes.byteLength / 4);
}

function nanToNull(v: number): number | null {
  return Number.isNaN(v) ? null : v;
}
