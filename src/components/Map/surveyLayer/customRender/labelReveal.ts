/**
 * Per-label REVEAL-ZOOM computation for the custom-render LABEL pass - the
 * declutter + decimation core (replaces the old boolean `labelCollide.ts`).
 *
 * PURE (no GL, no DOM, no MapLibre): given the viewport-scoped
 * {@link LabelDescriptor} set + each label's shaped em {@link ExtentEm} + a
 * {@link RevealInput} (build zoom, world-pixel scale, toggles), it returns one
 * float per label - the minimum LIVE ZOOM at which that label is visible. The
 * shader then draws a label iff `u_zoom >= a_reveal_zoom`, so the reveal value
 * folds BOTH the collision declutter AND the zoom-band decimation into a single
 * per-build threshold evaluated per frame (`labelShaders.ts`).
 *
 * WHY REVEAL ZOOM (fixes the three reported defects):
 *  1. Zoom updates no longer wait for a coalesced zoomend rebuild - admission is
 *     a per-frame shader compare, so zoom-in is instant and needs no rebuild.
 *  2. A zoom-band crossing never flashes the full set: there is no stale baked
 *     "hidden" bit to lag behind the new band; a label simply appears when the
 *     live zoom reaches its reveal threshold.
 *  3. The visible set is MONOTONE in zoom by construction - for a fixed build,
 *     `{ z >= reveal }` only grows as `z` grows, so a label visible at a lower
 *     zoom can never vanish at a higher one.
 *
 * MONOTONE CORRECTNESS. Screen separation between two anchors = mercator delta ×
 * world-pixel-scale(zoom), which is monotonically INCREASING in zoom (the map is
 * north-up, 0-pitch), while box sizes (text px) grow only mildly (9->12 px). So
 * pairwise overlap is monotone-DECREASING in zoom: once two boxes clear at some
 * zoom they stay clear at every higher zoom. That is what makes a single reveal
 * threshold well-defined and lets a zoom-LADDER of greedy grid passes produce a
 * declutter that is non-overlapping at EVERY zoom simultaneously.
 *
 * ALGORITHM (zoom-ladder greedy, chosen for provable monotonicity + determinism
 * over analytic elegance): candidates are ordered ONCE by a ZOOM-INDEPENDENT
 * priority - (decimation tier asc, kind priority, ordinal, stable id, kind) - then a
 * ladder of 0.5-zoom steps runs from the coarsest candidate minzoom up to the
 * emit horizon (`buildZoom + slack`). At each step, already-revealed winners are
 * re-seeded into the grid at THAT zoom, then still-unrevealed admitted candidates
 * are placed greedily in priority order; a candidate that first fits at zoom `z`
 * gets `reveal = z`. Seeding a revealed winner every step guarantees a
 * lower-zoom label keeps its slot, so a later finer-tier label can never evict
 * it - the visible set only ever grows.
 *
 * DETERMINISM ACROSS REBUILDS. The priority order uses a STABLE per-label id
 * (global shot / station index, or section id) rather than the array position,
 * so a pan-settle rebuild whose candidate membership shifts still resolves an
 * unchanged neighbourhood identically (padding absorbs the edge effects). The id
 * alone is not unique - a shot comment and a station comment share a kind
 * priority and can share a number (global shot 5, global station 5) - so the
 * kind breaks that tie; the array position only orders exact duplicates.
 */
import { LABEL_KIND, LABEL_FLAG, type LabelKind } from './labelShaders';
import type { LabelDescriptor } from './labelTessellate';
import type { ExtentEm } from './labelLayout';
import { LABEL_KIND_STYLE, interpolateStops } from './labelStyle';
import { decimationAdmitZoom, decimationTier } from './labelDecimation';

/** Reveal value for a label that never clears within the emit horizon (not
 *  emitted). Finite reveal values are always `<= buildZoom + slack`. */
export const REVEAL_NEVER = Infinity;

/** Zoom-ladder granularity (design suggests 0.5-zoom steps). */
export const REVEAL_LADDER_STEP = 0.5;

