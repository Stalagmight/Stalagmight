import { useEffect, useLayoutEffect, type MutableRefObject } from 'react';
import type * as maplibregl from 'maplibre-gl';
import type { Station } from '../../types/project';
import type { GpsOrigin } from '../../types/config';
import type { ProjectWarning } from '../../types/commands';
import type { ProjectStore } from '../../store/projectStore';
import type { UIStoreState } from '../../store/uiSelectKeys';
import {
  getEffectiveShowJumps,
  getEffectiveShowReferenceShots,
  getEffectiveShowShotData,
  getEffectiveShowStationComments,
  getEffectiveShowShotComments,
} from '../../store/displaySelectors';
import { getMapThemeColors } from '../../utils/themeColors';
import type { useDecimalPrecision } from '../../hooks/useDecimalPrecision';
import type SurveyLayer from './SurveyLayer';
import type { SurveyLayerCallbacks } from './SurveyLayer';
import { canWriteStyle } from './mapUtils';

// Helper to convert StationRef to highlight ID format (section_id:station_index)
// Moved outside component to avoid recreating on each render
const stationRefToId = (ref: { section_id: number; station_index: number } | null): string | null => {
  if (!ref) return null;
  return `${ref.section_id}:${ref.station_index}`;
};

interface SurveyLayerSyncOptions {
  map: MutableRefObject<maplibregl.Map | null>;
  surveyLayerRef: MutableRefObject<SurveyLayer | null>;
  mapLoaded: boolean;
  /** Moves once the style is back after a WebGL context restore (useStyleEpoch). */
  styleEpoch: number;
  project: ProjectStore['project'];
  activeStations: Station[];
  gpsOrigin: GpsOrigin | null;
  effectiveDepthGradient: boolean;
  depthRange: { min: number; max: number };
  precision: ReturnType<typeof useDecimalPrecision>;
  geometryMode: UIStoreState['geometryMode'];
  surveyDataAdjusted: ProjectStore['surveyDataAdjusted'];
  effectiveIssueSeverity: boolean;
  severityWarnings: ProjectWarning[];
  /** Must stay render-stable (#430) - MapCanvas memoizes it. */
  toolCallbacks: SurveyLayerCallbacks;
  config: ProjectStore['config'];
  resolvedTheme: UIStoreState['resolvedTheme'];
  selection: UIStoreState['selection'];
  snappedStation: UIStoreState['snappedStation'];
  mergeStationA: UIStoreState['mergeStationA'];
  mergeStationB: UIStoreState['mergeStationB'];
  showShotData: boolean;
  showShots: boolean;
  showSectionNames: boolean;
  showStationLabels: boolean;
  showStationComments: boolean;
  showStations: boolean;
  showShotComments: boolean;
  showReferenceShots: boolean;
  showJumps: boolean;
  moveCommentSource: UIStoreState['moveCommentSource'];
  anchoredSectionIds: Set<number>;
  immovableFeatureIds: { stationIds: Set<number>; shotIds: Set<number> };
}

/**
 * The effects that only push store / derived state into SurveyLayer: the
 * geometry update, the callbacks, config and theme, the selection, the snap /
 * merge highlight, label and shot-kind visibility, the move-comment hide, and
 * the anchored / immovable dims.
 *
 * Moved out of MapCanvas.tsx verbatim (#576 D3). MapCanvas calls it where the
 * setCallbacks effect used to be, so all the passive effects keep their place.
 * The geometry `update(...)` layout effect used to be declared further up, among
 * passive effects; layout effects run in their own phase before any passive
 * effect, and it is still the first of the layout effects it was ordered against
 * (selection, anchored, immovable), so the selection / dim clears still follow
 * the rebuild in the same commit exactly as their comments require.
 */
