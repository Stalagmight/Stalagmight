import type { SurveyData } from '../../../types/survey-data';
import { shotKindFromFlags } from '../../../types/survey-data';
import { SHOT_ALPHA_BY_KIND, SHOT_ALPHA_DIMMED, GAS_DIM_ALPHA } from './customRender/shaders';
import { resolveShotDisplayColor, resolveStationDisplayColor, type DisplayColorContext } from './displayColor';
import { SurveyGeometryIndex, isKindVisible, type VisibleKinds } from './geometryIndex';
import { offsetOverlayPoint, offsetOverlayShot, type MovingEnd, type Projector } from './dragPreviewSource';
import type { DistanceFeatureState } from '../../../utils/distanceGradient';
import { GAS_FAR, type GasReachFeatureState } from '../../../utils/gasReach';
import type { MapThemeColors } from '../../../utils/themeColors';

/** State the interact-overlay feature builders read off SurveyLayer (#157 split). */
export interface InteractOverlayDeps {
  geoIndex: SurveyGeometryIndex | null;
  cachedSurveyData: SurveyData | null;
  visibleKinds: VisibleKinds;
  parseFeatureId: (id: string | null) => number | null;
  measuredPathIds: Set<number> | null;
  measuredPathStationIds: Set<number> | null;
  measureTintColor: string;
  isLineAnchored: (lineId: number) => boolean;
  isPlanIdleActive: boolean;
  displayColorContext: DisplayColorContext;
  themeColors: MapThemeColors;
  distanceMode: boolean;
  distanceState: DistanceFeatureState | null;
  gasReachMode: boolean;
  gasReachState: GasReachFeatureState | null;
  dragOverlayOffset: { dx: number; dy: number } | null;
  dragMovingShots: Map<string, MovingEnd>;
  dragMovingStationKeys: Set<string>;
  dragProjector: () => Projector;
}

/**
 * One interact-overlay feature for a shot ("lineId:shotId"), drawn at
 * `width`. Kind `shotMeasured` (measure tint, round caps, above the tint
 * layer) when the shot is on the measured path, else `shot` (its exact bulk
 * color; the ramp expressions take over in distance/gas modes via the
 * carried dist/gasReach values).
 */
export function shotOverlayFeature(
  deps: InteractOverlayDeps,
  idStr: string,
  width: number,
): GeoJSON.Feature | null {
  if (!deps.geoIndex || !deps.cachedSurveyData) return null;
  const [secStr, shotStr] = idStr.split(':');
  const lineId = Number(secStr);
  const shotId = Number(shotStr);
  if (Number.isNaN(lineId) || Number.isNaN(shotId)) return null;
  const geo = deps.geoIndex.shotGeometry(lineId, shotId);
  if (!geo) return null;
  // A shot of a hidden kind must not surface via the overlay either (#27).
  if (geo.isReference && !isKindVisible(geo.kindFlags, deps.visibleKinds)) return null;

  const fid = deps.parseFeatureId(idStr);
  const measured = fid !== null && deps.measuredPathIds?.has(fid);
  const line = deps.geoIndex.line(lineId);
  // Plan-mode idle tint (#395 revision): a hovered/selected shot must not flash
  // its line color while there is no gas field yet - it paints the SAME flat
  // far color + dim alpha the bulk geometry does.
  const idleActive = deps.isPlanIdleActive;
  const color = measured
    ? deps.measureTintColor
    : idleActive
      ? GAS_FAR
      : resolveShotDisplayColor(
          deps.cachedSurveyData,
          line?.color ?? deps.themeColors.stationDefault,
          geo.globalIdx,
          deps.displayColorContext,
        );
  // Per-kind, from the same table the shader is generated from (#27):
  // `geo.isReference` is "any virtual kind", so a flat 0.5 here drew a
  // hovered/selected reference tie brighter than its own base geometry.
  const overlayKind = shotKindFromFlags(
    deps.cachedSurveyData?.shotFlags[geo.globalIdx] ?? 0,
  );
  // Gas-dim must never brighten a fainter per-kind base (a reference at 0.28
  // < GAS_DIM_ALPHA 0.45), matching the shader's min() clamp (#395 review).
  const opacity = deps.isLineAnchored(lineId)
    ? SHOT_ALPHA_DIMMED
    : idleActive
      ? Math.min(GAS_DIM_ALPHA, SHOT_ALPHA_BY_KIND[overlayKind])
      : SHOT_ALPHA_BY_KIND[overlayKind];
  const properties: Record<string, unknown> = {
    kind: measured ? 'shotMeasured' : 'shot',
    color,
    width,
    opacity,
  };
  // Overlay modes read these via the same ramp expressions as the bulk
  // layers (GPU-identical colors). -1 = unreachable (renders grey).
  if (deps.distanceMode && fid !== null) {
    properties.dist = deps.distanceState?.shots.get(fid) ?? -1;
  }
  if (deps.gasReachMode && fid !== null) {
    properties.gasReach = deps.gasReachState?.shots.get(fid) ?? -1;
  }
  // Follow an active drag: stretch this shot's moving endpoint(s) by the offset
  // so a selected/hovered highlight tracks the preview (#C).
  const off = deps.dragOverlayOffset;
  const { from, to } = off
    ? offsetOverlayShot(geo.from, geo.to, deps.dragMovingShots.get(idStr), deps.dragProjector(), off.dx, off.dy)
    : { from: geo.from, to: geo.to };
  return {
    type: 'Feature',
    geometry: { type: 'LineString', coordinates: [from, to] },
    properties,
  };
}