/** Emit slack in zoom levels: labels are computed / emitted up to
 *  `buildZoom + EMIT_SLACK`, so a modest zoom-in needs no rebuild. ~2 keeps the
 *  emitted vertex count bounded on huge surveys while covering a full band of
 *  headroom above the current zoom. This is the BOUNDED horizon, used only when
 *  the viewport-scoped candidate set is too large to emit in full (see
 *  {@link emitSlackFor} / {@link ADAPTIVE_EMIT_MAX}). */
export const EMIT_SLACK = 2;

/** The app's maximum zoom (`config.map.zoom_max`, default 22) - the top of the
 *  ADAPTIVE-EMIT ladder. When the viewport candidate set is small enough to emit
 *  in full (at/below {@link ADAPTIVE_EMIT_MAX}), the reveal ladder runs all the
 *  way to here so EVERY label that ever clears within reachable zoom is emitted
 *  with its true reveal. A zoom-in then needs NO rebuild - admission is a
 *  per-frame shader compare - which fixes the "newly-revealed labels appear late
 *  when zooming in at high zoom" report (the batch reveals at the z16 / z18
 *  decimation band edges used to fall past the old bounded horizon and only
 *  reappear after a coalesced settle rebuild). Overshooting the real max (build
 *  zoom already near/above the ceiling) is harmless: a reveal beyond the app max
 *  is never reached. */
export const REVEAL_CEILING = 22;

/** Adaptive-emit candidate threshold. At/below this many viewport-scoped
 *  candidates a build emits the FULL set (ladder to {@link REVEAL_CEILING}) so
 *  zoom-in is rebuild-free; above it the bounded {@link EMIT_SLACK} horizon caps
 *  the emitted vertex count (the 100k-candidate low-zoom case). Chosen from the
 *  reveal-pass benchmark: at high zoom the viewport scopes candidates far below
 *  this (adaptive path, sub-10ms rebuild), while a crowded low-zoom viewport
 *  (tens of thousands) stays on the bounded path (emitted count strictly capped,
 *  rebuilt lazily on settle). */
export const ADAPTIVE_EMIT_MAX = 2500;

/** The emit-horizon slack for a build of `candidateCount` viewport-scoped
 *  candidates at `buildZoom`: the bounded {@link EMIT_SLACK} when the set is too
 *  large to emit in full, else the ADAPTIVE slack running the ladder to
 *  {@link REVEAL_CEILING} (never below EMIT_SLACK). `maxCandidates` is
 *  overridable so a test can force either branch on a small fixture. */
export function emitSlackFor(
  candidateCount: number,
  buildZoom: number,
  maxCandidates: number = ADAPTIVE_EMIT_MAX,
): number {
  if (candidateCount > maxCandidates) return EMIT_SLACK;
  return Math.max(EMIT_SLACK, REVEAL_CEILING - buildZoom);
}

/** Default grid cell (CSS px): a touch larger than the tallest label so a label
 *  spans only a couple of cells vertically. Perf-only; overlap results do not
 *  depend on it. */
export const DEFAULT_CELL_PX = 48;

/** Screen-space axis-aligned bounding box, in CSS px. */
export interface ScreenBox {
  minX: number;
  minY: number;
  maxX: number;
  maxY: number;
}

