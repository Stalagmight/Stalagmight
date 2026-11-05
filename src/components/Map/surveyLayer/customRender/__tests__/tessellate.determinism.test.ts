/**
 * Determinism of the custom renderer's built vertex buffers (#576 fuzzers,
 * FZ-DET-TS). See `src/test/determinism.ts` for the kit and the env knob.
 *
 * Asserted, per seed, on a synthetic survey:
 *  - the same survey built twice - with a DIFFERENT survey and GPS origin built in
 *    between, so a module-level memo (the origin frame in `utils/coordinates.ts`)
 *    or any other carried state would show - gives byte-identical line + circle
 *    buffers, all-reference mask, station projection and dim arrays;
 *  - inputs that are SETS by contract leave the bytes unchanged when their
 *    insertion order changes: the (section, station index) -> global lookup
 *    (outer and inner Map), the exclusion / anchored / immovable / hide /
 *    highlight sets, the comment maps the move-comment tint reads, and the
 *    analytical-overlay value maps;
 *  - the stations re-laid-out in another GLOBAL order: the line buffers (which
 *    reach stations only through the lookup) do not move at all, and every
 *    per-station circle block moves with its station;
 *  - the drag ghost and the measure path: same input twice is byte-identical, and
 *    under a permutation of their feature / segment lists the anchor (a bbox
 *    midpoint) is unchanged and every per-feature block moves with its feature.
 *
 * NOT asserted (the order is semantic, by contract):
 *  - shot order within a section and section declaration order: the line buffer
 *    is laid out section by section, shot by shot (`tessellate.ts` 461-473), and
 *    `sectionRanges` is indexed by declaration order (104-109);
 *  - the global station order: circle slot == global index (`tessellate.ts`
 *    38-39, 639-641) - so it is checked as an equivariance, not an invariance;
 *  - ghost / measure-path feature order is the draw order of translucent quads
 *    (`ghostTessellate.ts` buildGhostBuffers appends in input order) - checked
 *    as an equivariance too.
 */
import { describe, expect, it } from 'vitest';
import {
  buildCircleBuffers,
  buildLineBuffers,
  buildStationLookup,
  computeCircleDim,
  computeLineDim,
  computeStationAllReference,
  countValidShots,
  projectStationsRel,
  type CustomRenderExclusion,
} from '../tessellate';
import { computeMercatorAnchor } from '../mercator';
import { makeColorResolvers } from '../colorResolvers';
import { buildGhostBuffers, type GhostBuildInputs, type GhostHighlights } from '../ghostTessellate';
import { buildMeasurePathBuffers, type MeasurePoint, type MeasureSegment } from '../measurePathTessellate';
import type { Rgba8 } from '../colorPack';
import type { DisplayColorContext } from '../../displayColor';
import type { DragPreviewFeature, MovingEnd } from '../../dragPreviewSource';
import type { GpsOrigin } from '../../../../../types/config';
import type { SurveyData } from '../../../../../types/survey-data';
import { FEATURE_ID_STRIDE } from '../../../../../utils/ids';
import {
  determinismSeeds,
  firstByteDiff,
  mulberry32,
  permutation,
  permutedBlocksDiff,
  pick,
  randInt,
  seededDiff,
  shuffled,
  shuffledMap,
  shuffledSet,
  syntheticSurvey,
  withPermutedStations,
  withShuffledSparseMaps,
  type Rng,
} from '../../../../../test/determinism';

const SEEDS = determinismSeeds();

const THEME = {
  stationDefault: '#ffffff',
  zeroDatum: '#00ffff',
  strokeColor: '#000000',
  depthShallow: '#0000ff',
  depthDeep: '#ff0000',
  severityError: '#ff0000',
  severityWarning: '#ffff00',
  severityClear: '#00000000',
  grayLine: '#808080',
};

const ORIGIN_A: GpsOrigin = { latitude: 20.21, longitude: -87.46 };
const ORIGIN_B: GpsOrigin = { latitude: -33.9, longitude: 151.2 };

/** Interaction sets a build reads (all set-like: only `has` is ever asked). */
interface InputSets {
  exclude: CustomRenderExclusion;
  anchored: Set<number>;
  immovableStations: Set<number>;
  immovableShots: Set<number>;
  hideSections: Set<number>;
  hideStations: Set<number>;
  hideShots: Set<number>;
  hover: Set<number>;
  selected: Set<number>;
}

function subset<T>(rng: Rng, items: readonly T[], rate: number): Set<T> {
  return new Set(items.filter(() => rng() < rate));
}

