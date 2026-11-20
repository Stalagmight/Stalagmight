import { type SurveyData, STATION_FLAG_IS_ZERO_DATUM } from '../types/survey-data';
import { alignedClosureTargets } from './closureTies';

/**
 * The set of immovable station keys (`"lineId:stationIndex"`) for the move
 * tools: a station is immovable if it is zero-datum, or transitively reachable
 * from one via `merged_with` and the ALIGNED closures that tie it. A
 * closure-chain entry of `closures_with` names a closure the station is not an
 * end of, so it is not followed (plan D15, `closureTies.ts`).
 *
 * Reads the SoA `SurveyData` directly (line ids / indices / flags + the sparse
 * merged/closure maps) rather than a materialized `Station[]`. The move-tool memo
 * used to `stations.map(...)` the lazy proxy, which rebuilt AND retained the whole
 * ~89k-`Station` object graph while a move tool was active - undoing the binary
 * loader's lazy memory win (#customRender, Codex round 5). Only a `Map<string,
 * number>` of keys (plus the result set) is allocated here.
 *
 * The unit test pins the SoA walk against a `Station[]` walk of the same rule
 * (written by Station ID) as an oracle.
 */
export function computeImmovableStationKeys(
  data: SurveyData,
  alignedClosureIds: ReadonlySet<bigint>,
): Set<string> {
  const keyOf = (lineId: number, stationIndex: number): string => `${lineId}:${stationIndex}`;

  // key -> global station index (numbers only; far lighter than a Station map).
  const keyToGlobal = new Map<string, number>();
  for (let g = 0; g < data.stationCount; g++) {
    keyToGlobal.set(keyOf(data.stationLineIds[g]!, data.stationIndices[g]!), g);
  }

  const immovable = new Set<string>();
  const queue: string[] = [];
  for (let g = 0; g < data.stationCount; g++) {
    if ((data.stationFlags[g]! & STATION_FLAG_IS_ZERO_DATUM) !== 0) {
      queue.push(keyOf(data.stationLineIds[g]!, data.stationIndices[g]!));
    }
  }

  while (queue.length > 0) {
    const key = queue.pop()!;
    if (immovable.has(key)) continue;
    immovable.add(key);
    const g = keyToGlobal.get(key);
    if (g === undefined) continue;
    for (const ref of data.stationMergedWith.get(g) ?? []) {
      queue.push(keyOf(ref.line_id, ref.station_index));
    }
    const holder = { line_id: data.stationLineIds[g]!, station_index: data.stationIndices[g]! };
    const tied = alignedClosureTargets(
      holder,
      data.stationMergedWith.get(g),
      data.stationClosuresWith.get(g),
      alignedClosureIds,
    );
    for (const ref of tied) {
      queue.push(keyOf(ref.line_id, ref.station_index));
    }
  }
  return immovable;
}
