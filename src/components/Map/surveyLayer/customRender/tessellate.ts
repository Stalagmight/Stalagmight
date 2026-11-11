/**
 * SoA -> WebGL vertex buffers for the custom-render prototype (#customRender).
 *
 * Pure (no GL, no DOM): builds typed-array vertex + index buffers straight from
 * `SurveyData`, so the whole tessellation / anchor-relative / sizing story is
 * unit-testable in the `node` vitest environment. The GL layer
 * (`surveyCustomLayer.ts`) just uploads what these return.
 *
 * LINES (shots): screen-space-width antialiased lines, MapLibre-style per-segment
 * quads extruded along the segment normal in the vertex shader. Tessellation is
 * SIMPLIFIED vs MapLibre: each valid shot is an INDEPENDENT butt-cap quad (2
 * triangles) - no miter / bevel joins, no round caps. Joins between consecutive
 * shots are visually covered by the station circles drawn at each vertex (the
 * same way the stock circle layer sits over the stock line joins), so butt caps
 * read as credible at cave scale. The extrusion + edge-feather math is lifted
 * from MapLibre GL JS `src/shaders/line.{vertex,fragment}.glsl` (BSD-3-Clause);
 * see `shaders.ts`.
 *
 * CIRCLES (stations): one quad per station, the disc + stroke ring drawn in the
 * fragment shader with a feathered edge (`shaders.ts`).
 *
 * DRAW ORDER: each kind draws in one call with no depth test, so a later quad
 * paints over an earlier one. The vertices sit in SoA order (what the
 * incremental path addresses), but both index buffers draw by rank - reference
 * first, then jump, then real (`writeLineDrawOrder` / `writeCircleDrawOrder`) -
 * so where a jump or reference line or marker crosses or coincides with the real
 * line, the real line is on top. A raised section (the selection's, see
 * `SurveyCustomController.setRaisedSection`) draws after all three.
 *
 * Valid-shot selection mirrors `vtSurveyBuild.buildShotsPerFeature`: a shot
 * contributes only when both endpoint stations (`stationIndex` i and i+1) exist.
 * That keeps the custom layer's shot set identical to the stock shots layer.
 */
import type { GpsOrigin } from '../../../../types/config';
import {
  type SurveyData,
  type SectionMetadata,
  shotKindFromFlags,
  shotKindToFlags,
  STATION_FLAG_IS_ZERO_DATUM,
  stationKindRank,
} from '../../../../types/survey-data';
import { metersToMapCoords } from '../../../../utils/coordinates';
import { lngLatToMercator } from '../mercatorCoords';
import type { Rgba8 } from './colorPack';

/** Anchor-relative float32 station positions, interleaved `[x0,y0,x1,y1,...]`
 *  in global station-index order. */
export type StationRel = Float32Array;

/**
 * Coerce a per-vertex flag predicate's result to the numeric value stored in a
 * flag buffer. Dim / hide predicates return a boolean (→ 0 / 1); the highlight
 * predicate returns a graded emphasis LEVEL (0 none, 1 selection, 2 hover) which
 * passes through unchanged. `undefined` (predicate absent) → 0. Keeps the dim /
 * hide byte-parity guarantees while letting one flag buffer carry a level.
 */
function flagValue(v: number | boolean | undefined): number {
  return v === true ? 1 : v === false || v === undefined ? 0 : v;
}

export interface LineBuffers {
  segmentCount: number;
  vertexCount: number;
  /** Endpoint A of each vertex's segment (anchor-relative), `vertexCount*2`. */
  posA: Float32Array;
  /** Endpoint B of each vertex's segment (anchor-relative), `vertexCount*2`. */
  posB: Float32Array;
  /** 0 at endpoint A, 1 at endpoint B; selects which endpoint this vertex sits on. */
  endpoint: Float32Array;
  /** Physical extrusion side, -1 / +1; also the AA cross-width coordinate. */
  side: Float32Array;
  /** Per-vertex from-endpoint RGBA8, replicated across all 4 verts of a segment.
   *  With round caps the quad is extended past both endpoints (shaders.ts), so
   *  the fragment can no longer rely on a single interpolated color anchored at
   *  the true endpoints; it interpolates `colorFrom`->`colorTo` by the CLAMPED
   *  along-fraction, which reproduces the flat / depth-gradient body exactly AND
   *  holds the endpoint color across each rounded cap. In flat / severity /
   *  overlay modes `colorFrom === colorTo`. */
  colorFrom: Uint8Array;
  /** Per-vertex to-endpoint RGBA8, replicated across all 4 verts. See
   *  {@link colorFrom}. */
  colorTo: Uint8Array;
  /** Per-vertex `ShotKind` discriminant (0 = normal, 1 = jump, 2 = reference;
   *  the shader decodes with two thresholds). Fades virtual shots
   *  (SHOT_ALPHA_JUMP 0.5, SHOT_ALPHA_REFERENCE 0.28) and collapses them when
   *  the matching toggle hides them. */
  reference: Float32Array;
  /** Per-vertex dim flag (1 = dimmed to 0.25, else 0). Mirrors the stock shots
   *  layer's anchored / immovable paint cases (move-segment / move-shot tools):
   *  the shader multiplies opacity by 0.25 and this WINS over the reference 0.5
   *  case, matching the stock `case immovable 0.25, anchored 0.25, reference
   *  0.5, 1` order. Rewritten in place on an anchored/immovable set change. */
  dim: Float32Array;
  /** Per-vertex hide flag (1 = collapse the vertex to a clipped point, drawing
   *  nothing). Mirrors the drag-exclusion the stock bulk layers apply via
   *  setFilter during a move: instead of omitting the moving features from the
   *  buffers (a full rebuild), the drag-begin path recomputes just this array
   *  and re-uploads it, and drag-end zeroes it. All 4 verts of a segment share
   *  the flag so both triangles degenerate together. */
  hide: Float32Array;
  /** Per-vertex highlight LEVEL (0 none, 1 selection, 2 hover). Drives section
   *  emphasis under the custom layer: segment-hover (Move Segment tool,
   *  SurveyLayer.setHoveredSegment), single-section + panel-row hover
   *  (setHoveredSection) resolve to level 2; section selection (setSelectedSection)
   *  to level 1; the shader widens by the matching stock feature-state bump
   *  (hover 2x, selection 1.5x). Independent of dim (a movable connected section
   *  is highlighted and not dimmed; anchored sections are dimmed and excluded from
   *  the highlight). Rewritten in place on a hover/selection-set change. */
  highlight: Float32Array;
  /** Two triangles per segment, `segmentCount*6`, `UNSIGNED_INT`, in draw
   *  order: by shot kind, reference first, real last ({@link writeLineDrawOrder}). */
  indices: Uint32Array;
  /** Per-section `[segStart, segCount]` pairs (`sections.length * 2`), in
   *  section declaration order. `segStart` is the first drawn segment index for
   *  the section; `segCount` the number of valid (drawn) segments. Lets the
   *  incremental edit path re-emit only a changed section's contiguous vertex
   *  range instead of the whole buffer. CPU-only metadata, not uploaded. */
  sectionRanges: Uint32Array;
  /** Total attribute + index bytes (for cost logging). */
  byteLength: number;
}

