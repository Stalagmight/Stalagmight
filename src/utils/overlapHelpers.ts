/**
 * Pure helper functions for overlap handling in the connection wizard.
 *
 * These functions compute overlap shot IDs, target index mappings,
 * merge options, and shot transformations for all 4 overlap cases:
 * (Front/Back) × (Forward/Reverse).
 *
 * Key insight: The Rust backend pre-reverses import shots for ImportEnd::Back
 * (import_from_n) via `.rev().map(reversed())` before matching. This means the
 * relationship between import overlap shot IDs and existing target shot indices
 * is inverted for Back compared to Front.
 */

import type { ShotKind } from '../types/ipc/ShotKind';
import { circularMeanDegrees } from './angles';
import { normalizeAzimuth } from './azimuth';

// ─── Types ──────────────────────────────────────────────────────────────────

/** A shot-like shape from a target section (existing project or another import). */
export interface TargetShotLike {
  azimuth_in: number;
  azimuth_out: number;
  length: number;
  depth_in: number;
  depth_out: number;
  pitch_in: number | null;
  pitch_out: number | null;
  left: number | null;
  right: number | null;
  up: number | null;
  down: number | null;
}

/** An editable shot with optional new/deleted/reference flags. */
export interface EditableShotLike extends TargetShotLike {
  id: number;
  comment: string | null;
  kind: ShotKind;
  isNew?: boolean;
  isDeleted?: boolean;
  tempId?: number;
}

export interface OverlapParams {
  importStationIndex: number;
  matchedShots: number;
  existingMatchedShots?: number;
  existingStationIndex: number;
  direction: 'forward' | 'reverse';
}

export interface MergeOption {
  offset: number;
  importStation: number;
  existingStation: number;
}

interface SuggestionLike {
  importStationIndex: number;
  existingStationIndex: number;
  matchedShots: number;
  existingMatchedShots?: number;
  preferredMergeOffset?: number;
  direction: 'forward' | 'reverse';
  isPartial: boolean;
}

// ─── Core logic ─────────────────────────────────────────────────────────────

/**
 * Determine whether target shots need direction reversal to match import direction.
 *
 * The backend pre-reverses import shots for Back (importStationIndex > 0),
 * so the reversal logic must account for this:
 *
 * | importStationIndex | direction | needsReversal |
 * |--------------------|-----------|---------------|
 * | 0 (Front)          | forward   | false         |
 * | 0 (Front)          | reverse   | true          |
 * | >0 (Back)          | forward   | true          |
 * | >0 (Back)          | reverse   | false         |
 */
export function needsDirectionReversal(
  importStationIndex: number,
  direction: 'forward' | 'reverse',
): boolean {
  return (importStationIndex === 0) !== (direction === 'forward');
}

/**
 * Compute the set of import shot IDs that fall within the overlap region.
 *
 * Front (importStationIndex === 0): shots 0..matchedShots-1
 * Back  (importStationIndex > 0):   shots (importStationIndex-matchedShots)..(importStationIndex-1)
 */
export function computeOverlapIds(
  importStationIndex: number,
  matchedShots: number,
): Set<number> {
  const ids = new Set<number>();
  if (importStationIndex === 0) {
    for (let i = 0; i < matchedShots; i++) ids.add(i);
  } else {
    for (let i = importStationIndex - matchedShots; i < importStationIndex; i++) {
      if (i >= 0) ids.add(i);
    }
  }
  return ids;
}

/**
 * Compute the subset of overlap IDs that are "actionable" - shots up to
 * (but not past) the merge station.
 *
 * The DMP wizard trims these shots from the not-yet-committed import; the
 * orphan-placement dialog resolves the same shots through `computeOverlapPairs`
 * (#681). Either way, acting past the merge station would cut the section off.
 *
 * Front: shots 0..mergeImportStation-1
 * Back:  shots mergeImportStation..(importStationIndex-1)
 */
export function computeActionableIds(
  importStationIndex: number,
  matchedShots: number,
  mergeImportStation: number,
): Set<number> {
  const allOverlap = computeOverlapIds(importStationIndex, matchedShots);
  const actionable = new Set<number>();

  if (importStationIndex === 0) {
    for (let i = 0; i < mergeImportStation; i++) {
      if (allOverlap.has(i)) actionable.add(i);
    }
  } else {
    for (let i = mergeImportStation; i < importStationIndex; i++) {
      if (allOverlap.has(i)) actionable.add(i);
    }
  }
  return actionable;
}

