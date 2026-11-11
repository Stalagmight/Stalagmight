/**
 * Controller wiring the custom-render LABEL pass into SurveyLayer (#customRender
 * labels, Phase 2). Sibling of `controller.ts` (geometry); intentionally a
 * SEPARATE class + custom layer so labels enable / fail / rebuild independently
 * of the geometry layer (design §8: "the two custom layers can fail
 * independently"). SurveyLayer constructs one of these alongside the geometry
 * controller and calls `sync(...)` after each flush.
 *
 * SOLE renderer (no gate): the custom label layer is now the only label path -
 * `sync()` always builds/enables it and hides the five stock text layers. It is
 * an independent sibling of the geometry layer (labels anchor from `SurveyData`,
 * not the geometry buffers), so it enables / fails / rebuilds on its own. There
 * is NO stock fallback: an unrecoverable GL failure removes the dead layer and
 * calls `onFatal(reason)` (error state) instead of restoring stock. The
 * `setStockLabelVisibility` restore path survives only for the non-fatal
 * `destroy()` teardown (a later stage deletes the stock label layers).
 *
 * SCOPE (Phase 3): ALL FIVE kinds - shot labels, station labels, section names,
 * station comments, shot comments. It builds every kind's descriptors, hides the
 * five stock text layers while active, and drives each kind's visibility via a
 * shader uniform (the `setShow*` setters). Reference-shot hiding and the
 * move-comment palette tint are reproduced from the flags + `commentColor`
 * resolver already threaded through the tessellator.
 *
 * SCOPE: the build is VIEWPORT-SCOPED with a per-label REVEAL ZOOM
 * (`labelReveal.ts`) that folds the screen-space declutter AND the zoom-band
 * decimation into one float per label - the min live zoom at which it draws.
 * Only anchors inside the padded viewport build; below the label-build zoom
 * (13.5/13 hysteresis) only the cheap section names build (down to their own
 * minzoom 10), below 10 nothing. The reveal pass runs synchronously on every
 * `sync()` (edits stay immediate - the whole point of this project); labels are
 * emitted up to `buildZoom + EMIT_SLACK` so a modest zoom-in needs no rebuild at
 * all (the shader admits more labels per frame as the live zoom rises). A camera
 * settle only rebuilds when the padded viewport is EXITED (pan / zoom-out) or the
 * live zoom passes `buildZoom + EMIT_SLACK/2` (deep zoom-in) - zoom-in otherwise
 * needs no rebuild, which is the core latency fix. ADAPTIVE EMIT sharpens this at
 * high zoom: when the viewport-scoped candidate set is small (`ADAPTIVE_EMIT_MAX`)
 * the build emits it IN FULL (reveal ladder to the app max zoom) so a zoom-in is
 * ENTIRELY rebuild-free - the emitted slack window only bounds vertices in the
 * crowded low-zoom case. A per-kind / reference toggle
 * stays off the render loop: it flips the uniform and reruns the reveal pass
 * (cheap reveal-only re-upload), never a buffer rebuild. The one exception is
 * switching station labels between IDs and names (`setStationLabelMode`): the
 * text is baked into the glyph quads, so that switch rebuilds, like any edit.
 *
 * FONT READINESS: the SDF atlas needs the bundled font loaded (async). If a sync
 * arrives before the font resolves, it is remembered and built once the font is
 * ready (then `triggerRepaint`); every later sync is synchronous.
 */
import type { Map as MaplibreMap } from 'maplibre-gl';
import type { GpsOrigin, DecimalPrecision } from '../../../../types/config';
import type { SurveyData } from '../../../../types/survey-data';
import type { MapThemeColors } from '../../../../utils/themeColors';
import { debugLog } from '../../../../utils/debugLog';
import { FEATURE_ID_STRIDE } from '../../../../utils/ids';
import { commentColorForKey } from '../displayColor';
import { lngLatToMercator } from '../mercatorCoords';
import { computeMercatorAnchor } from './mercator';
import {
  buildLabelDescriptors,
  shapeDescriptors,
  buildLabelBuffersFromShaped,
  buildLabelFlags,
  buildLabelRevealZooms,
  type LabelDescriptor,
  type ViewportRel,
} from './labelTessellate';
import { shapedExtent, ShapeCache, type ExtentEm, type ShapedLabel } from './labelLayout';
import { computeRevealZooms, REVEAL_NEVER, EMIT_SLACK, ADAPTIVE_EMIT_MAX, emitSlackFor } from './labelReveal';
import { labelColorContextFrom, type StationLabelText } from './labelText';
import type { ColorScaleInput } from '../../../../utils/palettes';
import { LABEL_KIND, LABEL_FLAG, type LabelKind } from './labelShaders';
import { LABEL_KIND_STYLE } from './labelStyle';
import { packCssColor, type Rgba8 } from './colorPack';
import type { CustomDimState } from './controller';
import type { CustomRenderExclusion } from './tessellate';
import {
  GlyphAtlas,
  ensureLabelFont,
  createTinySdfRasterizer,
  canRasterize,
  type GlyphRasterizer,
} from './glyphAtlas';
import { resolveHaloColors, type LabelStyleInputs } from './labelStyle';
import { SurveyCustomLabelLayer, type LabelRenderData } from './surveyCustomLabelLayer';
import { CUSTOM_LABELS_SYNC_HOOK } from './labelsGate';
import { addCustomLayer, isStyleRestoring, reAddCustomLayer, removeCustomLayer } from './styleRestore';

/** Same off-the-render-stack deferral as the geometry controller. Used both for
 *  the fail-safe restore and to coalesce camera-settle viewport rebuilds. */
function defaultScheduleRestore(fn: () => void): void {
  if (typeof requestAnimationFrame === 'function') requestAnimationFrame(() => fn());
  else if (typeof queueMicrotask === 'function') queueMicrotask(fn);
  else setTimeout(fn, 0);
}

/** Whether `inner` is fully contained by `outer` (both anchor-relative mercator
 *  rects). The camera-exit test: the current on-screen bounds must stay inside
 *  the padded viewport the last build scoped to, else labels near an edge are
 *  missing and a rebuild is due. */
function rectContains(outer: ViewportRel, inner: ViewportRel): boolean {
  return (
    inner.minX >= outer.minX &&
    inner.maxX <= outer.maxX &&
    inner.minY >= outer.minY &&
    inner.maxY <= outer.maxY
  );
}

// ─── Phase 4: viewport scoping + camera-triggered rebuild ─────────────────────

/** Zoom hysteresis for the EXPENSIVE label kinds (shot/station labels + comments),
 *  matching the stock `SurveyLayer.LABEL_BUILD_ZOOM` / `LABEL_DROP_ZOOM` +
 *  `labelBuildDecision.ts`: build once past 13.5, drop only once below 13. Below
 *  the band those kinds build NOTHING. */
const LABEL_BUILD_ZOOM = 13.5;
const LABEL_DROP_ZOOM = 13;
/** Section names are cheap (1 per section) and orienting, so they follow their
 *  own `minzoom` (10, the stock `survey-labels` layer) rather than the expensive
 *  band - they still show at z10..13.5. Below 10 the shader hides them anyway. */
