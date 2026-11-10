/**
 * Incremental-edit diff for the custom-render buffers (#customRender).
 *
 * A geometry flush after an edit normally full-rebuilds every vertex buffer
 * (~231 ms on 92.stalx) even when the edit moved a single station. When the SoA
 * *layout* is unchanged (same section sequence + per-section shot/station
 * counts, same station→section mapping) only a handful of sections' positions /
 * colors actually changed, so the controller can re-emit just those sections'
 * vertex ranges and `gl.bufferSubData` them (single-digit ms).
 *
 * This module is the PURE decision half: given the previous and next
 * `SurveyData` + color context, it either returns a plan naming the changed
 * sections (and the moved stations to reproject) or `null` to force a full
 * rebuild. It performs no mutation and no GL, so it is unit-testable in `node`.
 *
 * Why a direct SoA diff rather than reusing the vector-tile `diffVtLayerBounds`
 * path: the custom layer syncs synchronously from `SurveyData` at the top of the
 * flush, before the async feature worker has produced any `VtLayerData` to diff.
 * `sameSurveyLayout` mirrors `vectorTileData.isStructuralChange`'s intent
 * (identity + count stability ⇒ non-structural) but over the SoA the custom
 * layer already holds, so no second representation is needed.
 */
import type { GpsOrigin } from '../../../../types/config';
import type { SurveyData } from '../../../../types/survey-data';
import type { DisplayColorContext, OverlayColorInput } from '../displayColor';
import { computeMercatorAnchor } from './mercator';
import { sameColorScaleInput } from '../../../../utils/palettes';

/** What the incremental apply step must recompute. Section indices are into
 *  `next.sections` (declaration order, stable under a non-structural edit). */
export interface IncrementalPlan {
  /** Sections whose line range + stations + all-reference must be re-emitted. */
  dirtySectionIdxs: number[];
  /** Global station indices whose x/y moved (reproject their anchor-relative
   *  position before re-emitting). A subset of the dirty sections' stations. */
  reprojectStationGlobals: number[];
}

/** Two gps origins are equivalent for projection (same value, or both null). */
export function sameGpsOrigin(a: GpsOrigin | null, b: GpsOrigin | null): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  return a.latitude === b.latitude && a.longitude === b.longitude;
}

/**
 * Whether the SoA layout is byte-stable between two surveys: same section
 * sequence (id, shotOffset, shotCount) and same station→section/index mapping
 * and count. When true, every drawn vertex keeps its slot, so a per-section
 * re-emit lands in the exact range the full build wrote. A false here forces a
 * full rebuild (insert / delete / merge / split / reorder, and their undos).
 */
export function sameSurveyLayout(prev: SurveyData, next: SurveyData): boolean {
  if (prev.stationCount !== next.stationCount) return false;
  if (prev.shotCount !== next.shotCount) return false;
  if (prev.sections.length !== next.sections.length) return false;
  for (let i = 0; i < next.sections.length; i++) {
    const a = prev.sections[i]!;
    const b = next.sections[i]!;
    if (a.id !== b.id || a.shotOffset !== b.shotOffset || a.shotCount !== b.shotCount) return false;
  }
  // shotIds must be slot-stable too: the incremental path re-emits a dirty
  // section's positions/colors but NOT its dim/hide/highlight buffers, which the
  // immovable-feature predicates key on `shotIds` (controller.ts dimPredicates).
  // A shotId that changed slot under an otherwise-stable layout would leave a
  // stale dim/highlight on that vertex, so force a full rebuild instead.
  const pShotIds = prev.shotIds;
  const nShotIds = next.shotIds;
  for (let gi = 0; gi < next.shotCount; gi++) {
    if (pShotIds[gi] !== nShotIds[gi]) return false;
  }
  const pS = prev.stationSectionIds;
  const nS = next.stationSectionIds;
  const pI = prev.stationIndices;
  const nI = next.stationIndices;
  for (let g = 0; g < next.stationCount; g++) {
    if (pS[g] !== nS[g] || pI[g] !== nI[g]) return false;
  }
  return true;
}

/**
 * Whether the GLOBAL color inputs are unchanged, so color differences are
 * confined to the sections whose geometry / section-color changed. A change to
 * any of these can recolor every section, which the incremental path cannot
 * bound cheaply - the caller full-rebuilds instead. In issue-severity mode the
 * per-flush `issueSeverity` arrays are freshly computed (new identity) and a
 * move can shift severity non-locally, so severity mode always falls back here.
 */
