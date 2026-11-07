/**
 * Frontend state that a section reverse invalidates (#24).
 *
 * The backend mirrors everything it owns: shots, the section anchor, and every
 * stored `StationKey` (merges, loop closures, zero datums, station offsets and
 * comments). What it cannot reach is the frontend's own POSITIONAL references -
 * `StationRef { section_id, station_index }` held in UI selection, the merge
 * tool's staging and the move-comment pickup.
 *
 * Those are the dangerous kind of stale: a reverse renumbers station `i` to
 * `N - i`, so every one of them stays *in range* and simply starts naming a
 * different physical station. Nothing errors. The merge tool would merge the
 * wrong end. Selection, merge staging, the move-comment pickup and the comment
 * popover are therefore cleared, matching what every other structural mutation
 * does (ConfirmDialog, BulkDeleteDialog, DeleteShotChoice).
 *
 * The GAS PLAN's pins used to be mirrored here and no longer are (#310), and nor
 * is the distance-gradient origin (#576 D39(g)). Mirroring only ever answered a
 * reverse; a delete or an insert renumbers just as silently and had no answer at
 * all. Both now hold backend ANCHORS - a station named by the shots either side
 * of it - re-resolved around every mutation (`flushPlanAnchors` / `remapPlanPins`
 * below, and `utils/distanceOriginAnchor.ts`), which is reverse-proof by
 * construction and covers the other edits for free. The one origin still
 * mirrored here is one whose anchor could not be TAKEN: nothing re-resolves it,
 * so the reverse keeps the answer it had before the anchors (see
 * `applyReverseAftermath`).
 *
 * The clears have to run again whenever an undo or redo flips the numbering
 * BACK. The backend answers "which sections, against which shot count"
 * outright on `CommandResult.reversed_sections`, because the geometry cannot: a
 * reverse keeps every world coordinate, so a section that was reversed AND moved
 * in one action matches neither arrangement afterwards.
 *
 * Those entries are COMPOSED into one affine map per section and applied once
 * (see `composeReverseTransforms`), never replayed per entry. The mirror is not
 * an involution - it CLAMPS - and a multi-step `undo_to` can walk past a
 * `CreateShot`, so the count differs between steps; either fact on its own
 * breaks a per-entry fold.
 */
import type { Section, SectionId, StationRef } from '../types/project';
import type { ReversedSection } from '../types/ipc/ReversedSection';
import type { OriginCapture } from './distanceOriginAnchor';
import { selectHasPins, useGasPlanStore } from '../store/gasPlanStore';
import { useUIStore } from '../store/uiStore';

/**
 * The NET renumbering one section came out of an operation with, as the affine
 * map `i -> shift + sign * i` over station indices.
 *
 * One reverse of an `N`-shot section is `{ shift: N, sign: -1 }`; composing two
 * with counts `c1` then `c2` gives `{ shift: c2 - c1, sign: +1 }`, which is the
 * IDENTITY exactly when the counts match. That is the whole reason for carrying a
 * transform rather than a divisor: `i -> N - i` applied twice is only the
 * identity when nothing changed the count in between, and only for an in-range
 * index (see `applyTransform`'s clamp).
 */
export interface ReverseTransform {
  /** `A` in `i -> A + s * i`. */
  shift: number;
  /** `s` in `i -> A + s * i`: `-1` after an odd number of reverses, `+1` after an even one. */
  sign: 1 | -1;
  /** Highest valid station index in the section as it stands NOW (== its shot count). */
  maxIndex: number;
}

/**
 * The transform ONE reverse of a section with `shotCount` shots applies:
 * station `i` becomes station `shotCount - i`.
 *
 * TEST FIXTURE ONLY - no production caller. The dialog deliberately does NOT use it:
 * a count read when the dialog rendered can be stale by the time the reverse takes
 * the backend's command lock, so every production path composes
 * `CommandResult.reversed_sections` instead (see `composeReverseTransforms`).
 * Kept because it expresses one reverse's transform readably in assertions.
 */