const SECTION_NAME_MIN_ZOOM = LABEL_KIND_STYLE[LABEL_KIND.SECTION_NAME].minzoom;
/** Padded-viewport margin as a fraction of the viewport span, applied to EACH
 *  side (0.5 → the build box is 2× the viewport per axis, a half-screen margin
 *  all round). Generous enough that a modest pan does not reach blank areas
 *  before the next camera-settle rebuild; tunable. */
const VIEWPORT_PADDING = 0.5;
/** Live zoom above `buildZoom + this` triggers a coalesced rebuild while zooming
 *  IN, extending the emitted slack window (EMIT_SLACK/2 = one band of headroom
 *  before the emitted set runs out). Only deep zoom-ins pay a rebuild. */
const ZOOM_IN_SLACK_REBUILD = EMIT_SLACK / 2;

/** Below the expensive band but at/above section minzoom: only section names. */
const SECTION_ONLY_KINDS: ReadonlySet<LabelKind> = new Set([LABEL_KIND.SECTION_NAME]);
/** Below section minzoom: nothing builds. */
const NO_KINDS: ReadonlySet<LabelKind> = new Set();

// ─── Phase 5: dim + drag-hide interaction state ──────────────────────────────

/** Anchored / immovable dim set - the SAME shape the geometry controller takes
 *  ({@link CustomDimState}), so SurveyLayer pushes one state to both. */
const EMPTY_DIM: CustomDimState = {
  anchoredSectionIds: new Set<number>(),
  immovableStationFeatureIds: new Set<number>(),
  immovableShotFeatureIds: new Set<number>(),
  gasDimStationFeatureIds: new Set<number>(),
  gasDimShotFeatureIds: new Set<number>(),
  planIdleDim: false,
};
/** Drag-exclusion set - the SAME shape the geometry controller takes
 *  ({@link CustomRenderExclusion}); dragged features' labels hide. */
const EMPTY_EXCLUSION: CustomRenderExclusion = {};

/** The single comment picked up during `move_comment` (C3). `type` selects the
 *  comment kind; `id` is the station index (station comment) or shot id (shot
 *  comment) within `sectionId` - the exact identity SurveyLayer.setHiddenComment
 *  receives. That one comment's label is collapsed (LABEL_FLAG.COMMENT_HIDE) so
 *  only the cursor-following tag shows during pickup. */
export interface HiddenComment {
  type: 'station' | 'shot';
  sectionId: number;
  id: number;
}

/** The padded viewport rect (anchor-relative mercator) used for the build set,
 *  the UNPADDED viewport rect (anchor-relative mercator) used for the "did the
 *  camera exit coverage?" test, and the world-pixel scale (CSS px per unit
 *  anchor-relative mercator at the current zoom) the reveal pass needs - all
 *  derived from ONE `map.getBounds()` read. `null` when the map cannot supply
 *  bounds / a canvas (unit-test fakes), in which case the build skips viewport
 *  scoping + reveal (whole-survey, every label visible). */
interface ViewportContext {
  viewport: ViewportRel;
  bounds: ViewportRel;
  worldPixelScale: number;
}

export interface LabelSyncParams {
  surveyData: SurveyData;
  gpsOrigin: GpsOrigin | null;
  themeColors: MapThemeColors;
  showDepthGradient: boolean;
  depthRange: { min: number; max: number };
  /** The user's depth palette + Levels range; null / absent = the theme ramp. */
  depthScale?: ColorScaleInput | null | undefined;
  precision?: DecimalPrecision | undefined;
  /** move_comment tool active → comment labels use the FNV-1a palette tint
   *  (`commentColorForKey`), mirroring the stock `commentTextColor()` (#52). */
  commentTint?: boolean | undefined;
}

export interface SurveyLabelControllerOptions {
  /** Defer the fail-safe restore off the render stack (tests inject inline). */
  scheduleRestore?: (fn: () => void) => void;
  /** Coalesce + defer a camera-settle viewport rebuild off the render stack
   *  (tests inject inline to run it synchronously). */
  scheduleRebuild?: (fn: () => void) => void;
  /** Fresh-geometry resync backing the runtime hook (SurveyLayer reads the store).
   *  Absent (unit tests): replay the cached snapshot. */
  resync?: () => void;
  /** Inject a ready atlas (tests / deterministic fake rasterizer), skipping the
   *  async font load so a build is synchronous from the first sync. */
  atlas?: GlyphAtlas;
  /** Font loader (default {@link ensureLabelFont}); tests can stub it. */
  ensureFont?: () => Promise<boolean>;
  /** Rasterizer factory (default {@link createTinySdfRasterizer}). */
  makeRasterizer?: () => GlyphRasterizer | null;
  /** Viewport-scoped candidate count at/below which a build emits the FULL set
   *  (adaptive emit; default {@link ADAPTIVE_EMIT_MAX}). Tests override it to a
   *  tiny value to force the bounded-slack path on a small fixture. */
  adaptiveEmitMax?: number;
  /** Active-state transition callback (mirrors the geometry controller). Fires
   *  only on a real change so SurveyLayer can drop / re-publish the stock label
   *  VT groups in lockstep with the custom label layer taking over. */
  onActiveChange?: (active: boolean) => void;
  /** Called ONCE on an UNRECOVERABLE GL failure (no WebGL2, program link
   *  failure, or a render-time `gl.getError` self-disable). There is no stock
   *  fallback: the controller removes its dead layer and invokes this so the app
   *  can surface an error state (labels are now absent). Mirrors the geometry
   *  controller's onFatal. */
  onFatal?: (reason: string) => void;
}

export class SurveyLabelController {
  private readonly map: MaplibreMap;
  private readonly layerId: string;
  private layer: SurveyCustomLabelLayer | null = null;
  private active = false;
  /** The glyph atlas - created once the font resolves (or injected in tests). */
  private atlas: GlyphAtlas | null = null;
  private lastData: LabelRenderData | null = null;
  private lastThemeColors: MapThemeColors | null = null;
  /** The most recent sync params, replayed on font-resolve / the runtime hook. */
  private pendingParams: LabelSyncParams | null = null;
  /** Set once `destroy()` runs. The async font-resolve callback (and any other
   *  deferred path) checks this so it never builds a `GlyphAtlas` or re-adds the
   *  layer on a torn-down controller -- previously this was prevented only
   *  incidentally by `pendingParams === null`. */
  private destroyed = false;

  private hookInstalled = false;
  private selfDisableScheduled = false;
  private readonly scheduleRestore: (fn: () => void) => void;
  private readonly scheduleRebuild: (fn: () => void) => void;
  private readonly resync: (() => void) | undefined;
  /** Active-state transition callback; see {@link SurveyLabelControllerOptions.onActiveChange}. */
  private readonly onActiveChange: ((active: boolean) => void) | undefined;
  /** Unrecoverable-GL-failure callback; see {@link SurveyLabelControllerOptions.onFatal}. */
  private readonly onFatal: ((reason: string) => void) | undefined;
  /** Latched once an unrecoverable GL failure removes the layer: a fatal
   *  controller is dead and every later `sync()` bails (no stock to fall back
   *  to). Mirrors the geometry controller. */
  private fatal = false;
  /** Guards {@link emitFatal} so {@link onFatal} fires at most once. */
  private fatalEmitted = false;

