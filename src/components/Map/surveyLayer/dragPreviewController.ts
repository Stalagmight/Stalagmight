// Drag-preview session (#576 D3 split): the capture, drag exclusion, interact-
// overlay follow and commit/end sequencing of a Move Station / Shot / Segment
// drag, moved verbatim out of SurveyLayer. SurveyLayer constructs one, hands it
// the shared DragGhostController that draws the moving geometry, and keeps the
// public begin/update/commit/end entry points as one-line delegates.
//
// The survey caches, colors and the interact overlay stay SurveyLayer's and are
// reached through {@link DragPreviewHost}, read live at each use like the
// `this.` reads they replace. The drag state SurveyLayer reads back (whether a
// drag is active / committing, the follow offset and the moving sets) is public.

import type { Map as MaplibreMap } from 'maplibre-gl';
import type * as maplibregl from 'maplibre-gl';
import type { LoopClosure, Section, StationRef } from '../../../types/project';
import type { GpsOrigin } from '../../../types/config';
import type { SurveyData } from '../../../types/survey-data';
import { metersToMapCoords } from '../../../utils/coordinates';
import { getErrorColor } from '../../../utils/colors';
import type { MapThemeColors } from '../../../utils/themeColors';
import type { AnalyticalOverlays } from './analyticalOverlays';
import { canWriteStyle } from '../mapUtils';
import { AUX_IDS } from './auxiliaryLayers';
import {
  resolveShotDisplayColor,
  resolveStationDisplayColor,
  type DisplayColorContext,
} from './displayColor';
import type { SurveyGeometryIndex } from './geometryIndex';
import type { DragPreviewFeature, DragPreviewSet, MovingEnd } from './dragPreviewSource';
import { DRAG_PREVIEW_SOURCE } from './dragPreviewLayers';
import type { DragGhostController } from './dragGhostController';
import type { CustomRenderExclusion, SurveyCustomController } from './customRender/controller';
import type { SurveyLabelController } from './customRender/labelController';

/** What the drag preview reads off SurveyLayer, live at each call. */
export interface DragPreviewHost {
  geoIndex(): SurveyGeometryIndex | null;
  surveyData(): SurveyData | null;
  sections(): Section[];
  loopClosures(): LoopClosure[] | undefined;
  gpsOrigin(): GpsOrigin | null;
  displayColorContext(): DisplayColorContext;
  themeColors(): MapThemeColors;
  parseFeatureId(id: string | null): number | null;
  customRender(): SurveyCustomController | null;
  customLabels(): SurveyLabelController | null;
  /** SurveyLayer.refreshInteractOverlay. */
  refreshInteractOverlay(): void;
  /** Drop the interact overlay's cache key so the next refresh rebuilds. */
  invalidateInteractOverlay(): void;
}