export function sameColorGlobals(prev: DisplayColorContext, next: DisplayColorContext): boolean {
  if (prev.themeColors !== next.themeColors) return false;
  if (prev.showDepthGradient !== next.showDepthGradient) return false;
  if (prev.showIssueSeverity !== next.showIssueSeverity) return false;
  if (prev.depthRange.min !== next.depthRange.min || prev.depthRange.max !== next.depthRange.max) {
    return false;
  }
  // The user's depth palette / Levels range recolours every depth-ramped feature.
  if (!sameColorScaleInput(prev.depthScale, next.depthScale)) return false;
  // Severity mode: any edit rebuilds the severity arrays (new ref) and severity
  // is non-local, so require identical (⇒ only fires when severity is off/stable).
  if (next.showIssueSeverity && prev.issueSeverity !== next.issueSeverity) return false;
  // An analytical overlay recolors every feature; enter / exit and every
  // recompute (a fresh value-map ref) forces a full rebuild. A geometry flush
  // that leaves the overlay untouched keeps the same map refs ⇒ still same.
  if (!sameOverlay(prev.overlay ?? null, next.overlay ?? null)) return false;
  // Move-comment tint (#52) reads per-feature comment ownership the incremental
  // diff doesn't track (adding / moving / removing a comment recolors a feature
  // without any coord / flag / section-color change). Toggling the tool and every
  // flush while it is active force a full rebuild; the tool is not a hot-edit path
  // (full rebuild is ~1 ms on 4naharon), so the conservative gate is cheap.
  if ((prev.commentTint === true) || (next.commentTint === true)) return false;
  // Plan-mode idle tint (#395 revision): entering/leaving it recolors every
  // feature the same way an overlay enter/exit does, so a flip forces a recompute.
  if ((prev.planIdleTint === true) !== (next.planIdleTint === true)) return false;
  return true;
}

/** Two overlay inputs are color-equivalent iff both off, or same mode + same
 *  value-map identities + same scale. Map identity (not deep-equality) is the
 *  right test: SurveyLayer replaces the maps wholesale on every overlay
 *  recompute, so an identity change ⇔ a value change the incremental path can't
 *  bound locally. */
function sameOverlay(a: OverlayColorInput | null, b: OverlayColorInput | null): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  return (
    a.mode === b.mode &&
    a.shots === b.shots &&
    a.stations === b.stations &&
    a.max === b.max &&
    sameColorScaleInput(a.scale, b.scale)
  );
}

/**
 * Plan an incremental update, or return `null` to force a full rebuild. Pure:
 * compares the two surveys' per-station and per-section inputs and collects the
 * changed sections. Falls back (returns null) on a structural change, a global
 * color-input change, an anchor shift (all positions move), a gps-origin change,
 * a build-time exclusion (buffers would be compacted, breaking the g↦[4g] map),
 * or when so many sections changed that a full rebuild is cheaper.
 */
export function planIncrementalUpdate(
  prev: SurveyData,
  next: SurveyData,
  prevCtx: DisplayColorContext,
  nextCtx: DisplayColorContext,
  prevGpsOrigin: GpsOrigin | null,
  nextGpsOrigin: GpsOrigin | null,
  cachedAnchorX: number,
  cachedAnchorY: number,
): IncrementalPlan | null {
  if (!sameGpsOrigin(prevGpsOrigin, nextGpsOrigin)) return null;
  if (!sameSurveyLayout(prev, next)) return null;
  if (!sameColorGlobals(prevCtx, nextCtx)) return null;
  // Anchor shift ⇒ every anchor-relative position changes ⇒ full rebuild. Exact
  // equality is required (and holds when the edit does not move the bbox extent).
  const [ax, ay] = computeMercatorAnchor(next, nextGpsOrigin);
  if (ax !== cachedAnchorX || ay !== cachedAnchorY) return null;

  const sectionIdxById = new Map<number, number>();
  for (let i = 0; i < next.sections.length; i++) sectionIdxById.set(next.sections[i]!.id, i);

  const dirty = new Set<number>();
  const reproject: number[] = [];

  // Pass 1: moved / flag-changed stations. x/y move ⇒ reproject; any coord or
  // flag change ⇒ the station's section is dirty (position, depth-fill, size).
  const pC = prev.stationCoords;
  const nC = next.stationCoords;
  for (let g = 0; g < next.stationCount; g++) {
    const b = g * 3;
    const xyChanged = pC[b] !== nC[b] || pC[b + 1] !== nC[b + 1];
    const zChanged = pC[b + 2] !== nC[b + 2];
    const flagChanged = prev.stationFlags[g] !== next.stationFlags[g];
    if (xyChanged) reproject.push(g);
    if (xyChanged || zChanged || flagChanged) dirty.add(sectionIdxById.get(next.stationSectionIds[g]!)!);
  }

  // Pass 2: section-color and per-shot color/flag changes (depth-fill uses shot
  // depths; is_reference drives the reference + all-reference flags).
  const depthActive = nextCtx.showDepthGradient && !nextCtx.showIssueSeverity;
  for (let si = 0; si < next.sections.length; si++) {
    if (dirty.has(si)) continue;
    const section = next.sections[si]!;
    if (prev.sections[si]!.color !== section.color) {
      dirty.add(si);
      continue;
    }
    const off = section.shotOffset;
    const cnt = section.shotCount;
    for (let i = 0; i < cnt; i++) {
      const gi = off + i;
      if (prev.shotFlags[gi] !== next.shotFlags[gi]) {
        dirty.add(si);
        break;
      }
      if (
        depthActive &&
        (prev.shotDepthIn[gi] !== next.shotDepthIn[gi] || prev.shotDepthOut[gi] !== next.shotDepthOut[gi])
      ) {
        dirty.add(si);
        break;
      }
    }
  }

  // If most sections changed, a full rebuild is cheaper than many sub-uploads.
  if (dirty.size > next.sections.length / 2) return null;

  return { dirtySectionIdxs: [...dirty], reprojectStationGlobals: reproject };
}

