/**
 * Test helper (#576 A9-D2): pack a JSON `ProjectState`-shaped value into the
 * binary envelope `get_project_state_binary` answers with - the load's layout
 * (`core/load_binary.rs`, decoded by `src/utils/loadProjectBinary.ts`) - as the
 * bytes the IPC delivers. A store test that used to serve the JSON
 * `get_project_state` serves this instead, so the refresh it drives runs the real
 * decoder. Fields a fixture leaves out default the way the Playwright mock's
 * packer defaults them (`e2e-playwright/mocks/tauri-ipc.ts`). Synthetic data only.
 */
import { encode } from '@msgpack/msgpack';

interface ShotLike {
  id: number;
  azimuth_in?: number;
  azimuth_out?: number;
  length?: number;
  depth_in?: number;
  depth_out?: number;
  pitch_in?: number | null;
  pitch_out?: number | null;
  left?: number | null;
  right?: number | null;
  up?: number | null;
  down?: number | null;
  temperature?: number | null;
  comment?: string | null;
  kind?: string;
  marker_index?: number | null;
}

interface SectionLike {
  id: number;
  name?: string | null;
  surveyor?: string | null;
  date?: string | null;
  color?: string;
  direction?: string;
  quality?: number;
  next_shot_id?: number;
  azimuth_reference?: unknown;
  shots?: ShotLike[];
}

interface ProjectLike {
  name?: string;
  location?: string | null;
  created_at?: string;
  modified_at?: string;
  file_path?: string | null;
  next_section_id?: number;
  next_closure_id?: number;
  sections: SectionLike[];
  station_merges?: unknown[];
  zero_datums?: unknown[];
  loop_closures?: unknown[];
  section_offsets?: Record<string, [number, number]> | Array<[number, [number, number]]>;
  imported_file_hashes?: string[];
  imported_section_hashes?: string[];
  validation?: unknown;
  surface_elevation?: number | null;
  default_azimuth_reference?: unknown;
}

interface StationLike {
  station_ref: { section_id: number; station_index: number };
  coordinates?: { x: number; y: number; z: number };
  is_zero_datum?: boolean;
  gps_coordinates?: unknown;
  merged_with?: unknown[];
  closures_with?: unknown[];
  comment?: string;
}

export interface ProjectStateLike {
  project: ProjectLike;
  stations: StationLike[];
  stats?: unknown;
  frozen_shots?: unknown[];
}

const SHOT_FLOAT_STRIDE = 48;
const SHOT_UINT_STRIDE = 12;
const STATION_FLOAT_STRIDE = 24;
const STATION_UINT_STRIDE = 8;
const KIND_BITS: Record<string, number> = { Normal: 0, Jump: 1, Reference: 2 };

/** The envelope as msgpack bytes in an exactly-sized ArrayBuffer (what Tauri hands
 *  the webview for a raw response). `overrides` replaces top-level envelope keys,
 *  e.g. `{ version: 4 }` or the load-only fields a refresh must ignore. */
