import type { Browser } from 'webdriverio';
import { evalInApp } from './page';
import { dispatchMapPointer } from './controls';

export interface MapFeatureClickRequest {
  featureType: 'shot' | 'station';
  sectionId: number;
  shotId?: number | undefined;
  stationIndex?: number | undefined;
  button?: 'left' | 'right' | undefined;
  /** Disable for tools whose click intentionally does not update selection. */
  verifySelection?: boolean | undefined;
}

export interface ResolvedMapFeatureClick {
  featureType: 'shot' | 'station';
  sectionId: number;
  shotId?: number;
  stationIndex?: number;
  lngLat: [number, number];
  canvasPoint: { x: number; y: number };
  canvasSize: { width: number; height: number };
  point: { x: number; y: number };
  clearancePx: number;
  interactionSequenceBefore: number;
  interactionSequenceAfter: number;
  interactionVerified: boolean;
  selectionVerified: boolean;
}

interface MapSelection {
  type?: string;
  sectionId?: number;
  shotId?: number;
  stationRef?: { section_id?: number; station_index?: number };
}

interface MapPostClickState {
  interactionSequence: number;
  selection: MapSelection | null;
}

function validateRequest(request: MapFeatureClickRequest): void {
  if (request.featureType !== 'shot' && request.featureType !== 'station') {
    throw new Error('map_feature_click needs a featureType of shot or station');
  }
  if (!Number.isInteger(request.sectionId)) throw new Error('map_feature_click needs an integer sectionId');
  if (request.featureType === 'shot' && !Number.isInteger(request.shotId)) {
    throw new Error('map_feature_click for a shot needs an integer shotId');
  }
  if (request.featureType === 'station' && !Number.isInteger(request.stationIndex)) {
    throw new Error('map_feature_click for a station needs an integer stationIndex');
  }
}

function selectionMatches(request: MapFeatureClickRequest, selection: MapSelection | null): boolean {
  if (request.featureType === 'shot') {
    return selection?.type === 'shot' && selection.sectionId === request.sectionId && selection.shotId === request.shotId;
  }
  return (
    selection?.type === 'station' &&
    selection.stationRef?.section_id === request.sectionId &&
    selection.stationRef?.station_index === request.stationIndex
  );
}

