/**
 * SurveyData -> label descriptors -> per-glyph WebGL vertex buffers
 * (#customRender labels, Phase 1).
 *
 * Pure (no GL, no DOM beyond the injected {@link GlyphAtlas}), so the whole
 * label-buffer story is unit-testable in the `node` env - the GL layer (Phase 2)
 * only uploads what {@link buildLabelBuffers} returns. Sibling of `tessellate.ts`
 * (lines/circles): the same anchor-relative-mercator float32 positions +
 * per-glyph-quad-as-4-verts + `UNSIGNED_INT` indices, and the same single-writer
 * discipline ({@link emitLabel} is the one function that stamps a label's glyph
 * quads, mirroring `emitLineSection` / `emitStation`).
 *
 * TWO STAGES:
 *  1. {@link buildLabelDescriptors} walks the SoA once and emits a
 *     {@link LabelDescriptor} per label (5 kinds), computing the anchor
 *     (anchor-relative mercator, exactly the projection `features.ts` uses), the
 *     text + color (via the shared `labelText.ts` helpers -> parity), the
 *     decimation ordinal, and the static flag bits (reference / all-reference /
 *     first-in-section). Interaction flags (DIM / HIDE) default 0 here and are a
 *     later phase's concern.
 *  2. {@link buildLabelBuffers} shapes each descriptor's text (`labelLayout.ts`)
 *     and stamps its glyph quads into one set of typed arrays.
 *
 * Viewport scoping + collision (which labels to include) are a later phase; this
 * module builds whatever descriptor set it is given, so a Phase-4 viewport filter
 * simply pre-filters the descriptor list.
 */
import type { GpsOrigin, DecimalPrecision } from '../../../../types/config';
import { type SurveyData, shotKindFromFlags } from '../../../../types/survey-data';
import { metersToMapCoords } from '../../../../utils/coordinates';
import { lngLatToMercator } from '../mercatorCoords';
import { packCssColor, type Rgba8 } from './colorPack';
import { buildStationLookup, computeStationAllReference, projectStationsRel } from './tessellate';
import { LABEL_KIND, LABEL_FLAG, type LabelKind } from './labelShaders';
import { shapeLabel, type ShapedLabel, type ShapeCache } from './labelLayout';
import type { GlyphAtlas } from './glyphAtlas';

/**
 * Padded viewport rectangle in ANCHOR-RELATIVE mercator (`mercator - anchor`),
 * the same space descriptor anchors live in (§4 of the design). When passed to
 * {@link buildLabelDescriptors}, a label whose anchor falls outside is not
 * emitted at all - the viewport-scoped build that bounds the live glyph set to
 * what is near the screen. Absent = whole-survey (every anchor kept), preserving
 * the pre-Phase-4 behavior for callers / tests that pass no viewport.
 */
export interface ViewportRel {
  minX: number;
  minY: number;
  maxX: number;
  maxY: number;
}
import {
  formatShotLabel,
  formatStationLabel,
  formatStationNameLabel,
  formatSectionLabel,
  formatCommentLabel,
  resolveShotLabelColor,
  resolveStationLabelColor,
  resolveSectionLabelColor,
  type LabelColorContext,
  type StationLabelText,
} from './labelText';

/**
 * One label to render: kind (indexes every per-kind uniform + `labelLayout`
 * anchor/offset), its text, its anchor in anchor-relative mercator, its packed
 * data color, its decimation ordinal, and its static {@link LABEL_FLAG} bits.
 */