export function projectStateEnvelope(
  state: ProjectStateLike,
  overrides: Record<string, unknown> = {},
): ArrayBuffer {
  const { project, stations } = state;
  const nan = (v: number | null | undefined): number => (v === null || v === undefined ? NaN : v);

  const shotCount = project.sections.reduce((n, s) => n + (s.shots?.length ?? 0), 0);
  const shotsFloats = new Uint8Array(shotCount * SHOT_FLOAT_STRIDE);
  const shotsUints = new Uint8Array(shotCount * SHOT_UINT_STRIDE);
  const shotsFlags = new Uint8Array(shotCount);
  const fv = new DataView(shotsFloats.buffer);
  const uv = new DataView(shotsUints.buffer);
  const shotComments: Array<[number, string]> = [];
  const sections: Array<Record<string, unknown>> = [];
  let cursor = 0;
  for (const section of project.sections) {
    const shotOffset = cursor;
    for (const shot of section.shots ?? []) {
      const f = cursor * SHOT_FLOAT_STRIDE;
      const floats = [
        shot.azimuth_in ?? 0, shot.azimuth_out ?? 0, shot.length ?? 0, shot.depth_in ?? 0,
        shot.depth_out ?? 0, nan(shot.pitch_in), nan(shot.pitch_out), nan(shot.left),
        nan(shot.right), nan(shot.up), nan(shot.down), nan(shot.temperature),
      ];
      floats.forEach((v, i) => fv.setFloat32(f + i * 4, v, true));
      const u = cursor * SHOT_UINT_STRIDE;
      uv.setUint32(u, section.id >>> 0, true);
      uv.setUint32(u + 4, shot.id >>> 0, true);
      uv.setUint32(u + 8, shot.marker_index == null ? 0xffffffff : shot.marker_index >>> 0, true);
      shotsFlags[cursor] = KIND_BITS[shot.kind ?? 'Normal'] ?? 0;
      if (shot.comment) shotComments.push([cursor, shot.comment]);
      cursor++;
    }
    const count = cursor - shotOffset;
    sections.push({
      id: section.id,
      name: section.name ?? null,
      surveyor: section.surveyor ?? null,
      date: section.date ?? null,
      color: section.color ?? '#888888',
      direction: section.direction ?? 'in',
      quality: section.quality ?? 1,
      shot_offset: shotOffset,
      shot_count: count,
      next_shot_id: section.next_shot_id ?? section.id * 100_000 + count,
      azimuth_reference: section.azimuth_reference ?? null,
    });
  }

  const stationsFloats = new Uint8Array(stations.length * STATION_FLOAT_STRIDE);
  const stationsUints = new Uint8Array(stations.length * STATION_UINT_STRIDE);
  const stationsFlags = new Uint8Array(stations.length);
  const sfv = new DataView(stationsFloats.buffer);
  const suv = new DataView(stationsUints.buffer);
  const stationGps: Array<[number, unknown]> = [];
  const stationMerged: Array<[number, unknown[]]> = [];
  const stationClosures: Array<[number, unknown[]]> = [];
  const stationComments: Array<[number, string]> = [];
  stations.forEach((s, i) => {
    const f = i * STATION_FLOAT_STRIDE;
    sfv.setFloat64(f, s.coordinates?.x ?? 0, true);
    sfv.setFloat64(f + 8, s.coordinates?.y ?? 0, true);
    sfv.setFloat64(f + 16, s.coordinates?.z ?? 0, true);
    suv.setUint32(i * STATION_UINT_STRIDE, s.station_ref.section_id >>> 0, true);
    suv.setUint32(i * STATION_UINT_STRIDE + 4, s.station_ref.station_index >>> 0, true);
    stationsFlags[i] = s.is_zero_datum ? 1 : 0;
    if (s.gps_coordinates) stationGps.push([i, s.gps_coordinates]);
    if (s.merged_with?.length) stationMerged.push([i, s.merged_with]);
    if (s.closures_with?.length) stationClosures.push([i, s.closures_with]);
    if (s.comment) stationComments.push([i, s.comment]);
  });

  const offsets = project.section_offsets ?? {};
  const envelope = {
    version: 5,
    // The load-only fields at the no-news values `encode_refresh` sends.
    loading_id: 0,
    is_remote: false,
    newer_schema_version: null,
    speleo_link_unreadable: false,
    unreadable_project_settings: [],
    load_repairs: { shots: 0, section_offsets: 0, station_offsets: 0, gps_points: 0 },
    project_header: {
      name: project.name ?? 'Project',
      location: project.location ?? null,
      created_at: project.created_at ?? '2026-01-01T00:00:00Z',
      modified_at: project.modified_at ?? '2026-01-01T00:00:00Z',
      file_path: project.file_path ?? null,
      next_section_id: project.next_section_id ?? project.sections.length + 1,
      next_closure_id: project.next_closure_id ?? 1,
      sections,
      station_merges: project.station_merges ?? [],
      zero_datums: project.zero_datums ?? [],
      loop_closures: project.loop_closures ?? [],
      section_offsets: Array.isArray(offsets)
        ? offsets
        : Object.entries(offsets).map(([k, v]) => [Number(k), v]),
      station_offsets: [],
      station_comments_map: [],
      imported_file_hashes: project.imported_file_hashes ?? [],
      imported_section_hashes: project.imported_section_hashes ?? [],
      validation: project.validation ?? {},
      surface_elevation: project.surface_elevation ?? null,
      default_azimuth_reference: project.default_azimuth_reference ?? null,
    },
    shot_count: shotCount,
    shots_floats: shotsFloats,
    shots_uints: shotsUints,
    shots_flags: shotsFlags,
    shot_comments: shotComments,
    station_count: stations.length,
    stations_floats: stationsFloats,
    stations_uints: stationsUints,
    stations_flags: stationsFlags,
    station_gps: stationGps,
    station_merged_with: stationMerged,
    station_closures_with: stationClosures,
    station_comments: stationComments,
    stats: state.stats ?? null,
    warnings: [],
    display_settings: {},
    frozen_shots: state.frozen_shots ?? [],
    ...overrides,
  };
  // `encode` may return a view into a larger pooled buffer; `slice()` copies
  // exactly the encoded bytes.
  return encode(envelope).slice().buffer;
}