/** Inputs the reveal pass needs beyond the descriptors + their em extents. */
export interface RevealInput {
  /** Zoom at build time - the base for the world-pixel-scale ramp + the emit
   *  horizon (`buildZoom + slack`). */
  buildZoom: number;
  /** Screen CSS px per unit of anchor-relative mercator AT `buildZoom`
   *  (`viewportWidthPx / viewportMercatorSpan`). Scaled by `2^(z - buildZoom)`
   *  for other ladder zooms (mercator is isotropic, north-up, 0-pitch). */
  worldPixelScale: number;
  /** Per-kind toggle state (setShow*), indexed by {@link LabelKind}. A
   *  toggled-off kind is not a collision CANDIDATE (frees space) but its labels
   *  still get a reveal so they can be shown by a later toggle-on without a full
   *  rebuild. */
  kindVisible: readonly boolean[];
  /** setShowReferenceShots(false) → reference / all-reference labels are not
   *  collision candidates. */
  hideReference: boolean;
  /** #27: independent of hideReference, mirroring the split display toggles. */
  hideJump: boolean;
  /** A comment tool (comment / move_comment) is active → the two comment kinds
   *  are OVERLAP-EXEMPT: they skip the collision contest and are revealed
   *  uncontested at their decimation floor (their `minzoom`), so ALL comments
   *  stay visible / selectable while editing (parity with the deleted stock
   *  `text-allow-overlap`). Non-comment kinds are unaffected. (C1) */
  commentOverlapExempt?: boolean;
  /** Emit horizon slack in zoom levels (default {@link EMIT_SLACK}). */
  slack?: number;
  /** Ladder step (default {@link REVEAL_LADDER_STEP}). */
  step?: number;
  /** Grid cell size in CSS px (perf only; correctness is cell-size independent).
   *  Defaults to {@link DEFAULT_CELL_PX}. */
  cellSize?: number;
}

/** Collision priority (LOWER places first / wins overlaps), per design §5. Used
 *  as a SECONDARY sort key under the decimation tier. */
export function labelPriority(kind: LabelKind, firstInSection: boolean): number {
  switch (kind) {
    case LABEL_KIND.SECTION_NAME:
      return 0;
    case LABEL_KIND.STATION_LABEL:
      return firstInSection ? 1 : 2;
    case LABEL_KIND.SHOT_LABEL:
      return 3;
    default:
      return 4; // station + shot comments
  }
}

/** A stable per-label identity independent of the array position, so the
 *  priority order (and thus the reveal outcome) is deterministic across rebuilds
 *  whose candidate membership shifts. Global shot / station indices and section
 *  ids are viewport-independent. */
function stableId(desc: LabelDescriptor): number {
  if (desc.shotGlobal !== undefined) return desc.shotGlobal;
  if (desc.stationGlobal !== undefined) return desc.stationGlobal;
  return desc.sectionId ?? 0;
}

interface Candidate {
  index: number;
  desc: LabelDescriptor;
  extent: ExtentEm;
  minzoom: number;
  admitZoom: number;
  /** Sort keys: tier (coarser first), kind priority, ordinal, stable id. */
  tier: number;
  priority: number;
  ordinal: number;
  id: number;
}

/** The screen AABB a label would occupy at a given world-pixel scale: its
 *  anchor-relative mercator projected to px (× `wps`), plus the em extent scaled
 *  by the kind's CSS-px text size, plus the halo. em +y is UP, screen +y is DOWN,
 *  so the vertical extent flips. Only RELATIVE positions matter for overlap, so
 *  the shared anchor→screen translation is dropped (anchor px = relMercator × wps). */
export function revealScreenBox(
  desc: LabelDescriptor,
  extent: ExtentEm,
  wps: number,
  sizePx: number,
  haloPx: number,
): ScreenBox {
  const ax = desc.anchorX * wps;
  const ay = desc.anchorY * wps;
  return {
    minX: ax + extent.minX * sizePx - haloPx,
    maxX: ax + extent.maxX * sizePx + haloPx,
    minY: ay - extent.maxY * sizePx - haloPx,
    maxY: ay - extent.minY * sizePx + haloPx,
  };
}

/** Strict-overlap test: boxes touching exactly at an edge do NOT collide. */
function overlaps(a: ScreenBox, b: ScreenBox): boolean {
  return a.minX < b.maxX && a.maxX > b.minX && a.minY < b.maxY && a.maxY > b.minY;
}

/** Whether a label is a collision CANDIDATE under the current toggles - the CPU
 *  mirror of the shader's non-reveal collapse conditions. A non-candidate does
 *  not occupy collision space (a toggled-off / reference-hidden kind must not
 *  block others), but is still emitted at its decimation floor (below). */
