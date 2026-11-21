/**
 * A `SurveyData` without some of its lines: what the map draws, hit-tests and
 * labels while Sections are hidden (#327).
 *
 * The map's whole pipeline (custom geometry and label layers, the hit index,
 * the loop-closure overlay, the measure tint) reads ONE `SurveyData` per
 * flush, so handing it a compacted copy hides a line everywhere at once, and
 * every global index those consumers trade stays consistent with the copy.
 * Ids (line ids, shot ids, station refs) are untouched, so a pick on the map
 * still names the real line, shot or station.
 *
 * Cross-line references in the sparse maps (`stationMergedWith`,
 * `stationClosuresWith`) keep pointing at stations of hidden lines: they are
 * refs, not indices, and a consumer that resolves one simply finds nothing.
 */
import type { SurveyData } from '../types/survey-data';

/** Every `SurveyData` field this filter rebuilds, by how it is indexed. Kept
 *  explicit (not discovered) so a new field fails the coverage test in
 *  `surveyDataLineFilter.test.ts` instead of being copied at the wrong length. */
export const SHOT_ARRAY_FIELDS = [
  'shotIds', 'shotLineIds', 'shotAzimuthIn', 'shotAzimuthOut', 'shotLength',
  'shotDepthIn', 'shotDepthOut', 'shotPitchIn', 'shotPitchOut', 'shotLeft',
  'shotRight', 'shotUp', 'shotDown', 'shotTemperature', 'shotFlags', 'shotMarkerIndex',
] as const satisfies readonly (keyof SurveyData)[];
export const STATION_ARRAY_FIELDS = [
  'stationLineIds', 'stationIndices', 'stationFlags', 'stationIds',
] as const satisfies readonly (keyof SurveyData)[];
export const STATION_MAP_FIELDS = [
  'stationGps', 'stationMergedWith', 'stationClosuresWith', 'stationComments', 'stationNames',
  'stationAliases',
] as const satisfies readonly (keyof SurveyData)[];
/** The rest, handled one by one below. */
export const OTHER_FIELDS = [
  'lines', 'shotCount', 'shotComments', 'stationCount', 'stationCoords',
] as const satisfies readonly (keyof SurveyData)[];

type TypedArray = Uint32Array | Int32Array | Float32Array | Float64Array | Uint8Array;

/** `src` with only the elements whose (stride-1) index `keep` lists. */
function pick<T extends TypedArray>(src: T, keep: Uint32Array): T {
  const out = new (src.constructor as new (n: number) => T)(keep.length);
  for (let i = 0; i < keep.length; i++) out[i] = src[keep[i]!]!;
  return out;
}

function remapKeys<V>(src: Map<number, V>, newIndex: Int32Array): Map<number, V> {
  const out = new Map<number, V>();
  for (const [g, v] of src) {
    const n = newIndex[g];
    if (n !== undefined && n >= 0) out.set(n, v);
  }
  return out;
}

/** `data` without the lines in `hidden`; `data` itself when none of its
 *  lines is hidden (so an unchanged map keeps every cheap update path). */
export function filterSurveyDataLines(data: SurveyData, hidden: ReadonlySet<number>): SurveyData {
  if (hidden.size === 0 || !data.lines.some((l) => hidden.has(l.id))) return data;

  // Shots: each kept line's contiguous run, in line order.
  const keptShots: number[] = [];
  const lines = data.lines.flatMap((meta) => {
    if (hidden.has(meta.id)) return [];
    const shotOffset = keptShots.length;
    for (let g = meta.shotOffset; g < meta.shotOffset + meta.shotCount; g++) keptShots.push(g);
    return [{ ...meta, shotOffset }];
  });
  const shotKeep = Uint32Array.from(keptShots);
  const shotNew = new Int32Array(data.shotCount).fill(-1);
  shotKeep.forEach((g, i) => { shotNew[g] = i; });

  // Stations: by owning line, wherever they sit in the arrays.
  const keptStations: number[] = [];
  for (let g = 0; g < data.stationCount; g++) {
    if (!hidden.has(data.stationLineIds[g]!)) keptStations.push(g);
  }
  const stationKeep = Uint32Array.from(keptStations);
  const stationNew = new Int32Array(data.stationCount).fill(-1);
  stationKeep.forEach((g, i) => { stationNew[g] = i; });
  const stationCoords = new Float64Array(stationKeep.length * 3);
  stationKeep.forEach((g, i) => stationCoords.set(data.stationCoords.subarray(g * 3, g * 3 + 3), i * 3));

  const out: SurveyData = {
    ...data,
    lines,
    shotCount: shotKeep.length,
    shotComments: remapKeys(data.shotComments, shotNew),
    stationCount: stationKeep.length,
    stationCoords,
  };
  const writable = out as unknown as Record<string, unknown>;
  for (const f of SHOT_ARRAY_FIELDS) writable[f] = pick(data[f], shotKeep);
  for (const f of STATION_ARRAY_FIELDS) writable[f] = pick(data[f], stationKeep);
  for (const f of STATION_MAP_FIELDS) {
    writable[f] = remapKeys(data[f] as Map<number, unknown>, stationNew);
  }
  return out;
}

/** How many hidden sets each source keeps a filtered copy for. Selecting a
 *  line of a hidden Section reveals it (a new hidden set), and selecting
 *  something else hides it again: a few entries let the user flip between a
 *  handful of reveal states without re-filtering - and, since the copy is the
 *  SAME object, without the flush seeing new geometry it has to diff. Each
 *  entry is a full copy of the shown lines, hence small. */
export const FILTER_CACHE_KEYS_PER_SOURCE = 4;

/** Per source `SurveyData` (the measured and the adjusted copies each keep
 *  their own), its filtered copies by hidden-set key, least recently used
 *  first. Repeated flushes with the same hidden set hand the map the SAME
 *  object, so its incremental paths stay available. */
const cache = new WeakMap<SurveyData, Map<string, SurveyData>>();

/** {@link filterSurveyDataLines}, memoized on the source and the hidden set's
 *  `key` (any string that changes exactly when the set's contents do). */
export function filterSurveyDataLinesCached(
  data: SurveyData,
  hidden: ReadonlySet<number>,
  key: string,
): SurveyData {
  if (hidden.size === 0) return data;
  let entries = cache.get(data);
  const hit = entries?.get(key);
  if (entries && hit) {
    // Most recently used goes last.
    entries.delete(key);
    entries.set(key, hit);
    return hit;
  }
  const result = filterSurveyDataLines(data, hidden);
  if (!entries) {
    entries = new Map();
    cache.set(data, entries);
  }
  entries.set(key, result);
  if (entries.size > FILTER_CACHE_KEYS_PER_SOURCE) {
    const oldest = entries.keys().next().value;
    if (oldest !== undefined) entries.delete(oldest);
  }
  return result;
}