export interface CircleBuffers {
  stationCount: number;
  vertexCount: number;
  /** Station center (anchor-relative), `vertexCount*2`. */
  center: Float32Array;
  /** Unit quad corner in {-1,-1}..{-1,1}, `vertexCount*2`. */
  corner: Float32Array;
  /** Per-vertex fill RGBA8. */
  fill: Uint8Array;
  /** Per-vertex stroke RGBA8. */
  stroke: Uint8Array;
  /** Per-vertex size multiplier applied to BOTH radius and stroke width (1 for
   *  a normal station, 2 for a zero-datum station). Mirrors the stock stations
   *  layer's `['get','isZeroDatum']` cases: radius `baseRadius*2` and stroke
   *  `2` (vs the default `1`), i.e. a uniform 2x on both, since the custom
   *  layer's base stroke is always 1px. */
  sizeMul: Float32Array;
  /** Per-vertex dim flag (1 = dimmed to 0.25, else 0). Mirrors the stock
   *  stations layer's anchored / immovable circle-opacity + circle-stroke-
   *  opacity 0.25 cases (move-segment / move-station tools). Rewritten in place
   *  on an anchored/immovable set change. */
  dim: Float32Array;
  /** Per-vertex hide flag (1 = collapse the station quad to a clipped point).
   *  Circle counterpart of {@link LineBuffers.hide}: the drag-begin path marks a
   *  moving station hidden without omitting it from the buffers. */
  hide: Float32Array;
  /** Per-vertex highlight LEVEL (0 none, 1 selection, 2 hover). Circle
   *  counterpart of {@link LineBuffers.highlight}: the shader enlarges highlighted
   *  stations by the matching stock feature-state radius bump (hover 1.5x,
   *  selection 1.25x). Rewritten in place on a hover/selection-set change. */
  highlight: Float32Array;
  /** Per-vertex station kind mask (see `computeStationAllReference`: 0 = a
   *  Normal shot touches it, 1 = only Jumps, 2 = only References, 3 = both
   *  virtual kinds or no legs). The shader collapses a station when every kind
   *  that touches it is hidden; {@link writeCircleDrawOrder} ranks by it. */
  allReference: Float32Array;
  /** Two triangles per station, `stationCount*6`, `UNSIGNED_INT`, in draw
   *  order: by station rank, worst first ({@link writeCircleDrawOrder}). */
  indices: Uint32Array;
  byteLength: number;
}

/**
 * Which features the custom buffers must SKIP, mirroring the MapLibre
 * setFilter exclusions the stock bulk layers apply during a move drag
 * (SurveyLayer.beginDragPreview / beginStationDragPreview). Without this the
 * custom layer keeps drawing the originals under the drag preview.
 *
 * Key types are chosen so the SoA iteration can test membership O(1):
 *  - `sectionIds`         : segment drag hides whole connected sections (both
 *                           shots and stations) - tested against `section.id`
 *                           (lines) and `data.stationSectionIds[g]` (circles).
 *  - `stationGlobalIndices`: station / shot move hides specific stations -
 *                           tested against the circle loop's global index `g`.
 *  - `shotGlobalIndices`  : station / shot move hides specific shots - tested
 *                           against the line loop's `section.shotOffset + i`.
 *
 * All fields optional; an absent (or `undefined`) exclusion is the zero-cost
 * hot path (`undefined?.has(...)` short-circuits, no allocation).
 */
export interface CustomRenderExclusion {
  sectionIds?: ReadonlySet<number>;
  stationGlobalIndices?: ReadonlySet<number>;
  shotGlobalIndices?: ReadonlySet<number>;
  /** Sections whose NAME label additionally DRAG_HIDEs during a PARTIAL station /
   *  shot drag (SurveyLayer maps the moved globals back to their sections). The
   *  geometry hide + the shot / station label hide still key off the specific
   *  globals above; this drives ONLY the section-name DRAG_HIDE in the LABEL
   *  controller and is IGNORED by the geometry controller (a whole-section drag
   *  already carries its name via `sectionIds`, so it need not be listed here). */
  affectedSectionIds?: ReadonlySet<number>;
}