  /** Hysteresis latch for the expensive kinds (§4): true once past LABEL_BUILD_ZOOM,
   *  stays true until below LABEL_DROP_ZOOM. Drives {@link kindsToBuild}. */
  private fullLabelsBuilt = false;
  /** One camera-settle viewport rebuild is queued (coalesces a moveend+zoomend
   *  burst into a single rebuild). */
  private viewportRebuildScheduled = false;
  /** Bound moveend/zoomend handler, kept for `off()` in destroy. */
  private boundCameraSettle: (() => void) | null = null;

  // ── Phase 5 interaction state (mirrors the geometry controller) ──
  /** Anchored / immovable dim set (setDimming); applied to every build + the
   *  cheap flags-only refresh; survives re-enable. */
  private dimState: CustomDimState = EMPTY_DIM;
  /** Drag-exclusion set (setDragHide); dragged features' labels hide. */
  private hideState: CustomRenderExclusion = EMPTY_EXCLUSION;
  /** The single comment picked up during move_comment (setHiddenComment); its
   *  label collapses via LABEL_FLAG.COMMENT_HIDE. Flags-only, like the drag set. (C3) */
  private hiddenComment: HiddenComment | null = null;
  /** A comment tool (comment / move_comment) is active (setCommentOverlapExempt):
   *  the two comment kinds become overlap-exempt in the reveal pass so every
   *  comment stays visible / selectable. Drives the reveal-only re-upload. (C1) */
  private commentOverlapExempt = false;
  /** Cached build products backing the flags-only refresh + reveal rerun: the
   *  EMITTED (post-slack-filter) descriptor set + its shaped labels / em extents,
   *  the reveal-pass inputs (build zoom + world-pixel scale), and the last reveal
   *  zooms. Rewritten on every {@link build}. */
  private lastDescriptors: LabelDescriptor[] | null = null;
  private lastShaped: ShapedLabel[] | null = null;
  private lastExtents: (ExtentEm | null)[] | null = null;
  private lastBuildZoom: number | undefined = undefined;
  private lastWorldPixelScale: number | undefined = undefined;
  /** Emit-horizon slack the last build used (adaptive to the ceiling, or the
   *  bounded EMIT_SLACK). Reused by the reveal-only rerun so a toggle re-clamps to
   *  the same horizon the build emitted to. */
  private lastEmitSlack = EMIT_SLACK;
  /** Whether the last build was BOUNDED (candidate set too large to emit in
   *  full). Only a bounded build can run out of emitted slack on a zoom-in, so
   *  the zoom-in slack rebuild ({@link needsRebuild} clause 2) is gated on this -
   *  an adaptive (emit-all) build never schedules a pointless zoom-in rebuild.
   *  Defaults true (defensive before the first build). */
  private lastEmitBounded = true;
  /** The padded viewport rect (anchor-relative mercator) the last build scoped to
   *  + the anchor it was relative to, for the camera-exit rebuild test. */
  private builtViewport: ViewportRel | null = null;
  private lastAnchorX = 0;
  private lastAnchorY = 0;
  /** Whether the last build materialized the expensive kinds (hysteresis latch
   *  snapshot), so a settle can detect a band crossing that changes the set. */
  private lastBuiltWantFull = false;
  /** Shape cache shared across builds: a camera-driven re-scope re-projects +
   *  re-collides, never re-shapes unchanged (kind, text) labels. */
  private readonly shapeCache = new ShapeCache();

  /** Per-kind desired visibility (the setShow* toggles + comment-tool override),
   *  indexed by {@link LabelKind}. Held so a re-enable / stock restore reproduces
   *  it; drives `u_kind_visible` while active and each stock layer's restored
   *  visibility while inactive. Defaults mirror the stock layers' initial
   *  `visibility`: section names visible, everything else hidden. */
  private kindVisible: boolean[] = [false, false, true, false, false];
  /** What station labels say (IDs or names). Their visibility is
   *  `kindVisible[STATION_LABEL]`; this is the text a build bakes in. */
  private stationLabelText: StationLabelText = 'id';
  /** setShowReferenceShots state (false → collapse reference / all-reference). */
  private referenceShotsVisible = true;
  /** #27: jumps toggle separately. Label flags carry a single "virtual" bit, so
   *  a virtual shot's label hides only when BOTH kinds are hidden - the same
   *  conservative rule the circle shader uses for all-virtual stations. A label
   *  therefore never disappears while its own shot is still drawn. */
  private jumpShotsVisible = true;
  /** Adaptive-emit candidate threshold (see the option). */
  private readonly adaptiveEmitMax: number;

  constructor(map: MaplibreMap, layerId = 'survey-custom-labels', opts: SurveyLabelControllerOptions = {}) {
    this.map = map;
    this.layerId = layerId;
    this.scheduleRestore = opts.scheduleRestore ?? defaultScheduleRestore;
    this.scheduleRebuild = opts.scheduleRebuild ?? defaultScheduleRestore;
    this.resync = opts.resync;
    this.onActiveChange = opts.onActiveChange;
    this.onFatal = opts.onFatal;
    this.adaptiveEmitMax = opts.adaptiveEmitMax ?? ADAPTIVE_EMIT_MAX;
    this.installHook();
    // Camera-settle viewport rebuild (§4): re-scope + re-collide once the camera
    // stops. Coalesced, never per frame, never mid-animation. Both events route
    // here (a zoom fires moveend too - coalescing collapses the pair). Guarded
    // for unit-test fakes without an event emitter.
    this.boundCameraSettle = () => this.onCameraSettle();
    this.map.on?.('moveend', this.boundCameraSettle);
    this.map.on?.('zoomend', this.boundCameraSettle);
    if (opts.atlas) {
      this.atlas = opts.atlas;
    } else {
      const ensure = opts.ensureFont ?? ensureLabelFont;
      const makeRast = opts.makeRasterizer ?? createTinySdfRasterizer;
      void ensure().then((ready) => {
        if (this.destroyed) return; // torn down while the font was resolving.
        const rasterizer = ready && canRasterize() ? makeRast() : null;
        if (!rasterizer) {
          // Font asset failed, or a WebView without FontFace / canvas. There is no
          // stock label fallback anymore, so no labels can be drawn this session.
          // Building an INERT atlas would enable the layer while drawing ZERO
          // glyphs, so leave `this.atlas` null (sync() stays a no-op), latch fatal
          // so a later sync() bails, and SURFACE the failure - otherwise every
          // section / station / shot / comment label silently disappears.
          console.warn('[customRender] label font unavailable; labels disabled this session');
          this.fatal = true;
          this.emitFatal('label-font-unavailable');
          return;
        }
        this.atlas = new GlyphAtlas({ rasterizer });
        // A sync that arrived before the font resolved builds now (design §7).
        if (this.pendingParams) this.sync(this.pendingParams);
      });
    }
  }

