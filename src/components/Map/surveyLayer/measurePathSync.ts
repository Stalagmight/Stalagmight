/**
 * Drives the dedicated custom measure-path layer (#26) from SurveyLayer's
 * measured id sets (#576 D3 split): resolves the measured shots / stations
 * through the SoA geometry index, honors the shots / stations toggles and the
 * hidden virtual kinds, and tessellates the tint. SurveyLayer passes its live
 * state in on every sync.
 */
import type { Map as MaplibreMap } from 'maplibre-gl';
import { FEATURE_ID_STRIDE } from '../../../utils/ids';
import { hasLiveStyle } from '../mapUtils';
import { AUX_IDS } from './auxiliaryLayers';
import type { SurveyGeometryIndex, VisibleKinds } from './geometryIndex';
import type { MeasurePathController } from './customRender/measurePathController';
import type { MeasurePathRenderData } from './customRender/surveyMeasurePathLayer';
import {
  buildMeasurePathBuffers,
  type MeasureSegment,
  type MeasurePoint,
} from './customRender/measurePathTessellate';
import { packCssColor } from './customRender/colorPack';

/** The SurveyLayer state a measure-path sync reads. */
export interface MeasurePathSyncInput {
  geoIndex: SurveyGeometryIndex | null;
  measuredPathIds: Set<number> | null;
  measuredPathStationIds: Set<number> | null;
  shotsVisible: boolean;
  stationsVisible: boolean;
  /** Which virtual kinds are drawn (#27). */
  visibleKinds: VisibleKinds;
  measureTintColor: string;
  /** Configured shot width / station radius in CSS px. */
  shotWidth: number;
  stationRadius: number;
}

/**
 * (Re)build the dedicated custom measure-path layer (#26, stage 2a) from the
 * current measured id sets and geometry index, or clear it when no path is
 * set. Called from setMeasuredPath and after each geometry rebuild (endApply-
 * Features re-applies the tint), mirroring applyMeasuredPathFeatureState so a
 * mid-measurement edit keeps the custom tint in sync with moved coords.
 *
 * The measured id sets are the encoded feature ids setMeasuredPath received
 * (`sectionId * FEATURE_ID_STRIDE + scopedId`); they are decoded back and
 * projected via the SoA geometry index (`shotGeometry` / `stationCoords`), the
 * same source the interact overlay uses. Reference-hidden features are skipped
 * to match the stock `measurePathFilter` / interact-overlay guards.
 *
 * z-order: inserted beneath `survey-interact-shots-measured` (the hover/
 * selection tint), which sits above the stock `survey-measure-path`. Both the
 * custom and stock tints therefore draw between the survey geometry and the
 * hover overlay in identical MEASURE_TINT - the pixels match this stage.
 */
export function syncMeasurePath(
  measurePath: MeasurePathController | null,
  map: MaplibreMap,
  input: MeasurePathSyncInput,
): void {
  if (!measurePath) return;
  // No style (map removed, or a lost WebGL context): nothing to draw into, and
  // clear() would read the dead style. A context restore replays this.
  if (!map || !hasLiveStyle(map)) return;
  if (!input.geoIndex) {
    measurePath.clear();
    return;
  }
  if (input.measuredPathIds === null && input.measuredPathStationIds === null) {
    measurePath.clear();
    return;
  }
  // The tint must not float over hidden geometry: measured shots follow the
  // shots toggle, measured stations follow the stations toggle (setShowShots /
  // setShowStations re-run this so a live toggle updates the path).
  const segments: MeasureSegment[] = [];
  if (input.measuredPathIds && input.shotsVisible) {
    // Resolve by RE-DERIVING each shot's canonical feature id rather than
    // decoding the fid: the scoped id folded into a shot fid is the GLOBAL shot
    // id (>= FEATURE_ID_STRIDE for section >= 1), so a floor(fid / STRIDE) split
    // mis-attributes every such shot and drops its tint (#26). Hidden reference
    // legs are excluded to match the interact overlay / stock filter.
    for (const { from, to } of input.geoIndex.measuredShotSegments(
      input.measuredPathIds,
      input.visibleKinds,
    )) {
      segments.push({ a: from, b: to });
    }
  }
  const points: MeasurePoint[] = [];
  if (input.measuredPathStationIds && input.stationsVisible) {
    for (const fid of input.measuredPathStationIds) {
      const sectionId = Math.floor(fid / FEATURE_ID_STRIDE);
      const stationIndex = fid - sectionId * FEATURE_ID_STRIDE;
      const ref = { section_id: sectionId, station_index: stationIndex };
      // Match the interact-overlay guard: a station hidden because every
      // kind touching it is hidden must not surface.
      if (!input.geoIndex.isStationDrawn(ref, input.visibleKinds))
        continue;
      const coords = input.geoIndex.stationCoords(ref);
      if (coords) points.push({ center: coords });
    }
  }
  if (segments.length === 0 && points.length === 0) {
    measurePath.clear();
    return;
  }
  measurePath.setPath(buildMeasurePathRenderData(segments, points, input), [
    // ABOVE the custom survey geometry (which anchors before survey-interact-shots)
    // so the measured shot lines tint OVER the base geometry rather than being
    // covered by it; still below the interact hover/selection overlay. Added
    // after the geometry layer, so `before survey-interact-shots` lands it just
    // above the geometry. Fallback keeps it placed if that layer is absent.
    AUX_IDS.interactShotsLayerId,
    AUX_IDS.interactShotsMeasuredLayerId,
  ]);
}

/** Tessellate the measured path + resolve its device-px sizes and tint color
 *  for the custom measure-path layer. */
function buildMeasurePathRenderData(
  segments: MeasureSegment[],
  points: MeasurePoint[],
  input: MeasurePathSyncInput,
): MeasurePathRenderData {
  const dpr = (typeof window !== 'undefined' && window.devicePixelRatio) || 1;
  const [r, g, b, a] = packCssColor(input.measureTintColor);
  const { anchorX, anchorY, line, circle } = buildMeasurePathBuffers(segments, points);
  return {
    anchorX,
    anchorY,
    line,
    circle,
    // Match the stock tint's line width (baseWidth = getShotWidth) and the
    // station disc radius (getStationRadius); half-width for the capsule.
    lineHalfWidthPx: (input.shotWidth / 2) * dpr,
    circleRadiusPx: input.stationRadius * dpr,
    aaPx: dpr,
    color: [r / 255, g / 255, b / 255, a / 255],
  };
}