/** (sectionId, stationIndex) -> global station index. Mirrors vtSurveyBuild. */
export function buildStationLookup(data: SurveyData): Map<number, Map<number, number>> {
  const lookup = new Map<number, Map<number, number>>();
  for (let g = 0; g < data.stationCount; g++) {
    const sid = data.stationSectionIds[g]!;
    let inner = lookup.get(sid);
    if (!inner) {
      inner = new Map();
      lookup.set(sid, inner);
    }
    inner.set(data.stationIndices[g]!, g);
  }
  return lookup;
}

/**
 * Per global station index: which kinds of shot touch the station, as a small
 * mask (#27).
 *
 *   0 = at least one Normal shot touches it - always drawn
 *   1 = only Jumps
 *   2 = only References
 *   3 = both virtual kinds, no Normal
 *
 * `0` keeps the meaning the old boolean had (`allShotsReference === false`), so
 * the common case is unchanged. The virtual cases are split because the toggles
 * are: a station whose every leg is a Reference must disappear when References
 * are hidden, even while Jumps are still drawn. A single "any virtual kind is
 * visible" test kept an interactive orphan marker on it.
 *
 * A station with no feature-producing shots at all stays `3`, i.e. hidden only
 * when both kinds are off - it has no legs to follow either way.
 */
export function computeStationAllReference(
  data: SurveyData,
  lookup: Map<number, Map<number, number>>,
): Float32Array {
  // Start at "both virtual kinds" (3) - the no-legs default - then narrow:
  // a Normal shot forces 0, and each virtual kind contributes only its own bit.
  const mask = new Float32Array(data.stationCount).fill(3);
  const seen = new Uint8Array(data.stationCount);
  const touch = (global: number, bit: number) => {
    if (mask[global] === 0) return; // a Normal shot already won
    if (bit === 0) {
      mask[global] = 0;
      seen[global] = 1;
      return;
    }
    // First virtual touch replaces the no-legs default rather than OR-ing into it.
    mask[global] = seen[global] ? mask[global]! | bit : bit;
    seen[global] = 1;
  };
  for (const section of data.sections) {
    const inner = lookup.get(section.id);
    if (!inner) continue;
    for (let i = 0; i < section.shotCount; i++) {
      const shotGlobalIdx = section.shotOffset + i;
      const fromGlobal = inner.get(i);
      const toGlobal = inner.get(i + 1);
      // Only VALID shots (both endpoints present) count, matching the stock
      // build's `stationsWithNonReferenceShot` (features.ts) and the set of
      // shots the line buffers actually draw.
      if (fromGlobal === undefined || toGlobal === undefined) continue;
      const kind = shotKindFromFlags(data.shotFlags[shotGlobalIdx]!);
      const bit = kind === 'Jump' ? 1 : kind === 'Reference' ? 2 : 0;
      touch(fromGlobal, bit);
      touch(toGlobal, bit);
    }
  }
  return mask;
}

/**
 * Project every station meters->mercator once and store anchor-relative float32.
 * Uses the SAME two-function pass as the vt build so the custom layer lands in
 * the same coordinate space as the stock layers.
 */
export function projectStationsRel(
  data: SurveyData,
  gpsOrigin: GpsOrigin | null,
  anchorX: number,
  anchorY: number,
): StationRel {
  const n = data.stationCount;
  const rel = new Float32Array(n * 2);
  for (let g = 0; g < n; g++) {
    const ll = metersToMapCoords(data.stationCoords[g * 3]!, data.stationCoords[g * 3 + 1]!, gpsOrigin);
    const [mx, my] = lngLatToMercator(ll[0], ll[1]);
    rel[g * 2] = mx - anchorX;
    rel[g * 2 + 1] = my - anchorY;
  }
  return rel;
}

/** Count valid shots (both endpoints present), matching vtSurveyBuild. When
 *  `exclude` is given, excluded sections / shots are skipped so the count stays
 *  in lock-step with {@link buildLineBuffers}. */
export function countValidShots(
  data: SurveyData,
  lookup: Map<number, Map<number, number>>,
  exclude?: CustomRenderExclusion,
): number {
  const exSections = exclude?.sectionIds;
  const exShots = exclude?.shotGlobalIndices;
  let count = 0;
  for (const section of data.sections) {
    if (exSections?.has(section.id)) continue;
    const inner = lookup.get(section.id);
    if (!inner) continue;
    for (let i = 0; i < section.shotCount; i++) {
      if (exShots?.has(section.shotOffset + i)) continue;
      if (inner.get(i) !== undefined && inner.get(i + 1) !== undefined) count++;
    }
  }
  return count;
}

/**
 * Build the per-segment quad buffers for shots. Each valid shot becomes 4
 * vertices (A-left, A-right, B-left, B-right) and 6 indices (two triangles).
 * `colorForShot(globalShotIdx, sectionId)` resolves the packed RGBA for the
 * whole segment.
 *
 * `colorForShotEndpoints`, when supplied, wins over `colorForShot`: it returns
 * `[fromColor, toColor]` and the two A-vertices get the from-color while the
 * two B-vertices get the to-color, so the GPU interpolates a per-shot color
 * ramp along the segment. This reproduces the stock `survey-shot-gradients`
 * layer's depth ramp (from-endpoint depth color -> to-endpoint depth color)
 * natively, with no gradient sub-segments. Absent -> the flat, whole-segment
 * `colorForShot` path (byte-identical to the previous behavior).
 */
