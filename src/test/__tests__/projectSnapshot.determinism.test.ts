/**
 * Determinism of the undo/redo fuzzer's snapshot canonicalisation (#576 fuzzers,
 * FZ-DET-TS): `e2e/agent/lib/fuzzOracleCore.ts` (the canonical snapshot, the
 * targeting digest, the invariant checks) and `e2e/agent/lib/projectSnapshot.ts`
 * (the diff reporter and the shot-identity delta). See `src/test/determinism.ts`.
 *
 * It lives here rather than next to `projectSnapshot.test.ts` because the
 * harness tree is owned elsewhere; both modules are import-free, so the app
 * program can take them (as `src/utils/__tests__/exportPathMirror.test.ts`
 * already does for `exportOracles.ts`).
 *
 * The canonical snapshot's whole job is to make two equal states compare equal
 * whatever ORDER their object keys arrived in (`Project::section_offsets` is a
 * HashMap: fuzzOracleCore.ts 47-50). Asserted, per seed, on a synthetic
 * `get_project_state` payload:
 *  - the same state twice (another state canonicalised in between) gives the
 *    same canonical string, id-agnostic string, digest and invariant list;
 *  - the same state with every object's keys INSERTED in a shuffled order gives
 *    byte-identical canonical strings (both modes), digest and invariants;
 *  - the diff reporter (`diffJson` / `diffSnapshots` / `classifyDiffs` /
 *    `maxNumericDelta`) reports the same differences whatever the key order of
 *    either side;
 *  - `shotSetDelta` is keyed and sorted by section id (projectSnapshot.ts 522):
 *    the same for either side's sections listed in any order.
 *
 * NOT asserted (semantic order): every ARRAY keeps its order in the canonical
 * form (fuzzOracleCore.ts 97-102) - sections, shots, stations, merges, closures
 * are ordered state, and a reorder is exactly what the fuzzer must see - and the
 * digest preserves payload order on purpose (183-189).
 */
import { describe, expect, it } from 'vitest';
import {
  buildTargetingDigest,
  canonicalProjectSnapshot,
  checkProjectInvariants,
  idAgnosticSnapshot,
} from '../../../e2e/agent/lib/fuzzOracleCore';
import {
  classifyDiffs,
  diffJson,
  diffSnapshots,
  maxNumericDelta,
  RELOAD_RESOLVED_GEOMETRY,
  SAVE_HEALING_MUTATIONS,
  shotSetDelta,
} from '../../../e2e/agent/lib/projectSnapshot';
import {
  determinismSeeds,
  mulberry32,
  pick,
  randInt,
  reKeyed,
  seededDiff,
  seededTextDiff,
  shuffled,
  type Rng,
} from '../determinism';

const SEEDS = determinismSeeds();

type Json = Record<string, unknown>;

function ref(rng: Rng, sections: { id: number; n: number }[]) {
  const s = pick(rng, sections);
  return { section_id: s.id, station_index: randInt(rng, 0, s.n) };
}

/** A synthetic `get_project_state` payload. Numbers carry more than 12
 *  significant digits (so the rounding runs), one stat is a BigInt (some
 *  bridges deliver u64 that way), and section_offsets is keyed by section id. */
function randomState(rng: Rng): Json {
  const count = randInt(rng, 1, 5);
  const ids = shuffled(Array.from({ length: 40 }, (_, i) => i + 1), rng).slice(0, count);
  const shape = ids.map((id) => ({ id, n: randInt(rng, 0, 6) }));
  const sections = shape.map(({ id, n }, s) => ({
    id,
    name: `Cave${String.fromCharCode(65 + s)}`,
    surveyor: rng() < 0.5 ? null : 'DiverA',
    color: pick(rng, ['#e74c3c', '#3498db']),
    direction: pick(rng, ['in', 'out']),
    quality: 1,
    next_shot_id: id * 100_000 + n,
    gauge_shot_uuid: rng() < 0.5 ? null : `g-${id}`,
    shots: Array.from({ length: n }, (_, j) => ({
      id: id * 100_000 + j,
      uuid: `u-${id}-${j}`,
      azimuth_in: rng() * 360,
      length: rng() * 10,
      depth_in: rng() * 40,
      pitch_in: rng() < 0.5 ? null : rng() * 90 - 45,
      comment: rng() < 0.3 ? `Shot${j} sump` : null,
      kind: pick(rng, ['Normal', 'Jump', 'Reference']),
      reverse_parity: rng() < 0.5,
    })),
  }));
  const offsets: Json = {};
  for (const { id } of shape) if (rng() < 0.6) offsets[String(id)] = [rng() * 5, rng() * 5];
  const stations = shape.flatMap(({ id, n }) =>
    Array.from({ length: n + 1 }, (_, i) => ({
      station_ref: { section_id: id, station_index: i },
      coordinates: { x: rng() * 100, y: rng() * 100, z: -rng() * 30 },
      is_zero_datum: i === 0 && rng() < 0.3,
      ...(rng() < 0.2 ? { comment: `Station${i} tie-in` } : {}),
    })),
  );
  return {
    project: {
      name: 'CaveSystem',
      location: null,
      created_at: '2026-01-01T00:00:00Z',
      modified_at: `2026-01-0${randInt(rng, 1, 9)}T00:00:00Z`,
      file_path: '/caves/synthetic.stalx',
      next_section_id: 41,
      next_closure_id: 3,
      sections,
      station_merges: rng() < 0.5 ? [{ station_a: ref(rng, shape), station_b: ref(rng, shape), merged_at: 7 }] : [],
      zero_datums: [{ station_ref: ref(rng, shape), gps: { latitude: 20.2, longitude: -87.4, depth_reference: 0 } }],
      loop_closures: [{ id: 1, station_a: ref(rng, shape), station_b: ref(rng, shape), error_percentage: rng() * 3 }],
      section_offsets: offsets,
      imported_file_hashes: ['h1', 'h2'],
      imported_section_hashes: [],
      validation: { max_length: 100, min_length: 0.1 },
      surface_elevation: null,
    },
    stations,
    stats: { section_count: count, shot_count: shape.reduce((a, s) => a + s.n, 0), total_length: rng() * 500, loop_count: 1n },
    frozen_shots: [],
  };
}

