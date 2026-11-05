/**
 * Determinism of the binary-envelope decode (#576 fuzzers, FZ-DET-TS). See
 * `src/test/determinism.ts` for the kit and the env knob.
 *
 * Drives the real decoder through both IPC entry points (`loadProjectFullBinary`
 * and the refresh `fetchProjectStateBinary`) with synthetic envelopes packed by
 * the test helper `projectStateEnvelope`.
 *
 * Asserted, per seed:
 *  - the same bytes decoded twice give identical results - deep-equal, and the
 *    serialized project / stations byte-identical - also when OTHER envelopes
 *    were decoded (and their lazy shots materialized) in between and the first
 *    result's lazy `section.shots` is only read afterwards: nothing in the
 *    decode is shared across calls (loadProjectBinary.ts 422-428, 713-715);
 *  - the load and the refresh decode the same bytes to the same state;
 *  - the sparse pair lists (`shot_comments`, `station_gps`,
 *    `station_merged_with`, `station_closures_with`, `station_comments`) and the
 *    `section_offsets` pairs - all serialized from MAPS by the backend, so their
 *    wire order is not a contract - permuted on the wire: the decoded project
 *    and stations serialize byte-identically and the SoA maps hold the same
 *    entries.
 *
 * NOT asserted: the insertion ORDER of the SoA sparse maps after a wire
 * permutation - they are filled in wire order (loadProjectBinary.ts 411-412,
 * 550-557) and every consumer only calls `get` (the renderer, `immovableStations`,
 * the drag preview). Arrays the decoder passes through verbatim (sections,
 * merges, datums, closures, warnings, frozen shots: 511-527, 587-597) keep the
 * backend's order by contract and are not permuted.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { decode, encode } from '@msgpack/msgpack';

import { projectStateEnvelope, type ProjectStateLike } from '../../test/projectStateEnvelope';
import { invokeIpcMock as invokeIpc } from '../../test/factories/ipc';
import {
  determinismSeeds,
  mulberry32,
  pick,
  randInt,
  seededDiff,
  seededTextDiff,
  shuffled,
  type Rng,
} from '../../test/determinism';

vi.mock('../ipc');
const invoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', async () => (await import('../../test/factories/tauri')).tauriCore(invoke));

const { fetchProjectStateBinary, loadProjectFullBinary } = await import('../loadProjectBinary');
type Decoded = Awaited<ReturnType<typeof loadProjectFullBinary>>;

const SEEDS = determinismSeeds();

/** A synthetic project state: CaveA.. sections, every shot kind, comments,
 *  datums with GPS, merges and closure refs, section offsets. */
function randomState(rng: Rng): ProjectStateLike {
  const sectionCount = randInt(rng, 1, 4);
  const ids = shuffled(Array.from({ length: 50 }, (_, i) => i + 1), rng).slice(0, sectionCount);
  const sections: ProjectStateLike['project']['sections'] = [];
  const stations: ProjectStateLike['stations'] = [];
  const offsets: Record<string, [number, number]> = {};
  for (const [s, id] of ids.entries()) {
    const letter = String.fromCharCode(65 + s);
    const n = randInt(rng, 0, 7);
    const shots = Array.from({ length: n }, (_, j) => ({
      id: id * 100_000 + j,
      azimuth_in: Math.round(rng() * 3600) / 10,
      azimuth_out: Math.round(rng() * 3600) / 10,
      length: Math.round(rng() * 1000) / 100,
      depth_in: Math.round(rng() * 400) / 10,
      depth_out: Math.round(rng() * 400) / 10,
      pitch_in: rng() < 0.5 ? null : Math.round(rng() * 900) / 10 - 45,
      left: rng() < 0.5 ? null : rng() * 3,
      right: rng() < 0.5 ? null : rng() * 3,
      temperature: rng() < 0.7 ? null : 24.5,
      comment: rng() < 0.3 ? `Shot${letter}${j} restriction` : null,
      kind: pick(rng, ['Normal', 'Normal', 'Jump', 'Reference']),
      marker_index: rng() < 0.8 ? null : randInt(rng, 0, 9),
    }));
    sections.push({ id, name: `Cave${letter}`, color: pick(rng, ['#e74c3c', '#3498db']), direction: 'in', shots });
    if (rng() < 0.6) offsets[String(id)] = [rng() * 10, rng() * 10];
    for (let i = 0; i <= n; i++) {
      stations.push({
        station_ref: { section_id: id, station_index: i },
        coordinates: { x: rng() * 100, y: rng() * 100, z: -rng() * 30 },
        is_zero_datum: i === 0 && rng() < 0.5,
        ...(rng() < 0.2 ? { gps_coordinates: { latitude: 20 + rng(), longitude: -87 - rng(), depth_reference: 0 } } : {}),
        ...(rng() < 0.3 ? { comment: `Station${letter}${i} tie-in` } : {}),
      });
    }
  }
  // Merges / closure refs between random stations (only their wire lists matter here).
  for (const st of stations) {
    if (rng() < 0.2) st.merged_with = [pick(rng, stations).station_ref];
    if (rng() < 0.15) {
      st.closures_with = [{ closure_id: randInt(rng, 1, 5), station_ref: pick(rng, stations).station_ref, via_station: null, is_transitive: false }];
    }
  }
  return {
    project: { name: 'CaveSystem', file_path: '/caves/synthetic.stalx', sections, section_offsets: offsets },
    stations,
    stats: { section_count: sections.length, shot_count: 0, total_length: 0, min_depth: 0, max_depth: 0, station_count: stations.length, loop_count: 0, penetration_distance: 0 },
    frozen_shots: [],
  };
}

