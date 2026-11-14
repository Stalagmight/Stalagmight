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

/** Largest accuracy, in metres, the backend accepts. Mirrors
 *  `MAX_GPS_ACCURACY_M` in src-tauri/crates/models/src/station.rs, which
 *  refuses anything above it on save. */
export const MAX_DATUM_ACCURACY_M = 1000;

/**
 * The accuracy to send when saving a datum: `entered` when it differs from
 * what the datum reads as now, otherwise exactly what is stored (`undefined`
 * when nothing is, so the save sends none). The backend stores what it is sent
 * and an omitted accuracy as "none", so this keeps a re-save from writing an
 * explicit default the user never set, and keeps an explicit stored value
 * through a GPS-only edit.
 */
export function datumAccuracyToSend(entered: number, stored: number | undefined): number | undefined {
  return entered === (stored ?? DEFAULT_DATUM_ACCURACY_M) ? stored : entered;
}

/**
 * Strict parse of the accuracy field: a finite number of metres in
 * 0 ..= MAX_DATUM_ACCURACY_M (0 is a hard pin). `null` for anything else,
 * including an empty field.
 */
export function parseDatumAccuracy(raw: string): number | null {
  const s = raw.trim();
  if (s === '') return null;
  const n = Number(s);
  return Number.isFinite(n) && n >= 0 && n <= MAX_DATUM_ACCURACY_M ? n : null;
}
