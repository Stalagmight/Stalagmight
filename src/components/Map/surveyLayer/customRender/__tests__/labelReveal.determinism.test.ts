/**
 * Determinism of label PLACEMENT - the reveal-zoom declutter + decimation pass
 * (#576 fuzzers, FZ-DET-TS). See `src/test/determinism.ts` for the kit.
 *
 * `computeRevealZooms` places labels greedily in a fixed priority order, so its
 * output is only as deterministic as that order is total. Its contract
 * (labelReveal.ts 42-45) is that the order uses a STABLE per-label identity
 * "rather than the array position", i.e. the placement is a function of the
 * label SET, and the result array merely follows the input order.
 *
 * Asserted, per seed, on the descriptors of a synthetic survey (crowded on
 * purpose, so labels collide):
 *  - the same descriptors twice give byte-identical reveal zooms;
 *  - the descriptor list permuted (extents permuted with it) gives, for every
 *    label, the reveal zoom it had before (an equivariance: entry k of the
 *    permuted run == entry perm[k] of the original run).
 *
 * NOT asserted: the input ORDER of `kindVisible` (an array indexed by kind) and
 * the ladder parameters - those are values, not collections. Decimation
 * (`labelDecimation.ts`) is pure arithmetic on one label's ordinal (75-118), so
 * it has no order to leak and is covered here only through the reveal floor.
 */
import { describe, expect, it } from 'vitest';
import { computeRevealZooms, emitSlackFor, type RevealInput } from '../labelReveal';
import { buildLabelDescriptors, shapeDescriptors, type LabelDescriptor } from '../labelTessellate';
import { shapedExtent, type ExtentEm } from '../labelLayout';
import { computeMercatorAnchor } from '../mercator';
import type { GpsOrigin } from '../../../../../types/config';
import {
  determinismSeeds,
  fakeGlyphAtlas,
  mulberry32,
  permutation,
  permutedBlocksDiff,
  seededDiff,
  syntheticSurvey,
  type Rng,
} from '../../../../../test/determinism';

/** Regression seeds: none yet. */
const SEEDS = determinismSeeds();
const ORIGIN: GpsOrigin = { latitude: 20.21, longitude: -87.46 };

interface Case {
  descriptors: LabelDescriptor[];
  extents: (ExtentEm | null)[];
  input: RevealInput;
}

function makeCase(seed: number): Case {
  const rng: Rng = mulberry32(seed);
  // Dense comments + short shots: plenty of overlapping candidates.
  const data = syntheticSurvey(rng, { commentRate: 0.5, stepMetres: 2, maxStations: 10 });
  const [ax, ay] = computeMercatorAnchor(data, ORIGIN);
  const descriptors = buildLabelDescriptors(data, ORIGIN, ax, ay, {
    colorCtx: {
      showDepthGradient: false,
      depthShallow: '#00bfff',
      depthDeep: '#ff4444',
      depthMin: 0,
      depthMax: 40,
      labelColor: '#e0e0e0',
    },
  });
  const extents = shapeDescriptors(descriptors, fakeGlyphAtlas()).map(shapedExtent);
  const buildZoom = 14 + Math.floor(rng() * 6);
  const input: RevealInput = {
    buildZoom,
    // CSS px per mercator unit at buildZoom (512 px world at z0).
    worldPixelScale: 512 * Math.pow(2, buildZoom),
    kindVisible: [0, 1, 2, 3, 4].map(() => rng() < 0.85),
    hideReference: rng() < 0.3,
    hideJump: rng() < 0.3,
    commentOverlapExempt: rng() < 0.2,
    slack: emitSlackFor(descriptors.length, buildZoom),
  };
  return { descriptors, extents, input };
}

describe('label placement (reveal zooms) is deterministic (FZ-DET-TS)', () => {
  it('same descriptors twice (another set placed in between) -> byte-identical reveal zooms', () => {
    for (const seed of SEEDS) {
      const c = makeCase(seed);
      const first = computeRevealZooms(c.descriptors, c.extents, c.input);
      const other = makeCase(seed + 1_000_003);
      computeRevealZooms(other.descriptors, other.extents, other.input);
      const second = computeRevealZooms(c.descriptors, c.extents, c.input);
      expect(seededDiff(seed, 'reveal zooms', first, second)).toBeNull();
    }
  });

  it('descriptor list permuted -> every label keeps its reveal zoom', () => {
    for (const seed of SEEDS) {
      const c = makeCase(seed);
      const reference = computeRevealZooms(c.descriptors, c.extents, c.input);
      for (let round = 0; round < 3; round++) {
        const perm = permutation(c.descriptors.length, mulberry32(seed * 31 + round));
        const permuted = computeRevealZooms(
          perm.map((i) => c.descriptors[i]!),
          perm.map((i) => c.extents[i] ?? null),
          c.input,
        );
        const d = permutedBlocksDiff(reference, permuted, perm, 1);
        if (d !== null) {
          // Name the label that moved, so the failure is readable on its own.
          const k = Number(/^block (\d+)/.exec(d)?.[1] ?? -1);
          const label = c.descriptors[perm[k] ?? -1];
          expect(
            `seed=${seed} round=${round}: ${d}; label ${JSON.stringify(label && { kind: label.kind, text: label.text, shotGlobal: label.shotGlobal, stationGlobal: label.stationGlobal })}`,
          ).toBeNull();
        }
      }
    }
  });
});