  /** True while the custom label layer is drawing (stock text layers hidden).
   *  SurveyLayer reads this to route the setShow* toggles to the custom uniforms
   *  vs the stock layers' visibility. */
  isActive(): boolean {
    return this.active;
  }

  /** Build/refresh the label layer, all five kinds (the sole label path - no
   *  gate). */
  sync(params: LabelSyncParams): void {
    // A destroyed (unmounted) or fatal (dead GL) controller never rebuilds.
    if (this.destroyed || this.fatal) return;
    this.pendingParams = params;
    if (!this.atlas) return; // font not resolved yet - build on resolve.
    const data = this.build(params);
    if (!this.active) this.enable(data);
    else {
      this.layer?.update(data);
      this.pushStyle();
    }
  }

  /** Mirror the setShow* toggles (station labels: setStationLabelMode below):
   *  drive the kind's uniform when active (stock stays suppressed); store the
   *  desired state either way so a later enable / stock restore reproduces it. */
  setShotLabelsVisible(visible: boolean): void {
    this.setKindVisible(LABEL_KIND.SHOT_LABEL, visible);
  }
  /**
   * Station labels off, as IDs, or as names (the Display menu's exclusive
   * Station IDs / Station names pair). Showing or hiding the current text is
   * the usual uniform flip; changing the text rebuilds, because the text is
   * baked into the glyph quads (and names mode builds no label for an unnamed
   * station, so the declutter candidate set changes too). Hiding keeps the
   * current text, so turning the same labels back on needs no rebuild.
   */
  setStationLabelMode(mode: StationLabelText | 'off'): void {
    if (mode === 'off' || mode === this.stationLabelText) {
      this.setKindVisible(LABEL_KIND.STATION_LABEL, mode !== 'off');
      return;
    }
    this.stationLabelText = mode;
    this.kindVisible[LABEL_KIND.STATION_LABEL] = true;
    if (this.active) {
      this.rebuildViewport();
      this.pushStyle();
    }
  }
  setSectionNamesVisible(visible: boolean): void {
    this.setKindVisible(LABEL_KIND.SECTION_NAME, visible);
  }
  setStationCommentsVisible(visible: boolean): void {
    this.setKindVisible(LABEL_KIND.STATION_COMMENT, visible);
  }
  setShotCommentsVisible(visible: boolean): void {
    this.setKindVisible(LABEL_KIND.SHOT_COMMENT, visible);
  }

  private setKindVisible(kind: LabelKind, visible: boolean): void {
    this.kindVisible[kind] = visible;
    // The toggle itself is a pure uniform flip (the shader shows/hides the kind
    // with no rebuild). It also changes the collision CANDIDATE set, so re-run
    // the reveal pass + re-upload the reveal buffer - a toggled-off kind stops
    // occupying declutter space and a toggled-on kind is decluttered against
    // immediately. rerunReveal no-ops without a world-pixel scale (unit-test
    // fakes / below-gate), leaving the pure-uniform flip.
    if (this.active) {
      this.pushStyle();
      this.rerunReveal();
    }
  }

  /** Per-kind label hiding (#27). A label is dropped with the geometry of ITS
   *  OWN kind - a single both-off rule left one kind's shot-data and comment
   *  labels floating over the other kind's collapsed lines. The both-off case
   *  still applies to ALL_REFERENCE station/section labels, and that rule lives
   *  in the reveal pass + shader where the flag is read. */
  private hideReferenceLabels(): boolean {
    return !this.referenceShotsVisible;
  }

  private hideJumpLabels(): boolean {
    return !this.jumpShotsVisible;
  }

  /** Mirror setShowJumps (label half), #27. */
  setJumpShotsVisible(visible: boolean): void {
    this.jumpShotsVisible = visible;
    if (this.active) {
      this.pushStyle();
      this.rerunReveal();
    }
  }

  /** Mirror setShowReferenceShots (label half): flip the shader uniform, then
   *  re-declutter (reference-hide changes the collision candidate set). */
  setReferenceShotsVisible(visible: boolean): void {
    this.referenceShotsVisible = visible;
    if (this.active) {
      this.pushStyle();
      this.rerunReveal();
    }
  }

  /** Build the current NON-zoom style inputs (DPR, per-kind toggle, halo colors,
   *  reference-hide) the layer folds with live zoom each frame. */
  private computeStyle(): LabelStyleInputs {
    const dpr = (typeof window !== 'undefined' && window.devicePixelRatio) || 1;
    const haloColors = this.lastThemeColors ? resolveHaloColors(this.lastThemeColors) : [];
    return {
      dpr,
      kindEnabled: this.kindVisible.slice(),
      haloColors,
      hideReference: this.hideReferenceLabels(),
      hideJump: this.hideJumpLabels(),
    };
  }

  private pushStyle(): void {
    this.layer?.setStyleInputs(this.computeStyle());
    if (this.active) this.map.triggerRepaint();
  }