export interface LabelDescriptor {
  kind: LabelKind;
  text: string;
  /** Anchor in anchor-relative mercator (`mercator - anchor`), shared by every
   *  glyph of this label. */
  anchorX: number;
  anchorY: number;
  color: Rgba8;
  /** Decimation key: shot running index (shot labels) / station global ordinal
   *  (station labels); 0 for kinds without decimation. */
  ordinal: number;
  /** STATIC {@link LABEL_FLAG} bits only (FIRST_IN_SECTION / REFERENCE /
   *  ALL_REFERENCE). Interaction bits (DIM / DRAG_HIDE) are NOT stored here -
   *  they are computed per build / per flag-refresh from the controller's
   *  dim/hide state so a cheap flags-only re-upload can flip them without
   *  disturbing the reveal zoom (which the reveal-only path owns). */
  flags: number;
  /** Owning section id - keys anchored-dim + drag-section-hide for every kind. */
  sectionId?: number;
  /** Global station index (station label + station comment) - keys station
   *  drag-hide + (with {@link stationIndex}) immovable-station dim. */
  stationGlobal?: number;
  /** Station index within its section (station label + station comment) - the
   *  immovable feature id is `sectionId * FEATURE_ID_STRIDE + stationIndex`. */
  stationIndex?: number;
  /** Global shot index (shot label + shot comment) - keys shot drag-hide. */
  shotGlobal?: number;
  /** Shot id within its section (shot label + shot comment) - the immovable
   *  feature id is `sectionId * FEATURE_ID_STRIDE + shotId`. */
  shotId?: number;
}

/** Inputs `buildLabelDescriptors` needs beyond the survey geometry: the shared
 *  label color context + the number-format precision. */
export interface LabelBuildContext {
  colorCtx: LabelColorContext;
  precision?: DecimalPrecision | undefined;
  /**
   * Comment-label text color resolver for the `move_comment` palette tint (#52).
   * Given a comment's OWNER key (`${sectionId}:s${shotId}` for shot comments,
   * `${sectionId}:st${stationIndex}` for station comments - the exact keys
   * `features.ts` / `colorResolvers.ts` use), returns the packed tint color.
   * ABSENT (the common case) = comments use the flat `labelColor`, matching the
   * stock `commentTextColor()` when the tool is not active. Injected (never
   * imported here) so this pure module never depends on `features.ts`
   * (`commentColorForKey`) - no import cycle.
   */
  commentColor?: ((ownerKey: string) => Rgba8) | undefined;
  /**
   * What station labels say. ABSENT = `'id'` (the station index). `'name'`
   * labels each station with its name and skips unnamed stations - every other
   * field of the descriptor (ordinal, flags, owner) is the same as for the ID,
   * so names declutter, decimate, dim and hide exactly like the IDs do.
   */
  stationLabelText?: StationLabelText | undefined;
}

/**
 * Per-glyph-vertex label buffers (§3 of the design). One label's glyphs share
 * `anchor` / `color` / `kind` / `revealZoom` / `flags`; `offsetEm` (em, +y up)
 * and `uv` (atlas px) vary per glyph corner. `kind` / `flags` are integer
 * attributes (bind with `vertexAttribIPointer`, UNSIGNED_BYTE); the rest are
 * FLOAT except `color` (UNSIGNED_BYTE, normalized).
 */
export interface LabelBuffers {
  glyphCount: number;
  vertexCount: number;
  /** Label anchor (anchor-relative mercator), `vertexCount*2`. */
  anchor: Float32Array;
  /** Glyph-corner offset from the anchor, in em (+x right, +y up), `vertexCount*2`. */
  offsetEm: Float32Array;
  /** Atlas PIXEL coords (normalized in the shader), `vertexCount*2`. */
  uv: Float32Array;
  /** Per-vertex text color RGBA8, `vertexCount*4`. */
  color: Uint8Array;
  /** Per-vertex label kind (0..4), `vertexCount`. */
  kind: Uint8Array;
  /** Per-vertex reveal zoom - the min live zoom at which the label draws
   *  (`labelReveal.ts`); the shader shows it iff `u_zoom >= revealZoom`. Folds in
   *  both collision declutter and decimation. `vertexCount`. */
  revealZoom: Float32Array;
  /** Per-vertex {@link LABEL_FLAG} bitfield, `vertexCount`. */
  flags: Uint8Array;
  /** Two triangles per glyph, `glyphCount*6`, `UNSIGNED_INT`. */
  indices: Uint32Array;
  /** Total attribute + index bytes (for cost logging). */
  byteLength: number;
}

