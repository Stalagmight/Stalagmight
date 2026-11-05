/**
 * Determinism of the global view's custom-render buffers (#576 fuzzers,
 * FZ-DET-TS). See `src/test/determinism.ts` for the kit and the env knob.
 *
 * Asserted, per seed, over random systems (CaveA.tml, CaveB.tml, ...):
 *  - the same systems built twice (another build in between) are byte-identical,
 *    `systemIndex` included;
 *  - the selection `Set` in a shuffled insertion order changes nothing;
 *  - the systems list permuted: the anchor (a bbox midpoint) is unchanged and
 *    every system's line and station blocks carry the same bytes, except the
 *    per-vertex `system` index, which is the system's new position.
 *
 * NOT asserted (semantic order): the system index and the draw order ARE the
 * input order (`globalTessellate.ts` 229-235: a system's index is its first
 * position), and stations follow the shots' first-seen order inside a system
 * (39-73), so shot order within a system is not shuffled.
 */
import { describe, expect, it } from 'vitest';
import type { ProjectMetadata } from '../../../../types/project';
import type { GeometryShot } from '../../../../types/survey';
import type { SystemGeometry } from '../../systemColor';
import { buildGlobalBuffers, extractSystemStations } from '../globalTessellate';
import {
  determinismSeeds,
  firstByteDiff,
  mulberry32,
  permutation,
  pick,
  randInt,
  seededDiff,
  shuffledSet,
  type Rng,
} from '../../../../test/determinism';

const SEEDS = determinismSeeds();

function randomSystem(rng: Rng, i: number): SystemGeometry {
  const letter = String.fromCharCode(65 + (i % 26));
  const shots: GeometryShot[] = [];
  let at: [number, number] = [-87.5 + rng() * 0.5, 20 + rng() * 0.5];
  const n = randInt(rng, 0, 12);
  for (let s = 0; s < n; s++) {
    const next: [number, number] = [at[0] + (rng() - 0.5) * 1e-4, at[1] + (rng() - 0.5) * 1e-4];
    // Now and then a shot with a missing endpoint (not drawn) or a jump back.
    const from = rng() < 0.05 ? [] : at;
    shots.push({ from, to: next, kind: pick(rng, ['Normal', 'Normal', 'Jump', 'Reference'] as const), section_id: 1 });
    at = rng() < 0.2 ? [at[0], at[1]] : next;
  }
  const metadata: ProjectMetadata = {
    name: `Cave${letter}`,
    location: null,
    file_path: `/caves/Cave${letter}${i}.tml`,
    modified_at: '2026-01-01T00:00:00Z',
    section_count: 1,
    total_length: 100,
    station_count: n + 1,
    gps_center: [0, 0],
    gps_bounds: null,
    is_remote: false,
  };
  return { metadata, geometry: { shots, origin: [0, 0] } };
}

function randomCase(seed: number) {
  const rng = mulberry32(seed);
  const systems = Array.from({ length: randInt(rng, 1, 6) }, (_, i) => randomSystem(rng, i));
  const selectedIds = new Set(systems.filter(() => rng() < 0.4).map((s) => s.metadata.file_path));
  const searchTerm = rng() < 0.5 ? null : pick(rng, ['cavea', 'caveb', 'z']);
  return { rng, systems, opts: { searchTerm, selectedIds } };
}

/** `[first, count]` of each system's drawn segments and its stations, in input order. */
function ranges(systems: SystemGeometry[]) {
  let seg = 0;
  let st = 0;
  return systems.map((s) => {
    const segs = s.geometry.shots.filter((x) => x.from.length >= 2 && x.to.length >= 2).length;
    const stations = extractSystemStations(s.geometry.shots).length;
    const out = { seg: [seg, segs] as const, st: [st, stations] as const };
    seg += segs;
    st += stations;
    return out;
  });
}

/** Byte slice of `a`'s quads `[first, first+count)` (4 verts per quad). */
function quads(a: ArrayBufferView, vertexCount: number, first: number, count: number): Uint8Array {
  const perVertexBytes = vertexCount === 0 ? 0 : a.byteLength / vertexCount;
  return new Uint8Array(a.buffer, a.byteOffset + first * 4 * perVertexBytes, count * 4 * perVertexBytes);
}

describe('global-view custom-render buffers are deterministic (FZ-DET-TS)', () => {
  it('same systems twice, another build in between -> byte-identical', () => {
    for (const seed of SEEDS) {
      const { systems, opts } = randomCase(seed);
      const first = buildGlobalBuffers(systems, opts);
      const other = randomCase(seed + 1_000_003);
      buildGlobalBuffers(other.systems, other.opts);
      const second = buildGlobalBuffers(systems, opts);
      expect(seededDiff(seed, 'global rebuild', first, second)).toBeNull();
    }
  });

  it('selection set in a shuffled insertion order -> byte-identical', () => {
    for (const seed of SEEDS) {
      const { rng, systems, opts } = randomCase(seed);
      const reference = buildGlobalBuffers(systems, opts);
      const variant = buildGlobalBuffers(systems, { ...opts, selectedIds: shuffledSet(opts.selectedIds, rng) });
      expect(seededDiff(seed, 'selection order', reference, variant)).toBeNull();
    }
  });

  it('systems permuted -> same anchor, each system block unchanged but for its index', () => {
    for (const seed of SEEDS) {
      const { rng, systems, opts } = randomCase(seed);
      const perm = permutation(systems.length, rng);
      const permuted = perm.map((i) => systems[i]!);
      const a = buildGlobalBuffers(systems, opts);
      const b = buildGlobalBuffers(permuted, opts);
      expect(seededDiff(seed, 'anchor', [a.anchorX, a.anchorY], [b.anchorX, b.anchorY])).toBeNull();
      const ra = ranges(systems);
      const rb = ranges(permuted);
      for (let k = 0; k < perm.length; k++) {
        const j = perm[k]!;
        for (const [part, va, vb, field] of [
          ['line', a.line, b.line, 'seg'],
          ['circle', a.circle, b.circle, 'st'],
        ] as const) {
          const [fa, n] = ra[j]![field];
          const [fb] = rb[k]![field];
          for (const [key, arr] of Object.entries(va)) {
            if (!ArrayBuffer.isView(arr) || key === 'indices') continue;
            const other = (vb as unknown as Record<string, ArrayBufferView>)[key]!;
            if (key === 'system') {
              const sys = quads(other, vb.vertexCount, fb, n);
              const vals = new Float32Array(sys.buffer.slice(sys.byteOffset, sys.byteOffset + sys.byteLength));
              const bad = vals.findIndex((v) => v !== k);
              expect(bad === -1 ? null : `seed=${seed} ${part}.system of system ${k}: ${vals[bad]}`).toBeNull();
              continue;
            }
            const d = firstByteDiff(quads(arr, va.vertexCount, fa, n), quads(other, vb.vertexCount, fb, n));
            expect(d === null ? null : `seed=${seed} ${part}.${key} of system ${j} -> ${k}: ${d}`).toBeNull();
          }
        }
      }
    }
  });
});