  /**
   * Build the GPU-ready label buffers from SoA, VIEWPORT-SCOPED with the REVEAL
   * ZOOM baked in. Below the expensive-kinds zoom band only section names build;
   * below their minzoom nothing does. Labels are shaped (through the shape cache),
   * their reveal zooms computed once (`labelReveal.ts`), then EMITTED up to
   * `buildZoom + EMIT_SLACK` so a modest zoom-in needs no rebuild. In a unit-test
   * fake with no `getZoom` the zoom gate + viewport + reveal are all skipped
   * (whole-survey, all kinds, every label visible from any zoom).
   */
  private build(params: LabelSyncParams): LabelRenderData {
    const t0 = performance.now();
    const { surveyData, gpsOrigin } = params;
    this.lastThemeColors = params.themeColors;
    const [anchorX, anchorY] = computeMercatorAnchor(surveyData, gpsOrigin);
    const colorCtx = labelColorContextFrom(
      params.showDepthGradient,
      params.depthRange,
      params.themeColors,
      params.depthScale ?? null,
    );
    // Comment palette tint (#52): only while move_comment is active; else comments
    // use the flat labelColor (the tessellator's fallback). Same key + palette as
    // colorResolvers.ts / features.ts, so the label tint matches the geometry tint.
    const commentColor = params.commentTint
      ? (ownerKey: string): Rgba8 => packCssColor(commentColorForKey(ownerKey))
      : undefined;

    const zoom = this.map.getZoom?.();
    const hasData = surveyData.stationCount > 0;
    const kinds = zoom === undefined ? undefined : this.kindsToBuild(zoom, hasData);
    const vpCtx = zoom === undefined ? null : this.computeViewportContext(anchorX, anchorY);

    const tDesc0 = performance.now();
    let descriptors = buildLabelDescriptors(
      surveyData, gpsOrigin, anchorX, anchorY,
      { colorCtx, precision: params.precision, commentColor, stationLabelText: this.stationLabelText },
      kinds, vpCtx?.viewport,
    );
    const tShape0 = performance.now();
    let shaped = shapeDescriptors(descriptors, this.atlas!, this.shapeCache);
    let extents = shaped.map(shapedExtent);

    // Reveal pass: fold declutter + decimation into one per-label reveal zoom,
    // then keep only labels within the emit horizon (buildZoom + slack). Only
    // runs when the map can supply bounds (a fake without bounds keeps every
    // label at reveal 0 = always visible). Retained (aligned with the EMITTED
    // subset) so the cheap reveal-only rerun on toggle stays index-consistent.
    const tReveal0 = performance.now();
    let reveal: Float32Array;
    // Adaptive emit: a small viewport-scoped set is emitted IN FULL (ladder to
    // the app max zoom) so a zoom-in never needs a rebuild; a large set keeps the
    // bounded slack horizon to cap the emitted vertex count. `descriptors.length`
    // is the viewport-scoped candidate count (pre reveal-filter) - the upper
    // bound on emitted labels, so thresholding it directly bounds the vertices.
    let emitBounded = false;
    let emitSlack = EMIT_SLACK;
    if (vpCtx !== null && zoom !== undefined) {
      emitBounded = descriptors.length > this.adaptiveEmitMax;
      emitSlack = emitSlackFor(descriptors.length, zoom, this.adaptiveEmitMax);
      const full = computeRevealZooms(descriptors, extents, {
        buildZoom: zoom,
        worldPixelScale: vpCtx.worldPixelScale,
        kindVisible: this.kindVisible,
        hideReference: this.hideReferenceLabels(),
      hideJump: this.hideJumpLabels(),
        commentOverlapExempt: this.commentOverlapExempt,
        slack: emitSlack,
      });
      // Emit only labels that clear within the horizon (finite reveal).
      const keptDesc: LabelDescriptor[] = [];
      const keptShaped: ShapedLabel[] = [];
      const keptExtents: (ExtentEm | null)[] = [];
      const keptReveal: number[] = [];
      for (let i = 0; i < descriptors.length; i++) {
        if (full[i]! === REVEAL_NEVER) continue;
        keptDesc.push(descriptors[i]!);
        keptShaped.push(shaped[i]!);
        keptExtents.push(extents[i]!);
        keptReveal.push(full[i]!);
      }
      descriptors = keptDesc;
      shaped = keptShaped;
      extents = keptExtents;
      reveal = Float32Array.from(keptReveal);
    } else {
      reveal = new Float32Array(descriptors.length); // all 0 → visible from any zoom
    }
    const tReveal1 = performance.now();

    // Retain build products for the cheap flag paths + reveal rerun, and the
    // viewport coverage the settle-rebuild test compares against.
    this.lastDescriptors = descriptors;
    this.lastShaped = shaped;
    this.lastExtents = extents;
    this.lastBuildZoom = zoom;
    this.lastWorldPixelScale = vpCtx?.worldPixelScale;
    this.lastEmitSlack = emitSlack;
    this.lastEmitBounded = emitBounded;
    this.builtViewport = vpCtx?.viewport ?? null;
    this.lastAnchorX = anchorX;
    this.lastAnchorY = anchorY;
    this.lastBuiltWantFull = this.fullLabelsBuilt;

    const buffers = buildLabelBuffersFromShaped(
      descriptors, shaped,
      (i) => this.labelFlagByte(descriptors[i]!),
      (i) => reveal[i]!,
    );
    debugLog('[customRender] label build', {
      ms: +(performance.now() - t0).toFixed(1),
      descMs: +(tShape0 - tDesc0).toFixed(1),
      shapeMs: +(tReveal0 - tShape0).toFixed(1),
      revealMs: +(tReveal1 - tReveal0).toFixed(1),
      zoom: zoom === undefined ? null : +zoom.toFixed(2),
      emit: emitBounded ? `bounded+${emitSlack}` : `all->${(zoom ?? 0) + emitSlack}`,
      labels: descriptors.length,
      glyphs: buffers.glyphCount,
      shapeCache: this.shapeCache.size,
      mb: +(buffers.byteLength / 1048576).toFixed(2),
    });
    this.lastData = { anchorX, anchorY, buffers, atlas: this.atlas! };
    return this.lastData;
  }

  /**
   * The full per-label flag byte = STATIC bits (FIRST_IN_SECTION / REFERENCE /
   * ALL_REFERENCE, from the descriptor) OR the dynamic interaction bits: DIM
   * (anchored / immovable) and DRAG_HIDE (a move tool is dragging its feature).
   * Decluttering is NOT a flag anymore - it lives in the per-label reveal zoom -
   * so a flags-only refresh (dim / drag) never disturbs the declutter and vice
   * versa. One place both the build emit and the flags-only refresh compute the
   * byte, so they never diverge.
   */
  private labelFlagByte(desc: LabelDescriptor): number {
    let f = desc.flags;
    if (this.dimBitFor(desc)) f |= LABEL_FLAG.DIM;
    if (this.dragBitFor(desc)) f |= LABEL_FLAG.DRAG_HIDE;
    if (this.commentHideBitFor(desc)) f |= LABEL_FLAG.COMMENT_HIDE;
    return f;
  }

  /**
   * Whether a label is the ONE comment picked up during move_comment
   * (setHiddenComment), keyed on the SAME identity the deleted stock
   * hidden-comment filter used: a station comment matches by `sectionId` +
   * `stationIndex`, a shot comment by `sectionId` + `shotId`. Only comment kinds
   * can match, so a station/shot LABEL sharing the anchor never hides. (C3)
   */
  private commentHideBitFor(desc: LabelDescriptor): boolean {
    const hidden = this.hiddenComment;
    if (hidden === null || desc.sectionId !== hidden.sectionId) return false;
    if (hidden.type === 'station') {
      return desc.kind === LABEL_KIND.STATION_COMMENT && desc.stationIndex === hidden.id;
    }
    return desc.kind === LABEL_KIND.SHOT_COMMENT && desc.shotId === hidden.id;
  }

  /**
   * Whether a label dims (0.25), keyed on the SAME state the stock label paint
   * expressions use (updateLabelAnchoredOpacity / updateImmovableLabelOpacity):
   *  - plan-idle tint (#395 revision): EVERY label dims, no exceptions - it
   *    mirrors the geometry side's "every feature dims" (controller.ts
   *    `dimForStation`/`dimForShot`), so labels don't stay full-bright while the
   *    whole survey underneath them tints.
   *  - shot label:    anchored(section) OR immovable-shot(section·STRIDE+shotId)
   *                   OR gas-dim-shot(section·STRIDE+shotId) (#395)
   *  - station label: anchored(section) OR immovable-station(section·STRIDE+idx)
   *                   OR gas-dim-station(section·STRIDE+idx) (#395)
   *  - section name:  anchored(section) only (never immovable- or gas-dimmed)
   *  - comments:      never (stock never dims comment layers), except under the
   *                   plan-idle tint above.
   *
   *  Labels only ever dim/don't (this bit feeds {@link LABEL_FLAG}.DIM, a single
   *  flag) - unlike the geometry's graded `a_dim` level, there is no lighter
   *  "gas-dim" look for labels.
   */
  private dimBitFor(desc: LabelDescriptor): boolean {
    const {
      anchoredSectionIds, immovableStationFeatureIds, immovableShotFeatureIds,
      gasDimStationFeatureIds, gasDimShotFeatureIds, planIdleDim,
    } = this.dimState;
    if (planIdleDim) return true;
    const sid = desc.sectionId;
    if (sid === undefined) return false;
    const anchored = anchoredSectionIds.has(sid);
    switch (desc.kind) {
      case LABEL_KIND.SHOT_LABEL:
        return (
          anchored ||
          (desc.shotId !== undefined &&
            ((immovableShotFeatureIds.size > 0 &&
              immovableShotFeatureIds.has(sid * FEATURE_ID_STRIDE + desc.shotId)) ||
              (gasDimShotFeatureIds.size > 0 &&
                gasDimShotFeatureIds.has(sid * FEATURE_ID_STRIDE + desc.shotId))))
        );
      case LABEL_KIND.STATION_LABEL:
        return (
          anchored ||
          (desc.stationIndex !== undefined &&
            ((immovableStationFeatureIds.size > 0 &&
              immovableStationFeatureIds.has(sid * FEATURE_ID_STRIDE + desc.stationIndex)) ||
              (gasDimStationFeatureIds.size > 0 &&
                gasDimStationFeatureIds.has(sid * FEATURE_ID_STRIDE + desc.stationIndex))))
        );
      case LABEL_KIND.SECTION_NAME:
        return anchored;
      default:
        return false; // comments: stock leaves them at full opacity
    }
  }