function randomSets(rng: Rng, data: SurveyData): InputSets {
  const sectionIds = data.sections.map((s) => s.id);
  const stations = Array.from({ length: data.stationCount }, (_, g) => g);
  const shots = Array.from({ length: data.shotCount }, (_, g) => g);
  const stationFids = stations.map((g) => data.stationSectionIds[g]! * FEATURE_ID_STRIDE + data.stationIndices[g]!);
  const shotFids = shots.map((g) => data.shotSectionIds[g]! * FEATURE_ID_STRIDE + data.shotIds[g]!);
  return {
    exclude: {
      sectionIds: subset(rng, sectionIds, 0.2),
      stationGlobalIndices: subset(rng, stations, 0.15),
      shotGlobalIndices: subset(rng, shots, 0.15),
    },
    anchored: subset(rng, sectionIds, 0.3),
    immovableStations: subset(rng, stationFids, 0.2),
    immovableShots: subset(rng, shotFids, 0.2),
    hideSections: subset(rng, sectionIds, 0.2),
    hideStations: subset(rng, stations, 0.2),
    hideShots: subset(rng, shots, 0.2),
    hover: subset(rng, sectionIds, 0.3),
    selected: subset(rng, sectionIds, 0.4),
  };
}

function reshuffleSets(rng: Rng, s: InputSets): InputSets {
  const opt = (set: ReadonlySet<number> | undefined) => (set ? shuffledSet(set, rng) : undefined);
  const exclude: CustomRenderExclusion = {};
  const sec = opt(s.exclude.sectionIds);
  const st = opt(s.exclude.stationGlobalIndices);
  const sh = opt(s.exclude.shotGlobalIndices);
  if (sec) exclude.sectionIds = sec;
  if (st) exclude.stationGlobalIndices = st;
  if (sh) exclude.shotGlobalIndices = sh;
  return {
    exclude,
    anchored: shuffledSet(s.anchored, rng),
    immovableStations: shuffledSet(s.immovableStations, rng),
    immovableShots: shuffledSet(s.immovableShots, rng),
    hideSections: shuffledSet(s.hideSections, rng),
    hideStations: shuffledSet(s.hideStations, rng),
    hideShots: shuffledSet(s.hideShots, rng),
    hover: shuffledSet(s.hover, rng),
    selected: shuffledSet(s.selected, rng),
  };
}

/** Per seed, one of the color modes the controller can be in; the overlay maps
 *  are keyed by feature id and are set-like. */
function colorContext(seed: number, rng: Rng, data: SurveyData): DisplayColorContext {
  const base = {
    showDepthGradient: false,
    showIssueSeverity: false,
    depthRange: { min: 0, max: 40 },
    themeColors: THEME as unknown as DisplayColorContext['themeColors'],
  } as DisplayColorContext;
  switch (seed % 4) {
    case 1:
      return { ...base, showDepthGradient: true };
    case 2: {
      const shots = new Map<number, number>();
      const stations = new Map<number, number>();
      for (let g = 0; g < data.shotCount; g++) {
        if (rng() < 0.8) shots.set(data.shotSectionIds[g]! * FEATURE_ID_STRIDE + data.shotIds[g]!, rng() < 0.1 ? -1 : rng() * 120);
      }
      for (let g = 0; g < data.stationCount; g++) {
        if (rng() < 0.8) {
          stations.set(data.stationSectionIds[g]! * FEATURE_ID_STRIDE + data.stationIndices[g]!, rng() < 0.1 ? -1 : rng() * 120);
        }
      }
      return { ...base, overlay: { mode: 'distance', shots, stations, max: 100 } };
    }
    case 3:
      return { ...base, commentTint: true };
    default:
      return base;
  }
}

function withShuffledOverlay(ctx: DisplayColorContext, rng: Rng): DisplayColorContext {
  if (!ctx.overlay) return ctx;
  return {
    ...ctx,
    overlay: { ...ctx.overlay, shots: shuffledMap(ctx.overlay.shots, rng), stations: shuffledMap(ctx.overlay.stations, rng) },
  };
}

/**
 * Every product of one geometry build, wired the way `controller.ts` build()
 * wires it (anchor -> projection -> lookup -> colors -> all-reference -> line +
 * circle, plus the cheap dim recomputes). The dim / hide / highlight predicates
 * have the controller's shapes (`controller.ts` dimPredicates / hidePredicates /
 * highlightLevel): membership tests only.
 */