/** Project a meters (x, y) point to anchor-relative mercator, the exact pass
 *  `features.ts` / `tessellate.ts` use. */
function projectMeters(
  x: number,
  y: number,
  gpsOrigin: GpsOrigin | null,
  anchorX: number,
  anchorY: number,
): [number, number] {
  const ll = metersToMapCoords(x, y, gpsOrigin);
  const [mx, my] = lngLatToMercator(ll[0], ll[1]);
  return [mx - anchorX, my - anchorY];
}

/**
 * Walk the SoA once and emit a {@link LabelDescriptor} for every label the stock
 * layers would build: shot labels (+ shot comments), station labels (+ station
 * comments), and section names. Reproduces `features.ts`' anchor placement,
 * ordinals, all-reference / first-in-section predicates, and (via `labelText.ts`)
 * text + color exactly. `anchorX` / `anchorY` is the survey mercator anchor
 * (`computeMercatorAnchor`); all descriptor anchors are relative to it.
 */
export function buildLabelDescriptors(
  data: SurveyData,
  gpsOrigin: GpsOrigin | null,
  anchorX: number,
  anchorY: number,
  ctx: LabelBuildContext,
  kinds?: ReadonlySet<LabelKind>,
  viewport?: ViewportRel,
): LabelDescriptor[] {
  const descriptors: LabelDescriptor[] = [];
  // Kind filter (Phase 2 builds station labels only; Phase 3 drops the arg to
  // build all five). Absent = every kind, so existing callers are unchanged.
  const wants = (k: LabelKind): boolean => kinds === undefined || kinds.has(k);
  // Viewport filter (§4): keep only anchors inside the padded viewport. Absent =
  // whole-survey. Note the ORDINAL loops still run in full below (a shot's global
  // running index / a station's global ordinal must stay stable regardless of
  // what is on screen, so decimation parity holds) - only the `push` is gated.
  const inView = (x: number, y: number): boolean =>
    viewport === undefined ||
    (x >= viewport.minX && x <= viewport.maxX && y >= viewport.minY && y <= viewport.maxY);
  const lookup = buildStationLookup(data);
  // allRef[g] is the per-station kind mask (#27): 0 = a Normal shot touches it,
  // 1 = only Jumps, 2 = only References, 3 = both / no legs. Carried into the
  // label flags per-kind rather than collapsed, so a station label cannot
  // outlive the station marker it sits on.
  const allRef = computeStationAllReference(data, lookup);
  const stationKindFlags = (g: number): number => {
    const mask = allRef[g]!;
    if (mask === 0) return 0;
    return (
      ((mask & 1) !== 0 ? LABEL_FLAG.ALL_JUMP : 0) |
      ((mask & 2) !== 0 ? LABEL_FLAG.ALL_REFERENCE : 0)
    );
  };
  const stationRel = projectStationsRel(data, gpsOrigin, anchorX, anchorY);
  const { colorCtx } = ctx;

  // ─── Shot labels (+ shot comments) ─────────────────────────────
  const wantShotLabel = wants(LABEL_KIND.SHOT_LABEL);
  const wantShotComment = wants(LABEL_KIND.SHOT_COMMENT);
  let globalShotOrdinal = 0;
  if (wantShotLabel || wantShotComment) for (const section of data.sections) {
    const inner = lookup.get(section.id);
    if (!inner) continue;
    for (let i = 0; i < section.shotCount; i++) {
      const shotGlobalIdx = section.shotOffset + i;
      const fromG = inner.get(i);
      const toG = inner.get(i + 1);
      if (fromG === undefined || toG === undefined) continue;
      const shotKind = shotKindFromFlags(data.shotFlags[shotGlobalIdx]!);
      // Carry WHICH virtual kind, not just "virtual": the two hide independently
      // (#27), so a single shared bit orphans one kind's labels over collapsed
      // geometry whenever only the other kind is hidden.
      const virtualFlag =
        shotKind === 'Jump' ? LABEL_FLAG.JUMP : shotKind === 'Reference' ? LABEL_FLAG.REFERENCE : 0;
      const fx = data.stationCoords[fromG * 3]!;
      const fy = data.stationCoords[fromG * 3 + 1]!;
      const fz = data.stationCoords[fromG * 3 + 2]!;
      const tx = data.stationCoords[toG * 3]!;
      const ty = data.stationCoords[toG * 3 + 1]!;
      const tz = data.stationCoords[toG * 3 + 2]!;
      // Text uses the station-z average (labelDepth); color uses the shot
      // depth_in/out average (avgDepth) - two different depths, per features.ts.
      const labelDepth = (fz + tz) / 2;
      const avgDepth = (data.shotDepthIn[shotGlobalIdx]! + data.shotDepthOut[shotGlobalIdx]!) / 2;
      const azimuthIn = data.shotAzimuthIn[shotGlobalIdx]!;
      const length = data.shotLength[shotGlobalIdx]!;
      const [ax, ay] = projectMeters((fx + tx) / 2, (fy + ty) / 2, gpsOrigin, anchorX, anchorY);
      const shotInView = inView(ax, ay);
      const shotId = data.shotIds[shotGlobalIdx]!;
      if (wantShotLabel && shotInView) descriptors.push({
        kind: LABEL_KIND.SHOT_LABEL,
        text: formatShotLabel(azimuthIn, length, labelDepth, ctx.precision),
        anchorX: ax,
        anchorY: ay,
        color: packCssColor(resolveShotLabelColor(avgDepth, colorCtx)),
        ordinal: globalShotOrdinal,
        flags: virtualFlag,
        sectionId: section.id,
        shotGlobal: shotGlobalIdx,
        shotId,
      });
      const shotComment = wantShotComment && shotInView ? data.shotComments.get(shotGlobalIdx) : undefined;
      if (shotComment !== undefined) {
        descriptors.push({
          kind: LABEL_KIND.SHOT_COMMENT,
          text: formatCommentLabel(shotComment),
          anchorX: ax,
          anchorY: ay,
          color: ctx.commentColor
            ? ctx.commentColor(`${section.id}:s${shotId}`)
            : packCssColor(colorCtx.labelColor),
          ordinal: 0,
          // Mirror the shot label: a virtual shot's comment carries its own
          // kind bit so the matching hide uniform collapses it with its
          // geometry. Without it the comment orphans (stays visible) when that
          // kind is hidden, since flags:0 never matches the hide mask.
          flags: virtualFlag,
          sectionId: section.id,
          shotGlobal: shotGlobalIdx,
          shotId,
        });
      }
      globalShotOrdinal++;
    }
  }

  // ─── Station labels (+ station comments) ───────────────────────
  const wantStationLabel = wants(LABEL_KIND.STATION_LABEL);
  const wantStationComment = wants(LABEL_KIND.STATION_COMMENT);
  const namesMode = ctx.stationLabelText === 'name';
  if (wantStationLabel || wantStationComment) {
    // Per-section min station index == the "first in section" predicate.
    const sectionMinIndex = new Map<number, number>();
    for (let g = 0; g < data.stationCount; g++) {
      const sid = data.stationSectionIds[g]!;
      const idx = data.stationIndices[g]!;
      const cur = sectionMinIndex.get(sid);
      if (cur === undefined || idx < cur) sectionMinIndex.set(sid, idx);
    }
    for (let g = 0; g < data.stationCount; g++) {
      const sid = data.stationSectionIds[g]!;
      const stationIndex = data.stationIndices[g]!;
      const z = data.stationCoords[g * 3 + 2]!;
      let flags = 0;
      if (stationIndex === sectionMinIndex.get(sid)) flags |= LABEL_FLAG.FIRST_IN_SECTION;
      flags |= stationKindFlags(g);
      const ax = stationRel[g * 2]!;
      const ay = stationRel[g * 2 + 1]!;
      const stationInView = inView(ax, ay);
      const stationText = !(wantStationLabel && stationInView)
        ? null
        : namesMode
          ? formatStationNameLabel(data.stationNames.get(g))
          : formatStationLabel(stationIndex);
      if (stationText !== null) descriptors.push({
        kind: LABEL_KIND.STATION_LABEL,
        text: stationText,
        anchorX: ax,
        anchorY: ay,
        color: packCssColor(resolveStationLabelColor(z, colorCtx)),
        ordinal: g,
        flags,
        sectionId: sid,
        stationGlobal: g,
        stationIndex,
      });
      const stationComment = wantStationComment && stationInView ? data.stationComments.get(g) : undefined;
      if (stationComment !== undefined) {
        descriptors.push({
          kind: LABEL_KIND.STATION_COMMENT,
          text: formatCommentLabel(stationComment),
          anchorX: ax,
          anchorY: ay,
          color: ctx.commentColor
            ? ctx.commentColor(`${sid}:st${stationIndex}`)
            : packCssColor(colorCtx.labelColor),
          ordinal: 0,
          // Mirror the station label: an all-reference station's comment carries
          // ALL_REFERENCE so the hide-reference uniform collapses it with its
          // geometry (the station comment shows below the station label). Without
          // it the comment orphans when reference shots are hidden.
          flags: stationKindFlags(g),
          sectionId: sid,
          stationGlobal: g,
          stationIndex,
        });
      }
    }
  }

  // ─── Section names ─────────────────────────────────────────────
  if (wants(LABEL_KIND.SECTION_NAME)) for (const section of data.sections) {
    const inner = lookup.get(section.id);
    if (!inner || inner.size === 0) continue;
    // Prefer the first NON-reference station (so the name doesn't float when
    // reference shots are hidden); fall back to the first station overall.
    let anchorGlobal: number | undefined;
    let anchorStationIdx = Infinity;
    let foundNonRef = false;
    // Union of the kinds touching this section's stations, for the case where
    // NONE of them has a Normal shot: the section name then hides on the same
    // per-kind rule its stations do, not on a both-off rule (#27).
    let sectionKindFlags = 0;
    for (const [stIdx, gIdx] of inner.entries()) {
      sectionKindFlags |= stationKindFlags(gIdx);
      const hasNonRef = allRef[gIdx] === 0;
      if (foundNonRef && !hasNonRef) continue;
      if (hasNonRef && !foundNonRef) {
        foundNonRef = true;
        anchorStationIdx = Infinity; // restart the min search among non-ref stations
      }
      if (stIdx < anchorStationIdx) {
        anchorStationIdx = stIdx;
        anchorGlobal = gIdx;
      }
    }
    if (anchorGlobal === undefined) continue;
    const secX = stationRel[anchorGlobal * 2]!;
    const secY = stationRel[anchorGlobal * 2 + 1]!;
    if (!inView(secX, secY)) continue;
    descriptors.push({
      kind: LABEL_KIND.SECTION_NAME,
      text: formatSectionLabel(section.name),
      anchorX: secX,
      anchorY: secY,
      color: packCssColor(resolveSectionLabelColor(section.color)),
      ordinal: 0,
      flags: foundNonRef ? 0 : sectionKindFlags,
      sectionId: section.id,
    });
  }

  return descriptors;
}