  /**
   * Whether a label is drag-hidden, keyed on the SAME drag exclusion the geometry
   * controller + the stock drag filters use (excludedStationGlobals /
   * excludedShotGlobals / whole-section drag): station-owned labels + comments
   * follow the station set, shot-owned follow the shot set, and either follows a
   * whole-section exclusion.
   *
   * SECTION NAMES now drag-hide too, for every AFFECTED section - a DELIBERATE
   * break from stock parity (stock left `survey-labels` out of the drag
   * hide-layers, so a section name stayed pinned at the OLD anchor while the
   * geometry moved, reading as stuck). A whole-section (segment) drag hides the
   * name via `sectionIds`; a partial station / shot drag hides it via the
   * `affectedSectionIds` set SurveyLayer threads in (the moved globals mapped
   * back to their sections). The name reappears at the new anchor on the
   * synchronous commit / cancel rebuild.
   */
  private dragBitFor(desc: LabelDescriptor): boolean {
    const { sectionIds, stationGlobalIndices, shotGlobalIndices, affectedSectionIds } = this.hideState;
    const sid = desc.sectionId;
    if (sid === undefined) return false;
    // Whole-section (segment) drag: the entire section translates, so every label
    // of the section hides - section name included.
    if (sectionIds?.has(sid) === true) return true;
    switch (desc.kind) {
      case LABEL_KIND.STATION_LABEL:
      case LABEL_KIND.STATION_COMMENT:
        return desc.stationGlobal !== undefined && stationGlobalIndices?.has(desc.stationGlobal) === true;
      case LABEL_KIND.SHOT_LABEL:
      case LABEL_KIND.SHOT_COMMENT:
        return desc.shotGlobal !== undefined && shotGlobalIndices?.has(desc.shotGlobal) === true;
      case LABEL_KIND.SECTION_NAME:
        // Partial station / shot drag: hide the name of every affected section.
        return affectedSectionIds?.has(sid) === true;
      default:
        return false;
    }
  }

  /**
   * Re-stamp ONLY the per-vertex flags buffer from the retained build products +
   * the current dim / drag / collision state, and re-upload it (no reshape /
   * reproject / recolor). The cheap path behind setDimming / setDragHide /
   * collision rerun. No-op until a build has run (state is still stored, so the
   * next build reproduces it).
   */
  private refreshFlags(): void {
    if (!this.active || !this.layer || !this.lastDescriptors || !this.lastShaped) return;
    const descriptors = this.lastDescriptors;
    const flags = buildLabelFlags(this.lastShaped, (i) => this.labelFlagByte(descriptors[i]!));
    this.layer.updateFlags(flags);
    this.map.triggerRepaint?.();
  }

  /**
   * Re-run the reveal pass over the retained (emitted) viewport set with the
   * CURRENT candidate inputs (kind toggles, reference-hide) and re-upload just
   * the reveal-zoom buffer - no reshape / reproject / recolor / rebuild. Used
   * when a toggle changes the collision candidate set so the declutter updates
   * off the render loop (a toggled-off kind frees space; a toggled-on kind is
   * decluttered against immediately). No-op without a world-pixel scale
   * (unit-test fakes / below-gate builds) - the toggle stays a pure uniform flip
   * there. Note: labels that were NOT emitted (reveal past the horizon at build
   * time) can only reappear at the next settle / edit rebuild - an accepted deep
   * approximation, since freed space beyond the emit slack is rare and a camera
   * nudge rebuilds.
   */
  private rerunReveal(): void {
    if (
      !this.active ||
      !this.layer ||
      !this.lastDescriptors ||
      !this.lastExtents ||
      !this.lastShaped ||
      this.lastWorldPixelScale === undefined ||
      this.lastBuildZoom === undefined
    ) {
      return;
    }
    const reveal = computeRevealZooms(this.lastDescriptors, this.lastExtents, {
      buildZoom: this.lastBuildZoom,
      worldPixelScale: this.lastWorldPixelScale,
      kindVisible: this.kindVisible,
      hideReference: this.hideReferenceLabels(),
      hideJump: this.hideJumpLabels(),
      commentOverlapExempt: this.commentOverlapExempt,
      slack: this.lastEmitSlack,
    });
    // The retained set is already the emitted subset; a rerun only lowers /
    // raises finite reveals within it (removing / re-adding competitors), so
    // clamp any REVEAL_NEVER back to the emit horizon (the SAME slack the build
    // emitted to) to keep the emitted label drawable at (or just past) the top of
    // the window rather than vanishing.
    const horizon = this.lastBuildZoom + this.lastEmitSlack;
    for (let i = 0; i < reveal.length; i++) {
      if (reveal[i]! === REVEAL_NEVER) reveal[i] = horizon;
    }
    this.layer.updateReveal(buildLabelRevealZooms(this.lastShaped, (i) => reveal[i]!));
    this.map.triggerRepaint?.();
  }

  /**
   * Update the anchored / immovable dim set (SurveyLayer.syncCustomDimming). Cheap
   * flags-only refresh; state stored regardless so the next build reproduces it.
   * Mirrors the geometry controller's {@link CustomDimState} setDimming.
   */
  setDimming(state: CustomDimState): void {
    this.dimState = state;
    if (this.active) this.refreshFlags();
  }

  /**
   * Point the label layer at (or off, with null) the active drag's hidden feature
   * set (SurveyLayer.setDragExclusion). Cheap flags-only refresh; state stored so
   * a mid-drag build / re-enable reproduces it. The dragged features' labels
   * collapse via DRAG_HIDE - the synchronous GPU flag that makes the stock
   * LabelRestoreController deferral unnecessary under the gate (§8).
   */
  setDragHide(exclusion: CustomRenderExclusion | null): void {
    this.hideState = exclusion ?? EMPTY_EXCLUSION;
    if (this.active) this.refreshFlags();
  }

