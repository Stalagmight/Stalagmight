import { invokeIpc } from './ipc';
import type { GpsCoordinates, StationRef } from '../types/project';
import type { ZeroDatumPreview } from '../types/ipc/ZeroDatumPreview';

/** Default horizontal accuracy of a zero datum's GPS, in metres: 0, a hard pin.
 *  Mirrors the backend's `DEFAULT_GPS_ACCURACY_M` (#741): a datum stored
 *  without one reads as this. */
export const DEFAULT_DATUM_ACCURACY_M = 0;

/**
 * What setting a zero datum with `coordinates` on `stationRef` would do, without
 * doing it (#741): the role it would take and, for a GPS fix, how far its GPS is
 * from where the survey puts the station. `null` when the station does not
 * exist. Read-only on the backend.
 */
export function previewZeroDatum(
  stationRef: StationRef,
  coordinates: GpsCoordinates
): Promise<ZeroDatumPreview | null> {
  return invokeIpc<ZeroDatumPreview | null>('preview_zero_datum', { stationRef, coordinates }, {
    purpose: 'preview what a zero datum would do',
    fields: { stationRef },
  });
}

const COMPASS_POINTS = ['N', 'NE', 'E', 'SE', 'S', 'SW', 'W', 'NW'] as const;

/** The nearest of the eight compass points for a bearing in degrees clockwise
 *  from north (any finite value; wrapped into [0, 360)). */
export function compassPoint(bearingDeg: number): string {
  const wrapped = ((bearingDeg % 360) + 360) % 360;
  return COMPASS_POINTS[Math.round(wrapped / 45) % COMPASS_POINTS.length]!;
}

/**
 * Strict parse of the accuracy field: a finite number of metres, 0 or more
 * (0 is a hard pin). `null` for anything else, including an empty field.
 */
export function parseDatumAccuracy(raw: string): number | null {
  const s = raw.trim();
  if (s === '') return null;
  const n = Number(s);
  return Number.isFinite(n) && n >= 0 ? n : null;
}