// Per-glyph corner template: v0 top-left, v1 top-right, v2 bottom-left,
// v3 bottom-right. `useR` picks xR over xL; `useB` picks yBot over yTop.
const CORNER_USE_R = [0, 1, 0, 1];
const CORNER_USE_B = [0, 0, 1, 1];

/**
 * Stamp one shaped label's glyph quads into `b` starting at glyph `startGlyph`,
 * returning the next free glyph index. The single writer (mirrors
 * `emitLineSection` / `emitStation`): all 4 verts of a glyph share the label's
 * anchor / color / kind / revealZoom / flags; `offsetEm` + `uv` come from the
 * shaped glyph corners. `revealZoom` (min live zoom at which the label draws)
 * defaults to 0 (visible from any zoom - the whole-survey / no-projector
 * fallback). Two-triangle winding `0,1,2, 2,1,3` matches the geometry layer.
 */
export function emitLabel(
  b: LabelBuffers,
  startGlyph: number,
  shaped: ShapedLabel,
  desc: LabelDescriptor,
  flagByte: number = desc.flags,
  revealZoom = 0,
): number {
  const { anchor, offsetEm, uv, color, kind, revealZoom: reveal, flags, indices } = b;
  const [cr, cg, cb, ca] = desc.color;
  let glyph = startGlyph;
  for (const g of shaped.glyphs) {
    const base = glyph * 4;
    for (let c = 0; c < 4; c++) {
      const v = base + c;
      const useR = CORNER_USE_R[c]!;
      const useB = CORNER_USE_B[c]!;
      anchor[v * 2] = desc.anchorX;
      anchor[v * 2 + 1] = desc.anchorY;
      offsetEm[v * 2] = useR ? g.xR : g.xL;
      offsetEm[v * 2 + 1] = useB ? g.yBot : g.yTop;
      uv[v * 2] = useR ? g.ax + g.aw : g.ax;
      uv[v * 2 + 1] = useB ? g.ay + g.ah : g.ay;
      color[v * 4] = cr;
      color[v * 4 + 1] = cg;
      color[v * 4 + 2] = cb;
      color[v * 4 + 3] = ca;
      kind[v] = desc.kind;
      reveal[v] = revealZoom;
      flags[v] = flagByte;
    }
    const io = glyph * 6;
    indices[io] = base;
    indices[io + 1] = base + 1;
    indices[io + 2] = base + 2;
    indices[io + 3] = base + 2;
    indices[io + 4] = base + 1;
    indices[io + 5] = base + 3;
    glyph++;
  }
  return glyph;
}