  /**
   * Hide (or clear, with null) the single comment picked up during move_comment
   * (SurveyLayer.setHiddenComment). Cheap flags-only refresh - the one matching
   * comment label collapses via LABEL_FLAG.COMMENT_HIDE so only the
   * cursor-following tag shows; state stored so a mid-pickup build / re-enable
   * reproduces it. Mirrors {@link setDragHide}. (C3)
   */
  setHiddenComment(hidden: HiddenComment | null): void {
    this.hiddenComment = hidden;
    if (this.active) this.refreshFlags();
  }

  /**
   * Mark the two comment kinds overlap-exempt (or not) while a comment tool is
   * active (SurveyLayer comment-tool override). Reruns the reveal pass + re-uploads
   * just the reveal buffer (like a kind toggle): exempt comments skip the collision
   * cull and stay revealed at their minzoom, so every comment is visible /
   * selectable for editing. State stored so a build / re-enable reproduces it. (C1)
   */
  setCommentOverlapExempt(exempt: boolean): void {
    if (this.commentOverlapExempt === exempt) return;
    this.commentOverlapExempt = exempt;
    if (this.active) this.rerunReveal();
  }

  /**
   * Which kinds to build at `zoom` (§4). The expensive kinds (shot/station
   * labels + comments) follow the 13.5/13 hysteresis; section names follow their
   * own minzoom (10) so they still show at z10..13.5. Returns `undefined` (=
   * every kind) once past the band, a section-only set in [10, band), or an
   * empty set below 10. Also latches {@link fullLabelsBuilt} for the hysteresis.
   */
  private kindsToBuild(zoom: number, hasData: boolean): ReadonlySet<LabelKind> | undefined {
    const wantFull = this.fullLabelsBuilt ? zoom >= LABEL_DROP_ZOOM : zoom >= LABEL_BUILD_ZOOM;
    this.fullLabelsBuilt = wantFull && hasData;
    if (wantFull) return undefined;
    if (zoom >= SECTION_NAME_MIN_ZOOM) return SECTION_ONLY_KINDS;
    return NO_KINDS;
  }

  /**
   * The padded viewport rect (build set), the UNPADDED viewport rect (exit test)
   * - both anchor-relative mercator - and the world-pixel scale (CSS px per unit
   * anchor-relative mercator at the current zoom), all from one
   * `map.getBounds()`. Rotation + pitch are disabled app-wide (north-up, 0°
   * pitch) and mercator is isotropic, so the reveal pass scales the world-pixel
   * scale by `2^Δzoom` for other ladder zooms - no per-label `map.project`.
   * `null` when the map cannot supply bounds / a canvas (unit-test fakes → no
   * viewport scoping, no reveal).
   */
  private computeViewportContext(anchorX: number, anchorY: number): ViewportContext | null {
    const bounds = this.map.getBounds?.();
    const canvas = this.map.getCanvas?.();
    if (!bounds || !canvas) return null;
    const [minX] = lngLatToMercator(bounds.getWest(), bounds.getNorth()); // x ← lng
    const [maxX] = lngLatToMercator(bounds.getEast(), bounds.getNorth());
    const [, minY] = lngLatToMercator(bounds.getWest(), bounds.getNorth()); // y ← lat; north → min
    const [, maxY] = lngLatToMercator(bounds.getWest(), bounds.getSouth()); // south → max
    const spanX = maxX - minX;
    const spanY = maxY - minY;
    const width = canvas.clientWidth || canvas.width;
    const height = canvas.clientHeight || canvas.height;
    if (!(spanX > 0) || !(spanY > 0) || !(width > 0) || !(height > 0)) return null;
    const padX = spanX * VIEWPORT_PADDING;
    const padY = spanY * VIEWPORT_PADDING;
    const viewport: ViewportRel = {
      minX: minX - padX - anchorX,
      maxX: maxX + padX - anchorX,
      minY: minY - padY - anchorY,
      maxY: maxY + padY - anchorY,
    };
    const boundsRel: ViewportRel = {
      minX: minX - anchorX,
      maxX: maxX - anchorX,
      minY: minY - anchorY,
      maxY: maxY - anchorY,
    };
    return { viewport, bounds: boundsRel, worldPixelScale: width / spanX };
  }

  /**
   * A camera gesture ended (moveend/zoomend). Coalesce a burst into ONE deferred
   * rebuild EVALUATION (never per frame); the deferred callback skips if the
   * camera is still animating (a chained ease) and otherwise rebuilds only when
   * {@link needsRebuild} says the emitted set no longer covers the view. A plain
   * zoom-in (superset already emitted) does nothing - the shader admits more
   * labels per frame from their reveal zooms - which is the core latency fix.
   */
  private onCameraSettle(): void {
    if (!this.active || this.viewportRebuildScheduled) return;
    this.viewportRebuildScheduled = true;
    this.scheduleRebuild(() => {
      this.viewportRebuildScheduled = false;
      if (!this.active) return;
      if (this.map.isMoving?.()) return; // still animating - the next settle re-evaluates
      if (this.needsRebuild()) this.rebuildViewport();
    });
  }

  /**
   * Whether the emitted label set no longer covers the current camera, so a
   * rebuild is warranted. True when: the padded built viewport was EXITED (a pan
   * or a zoom-OUT grows the on-screen bounds past it); OR the last build was
   * BOUNDED and the live zoom rose past `buildZoom + EMIT_SLACK/2` (a deep
   * zoom-IN would soon run out of the emitted slack - moot for an adaptive
   * emit-all build, which reached the ceiling); OR the expensive-kinds hysteresis
   * band was crossed (the set of kinds to build changed). A plain zoom-in within
   * the slack (or any zoom-in after an adaptive build) returns false - no
   * rebuild, the reveal-zoom shader handles admission. Rebuilds defensively when
   * it cannot read the camera (no bounds / zoom).
   */
  private needsRebuild(): boolean {
    const zoom = this.map.getZoom?.();
    if (zoom === undefined) return true;
    const vpCtx = this.computeViewportContext(this.lastAnchorX, this.lastAnchorY);
    if (!vpCtx || this.builtViewport === null) return true;
    // 1. Camera exited the padded built viewport (pan / zoom-out).
    if (!rectContains(this.builtViewport, vpCtx.bounds)) return true;
    // 2. Zoom-in past the emitted slack window - ONLY for a bounded build. An
    //    adaptive (emit-all) build already emitted every reachable label to the
    //    ceiling, so a zoom-in has nothing more to gain and never rebuilds here.
    if (
      this.lastEmitBounded &&
      this.lastBuildZoom !== undefined &&
      zoom > this.lastBuildZoom + ZOOM_IN_SLACK_REBUILD
    ) {
      return true;
    }
    // 3. Expensive-kinds hysteresis band crossing (section-only ⇆ full).
    const hasData = (this.pendingParams?.surveyData.stationCount ?? 0) > 0;
    const wantFull = this.fullLabelsBuilt ? zoom >= LABEL_DROP_ZOOM : zoom >= LABEL_BUILD_ZOOM;
    if ((wantFull && hasData) !== this.lastBuiltWantFull) return true;
    // 4. Section-name minzoom crossing (nothing ⇆ section-only). A build below 10
    //    emits NO section names (empty set); the padded low-zoom viewport still
    //    covers a zoom-in into [10, band), and wantFull is false on both sides of
    //    10, so checks 1-3 all miss it. Without this, section names stay absent
    //    until 13.5 / an edit / a large pan. Compare the last build's zoom band
    //    against the live one (last build's zoom is undefined only before any
    //    build, handled by the earlier guards).
    if (
      this.lastBuildZoom !== undefined &&
      zoom >= SECTION_NAME_MIN_ZOOM !== this.lastBuildZoom >= SECTION_NAME_MIN_ZOOM
    ) {
      return true;
    }
    return false;
  }