/** A structurally different state: numbers moved, a uuid dropped, an offset added,
 *  a shot removed - the shapes the diff reporter has to describe. */
function mutated(state: Json, rng: Rng): Json {
  const copy = reKeyed(state, rng);
  const project = copy.project as Json;
  const sections = project.sections as Json[];
  for (const section of sections) {
    const shots = section.shots as Json[];
    for (const shot of shots) {
      if (rng() < 0.3) shot.azimuth_in = (shot.azimuth_in as number) + rng();
      if (rng() < 0.1) delete shot.uuid;
    }
    if (shots.length > 0 && rng() < 0.3) section.shots = shots.slice(1);
  }
  (project.section_offsets as Json)['99'] = [1, 2];
  return copy;
}

const both = (state: unknown) => ({
  canonical: canonicalProjectSnapshot(state),
  idAgnostic: canonicalProjectSnapshot(state, true),
});

describe('snapshot canonicalisation is deterministic (FZ-DET-TS)', () => {
  it('same state twice (another state in between) -> identical canonical forms, digest, invariants', () => {
    for (const seed of SEEDS) {
      const state = randomState(mulberry32(seed));
      const first = both(state);
      both(randomState(mulberry32(seed + 1_000_003)));
      const second = both(state);
      expect(seededTextDiff(seed, 'canonical', first.canonical, second.canonical)).toBeNull();
      expect(seededTextDiff(seed, 'idAgnostic', first.idAgnostic, second.idAgnostic)).toBeNull();
      expect(seededTextDiff(seed, 'idAgnosticSnapshot', idAgnosticSnapshot(first.canonical), first.idAgnostic)).toBeNull();
      expect(seededDiff(seed, 'digest', buildTargetingDigest(state), buildTargetingDigest(state))).toBeNull();
      expect(seededDiff(seed, 'invariants', checkProjectInvariants(state), checkProjectInvariants(state))).toBeNull();
    }
  });

  it('keys inserted in a shuffled order at every level -> byte-identical canonical forms', () => {
    for (const seed of SEEDS) {
      const state = randomState(mulberry32(seed));
      const reference = both(state);
      for (let round = 0; round < 3; round++) {
        const variant = reKeyed(state, mulberry32(seed * 31 + round));
        const v = both(variant);
        expect(seededTextDiff(seed, `canonical (round ${round})`, reference.canonical, v.canonical)).toBeNull();
        expect(seededTextDiff(seed, `idAgnostic (round ${round})`, reference.idAgnostic, v.idAgnostic)).toBeNull();
        expect(seededDiff(seed, `digest (round ${round})`, buildTargetingDigest(state), buildTargetingDigest(variant))).toBeNull();
        expect(
          seededDiff(seed, `invariants (round ${round})`, checkProjectInvariants(state), checkProjectInvariants(variant)),
        ).toBeNull();
      }
    }
  });

  it('diff reporter: same differences whatever the key order of either side', () => {
    for (const seed of SEEDS) {
      const rng = mulberry32(seed);
      const expected = randomState(rng);
      const actual = mutated(expected, rng);
      const report = (e: unknown, a: unknown) => {
        const diffs = diffJson(e, a, 1_000);
        const classified = classifyDiffs(diffs, [...SAVE_HEALING_MUTATIONS, ...RELOAD_RESOLVED_GEOMETRY]);
        return {
          diffs,
          known: classified.known.map((k) => [k.diff.path, k.asymmetry.id]),
          unknown: classified.unknown,
          asymmetries: classified.asymmetries.map((x) => x.id),
          maxDelta: maxNumericDelta(diffs),
          canonical: diffSnapshots(canonicalProjectSnapshot(e), canonicalProjectSnapshot(a), 1_000),
        };
      };
      const reference = report(expected, actual);
      for (let round = 0; round < 3; round++) {
        const r = mulberry32(seed * 31 + round);
        const variant = report(reKeyed(expected, r), reKeyed(actual, r));
        expect(seededDiff(seed, `diff report (round ${round})`, reference, variant)).toBeNull();
      }
    }
  });

  it('shotSetDelta: the same for either side listing its sections in any order', () => {
    for (const seed of SEEDS) {
      const rng = mulberry32(seed);
      const expected = randomState(rng);
      const actual = mutated(expected, rng);
      const withSectionsShuffled = (state: Json, r: Rng): Json => {
        const project = state.project as Json;
        return { ...state, project: { ...project, sections: shuffled(project.sections as unknown[], r) } };
      };
      const reference = shotSetDelta(canonicalProjectSnapshot(expected), canonicalProjectSnapshot(actual));
      for (let round = 0; round < 3; round++) {
        const r = mulberry32(seed * 31 + round);
        const variant = shotSetDelta(
          canonicalProjectSnapshot(withSectionsShuffled(expected, r)),
          canonicalProjectSnapshot(withSectionsShuffled(actual, r)),
        );
        expect(seededDiff(seed, `shotSetDelta (round ${round})`, reference, variant)).toBeNull();
      }
    }
  });
});