/**
 * Map an overlap shot ID to the corresponding target section shot index.
 *
 * The mapping depends on whether the overlap is at the Front or Back of the
 * import section, and whether the direction is forward or reverse:
 *
 * Front + forward:  existingStationIndex + k
 * Front + reverse:  (existingStationIndex - 1) - k
 * Back  + forward:  (existingStationIndex + matchedShots - 1) - k
 * Back  + reverse:  (existingStationIndex - matchedShots) + k
 *
 * where k = overlapId - overlapStart, and overlapStart is computed from importStationIndex.
 */
export function computeTargetShotIndex(
  overlapId: number,
  params: OverlapParams,
): number {
  const { importStationIndex, matchedShots, existingStationIndex, direction } = params;
  const overlapStart = importStationIndex === 0 ? 0 : importStationIndex - matchedShots;
  const k = overlapId - overlapStart;

  if (importStationIndex === 0) {
    // Front
    return direction === 'forward'
      ? existingStationIndex + k
      : (existingStationIndex - 1) - k;
  } else {
    // Back
    return direction === 'forward'
      ? (existingStationIndex + matchedShots - 1) - k
      : (existingStationIndex - matchedShots) + k;
  }
}

/** One repeated shot: the new section's copy and the existing shot it repeats. */
export interface OverlapPair {
  orphanShotIndex: number;
  targetShotIndex: number;
}

/**
 * The repeated shots a placement resolves, each paired with the existing shot it
 * repeats, in new-section shot order (#681).
 *
 * The "Merge at" station decides how long the repeat is: only the shots on the
 * dangling side of it count (Front: before it, Back: after it). Shots past it
 * are new survey carrying the section on, and must stay Normal - a Reference leg
 * costs infinity in Plan mode by default, so marking them would cut the rest of
 * the section off. The dialog's default, the end of the repeat, takes it all.
 */
export function computeOverlapPairs(params: OverlapParams, mergeImportStation: number): OverlapPair[] {
  return [...computeActionableIds(params.importStationIndex, params.matchedShots, mergeImportStation)]
    .map((orphanShotIndex) => ({
      orphanShotIndex,
      targetShotIndex: computeTargetShotIndex(orphanShotIndex, params),
    }));
}

/**
 * Compute merge station options for a suggestion.
 *
 * Each option pairs an import station with the corresponding existing station.
 * The formula does NOT branch on importStationIndex for existingStation:
 *
 *   importStation  = importStationIndex === 0 ? k : importStationIndex - k
 *   existingStation = direction === 'forward'
 *     ? existingStationIndex + k
 *     : existingStationIndex - k
 */
export function computeMergeOptions(
  suggestion: SuggestionLike,
  importShotCount: number,
): MergeOption[] {
  const {
    importStationIndex,
    existingStationIndex,
    matchedShots,
    direction,
    isPartial,
  } = suggestion;
  const maxImportOffset = isPartial ? matchedShots - 1 : matchedShots;
  const options: MergeOption[] = [];
  const offsets = Array.from({ length: maxImportOffset + 1 }, (_, k) => k);

  for (const offset of offsets) {
    const importStation = importStationIndex === 0 ? offset : importStationIndex - offset;
    const existingStation = direction === 'forward'
      ? existingStationIndex + offset
      : existingStationIndex - offset;

    // Guard: stations must stay within valid ranges
    if (importStation < 0 || importStation > importShotCount) continue;
    if (existingStation < 0) continue;

    if (!options.some((o) => o.importStation === importStation && o.existingStation === existingStation)) {
      options.push({ offset, importStation, existingStation });
    }
  }
  return options;
}

/**
 * The "Merge at" offset the mNemo import wizard preselects (#685): the LAST
 * option, the end of the repeat. Joining there makes every repeated shot
 * actionable (`computeActionableIds` covers the stations before the merge
 * point), so none is imported a second time as a normal shot. The
 * orphan-placement dialog makes the same choice for #681.
 *
 * Deliberately NOT the matcher's `preferredMergeOffset`. That is the best
 * offset by the matcher's own scoring: it favours an interior station (a 1.18x
 * boost) and skips an offset whose join would cross too deep a passage, so for
 * a repeat of two or more shots it is usually mid-repeat. The temporal rerank
 * (`dmpTemporalRerank.ts`) still models suggestions at that offset. The visible
 * wizard default was already the last option (a reset effect chose it once the
 * suggestion rendered); this makes the up-front value agree with it. Returns 0
 * when there are no options.
 */
