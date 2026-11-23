import type { IdentityLabels } from '../../utils/stationIdentity';

/** A shot pill's two ends: each bare name with its home-line dot and loop mark. */
export interface ShotEndMarks {
  from: string;
  to: string;
  fromHomeLineId?: number | null | undefined;
  toHomeLineId?: number | null | undefined;
  fromLoopEnd?: boolean | undefined;
  toLoopEnd?: boolean | undefined;
}

/**
 * A shot pill's two ends with their marks (RS 6), for the shot at 0-based
 * position `shotIndex` of line `lineId`: an end on a foreign slot (it shows
 * another line's station) carries that station's home-line dot, and a loop
 * end the loop mark, as a station pill does; the names are bare, the marks
 * saying what plain text's qualifier says (`12 (Main) → 13`). Null when
 * neither end is marked, so the pill shows the shot as one label.
 */
export function shotEndMarks(
  labels: Pick<IdentityLabels, 'stationPill' | 'homeMark' | 'shown'>,
  lineId: number,
  shotIndex: number,
): Required<ShotEndMarks> | null {
  const from = { line_id: lineId, station_index: shotIndex };
  const to = { line_id: lineId, station_index: shotIndex + 1 };
  const fromHomeLineId = labels.homeMark(from);
  const toHomeLineId = labels.homeMark(to);
  const fromLoopEnd = labels.shown(from)?.loop_end === true;
  const toLoopEnd = labels.shown(to)?.loop_end === true;
  if (fromHomeLineId === null && toHomeLineId === null && !fromLoopEnd && !toLoopEnd) return null;
  return {
    from: labels.stationPill(from),
    to: labels.stationPill(to),
    fromHomeLineId,
    toHomeLineId,
    fromLoopEnd,
    toLoopEnd,
  };
}
