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
 * priority - (decimation tier asc, kind priority, ordinal, stable id) - then a
 * ladder of 0.5-zoom steps runs from the coarsest candidate minzoom up to the
 * emit horizon (`buildZoom + slack`). At each step, already-revealed winners are
 * re-seeded into the grid at THAT zoom, then still-unrevealed admitted candidates
 * are placed greedily in priority order; a candidate that first fits at zoom `z`
 * gets `reveal = z`. Seeding a revealed winner every step guarantees a
 * lower-zoom label keeps its slot, so a later finer-tier label can never evict
 * it - the visible set only ever grows.
 *
 * REAL LINE FIRST (virtual rank). A label that belongs only to virtual geometry -
 * a Jump or Reference shot, or a station / section every leg of which is one - is
 * never shown in place of a real-line label. The ladder above runs once per
 * {@link labelVirtualRank} (real 0, jump 1, reference 2), best rank first, and a
 * later pass RESERVES every earlier-rank winner's slot, including the ones that
 * only reveal at a higher zoom: a lower-rank label J may reveal at `z` only if it
 * clears each earlier-rank winner R (reveal `zR`) at every zoom from `max(z, zR)`
 * up, the zooms at which both are drawn - checked level by level, since offset
 * boxes need not clear for good once they clear (see `nearVirtual`). Otherwise J
 * could take a slot at z14 that a real label needs at z18, and since neither may
 * vanish the two would then overlap. So a real
 * label's reveal is exactly what it would be with no virtual labels at all, and a
 * jump label's likewise with no reference labels. The tier / kind / ordinal order
 * still decides WITHIN a rank. Below the label build band only section names are
 * built, so there the station / shot labels around a jump or reference section's
 * name come in as RESERVED slots ({@link RevealInput.reserved}) - otherwise the
 * name would show at z10..13.5 only to lose its slot, and vanish, once they are.
 *
 * ERRORS LABELS (SHOT_ERROR / CLOSURE_ERROR) rank by their SCORE (worst first)
 * within a pass, so where two overlap the worse error is the one shown: those at
 * or past a warning level right after section names, minor ones after station
 * labels. They are not ordinal-decimated (tier 0). An error label that is
 * admitted at a ladder step but does not fit there RESERVES its box for the rest
 * of that step: no later candidate of the same pass may take the space, so at
 * every step a weaker error - or a station / shot label - never takes the room a
 * stronger error is waiting for. Reserved boxes never evict a placed winner, so
 * the visible set stays monotone in zoom.
 *
 * DETERMINISM ACROSS REBUILDS. The priority order uses a STABLE per-label id
 * (global shot / station index, or section id) rather than the array position,
 * so a pan-settle rebuild whose candidate membership shifts still resolves an
 * unchanged neighbourhood identically (padding absorbs the edge effects).
 */
import { LABEL_KIND, LABEL_KIND_COUNT, LABEL_FLAG, type LabelKind } from './labelShaders';
import type { LabelDescriptor } from './labelTessellate';
import type { ExtentEm } from './labelLayout';
import { LABEL_KIND_STYLE, interpolateStops } from './labelStyle';
import { decimationAdmitZoom, decimationTier } from './labelDecimation';
import { SCORE_WARNING } from '../issueScale';

/** Reveal value for a label that never clears within the emit horizon (not
 *  emitted). Finite reveal values are always `<= buildZoom + slack`. */
export const REVEAL_NEVER = Infinity;

/** The reveal UPLOADED for an emitted label that a reveal-only rerun leaves
 *  uncleared: finite (an ordinary attribute float) but past any zoom the map
 *  reaches, so the shader keeps it hidden until the next build drops it. */
export const REVEAL_UNREACHABLE = 1e6;

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

/** Labels a build reserves without drawing ({@link RevealInput.reserved}),
 *  with their shaped em extents (`null` = no ink, reserves nothing). */
export interface RevealReservation {
  descriptors: readonly LabelDescriptor[];
  extents: readonly (ExtentEm | null)[];
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
  /** Labels this build does NOT draw - their kinds are not built at this zoom -
   *  but a build at a higher zoom would. A virtual candidate leaves their slots
   *  free exactly as if they were drawn (from their decimation floor, the
   *  earliest they could reveal), so it never shows here only to lose its slot
   *  once they are built: the section-only build below the label band reserves
   *  the station / shot labels around a jump or reference section's name. They
   *  get no reveal of their own and block only labels of a worse rank. */
  reserved?: RevealReservation | undefined;
  /** Emit horizon slack in zoom levels (default {@link EMIT_SLACK}). */
  slack?: number;
  /** Ladder step (default {@link REVEAL_LADDER_STEP}). */
  step?: number;
  /** Grid cell size in CSS px (perf only; correctness is cell-size independent).
   *  Defaults to {@link DEFAULT_CELL_PX}. */
  cellSize?: number;
}

