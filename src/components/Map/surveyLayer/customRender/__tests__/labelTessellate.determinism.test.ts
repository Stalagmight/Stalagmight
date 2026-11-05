/**
 * Determinism of the custom renderer's LABEL descriptors and glyph buffers
 * (#576 fuzzers, FZ-DET-TS). See `src/test/determinism.ts` for the kit.
 *
 * Asserted, per seed, on a synthetic survey:
 *  - the same survey built twice (another survey built in between) gives
 *    identical descriptors and byte-identical glyph buffers;
 *  - set-like inputs in a shuffled insertion order - the shot / station comment
 *    maps and the kind filter `Set` - change nothing;
 *  - a shared `ShapeCache` is transparent: the same atlas shaped through the
 *    cache (warmed by another survey) and directly gives the same bytes;
 *  - the stations re-laid-out in another GLOBAL order: shot labels / comments
 *    and SECTION NAMES (whose anchor is "the lowest-index non-reference station",
 *    picked by iterating the lookup Map - labelTessellate.ts 343-362) do not
 *    move; each station label / comment moves with its station.
 *
 * NOT asserted (semantic order, by contract):
 *  - descriptor order follows the SoA walk (shots by section, then stations by
 *    global index, then section names: labelTessellate.ts 213, 291, 338), and the
 *    decimation ordinal of a station label IS its global index (307) and of a
 *    shot label the running shot index (248, 275) - so a station permutation
 *    remaps those ordinals rather than keeping them;
 *  - uv rects depend on the atlas's glyph first-use order (glyphAtlas.ts
 *    121-134), so buffers are only compared across builds fed the same atlas
 *    history.
 */
import { describe, expect, it } from 'vitest';
import {
  buildLabelBuffers,
  buildLabelBuffersFromShaped,
  buildLabelDescriptors,
  shapeDescriptors,
  type LabelBuildContext,
  type LabelDescriptor,
  type ViewportRel,
} from '../labelTessellate';
import { ShapeCache } from '../labelLayout';
import { LABEL_KIND, type LabelKind } from '../labelShaders';
import { computeMercatorAnchor } from '../mercator';
import { packCssColor } from '../colorPack';
import { commentColorForKey } from '../../displayColor';
import type { GpsOrigin } from '../../../../../types/config';
import type { SurveyData } from '../../../../../types/survey-data';
import {
  determinismSeeds,
  fakeGlyphAtlas,
  mulberry32,
  permutation,
  seededDiff,
  shuffledSet,
  syntheticSurvey,
  withPermutedStations,
  withShuffledSparseMaps,
  type Rng,
} from '../../../../../test/determinism';

const SEEDS = determinismSeeds();
const ORIGIN: GpsOrigin = { latitude: 20.21, longitude: -87.46 };
const ALL_KINDS: readonly LabelKind[] = [
  LABEL_KIND.SHOT_LABEL,
  LABEL_KIND.STATION_LABEL,
  LABEL_KIND.SECTION_NAME,
  LABEL_KIND.STATION_COMMENT,
  LABEL_KIND.SHOT_COMMENT,
];

function context(seed: number): LabelBuildContext {
  const ctx: LabelBuildContext = {
    colorCtx: {
      showDepthGradient: seed % 2 === 0,
      depthShallow: '#00bfff',
      depthDeep: '#ff4444',
      depthMin: 0,
      depthMax: 40,
      labelColor: '#e0e0e0',
    },
    precision: { length: 1 + (seed % 2), coordinates: 2, depth: 1 + (seed % 3), azimuth: seed % 2, inclination: 1, gps: 6 },
  };
  if (seed % 4 === 1) ctx.commentColor = (key) => packCssColor(commentColorForKey(key));
  return ctx;
}

function randomKinds(rng: Rng): Set<LabelKind> | undefined {
  if (rng() < 0.3) return undefined;
  const kinds = new Set(ALL_KINDS.filter(() => rng() < 0.7));
  return kinds.size > 0 ? kinds : new Set([LABEL_KIND.STATION_LABEL]);
}

/** Half the time a viewport over part of the survey (the controller's scoped build). */
function randomViewport(rng: Rng, data: SurveyData, anchor: [number, number]): ViewportRel | undefined {
  if (rng() < 0.5 || data.stationCount === 0) return undefined;
  const all = buildLabelDescriptors(data, ORIGIN, anchor[0], anchor[1], context(0));
  const xs = all.map((d) => d.anchorX);
  const ys = all.map((d) => d.anchorY);
  const [minX, maxX, minY, maxY] = [Math.min(...xs), Math.max(...xs), Math.min(...ys), Math.max(...ys)];
  const cx = minX + (maxX - minX) * rng();
  const cy = minY + (maxY - minY) * rng();
  const hw = (maxX - minX) * (0.2 + rng() * 0.4);
  const hh = (maxY - minY) * (0.2 + rng() * 0.4);
  return { minX: cx - hw, maxX: cx + hw, minY: cy - hh, maxY: cy + hh };
}