/**
 * Shape every descriptor's text once (one {@link ShapedLabel} per descriptor,
 * index-aligned). Split from {@link buildLabelBuffers} so Phase 4 can shape ONCE
 * and feed the same shaped set to both collision (which needs each label's em
 * extent, `labelLayout.ts::shapedExtent`) and the buffer emit
 * ({@link buildLabelBuffersFromShaped}) - no double shaping.
 */
export function shapeDescriptors(
  descriptors: LabelDescriptor[],
  atlas: GlyphAtlas,
  cache?: ShapeCache,
): ShapedLabel[] {
  const shaped: ShapedLabel[] = new Array(descriptors.length);
  for (let i = 0; i < descriptors.length; i++) {
    const d = descriptors[i]!;
    shaped[i] = cache ? cache.get(d.kind, d.text, atlas) : shapeLabel(d.text, d.kind, atlas);
  }
  return shaped;
}

/**
 * Shape every descriptor and build the per-glyph label buffers. Allocates once
 * to the exact total glyph count, then stamps each label via {@link emitLabel}.
 */
export function buildLabelBuffers(descriptors: LabelDescriptor[], atlas: GlyphAtlas): LabelBuffers {
  return buildLabelBuffersFromShaped(descriptors, shapeDescriptors(descriptors, atlas));
}