function buildAll(
  data: SurveyData,
  origin: GpsOrigin | null,
  ctx: DisplayColorContext,
  sets: InputSets,
  lookupOverride?: Map<number, Map<number, number>>,
) {
  const [anchorX, anchorY] = computeMercatorAnchor(data, origin);
  const rel = projectStationsRel(data, origin, anchorX, anchorY);
  const lookup = lookupOverride ?? buildStationLookup(data);
  const res = makeColorResolvers(data, ctx);
  const allRef = computeStationAllReference(data, lookup);
  const stationFid = (g: number) => data.stationSectionIds[g]! * FEATURE_ID_STRIDE + data.stationIndices[g]!;
  const dimForStation = (g: number): number =>
    sets.anchored.has(data.stationSectionIds[g]!) || sets.immovableStations.has(stationFid(g)) ? 1 : 0;
  const dimForShot = (g: number, sid: number): number =>
    sets.anchored.has(sid) || sets.immovableShots.has(sid * FEATURE_ID_STRIDE + data.shotIds[g]!) ? 1 : 0;
  const hideForStation = (g: number): boolean =>
    sets.hideStations.has(g) || sets.hideSections.has(data.stationSectionIds[g]!);
  const hideForShot = (g: number, sid: number): boolean => sets.hideShots.has(g) || sets.hideSections.has(sid);
  const level = (sid: number): number => (sets.hover.has(sid) ? 2 : sets.selected.has(sid) ? 1 : 0);
  const line = buildLineBuffers(
    data, rel, lookup, res.colorForShot, undefined,
    res.useDepthEndpoints ? res.colorForShotEndpoints : undefined,
    dimForShot, hideForShot, (_g, sid) => level(sid),
  );
  const circle = buildCircleBuffers(
    data, rel, res.fillForStation, res.strokeForStation, undefined, allRef,
    dimForStation, hideForStation, (g) => level(data.stationSectionIds[g]!),
  );
  // The build-time exclusion path (segments / stations / shots skipped outright).
  const lineEx = buildLineBuffers(data, rel, lookup, res.colorForShot, sets.exclude);
  const circleEx = buildCircleBuffers(data, rel, res.fillForStation, res.strokeForStation, sets.exclude, allRef);
  return {
    anchor: [anchorX, anchorY],
    rel,
    allRef,
    line,
    circle,
    lineEx,
    circleEx,
    lineDim: computeLineDim(data, lookup, dimForShot, sets.exclude),
    circleDim: computeCircleDim(data, dimForStation, sets.exclude),
    validShots: countValidShots(data, lookup, sets.exclude),
  };
}

/** A lookup with the same entries, inserted in a shuffled order at both levels. */
function shuffledLookup(lookup: Map<number, Map<number, number>>, rng: Rng): Map<number, Map<number, number>> {
  return new Map(shuffled([...lookup.entries()], rng).map(([sid, inner]) => [sid, shuffledMap(inner, rng)]));
}