// Per-segment vertex template: [endpoint, side] for the 4 corners.
// v0 A-left, v1 A-right, v2 B-left, v3 B-right. Exported so the drag-ghost
// tessellator (ghostTessellate.ts) emits capsule quads with the SAME vertex
// layout - one source of truth for the extrusion the round-cap shaders assume.
export const LINE_EP = [0, 0, 1, 1];
export const LINE_SIDE = [-1, 1, -1, 1];

/**
 * Optional per-shot resolvers/predicates a section emit reads. Bundled so the
 * full build and the incremental section recompute pass the identical set (a
 * prerequisite for the byte-identical incremental guarantee).
 */
export interface LineSectionInputs {
  colorForShot: (globalShotIdx: number, sectionId: number) => Rgba8;
  colorForShotEndpoints?: ((globalShotIdx: number, sectionId: number) => readonly [Rgba8, Rgba8]) | undefined;
  /** Dim LEVEL for a shot (0 none, 0.5 gas-beyond-reach, 1 anchored/immovable),
   *  or a boolean the writer coerces to 0 / 1 (legacy callers / tests). */
  dimForShot?: ((globalShotIdx: number, sectionId: number) => number | boolean) | undefined;
  hideForShot?: ((globalShotIdx: number, sectionId: number) => boolean) | undefined;
  /** Emphasis LEVEL for a shot (0 none, 1 selection, 2 hover), or a boolean the
   *  writer coerces to 0 / 1 (legacy callers / tests). */
  highlightForShot?: ((globalShotIdx: number, sectionId: number) => number | boolean) | undefined;
}

/**
 * Emit one section's valid shots into `b`, starting at drawn-segment `startSeg`,
 * returning the next free segment index. The single writer used by BOTH the full
 * build (looping every section) and the incremental recompute (re-emitting one
 * changed section into its stored range), so a section rebuilt incrementally is
 * byte-for-byte identical to the same section in a full rebuild. `exShots` skips
 * individually-excluded shots (build-time exclusion path only; the live drag
 * path leaves it undefined and hides via {@link LineBuffers.hide}).
 */
function emitLineSection(
  b: LineBuffers,
  section: SectionMetadata,
  startSeg: number,
  data: SurveyData,
  stationRel: StationRel,
  lookup: Map<number, Map<number, number>>,
  inputs: LineSectionInputs,
  exShots: ReadonlySet<number> | undefined,
): number {
  const inner = lookup.get(section.id);
  if (!inner) return startSeg;
  const { posA, posB, endpoint, side, colorFrom, colorTo, reference, dim, hide, highlight } = b;
  const { colorForShot, colorForShotEndpoints, dimForShot, hideForShot, highlightForShot } = inputs;
  let seg = startSeg;
  for (let i = 0; i < section.shotCount; i++) {
    const shotGlobalIdx = section.shotOffset + i;
    if (exShots?.has(shotGlobalIdx)) continue;
    const fromGlobal = inner.get(i);
    const toGlobal = inner.get(i + 1);
    if (fromGlobal === undefined || toGlobal === undefined) continue;
    // Per-endpoint colors (depth gradient) win; else the flat segment color at
    // both ends. Both endpoint colors are replicated on every vertex so the
    // fragment can interpolate from->to by the clamped along-fraction (round
    // caps, shaders.ts); in the flat path colA === colB.
    const ends = colorForShotEndpoints?.(shotGlobalIdx, section.id);
    const colA = ends ? ends[0] : colorForShot(shotGlobalIdx, section.id);
    const colB = ends ? ends[1] : colA;
    // #27: the attribute carries the ShotKind discriminant (0/1/2), not a
    // bool, so the shader can fade and hide jumps and references separately.
    // Every "is virtual" test still reads as > 0.5.
    const isRef = shotKindToFlags(shotKindFromFlags(data.shotFlags[shotGlobalIdx]!));
    const isDim = flagValue(dimForShot?.(shotGlobalIdx, section.id));
    const isHide = hideForShot?.(shotGlobalIdx, section.id) === true ? 1 : 0;
    const isHighlight = flagValue(highlightForShot?.(shotGlobalIdx, section.id));
    const ax = stationRel[fromGlobal * 2]!;
    const ay = stationRel[fromGlobal * 2 + 1]!;
    const bx = stationRel[toGlobal * 2]!;
    const by = stationRel[toGlobal * 2 + 1]!;

    const base = seg * 4;
    for (let c = 0; c < 4; c++) {
      const v = base + c;
      posA[v * 2] = ax;
      posA[v * 2 + 1] = ay;
      posB[v * 2] = bx;
      posB[v * 2 + 1] = by;
      endpoint[v] = LINE_EP[c]!;
      side[v] = LINE_SIDE[c]!;
      colorFrom[v * 4] = colA[0];
      colorFrom[v * 4 + 1] = colA[1];
      colorFrom[v * 4 + 2] = colA[2];
      colorFrom[v * 4 + 3] = colA[3];
      colorTo[v * 4] = colB[0];
      colorTo[v * 4 + 1] = colB[1];
      colorTo[v * 4 + 2] = colB[2];
      colorTo[v * 4 + 3] = colB[3];
      reference[v] = isRef;
      dim[v] = isDim;
      hide[v] = isHide;
      highlight[v] = isHighlight;
    }
    seg++;
  }
  return seg;
}

/** Index pattern of one line segment quad (two triangles over its 4 vertices). */
const LINE_TRIS = [0, 1, 2, 2, 1, 3] as const;
/** Index pattern of one station quad. */
const CIRCLE_TRIS = [0, 1, 2, 0, 2, 3] as const;

