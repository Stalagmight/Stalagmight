/**
 * What "the whole survey" means when the map frames it - Fit to View, the fit
 * after a load or an import, the initial centre - while Sections are hidden
 * (#327): every station except those of the hidden lines, so the view frames
 * what the map draws. When that leaves nothing (every line hidden), every
 * station: the frame never collapses onto nothing.
 */
import type { SurveyData } from '../types/survey-data';
import { getBoundsFromStationCoords } from './coordinates';

export type FrameBounds = ReturnType<typeof getBoundsFromStationCoords>;

/** The bounds of `data`'s stations, without those of the `hidden` lines when
 *  at least one station of another line is left. */
export function surveyFrameBounds(data: SurveyData, hidden: ReadonlySet<number>): FrameBounds {
  const all = getBoundsFromStationCoords(data.stationCoords);
  if (hidden.size === 0) return all;
  const coords = data.stationCoords;
  const lineIds = data.stationLineIds;
  let minX = Infinity;
  let maxX = -Infinity;
  let minY = Infinity;
  let maxY = -Infinity;
  let minZ = Infinity;
  let maxZ = -Infinity;
  let shown = 0;
  for (let i = 0; i < data.stationCount; i++) {
    if (hidden.has(lineIds[i]!)) continue;
    const x = coords[i * 3]!;
    const y = coords[i * 3 + 1]!;
    const z = coords[i * 3 + 2]!;
    minX = Math.min(minX, x);
    maxX = Math.max(maxX, x);
    minY = Math.min(minY, y);
    maxY = Math.max(maxY, y);
    minZ = Math.min(minZ, z);
    maxZ = Math.max(maxZ, z);
    shown++;
  }
  return shown === 0 ? all : { minX, maxX, minY, maxY, minZ, maxZ };
}