export class DragPreviewController {
  // Drag preview state - uses temporary layers with *-translate paint properties
  // instead of rebuilding GeoJSON on every frame
  dragPreviewActive = false;
  private dragSavedFilters = new Map<string, maplibregl.FilterSpecification | undefined>();
  /**
   * Saved filters a carry ended while the WebGL context was lost could not put
   * back. MapLibre serializes the style at the loss - the carry's exclusion
   * filter with it - and re-applies that on restore, so they are re-applied by
   * {@link replayAfterContextRestore} (#576 RV4-FE-SL1). Without this the
   * formerly carried sections' closures stayed hidden, and every later carry
   * saved the stale exclusion as the layer's "real" filter.
   */
  private filtersAwaitingRestore = new Map<string, maplibregl.FilterSpecification | undefined>();
  // Unified drag-preview source (#mem drag fixes): ONE GeoJSON source holding
  // every moving GEOMETRY feature for the active drag - stations (points), shots
  // (lines), loop-closure indicator lines - rebuilt from captured originals each
  // frame and pushed via a SINGLE setData. Real moved coordinates (not
  // `*-translate`) so the tiler clips moved geometry - no background wedges at
  // large offsets (BUG A); one setData so stations/shots can't skew apart (BUG B).
  // Attached text is not previewed (it is hidden from the stock text layers).
  // The moving ghost is owned by {@link DragGhostController}; DRAG_PREVIEW_SOURCE
  // is imported only for setDragExclusionWhenPreviewReady's stock-preview load gate.
  /** Captured moving geometry for the active drag (originals + movingEnd). */
  private dragPreviewSet: DragPreviewSet | null = null;
  /** rAF coalescer for the per-frame interact-overlay follow: MapCanvas may push
   *  several offsets per frame (pointermove + the map-'move' re-apply), but the
   *  overlay only needs the latest - one small setData per frame. (The GHOST is
   *  driven synchronously by {@link DragGhostController}, decoupled from this rAF.) */
  private dragUpdateRaf: number | null = null;
  private pendingDragOffset: { dx: number; dy: number } | null = null;
  /** Interact-overlay follow (#C): a hover/selected highlight on a moving
   *  station/shot is re-emitted at the drag offset each frame so it tracks the
   *  cursor. Stationary overlay features (snap ring, highlightedStationIds,
   *  unrelated selections) keep their true coordinates. */
  dragMovingStationKeys = new Set<string>();
  dragMovingShots = new Map<string, MovingEnd>();
  dragOverlayOffset: { dx: number; dy: number } | null = null;
  /** Set by commitDragPreview at placement: the follow-offset (dragOverlayOffset +
   *  moving sets) is deliberately KEPT applied across the IPC + flush window so the
   *  grabbed highlight stays on the cursor instead of snapping back to the OLD
   *  coords. endApplyFeatures drops it atomically, right before the post-commit
   *  overlay refresh reads the freshly installed geometry (no snap-back, and no
   *  old+offset overshoot). */
  dragCommitPending = false;
  /** Deferred custom-render drag-hide (START-flash fix). The moving originals are
   *  hidden from the custom WebGL layer only once the drag-preview GeoJSON source
   *  has parsed and drawn its first frame - the preview's setData is async (~5
   *  frames on a real project) while setDragHide is a synchronous GPU upload, so
   *  hiding eagerly leaves a window where the geometry is in NEITHER the custom
   *  layer nor the (not-yet-parsed) preview: a blank flash. Stored so it can be
   *  force-applied at commit and torn down on end; `dragGeneration` is bumped per
   *  drag so a late sourcedata callback from a superseded drag is ignored. */
  private dragGeneration = 0;
  private pendingDragHide: {
    exclusion: CustomRenderExclusion;
    listener: (e: maplibregl.MapSourceDataEvent) => void;
  } | null = null;

  constructor(
    private readonly map: MaplibreMap,
    private readonly dragGhost: DragGhostController,
    private readonly overlays: AnalyticalOverlays,
    private readonly host: DragPreviewHost,
  ) {}

  /** Install the captured drag preview geometry (shots / stations / closures) as
   *  the moving ghost, and hide the moving originals. Attached text is not
   *  previewed - it is hidden from the stock text layers for the drag duration by
   *  the begin methods. Shared by the segment and station/shot begins.
   *
   *  The ghost is a custom WebGL layer whose live drag delta is a single
   *  vertex-shader uniform (no worker, no re-bake -> no overshoot). It draws
   *  synchronously on the next frame, so the moving originals are hidden NOW (they
   *  overlap the ghost at offset 0). If the custom ghost is not the active path
   *  (bulk geometry stock, or the ghost layer failed / self-disabled), the STOCK
   *  GeoJSON preview stands in and its async setData means the hide waits for the
   *  first source load (START-flash fix). See {@link DragGhostController}. */
  private installDragPreview(features: DragPreviewFeature[], exclusion: CustomRenderExclusion): void {
    // New drag generation: invalidates any still-pending deferred drag-hide from
    // a superseded drag (guards the sourcedata callback in
    // setDragExclusionWhenPreviewReady).
    this.dragGeneration += 1;
    this.dragPreviewSet = { features };
    this.dragPreviewActive = true;
    const ghost = this.dragGhost.begin(this.dragPreviewSet);
    if (ghost === 'custom') this.setDragExclusion(exclusion);
    else if (ghost === 'stock') this.setDragExclusionWhenPreviewReady(exclusion);
    // 'idle': the WebGL context is lost (#576 NF-W4MAP-1). No preview source will
    // load to wait for - and isSourceLoaded reads the missing style - so the
    // originals are not hidden; nothing is drawn until the restore anyway.
    this.renderOverlayFollow(0, 0);
  }