/**
 * Write the line index buffer in draw order: every Reference segment first, then
 * Jump, then Normal (the per-vertex kind in `b.reference` is the rank), then the
 * raised segments, segment order within each. One draw call, no depth test:
 * where a jump or reference line crosses or runs over the real line, the real
 * line ends on top, and a raised section ends above them all. `raised` is a
 * per-vertex flag array in the buffers' layout (`computeLineDim` builds one);
 * omitted, nothing is raised. Line counterpart of {@link writeCircleDrawOrder};
 * returns whether any index changed, so a caller re-uploads only when a segment
 * moved.
 */
export function writeLineDrawOrder(b: LineBuffers, raised?: Float32Array | null): boolean {
  const { reference } = b;
  return writeQuadDrawOrder(b.indices, b.segmentCount, (s) => lineKindRank(reference[s * 4]!), raised, LINE_TRIS);
}

/**
 * Shared body of {@link writeLineDrawOrder} / {@link writeCircleDrawOrder}: a
 * stable counting sort of the `count` quads into four buckets, drawn in order -
 * rank 2 (reference), rank 1 (jump), rank 0 (real), then raised (`raised[4s]`
 * set). Returns whether any quad changed position.
 */
function writeQuadDrawOrder(
  indices: Uint32Array,
  count: number,
  rankOf: (slot: number) => 0 | 1 | 2,
  raised: Float32Array | null | undefined,
  tris: readonly number[],
): boolean {
  const bucketOf = (s: number): number => (raised && raised[s * 4]! > 0.5 ? 3 : 2 - rankOf(s));
  const start = [0, 0, 0, 0];
  for (let s = 0; s < count; s++) start[bucketOf(s)]!++;
  // Counts -> start offsets.
  let acc = 0;
  for (let k = 0; k < 4; k++) {
    const n = start[k]!;
    start[k] = acc;
    acc += n;
  }
  let changed = false;
  for (let s = 0; s < count; s++) {
    const io = start[bucketOf(s)]!++ * 6;
    const base = s * 4;
    if (indices[io] !== base) changed = true;
    for (let k = 0; k < 6; k++) indices[io + k] = base + tris[k]!;
  }
  return changed;
}

/** Draw rank of a segment from its per-vertex kind attribute (0 Normal, 1 Jump,
 *  2 Reference - already the rank; clamped for safety). */
function lineKindRank(kind: number): 0 | 1 | 2 {
  return kind >= 1.5 ? 2 : kind >= 0.5 ? 1 : 0;
}

export function buildLineBuffers(
  data: SurveyData,
  stationRel: StationRel,
  lookup: Map<number, Map<number, number>>,
  colorForShot: (globalShotIdx: number, sectionId: number) => Rgba8,
  exclude?: CustomRenderExclusion,
  colorForShotEndpoints?: (globalShotIdx: number, sectionId: number) => readonly [Rgba8, Rgba8],
  dimForShot?: (globalShotIdx: number, sectionId: number) => number | boolean,
  hideForShot?: (globalShotIdx: number, sectionId: number) => boolean,
  highlightForShot?: (globalShotIdx: number, sectionId: number) => number | boolean,
): LineBuffers {
  const exSections = exclude?.sectionIds;
  const exShots = exclude?.shotGlobalIndices;
  const segmentCount = countValidShots(data, lookup, exclude);
  const vertexCount = segmentCount * 4;
  const posA = new Float32Array(vertexCount * 2);
  const posB = new Float32Array(vertexCount * 2);
  const endpoint = new Float32Array(vertexCount);
  const side = new Float32Array(vertexCount);
  const colorFrom = new Uint8Array(vertexCount * 4);
  const colorTo = new Uint8Array(vertexCount * 4);
  const reference = new Float32Array(vertexCount);
  const dim = new Float32Array(vertexCount);
  const hide = new Float32Array(vertexCount);
  const highlight = new Float32Array(vertexCount);
  const indices = new Uint32Array(segmentCount * 6);
  const sectionRanges = new Uint32Array(data.sections.length * 2);
  const byteLength =
    posA.byteLength + posB.byteLength + endpoint.byteLength + side.byteLength +
    colorFrom.byteLength + colorTo.byteLength + reference.byteLength + dim.byteLength +
    hide.byteLength + highlight.byteLength + indices.byteLength;
  const b: LineBuffers = {
    segmentCount, vertexCount, posA, posB, endpoint, side, colorFrom, colorTo, reference, dim, hide,
    highlight, indices, sectionRanges, byteLength,
  };

  const inputs: LineSectionInputs = { colorForShot, colorForShotEndpoints, dimForShot, hideForShot, highlightForShot };
  let seg = 0;
  for (let si = 0; si < data.sections.length; si++) {
    const section = data.sections[si]!;
    const segStart = seg;
    // A section-level exclusion drops the whole section (segCount 0); otherwise
    // emit its valid shots. The range map is recorded either way so the
    // incremental path can address every section by index.
    if (!exSections?.has(section.id)) {
      seg = emitLineSection(b, section, seg, data, stationRel, lookup, inputs, exShots);
    }
    sectionRanges[si * 2] = segStart;
    sectionRanges[si * 2 + 1] = seg - segStart;
  }
  writeLineDrawOrder(b);
  return b;
}

/**
 * Re-emit ONE section's line vertices in place (incremental edit path). The
 * section's drawn-segment range is stable (unchanged SoA layout is the caller's
 * precondition), so this rewrites exactly the slots {@link buildLineBuffers}
 * wrote for that section - positions, colors, reference/dim/hide flags - leaving
 * every other section's bytes untouched. Leaves the index buffer alone: a caller
 * that changed a shot's kind calls {@link writeLineDrawOrder}. Never called with a build-time
 * exclusion (the live path hides via the per-vertex flag), so `exShots` is
 * omitted and the re-emit stays in lock-step with the stored range.
 */