export function singleReverseTransform(shotCount: number): ReverseTransform {
  return { shift: shotCount, sign: -1, maxIndex: shotCount };
}

/** True when the composed transform is `i -> i`: the numbering ended where it started. */
export function isIdentityTransform(transform: ReverseTransform): boolean {
  return transform.sign === 1 && transform.shift === 0;
}

/**
 * Apply a composed transform to one index, clamping ONCE at the end.
 *
 * Clamped, not a bare affine map. Its one production ref (via `mirrorRef`, from
 * `applyReverseAftermath`) is a distance-gradient origin whose anchor could not
 * be taken: nothing re-resolves that origin, so an earlier delete can have left
 * its `station_index` past the end of a shortened section - and
 * `shotCount - index` then goes NEGATIVE. `StationRef.station_index` is a `u32`,
 * so serde would reject the `get_station_distances_from` payload the origin
 * feeds, and the gradient would fail on every refetch rather than degrade. (The
 * gas plan's pins, which this clamp was first written for, no longer pass here:
 * they are re-resolved from backend anchors, #310.) An out-of-range ref lands on
 * an end station instead of a negative index; it was not naming a real station
 * either way.
 *
 * ONCE is load-bearing: clamping per step would send an out-of-range ref
 * `p -> 0 -> shotCount` across two reverses that between them changed nothing.
 * Composing first and clamping at the end keeps a net no-op a no-op.
 */
export function applyTransform(ref: StationRef, transform: ReverseTransform): StationRef {
  const moved = transform.shift + transform.sign * ref.station_index;
  if (moved < 0) return { ...ref, station_index: 0 };
  if (moved > transform.maxIndex) return { ...ref, station_index: transform.maxIndex };
  return { ...ref, station_index: moved };
}

/** Re-point `ref` by whatever its section's numbering ended up doing. */
export function mirrorRef(
  ref: StationRef,
  transforms: Map<SectionId, ReverseTransform>
): StationRef {
  const transform = transforms.get(ref.section_id);
  // An identity entry is a section whose numbering ended exactly where it
  // started. Returning `ref` itself rather than running the clamp keeps that a
  // TOTAL no-op: `applyTransform` would still pull an already out-of-range pin
  // down to `maxIndex`, which is a silent move nothing asked for.
  if (transform === undefined || isIdentityTransform(transform)) return ref;
  return applyTransform(ref, transform);
}

/**
 * The gas plan's half of the aftermath, either side of a survey mutation (#310).
 *
 * Split in two because the two halves have to straddle the edit: the anchors must be taken while
 * the pins still describe the survey the diver placed them on, and re-resolved once the backend
 * has applied the edit. Both are no-ops for a plan with no pins, which is why the mutation seam
 * can call them unconditionally.
 */
export function flushPlanAnchors(): Promise<void> {
  const plan = useGasPlanStore.getState();
  return selectHasPins(plan) ? plan.flushAnchorCapture() : Promise.resolve();
}

/** Re-resolve the plan pins against the survey as it now stands. Must run BEFORE the project
 *  refetch, so the reach field is recomputed from corrected pins rather than stale ones. */
export function remapPlanPins(): Promise<void> {
  const plan = useGasPlanStore.getState();
  return selectHasPins(plan) ? plan.remapPinsFromAnchors() : Promise.resolve();
}