  /** Re-emit any hover/selected highlight on a moving feature at the drag offset
   *  so it tracks the cursor instead of sticking at the committed position (#C).
   *  A small setData, coalesced one-per-frame by the rAF (see scheduleDragRender).
   *  Kept separate from the ghost bake so the highlight stays as responsive as
   *  before while the ghost bakes single-flight. */
  private renderOverlayFollow(dx: number, dy: number): void {
    if (!this.dragPreviewActive) return;
    this.dragOverlayOffset = { dx, dy };
    this.host.refreshInteractOverlay();
  }

  /** Stop the interact overlay following the drag and snap it back to committed
   *  coordinates (called on commit / end). */
  clearDragOverlayFollow(): void {
    this.dragOverlayOffset = null;
    this.dragCommitPending = false;
    this.dragMovingStationKeys.clear();
    this.dragMovingShots.clear();
    this.host.invalidateInteractOverlay();
    this.host.refreshInteractOverlay();
  }

  /** The per-feature analytical-overlay props (dist / gas-reach) a moving feature
   *  must carry so the preview's ramp paint matches the bulk layers. Keyed the
   *  same way the interact overlay keys them ("sec:idx" / "sec:shotId"). */
  private dragOverlayProps(idStr: string, kind: 'station' | 'shot'): Record<string, unknown> {
    const props: Record<string, unknown> = {};
    if (!this.overlays.distanceMode && !this.overlays.gasReachMode) return props;
    const fid = this.host.parseFeatureId(idStr);
    if (fid === null) return props;
    const map = kind === 'station'
      ? { dist: this.overlays.distanceState?.stations, gas: this.overlays.gasReachState?.stations }
      : { dist: this.overlays.distanceState?.shots, gas: this.overlays.gasReachState?.shots };
    if (this.overlays.distanceMode) props.dist = map.dist?.get(fid) ?? -1;
    if (this.overlays.gasReachMode) props.gasReach = map.gas?.get(fid) ?? -1;
    return props;
  }