function isCandidate(desc: LabelDescriptor, input: RevealInput): boolean {
  if (!input.kindVisible[desc.kind]) return false;
  // Per-kind, mirroring the shader: a hidden kind's own labels stop being
  // candidates, while an ALL_REFERENCE station/section label needs both kinds
  // hidden before its geometry is actually gone.
  if (input.hideReference && (desc.flags & LABEL_FLAG.REFERENCE) !== 0) return false;
  if (input.hideJump && (desc.flags & LABEL_FLAG.JUMP) !== 0) return false;
  // A station/section label whose every leg is virtual stops being a candidate
  // when every kind touching it is hidden - per-kind, mirroring the shader and
  // the circle. A station can be touched by BOTH kinds, hence a bit pair.
  const allJump = (desc.flags & LABEL_FLAG.ALL_JUMP) !== 0;
  const allRef = (desc.flags & LABEL_FLAG.ALL_REFERENCE) !== 0;
  if (
    (allJump || allRef) &&
    !(allJump && !input.hideJump) &&
    !(allRef && !input.hideReference)
  ) {
    return false;
  }
  // Comment tool active (C1): comment kinds are overlap-exempt. Treated as
  // non-candidates so they neither get culled nor occupy collision space - the
  // same uncontested-emit path a toggled-off kind takes, giving `reveal =
  // admitZoom` (their minzoom). Non-comment kinds fall through unchanged.
  if (
    input.commentOverlapExempt === true &&
    (desc.kind === LABEL_KIND.STATION_COMMENT || desc.kind === LABEL_KIND.SHOT_COMMENT)
  ) {
    return false;
  }
  return true;
}

/** Grid-bucket key for a cell coordinate pair. */
function cellKey(cx: number, cy: number): string {
  return `${cx},${cy}`;
}

/** Insert a box into every grid cell it touches. */
function insertBox(grid: Map<string, ScreenBox[]>, box: ScreenBox, cellSize: number): void {
  const x0 = Math.floor(box.minX / cellSize);
  const x1 = Math.floor(box.maxX / cellSize);
  const y0 = Math.floor(box.minY / cellSize);
  const y1 = Math.floor(box.maxY / cellSize);
  for (let cx = x0; cx <= x1; cx++) {
    for (let cy = y0; cy <= y1; cy++) {
      const key = cellKey(cx, cy);
      const bucket = grid.get(key);
      if (bucket) bucket.push(box);
      else grid.set(key, [box]);
    }
  }
}

/** Whether a box overlaps any already-inserted box in the cells it touches. */
function fits(grid: Map<string, ScreenBox[]>, box: ScreenBox, cellSize: number): boolean {
  const x0 = Math.floor(box.minX / cellSize);
  const x1 = Math.floor(box.maxX / cellSize);
  const y0 = Math.floor(box.minY / cellSize);
  const y1 = Math.floor(box.maxY / cellSize);
  for (let cx = x0; cx <= x1; cx++) {
    for (let cy = y0; cy <= y1; cy++) {
      const bucket = grid.get(cellKey(cx, cy));
      if (bucket) {
        for (const placed of bucket) if (overlaps(placed, box)) return false;
      }
    }
  }
  return true;
}

/**
 * Compute the per-label reveal zoom for a viewport-scoped build. `extents[i]` is
 * the shaped em extent of `descriptors[i]` (or `null` for an empty /
 * whitespace-only label). Returns a `Float32Array` of length `descriptors.length`
 * where entry `i` is the min live zoom at which label `i` draws, or
 * {@link REVEAL_NEVER} if it never clears within the emit horizon (not emitted).
 * Deterministic: identical inputs always yield identical output.
 */