/** Collision priority (LOWER places first / wins overlaps), per design §5. Used
 *  as a SECONDARY sort key under the decimation tier, within one
 *  {@link labelVirtualRank} pass - it never lets a virtual label beat a real one.
 *  An Errors label at or past a warning level (`score >= 1`) outranks station
 *  labels; a minor one comes after them, so trivial deltas do not crowd out
 *  station IDs. Errors stay ordered worst first (the split is by score). */
export function labelPriority(kind: LabelKind, firstInSection: boolean, score = 0): number {
  switch (kind) {
    case LABEL_KIND.SECTION_NAME:
      return 0;
    case LABEL_KIND.SHOT_ERROR:
    case LABEL_KIND.CLOSURE_ERROR:
      return score >= SCORE_WARNING ? 1 : 4;
    case LABEL_KIND.STATION_LABEL:
      return firstInSection ? 2 : 3;
    case LABEL_KIND.SHOT_LABEL:
      return 5;
    default:
      return 6; // station + shot comments
  }
}

/** Whether a kind is an Errors label (ranked by score, reserves its box). */
export function isErrorKind(kind: LabelKind): boolean {
  return kind === LABEL_KIND.SHOT_ERROR || kind === LABEL_KIND.CLOSURE_ERROR;
}

/** Real-line labels: a Normal shot's own labels, or a station / section that at
 *  least one Normal shot touches. */
export const VIRTUAL_RANK_REAL = 0;
/** Labels of a Jump (swum, no line laid), or of a station / section touched only
 *  by virtual shots, at least one of them a Jump. */
export const VIRTUAL_RANK_JUMP = 1;
/** Labels of a Reference (tape tie, not swimmable), or of a station / section
 *  touched only by References. */
export const VIRTUAL_RANK_REFERENCE = 2;
const VIRTUAL_RANK_COUNT = 3;

/**
 * Which declutter pass a label belongs to: real-line labels place first, then
 * jump labels, then reference labels, each pass unable to take a slot an earlier
 * one holds (module header, REAL LINE FIRST).
 *
 * Shot labels and shot comments carry their own shot's kind (`JUMP` /
 * `REFERENCE`). Station labels, station comments and section names carry the
 * all-virtual bit pair (`ALL_JUMP` / `ALL_REFERENCE`), set only when no Normal
 * shot touches them; such a label takes the better of the kinds that touch it,
 * so a station on both a jump and a reference ranks as a jump (as does a station
 * with no legs, which carries both bits). A pure function of the flags - never
 * of the hide toggles - so a toggle can only add or remove candidates, never
 * reorder the ones the last build emitted.
 */