  /**
   * Begin SEGMENT drag preview - hide the connected sections' real features from
   * the bulk layers (excluded by sectionId), capture every moving station / shot
   * / label / closure ONCE with its original coordinates, and render the moving
   * copy from the unified preview source. All geometry moves uniformly, so a
   * later per-frame offset is applied to every captured vertex.
   */
  beginDragPreview(connectedSectionIds: Set<number>): void {
    if (this.dragPreviewActive) this.endDragPreview();
    if (connectedSectionIds.size === 0) return;
    const geoIndex = this.host.geoIndex();
    const surveyData = this.host.surveyData();
    if (!geoIndex || !surveyData) return;

    const idsArray = [...connectedSectionIds];

    // Hide closures with EITHER endpoint in the connected set. Closures with BOTH
    // endpoints inside move uniformly (captured below); closures bridging a
    // dragged and a stationary section are hidden during the drag and reappear
    // with correct geometry on commit. (The survey geometry + labels are hidden in
    // the custom layers via the drag exclusion in installDragPreview.)
    // (Not while the WebGL context is lost: no style to filter, #576 NF-MAP-5; nor
    // while a rebuilt style loads: it holds no layer yet and refuses the write,
    // RV18-MAP-1.)
    if (canWriteStyle(this.map)) {
      const existingClosureFilter = this.map.getFilter(AUX_IDS.loopClosuresLayerId);
      this.dragSavedFilters.set(AUX_IDS.loopClosuresLayerId, existingClosureFilter ?? undefined);
      const eitherEndpointExclusion: maplibregl.FilterSpecification = ['!', ['any',
        ['in', ['get', 'sectionIdA'], ['literal', idsArray]],
        ['in', ['get', 'sectionIdB'], ['literal', idsArray]],
      ]] as unknown as maplibregl.FilterSpecification;
      this.map.setFilter(AUX_IDS.loopClosuresLayerId, (existingClosureFilter && typeof existingClosureFilter === 'object')
        ? (['all', existingClosureFilter, eitherEndpointExclusion] as unknown as maplibregl.FilterSpecification)
        : eitherEndpointExclusion);
    }

    // Capture the moving geometry once. Everything in the segment moves uniformly.
    // Attached text (labels / comments / section names) is not captured - it is
    // hidden from the stock layers above, not previewed.
    const features: DragPreviewFeature[] = [];
    const ctx = this.host.displayColorContext();
    const stationDefault = this.host.themeColors().stationDefault;

    // Read the moving geometry from the SoA (sectionGeometry), NOT from the lazy
    // `Station[]` proxy or the lazy `section.shots` getter: filtering all ~89k
    // stations per section (and a linear `shotGeometry` scan per shot) made this
    // quadratic and froze a 14k-section project on drag start (#mem).
    // Iterate cachedSections (not the Set) to keep the original emit order; reading
    // `id`/`color` off a section does not trip its lazy `shots` getter.
    for (const sectionMeta of this.host.sections()) {
      const sectionId = sectionMeta.id;
      if (!connectedSectionIds.has(sectionId)) continue;
      const sectionColor = sectionMeta.color ?? stationDefault;
      const { stations: sectionStations, shots: sectionShots } =
        geoIndex.sectionGeometry(sectionId);

      // Stations (points).
      for (const st of sectionStations) {
        const idStr = `${sectionId}:${st.stationIndex}`;
        this.dragMovingStationKeys.add(idStr);
        const color = resolveStationDisplayColor(
          surveyData,
          sectionColor,
          st.globalIdx,
          ctx,
        );
        features.push({
          emit: 'point',
          coords: [st.lngLat],
          movingEnd: 'uniform',
          properties: { kind: 'station', color, ...this.dragOverlayProps(idStr, 'station') },
        });
      }

      // Shots (lines).
      for (const shot of sectionShots) {
        const shotKey = `${sectionId}:${shot.shotId}`;
        const color = resolveShotDisplayColor(
          surveyData,
          sectionColor,
          shot.globalIdx,
          ctx,
        );
        this.dragMovingShots.set(shotKey, 'both');
        features.push({
          emit: 'line',
          coords: [shot.from, shot.to],
          movingEnd: 'uniform',
          properties: { kind: 'shot', color, ...this.dragOverlayProps(shotKey, 'shot') },
        });
      }
    }

    // Loop closures with BOTH endpoints in the moving set (move uniformly).
    for (const lc of this.host.loopClosures() ?? []) {
      if (!connectedSectionIds.has(lc.station_a.section_id) || !connectedSectionIds.has(lc.station_b.section_id)) continue;
      const a = geoIndex.stationCoords(lc.station_a);
      const b = geoIndex.stationCoords(lc.station_b);
      if (!a || !b) continue;
      features.push({
        emit: 'line',
        coords: [a, b],
        movingEnd: 'uniform',
        properties: { kind: 'closure', color: getErrorColor(lc.error_percentage) },
      });
    }

    // Segment drag hides whole connected sections (shots + stations) in the
    // custom layer, mirroring the sectionId exclusion applied to the stock layers.
    this.installDragPreview(features, { sectionIds: new Set(idsArray) });
  }