/**
 * Build the per-glyph label buffers from a PRE-SHAPED label set (index-aligned
 * with `descriptors`). The single allocation + emit half of {@link buildLabelBuffers}.
 * `flagFor(i)` supplies the per-label flag byte (default: the descriptor's static
 * `flags`); the controller passes a resolver that folds in the dynamic DIM /
 * DRAG_HIDE bits so they land in the fresh buffer with no separate upload - the
 * same bytes {@link buildLabelFlags} would produce for a later flags-only
 * refresh. `revealFor(i)` supplies the per-label reveal zoom (default 0 = visible
 * from any zoom), the same values {@link buildLabelRevealZooms} would produce.
 */
export function buildLabelBuffersFromShaped(
  descriptors: LabelDescriptor[],
  shaped: ShapedLabel[],
  flagFor?: (index: number) => number,
  revealFor?: (index: number) => number,
): LabelBuffers {
  let glyphCount = 0;
  for (let i = 0; i < shaped.length; i++) glyphCount += shaped[i]!.glyphCount;

  const vertexCount = glyphCount * 4;
  const anchor = new Float32Array(vertexCount * 2);
  const offsetEm = new Float32Array(vertexCount * 2);
  const uv = new Float32Array(vertexCount * 2);
  const color = new Uint8Array(vertexCount * 4);
  const kind = new Uint8Array(vertexCount);
  const revealZoom = new Float32Array(vertexCount);
  const flags = new Uint8Array(vertexCount);
  const indices = new Uint32Array(glyphCount * 6);
  const byteLength =
    anchor.byteLength +
    offsetEm.byteLength +
    uv.byteLength +
    color.byteLength +
    kind.byteLength +
    revealZoom.byteLength +
    flags.byteLength +
    indices.byteLength;
  const b: LabelBuffers = {
    glyphCount,
    vertexCount,
    anchor,
    offsetEm,
    uv,
    color,
    kind,
    revealZoom,
    flags,
    indices,
    byteLength,
  };

  let cursor = 0;
  for (let i = 0; i < descriptors.length; i++) {
    const flagByte = flagFor ? flagFor(i) : descriptors[i]!.flags;
    const reveal = revealFor ? revealFor(i) : 0;
    cursor = emitLabel(b, cursor, shaped[i]!, descriptors[i]!, flagByte, reveal);
  }
  return b;
}

