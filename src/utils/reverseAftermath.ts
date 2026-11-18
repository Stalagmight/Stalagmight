/**
 * Frontend state that a line reverse invalidates (#24).
 *
 * The backend mirrors everything it owns: shots, the line anchor, and every
 * stored `StationKey` (merges, loop closures, zero datums, station offsets and
 * comments). What it cannot reach is the frontend's own POSITIONAL references -
 * `StationRef { line_id, station_index }` held in UI selection, the merge
 * tool's staging, the move-comment pickup and the distance-gradient origin.
 *
 * Those are the dangerous kind of stale: a reverse renumbers station `i` to
 * `N - i`, so every one of them stays *in range* and simply starts naming a
 * different physical station. Nothing errors. The merge tool would merge the
 * wrong end; the distance ramp would silently re-root from the far end while
 * still looking correct.
 *
 * The distance-gradient origin is mirrored (the user's chosen root stays on the
 * physical station they picked); selection, merge staging, the move-comment
 * pickup and the comment popover are cleared, matching what every other
 * structural mutation does (ConfirmDialog, BulkDeleteDialog, DeleteShotChoice).
 *
 * The GAS PLAN's pins used to be mirrored here too and no longer are (#310).
 * Mirroring only ever answered a reverse; a delete or an insert renumbers just
 * as silently and had no answer at all. The pins now hold backend ANCHORS -
 * a station named by the shots either side of it - and are re-resolved from
 * them around every mutation (`flushPlanAnchors` / `remapPlanPins` below), which
 * is reverse-proof by construction and covers the other edits for free.
 *
 * The same mirror has to run again whenever an undo or redo flips the numbering
 * BACK - otherwise Ctrl+Z restores the old numbering while the origin stays
 * mirrored onto the opposite physical station. The backend answers "which
 * lines, against which shot count"
 * outright on `CommandResult.reversed_lines`, because the geometry cannot: a
 * reverse keeps every world coordinate, so a line that was reversed AND moved
 * in one action matches neither arrangement afterwards.
 *
 * Those entries are COMPOSED into one affine map per line and applied once
 * (see `composeReverseTransforms`), never replayed per entry. The mirror is not
 * an involution - it CLAMPS - and a multi-step `undo_to` can walk past a
 * `CreateShot`, so the count differs between steps; either fact on its own
 * breaks a per-entry fold.
 *
 * SPLIT LINE HERE and COMBINE LINES (and their undo and redo) move shots
 * between lines, which no index transform describes. Around every survey
 * edit the seam therefore also anchors the distance-gradient origin the way the
 * plan pins are anchored (`beforeSurveyEdit` / `afterSurveyEdit`): by the shots
 * either side of it, which the backend places again after the edit, in
 * whichever line now holds them. An origin that places nowhere, or that
 * could not be anchored and has fallen out of range, is cleared with a notice
 * (the ramp falls back to the entrance) rather than left to re-root silently;
 * and an edit that moved shots between lines clears the selection, the
 * merge staging, the move-comment pickup, the comment popover and the hover,
 * as a reverse does (`applyEditAftermath`).
 *
 * The rest of the frontend's station-keyed state needs nothing here: the plan
 * pins (route origin, endpoint, way points, marked points, stage drops) are
 * anchored the same way (`flushPlanAnchors` / `remapPlanPins`); the ruler is
 * cleared on every `modified_at` change (MapCanvas), which every edit, undo
 * and redo makes; the reach field and its route are recomputed from the
 * re-resolved pins on that same change; and line focus exits when its
 * line disappears (SingleSystemView).
 */
import i18n from '../i18n';
import type { Line, LineId, StationRef } from '../types/project';
import type { ReversedLine } from '../types/ipc/ReversedLine';
import { selectHasPins, useGasPlanStore } from '../store/gasPlanStore';
import { useUIStore } from '../store/uiStore';
import { planAnchorsForRefs, planRefsForAnchors, type PlanAnchor } from './divePlanApi';
import { TOAST_KEYS } from './toastKeys';