describe('custom-render geometry buffers are deterministic (FZ-DET-TS)', () => {
  it('same survey twice, with another survey and origin built in between -> byte-identical', () => {
    for (const seed of SEEDS) {
      const rng = mulberry32(seed);
      const data = syntheticSurvey(rng);
      const ctx = colorContext(seed, rng, data);
      const sets = randomSets(rng, data);
      const origin = seed % 2 === 0 ? ORIGIN_A : null;
      const first = buildAll(data, origin, ctx, sets);
      const otherRng = mulberry32(seed + 1_000_003);
      const other = syntheticSurvey(otherRng);
      buildAll(other, ORIGIN_B, colorContext(seed + 1, otherRng, other), randomSets(otherRng, other));
      const second = buildAll(data, origin, ctx, sets);
      expect(seededDiff(seed, 'rebuild', first, second)).toBeNull();
    }
  });

  it('set-like inputs in a shuffled insertion order -> byte-identical', () => {
    for (const seed of SEEDS) {
      const rng = mulberry32(seed);
      const data = syntheticSurvey(rng);
      const ctx = colorContext(seed, rng, data);
      const sets = randomSets(rng, data);
      const origin = seed % 2 === 0 ? ORIGIN_A : null;
      const reference = buildAll(data, origin, ctx, sets);
      for (let round = 0; round < 3; round++) {
        const shuffleRng = mulberry32(seed * 31 + round);
        const lookup = shuffledLookup(buildStationLookup(data), shuffleRng);
        const variant = buildAll(
          withShuffledSparseMaps(data, shuffleRng),
          origin,
          withShuffledOverlay(ctx, shuffleRng),
          reshuffleSets(shuffleRng, sets),
          lookup,
        );
        expect(seededDiff(seed, `shuffled inputs (round ${round})`, reference, variant)).toBeNull();
      }
    }
  });

  it('stations in another global order: lines do not move, circle blocks move with their station', () => {
    for (const seed of SEEDS) {
      const rng = mulberry32(seed);
      const data = syntheticSurvey(rng);
      const ctx = colorContext(seed, rng, data);
      const sets = randomSets(rng, data);
      // Station-keyed sets follow their stations; the rest are not station-indexed.
      const perm = permutation(data.stationCount, rng);
      const newIndexOf = new Map(perm.map((oldG, newG) => [oldG, newG] as const));
      const moved: InputSets = {
        ...sets,
        hideStations: new Set([...sets.hideStations].map((g) => newIndexOf.get(g)!)),
      };
      const permuted = withPermutedStations(data, perm);
      const a = buildAll(data, ORIGIN_A, ctx, sets);
      const b = buildAll(permuted, ORIGIN_A, ctx, moved);

      expect(seededDiff(seed, 'anchor', a.anchor, b.anchor)).toBeNull();
      expect(seededDiff(seed, 'line buffers under a station permutation', a.line, b.line)).toBeNull();
      const blocks = (label: string, x: ArrayBufferView, y: ArrayBufferView, per: number) => {
        const d = permutedBlocksDiff(x, y, perm, per);
        return d === null ? null : `seed=${seed} ${label}: ${d}`;
      };
      expect(blocks('stationRel', a.rel, b.rel, 2)).toBeNull();
      expect(blocks('allRef', a.allRef, b.allRef, 1)).toBeNull();
      const { circle: ca } = a;
      const { circle: cb } = b;
      expect(blocks('circle.center', ca.center, cb.center, 8)).toBeNull();
      expect(blocks('circle.corner', ca.corner, cb.corner, 8)).toBeNull();
      expect(blocks('circle.fill', ca.fill, cb.fill, 16)).toBeNull();
      expect(blocks('circle.stroke', ca.stroke, cb.stroke, 16)).toBeNull();
      expect(blocks('circle.sizeMul', ca.sizeMul, cb.sizeMul, 4)).toBeNull();
      expect(blocks('circle.dim', ca.dim, cb.dim, 4)).toBeNull();
      expect(blocks('circle.hide', ca.hide, cb.hide, 4)).toBeNull();
      expect(blocks('circle.highlight', ca.highlight, cb.highlight, 4)).toBeNull();
      expect(blocks('circle.allReference', ca.allReference, cb.allReference, 4)).toBeNull();
      const idx = firstByteDiff(ca.indices, cb.indices);
      expect(idx === null ? null : `seed=${seed} circle.indices: ${idx}`).toBeNull();
    }
  });
});

// ─── Drag ghost + measure path ──────────────────────────────────────

const MOVING_ENDS: readonly MovingEnd[] = ['from', 'to', 'both', 'uniform', 'none'];

function lngLat(rng: Rng): [number, number] {
  return [-87.46 + (rng() - 0.5) * 0.002, 20.21 + (rng() - 0.5) * 0.002];
}

function randomGhostSet(rng: Rng): { features: DragPreviewFeature[]; highlights: GhostHighlights } {
  const features: DragPreviewFeature[] = [];
  const n = randInt(rng, 1, 24);
  for (let i = 0; i < n; i++) {
    const kind = pick(rng, ['station', 'shot', 'closure'] as const);
    features.push({
      emit: kind === 'station' ? 'point' : 'line',
      coords: kind === 'station' ? [lngLat(rng)] : [lngLat(rng), lngLat(rng)],
      movingEnd: pick(rng, MOVING_ENDS),
      properties: { kind, color: pick(rng, ['#e74c3c', '#3498db', '#2ecc71']), tag: `StationA${i}` },
    });
  }
  const highlights: GhostHighlights = {
    stations: rng() < 0.5 ? [{ coord: lngLat(rng), movingEnd: 'uniform', fill: [1, 2, 3, 255], stroke: [9, 9, 9, 255], opacity: 0.9, sizeMul: 1.5, strokeMul: 2 }] : [],
    shots: rng() < 0.5 ? [{ a: lngLat(rng), b: lngLat(rng), movingEnd: 'from', color: [200, 10, 10, 255], opacity: 0.8, widthMul: 2 }] : [],
  };
  return { features, highlights };
}