export function recomputeLineSection(
  b: LineBuffers,
  sectionIndex: number,
  data: SurveyData,
  stationRel: StationRel,
  lookup: Map<number, Map<number, number>>,
  inputs: LineSectionInputs,
): void {
  const section = data.sections[sectionIndex]!;
  const startSeg = b.sectionRanges[sectionIndex * 2]!;
  emitLineSection(b, section, startSeg, data, stationRel, lookup, inputs, undefined);
}

/**
 * Rewrite ONLY the per-vertex line color attributes (`colorFrom` / `colorTo`) in
 * place, replaying {@link buildLineBuffers}' exact valid-shot iteration so the
 * k-th drawn segment's 4 verts line up byte-for-byte with a full rebuild. The
 * color-only fast path for a GLOBAL recolor (analytical-overlay enter/exit,
 * theme change, depth / severity toggle) where geometry is unchanged: positions,
 * reference / dim / hide / highlight, and indices are left untouched, so only the
 * two color buffers need re-uploading. Live (unexcluded) path only, matching the
 * incremental preconditions. In flat modes `colorFrom === colorTo`; the
 * depth-endpoint resolver, when supplied, splits them (per-shot depth ramp).
 */
export function recomputeLineColors(
  b: LineBuffers,
  data: SurveyData,
  lookup: Map<number, Map<number, number>>,
  colorForShot: (globalShotIdx: number, sectionId: number) => Rgba8,
  colorForShotEndpoints:
    | ((globalShotIdx: number, sectionId: number) => readonly [Rgba8, Rgba8])
    | undefined,
): void {
  const { colorFrom, colorTo } = b;
  let seg = 0;
  for (const section of data.sections) {
    const inner = lookup.get(section.id);
    if (!inner) continue;
    for (let i = 0; i < section.shotCount; i++) {
      const fromGlobal = inner.get(i);
      const toGlobal = inner.get(i + 1);
      if (fromGlobal === undefined || toGlobal === undefined) continue;
      const shotGlobalIdx = section.shotOffset + i;
      const ends = colorForShotEndpoints?.(shotGlobalIdx, section.id);
      const colA = ends ? ends[0] : colorForShot(shotGlobalIdx, section.id);
      const colB = ends ? ends[1] : colA;
      const base = seg * 4;
      for (let c = 0; c < 4; c++) {
        const o = (base + c) * 4;
        colorFrom[o] = colA[0];
        colorFrom[o + 1] = colA[1];
        colorFrom[o + 2] = colA[2];
        colorFrom[o + 3] = colA[3];
        colorTo[o] = colB[0];
        colorTo[o + 1] = colB[1];
        colorTo[o + 2] = colB[2];
        colorTo[o + 3] = colB[3];
      }
      seg++;
    }
  }
}

/**
 * Rewrite ONLY the per-vertex circle color attributes (`fill` / `stroke`) in
 * place, replaying {@link buildCircleBuffers}' live-path station iteration
 * (output slot == global index). Circle counterpart of {@link recomputeLineColors}
 * for the color-only fast path; leaves center / size / dim / hide / highlight /
 * all-reference untouched.
 */
export function recomputeCircleColors(
  b: CircleBuffers,
  data: SurveyData,
  fillForStation: (globalIdx: number) => Rgba8,
  strokeForStation: (globalIdx: number) => Rgba8,
): void {
  const { fill, stroke } = b;
  for (let g = 0; g < data.stationCount; g++) {
    const [fr, fg, fb, fa] = fillForStation(g);
    const [sr, sg, sb, sa] = strokeForStation(g);
    const base = g * 4;
    for (let c = 0; c < 4; c++) {
      const o = (base + c) * 4;
      fill[o] = fr;
      fill[o + 1] = fg;
      fill[o + 2] = fb;
      fill[o + 3] = fa;
      stroke[o] = sr;
      stroke[o + 1] = sg;
      stroke[o + 2] = sb;
      stroke[o + 3] = sa;
    }
  }
}

/**
 * Recompute ONLY the per-vertex line dim flags for the CURRENT anchored /
 * immovable set, replaying {@link buildLineBuffers}' exact skip + iteration so
 * the k-th drawn segment lines up vertex-for-vertex. Lets the controller refresh
 * dimming on a tool-activation change without the full geometry rebuild (no
 * reprojection, no color packing). The returned length equals the line buffers'
 * `vertexCount`; a parity test locks it against the build path's `dim` array.
 */
export function computeLineDim(
  data: SurveyData,
  lookup: Map<number, Map<number, number>>,
  dimForShot: (globalShotIdx: number, sectionId: number) => number | boolean,
  exclude?: CustomRenderExclusion,
): Float32Array {
  const exSections = exclude?.sectionIds;
  const exShots = exclude?.shotGlobalIndices;
  const segmentCount = countValidShots(data, lookup, exclude);
  const dim = new Float32Array(segmentCount * 4);
  let seg = 0;
  for (const section of data.sections) {
    if (exSections?.has(section.id)) continue;
    const inner = lookup.get(section.id);
    if (!inner) continue;
    for (let i = 0; i < section.shotCount; i++) {
      const shotGlobalIdx = section.shotOffset + i;
      if (exShots?.has(shotGlobalIdx)) continue;
      if (inner.get(i) === undefined || inner.get(i + 1) === undefined) continue;
      const isDim = flagValue(dimForShot(shotGlobalIdx, section.id));
      const base = seg * 4;
      for (let c = 0; c < 4; c++) dim[base + c] = isDim;
      seg++;
    }
  }
  return dim;
}

