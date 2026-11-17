import { lineIdFromKey } from '../../../utils/ids';

/**
 * The line the map draws above every other one and gives the click where
 * they overlap: the selected line, or the line of the selected station or
 * shot (`SurveyCustomController.setRaisedLine`, `ResolveOptions.raisedLineId`).
 * The keys are SurveyLayer's selection ids: a line id, or
 * `"<lineId>:<stationIndex>"` / `"<lineId>:<shotId>"`. The app selects one
 * kind at a time; null when nothing is selected or the key is malformed.
 */
export function raisedLineOf(
  lineKey: string | null,
  stationKey: string | null,
  shotKey: string | null,
): number | null {
  const key = lineKey ?? stationKey ?? shotKey;
  const id = key === null ? NaN : lineIdFromKey(key);
  return Number.isNaN(id) ? null : id;
}