/** Resolve custom-WebGL geometry by domain identity, then perform a real pointer click. */
export async function mapFeatureClick(
  browser: Browser,
  request: MapFeatureClickRequest,
): Promise<ResolvedMapFeatureClick> {
  validateRequest(request);
  const resolved = await evalInApp<
    Omit<ResolvedMapFeatureClick, 'interactionSequenceAfter' | 'interactionVerified' | 'selectionVerified'>
  >(
    browser,
    `
      const request = args[0];
      const map = window.__stalagmight_map__;
      const readGeometry = window.__stalagmight_geometry__;
      const readInteraction = window.__stalagmight_map_interaction__;
      const setSemanticTarget = window.__stalagmight_set_map_target__;
      if (!map || typeof map.project !== 'function') throw new Error('map is unavailable');
      if (typeof readGeometry !== 'function') throw new Error('map geometry hooks are unavailable; start an E2E build');
      if (typeof readInteraction !== 'function' || typeof setSemanticTarget !== 'function') {
        throw new Error('semantic map controls are unavailable; start an E2E build');
      }
      const interactionSequenceBefore = readInteraction();
      const geometry = readGeometry();
      if (!geometry || !Array.isArray(geometry.stations) || !Array.isArray(geometry.shots)) {
        throw new Error('map geometry is unavailable');
      }
      const canvas = document.querySelector('[data-testid="map-canvas"]');
      if (!canvas) throw new Error('map canvas is unavailable');
      const rect = canvas.getBoundingClientRect();
      const project = (lngLat) => {
        const point = map.project(lngLat);
        return { x: Number(point.x), y: Number(point.y) };
      };
      const inside = (point) =>
        point.x >= 1 && point.y >= 1 && point.x <= rect.width - 1 && point.y <= rect.height - 1;
      const distanceToSegment = (point, from, to) => {
        const dx = to.x - from.x;
        const dy = to.y - from.y;
        const lengthSquared = dx * dx + dy * dy;
        if (lengthSquared === 0) return Math.hypot(point.x - from.x, point.y - from.y);
        const t = Math.max(0, Math.min(1, ((point.x - from.x) * dx + (point.y - from.y) * dy) / lengthSquared));
        return Math.hypot(point.x - (from.x + t * dx), point.y - (from.y + t * dy));
      };
      const finish = (lngLat, canvasPoint, clearancePx) => ({
        featureType: request.featureType,
        sectionId: request.sectionId,
        ...(request.featureType === 'shot' ? { shotId: request.shotId } : { stationIndex: request.stationIndex }),
        lngLat,
        canvasPoint: { x: Math.round(canvasPoint.x), y: Math.round(canvasPoint.y) },
        canvasSize: { width: rect.width, height: rect.height },
        point: { x: Math.round(rect.left + canvasPoint.x), y: Math.round(rect.top + canvasPoint.y) },
        clearancePx: Number.isFinite(clearancePx) ? Math.round(clearancePx * 100) / 100 : 0,
        interactionSequenceBefore,
      });

      if (request.featureType === 'station') {
        const station = geometry.stations.find(
          (item) => item.sectionId === request.sectionId && item.stationIndex === request.stationIndex,
        );
        if (!station) throw new Error('station feature not found or not visible');
        const point = project(station.lngLat);
        if (!inside(point)) throw new Error('station feature is outside the map viewport');
        const resolved = finish(station.lngLat, point, 0);
        if (!setSemanticTarget(request)) throw new Error('semantic map target could not be armed');
        return resolved;
      }

      const shot = geometry.shots.find(
        (item) => item.sectionId === request.sectionId && item.shotId === request.shotId,
      );
      if (!shot) throw new Error('shot feature not found or not visible');
      const from = project(shot.from);
      const to = project(shot.to);
      const otherSegments = geometry.shots
        .filter((item) => item.sectionId !== request.sectionId || item.shotId !== request.shotId)
        .map((item) => ({ from: project(item.from), to: project(item.to) }));
      const stationPoints = geometry.stations.map((item) => project(item.lngLat));
      const candidates = [0.125, 0.25, 0.5, 0.75, 0.875]
        .map((t) => ({
          t,
          point: { x: from.x + (to.x - from.x) * t, y: from.y + (to.y - from.y) * t },
        }))
        .filter((candidate) => inside(candidate.point))
        .map((candidate) => {
          let clearance = Infinity;
          for (const other of otherSegments) {
            clearance = Math.min(clearance, distanceToSegment(candidate.point, other.from, other.to));
          }
          for (const stationPoint of stationPoints) {
            clearance = Math.min(clearance, Math.hypot(candidate.point.x - stationPoint.x, candidate.point.y - stationPoint.y));
          }
          return { ...candidate, clearance };
        });
      if (!candidates.length) throw new Error('shot feature is outside the map viewport');
      candidates.sort((a, b) => b.clearance - a.clearance || Math.abs(a.t - 0.5) - Math.abs(b.t - 0.5));
      const best = candidates[0];
      const lngLat = [
        shot.from[0] + (shot.to[0] - shot.from[0]) * best.t,
        shot.from[1] + (shot.to[1] - shot.from[1]) * best.t,
      ];
      const resolved = finish(lngLat, best.point, best.clearance);
      if (!setSemanticTarget(request)) throw new Error('semantic map target could not be armed');
      return resolved;
    `,
    [request],
  );

  let target = 'unknown target';
  let postClick: MapPostClickState;
  try {
    target = await dispatchMapPointer(browser, resolved.point, request.button);
    postClick = await evalInApp<MapPostClickState>(
      browser,
      `
        const readInteraction = window.__stalagmight_map_interaction__;
        if (typeof readInteraction !== 'function') throw new Error('semantic map controls are unavailable');
        return {
          interactionSequence: readInteraction(),
          selection: window.__E2E_TEST__?.uiStore?.getState().selection ?? null,
        };
      `,
    );
  } finally {
    await evalInApp(
      browser,
      `
        const clearSemanticTarget = window.__stalagmight_clear_map_target__;
        if (typeof clearSemanticTarget === 'function') clearSemanticTarget();
      `,
    );
  }
  if (postClick.interactionSequence <= resolved.interactionSequenceBefore) {
    // The target is the page's own description of the element (its test id and
    // classes), so it goes on a later line, which the runners withhold.
    throw new Error(`map_feature_click did not reach a semantic map interaction handler\n  via: ${target}`);
  }

  const shouldVerify = request.verifySelection ?? true;
  if (shouldVerify) {
    if (!selectionMatches(request, postClick.selection)) {
      // The selection is the app's store value, so it goes on a later line; the
      // request's type and ids were checked by `validateRequest`.
      throw new Error(
        `map_feature_click selected something else instead of ${request.featureType} ` +
          `${request.sectionId}:${request.featureType === 'shot' ? request.shotId : request.stationIndex}\n` +
          `  selected: ${JSON.stringify(postClick.selection)}`,
      );
    }
  }

  return {
    ...resolved,
    interactionSequenceAfter: postClick.interactionSequence,
    interactionVerified: true,
    selectionVerified: shouldVerify,
  };
}
