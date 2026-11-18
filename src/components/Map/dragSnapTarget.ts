/**
 * Snap-to-station for the map's move drags (station and segment). While a
 * drag's origin station passes near another station, the preview snaps onto
 * it and the drop offers a merge or a loop closure with it.
 *
 * Only stations the map DRAWS may be snapped to: the lines of hidden Sections
 * (#327, `mapHiddenLines`) are left out, so a drop never offers a merge or a
 * closure with a station the user cannot see.
 */
import type { Coordinates, StationRef } from '../../types/project';

export interface ScreenPoint {
  x: number;
  y: number;
}

interface SnapCandidate {
  station_ref: StationRef;
  coordinates: Coordinates;
}

/**
 * The station nearest `point` (screen pixels) within `threshold`, or null.
 * Skips the stations of `hiddenLines` and those `skip` names (the dragged
 * feature's own stations). `toScreen` projects a station onto the screen.
 */
export function nearestSnapStation<S extends SnapCandidate>(
  stations: Iterable<S>,
  toScreen: (station: S) => ScreenPoint,
  point: ScreenPoint,
  threshold: number,
  hiddenLines: ReadonlySet<number>,
  skip: (station: S) => boolean,
): StationRef | null {
  let nearest: StationRef | null = null;
  let nearestDist = Infinity;
  for (const st of stations) {
    if (hiddenLines.has(st.station_ref.line_id) || skip(st)) continue;
    const sp = toScreen(st);
    const dx = sp.x - point.x;
    const dy = sp.y - point.y;
    const dist = Math.sqrt(dx * dx + dy * dy);
    if (dist < threshold && dist < nearestDist) {
      nearestDist = dist;
      nearest = st.station_ref;
    }
  }
  return nearest;
}

/**
 * The station a held snap target names, if the map still draws it: a target
 * whose line became hidden (its Section was hidden mid-drag, or the selection
 * that revealed it moved) is no target any more.
 */
export function heldSnapStation<S extends SnapCandidate>(
  stations: readonly S[],
  target: StationRef,
  hiddenLines: ReadonlySet<number>,
): S | undefined {
  if (hiddenLines.has(target.line_id)) return undefined;
  return stations.find(
    (s) => s.station_ref.line_id === target.line_id && s.station_ref.station_index === target.station_index,
  );
}