export function defaultMergeOffset(suggestion: SuggestionLike, importShotCount: number): number {
  const options = computeMergeOptions(suggestion, importShotCount);
  return options[options.length - 1]?.offset ?? 0;
}

// ─── Shot transformations ───────────────────────────────────────────────────

/** Average two nullable numbers; returns null if both are null. */
export function avgNullable(a: number | null, b: number | null): number | null {
  if (a == null && b == null) return null;
  if (a == null) return b;
  if (b == null) return a;
  return (a + b) / 2;
}

/**
 * Reverse a target shot's direction so it matches the opposite traversal direction.
 *
 * Mirrors `reverse_shots` in `src-tauri/src/models/project/reverse.rs` step for
 * step, because the two placements must agree: this runs on the import preview
 * and the backend runs on the committed section.
 *
 * Two things the naive `(x + 180) % 360` got wrong:
 *
 * - JS `%` is TRUNCATED, so its result is only in `(-360, 360)`. An out-of-range
 *   reading came straight back out of range: `(-200 + 180) % 360 === -20`, a
 *   non-ignorable `INVALID_AZIMUTH` at whole-degree scale. Normalising both
 *   readings up front, as the backend does, closes that for either branch.
 * - Readings exactly 180 apart are DEGENERATE: reciprocate-and-swap maps the
 *   pair to itself, so the shot keeps its old bearing while everything around it
 *   turns around. The backend swaps such a pair instead, which flips the
 *   arbitrary tie-break `average_azimuth` resolves the undefined mean with.
 *   Without that branch this disagreed with the backend by a full 180 degrees:
 *   `{45, 225}` came back `{45, 225}` against the backend's `{225, 45}`.
 */
export function reverseTargetShot(shot: TargetShotLike): TargetShotLike {
  const azimuthIn = normalizeAzimuth(shot.azimuth_in);
  const azimuthOut = normalizeAzimuth(shot.azimuth_out);
  const antipodal = normalizeAzimuth(azimuthOut - azimuthIn) === 180;
  const [reversedIn, reversedOut] = antipodal
    ? [azimuthOut, azimuthIn]
    : [normalizeAzimuth(azimuthOut + 180), normalizeAzimuth(azimuthIn + 180)];
  return {
    azimuth_in: reversedIn,
    azimuth_out: reversedOut,
    length: shot.length,
    depth_in: shot.depth_out,
    depth_out: shot.depth_in,
    pitch_in: shot.pitch_out != null ? -shot.pitch_out : null,
    pitch_out: shot.pitch_in != null ? -shot.pitch_in : null,
    left: shot.right,
    right: shot.left,
    up: shot.up,
    down: shot.down,
  };
}

/** Replace an editable shot's measurement values with target data. */
export function applyTargetData<T extends EditableShotLike>(shot: T, target: TargetShotLike): T {
  return {
    ...shot,
    azimuth_in: target.azimuth_in,
    azimuth_out: target.azimuth_out,
    length: target.length,
    depth_in: target.depth_in,
    depth_out: target.depth_out,
    pitch_in: target.pitch_in,
    pitch_out: target.pitch_out,
    left: target.left,
    right: target.right,
    up: target.up,
    down: target.down,
  };
}

/** Average import and target shot values (circular mean for azimuths). */
export function interpolateShots<T extends EditableShotLike>(importShot: T, target: TargetShotLike): T {
  return {
    ...importShot,
    azimuth_in: circularMeanDegrees(importShot.azimuth_in, target.azimuth_in),
    azimuth_out: circularMeanDegrees(importShot.azimuth_out, target.azimuth_out),
    length: (importShot.length + target.length) / 2,
    depth_in: (importShot.depth_in + target.depth_in) / 2,
    depth_out: (importShot.depth_out + target.depth_out) / 2,
    pitch_in: avgNullable(importShot.pitch_in, target.pitch_in),
    pitch_out: avgNullable(importShot.pitch_out, target.pitch_out),
    left: avgNullable(importShot.left, target.left),
    right: avgNullable(importShot.right, target.right),
    up: avgNullable(importShot.up, target.up),
    down: avgNullable(importShot.down, target.down),
  };
}
