import { sectionIdFromKey } from '../../../utils/ids';

/**
 * The section the map draws above every other one and gives the click where
 * they overlap: the selected section, or the section of the selected station or
 * shot (`SurveyCustomController.setRaisedSection`, `ResolveOptions.raisedSectionId`).
 * The keys are SurveyLayer's selection ids: a section id, or
 * `"<sectionId>:<stationIndex>"` / `"<sectionId>:<shotId>"`. The app selects one
 * kind at a time; null when nothing is selected or the key is malformed.
 */
export function raisedSectionOf(
  sectionKey: string | null,
  stationKey: string | null,
  shotKey: string | null,
): number | null {
  const key = sectionKey ?? stationKey ?? shotKey;
  const id = key === null ? NaN : sectionIdFromKey(key);
  return Number.isNaN(id) ? null : id;
}