export function labelVirtualRank(desc: LabelDescriptor): number {
  const f = desc.flags;
  if ((f & (LABEL_FLAG.JUMP | LABEL_FLAG.ALL_JUMP)) !== 0) return VIRTUAL_RANK_JUMP;
  if ((f & (LABEL_FLAG.REFERENCE | LABEL_FLAG.ALL_REFERENCE)) !== 0) return VIRTUAL_RANK_REFERENCE;
  return VIRTUAL_RANK_REAL;
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
  /** Which pass places it ({@link labelVirtualRank}). */
  rank: number;
  /** Sort keys within a pass: tier (coarser first), kind priority, score rank
   *  (an Errors label's negated score, so the worst sorts first; 0 otherwise),
   *  ordinal, stable id. */
  tier: number;
  priority: number;
  scoreRank: number;
  ordinal: number;
  id: number;
  /** An Errors label: reserves its box at a step where it does not fit. */
  reserves: boolean;
  /** Ladder level it revealed at (set when placed; -1 while unrevealed). */
  level: number;
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

/** Cell coordinates below this magnitude pack into one exact integer key. */
const CELL_KEY_RANGE = 2 ** 24;

/** Grid-bucket key for a cell coordinate pair: one integer while both fit the
 *  packed range (the norm - cells are anchor-relative), else a string, which
 *  can never equal a number key. A number key skips building a string per cell
 *  visit, the hottest step of the reveal pass. */
function cellKey(cx: number, cy: number): number | string {
  if (cx > -CELL_KEY_RANGE && cx < CELL_KEY_RANGE && cy > -CELL_KEY_RANGE && cy < CELL_KEY_RANGE) {
    return (cx + CELL_KEY_RANGE) * (2 * CELL_KEY_RANGE) + (cy + CELL_KEY_RANGE);
  }
  return `${cx},${cy}`;
}

/** A spatial hash of screen boxes, bucketed by {@link cellKey}. */
type BoxGrid = Map<number | string, ScreenBox[]>;

/** Insert a box into every grid cell it touches. */
function insertBox(grid: BoxGrid, box: ScreenBox, cellSize: number): void {
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
function fits(grid: BoxGrid, box: ScreenBox, cellSize: number): boolean {
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
    const errorKind = isErrorKind(desc.kind);
    candidates.push({
      index: i,
      desc,
      extent,
      minzoom: style.minzoom,
      admitZoom,
      rank: labelVirtualRank(desc),
      tier: errorKind ? 0 : decimationTier(desc.ordinal),
      priority: labelPriority(desc.kind, firstInSection, desc.score),
      scoreRank: errorKind ? -(desc.score ?? 0) : 0,
      ordinal: desc.ordinal,
      id: stableId(desc),
      reserves: errorKind,
      level: -1,
    });
    if (style.minzoom < ladderStart) ladderStart = style.minzoom;
  }
  if (candidates.length === 0) return reveal;

  // Zoom-INDEPENDENT priority order: coarser decimation tier first (so a finer
  // tier can never evict it), then kind priority, then score rank (worst error
  // first), then ordinal, then stable id.
  candidates.sort(
    (a, b) =>
      a.tier - b.tier ||
      a.priority - b.priority ||
      a.scoreRank - b.scoreRank ||
      a.ordinal - b.ordinal ||
      a.id - b.id ||
      a.index - b.index,
  );
  // One pass per virtual rank, best first; the sort above carries into each.
  const passes: Candidate[][] = Array.from({ length: VIRTUAL_RANK_COUNT }, () => []);
  for (const c of candidates) passes[c.rank]!.push(c);

  // Ladder levels are integer steps from ONE start shared by every pass, so a
  // level means the same zoom in each (reservations are keyed by level).
  const levelCount = Math.floor((ladderMax - ladderStart) / step + 1e-9) + 1;
  if (levelCount <= 0) return reveal; // horizon below every minzoom: nothing clears
  const levels: LadderLevel[] = [];
  for (let l = 0; l < levelCount; l++) {
    const z = ladderStart + l * step;
    // Per-kind CSS-px size at this ladder zoom (halo is a fixed per-kind width).
    const sizeByKind = new Float64Array(LABEL_KIND_COUNT);
    for (let k = 0; k < LABEL_KIND_COUNT; k++) {
      sizeByKind[k] = interpolateStops(z, LABEL_KIND_STYLE[k as LabelKind].size);
    }
    levels.push({ z, wps: input.worldPixelScale * Math.pow(2, z - input.buildZoom), sizeByKind });
  }
  /** The first ladder level at or above `zoom`. */
  const levelAt = (zoom: number): number => Math.max(0, Math.ceil((zoom - ladderStart) / step - 1e-9));
  const boxAt = (c: Candidate, lv: LadderLevel): ScreenBox =>
    revealScreenBox(c.desc, c.extent, lv.wps, lv.sizeByKind[c.desc.kind]!, LABEL_KIND_STYLE[c.desc.kind].haloWidth);

  const place = (c: Candidate, l: number): void => {
    reveal[c.index] = levels[l]!.z;
    c.level = l;
  };

  // Real-line pass: the plain ladder, exactly as if no virtual label existed. A
  // fresh grid per level, re-seeded with the winners so far at THAT zoom (keeps
  // their slots) - cheaper than holding every level's grid at once, which only
  // the virtual passes need.
  const realWinners: Candidate[] = [];
  const real = passes[VIRTUAL_RANK_REAL]!;
  for (let l = 0; l < levelCount && real.length > 0; l++) {
    const lv = levels[l]!;
    const grid: BoxGrid = new Map();
    for (const c of realWinners) insertBox(grid, boxAt(c, lv), cellSize);
    // Boxes of Errors labels that did not fit at this step (module header).
    const waiting: BoxGrid = new Map();
    // Try to reveal more, in the fixed priority order.
    for (const c of real) {
      if (c.level >= 0) continue; // already revealed
      if (lv.z + 1e-9 < c.admitZoom) continue; // decimation not yet admitted
      const box = boxAt(c, lv);
      if (fits(grid, box, cellSize) && (waiting.size === 0 || fits(waiting, box, cellSize))) {
        place(c, l);
        realWinners.push(c);
        insertBox(grid, box, cellSize);
      } else if (c.reserves) {
        insertBox(waiting, box, cellSize);
      }
    }
  }

  // Virtual passes (jump, then reference) share one grid per level holding every
  // label drawn there, boxed at that level's zoom: a winner goes into its own
  // level and every one above it. A virtual candidate thus sees the slot a
  // better-rank label takes further up the ladder, not just the ones drawn at
  // the level it is trying. Reserved labels ({@link RevealInput.reserved}) get
  // grids of their own, so a candidate can tell a slot that is certainly taken
  // from one that only might be.
  const virtual = passes.slice(VIRTUAL_RANK_REAL + 1).flat();
  if (virtual.length === 0) return reveal;
  const phantoms = reservedPhantoms(input, levelAt, levelCount);
  const reserving = phantoms.some((p) => p.length > 0);
  const grids = levels.map((): BoxGrid => new Map());
  const reservedGrids = levels.map((): BoxGrid => new Map());
  const occupy = (into: BoxGrid[], c: Candidate): void => {
    for (let m = c.level; m < levelCount; m++) insertBox(into[m]!, boxAt(c, levels[m]!), cellSize);
  };
  for (const w of nearVirtual(realWinners, virtual, levelAt, levels, boxAt, cellSize)) occupy(grids, w);
  for (const w of phantoms[VIRTUAL_RANK_REAL]!) occupy(reservedGrids, w);
  for (let rank = VIRTUAL_RANK_REAL + 1; rank < VIRTUAL_RANK_COUNT; rank++) {
    // Reserved better-rank labels (a jump-rank one outranks a reference label)
    // join just before the pass they outrank.
    if (rank > VIRTUAL_RANK_JUMP) for (const w of phantoms[rank - 1]!) occupy(reservedGrids, w);
    const pass = passes[rank]!;
    // The first level each candidate may reveal at (-1 until computed, at its
    // first admitted level).
    const earliest = new Int32Array(pass.length).fill(-1);
    for (let l = 0; l < levelCount && pass.length > 0; l++) {
      const lv = levels[l]!;
      // Errors labels of this pass that did not fit at this level (module header).
      const waiting: BoxGrid = new Map();
      for (let j = 0; j < pass.length; j++) {
        const c = pass[j]!;
        if (c.level >= 0) continue; // already revealed
        if (lv.z + 1e-9 < c.admitZoom) continue; // decimation not yet admitted
        if (earliest[j]! < 0) {
          const placedClear = firstClearLevel(c, l, grids, levels, boxAt, cellSize);
          const reservedClear = reserving ? firstClearLevel(c, l, reservedGrids, levels, boxAt, cellSize) : l;
          if (reservedClear > placedClear) {
            // Only a reserved label holds it back - one a build that draws it
            // may never show, so this label's own fate there is open. Hide it
            // here but keep its slot, as a reservation itself (a label it holds
            // back is just as open), so the labels it would block there are
            // not shown here only to vanish.
            occupy(reservedGrids, { ...c, level: l });
            earliest[j] = levelCount;
          } else {
            earliest[j] = placedClear;
          }
        }
        if (l < earliest[j]!) continue; // would overlap a better-rank label once both show
        const box = boxAt(c, lv);
        if (
          fits(grids[l]!, box, cellSize) &&
          (!reserving || fits(reservedGrids[l]!, box, cellSize)) &&
          (waiting.size === 0 || fits(waiting, box, cellSize))
        ) {
          place(c, l);
          occupy(grids, c);
        } else if (c.reserves) {
          insertBox(waiting, box, cellSize);
        }
      }
    }
  }
  return reveal;
}

/**
 * The reserved labels ({@link RevealInput.reserved}) as already-placed
 * candidates, bucketed by rank: each holds its slot from the level of its
 * decimation floor - the earliest it could reveal in a build that draws it, so
 * leaving its slot free from there is never too little. Ones the toggles hide,
 * with no ink, or past the horizon reserve nothing.
 */
function reservedPhantoms(input: RevealInput, levelAt: (zoom: number) => number, levelCount: number): Candidate[][] {
  const out: Candidate[][] = Array.from({ length: VIRTUAL_RANK_COUNT }, () => []);
  const reserved = input.reserved;
  if (!reserved) return out;
  for (let i = 0; i < reserved.descriptors.length; i++) {
    const extent = reserved.extents[i];
    const desc = reserved.descriptors[i]!;
    if (extent === null || extent === undefined || !isCandidate(desc, input)) continue;
    const style = LABEL_KIND_STYLE[desc.kind];
    const firstInSection = (desc.flags & LABEL_FLAG.FIRST_IN_SECTION) !== 0;
    const admitZoom = decimationAdmitZoom(desc.kind, desc.ordinal, firstInSection, style.minzoom);
    const level = levelAt(admitZoom);
    if (level >= levelCount) continue;
    const rank = labelVirtualRank(desc);
    out[rank]!.push({
      index: -1,
      desc,
      extent,
      minzoom: style.minzoom,
      admitZoom,
      rank,
      tier: 0,
      priority: 0,
      scoreRank: 0,
      ordinal: desc.ordinal,
      id: stableId(desc),
      reserves: false,
      level,
    });
  }
  return out;
}

/**
 * The placed labels that could ever overlap a virtual candidate - a filter that
 * keeps the shared grids small when, as usual, the virtual labels are few.
 *
 * Overlap does NOT simply shrink with zoom: every kind's box is offset from its
 * anchor (`labelLayout.ts`), so two boxes facing each other can clear, overlap
 * further up, and clear again. What does hold is that each box only moves
 * TOWARDS its anchor, in map units, as the zoom rises - the text grows far
 * slower than the map (`labelStyle.ts` stops) and the halo not at all. So a
 * label's boxes at every level from `L` up stay inside its HULL at `L`: the
 * box there stretched to cover its anchor. Labels whose hulls are apart at the
 * lowest level a virtual candidate is admitted at - none of them can be drawn
 * below it - never overlap. When the virtual labels outnumber the placed ones
 * the test costs more than it saves, so every placed label is kept.
 */
function nearVirtual(
  placed: readonly Candidate[],
  virtual: readonly Candidate[],
  levelAt: (zoom: number) => number,
  levels: readonly LadderLevel[],
  boxAt: (c: Candidate, lv: LadderLevel) => ScreenBox,
  cellSize: number,
): readonly Candidate[] {
  if (virtual.length >= placed.length) return placed;
  let lowest = levels.length - 1;
  for (const c of virtual) lowest = Math.min(lowest, levelAt(c.admitZoom));
  const at = levels[lowest]!;
  const hull = (c: Candidate): ScreenBox => anchorHull(c.desc, boxAt(c, at), at.wps);
  const grid: BoxGrid = new Map();
  for (const c of virtual) insertBox(grid, hull(c), cellSize);
  return placed.filter((w) => !fits(grid, hull(w), cellSize));
}

/** `box` stretched to cover its label's anchor (plus the kind's halo) - the
 *  region every box of that label at this zoom or above stays inside. */
export function anchorHull(desc: LabelDescriptor, box: ScreenBox, wps: number): ScreenBox {
  const ax = desc.anchorX * wps;
  const ay = desc.anchorY * wps;
  const h = LABEL_KIND_STYLE[desc.kind].haloWidth;
  return {
    minX: Math.min(box.minX, ax - h),
    maxX: Math.max(box.maxX, ax + h),
    minY: Math.min(box.minY, ay - h),
    maxY: Math.max(box.maxY, ay + h),
  };
}

/** One rung of the zoom ladder, with its world-pixel scale and per-kind sizes. */
interface LadderLevel {
  z: number;
  wps: number;
  sizeByKind: Float64Array;
}

/**
 * The first level at or above `from` at which `c` may reveal without ever
 * overlapping a label in `grids`: one above the highest level where its box hits
 * one (`levels.length` if that is the top: it never may). Every level above the
 * returned one is checked, so this does not lean on overlap shrinking with zoom
 * - which it need not (see {@link nearVirtual}). Below that level `c` would
 * overlap that label once both are drawn, even a better-rank one that only
 * reveals further up. A same-pass winner placed after this runs is only checked
 * against at the level being tried (the per-level fit check) - the same
 * one-level test the real pass applies between two real labels, which offset
 * boxes can outgrow further up.
 */
function firstClearLevel(
  c: Candidate,
  from: number,
  grids: readonly BoxGrid[],
  levels: readonly LadderLevel[],
  boxAt: (c: Candidate, lv: LadderLevel) => ScreenBox,
  cellSize: number,
): number {
  for (let m = grids.length - 1; m >= from; m--) {
    if (!fits(grids[m]!, boxAt(c, levels[m]!), cellSize)) return m + 1;
  }
  return from;
}
