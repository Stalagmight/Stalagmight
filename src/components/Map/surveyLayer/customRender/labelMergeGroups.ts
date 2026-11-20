/**
 * One station label per merge group (SPEC 4.2, Labels).
 *
 * Merged stations are one physical point, so the Station IDs / Station names
 * label draws ONE label there, for the group's representative: a station that
 * is not its line's first station if any (a branch's start is merged onto the
 * main line; the main line's station is the junction's name), then one that
 * is not its line's last station, ties broken by the line's order in the
 * project and then by station index. A station that is not merged is its own
 * representative. The image export picks by the same rule
 * (`point_representatives` in `image_labels.rs`); keep the two in step.
 *
 * A merge group is the slots that share one Station ID: on the station graph a
 * junction is ONE station, so every slot of it carries the same `stationIds`
 * entry (plan 9.3), as well as one name. A slot on a line the SoA leaves out
 * (a hidden Section) is simply not in the SoA, so the visible part of a group
 * still gets its label.
 *
 * PURE: no DOM, no GL.
 */
import type { SurveyData } from '../../../../types/survey-data';

/** Where each station's label goes when stations are merged. */
export interface MergedStationLabels {
  /** Per global station: the global index of its group's representative (the
   *  station itself when it is not merged, or represents its group). */
  representative: Int32Array;
  /** Per representative of a merged group (two or more members in the SoA):
   *  every member's global index, the representative included. */
  members: Map<number, number[]>;
}

/**
 * The junctions of one SoA: the global indices of the slots of every Station
 * ID two or more slots carry, slot order. Built once per `stationIds` array
 * (the SoA is rebuilt, never written in place, as `stationIdentity` relies on
 * too), so a label rebuild on pan or zoom does no per-station work here and a
 * survey with no junction costs nothing (H-frontend-wire-10).
 */
const junctionCache = new WeakMap<Uint32Array, number[][]>();

function junctionsOf(data: SurveyData): number[][] {
  const cached = junctionCache.get(data.stationIds);
  if (cached) return cached;
  // The first slot of each id; a group array only for an id seen twice.
  const firstSlot = new Map<number, number>();
  const byId = new Map<number, number[]>();
  for (let g = 0; g < data.stationCount; g++) {
    const id = data.stationIds[g];
    if (id === undefined) continue;
    const first = firstSlot.get(id);
    if (first === undefined) {
      firstSlot.set(id, g);
      continue;
    }
    const slots = byId.get(id);
    if (slots) slots.push(g);
    else byId.set(id, [first, g]);
  }
  const junctions = [...byId.values()];
  junctionCache.set(data.stationIds, junctions);
  return junctions;
}

/**
 * The representative of every merge group. `lineFirst` / `lineLast` give each
 * line's first and last station index in the SoA (the FIRST_IN_LINE predicate
 * and its counterpart); `hasLabel(g)` says whether a station has text to draw
 * (a station whose name shows as nothing cannot represent its group, as in the
 * export). `null` when no station of the SoA is merged.
 */
export function mergedStationLabels(
  data: SurveyData,
  lineFirst: ReadonlyMap<number, number>,
  lineLast: ReadonlyMap<number, number>,
  hasLabel: (g: number) => boolean,
): MergedStationLabels | null {
  const junctions = junctionsOf(data);
  if (junctions.length === 0) return null;
  const n = data.stationCount;
  const lineOrder = new Map<number, number>();
  data.lines.forEach((line, order) => lineOrder.set(line.id, order));
  const representative = new Int32Array(n);
  for (let g = 0; g < n; g++) representative[g] = g;
  const members = new Map<number, number[]>();
  // Lower is better: not first, then not last, then line order, then index.
  const rank = (g: number): [number, number, number, number] => {
    const line = data.stationLineIds[g]!;
    const index = data.stationIndices[g]!;
    return [
      index === lineFirst.get(line) ? 1 : 0,
      index === lineLast.get(line) ? 1 : 0,
      lineOrder.get(line) ?? Number.MAX_SAFE_INTEGER,
      index,
    ];
  };
  const better = (a: number, b: number): boolean => {
    const ra = rank(a);
    const rb = rank(b);
    for (let k = 0; k < ra.length; k++) {
      if (ra[k] !== rb[k]) return ra[k]! < rb[k]!;
    }
    return false;
  };
  for (const group of junctions) {
    let best = -1;
    for (const m of group) {
      if (hasLabel(m) && (best < 0 || better(m, best))) best = m;
    }
    if (best < 0) continue;
    for (const m of group) representative[m] = best;
    members.set(best, group);
  }
  return { representative, members };
}