  /**
   * Begin station / shot drag preview - finds all co-located stations
   * (merged/closured), hides them and their connected shots/labels/closures from
   * the bulk layers, captures that moving geometry (with per-shot movingEnd so a
   * shot stretches around its fixed endpoint), and renders the moving copy from
   * the unified preview source. Everything moves through ONE per-frame setData.
   *
   * Returns the list of co-located StationRefs so MapCanvas can batch-move them.
   */
  beginStationDragPreview(sectionId: number, stationIndex: number, additionalSeeds?: StationRef[]): StationRef[] {
    if (this.dragPreviewActive) this.endDragPreview();

    // 1. Find clicked station and all co-located stations via BFS:
    //    - merged_with: always co-located
    //    - closures_with: only when the closure has been aligned
    //    additionalSeeds allows shot drag to seed from both endpoints simultaneously
    // key -> global SoA index, built from NUMBERS only. Mapping the lazy
    // `Station[]` proxy here materialized the whole ~89k station object graph on
    // every drag start (and `.filter()` below did it a second time) - the same
    // quadratic trap that froze segment drags (#mem).
    const data = this.host.surveyData();
    if (!data) return [];
    const globalByKey = new Map<string, number>();
    for (let g = 0; g < data.stationCount; g++) {
      globalByKey.set(`${data.stationSectionIds[g]}:${data.stationIndices[g]}`, g);
    }
    const alignedClosureIds = new Set<number>(
      (this.host.loopClosures() ?? [])
        .filter((closure) => closure.is_aligned)
        .map((closure) => closure.id)
    );
    const clickedKey = `${sectionId}:${stationIndex}`;
    if (!globalByKey.has(clickedKey)) return [];

    // Seed BFS from primary station + any additional seeds (e.g., shot's other endpoint)
    const seedKeys = [clickedKey];
    if (additionalSeeds) {
      for (const seed of additionalSeeds) {
        const key = `${seed.section_id}:${seed.station_index}`;
        if (globalByKey.has(key)) seedKeys.push(key);
      }
    }

    const movingRefKeys = new Set<string>();
    const queue: string[] = [...seedKeys];
    while (queue.length > 0) {
      const key = queue.pop()!;
      if (movingRefKeys.has(key)) continue;
      movingRefKeys.add(key);
      const g = globalByKey.get(key);
      if (g === undefined) continue;
      for (const ref of data.stationMergedWith.get(g) ?? []) {
        queue.push(`${ref.section_id}:${ref.station_index}`);
      }
      for (const cr of data.stationClosuresWith.get(g) ?? []) {
        if (!alignedClosureIds.has(cr.closure_id)) continue;
        queue.push(`${cr.station_ref.section_id}:${cr.station_ref.station_index}`);
      }
    }
    // The moving set is a merge/closure cluster (small); resolve each member's
    // coords + global index straight from the SoA.
    const movingStations = [...movingRefKeys].flatMap((key) => {
      const g = globalByKey.get(key);
      if (g === undefined) return [];
      const base = g * 3;
      return [{
        station_ref: {
          section_id: data.stationSectionIds[g]!,
          station_index: data.stationIndices[g]!,
        } as StationRef,
        lngLat: metersToMapCoords(
          data.stationCoords[base]!,
          data.stationCoords[base + 1]!,
          this.host.gpsOrigin(),
        ) as [number, number],
        globalIdx: g,
      }];
    });

    // 2. Capture the moving geometry once (originals + movingEnd). The moving copy
    // renders from the drag ghost; the originals are hidden in the custom layers via
    // the drag exclusion in installDragPreview. Only endpoints in the moving set
    // follow the offset, so a shot stretches around a fixed end instead of whole.
    const geoIndex = this.host.geoIndex();
    const surveyData = this.host.surveyData();
    if (!geoIndex || !surveyData) return movingStations.map(s => s.station_ref);
    const ctx = this.host.displayColorContext();
    const stationDefault = this.host.themeColors().stationDefault;
    const features: DragPreviewFeature[] = [];
    // Custom-render drag exclusion: the moving stations + shots, keyed by global
    // SoA index for O(1) skip in tessellate.
    const excludedStationGlobals = new Set<number>();
    const excludedShotGlobals = new Set<number>();
    // Sections owning any moved station / shot: their NAME label drag-hides too
    // (custom label layer), so it does not sit pinned at the old anchor while the
    // partial geometry moves. Whole-section segment drags carry the name via
    // `sectionIds` instead (see beginDragPreview).
    const affectedSectionIds = new Set<number>();

    // Moving stations (points) - all uniform.
    for (const st of movingStations) {
      const ref = st.station_ref;
      const idStr = `${ref.section_id}:${ref.station_index}`;
      this.dragMovingStationKeys.add(idStr);
      const section = geoIndex.section(ref.section_id);
      excludedStationGlobals.add(st.globalIdx);
      affectedSectionIds.add(ref.section_id);
      const color = resolveStationDisplayColor(
        surveyData,
        section?.color ?? stationDefault,
        st.globalIdx,
        ctx,
      );
      features.push({
        emit: 'point',
        coords: [st.lngLat],
        movingEnd: 'uniform',
        properties: { kind: 'station', color, ...this.dragOverlayProps(idStr, 'station') },
      });
    }

    // 4. Connected shots (lines) + shot labels. Walk each AFFECTED SECTION once
    // (from the SoA), not once per moving station: re-filtering the lazy
    // `Station[]` proxy per station materialized ~89k objects each time, and the
    // per-shot `shotGeometry()` scan made it quadratic in section size (#mem).
    const seenShotKeys = new Set<string>();
    const movingSectionIds = new Set(movingStations.map((st) => st.station_ref.section_id));
    for (const secId of movingSectionIds) {
      const section = geoIndex.section(secId);
      if (!section) continue;
      const { shots: sectionShots } = geoIndex.sectionGeometry(secId);

      for (const shot of sectionShots) {
        // Shot `index` joins station `index` to `index + 1`.
        const fromKey = `${secId}:${shot.index}`;
        const toKey = `${secId}:${shot.index + 1}`;
        const fromIsMoving = movingRefKeys.has(fromKey);
        const toIsMoving = movingRefKeys.has(toKey);
        if (!fromIsMoving && !toIsMoving) continue;

        const shotKey = `${secId}:${shot.shotId}`;
        if (seenShotKeys.has(shotKey)) continue;
        seenShotKeys.add(shotKey);

        const movingEnd: MovingEnd = fromIsMoving && toIsMoving ? 'both' : fromIsMoving ? 'from' : 'to';
        this.dragMovingShots.set(shotKey, movingEnd);
        const fromCoord = shot.from;
        const toCoord = shot.to;
        excludedShotGlobals.add(shot.globalIdx);
        affectedSectionIds.add(secId);
        const color = resolveShotDisplayColor(
          surveyData,
          section.color ?? stationDefault,
          shot.globalIdx,
          ctx,
        );
        features.push({
          emit: 'line',
          coords: [fromCoord, toCoord],
          movingEnd,
          properties: { kind: 'shot', color, ...this.dragOverlayProps(shotKey, 'shot') },
        });
      }
    }

    // (The moving shots are hidden in the custom geometry + label layers via the
    // drag exclusion in installDragPreview; no stock-layer filters to set.)

    // 5. Closure indicator lines with >= 1 endpoint in the moving set; hide those
    // from the bulk closures layer (they render in the preview, stretched).
    const closureExclusions: maplibregl.FilterSpecification[] = [];
    for (const lc of this.host.loopClosures() ?? []) {
      const keyA = `${lc.station_a.section_id}:${lc.station_a.station_index}`;
      const keyB = `${lc.station_b.section_id}:${lc.station_b.station_index}`;
      const aMoving = movingRefKeys.has(keyA);
      const bMoving = movingRefKeys.has(keyB);
      if (!aMoving && !bMoving) continue;

      // Coords from the SoA (O(1) per endpoint) - no Station materialization.
      const fromCoord = geoIndex.stationCoords(lc.station_a);
      const toCoord = geoIndex.stationCoords(lc.station_b);
      if (!fromCoord || !toCoord) continue;
      features.push({
        emit: 'line',
        coords: [fromCoord, toCoord],
        movingEnd: aMoving && bMoving ? 'both' : aMoving ? 'from' : 'to',
        properties: { kind: 'closure', color: getErrorColor(lc.error_percentage) },
      });
      closureExclusions.push(['all',
        ['==', ['get', 'stationA'], keyA],
        ['==', ['get', 'stationB'], keyB],
      ] as unknown as maplibregl.FilterSpecification);
    }
    if (closureExclusions.length > 0 && canWriteStyle(this.map)) {
      const closureExclusion: maplibregl.FilterSpecification = ['!',
        ['any', ...closureExclusions],
      ] as unknown as maplibregl.FilterSpecification;
      const existingClosureFilter = this.map.getFilter(AUX_IDS.loopClosuresLayerId);
      this.dragSavedFilters.set(AUX_IDS.loopClosuresLayerId, existingClosureFilter ?? undefined);
      const composedClosure: maplibregl.FilterSpecification = (existingClosureFilter && typeof existingClosureFilter === 'object')
        ? (['all', existingClosureFilter, closureExclusion] as unknown as maplibregl.FilterSpecification)
        : closureExclusion;
      this.map.setFilter(AUX_IDS.loopClosuresLayerId, composedClosure);
    }

    // Hide the moving originals from the custom layer. installDragPreview applies
    // this now for the custom ghost (draws next frame) or defers it behind the
    // async stock preview's first load (START-flash fix).
    this.installDragPreview(features, {
      stationGlobalIndices: excludedStationGlobals,
      shotGlobalIndices: excludedShotGlobals,
      affectedSectionIds,
    });
    return movingStations.map(s => s.station_ref);
  }

