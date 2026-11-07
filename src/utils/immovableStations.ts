import { type SurveyData, STATION_FLAG_IS_ZERO_DATUM } from '../types/survey-data';

/**
 * The set of immovable station keys (`"sectionId:stationIndex"`) for the move
 * tools: a station is immovable if it is zero-datum, or transitively reachable
 * from one via `merged_with` and ALIGNED `closures_with` links.
 *
 * Reads the SoA `SurveyData` directly (section ids / indices / flags + the sparse
 * merged/closure maps) rather than a materialized `Station[]`. The move-tool memo
 * used to `stations.map(...)` the lazy proxy, which rebuilt AND retained the whole
 * ~89k-`Station` object graph while a move tool was active - undoing the binary
 * loader's lazy memory win (#customRender, Codex round 5). Only a `Map<string,
 * number>` of keys (plus the result set) is allocated here.
 *
 * Behavior is identical to the previous `Station[]` BFS; the unit test pins that
 * with the original algorithm as an oracle.
 */
export function computeImmovableStationKeys(
  data: SurveyData,
  alignedClosureIds: ReadonlySet<number>,
): Set<string> {
  const keyOf = (sectionId: number, stationIndex: number): string => `${sectionId}:${stationIndex}`;

  // key -> global station index (numbers only; far lighter than a Station map).
  const keyToGlobal = new Map<string, number>();
  for (let g = 0; g < data.stationCount; g++) {
    keyToGlobal.set(keyOf(data.stationSectionIds[g]!, data.stationIndices[g]!), g);
  }

  const immovable = new Set<string>();
  const queue: string[] = [];
  for (let g = 0; g < data.stationCount; g++) {
    if ((data.stationFlags[g]! & STATION_FLAG_IS_ZERO_DATUM) !== 0) {
      queue.push(keyOf(data.stationSectionIds[g]!, data.stationIndices[g]!));
    }
  }

  while (queue.length > 0) {
    const key = queue.pop()!;
    if (immovable.has(key)) continue;
    immovable.add(key);
    const g = keyToGlobal.get(key);
    if (g === undefined) continue;
    for (const ref of data.stationMergedWith.get(g) ?? []) {
      queue.push(keyOf(ref.section_id, ref.station_index));
    }
    for (const cr of data.stationClosuresWith.get(g) ?? []) {
      if (!alignedClosureIds.has(cr.closure_id)) continue;
      queue.push(keyOf(cr.station_ref.section_id, cr.station_ref.station_index));
    }
  }
  return immovable;
}