export function useSurveyLayerSync({
  map,
  surveyLayerRef,
  mapLoaded,
  styleEpoch,
  project,
  activeStations,
  gpsOrigin,
  effectiveDepthGradient,
  depthRange,
  precision,
  geometryMode,
  surveyDataAdjusted,
  effectiveIssueSeverity,
  severityWarnings,
  toolCallbacks,
  config,
  resolvedTheme,
  selection,
  snappedStation,
  mergeStationA,
  mergeStationB,
  showShotData,
  showShots,
  showSectionNames,
  showStationLabels,
  showStationComments,
  showStations,
  showShotComments,
  showReferenceShots,
  showJumps,
  moveCommentSource,
  anchoredSectionIds,
  immovableFeatureIds,
}: SurveyLayerSyncOptions): void {
  // Update survey layer when data changes or map loads.
  // useLayoutEffect ensures setData() runs before the browser paints - critical
  // for drag commit where endDragPreview() must fire in the same frame as setData().
  useLayoutEffect(() => {
    if (surveyLayerRef.current && project) {
      surveyLayerRef.current.update(
        project.sections,
        activeStations,
        // Closure overlays are drawn from the active coordinates, so an adjusted
        // (closed) loop collapses to a zero-length, invisible line, while a loop
        // that wasn't closed - an ineligible/deferred closure, or a component whose
        // solve fell back to measured - stays visible. Pass closures in both modes
        // rather than hiding them wholesale, so the adjusted view doesn't look
        // resolved when some loops are still open.
        project.loop_closures,
        gpsOrigin,
        effectiveDepthGradient,
        depthRange,
        precision,
        effectiveIssueSeverity,
        severityWarnings
      );
    }
  }, [project, activeStations, mapLoaded, gpsOrigin, effectiveDepthGradient, depthRange, precision, geometryMode, surveyDataAdjusted, effectiveIssueSeverity, severityWarnings]);

  // Update callbacks when they change. `mapLoaded` is a dependency on purpose:
  // SurveyLayer is constructed in the async map 'load' handler with whatever
  // `toolCallbacks` the map-creation effect closed over on first render, and
  // `toolCallbacks` is memoized (#430), so it can settle to a stable identity
  // while `surveyLayerRef.current` is still null and this effect no-ops. Re-run
  // it on the setMapLoaded(true) render to apply the current callbacks.
  useEffect(() => {
    if (surveyLayerRef.current) {
      surveyLayerRef.current.setCallbacks(toolCallbacks);
    }
  }, [toolCallbacks, mapLoaded]);

  // Sync config with SurveyLayer for visual and hitbox layer updates
  useEffect(() => {
    if (surveyLayerRef.current && config) {
      surveyLayerRef.current.setConfig(config);
    }
  }, [config, mapLoaded]);

  // Apply theme colors to SurveyLayer when resolvedTheme changes
  // MapLibre GL JS does NOT read CSS variables - colors must be re-applied explicitly
  // Not on a context restore (#576 RV4-FE-SL-4): with a live style this is a full
  // survey rebuild, and the restore already replays the survey with these colors.
  useEffect(() => {
    if (surveyLayerRef.current) {
      surveyLayerRef.current.applyThemeColors(getMapThemeColors());
    }
    // surveyLayerRef is MapCanvas's useRef object: stable, listed for the linter only.
  }, [resolvedTheme, mapLoaded, surveyLayerRef]);

  // Re-apply map background color to match the active theme. Not while the
  // WebGL context is lost (no style; the write would throw) nor while a rebuilt
  // style loads (it refuses the write): styleEpoch runs this again once the
  // restored style has loaded (#576 NF-MAP-5, RV18-MAP-1).
  useEffect(() => {
    if (map.current && mapLoaded && canWriteStyle(map.current)) {
      map.current.setPaintProperty('background', 'background-color', getMapThemeColors().background);
    }
    // map is MapCanvas's useRef object: stable, listed for the linter only.
  }, [resolvedTheme, mapLoaded, styleEpoch, map]);

  // Sync selection state with SurveyLayer for visual feedback.
  // useLayoutEffect (not useEffect) so it clears before paint, in the same commit
  // as the geometry-apply layout effect above. On a project switch the geometry
  // rebuild runs first and re-applies the controller's stored highlight; because
  // section ids are per-project sequential (they collide across projects), a
  // regular useEffect would let the colliding section flash highlighted for one
  // frame before this cleared it.
  useLayoutEffect(() => {
    if (!surveyLayerRef.current) return;

    if (selection?.type === 'station' && selection.stationRef) {
      surveyLayerRef.current.setSelectedStation(selection.stationRef);
      surveyLayerRef.current.setSelectedShot(null, null);
      surveyLayerRef.current.setSelectedSection(null);
    } else if (selection?.type === 'shot' && selection.sectionId !== undefined && selection.shotId !== undefined) {
      surveyLayerRef.current.setSelectedStation(null);
      surveyLayerRef.current.setSelectedShot(selection.sectionId, selection.shotId);
      surveyLayerRef.current.setSelectedSection(null);
    } else if (selection?.type === 'section' && selection.sectionId !== undefined) {
      // Section selection
      surveyLayerRef.current.setSelectedStation(null);
      surveyLayerRef.current.setSelectedShot(null, null);
      surveyLayerRef.current.setSelectedSection(selection.sectionId);
    } else {
      surveyLayerRef.current.setSelectedStation(null);
      surveyLayerRef.current.setSelectedShot(null, null);
      surveyLayerRef.current.setSelectedSection(null);
    }
  }, [selection]);

  // Sync snapped station with SurveyLayer for snap indicator
  // (Also called directly from checkSnapToStation for immediate visual response;
  // this effect is the fallback for store-driven changes like tool-change clears.)
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setSnappedStation(snappedStation);
  }, [snappedStation]);

  // Hover sync effects moved to <MapHoverSync> child component to avoid
  // re-rendering this large component on every mousemove (~60fps).

  // Sync highlight layer with snap/merge state
  // Shows snapped/origin stations even when stations layer is hidden
  useEffect(() => {
    if (!surveyLayerRef.current) return;

    const highlightIds: string[] = [];

    const snappedId = stationRefToId(snappedStation);
    if (snappedId) highlightIds.push(snappedId);

    const mergeAId = stationRefToId(mergeStationA);
    if (mergeAId && !highlightIds.includes(mergeAId)) highlightIds.push(mergeAId);

    const mergeBId = stationRefToId(mergeStationB);
    if (mergeBId && !highlightIds.includes(mergeBId)) highlightIds.push(mergeBId);

    surveyLayerRef.current.setHighlightedStations(highlightIds);
  }, [snappedStation, mergeStationA, mergeStationB, mapLoaded]);

  // Sync shot data labels visibility with SurveyLayer (gated by showShots)
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setShowShotLabels(getEffectiveShowShotData(showShots, showShotData));
  }, [showShotData, showShots, mapLoaded]);

  // Sync section name labels visibility with SurveyLayer
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setShowSectionNames(showSectionNames);
  }, [showSectionNames, mapLoaded]);

  // Sync station index labels visibility with SurveyLayer
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setShowStationLabels(showStationLabels);
  }, [showStationLabels, mapLoaded]);

  // Sync station comments visibility with SurveyLayer (gated by showStations)
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setShowStationComments(getEffectiveShowStationComments(showStations, showStationComments));
  }, [showStationComments, showStations, mapLoaded]);

  // Sync shot comments visibility with SurveyLayer (gated by showShots)
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setShowShotComments(getEffectiveShowShotComments(showShots, showShotComments));
  }, [showShotComments, showShots, mapLoaded]);

  // Hide source comment on map during move_comment pickup (no backend mutation on pickup)
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    if (moveCommentSource) {
      if (moveCommentSource.type === 'station' && moveCommentSource.stationRef) {
        surveyLayerRef.current.setHiddenComment({
          type: 'station',
          sectionId: moveCommentSource.stationRef.section_id,
          id: moveCommentSource.stationRef.station_index,
        });
      } else if (moveCommentSource.type === 'shot' && moveCommentSource.sectionId !== undefined && moveCommentSource.shotId !== undefined) {
        surveyLayerRef.current.setHiddenComment({
          type: 'shot',
          sectionId: moveCommentSource.sectionId,
          id: moveCommentSource.shotId,
        });
      }
    } else {
      surveyLayerRef.current.setHiddenComment(null);
    }
  }, [moveCommentSource, mapLoaded]);

  // Sync stations visibility with SurveyLayer
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setShowStations(showStations);
  }, [showStations, mapLoaded]);

  // Sync shots visibility with SurveyLayer
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setShowShots(showShots);
  }, [showShots, mapLoaded]);

  // Sync reference shots visibility with SurveyLayer (gated by showShots)
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setShowReferenceShots(getEffectiveShowReferenceShots(showShots, showReferenceShots));
  }, [showReferenceShots, showShots, mapLoaded]);

  // Sync jump shots visibility (#27): jumps hide independently of references.
  useEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setShowJumps(getEffectiveShowJumps(showShots, showJumps));
  }, [showJumps, showShots, mapLoaded]);

  // Sync anchored section IDs with SurveyLayer for fading.
  // useLayoutEffect (not useEffect) for the same reason as the selection sync
  // above: the controller retains its dim state across a project switch, and
  // section/feature ids collide across projects. A post-paint useEffect would
  // let the pre-paint geometry rebuild re-apply the previous project's dim to
  // the new project's colliding ids for one frame before this cleared it.
  useLayoutEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setAnchoredSections(anchoredSectionIds);
  }, [anchoredSectionIds, mapLoaded]);

  // Sync immovable feature IDs with SurveyLayer for disabled visual.
  // useLayoutEffect for the same cross-project-flash reason as above; immovable
  // ids are non-empty for the entire duration of a move tool, so this is the
  // more reachable trigger of the stale-dim flash.
  useLayoutEffect(() => {
    if (!surveyLayerRef.current) return;
    surveyLayerRef.current.setImmovableFeatures(
      immovableFeatureIds.stationIds,
      immovableFeatureIds.shotIds
    );
  }, [immovableFeatureIds, mapLoaded]);
}