  /** Point the custom-render layer at (or off, with null) the active drag's
   *  hidden feature set, so the originals vanish at drag begin and reappear at
   *  drag end - matching the stock bulk layers' setFilter exclusions the custom
   *  layer cannot see. Cheap: recomputes + re-uploads only the per-vertex hide
   *  flag (no buffer rebuild). No-op unless the custom render gate is on. */
  private setDragExclusion(exclusion: CustomRenderExclusion | null): void {
    this.host.customRender()?.setDragHide(exclusion);
    // Labels mirror the SAME exclusion: the dragged features' labels collapse via
    // the DRAG_HIDE flag (synchronous GPU upload), replacing the stock text-layer
    // filter drop. On drag commit the synchronous sync() rebuild reflows them at
    // the new positions and setDragExclusion(null) clears the flag - labels never
    // lag the geometry (the anti-flicker payoff).
    this.host.customLabels()?.setDragHide(exclusion);
  }

  /**
   * Apply the custom-render drag-hide only once the drag-preview source has
   * parsed and drawn its first frame (START-flash fix). setDragHide is a
   * synchronous GPU upload that takes effect on the very next frame, but the
   * preview's GeoJSON setData is async (worker parse + retile, ~5 frames on a
   * real project). Hiding the originals eagerly therefore blanks the geometry
   * for those frames - it is in neither the custom layer nor the not-yet-drawn
   * preview. Deferring the hide to the source's first `sourcedata`-loaded event
   * keeps the originals visible until the preview stands in for them (at offset
   * ~0 they overlap, so there is no visible double either), then swaps to the
   * preview atomically.
   */
  private setDragExclusionWhenPreviewReady(exclusion: CustomRenderExclusion): void {
    this.clearPendingDragHide();
    // If the source is already parsed (rare - typically false right after the
    // begin's setData), hide immediately; nothing to wait for.
    if (this.map.isSourceLoaded(DRAG_PREVIEW_SOURCE)) {
      this.setDragExclusion(exclusion);
      return;
    }
    const gen = this.dragGeneration;
    const listener = (e: maplibregl.MapSourceDataEvent): void => {
      if (e.sourceId !== DRAG_PREVIEW_SOURCE) return;
      if (!this.map.isSourceLoaded(DRAG_PREVIEW_SOURCE)) return;
      this.map.off('sourcedata', listener);
      if (this.pendingDragHide?.listener === listener) this.pendingDragHide = null;
      // Ignore a stale callback from a drag that has since ended or been replaced.
      if (gen !== this.dragGeneration || !this.dragPreviewActive) return;
      this.setDragExclusion(exclusion);
    };
    this.pendingDragHide = { exclusion, listener };
    this.map.on('sourcedata', listener);
  }