/**
 * Whether an edit is a COLOR-ONLY global recolor: the color globals changed
 * (analytical-overlay enter/exit or recompute, theme, depth / severity / depth-
 * range / move-comment-tint toggle) while every geometry input the color-only
 * fast path does NOT recompute is byte-stable. When true the controller
 * recomputes only the color arrays via `makeColorResolvers(next, nextCtx)` and
 * re-uploads the two color buffers, skipping reprojection, tessellation, and the
 * position / reference / all-reference / size buffers entirely (~low ms vs the
 * full-rebuild reproject + tessellate).
 *
 * Returns false (⇒ the caller full-rebuilds, or the geometry-incremental path
 * ran first) when the color globals are UNCHANGED (nothing to recolor globally -
 * a geometry edit or no-op), on a gps / layout / anchor change, or when any
 * non-color geometry input moved too (a combined edit the color-only path cannot
 * bound): station x/y (⇒ positions), station flags (⇒ zero-datum `sizeMul`), or
 * shot flags (⇒ per-vertex `reference` + per-station `allReference`). Section
 * colors, depths, and z are pure color inputs and MAY change - the resolvers
 * read them from `next`, so the recolor stays correct.
 */
export function planColorOnlyUpdate(
  prev: SurveyData,
  next: SurveyData,
  prevCtx: DisplayColorContext,
  nextCtx: DisplayColorContext,
  prevGpsOrigin: GpsOrigin | null,
  nextGpsOrigin: GpsOrigin | null,
  cachedAnchorX: number,
  cachedAnchorY: number,
): boolean {
  if (sameColorGlobals(prevCtx, nextCtx)) return false;
  if (!sameGpsOrigin(prevGpsOrigin, nextGpsOrigin)) return false;
  if (!sameSurveyLayout(prev, next)) return false;
  const [ax, ay] = computeMercatorAnchor(next, nextGpsOrigin);
  if (ax !== cachedAnchorX || ay !== cachedAnchorY) return false;
  const pC = prev.stationCoords;
  const nC = next.stationCoords;
  for (let g = 0; g < next.stationCount; g++) {
    const b = g * 3;
    // x/y feed posA/posB (not reprojected here); station flags feed sizeMul.
    if (pC[b] !== nC[b] || pC[b + 1] !== nC[b + 1]) return false;
    if (prev.stationFlags[g] !== next.stationFlags[g]) return false;
  }
  // Shot flags feed the reference + all-reference buffers (not recomputed here).
  for (let gi = 0; gi < next.shotCount; gi++) {
    if (prev.shotFlags[gi] !== next.shotFlags[gi]) return false;
  }
  return true;
}

/**
 * Coalesce a sorted list of vertex-slot starts (each `count` verts wide) into
 * `[startVertex, vertexCount]` runs, so scattered per-station updates collapse
 * into as few `bufferSubData` calls as the layout allows. `slots` must be sorted
 * ascending and unique.
 */
export function coalesceVertexRuns(slots: number[], vertsPerSlot: number): Array<[number, number]> {
  const runs: Array<[number, number]> = [];
  let i = 0;
  while (i < slots.length) {
    const start = slots[i]!;
    let end = start;
    while (i + 1 < slots.length && slots[i + 1] === end + 1) {
      end = slots[++i]!;
    }
    runs.push([start * vertsPerSlot, (end - start + 1) * vertsPerSlot]);
    i++;
  }
  return runs;
}