/**
 * The NET renumbering one line came out of an operation with, as the affine
 * map `i -> shift + sign * i` over station indices.
 *
 * One reverse of an `N`-shot line is `{ shift: N, sign: -1 }`; composing two
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
  /** Highest valid station index in the line as it stands NOW (== its shot count). */
  maxIndex: number;
}

/**
 * The transform ONE reverse of a line with `shotCount` shots applies:
 * station `i` becomes station `shotCount - i`.
 *
 * TEST FIXTURE ONLY - no production caller. The dialog deliberately does NOT use it:
 * a count read when the dialog rendered can be stale by the time the reverse takes
 * the backend's command lock, so every production path composes
 * `CommandResult.reversed_lines` instead (see `composeReverseTransforms`).
 * Kept because it expresses one reverse's transform readably in assertions.
 */
export function singleReverseTransform(shotCount: number): ReverseTransform {
  return { shift: shotCount, sign: -1, maxIndex: shotCount };
}

/** True when the composed transform is `i -> i`: the numbering ended where it started. */
export function isIdentityTransform(transform: ReverseTransform): boolean {
  return transform.sign === 1 && transform.shift === 0;
}

/** True when `transforms` actually renumbered `lineId`; an identity entry did not. */
function flipsLine(
  transforms: Map<LineId, ReverseTransform>,
  lineId: LineId
): boolean {
  const transform = transforms.get(lineId);
  return transform !== undefined && !isIdentityTransform(transform);
}

/**
 * Apply a composed transform to one index, clamping ONCE at the end.
 *
 * Clamped, not a bare affine map: a plan pin outlives the shots it was dropped
 * on (its only reset is a project switch), so `station_index` can already sit
 * past the end of a shortened line - and `shotCount - index` then goes
 * NEGATIVE. `StationRef.station_index` is a `u32`, so serde rejects the whole
 * `compute_gas_reach_field` payload and the reach overlay fails permanently on
 * one poisoned pin rather than degrading. The backend's import-side mirror clamps
 * for the same reason. An already-broken pin lands on an end station instead of a
 * negative index; it was not naming a real station either way, and the clamp
 * keeps every other pin working.
 *
 * ONCE is load-bearing: clamping per step would send an out-of-range pin
 * `p -> 0 -> shotCount` across two reverses that between them changed nothing.
 * Composing first and clamping at the end keeps a net no-op a no-op.
 */
export function applyTransform(ref: StationRef, transform: ReverseTransform): StationRef {
  const moved = transform.shift + transform.sign * ref.station_index;
  if (moved < 0) return { ...ref, station_index: 0 };
  if (moved > transform.maxIndex) return { ...ref, station_index: transform.maxIndex };
  return { ...ref, station_index: moved };
}