/**
 * One interact-overlay feature for a station ("lineId:stationIndex") in
 * hover or selected style - the same radius/stroke the old feature-state
 * paint cases produced.
 */
export function stationOverlayFeature(
  deps: InteractOverlayDeps,
  idStr: string,
  style: 'hover' | 'selected',
  radius: number,
): GeoJSON.Feature | null {
  if (!deps.geoIndex || !deps.cachedSurveyData) return null;
  const [secStr, idxStr] = idStr.split(':');
  const ref = { line_id: Number(secStr), station_index: Number(idxStr) };
  if (Number.isNaN(ref.line_id) || Number.isNaN(ref.station_index)) return null;
  const coords = deps.geoIndex.stationCoords(ref);
  const g = deps.geoIndex.stationGlobalIndex(ref);
  if (!coords || g === null) return null;
  // A station hidden because every kind touching it is hidden must not
  // surface via a highlight (matches the circle shader's both-off test).
  if (!deps.geoIndex.isStationDrawn(ref, deps.visibleKinds))
    return null;

  const fid = deps.parseFeatureId(idStr);
  const line = deps.geoIndex.line(ref.line_id);
  const ctx = deps.displayColorContext;
  const measured = fid !== null && deps.measuredPathStationIds?.has(fid);
  // Plan-mode idle tint (#395 revision): see shotOverlayFeature - same flat far
  // color + dim alpha instead of line color while there is no gas field yet.
  const idleActive = deps.isPlanIdleActive;
  const color = measured
    ? deps.measureTintColor
    : idleActive
      ? GAS_FAR
      : resolveStationDisplayColor(
          deps.cachedSurveyData,
          line?.color ?? deps.themeColors.stationDefault,
          g,
          ctx,
        );
  const properties: Record<string, unknown> = {
    kind: 'station',
    color,
    radius,
    strokeWidth: style === 'hover' ? 4 : 2.5,
    strokeColor:
      style === 'hover' ? deps.themeColors.hoverHighlight : deps.themeColors.selectHighlight,
    opacity: deps.isLineAnchored(ref.line_id) ? 0.25 : idleActive ? GAS_DIM_ALPHA : 1,
  };
  if (deps.distanceMode && fid !== null) {
    properties.dist = deps.distanceState?.stations.get(fid) ?? -1;
  }
  if (deps.gasReachMode && fid !== null) {
    properties.gasReach = deps.gasReachState?.stations.get(fid) ?? -1;
  }
  // Follow an active drag: offset this station's highlight if it is moving (#C).
  const off = deps.dragOverlayOffset;
  const point = off
    ? offsetOverlayPoint(coords, deps.dragMovingStationKeys.has(idStr), deps.dragProjector(), off.dx, off.dy)
    : coords;
  return {
    type: 'Feature',
    geometry: { type: 'Point', coordinates: point },
    properties,
  };
}
