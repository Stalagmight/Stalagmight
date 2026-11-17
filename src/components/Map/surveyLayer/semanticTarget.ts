import { distanceToLineSegment, projectParamOnSegment } from './hitTest';

export type SemanticMapTarget =
  | { featureType: 'station'; lineId: number; stationIndex: number }
  | { featureType: 'shot'; lineId: number; shotId: number };

export interface SemanticMapGeometry {
  stations: Array<{ lineId: number; stationIndex: number; lngLat: [number, number] }>;
  shots: Array<{ lineId: number; shotId: number; from: [number, number]; to: [number, number] }>;
}

export type ResolvedSemanticMapTarget =
  | { type: 'station'; lineId: number; stationIndex: number }
  | { type: 'shot'; lineId: number; shotId: number; alongT: number };

export function resolveSemanticMapTarget(
  target: SemanticMapTarget,
  geometry: SemanticMapGeometry,
  project: (lngLat: [number, number]) => { x: number; y: number },
  point: { x: number; y: number },
  stationReach: number,
  shotReach: number,
): ResolvedSemanticMapTarget | null {
  if (target.featureType === 'station') {
    const station = geometry.stations.find(
      (item) => item.lineId === target.lineId && item.stationIndex === target.stationIndex,
    );
    if (!station) return null;
    const projected = project(station.lngLat);
    if (Math.hypot(projected.x - point.x, projected.y - point.y) > stationReach) return null;
    return { type: 'station', lineId: target.lineId, stationIndex: target.stationIndex };
  }

  const shot = geometry.shots.find(
    (item) => item.lineId === target.lineId && item.shotId === target.shotId,
  );
  if (!shot) return null;
  const from = project(shot.from);
  const to = project(shot.to);
  const distance = distanceToLineSegment(point.x, point.y, from.x, from.y, to.x, to.y);
  if (distance > shotReach) return null;
  return {
    type: 'shot',
    lineId: target.lineId,
    shotId: target.shotId,
    alongT: projectParamOnSegment(point.x, point.y, from.x, from.y, to.x, to.y),
  };
}
