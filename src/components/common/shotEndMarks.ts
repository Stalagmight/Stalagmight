import { displayStationName } from '../../utils/formatters';
import type { IdentityLabels } from '../../utils/stationIdentity';

/**
 * A shot pill's two ends with their home-line dots (RS 6), for the shot at
 * 0-based position `shotIndex` of line `lineId`: an end on a foreign slot (it
 * shows another line's station) carries that station's home-line dot. Null
 * when neither end is foreign, so the pill shows the shot as one label.
 */
export function shotEndMarks(
  labels: Pick<IdentityLabels, 'stationName' | 'homeMark'>,
  lineId: number,
  shotIndex: number,
): { from: string; to: string; fromHomeLineId: number | null; toHomeLineId: number | null } | null {
  const from = { line_id: lineId, station_index: shotIndex };
  const to = { line_id: lineId, station_index: shotIndex + 1 };
  const fromHomeLineId = labels.homeMark(from);
  const toHomeLineId = labels.homeMark(to);
  if (fromHomeLineId === null && toHomeLineId === null) return null;
  return {
    from: displayStationName(labels.stationName(from)),
    to: displayStationName(labels.stationName(to)),
    fromHomeLineId,
    toHomeLineId,
  };
}