  /** Tear down a still-pending deferred drag-hide without applying it (on end). */
  private clearPendingDragHide(): void {
    if (!this.pendingDragHide) return;
    this.map.off('sourcedata', this.pendingDragHide.listener);
    this.pendingDragHide = null;
  }

  /** Force-apply a still-pending deferred drag-hide immediately (on commit). If a
   *  very fast drag commits before the async preview ever loaded, the originals
   *  must be hidden NOW so they cannot show through the preview that is kept alive
   *  across the IPC window (see commitDragPreview). No-op once already applied. */
  private flushPendingDragHide(): void {
    const pending = this.pendingDragHide;
    if (!pending) return;
    this.map.off('sourcedata', pending.listener);
    this.pendingDragHide = null;
    if (this.dragPreviewActive) this.setDragExclusion(pending.exclusion);
  }

  /** Drive both followers from a pointermove offset. The GHOST is driven
   *  SYNCHRONOUSLY by the controller (custom path: one shader-uniform write +
   *  triggerRepaint, no worker; stock fallback: the single-flight bake / translate
   *  machine). The interact overlay follow stays rAF-coalesced (one small setData
   *  per frame). */
  scheduleDragRender(dx: number, dy: number): void {
    this.dragGhost.move(dx, dy);
    this.pendingDragOffset = { dx, dy };
    if (this.dragUpdateRaf !== null) return;
    this.dragUpdateRaf = requestAnimationFrame(() => {
      this.dragUpdateRaf = null;
      const off = this.pendingDragOffset;
      this.pendingDragOffset = null;
      if (off) this.renderOverlayFollow(off.dx, off.dy);
    });
  }

  /** Cancel any pending coalesced render (on commit / end). */
  cancelDragRender(): void {
    if (this.dragUpdateRaf !== null) {
      cancelAnimationFrame(this.dragUpdateRaf);
      this.dragUpdateRaf = null;
    }
    this.pendingDragOffset = null;
  }