const GHOST_INPUTS: GhostBuildInputs = {
  colorFor: (f) => {
    const css = String(f.properties.color);
    return [parseInt(css.slice(1, 3), 16), parseInt(css.slice(3, 5), 16), parseInt(css.slice(5, 7), 16), 255] as Rgba8;
  },
  strokeColor: [0, 0, 0, 255],
  fillAlpha: 0.85,
  widthMulFor: (f) => (f.properties.kind === 'closure' ? 1.5 : 1),
};

/** Per-field block equivariance over a buffer record: `count` items, each
 *  field's block size derived from its length. `indices` must be identical. */
function blocksDiff(
  seed: number,
  label: string,
  a: Record<string, unknown>,
  b: Record<string, unknown>,
  perm: readonly number[],
): string | null {
  for (const [key, value] of Object.entries(a)) {
    const other = b[key];
    if (!ArrayBuffer.isView(value) || !ArrayBuffer.isView(other)) continue;
    const d =
      key === 'indices'
        ? firstByteDiff(value, other)
        : perm.length === 0
          ? firstByteDiff(value, other)
          : permutedBlocksDiff(value, other, perm, (value as unknown as ArrayLike<number>).length / perm.length);
    if (d !== null) return `seed=${seed} ${label}.${key}: ${d}`;
  }
  return null;
}

describe('drag ghost + measure path buffers are deterministic (FZ-DET-TS)', () => {
  it('ghost: same set twice is byte-identical; a feature permutation only moves blocks', () => {
    for (const seed of SEEDS) {
      const rng = mulberry32(seed);
      const { features, highlights } = randomGhostSet(rng);
      const first = buildGhostBuffers({ features }, GHOST_INPUTS, highlights);
      buildGhostBuffers(randomGhostSet(mulberry32(seed + 7)), GHOST_INPUTS);
      const second = buildGhostBuffers({ features }, GHOST_INPUTS, highlights);
      expect(seededDiff(seed, 'ghost rebuild', first, second)).toBeNull();

      const order = permutation(features.length, rng);
      const permutedFeatures = order.map((i) => features[i]!);
      const variant = buildGhostBuffers({ features: permutedFeatures }, GHOST_INPUTS, highlights);
      expect(seededDiff(seed, 'ghost anchor', [first.anchorX, first.anchorY], [variant.anchorX, variant.anchorY])).toBeNull();
      // Highlights are appended after the features, in their own order.
      const lineOf = new Map(features.filter((f) => f.emit === 'line').map((f, k) => [f, k] as const));
      const pointOf = new Map(features.filter((f) => f.emit === 'point').map((f, k) => [f, k] as const));
      const linePerm = permutedFeatures.filter((f) => f.emit === 'line').map((f) => lineOf.get(f)!);
      const pointPerm = permutedFeatures.filter((f) => f.emit === 'point').map((f) => pointOf.get(f)!);
      highlights.shots.forEach((_, k) => linePerm.push(lineOf.size + k));
      highlights.stations.forEach((_, k) => pointPerm.push(pointOf.size + k));
      expect(blocksDiff(seed, 'ghost.line', { ...first.line }, { ...variant.line }, linePerm)).toBeNull();
      expect(blocksDiff(seed, 'ghost.circle', { ...first.circle }, { ...variant.circle }, pointPerm)).toBeNull();
    }
  });

  it('measure path: same path twice is byte-identical; a segment permutation only moves blocks', () => {
    for (const seed of SEEDS) {
      const rng = mulberry32(seed);
      const points: MeasurePoint[] = Array.from({ length: randInt(rng, 1, 12) }, () => ({ center: lngLat(rng) }));
      const segments: MeasureSegment[] = points.slice(1).map((p, i) => ({ a: points[i]!.center, b: p.center }));
      const first = buildMeasurePathBuffers(segments, points);
      buildMeasurePathBuffers([{ a: lngLat(rng), b: lngLat(rng) }], []);
      const second = buildMeasurePathBuffers(segments, points);
      expect(seededDiff(seed, 'measure rebuild', first, second)).toBeNull();

      const segPerm = permutation(segments.length, rng);
      const ptPerm = permutation(points.length, rng);
      const variant = buildMeasurePathBuffers(segPerm.map((i) => segments[i]!), ptPerm.map((i) => points[i]!));
      expect(seededDiff(seed, 'measure anchor', [first.anchorX, first.anchorY], [variant.anchorX, variant.anchorY])).toBeNull();
      expect(blocksDiff(seed, 'measure.line', { ...first.line }, { ...variant.line }, segPerm)).toBeNull();
      expect(blocksDiff(seed, 'measure.circle', { ...first.circle }, { ...variant.circle }, ptPerm)).toBeNull();
    }
  });
});
