/**
 * Per-shot decisions for repeated survey in the orphan-placement dialog (#681).
 *
 * When a placed section repeats shots the project already has, each repeated
 * shot gets one of three choices. Whatever the choice, the new section's copy
 * becomes a Reference shot: kept, never deleted, drawn de-emphasised, and not
 * counted as swimmable length in Plan mode (it still counts in the section's
 * total length). The choice only decides which reading the EXISTING shot ends
 * up with, so its stations, connections, closures and comments stay where they
 * are.
 */

import { averageAzimuth, normalizeAzimuth } from './azimuth';
import {
  computeOverlapPairs,
  interpolateShots,
  needsDirectionReversal,
  reverseTargetShot,
  type OverlapParams,
  type TargetShotLike,
} from './overlapHelpers';

export type OverlapChoice = 'keep_existing' | 'use_new' | 'average';

export const OVERLAP_CHOICES: readonly OverlapChoice[] = ['keep_existing', 'use_new', 'average'];

/** One repeated shot: the new section's copy and the existing shot it repeats. */
export interface RepeatedShot<S extends TargetShotLike> {
  orphanShotIndex: number;
  targetShotIndex: number;
  orphanShot: S;
  targetShot: S;
  /** The new reading, oriented the way the existing shot runs. */
  incoming: TargetShotLike;
}

/**
 * The repeated shots a placement resolves (`computeOverlapPairs`), with both
 * shots looked up. A pair missing either shot is dropped whole, so the table
 * the user decides on and the step that applies it list exactly the same shots.
 */
export function repeatedShots<S extends TargetShotLike>(
  params: OverlapParams,
  mergeImportStation: number,
  orphanShots: readonly S[],
  targetShots: readonly S[],
): RepeatedShot<S>[] {
  const reverse = needsDirectionReversal(params.importStationIndex, params.direction);
  const shots: RepeatedShot<S>[] = [];
  for (const pair of computeOverlapPairs(params, mergeImportStation)) {
    const orphanShot = orphanShots[pair.orphanShotIndex];
    const targetShot = targetShots[pair.targetShotIndex];
    if (!orphanShot || !targetShot) continue;
    const incoming = reverse ? reverseTargetShot(orphanShot) : orphanShot;
    shots.push({ ...pair, orphanShot, targetShot, incoming });
  }
  return shots;
}

/**
 * How far two readings of one shot may differ before the row is highlighted:
 * above the rounding any survey format stores (cm, tenths of a degree), below
 * a genuinely different measurement. The matcher's own limits are no use here -
 * they are reject cutoffs, so every matched shot is already inside them.
 */
export const REPEAT_DIFF_TOLERANCE = {
  length: 0.1,
  azimuth: 1,
  inclination: 1,
  depth: 0.1,
} as const;

/** How far apart two readings of the same shot are, per measurement. */
export interface ReadingDiff {
  length: number;
  azimuth: number;
  /** Null when either reading has no inclination. */
  inclination: number | null;
  /** The larger of the start-depth and end-depth differences. */
  depth: number;
}

/** The measured fields of a shot, without its identity, kind or comment. */
export function toReading(shot: TargetShotLike): TargetShotLike {
  return {
    azimuth_in: shot.azimuth_in,
    azimuth_out: shot.azimuth_out,
    length: shot.length,
    depth_in: shot.depth_in,
    depth_out: shot.depth_out,
    pitch_in: shot.pitch_in,
    pitch_out: shot.pitch_out,
    left: shot.left,
    right: shot.right,
    up: shot.up,
    down: shot.down,
  };
}

/** The shot's displayed bearing: the mean of its two legs. */
export function shotAzimuth(shot: TargetShotLike): number {
  return averageAzimuth(shot.azimuth_in, shot.azimuth_out);
}

/** The shot's displayed inclination: the mean of the legs it has, or null. */
export function shotInclination(shot: TargetShotLike): number | null {
  if (shot.pitch_in == null && shot.pitch_out == null) return null;
  if (shot.pitch_in == null) return shot.pitch_out;
  if (shot.pitch_out == null) return shot.pitch_in;
  return (shot.pitch_in + shot.pitch_out) / 2;
}