/**
 * Compose a renumbering log into one transform per section: a forward edit's
 * `reversed_sections` (`ReverseOutcome`, `BatchOutcome`) or an undo/redo's
 * `reversed_steps` (`HistoryOutcome`).
 *
 * Taken from the backend rather than inferred from the geometry: a reverse keeps
 * every station's world position, so a section that was reversed AND moved in
 * the same action matches NEITHER arrangement afterwards and a position diff
 * reports nothing for it, leaving its pins on the opposite end.
 *
 * The entries arrive in step order, each carrying the shot count in force AT ITS
 * STEP. A forward edit names a section at most once (the backend nets a batch's
 * repeats by parity, `Command::reversed_section_ids`); a multi-step undo/redo
 * names it once per step that reversed it, and those steps' counts can differ.
 * `m_c(i) = c - i`, so over counts `c1..ck` the composition
 * is `i -> A + s*i` with `s = (-1)^k` and `A_m = c_m - A_(m-1)` (`A_0 = 0`). Two
 * equal counts collapse to the exact identity; two different ones (a `CreateShot`
 * undone between two reverses of the same section) leave a pure shift, which is
 * the answer neither "cancel the repeat" nor "replay the mirror twice" can give.
 *
 * `sections` is the project state AFTER the operation, and supplies `maxIndex`:
 * the numbering the pins now have to fit. A section the refreshed project no
 * longer has is dropped - there is nothing left to mirror into - and so is a
 * shot-less one, which is a single station with no other end (mirroring it would
 * only churn the UI state `applyReverseAftermath` clears).
 */
export function composeReverseTransforms(
  entries: readonly ReversedSection[] | undefined,
  sections: readonly Pick<Section, 'id' | 'shots'>[]
): Map<SectionId, ReverseTransform> {
  const out = new Map<SectionId, ReverseTransform>();
  if (entries === undefined || entries.length === 0) return out;

  const composed = new Map<SectionId, { shift: number; sign: 1 | -1 }>();
  for (const entry of entries) {
    const soFar = composed.get(entry.section_id) ?? { shift: 0, sign: 1 as const };
    composed.set(entry.section_id, {
      shift: entry.shot_count - soFar.shift,
      sign: soFar.sign === 1 ? -1 : 1,
    });
  }

  for (const section of sections) {
    const transform = composed.get(section.id);
    if (transform === undefined || section.shots.length === 0) continue;
    out.set(section.id, { ...transform, maxIndex: section.shots.length });
  }
  return out;
}

/**
 * Re-point (or drop) every frontend-held positional ref after the sections in
 * `transforms` were renumbered.
 *
 * Applied by the store, from `CommandResult.reversed_sections`, on EVERY route -
 * `executeCommand` for a forward reverse and `executeUndoRedo` for the flip back -
 * immediately after the refresh each belongs to. Keeping both in one place is what
 * stops a forward mirror landing after a concurrent undo has already mirrored the
 * pins back, which would re-apply the shift onto restored numbering.
 *
 * An all-identity map returns immediately instead of running an identity mirror.
 * That is not an optimisation: the four clears below are destructive, and a range
 * of reverses that cancelled out renumbered nothing, so the user's selection and
 * half-staged merge are still valid.
 */
export function applyReverseAftermath(
  transforms: Map<SectionId, ReverseTransform>,
  origin: OriginCapture | null = null
): void {
  if (Array.from(transforms.values()).every(isIdentityTransform)) return;

  // The gas plan (#310) and an anchored distance-gradient origin (#576 D39(g)) are deliberately
  // absent here: both are re-resolved from backend anchors (`remapPlanPins`,
  // `remapDistanceOrigin`), which the same seam already awaited before this ran. Mirroring them
  // as well would apply the shift twice.
  const ui = useUIStore.getState();

  // An origin whose anchor could not be taken (`origin` is the capture from before this edit)
  // was re-resolved by nothing, so it is mirrored here as it was before the anchors - only while
  // it is still the station that failed: a pick since was made against the new numbering.
  const current = ui.distanceGradientOrigin;
  if (
    origin !== null &&
    !origin.anchored &&
    current !== null &&
    current.section_id === origin.ref.section_id &&
    current.station_index === origin.ref.station_index
  ) {
    const mirrored = mirrorRef(current, transforms);
    if (mirrored !== current) ui.setDistanceGradientOrigin(mirrored);
  }

  // Cleared instead - there is no "same place" for a highlight, and a
  // half-staged merge (or a picked-up comment) that silently retargets the far
  // end is worse than one the user has to restart.
  ui.clearSelection();
  ui.clearMergeStations();
  ui.clearMoveCommentSource();
  ui.clearCommentTarget();
}