/** The same envelope with every Map-derived pair list permuted on the wire. */
function permutedWire(bytes: ArrayBuffer, rng: Rng): ArrayBuffer {
  const env = decode(new Uint8Array(bytes)) as Record<string, unknown> & { project_header: Record<string, unknown> };
  for (const key of ['shot_comments', 'station_gps', 'station_merged_with', 'station_closures_with', 'station_comments']) {
    env[key] = shuffled(env[key] as unknown[], rng);
  }
  env.project_header.section_offsets = shuffled(env.project_header.section_offsets as unknown[], rng);
  return encode(env).slice().buffer;
}

/** Everything a decode produced, in comparable form. Reading `project` through
 *  JSON materializes every lazy `section.shots`. */
function view(r: Decoded, mapOrder: 'ordered' | 'content') {
  const stations = Array.from({ length: r.stations.length }, (_, i) => r.stations[i]);
  const projectJson = JSON.stringify(r.project);
  const sd = r.surveyData;
  const maps: Record<string, ReadonlyMap<number, unknown>> = {
    shotComments: sd.shotComments,
    stationGps: sd.stationGps,
    stationMergedWith: sd.stationMergedWith,
    stationClosuresWith: sd.stationClosuresWith,
    stationComments: sd.stationComments,
  };
  // Every typed column (and the section metadata); the maps are compared apart.
  const columns: Record<string, unknown> = { ...sd };
  for (const key of Object.keys(maps)) delete columns[key];
  return {
    projectJson,
    stationsJson: JSON.stringify(stations),
    columns,
    maps: mapOrder === 'ordered' ? maps : Object.fromEntries(Object.entries(maps).map(([k, m]) => [k, sortedEntries(m)])),
    rest: {
      loadingId: r.loadingId,
      stats: r.stats,
      warnings: r.warnings,
      frozen_shots: r.frozen_shots,
      displaySettings: r.displaySettings,
      isRemote: r.isRemote,
      newerSchemaVersion: r.newerSchemaVersion,
      speleoLinkUnreadable: r.speleoLinkUnreadable,
      unreadableProjectSettings: r.unreadableProjectSettings,
      loadRepairs: r.loadRepairs,
    },
  };
}

function sortedEntries<V>(m: ReadonlyMap<number, V>): [number, V][] {
  return [...m.entries()].sort((a, b) => a[0] - b[0]);
}

/** Every decode gets its own copy of the bytes, the way each IPC answer does. */
async function load(bytes: ArrayBuffer): Promise<Decoded> {
  invoke.mockResolvedValueOnce(bytes.slice(0));
  return loadProjectFullBinary('/caves/synthetic.stalx');
}

describe('binary envelope decode is deterministic (FZ-DET-TS)', () => {
  beforeEach(() => {
    invoke.mockReset();
    invokeIpc.mockReset();
  });

  it('same bytes twice, other envelopes decoded in between -> identical results', async () => {
    for (const seed of SEEDS) {
      const bytes = projectStateEnvelope(randomState(mulberry32(seed)));
      // Decode, but read nothing yet: the lazy shots are read only after other decodes.
      const first = await load(bytes);
      for (let k = 1; k <= 2; k++) {
        const other = await load(projectStateEnvelope(randomState(mulberry32(seed + k * 1_000_003))));
        JSON.stringify(other.project);
      }
      const second = await load(bytes);
      const a = view(first, 'ordered');
      const b = view(second, 'ordered');
      expect(seededTextDiff(seed, 'project JSON', a.projectJson, b.projectJson)).toBeNull();
      expect(seededTextDiff(seed, 'stations JSON', a.stationsJson, b.stationsJson)).toBeNull();
      expect(seededDiff(seed, 'decode', a, b)).toBeNull();

      // The refresh decodes the same bytes to the same state.
      invokeIpc.mockResolvedValueOnce(bytes.slice(0));
      const refreshed = await fetchProjectStateBinary();
      expect(refreshed).not.toBeNull();
      const r = view({ ...first, ...refreshed! }, 'ordered');
      expect(seededTextDiff(seed, 'refresh project JSON', a.projectJson, r.projectJson)).toBeNull();
      expect(seededDiff(seed, 'refresh SoA', { columns: a.columns, maps: a.maps }, { columns: r.columns, maps: r.maps })).toBeNull();
    }
  });

  it('Map-derived pair lists permuted on the wire -> identical decode', async () => {
    for (const seed of SEEDS) {
      const bytes = projectStateEnvelope(randomState(mulberry32(seed)));
      const reference = view(await load(bytes), 'content');
      for (let round = 0; round < 3; round++) {
        const variant = view(await load(permutedWire(bytes, mulberry32(seed * 31 + round))), 'content');
        expect(seededTextDiff(seed, `project JSON (round ${round})`, reference.projectJson, variant.projectJson)).toBeNull();
        expect(seededTextDiff(seed, `stations JSON (round ${round})`, reference.stationsJson, variant.stationsJson)).toBeNull();
        expect(seededDiff(seed, `decode (round ${round})`, reference, variant)).toBeNull();
      }
    }
  });
});