interface Case {
  data: SurveyData;
  anchor: [number, number];
  ctx: LabelBuildContext;
  kinds: Set<LabelKind> | undefined;
  viewport: ViewportRel | undefined;
}

function makeCase(seed: number): Case {
  const rng = mulberry32(seed);
  const data = syntheticSurvey(rng);
  const anchor = computeMercatorAnchor(data, ORIGIN);
  return { data, anchor, ctx: context(seed), kinds: randomKinds(rng), viewport: randomViewport(rng, data, anchor) };
}

function describeCase(c: Case, data: SurveyData = c.data, kinds = c.kinds): LabelDescriptor[] {
  return buildLabelDescriptors(data, ORIGIN, c.anchor[0], c.anchor[1], c.ctx, kinds, c.viewport);
}

const isShotKind = (d: LabelDescriptor) => d.kind === LABEL_KIND.SHOT_LABEL || d.kind === LABEL_KIND.SHOT_COMMENT;
const isStationKind = (d: LabelDescriptor) =>
  d.kind === LABEL_KIND.STATION_LABEL || d.kind === LABEL_KIND.STATION_COMMENT;

describe('custom-render label descriptors + glyph buffers are deterministic (FZ-DET-TS)', () => {
  it('same survey twice, another survey built in between -> identical descriptors and bytes', () => {
    for (const seed of SEEDS) {
      const c = makeCase(seed);
      const first = describeCase(c);
      const firstBuf = buildLabelBuffers(first, fakeGlyphAtlas());
      const other = makeCase(seed + 1_000_003);
      buildLabelBuffers(describeCase(other), fakeGlyphAtlas());
      const second = describeCase(c);
      const secondBuf = buildLabelBuffers(second, fakeGlyphAtlas());
      expect(seededDiff(seed, 'descriptors', first, second)).toBeNull();
      expect(seededDiff(seed, 'glyph buffers', firstBuf, secondBuf)).toBeNull();
    }
  });

  it('comment maps and the kind filter in a shuffled insertion order -> identical', () => {
    for (const seed of SEEDS) {
      const c = makeCase(seed);
      const reference = describeCase(c);
      const referenceBuf = buildLabelBuffers(reference, fakeGlyphAtlas());
      for (let round = 0; round < 3; round++) {
        const rng = mulberry32(seed * 31 + round);
        const variant = describeCase(c, withShuffledSparseMaps(c.data, rng), c.kinds && shuffledSet(c.kinds, rng));
        expect(seededDiff(seed, `descriptors (round ${round})`, reference, variant)).toBeNull();
        const variantBuf = buildLabelBuffers(variant, fakeGlyphAtlas());
        expect(seededDiff(seed, `glyph buffers (round ${round})`, referenceBuf, variantBuf)).toBeNull();
      }
    }
  });

  it('a shared ShapeCache is transparent (cached shaping == direct shaping, same atlas)', () => {
    for (const seed of SEEDS) {
      const c = makeCase(seed);
      const atlas = fakeGlyphAtlas();
      const cache = new ShapeCache(64);
      // Warm the cache (and the atlas) on a different survey first, then overflow
      // the small cap so evictions happen mid-build.
      shapeDescriptors(describeCase(makeCase(seed + 1_000_003)), atlas, cache);
      const descs = describeCase(c);
      const viaCache = buildLabelBuffersFromShaped(descs, shapeDescriptors(descs, atlas, cache));
      const direct = buildLabelBuffersFromShaped(descs, shapeDescriptors(descs, atlas));
      expect(seededDiff(seed, 'cached vs direct shaping', direct, viaCache)).toBeNull();
    }
  });

  it('stations in another global order: shot labels and section names stay, station labels move with their station', () => {
    for (const seed of SEEDS) {
      const c = makeCase(seed);
      const rng = mulberry32(seed * 977);
      const perm = permutation(c.data.stationCount, rng);
      const newIndexOf = new Map(perm.map((oldG, newG) => [oldG, newG] as const));
      const before = describeCase(c);
      const after = describeCase(c, withPermutedStations(c.data, perm));

      expect(seededDiff(seed, 'shot labels', before.filter(isShotKind), after.filter(isShotKind))).toBeNull();
      const names = (list: LabelDescriptor[]) => list.filter((d) => d.kind === LABEL_KIND.SECTION_NAME);
      expect(seededDiff(seed, 'section names', names(before), names(after))).toBeNull();

      // Station labels: renumber the originals to their new global index (the
      // ordinal of a station label is that index), then lay them out in the new
      // global order; label before comment, as the walk emits them.
      const expected = before
        .filter(isStationKind)
        .map((d) => {
          const g = newIndexOf.get(d.stationGlobal!)!;
          return { ...d, stationGlobal: g, ordinal: d.kind === LABEL_KIND.STATION_LABEL ? g : d.ordinal };
        })
        .sort((a, b) => a.stationGlobal - b.stationGlobal || a.kind - b.kind);
      expect(seededDiff(seed, 'station labels', expected, after.filter(isStationKind))).toBeNull();
    }
  });
});