/**
 * Build ONLY the per-vertex flags array (length `vertexCount`) from a pre-shaped
 * label set, stamping `flagFor(i)` across every vertex of descriptor `i`. The
 * cheap flags-only refresh path: the controller recomputes DIM / DRAG_HIDE from
 * its interaction state and re-uploads just this array
 * (`SurveyCustomLabelLayer.updateFlags`) - no reshape, reproject, or re-emit of
 * positions / uv / color. `flagFor` is indexed the same as the shaped array (==
 * the descriptor array), so a byte written here lands on exactly the vertices
 * `buildLabelBuffersFromShaped` would have stamped for that label.
 */
export function buildLabelFlags(shaped: ShapedLabel[], flagFor: (index: number) => number): Uint8Array {
  let vertexCount = 0;
  for (let i = 0; i < shaped.length; i++) vertexCount += shaped[i]!.glyphCount * 4;
  const flags = new Uint8Array(vertexCount);
  let v = 0;
  for (let i = 0; i < shaped.length; i++) {
    const byte = flagFor(i);
    const verts = shaped[i]!.glyphCount * 4;
    for (let k = 0; k < verts; k++) flags[v++] = byte;
  }
  return flags;
}

/**
 * Build ONLY the per-vertex reveal-zoom array (length `vertexCount`) from a
 * pre-shaped label set, stamping `revealFor(i)` across every vertex of
 * descriptor `i`. The cheap reveal-only refresh path (sibling of
 * {@link buildLabelFlags}): a kind / reference toggle reruns the reveal pass off
 * the render loop and re-uploads just this array
 * (`SurveyCustomLabelLayer.updateReveal`) - no reshape, reproject, or re-emit -
 * so decluttering updates immediately without a buffer rebuild. Index-aligned
 * with the shaped array exactly like {@link buildLabelFlags}.
 */
export function buildLabelRevealZooms(shaped: ShapedLabel[], revealFor: (index: number) => number): Float32Array {
  let vertexCount = 0;
  for (let i = 0; i < shaped.length; i++) vertexCount += shaped[i]!.glyphCount * 4;
  const reveal = new Float32Array(vertexCount);
  let v = 0;
  for (let i = 0; i < shaped.length; i++) {
    const value = revealFor(i);
    const verts = shaped[i]!.glyphCount * 4;
    for (let k = 0; k < verts; k++) reveal[v++] = value;
  }
  return reveal;
}
