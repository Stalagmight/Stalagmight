/**
 * Stations of hidden Sections that a station / shot drag must still move
 * (#327).
 *
 * `SurveyLayer.beginStationDragPreview` finds the stations that move with the
 * dragged one (merged, or tied by an aligned loop closure) in the data the MAP
 * holds - which leaves out hidden lines. A visible station merged with one on a
 * hidden line would then move alone and tear the merge apart. This walks the
 * same links through the WHOLE survey and adds what the map could not see. The
 * hidden stations get no preview (they are hidden), only the move.
 */
import type { StationRef } from '../../types/project';
import type { SurveyData } from '../../types/survey-data';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { selectActiveSurveyData } from '../../store/geometrySelectors';
import { mapHiddenLines } from '../../store/sectionVisibility';

const keyOf = (r: StationRef) => `${r.line_id}:${r.station_index}`;

/**
 * `seeds` plus every station reachable from them through `stationMergedWith`
 * and through closures in `alignedClosureIds`, over all of `data`. The seeds
 * keep their order and come first.
 */
export function expandColocated(
  seeds: readonly StationRef[],
  data: SurveyData,
  alignedClosureIds: ReadonlySet<bigint>,
): StationRef[] {
  const globalByKey = new Map<string, number>();
  for (let g = 0; g < data.stationCount; g++) {
    globalByKey.set(`${data.stationLineIds[g]}:${data.stationIndices[g]}`, g);
  }
  const out: StationRef[] = [...seeds];
  const seen = new Set(seeds.map(keyOf));
  const queue = [...seeds];
  while (queue.length > 0) {
    const g = globalByKey.get(keyOf(queue.pop()!));
    if (g === undefined) continue;
    const linked = [
      ...(data.stationMergedWith.get(g) ?? []),
      ...(data.stationClosuresWith.get(g) ?? [])
        .filter((c) => alignedClosureIds.has(c.closure_id))
        .map((c) => c.station_ref),
    ];
    for (const ref of linked) {
      const key = keyOf(ref);
      if (seen.has(key) || !globalByKey.has(key)) continue;
      seen.add(key);
      const copy = { line_id: ref.line_id, station_index: ref.station_index };
      out.push(copy);
      queue.push(copy);
    }
  }
  return out;
}

/** The drag's co-located stations, completed with any on hidden lines. A
 *  no-op (the same array) while no line is hidden. */
export function withHiddenColocated(colocated: StationRef[]): StationRef[] {
  if (colocated.length === 0) return colocated;
  const projectState = useProjectStore.getState();
  const ui = useUIStore.getState();
  if (mapHiddenLines(projectState.project?.lines, ui).ids.size === 0) return colocated;
  const data = selectActiveSurveyData(projectState, ui.geometryMode);
  if (!data) return colocated;
  const aligned = new Set(
    (projectState.project?.loop_closures ?? []).filter((c) => c.is_aligned).map((c) => c.id),
  );
  return expandColocated(colocated, data, aligned);
}