  /**
   * Commit drag preview - removes the preview layers but keeps the exclusion
   * filters on the bulk layers so the moved features stay hidden until React
   * re-renders with their new positions. Call this before the backend commit,
   * then endDragPreview() after the re-render. This prevents a blink where
   * features briefly appear at newPosition + dragOffset.
   */
  commitDragPreview(): void {
    if (!this.dragPreviewActive) return;
    this.cancelDragRender();
    // FINISH-flash fix: do NOT remove the preview here. cancelDragRender above
    // freezes it at the placement offset (its last setData), which is where the
    // committed geometry will land. The committed geometry does not exist
    // client-side until the post-IPC flush installs it (~100ms), and the
    // custom-render drag-hide stays applied until endDragPreview - so removing
    // the preview now would leave the moved geometry in NEITHER the preview nor
    // the custom layer for the whole IPC window: a blank hole where the user just
    // placed it. Instead endDragPreview (sequenced via onNextUpdate, firing in the
    // same synchronous flush block that installs the committed geometry and clears
    // the hide) removes the preview and swaps to the committed geometry atomically.
    // Force-apply any still-deferred hide now so a very fast drag's stale
    // originals cannot show through the kept preview during the window.
    this.flushPendingDragHide();
    // Keep the interact overlay following (dragOverlayOffset + moving sets) across
    // the IPC + flush window so the grabbed highlight stays glued to the cursor
    // instead of snapping back to the OLD coords for the ~100ms until the committed
    // geometry lands. endApplyFeatures drops the offset atomically, right before the
    // post-commit overlay refresh reads the installed geometry - so the overlay
    // never renders old+offset after the install (no 2x flash) nor plain old coords
    // during the window (no snap-back). It must survive until the geometry swaps,
    // hence the dragCommitPending handoff. (dragPreviewSet is nulled by the
    // endDragPreview that MapCanvas sequences right after this via onNextUpdate.)
    this.dragCommitPending = true;
  }

  /**
   * End drag preview - removes the preview layers (if still present) and restores
   * every saved bulk-layer filter so the real features are visible again.
   */
  endDragPreview(): void {
    if (!this.dragPreviewActive) return;

    this.cancelDragRender();
    // Tear down the ghost (custom layer or stock follow machine + preview layers).
    // This is the single removal point for the preview now that commitDragPreview
    // keeps it alive across the IPC window.
    this.dragGhost.end();
    // Drop any deferred drag-hide that never fired (drag ended before ready).
    this.clearPendingDragHide();
    // Restore the saved auxiliary filters (only loop closures now - the survey
    // geometry + labels are custom-rendered and their drag exclusion is dropped
    // synchronously below, so they land at the new position at once).
    // While the context is lost there is no style to write, and a rebuilt one
    // has none of these layers until it loads (it would come back with the
    // exclusion serialized in): keep the originals for the restore (an earlier
    // lost carry's original wins: it is the real one).
    const live = canWriteStyle(this.map);
    for (const [layerId, saved] of this.dragSavedFilters) {
      if (live) {
        if (this.map.getLayer(layerId)) this.map.setFilter(layerId, saved ?? null);
      } else if (!this.filtersAwaitingRestore.has(layerId)) {
        this.filtersAwaitingRestore.set(layerId, saved);
      }
    }
    this.clearDragOverlayFollow();

    this.dragPreviewSet = null;
    this.dragSavedFilters.clear();
    this.dragPreviewActive = false;
    // Drop the custom-render exclusion and rebuild full buffers so the moved
    // features reappear at their committed positions (mirrors the filter restore).
    this.setDragExclusion(null);
  }

  /**
   * After a WebGL context restore (SurveyLayer.replayAfterContextRestore): put
   * back the filters a carry ended during the loss could not restore. A carry
   * picked up during the loss and still in flight saved nothing (no style to
   * read), so it adopts them and restores them at its own drop instead.
   */
  replayAfterContextRestore(): void {
    if (this.filtersAwaitingRestore.size === 0 || !canWriteStyle(this.map)) return;
    for (const [layerId, saved] of this.filtersAwaitingRestore) {
      if (this.dragPreviewActive) {
        if (!this.dragSavedFilters.has(layerId)) this.dragSavedFilters.set(layerId, saved);
      } else if (this.map.getLayer(layerId)) {
        this.map.setFilter(layerId, saved ?? null);
      }
    }
    this.filtersAwaitingRestore.clear();
  }
}