/** Absolute difference between two readings, both oriented the same way. */
export function readingDiff(existing: TargetShotLike, incoming: TargetShotLike): ReadingDiff {
  const existingInclination = shotInclination(existing);
  const incomingInclination = shotInclination(incoming);
  return {
    length: Math.abs(existing.length - incoming.length),
    azimuth: Math.abs(normalizeAzimuth(shotAzimuth(incoming) - shotAzimuth(existing) + 180) - 180),
    inclination:
      existingInclination == null || incomingInclination == null
        ? null
        : Math.abs(existingInclination - incomingInclination),
    depth: Math.max(
      Math.abs(existing.depth_in - incoming.depth_in),
      Math.abs(existing.depth_out - incoming.depth_out),
    ),
  };
}

/**
 * Slack on every tolerance comparison: readings are stored in binary floats, so
 * 1.1 - 1.0 lands just above 0.1 and 4.3 - 4.2 just below it. A difference of
 * exactly the tolerance must classify the same whatever the absolute values.
 */
const FLOAT_SLACK = 1e-9;

/** Which measurements differ by more than `REPEAT_DIFF_TOLERANCE`. */
export function repeatDiffFlags(diff: ReadingDiff): Record<keyof ReadingDiff, boolean> {
  const over = (value: number, tolerance: number) => value > tolerance + FLOAT_SLACK;
  return {
    length: over(diff.length, REPEAT_DIFF_TOLERANCE.length),
    azimuth: over(diff.azimuth, REPEAT_DIFF_TOLERANCE.azimuth),
    inclination: diff.inclination != null && over(diff.inclination, REPEAT_DIFF_TOLERANCE.inclination),
    depth: over(diff.depth, REPEAT_DIFF_TOLERANCE.depth),
  };
}

/** Whether any measurement differs by more than `REPEAT_DIFF_TOLERANCE`. */
export function exceedsRepeatTolerance(diff: ReadingDiff): boolean {
  const flags = repeatDiffFlags(diff);
  return flags.length || flags.azimuth || flags.inclination || flags.depth;
}

/**
 * Whether two readings are the same within float noise. Azimuths compare round
 * the circle, so 0 and 360 match; nullable fields must both be null or both close.
 */
function sameReading(a: TargetShotLike, b: TargetShotLike): boolean {
  const close = (x: number, y: number) => Math.abs(x - y) <= FLOAT_SLACK;
  const closeAzimuth = (x: number, y: number) => close(normalizeAzimuth(x - y + 180), 180);
  const closeNullable = (x: number | null, y: number | null) =>
    x == null || y == null ? x === y : close(x, y);
  return (
    closeAzimuth(a.azimuth_in, b.azimuth_in) &&
    closeAzimuth(a.azimuth_out, b.azimuth_out) &&
    close(a.length, b.length) &&
    close(a.depth_in, b.depth_in) &&
    close(a.depth_out, b.depth_out) &&
    closeNullable(a.pitch_in, b.pitch_in) &&
    closeNullable(a.pitch_out, b.pitch_out) &&
    closeNullable(a.left, b.left) &&
    closeNullable(a.right, b.right) &&
    closeNullable(a.up, b.up) &&
    closeNullable(a.down, b.down)
  );
}

/**
 * The new reading for Use new. A field the new shot does not have keeps the
 * existing value: `updateShot` reads `null` as CLEAR, so passing the gap through
 * would erase the existing shot's pitch or LRUD.
 */
function newReadingOver(existing: TargetShotLike, incoming: TargetShotLike): TargetShotLike {
  return {
    ...toReading(incoming),
    pitch_in: incoming.pitch_in ?? existing.pitch_in,
    pitch_out: incoming.pitch_out ?? existing.pitch_out,
    left: incoming.left ?? existing.left,
    right: incoming.right ?? existing.right,
    up: incoming.up ?? existing.up,
    down: incoming.down ?? existing.down,
  };
}

/**
 * The reading the existing shot should take for `choice`, or null when there
 * is nothing to write: Keep existing, or a result equal to the existing reading
 * (so an identical repeat pushes no undo entry). `incoming` must already be
 * oriented the way the existing shot runs.
 *
 * Neither choice ever clears a value: Use new keeps the existing value where the
 * new shot has none, and Average (`avgNullable`) takes whichever reading has
 * the field when only one does.
 */
export function resolvedReading(
  choice: OverlapChoice,
  existing: TargetShotLike,
  incoming: TargetShotLike,
): TargetShotLike | null {
  let reading: TargetShotLike;
  switch (choice) {
    case 'keep_existing':
      return null;
    case 'use_new':
      reading = newReadingOver(existing, incoming);
      break;
    case 'average':
      reading = toReading(
        interpolateShots({ ...toReading(existing), id: 0, comment: null, kind: 'Normal' }, incoming),
      );
      break;
  }
  return sameReading(reading, existing) ? null : reading;
}