  /** Re-scope + re-collide from the last sync params (geometry unchanged on a
   *  camera move, so replaying them is correct and cheap) and swap the buffer. */
  private rebuildViewport(): void {
    if (!this.active || !this.atlas || !this.pendingParams) return;
    const data = this.build(this.pendingParams);
    this.layer?.update(data);
    this.map.triggerRepaint?.();
  }

  private enable(data: LabelRenderData): void {
    if (!this.map.getStyle) return;
    const layer = new SurveyCustomLabelLayer(this.layerId, data, (reason) => this.handleSelfDisable(reason));
    layer.setStyleInputs(this.computeStyle());
    // No beforeId: the label layer draws on top (labels above geometry + overlays).
    addCustomLayer(this.map, layer);
    if (layer.isDisabled()) {
      // FATAL: the custom label layer cannot draw (no WebGL2 / shader link
      // failure). There is NO stock fallback - remove the dead layer and route
      // to the error state instead of restoring stock labels.
      const reason = layer.getDisabledReason() ?? 'unknown';
      console.warn(
        `[customRender] custom label layer failed to initialize (${reason}); ` +
          'survey labels unavailable',
      );
      removeCustomLayer(this.map, layer);
      this.layer = null;
      this.fatal = true;
      this.setActive(false); // active was never true here, so this does not fire.
      this.emitFatal(reason);
      return;
    }
    this.layer = layer;
    this.setActive(true);
    console.info('[customRender] label layer enabled', {
      glyphs: data.buffers.glyphCount,
    });
    this.map.triggerRepaint();
  }

  /** Re-add the layer after MapLibre rebuilt the style without it (context
   *  restore / worker restart, see styleRestore.ts). Labels stay on top. */
  reattach(): void {
    const layer = this.layer;
    if (!this.active || !layer) return;
    if (!reAddCustomLayer(this.map, layer)) {
      this.tearDownFatal(layer.getDisabledReason() ?? 'unknown');
      return;
    }
    this.map.triggerRepaint();
  }

  private disable(): void {
    this.teardown();
    console.info('[customRender] label layer disabled');
  }

  /** Remove the custom label layer (the sole label renderer - no stock to
   *  restore). Safe when already inactive. Non-fatal teardown only (destroy). */
  private teardown(): void {
    this.setActive(false);
    removeCustomLayer(this.map, this.layer);
    this.layer = null;
    this.map.triggerRepaint();
  }

  /** Set the active flag, firing {@link onActiveChange} only on a real change so
   *  SurveyLayer drops / re-publishes the stock label VT groups in lockstep. */
  private setActive(v: boolean): void {
    if (this.active === v) return;
    this.active = v;
    this.onActiveChange?.(v);
  }

  /**
   * The custom label layer hit a GL error mid-render and disabled itself. Fires
   * INSIDE MapLibre's frame, so defer the FATAL teardown off the render stack.
   * Idempotent + re-entrancy-safe (mirrors the geometry controller). No stock
   * fallback: the deferred handler routes to {@link tearDownFatal} (error state),
   * not the stock-restoring {@link teardown}.
   */
  private handleSelfDisable(reason: string): void {
    // A lost context fails every GL call; the restore's re-add rebuilds the layer.
    if (!this.active || this.selfDisableScheduled || isStyleRestoring(this.map)) return;
    this.selfDisableScheduled = true;
    console.warn(
      `[customRender] custom label layer self-disabled (${reason}); survey labels unavailable`,
    );
    this.scheduleRestore(() => {
      this.selfDisableScheduled = false;
      if (this.active) this.tearDownFatal(reason);
    });
  }

  /**
   * FATAL teardown: remove the dead label layer WITHOUT restoring or
   * republishing the stock label layers, latch {@link fatal} so a later sync()
   * bails, and surface the error via {@link onFatal}. Deliberately does NOT fire
   * `onActiveChange(false)` - that would drive SurveyLayer to republish the stock
   * label VT groups, exactly the fallback this stage removes.
   */
  private tearDownFatal(reason: string): void {
    this.fatal = true;
    removeCustomLayer(this.map, this.layer);
    this.layer = null;
    this.active = false; // set directly: onActiveChange must NOT fire (no republish).
    this.map.triggerRepaint();
    this.emitFatal(reason);
  }

  /** Fire {@link onFatal} at most once. */
  private emitFatal(reason: string): void {
    if (this.fatalEmitted) return;
    this.fatalEmitted = true;
    this.onFatal?.(reason);
  }

  private installHook(): void {
    if (this.hookInstalled || typeof window === 'undefined') return;
    (window as unknown as Record<string, unknown>)[CUSTOM_LABELS_SYNC_HOOK] = () => this.runSyncHook();
    this.hookInstalled = true;
  }

  private runSyncHook(): void {
    if (this.resync) this.resync();
    else if (this.pendingParams) this.sync(this.pendingParams);
  }

  /**
   * Lightweight release WITHOUT touching the map (no removeLayer, no stock
   * republish). Called on MapCanvas unmount via SurveyLayer.releaseVtRegistry(),
   * which deliberately skips the full {@link destroy} for unmount speed. Without
   * this the window sync hook would pin this controller (and its cached
   * SurveyData + glyph atlas + GL buffers) alive until the next SurveyLayer
   * overwrites the global -> hundreds of MB retained on large projects (#mem).
   * Uninstalls the hook, sets the {@link destroyed} guard so a late async
   * callback / sync bails, and nulls the heavy caches. `map.remove()` handles the
   * layer's GL teardown, so a republish here would be wasted work. Idempotent.
   */
  dispose(): void {
    this.destroyed = true;
    if (this.hookInstalled && typeof window !== 'undefined') {
      delete (window as unknown as Record<string, unknown>)[CUSTOM_LABELS_SYNC_HOOK];
      this.hookInstalled = false;
    }
    this.pendingParams = null;
    this.lastData = null;
    this.atlas = null;
  }

  destroy(): void {
    this.destroyed = true;
    if (this.active) this.disable();
    if (this.boundCameraSettle) {
      this.map.off?.('moveend', this.boundCameraSettle);
      this.map.off?.('zoomend', this.boundCameraSettle);
      this.boundCameraSettle = null;
    }
    // Reuse dispose() for the hook removal + cache clearing (DRY); the active-layer
    // teardown above is destroy()'s addition over the unmount-lite dispose() path.
    this.dispose();
  }
}