export function computeRevealZooms(
  descriptors: LabelDescriptor[],
  extents: (ExtentEm | null)[],
  input: RevealInput,
): Float32Array {
  const n = descriptors.length;
  const reveal = new Float32Array(n).fill(REVEAL_NEVER);
  const step = input.step && input.step > 0 ? input.step : REVEAL_LADDER_STEP;
  const slack = input.slack ?? EMIT_SLACK;
  const cellSize = input.cellSize && input.cellSize > 0 ? input.cellSize : DEFAULT_CELL_PX;
  const ladderMax = input.buildZoom + slack;

  const candidates: Candidate[] = [];
  let ladderStart = Infinity;
  for (let i = 0; i < n; i++) {
    const extent = extents[i];
    if (extent === null || extent === undefined) continue; // no ink → never emitted
    const desc = descriptors[i]!;
    const style = LABEL_KIND_STYLE[desc.kind];
    const firstInSection = (desc.flags & LABEL_FLAG.FIRST_IN_SECTION) !== 0;
    const admitZoom = decimationAdmitZoom(desc.kind, desc.ordinal, firstInSection, style.minzoom);
    if (!isCandidate(desc, input)) {
      // Toggled-off / reference-hidden: emitted UNCONTESTED at its decimation
      // floor (mirrors the old "emitted, no collide-hide"), so a later toggle-on
      // reveals it without a rebuild. Dropped only if past the emit horizon.
      reveal[i] = admitZoom <= ladderMax + 1e-9 ? admitZoom : REVEAL_NEVER;
      continue;
    }
    candidates.push({
      index: i,
      desc,
      extent,
      minzoom: style.minzoom,
      admitZoom,
      tier: decimationTier(desc.ordinal),
      priority: labelPriority(desc.kind, firstInSection),
      ordinal: desc.ordinal,
      id: stableId(desc),
    });
    if (style.minzoom < ladderStart) ladderStart = style.minzoom;
  }
  if (candidates.length === 0) return reveal;

  // Zoom-INDEPENDENT priority order: coarser decimation tier first (so a finer
  // tier can never evict it), then kind priority, then ordinal, then stable id,
  // then kind. Both comment kinds have priority 4 and their ids are a global
  // shot index and a global station index, so the two can tie on everything
  // above; without the kind that tie fell through to the array position, i.e.
  // to the order the caller happened to list the labels in. Descending kind puts
  // the shot comment first - the order `buildLabelDescriptors` always emitted
  // them in - so the winner is unchanged, just no longer order-dependent.
  candidates.sort(
    (a, b) =>
      a.tier - b.tier ||
      a.priority - b.priority ||
      a.ordinal - b.ordinal ||
      a.id - b.id ||
      b.desc.kind - a.desc.kind ||
      a.index - b.index,
  );

  const placed: Candidate[] = [];
  for (let z = ladderStart; z <= ladderMax + 1e-9; z += step) {
    const wps = input.worldPixelScale * Math.pow(2, z - input.buildZoom);
    // Per-kind CSS-px size at this ladder zoom (halo is a fixed per-kind width).
    const sizeByKind = new Float64Array(LABEL_KIND.SHOT_COMMENT + 1);
    for (let k = 0; k <= LABEL_KIND.SHOT_COMMENT; k++) {
      sizeByKind[k] = interpolateStops(z, LABEL_KIND_STYLE[k as LabelKind].size);
    }
    const grid = new Map<string, ScreenBox[]>();
    // Re-seed every already-revealed winner at THIS zoom (keeps its slot).
    for (const c of placed) {
      const size = sizeByKind[c.desc.kind]!;
      const halo = LABEL_KIND_STYLE[c.desc.kind].haloWidth;
      insertBox(grid, revealScreenBox(c.desc, c.extent, wps, size, halo), cellSize);
    }
    // Try to reveal more, in the fixed priority order.
    for (const c of candidates) {
      if (reveal[c.index] !== REVEAL_NEVER) continue; // already revealed
      if (z + 1e-9 < c.admitZoom) continue; // decimation not yet admitted
      const size = sizeByKind[c.desc.kind]!;
      const halo = LABEL_KIND_STYLE[c.desc.kind].haloWidth;
      const box = revealScreenBox(c.desc, c.extent, wps, size, halo);
      if (fits(grid, box, cellSize)) {
        reveal[c.index] = z;
        placed.push(c);
        insertBox(grid, box, cellSize);
      }
    }
  }
  return reveal;
}
