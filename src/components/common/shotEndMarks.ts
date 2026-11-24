import type { IdentityLabels } from '../../utils/stationIdentity';

/**
 * A shot pill's two ends: each bare name with its home-line dot and loop
 * mark, and the line each end's station belongs to when it is not the
 * shot's line (RS-OWNERSHIP: the half of the pill holding that end takes
 * that line's colour).
 */
export interface ShotEndMarks {
  from: string;
  to: string;
  fromHomeLineId?: number | null | undefined;
  toHomeLineId?: number | null | undefined;
  fromLoopEnd?: boolean | undefined;
  toLoopEnd?: boolean | undefined;
  /** The line the from end's station belongs to, when not the shot's (its half's colour). */
  fromOwnerLineId?: number | null | undefined;
  /** The line the to end's station belongs to, when not the shot's (its half's colour). */
  toOwnerLineId?: number | null | undefined;
}

/**
 * A shot pill's two ends with their marks (RS 6), for the shot at 0-based
 * position `shotIndex` of line `lineId`: an end on a foreign slot (it shows
 * another line's station) carries that station's home-line dot, and a loop
 * end the loop mark; an end whose station belongs to another line (a foreign
 * slot, or a loop end whose kept end lives elsewhere) colours its half of
 * the pill by that line (RS-OWNERSHIP). The names are bare, the marks and
 * colours saying what plain text's qualifier says (`12 (Main) → 13`). Null
 * when neither end is marked, so the pill shows the shot as one label.
 */
export function shotEndMarks(
  labels: Pick<IdentityLabels, 'stationPill' | 'homeMark' | 'shown' | 'ownerLine'>,
  lineId: number,
  shotIndex: number,
): Required<ShotEndMarks> | null {
  const from = { line_id: lineId, station_index: shotIndex };
  const to = { line_id: lineId, station_index: shotIndex + 1 };
  const fromHomeLineId = labels.homeMark(from);
  const toHomeLineId = labels.homeMark(to);
  const fromLoopEnd = labels.shown(from)?.loop_end === true;
  const toLoopEnd = labels.shown(to)?.loop_end === true;
  const fromOwnerLineId = labels.ownerLine(from);
  const toOwnerLineId = labels.ownerLine(to);
  if (
    fromHomeLineId === null &&
    toHomeLineId === null &&
    !fromLoopEnd &&
    !toLoopEnd &&
    fromOwnerLineId === null &&
    toOwnerLineId === null
  ) {
    return null;
  }
  return {
    from: labels.stationPill(from),
    to: labels.stationPill(to),
    fromHomeLineId,
    toHomeLineId,
    fromLoopEnd,
    toLoopEnd,
    fromOwnerLineId,
    toOwnerLineId,
  };
}
