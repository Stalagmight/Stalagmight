import type { ClosureRef } from '../types/ipc/ClosureRef';
import type { StationRef } from '../types/project';

/**
 * Which `closures_with` entries of a slot tie THAT slot's station, for the
 * walks that move or freeze stations together (immovable stations, the
 * Move Station drag set, its hidden-line completion). Plan D15.
 *
 * The wire (`survey/wire/stations.rs`) lists four kinds of entry on a slot H:
 * - **direct**: H is the closure end's own slot; `via_station` is null and
 *   `station_ref` is the far end;
 * - **junction-transitive**: H is another slot of the end station;
 *   `via_station` is the end's own slot, a slot of H's station;
 * - **far-transitive**: `station_ref` is another slot of the far station and
 *   `via_station` is the far end, a slot of the TARGET's station (H lists
 *   that far end too, under the same closure, as a direct or
 *   junction-transitive entry);
 * - **closure-chain**: a slot X with direct closures to Y and Z lets Y see Z
 *   "via X", under the id of the X-Z closure. `via_station` is X, a third
 *   station: neither H's nor the target's.
 *
 * Only the first three tie H to the target by that closure. A chain entry
 * carries a closure H's station is not an end of, so a walk that follows it
 * as a tie treats "unaligned X-Y, then aligned X-Z" as rigid: Move Station
 * on Y then also moves Z, off X, and breaks the aligned X-Z tie; with a
 * datum on Y, Z reads immovable while X does not.
 *
 * Decided from the holder's own entries and `merged_with` only, so the
 * answer is the same on the map's partial (hidden-lines) data, where the
 * via slot or the target may not be resolvable.
 */
export function isClosureChainEntry(
  holder: StationRef,
  mergedWith: readonly StationRef[] | undefined,
  closuresWith: readonly ClosureRef[],
  entry: ClosureRef,
): boolean {
  const via = entry.via_station;
  if (via === null) return false;
  if (isOwnStation(holder, mergedWith, via)) return false;
  // Far-transitive: `via` is a far end the holder reaches by this closure.
  return !closuresWith.some(
    (other) =>
      other.closure_id === entry.closure_id &&
      sameSlot(other.station_ref, via) &&
      (other.via_station === null || isOwnStation(holder, mergedWith, other.via_station)),
  );
}

/**
 * The slots `holder` is tied to by the closures in `aligned`: its
 * `closures_with` targets, less the closure-chain entries
 * ({@link isClosureChainEntry}).
 */
export function alignedClosureTargets(
  holder: StationRef,
  mergedWith: readonly StationRef[] | undefined,
  closuresWith: readonly ClosureRef[] | undefined,
  aligned: ReadonlySet<bigint>,
): StationRef[] {
  if (!closuresWith || closuresWith.length === 0) return [];
  const out: StationRef[] = [];
  for (const entry of closuresWith) {
    if (!aligned.has(entry.closure_id)) continue;
    if (isClosureChainEntry(holder, mergedWith, closuresWith, entry)) continue;
    out.push(entry.station_ref);
  }
  return out;
}

function sameSlot(a: StationRef, b: StationRef): boolean {
  return a.line_id === b.line_id && a.station_index === b.station_index;
}

/** Whether `ref` is a slot of the holder's station: the holder itself or a
 *  slot it is merged with (a junction's slots list each other). */
function isOwnStation(
  holder: StationRef,
  mergedWith: readonly StationRef[] | undefined,
  ref: StationRef,
): boolean {
  return sameSlot(holder, ref) || (mergedWith ?? []).some((m) => sameSlot(m, ref));
}