// Unit corners: v0 bottom-left, v1 bottom-right, v2 top-right, v3 top-left.
// Exported so the drag-ghost tessellator reuses the same station quad template.
export const CIRCLE_CX = [-1, 1, 1, -1];
export const CIRCLE_CY = [-1, -1, 1, 1];

/** Optional per-station resolvers/predicates one station emit reads. */
export interface CircleStationInputs {
  fillForStation: (globalIdx: number) => Rgba8;
  strokeForStation: (globalIdx: number) => Rgba8;
  /** Dim LEVEL for a station (0 none, 0.5 gas-beyond-reach, 1 anchored/immovable),
   *  or a boolean the writer coerces to 0 / 1 (legacy callers / tests). */
  dimForStation?: ((globalIdx: number) => number | boolean) | undefined;
  hideForStation?: ((globalIdx: number) => boolean) | undefined;
  /** Emphasis LEVEL for a station (0 none, 1 selection, 2 hover), or a boolean
   *  the writer coerces to 0 / 1 (legacy callers / tests). */
  highlightForStation?: ((globalIdx: number) => number | boolean) | undefined;
}

/**
 * Emit one station's quad (4 verts) into `b` at output slot `out`, reading its
 * center/color/size/flags for global station index `g`. The single writer shared
 * by the full build and the incremental station recompute. In the live
 * (unexcluded) path `out === g`, so a station's vertices are always at
 * `[4g, 4g+4)` - the property the incremental path relies on to address a
 * changed station without a compaction map. The index buffer is written apart,
 * by {@link writeCircleDrawOrder}, since the draw order is by rank, not slot.
 */
function emitStation(
  b: CircleBuffers,
  out: number,
  g: number,
  data: SurveyData,
  stationRel: StationRel,
  inputs: CircleStationInputs,
  allRef: number,
): void {
  const { center, corner, fill, stroke, sizeMul, dim, hide, highlight, allReference } = b;
  const { fillForStation, strokeForStation, dimForStation, hideForStation, highlightForStation } = inputs;
  const cx = stationRel[g * 2]!;
  const cy = stationRel[g * 2 + 1]!;
  const [fr, fg, fb, fa] = fillForStation(g);
  const [sr, sg, sb, sa] = strokeForStation(g);
  // Zero-datum stations render at 2x radius + 2x stroke (stock parity).
  const sm = (data.stationFlags[g]! & STATION_FLAG_IS_ZERO_DATUM) !== 0 ? 2 : 1;
  const dm = flagValue(dimForStation?.(g));
  const hd = hideForStation?.(g) === true ? 1 : 0;
  const hl = flagValue(highlightForStation?.(g));
  const base = out * 4;
  for (let c = 0; c < 4; c++) {
    const v = base + c;
    center[v * 2] = cx;
    center[v * 2 + 1] = cy;
    corner[v * 2] = CIRCLE_CX[c]!;
    corner[v * 2 + 1] = CIRCLE_CY[c]!;
    fill[v * 4] = fr;
    fill[v * 4 + 1] = fg;
    fill[v * 4 + 2] = fb;
    fill[v * 4 + 3] = fa;
    stroke[v * 4] = sr;
    stroke[v * 4 + 1] = sg;
    stroke[v * 4 + 2] = sb;
    stroke[v * 4 + 3] = sa;
    sizeMul[v] = sm;
    dim[v] = dm;
    hide[v] = hd;
    highlight[v] = hl;
    allReference[v] = allRef;
  }
}

/**
 * Write the circle index buffer in draw order: every reference-rank station
 * first, then jump, then real (`stationKindRank` of each slot's kind mask), then
 * the raised stations, slot order within each. The circles draw in one call with
 * no depth test, so a later quad paints over an earlier one: where markers of
 * several sections coincide (a jump or reference section tied in at a real
 * station), the real-line marker ends on top, unless the other section is raised
 * (the selection's). The hit test picks the same one
 * (`SurveyGeometryIndex.nearestStation`).
 *
 * Reads the per-vertex kind mask already in `b.allReference`, so it works for
 * the compacted (excluded) build and after an incremental re-emit alike.
 * `raised` is a per-vertex flag array in the same layout (`computeCircleDim`
 * builds one); omitted, nothing is raised. Returns whether any index changed, so
 * a caller re-uploads the index buffer only when a station moved.
 */
export function writeCircleDrawOrder(b: CircleBuffers, raised?: Float32Array | null): boolean {
  const { allReference } = b;
  return writeQuadDrawOrder(
    b.indices, b.stationCount, (s) => stationKindRank(allReference[s * 4]!), raised, CIRCLE_TRIS,
  );
}

/**
 * Build the per-station quad buffers for station discs. Each station becomes 4
 * corner vertices and 6 indices, the indices in rank draw order
 * ({@link writeCircleDrawOrder}). `fillForStation` / `strokeForStation` resolve
 * packed RGBA per station (global index).
 */