/** Re-point `ref` by whatever its line's numbering ended up doing. */
export function mirrorRef(
  ref: StationRef,
  transforms: Map<LineId, ReverseTransform>
): StationRef {
  const transform = transforms.get(ref.line_id);
  // An identity entry is a line whose numbering ended exactly where it
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
 * Compose a `CommandResult.reversed_lines` log into one transform per line.
 *
 * Taken from the backend rather than inferred from the geometry: a reverse keeps
 * every station's world position, so a line that was reversed AND moved in
 * the same action matches NEITHER arrangement afterwards and a position diff
 * reports nothing for it, leaving its pins on the opposite end.
 *
 * The entries arrive in step order and WITH REPEATS, each carrying the shot count
 * in force AT ITS STEP. `m_c(i) = c - i`, so over counts `c1..ck` the composition
 * is `i -> A + s*i` with `s = (-1)^k` and `A_m = c_m - A_(m-1)` (`A_0 = 0`). Two
 * equal counts collapse to the exact identity; two different ones (a `CreateShot`
 * undone between two reverses of the same line) leave a pure shift, which is
 * the answer neither "cancel the repeat" nor "replay the mirror twice" can give.
 *
 * `lines` is the project state AFTER the operation, and supplies `maxIndex`:
 * the numbering the pins now have to fit. A line the refreshed project no
 * longer has is dropped - there is nothing left to mirror into - and so is a
 * shot-less one, which is a single station with no other end (mirroring it would
 * only churn the UI state `applyReverseAftermath` clears).
 */
export function composeReverseTransforms(
  entries: readonly ReversedLine[] | undefined,
  lines: readonly Pick<Line, 'id' | 'shots'>[]
): Map<LineId, ReverseTransform> {
  const out = new Map<LineId, ReverseTransform>();
  if (entries === undefined || entries.length === 0) return out;

  const composed = new Map<LineId, { shift: number; sign: 1 | -1 }>();
  for (const entry of entries) {
    const soFar = composed.get(entry.line_id) ?? { shift: 0, sign: 1 as const };
    composed.set(entry.line_id, {
      shift: entry.shot_count - soFar.shift,
      sign: soFar.sign === 1 ? -1 : 1,
    });
  }

  for (const line of lines) {
    const transform = composed.get(line.id);
    if (transform === undefined || line.shots.length === 0) continue;
    out.set(line.id, { ...transform, maxIndex: line.shots.length });
  }
  return out;
}

/**
 * Re-point (or drop) every frontend-held positional ref after the lines in
 * `transforms` were renumbered.
 *
 * Applied by the store, from `CommandResult.reversed_lines`, on EVERY route -
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
  transforms: Map<LineId, ReverseTransform>,
  options: { originPlaced?: boolean } = {}
): void {
  if (Array.from(transforms.values()).every(isIdentityTransform)) return;

  // The gas plan is deliberately absent here (#310): its pins and its saved-plan baseline are
  // re-resolved from backend anchors by `remapPlanPins`, which the same seam already awaited
  // before this ran. Mirroring them a second time would apply the shift twice. The same holds
  // for the origin once `afterSurveyEdit` has placed it from its anchor (`originPlaced`).
  const ui = useUIStore.getState();
  const origin = ui.distanceGradientOrigin;
  if (origin !== null && !options.originPlaced && flipsLine(transforms, origin.line_id)) {
    // Same reasoning as the plan pins: the penetration ramp is rooted on a
    // station the user chose, and its only other reset is a project switch, so
    // without this the ramp silently re-roots from the opposite end of the
    // line while the origin ring and the DetailPanel badge agree with it.
    ui.setDistanceGradientOrigin(mirrorRef(origin, transforms));
  }

  // Cleared instead - there is no "same place" for a highlight, and a
  // half-staged merge (or a picked-up comment) that silently retargets the far
  // end is worse than one the user has to restart.
  ui.clearSelection();
  ui.clearMergeStations();
  ui.clearMoveCommentSource();
  ui.clearCommentTarget();
}

/** What a survey edit carries from before it to after it for the frontend's positional refs. */
export interface SurveyEdit {
  /** Every line's shot count before the edit. */
  shape: ReadonlyMap<LineId, number>;
  /** The distance-gradient origin before the edit, and the backend's anchor for it. */
  origin: StationRef | null;
  originAnchor: PlanAnchor | null;
  /** Set once `afterSurveyEdit` has placed (or cleared) the origin from its anchor. */
  originPlaced: boolean;
}

const sameRef = (a: StationRef | null, b: StationRef | null): boolean =>
  a === b || (a !== null && b !== null && a.line_id === b.line_id && a.station_index === b.station_index);

/**
 * Before a survey edit: flush the plan-pin anchors (#310), note every line's shot count, and
 * anchor the distance-gradient origin, if one is set. A failed anchor is a degradation, not a
 * failure: the origin then falls back to the reverse mirror and the range check.
 */
export async function beforeSurveyEdit(
  lines: readonly Pick<Line, 'id' | 'shots'>[] | undefined
): Promise<SurveyEdit> {
  await flushPlanAnchors();
  const shape = new Map((lines ?? []).map((s) => [s.id, s.shots.length] as const));
  const origin = useUIStore.getState().distanceGradientOrigin;
  let originAnchor: PlanAnchor | null = null;
  if (origin !== null) {
    try {
      originAnchor = (await planAnchorsForRefs([origin]))[0] ?? null;
    } catch (e) {
      console.debug('[distance] origin anchor capture failed', e);
    }
  }
  return { shape, origin, originAnchor, originPlaced: false };
}

/** Clear the distance-gradient origin, saying so: the ramp is measured from the entrance again. */
function dropOrigin(): void {
  const ui = useUIStore.getState();
  ui.setDistanceGradientOrigin(null);
  ui.showToast(i18n.t('map.toasts.distanceOriginReset'), TOAST_KEYS.DISTANCE_ORIGIN_RESET, 'info');
}

/**
 * After a survey edit, before the refetch: re-resolve the plan pins, and place the origin again
 * from its anchor. A user who re-rooted during the round trip keeps their pick.
 */
export async function afterSurveyEdit(edit: SurveyEdit | undefined): Promise<void> {
  await remapPlanPins();
  if (edit === undefined || edit.origin === null || edit.originAnchor === null) return;
  let placed: StationRef | null;
  try {
    placed = (await planRefsForAnchors([edit.originAnchor]))[0] ?? null;
  } catch (e) {
    console.debug('[distance] origin re-resolve failed', e);
    return;
  }
  const ui = useUIStore.getState();
  if (!sameRef(ui.distanceGradientOrigin, edit.origin)) return;
  edit.originPlaced = true;
  if (placed === null) dropOrigin();
  else if (!sameRef(placed, edit.origin)) ui.setDistanceGradientOrigin(placed);
}

/**
 * True when an edit moved shots between lines: a line vanished, or one appeared while an
 * existing one changed its shot count. That is a split, a combine, a delete-and-split, or the undo
 * or redo of any of them; a line created or imported on its own is not.
 */
export function movedShotsBetweenLines(
  before: ReadonlyMap<LineId, number>,
  after: readonly Pick<Line, 'id' | 'shots'>[]
): boolean {
  const now = new Map(after.map((s) => [s.id, s.shots.length] as const));
  if (Array.from(before.keys()).some((id) => !now.has(id))) return true;
  const appeared = Array.from(now.keys()).some((id) => !before.has(id));
  return appeared && Array.from(before).some(([id, count]) => now.get(id) !== count);
}

/**
 * Everything the seam does after the refresh: the reverse mirror (skipped for an origin its anchor
 * already placed), and for an edit that moved shots between lines the four clears and a range
 * check, so an origin no anchor could place is cleared, with a notice, rather than left past the
 * end of a line the edit shortened. (Any other edit leaves an out-of-range origin exactly as it
 * was, as before.)
 */
export function applyEditAftermath(
  reversed: readonly ReversedLine[] | undefined,
  lines: readonly Pick<Line, 'id' | 'shots'>[],
  edit: SurveyEdit | undefined
): void {
  applyReverseAftermath(composeReverseTransforms(reversed, lines), {
    originPlaced: edit?.originPlaced ?? false,
  });
  if (edit === undefined || !movedShotsBetweenLines(edit.shape, lines)) return;
  const ui = useUIStore.getState();
  ui.clearSelection();
  ui.clearMergeStations();
  ui.clearMoveCommentSource();
  ui.clearCommentTarget();
  // Display-only, but a hover left on a moved station highlights whatever now
  // has its index (the data table row, the Sections panel row) until the
  // pointer moves again.
  ui.setHoveredStation(null);
  ui.setHoveredShot(null);
  ui.setHoveredLineId(null);
  const origin = ui.distanceGradientOrigin;
  if (origin === null) return;
  const line = lines.find((s) => s.id === origin.line_id);
  if (line === undefined || origin.station_index > line.shots.length) dropOrigin();
}