export function buildCircleBuffers(
  data: SurveyData,
  stationRel: StationRel,
  fillForStation: (globalIdx: number) => Rgba8,
  strokeForStation: (globalIdx: number) => Rgba8,
  exclude?: CustomRenderExclusion,
  allReferenceFlags?: Float32Array,
  dimForStation?: (globalIdx: number) => number | boolean,
  hideForStation?: (globalIdx: number) => boolean,
  highlightForStation?: (globalIdx: number) => number | boolean,
): CircleBuffers {
  const exSections = exclude?.sectionIds;
  const exStations = exclude?.stationGlobalIndices;
  const skip = (g: number): boolean =>
    exStations?.has(g) === true || exSections?.has(data.stationSectionIds[g]!) === true;

  // Count first only when an exclusion is present; the no-exclusion hot path
  // keeps every station and skips the extra pass entirely.
  let stationCount = data.stationCount;
  if (exStations || exSections) {
    stationCount = 0;
    for (let g = 0; g < data.stationCount; g++) if (!skip(g)) stationCount++;
  }
  const vertexCount = stationCount * 4;
  const center = new Float32Array(vertexCount * 2);
  const corner = new Float32Array(vertexCount * 2);
  const fill = new Uint8Array(vertexCount * 4);
  const stroke = new Uint8Array(vertexCount * 4);
  const sizeMul = new Float32Array(vertexCount);
  const dim = new Float32Array(vertexCount);
  const hide = new Float32Array(vertexCount);
  const highlight = new Float32Array(vertexCount);
  const allReference = new Float32Array(vertexCount);
  const indices = new Uint32Array(stationCount * 6);
  const byteLength =
    center.byteLength + corner.byteLength + fill.byteLength + stroke.byteLength +
    sizeMul.byteLength + dim.byteLength + hide.byteLength + highlight.byteLength +
    allReference.byteLength + indices.byteLength;
  const b: CircleBuffers = {
    stationCount, vertexCount, center, corner, fill, stroke, sizeMul, dim, hide, highlight,
    allReference, indices, byteLength,
  };

  const inputs: CircleStationInputs = { fillForStation, strokeForStation, dimForStation, hideForStation, highlightForStation };
  let out = 0;
  for (let g = 0; g < data.stationCount; g++) {
    if (skip(g)) continue;
    emitStation(b, out, g, data, stationRel, inputs, allReferenceFlags ? allReferenceFlags[g]! : 0);
    out++;
  }
  writeCircleDrawOrder(b);
  return b;
}

/**
 * Re-emit ONE station's circle vertices in place (incremental edit path). Valid
 * only for the unexcluded (live) build where output slot equals global index, so
 * the station occupies `[4g, 4g+4)`. `allRef` is the station's current kind
 * mask (the caller recomputes it per changed section). Leaves the index buffer
 * alone: a caller that changed a station's rank calls
 * {@link writeCircleDrawOrder}.
 */
export function recomputeStation(
  b: CircleBuffers,
  g: number,
  data: SurveyData,
  stationRel: StationRel,
  inputs: CircleStationInputs,
  allRef: number,
): void {
  emitStation(b, g, g, data, stationRel, inputs, allRef);
}

/**
 * Reset one section's stations' all-reference flags in `allRef` (in place),
 * replaying {@link computeStationAllReference}'s per-section clear so the result
 * for those stations is identical to a global recompute. Used by the incremental
 * path when a section's shot reference flags change; `allRef[g]` depends only on
 * `g`'s own section's shots, so a scoped reset is exact.
 */
export function recomputeSectionAllReference(
  data: SurveyData,
  lookup: Map<number, Map<number, number>>,
  section: SectionMetadata,
  allRef: Float32Array,
): void {
  const inner = lookup.get(section.id);
  if (!inner) return;
  // Same mask as the full build (see `computeStationAllReference`): reset to the
  // no-legs default, then let each shot contribute only its own kind's bit. This
  // has to stay byte-identical to the full path - the incremental property test
  // compares every circle buffer against a from-scratch rebuild.
  const seen = new Set<number>();
  for (const g of inner.values()) allRef[g] = 3;
  const touch = (global: number, bit: number) => {
    if (allRef[global] === 0) return; // a Normal shot already won
    if (bit === 0) {
      allRef[global] = 0;
      seen.add(global);
      return;
    }
    allRef[global] = seen.has(global) ? allRef[global]! | bit : bit;
    seen.add(global);
  };
  for (let i = 0; i < section.shotCount; i++) {
    const fromGlobal = inner.get(i);
    const toGlobal = inner.get(i + 1);
    if (fromGlobal === undefined || toGlobal === undefined) continue;
    const kind = shotKindFromFlags(data.shotFlags[section.shotOffset + i]!);
    const bit = kind === 'Jump' ? 1 : kind === 'Reference' ? 2 : 0;
    touch(fromGlobal, bit);
    touch(toGlobal, bit);
  }
}

/**
 * Recompute ONLY the per-vertex circle dim flags for the CURRENT anchored /
 * immovable set, replaying {@link buildCircleBuffers}' exact skip + compaction
 * so the k-th drawn station lines up vertex-for-vertex. Companion to
 * {@link computeLineDim} for the cheap tool-activation dim refresh.
 */
export function computeCircleDim(
  data: SurveyData,
  dimForStation: (globalIdx: number) => number | boolean,
  exclude?: CustomRenderExclusion,
): Float32Array {
  const exSections = exclude?.sectionIds;
  const exStations = exclude?.stationGlobalIndices;
  const skip = (g: number): boolean =>
    exStations?.has(g) === true || exSections?.has(data.stationSectionIds[g]!) === true;
  let stationCount = data.stationCount;
  if (exStations || exSections) {
    stationCount = 0;
    for (let g = 0; g < data.stationCount; g++) if (!skip(g)) stationCount++;
  }
  const dim = new Float32Array(stationCount * 4);
  let out = 0;
  for (let g = 0; g < data.stationCount; g++) {
    if (skip(g)) continue;
    const isDim = flagValue(dimForStation(g));
    const base = out * 4;
    for (let c = 0; c < 4; c++) dim[base + c] = isDim;
    out++;
  }
  return dim;
}
