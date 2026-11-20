/**
 * The COVERAGE PLAN for `undo-persistence-fuzz`: a fixed sequence of mutations
 * chosen so that every UndoData variant the live app can actually produce is
 * present on the undo stack when the run crosses the save/reload boundary.
 *
 * ## Why a plan and not a random draw
 *
 * `undo-redo-fuzz` draws its mutations at random, which is right for finding
 * unexpected interactions but wrong for a scenario whose hypothesis is "bug 8's
 * mechanism afflicts every id-bearing variant". A random stream reaches a given
 * variant by luck and a short run may miss several, so the coverage claim would
 * be unfalsifiable. Here the ORDER and the SET are fixed; only the targets and
 * the values are drawn from `ctx.rng()`, so different seeds still exercise
 * different lines, stations, geometry and text while every run makes the same
 * coverage promise. The scenario then re-reads `get_undo_stack` and ASSERTS the
 * promised variants are really there.
 *
 * ## Which variants the app can actually produce
 *
 * `AppState::generate_undo_data` / `finalize_undo_data`
 * (src-tauri/src/core/state/undo_capture.rs) plus the two paths that push
 * their own entry (`execute_batch`, `execute_move_comment`) are the ONLY
 * producers (the integrity Repair, a third, is gone). Measured against
 * them, one `UndoData` variant never reaches a stack - see
 * `UNREACHABLE_VARIANTS`.
 *
 * ## The variant map
 *
 * `get_undo_stack` gives the frontend a `commandType` (the snake_case `Command`
 * name, `commands/history.rs::get_command_type`) and a description, never the
 * `UndoData` variant. So the mapping below is transcribed from
 * `undo_capture.rs`, one row per command, and is the thing to re-check when that
 * file changes.
 */
import type { ScenarioContext } from './framework';
import type { TargetingDigest } from '../lib/fuzzOracleCore';

/**
 * `src-tauri/src/models/id_stride.rs`. The loader renumbers every shot to
 * `line_id * FEATURE_ID_STRIDE + sequence_order`
 * (persistence/sqlite/read.rs), and the in-session allocator hands out ids from
 * the same space - which is why a stale persisted id COLLIDES rather than
 * missing. Mirrored here so the scenario can prove, before it saves, that the
 * ids it is about to persist really have diverged from positional order.
 */
export const FEATURE_ID_STRIDE = 100_000;

/** The `UndoData` variants a live command can produce. */
export type UndoVariant =
  | 'CreatedLineSnapshot'
  | 'CreatedShotSnapshot'
  | 'PreviousValues(shot)'
  | 'PreviousValues(line)'
  | 'InverseCommand'
  | 'ClosureInfo'
  | 'ZeroDatumInfo'
  | 'SegmentMoveInfo'
  | 'Delta';

/**
 * `UndoHistoryItem.commandType` -> the `UndoData` variant that command's entry
 * carries. Transcribed from `undo_capture.rs`; every row is a `match` arm there.
 * A command captured as two whole-project snapshots (`ProjectSnapshotPair`) is
 * STORED as the compact `Delta` between them (`UndoData::compact`), so that is
 * the variant its entry carries.
 *
 * Two rows are conditional in the Rust and unconditional here, deliberately:
 *
 *   - `create_loop_closure` produces `ClosureInfo` for `Move` /
 *     `LeastSquaresWeighted` and a `ProjectSnapshotPair` for
 *     `ProportionalByLength` / `EqualDistribution` - but those two strategies are
 *     rejected outright by `Project::create_loop_closure`
 *     (models/project/mutations.rs "Closure distribution strategies are
 *     deferred"), so no entry is ever pushed for them. The plan only ever asks
 *     for `Move`.
 *   - `reverse_line` produces a `ProjectSnapshotPair` only for a line with
 *     shots; a shot-less line reverses to itself and pushes nothing at all
 *     (so no row is created to be mapped). The plan only ever reverses a
 *     populated line.
 */
export const VARIANT_BY_COMMAND_TYPE: Readonly<Record<string, UndoVariant>> = {
  create_line: 'CreatedLineSnapshot',
  create_shot: 'CreatedShotSnapshot',
  update_shot: 'PreviousValues(shot)',
  update_line_metadata: 'PreviousValues(line)',
  // `PreviousValues`, not the `InverseCommand` it used to be: that variant is a
  // newtype around `Command` with nowhere to record the target shot's uuid, and
  // the in-session id the inverse command names is renumbered by the loader
  // (issue #413, finding 12). `set_station_comment` still produces
  // `InverseCommand`, so the variant is still covered.
  set_shot_kind: 'PreviousValues(shot)',
  set_station_comment: 'InverseCommand',
  create_loop_closure: 'ClosureInfo',
  remove_loop_closure: 'ClosureInfo',
  set_zero_datum: 'ZeroDatumInfo',
  // Captured as a snapshot pair (the removal can convert a closure), so a
  // compact delta.
  remove_zero_datum: 'Delta',
  move_line: 'SegmentMoveInfo',
  move_segment: 'SegmentMoveInfo',
  delete_line: 'Delta',
  delete_shot: 'Delta',
  reverse_line: 'Delta',
  merge_stations: 'Delta',
  unmerge_stations: 'Delta',
  unmerge_station_pair: 'Delta',
  move_station: 'Delta',
  move_comment: 'Delta',
  batch: 'Delta',
};

/**
 * `UndoData` variants that exist in `src-tauri/src/core/undo.rs` and have a
 * restore arm in `undo()`/`redo()`, but which no live entry holds.
 *
 * Verified by grepping every construction site outside `#[cfg(test)]`: only
 * `undo_capture.rs` and `app_state.rs` build `UndoData`, and between them they
 * emit exactly the nine variants above plus `None` - and the snapshot pair that
 * `UndoData::compact` turns into a `Delta` before it is pushed.
 *
 * This matters for the scope of this scenario: a stale-id defect in one of these
 * arms is unreachable from a project whose persisted history was stripped, so
 * the hunt is bounded by this list rather than by the enum.
 */
export const UNREACHABLE_VARIANTS: Readonly<Record<string, string>> = {
  ProjectSnapshotPair: 'a capture shape only: every pair is stored as a compact Delta',
};

/** One `projectStore` action plus its args, exactly as the bridge dispatches it. */
export interface CoverageAction {
  action: string;
  args: unknown[];
  /** Short label for the action history in a failure message. */
  label: string;
}

/**
 * `STALAGMIGHT_PERSIST_FUZZ_DELETE_IN_TARGET=1` moves the plan's mid-line
 * delete onto the TARGET line instead of the spare one.
 *
 * That single change was the repro for finding 13 (the stale-id WEDGE): a
 * line that has had a shot deleted has handed out more ids than it has shots,
 * so the newest persisted id ran PAST the renumbered range and the undo hit
 * nothing, failed, and was pushed back by `undo_inner` - after which every entry
 * beneath it was unreachable for good, and the unwind could not continue.
 *
 * THE WEDGE IS FIXED (b9c9d649): undo data addresses a shot by `Shot::uuid`, and
 * a load-time applicability pass drops an entry whose target no longer resolves
 * instead of leaving it to fail forever. So the flag is no longer terminal - a
 * run with it set walks the whole restored stack - and the repro line above is
 * kept as a regression check that must pass CLEAN.
 *
 * It stays off by default because the two arrangements are different id-space
 * shapes and the default is the one the plan's coverage promises are written
 * against: with the delete on the SPARE line the target's ids stay inside the
 * renumbered range (a stale id would COLLIDE), and with it on the target they
 * run past it (a stale id would MISS). Both are worth exercising; only one can
 * be the default.
 *
 * Read at CALL time, never frozen into a const: it is part of a trace header, so
 * `pnpm agent:fuzz-shrink` restores it from inside its own `main()` - long after
 * this module was imported. Same rule as the knobs in `undoRedoFuzz.ts`.
 */
export function deleteInTarget(): boolean {
  return process.env.STALAGMIGHT_PERSIST_FUZZ_DELETE_IN_TARGET === '1';
}

/** State the plan carries between its steps. */
export interface CoverageMemo {
  /**
   * The line every shot-level step targets.
   *
   * Deliberately a FRESH line rather than one of the fixture's: it is
   * disconnected, has no zero datum and is in no loop, so `delete_shot`'s
   * "part of a loop / anchored to a zero datum" guard cannot refuse the undo of
   * a shot insert. That guard was open bug 6 (fixed in 72595cc6 - an undo now
   * routes through the guard-free `remove_created_shot`), and the choice of a
   * fresh, unanchored line is kept: it is what makes the plan's promises hold
   * on ANY fixture, rather than only on one whose topology happens to cooperate.
   *
   * It also never has a shot DELETED from it (unless `deleteInTarget()` is set),
   * which is what keeps every id it hands out inside the range the loader
   * renumbers into - see that constant for why that matters.
   */
  createdLineId: number | null;
  /**
   * A second fresh line, whose only job is to be the victim of the plan's
   * mid-line delete without disturbing the target's id space.
   */
  spareLineId: number | null;
  /** Closure ids present before the plan's `createLoopClosure` step. */
  closureIdsBefore: number[];
}

export function newCoverageMemo(): CoverageMemo {
  return { createdLineId: null, spareLineId: null, closureIdsBefore: [] };
}

export interface CoverageStep {
  /** Stable name; used in metrics (`step:<name>`) and the action history. */
  name: string;
  /** The variant this step exists to put on the stack. */
  variant: UndoVariant;
  /**
   * True when NO fixture topology can refuse this step. A guaranteed step that
   * pushes no entry is a bug in the plan and fails the scenario; a
   * topology-dependent one is logged and counted instead.
   */
  guaranteed: boolean;
  /** Why the step is here, one line, shown in the coverage report. */
  purpose: string;
  /** Set when the step creates a line the plan needs to keep hold of. */
  remembers?: 'target' | 'spare';
  build(ctx: ScenarioContext, digest: TargetingDigest, memo: CoverageMemo): CoverageAction | null;
}

// ---------------------------------------------------------------------------
// Value generators. Deliberately NOT shared with `undoRedoFuzzActions.ts`.
//
// That module's generators are boundary-biased on purpose (empty strings, 1e3 m
// displacements, zero-length shots), which is exactly right for a scenario whose
// job is to find input-shaped failures. Here every step MUST succeed, because a
// refused step is a hole in the coverage claim, so the ranges below are nominal
// and well inside every validator. Keeping them local also means this scenario
// can never perturb the main fuzzer's draw stream or its documented repro seeds.
// ---------------------------------------------------------------------------

const COLORS = ['#3388FF', '#FF8833', '#33CC66', '#CC33AA', '#FFCC00', '#6666FF'] as const;
/**
 * Kinds that differ from `Normal`, which is what every shot this plan creates
 * starts as. Setting `Normal` would be a forward no-op with a no-op inverse, and
 * an undo landing on the WRONG shot would then be unobservable.
 */
const NON_DEFAULT_SHOT_KINDS = ['Jump', 'Reference'] as const;
const SURVEY_DIRECTIONS = ['in', 'out'] as const;

function rounded(ctx: ScenarioContext, min: number, max: number, decimals: number): number {
  return Number((min + ctx.rng() * (max - min)).toFixed(decimals));
}

/** A valid, unremarkable `ShotData` (every field present). */
export function nominalShot(ctx: ScenarioContext): Record<string, unknown> {
  const azimuth = rounded(ctx, 0, 359.9, 1);
  const depthIn = rounded(ctx, 0, 30, 2);
  return {
    azimuth_in: azimuth,
    azimuth_out: Number(((azimuth + rounded(ctx, -1.5, 1.5, 1) + 360) % 360).toFixed(1)),
    length: rounded(ctx, 1, 12, 2),
    depth_in: depthIn,
    depth_out: Number(Math.max(0, depthIn + rounded(ctx, -3, 3, 2)).toFixed(2)),
    pitch_in: null,
    pitch_out: null,
    left: rounded(ctx, 0.1, 4, 2),
    right: rounded(ctx, 0.1, 4, 2),
    up: rounded(ctx, 0.1, 4, 2),
    down: rounded(ctx, 0.1, 4, 2),
    comment: null,
    kind: 'Normal',
    marker_index: null,
    temperature: null,
  };
}

/** A valid `LineData` with 3 shots (enough for a mid-line insert AND a delete). */
function nominalLine(ctx: ScenarioContext): Record<string, unknown> {
  return {
    name: `persist-${ctx.randInt(1000, 9999)}`,
    surveyor: `persist-surveyor-${ctx.randInt(1, 9)}`,
    date: `20${ctx.randInt(10, 26)}-0${ctx.randInt(1, 9)}-1${ctx.randInt(0, 9)}`,
    color: ctx.pick(COLORS),
    direction: ctx.pick(SURVEY_DIRECTIONS),
    quality: ctx.randInt(1, 5),
    shots: [nominalShot(ctx), nominalShot(ctx), nominalShot(ctx)],
  };
}

function nominalOffset(ctx: ScenarioContext): Record<string, number> {
  return {
    x: rounded(ctx, -3, 3, 3),
    y: rounded(ctx, -3, 3, 3),
    z: rounded(ctx, -1.5, 1.5, 3),
  };
}

/** Lines by ascending id, so a pick never depends on `Vec` order. */
function sortedLines(digest: TargetingDigest): [number, number[]][] {
  return [...digest.s].sort((a, b) => a[0] - b[0]);
}

function shotsOf(digest: TargetingDigest, lineId: number): number[] {
  const found = digest.s.find((entry) => entry[0] === lineId);
  return found ? [...found[1]] : [];
}

/**
 * A shot whose id will address a DIFFERENT shot after the reload, together with
 * the position it will then land on.
 *
 * The digest lists a line's shot ids in payload (i.e. positional) order, and
 * the loader renumbers position `i` to `lineId * FEATURE_ID_STRIDE + i`. So a
 * persisted id `ids[i]` addresses position `ids[i] - lineId * STRIDE` after
 * the reload, and it is a COLLISION exactly when that number is a different, but
 * still valid, position.
 *
 * Targeting such a shot deliberately is what turns "a stale id might hit the
 * wrong shot" into a property the plan guarantees. Picking uniformly instead
 * leaves it to luck: a run where every id happens to sit at its own position
 * exercises the same code and observes nothing, which is how a coverage claim
 * quietly becomes vacuous.
 *
 * Returns null when the line has no colliding shot; callers fall back to any
 * shot rather than dropping the step, since the entry is still worth having.
 */
function pickCollidingShot(
  ctx: ScenarioContext,
  digest: TargetingDigest,
  lineId: number,
): { shotId: number; position: number; lands: number } | null {
  const ids = shotsOf(digest, lineId);
  const colliding = ids
    .map((id, position) => ({ shotId: id, position, lands: id - lineId * FEATURE_ID_STRIDE }))
    .filter((entry) => entry.lands !== entry.position && entry.lands >= 0 && entry.lands < ids.length);
  if (colliding.length === 0) return null;
  return colliding[ctx.randInt(0, colliding.length - 1)] ?? null;
}

/** Station refs inside one line, ascending by index. */
function stationsOf(digest: TargetingDigest, lineId: number): number[] {
  return digest.st
    .filter((ref) => ref[0] === lineId)
    .map((ref) => ref[1])
    .sort((a, b) => a - b);
}

function toRef(lineId: number, stationIndex: number): Record<string, number> {
  return { line_id: lineId, station_index: stationIndex };
}

/** The created line, or null when `createLine` did not land. */
function target(memo: CoverageMemo): number | null {
  return memo.createdLineId;
}

// ---------------------------------------------------------------------------
// The plan. The ORDER is the most load-bearing thing in this file, and it is not
// the obvious one. It was corrected after a 60-iteration run showed the first
// arrangement could not expose a single stale-id defect.
//
// ## Why the snapshot-captured steps sit low in the stack
//
// `Shot::id` IS serialized inside a project snapshot (models/shot.rs has no
// `skip` on it). Before compact undo entries, a `ProjectSnapshotPair` restored
// the whole recorded project verbatim, so the moment a walk crossed ONE such
// entry the project was back in the PRE-SAVE id space - and every stale id in
// every entry beyond it was suddenly valid again: a snapshot-pair entry healed
// the very corruption this scenario looks for. The exposed window was exactly
// from the depth the reload lands on down to the first pair beneath it; in the
// first arrangement every pair sat ABOVE the id-bearing steps, so that window
// was two or three entries wide and contained none of them (60 iterations
// reported only known bugs).
//
// A snapshot-captured command is now STORED as a compact `Delta`, which
// restores only what its command changed and settles the shot ids it puts back
// against the live ones (`ProjectDelta::apply` in core/undo/delta.rs). It no
// longer resets the id space, so it no longer shrinks the window - but the
// arrangement below is kept, because it still puts every id-bearing entry where
// the reload lands first.
//
// ## The arrangement that follows from that
//
//   1-2   the fresh target line and the spare one the delete uses.
//   3-8   every ProjectSnapshotPair step, as LOW in the stack as their
//         preconditions allow, so they are the last thing a walk reaches.
//   9-14  the graph and metadata entries, whose undo data names lines and
//         uuid-anchored stations rather than shot ids (`StationKey` is
//         `LineStart` / `AfterShot(uuid)`, models/station.rs), so they are
//         id-safe and belong outside the window too. Two of them are the
//         `updateLine` PAIR: one clears name/surveyor/date to explicit null,
//         the next sets all three from null, so both directions of
//         `LineUpdates`' double_option cross the save/reload boundary
//         (issue #413 finding 5).
//   15-19 EVERY id-bearing entry, contiguous, at the top: two `CreateShot`s
//         (bug 8's own variant, both anchor forms), two `UpdateShot`s
//         (`PreviousValues { shot_id }`, whose undo calls
//         `update_shot(line, stale_id)` and whose redo re-executes
//         `Command::UpdateShot` with the same id) and a `SetShotKind` (also
//         `PreviousValues` since #413, and id-bearing on the redo). Their undo
//         data now also records the target's `shot_uuid`, which is what the
//         restore paths address by - the ids are still written, still stale
//         after the reload, and still what a regression would fall back to.
//
// `MAX_PRE_SAVE_UNDOS` is 4 and this block is 5 entries deep, so wherever the
// pre-save undos leave the cursor, the reload lands INSIDE the block and the
// unwind starts on an id-bearing entry against the loader's renumbered ids.
// ---------------------------------------------------------------------------

export const COVERAGE_STEPS: readonly CoverageStep[] = [
  {
    name: 'createLine',
    variant: 'CreatedLineSnapshot',
    guaranteed: true,
    purpose: 'CreatedLineSnapshot { line_id, line_data }, and the fresh target line',
    remembers: 'target',
    build(ctx) {
      const data = nominalLine(ctx);
      return { action: 'createLine', args: [data], label: 'createLine(target,shots=3)' };
    },
  },
  {
    name: 'createLineSpare',
    variant: 'CreatedLineSnapshot',
    guaranteed: true,
    purpose:
      'a second fresh line, so the DeleteShot pair path has a victim that is NOT the target. ' +
      'A delete leaves the line with fewer shots than ids handed out, and the plan needs the ' +
      'targets ids to stay inside the range the loader renumbers into (see deleteInTarget)',
    remembers: 'spare',
    build(ctx) {
      const data = nominalLine(ctx);
      return { action: 'createLine', args: [data], label: 'createLine(spare,shots=3)' };
    },
  },
  {
    name: 'deleteShotMid',
    variant: 'Delta',
    guaranteed: true,
    purpose:
      'the DeleteShot ProjectSnapshotPair path. Runs on the SPARE line by default; ' +
      'STALAGMIGHT_PERSIST_FUZZ_DELETE_IN_TARGET=1 moves it onto the target, which is the ' +
      'repro for finding 13 (the stale-id wedge)',
    build(ctx, digest, memo) {
      const lineId = deleteInTarget() ? target(memo) : memo.spareLineId;
      if (lineId === null) return null;
      const shotIds = shotsOf(digest, lineId);
      if (shotIds.length < 3) return null;
      // A middle shot, never an end one: removing an end shot leaves the
      // remaining ids in positional order again on one side.
      const shotId = shotIds[1 + ctx.randInt(0, shotIds.length - 3)];
      if (shotId === undefined) return null;
      return {
        action: 'deleteShot',
        args: [lineId, shotId],
        label: `deleteShot(s=${lineId},shot=${shotId})`,
      };
    },
  },
  {
    name: 'moveStation',
    variant: 'Delta',
    guaranteed: true,
    purpose:
      'a snapshot-captured entry recorded BEFORE the save (stored as a compact Delta): undoing ' +
      'or redoing it after the reload must neither revert a save heal (finding 11) nor put a ' +
      'PRE-RENUMBER shot id back beside the renumbered ones',
    build(ctx, digest, memo) {
      const lineId = target(memo);
      if (lineId === null) return null;
      const stations = stationsOf(digest, lineId);
      const index = stations[ctx.randInt(0, stations.length - 1)];
      if (index === undefined) return null;
      return {
        action: 'moveStation',
        args: [toRef(lineId, index), nominalOffset(ctx)],
        label: `moveStation(${lineId}/${index})`,
      };
    },
  },
  {
    name: 'reverseLine',
    variant: 'Delta',
    guaranteed: true,
    purpose: 'the ReverseLine pair path, and it re-orders the diverged line once more',
    build(ctx, digest, memo) {
      const lineId = target(memo);
      if (lineId === null) return null;
      if (shotsOf(digest, lineId).length === 0) return null;
      const swap = ctx.rng() < 0.5;
      return {
        action: 'reverseLine',
        args: [lineId, swap],
        label: `reverseLine(s=${lineId},swapLR=${swap})`,
      };
    },
  },
  {
    name: 'executeBatch',
    variant: 'Delta',
    guaranteed: true,
    purpose: 'one entry covering several sub-commands (execute_batch pushes its own pair)',
    build(ctx, digest, memo) {
      const lineId = target(memo);
      if (lineId === null) return null;
      const shotIds = shotsOf(digest, lineId).sort((a, b) => a - b);
      const shotId = shotIds[ctx.randInt(0, shotIds.length - 1)];
      if (shotId === undefined) return null;
      return {
        action: 'executeBatch',
        args: [
          [
            {
              type: 'update_shot',
              line_id: lineId,
              shot_id: shotId,
              updates: { length: rounded(ctx, 1, 15, 2) },
            },
            { type: 'move_line', line_id: lineId, offset: nominalOffset(ctx) },
          ],
        ],
        label: `executeBatch(updShot+mvSec,s=${lineId})`,
      };
    },
  },
  {
    name: 'moveLine',
    variant: 'SegmentMoveInfo',
    guaranteed: true,
    purpose: 'SegmentMoveInfo { affected_lines, previous_offsets } - line ids, which DO persist',
    build(ctx, digest, memo) {
      // Runs before the merge and before the zero datum, both of which would
      // refuse it: `Cannot move a segment whose rigid group holds a zero datum`
      // (core/state/command_move.rs) applies to the whole rigid group, so once
      // the fresh line is merged into a fixture component that carries one
      // (naharon ships several) the move is rejected and the variant is lost.
      const lineId = target(memo);
      if (lineId === null) return null;
      return {
        action: 'moveLine',
        args: [lineId, nominalOffset(ctx)],
        label: `moveLine(s=${lineId})`,
      };
    },
  },
  {
    name: 'mergeStations',
    variant: 'Delta',
    guaranteed: true,
    purpose:
      'the MergeStations pair path, and the connectivity createLoopClosure needs. One side is ' +
      'always the FRESH line, so "stations are already connected" cannot refuse it',
    build(ctx, digest, memo) {
      const lineId = target(memo);
      if (lineId === null) return null;
      const own = stationsOf(digest, lineId);
      const ownIndex = own[own.length - 1];
      const others = sortedLines(digest).filter(
        (entry) => entry[0] !== lineId && entry[1].length > 0,
      );
      const other = others[ctx.randInt(0, others.length - 1)];
      if (ownIndex === undefined || other === undefined) return null;
      const otherStations = stationsOf(digest, other[0]);
      const otherIndex = otherStations[ctx.randInt(0, otherStations.length - 1)];
      if (otherIndex === undefined) return null;
      return {
        action: 'mergeStations',
        args: [toRef(lineId, ownIndex), toRef(other[0], otherIndex)],
        label: `mergeStations(${lineId}/${ownIndex},${other[0]}/${otherIndex})`,
      };
    },
  },
  {
    name: 'createLoopClosure',
    variant: 'ClosureInfo',
    guaranteed: false,
    purpose:
      'ClosureInfo from the CREATE side. Topology-dependent: the merge above supplies the ' +
      'connectivity, but the pair must also be unmerged and not already closed',
    build(ctx, digest, memo) {
      memo.closureIdsBefore = [...digest.c].sort((a, b) => a - b);
      const lineId = target(memo);
      if (lineId === null) return null;
      const own = stationsOf(digest, lineId);
      // Station 0 of the fresh line: the merge above used its LAST station,
      // so this pair is connected but not itself merged.
      const ownIndex = own[0];
      const merged = [...digest.m].sort(
        (a, b) => a[0] - b[0] || a[1] - b[1] || a[2] - b[2] || a[3] - b[3],
      );
      const link = merged.find((entry) => entry[0] === lineId || entry[2] === lineId);
      if (ownIndex === undefined || link === undefined) return null;
      const otherLine = link[0] === lineId ? link[2] : link[0];
      const otherMergedIndex = link[0] === lineId ? link[3] : link[1];
      const otherStations = stationsOf(digest, otherLine).filter(
        (index) => index !== otherMergedIndex,
      );
      const otherIndex = otherStations[ctx.randInt(0, otherStations.length - 1)];
      if (otherIndex === undefined) return null;
      return {
        action: 'createLoopClosure',
        args: [toRef(lineId, ownIndex), toRef(otherLine, otherIndex), 'Move'],
        label: `createLoopClosure(${lineId}/${ownIndex},${otherLine}/${otherIndex})`,
      };
    },
  },
  {
    name: 'removeLoopClosure',
    variant: 'ClosureInfo',
    guaranteed: false,
    purpose:
      'ClosureInfo from the REMOVE side (a different arm of the same match in undo.rs). ' +
      'Topology-dependent: it needs a closure to exist, which is either the one above or one ' +
      'the fixture shipped',
    build(ctx, digest, memo) {
      const ids = [...digest.c].sort((a, b) => a - b);
      // Prefer the closure the step above created. Removing it puts the target
      // line back to being a pendant of the survey graph, so the shot inserts
      // that follow are not inserts INTO A LOOP - which `delete_shot` refused,
      // wedging their undo (bug 6, fixed in 72595cc6; the preference is kept
      // because it keeps the plan's shape independent of that fix). On a fixture
      // that shipped closures the
      // fallback is any of them, which is still a real ClosureInfo entry.
      const fresh = ids.filter((id) => !memo.closureIdsBefore.includes(id));
      const pool = fresh.length > 0 ? fresh : ids;
      const id = pool[ctx.randInt(0, pool.length - 1)];
      if (id === undefined) return null;
      return {
        action: 'removeLoopClosure',
        args: [id],
        label: `removeLoopClosure(${id}${fresh.length > 0 ? ',just-created' : ',pre-existing'})`,
      };
    },
  },
  {
    name: 'zeroDatum',
    variant: 'ZeroDatumInfo',
    guaranteed: true,
    purpose:
      'ZeroDatumInfo { station_ref, previous_gps, previous_offsets }. Adaptive: a fixture that ' +
      'already has datums (naharon) gets a REMOVE, one that has none (fractal) gets a SET, so ' +
      'the variant is reached on both without tripping the one-datum-per-component guard',
    build(ctx, digest, memo) {
      const existing = [...digest.z].sort((a, b) => a[0] - b[0] || a[1] - b[1]);
      const first = existing[0];
      if (first !== undefined) {
        return {
          action: 'removeZeroDatum',
          args: [toRef(first[0], first[1])],
          label: `removeZeroDatum(${first[0]}/${first[1]})`,
        };
      }
      const lineId = target(memo);
      if (lineId === null) return null;
      const gps = {
        latitude: rounded(ctx, 20.2, 20.5, 6),
        longitude: rounded(ctx, -87.5, -87.2, 6),
        depth_reference: rounded(ctx, 0, 5, 2),
      };
      return {
        action: 'setZeroDatum',
        args: [toRef(lineId, 0), gps, { fitToBounds: false }],
        label: `setZeroDatum(${lineId}/0)`,
      };
    },
  },
  {
    name: 'updateLineClear',
    variant: 'PreviousValues(line)',
    guaranteed: true,
    purpose:
      'PreviousValues { line_id, shot_id: None } that CLEARS name/surveyor/date to explicit ' +
      'null. Forward: Some(None) must serialize as an explicit null in the persisted command, so ' +
      'the redo re-clears after a reload. Undo: the capture is Some(Some(old)) and must put all ' +
      'three back. Issue #413 finding 5 broke both halves',
    build(ctx, digest, memo) {
      const lineId = target(memo);
      if (lineId === null) return null;
      // Explicit nulls, deliberately. `createLine` gave the target a
      // non-null name/surveyor/date (`nominalLine`), so this step is the
      // value -> null direction and the step after it is the null -> value one.
      // Together they cover both sides of LineUpdates' double_option.
      return {
        action: 'updateLine',
        args: [lineId, { name: null, surveyor: null, date: null, quality: ctx.randInt(1, 5) }],
        label: `updateLine(s=${lineId},clear name/surveyor/date)`,
      };
    },
  },
  {
    name: 'updateLine',
    variant: 'PreviousValues(line)',
    guaranteed: true,
    purpose:
      'PreviousValues { line_id, shot_id: None } over EVERY updatable field, on a line ' +
      'whose name/surveyor/date the step before it set to null - so the capture is Some(None) ' +
      'and the undo has to restore a previously-null value, which is finding 5 exactly',
    build(ctx, digest, memo) {
      const lineId = target(memo);
      if (lineId === null) return null;
      // All six fields, including the three nullable ones. They were EXCLUDED
      // here while issue #413 finding 5 was open: `LineUpdates` carried only
      // `#[serde(default)]` on name/surveyor/date, so a captured `Some(None)`
      // ("this line had none, restore that") serialized to the same `null` as
      // `None` ("leave it alone") and read back as the latter - the undo left the
      // new value in place. Every iteration would have failed on that in-session
      // defect before the persistence walk it exists to run had started.
      //
      // Fixed in b9c9d649 (double_option + skip_serializing_if, schema v17), so
      // the restriction is lifted: this is the harness-tier, end-to-end proof
      // that a Some(None) line field survives capture -> save -> reload ->
      // undo -> redo. The oracle was never relaxed for it - an unexpected change
      // to any of these fields always failed - only the INPUT was narrowed.
      return {
        action: 'updateLine',
        args: [
          lineId,
          {
            name: `persist-renamed-${ctx.randInt(1000, 9999)}`,
            surveyor: `persist-surveyor-${ctx.randInt(1, 9)}`,
            date: `20${ctx.randInt(10, 26)}-0${ctx.randInt(1, 9)}-1${ctx.randInt(0, 9)}`,
            color: ctx.pick(COLORS),
            direction: ctx.pick(SURVEY_DIRECTIONS),
            quality: ctx.randInt(1, 5),
          },
        ],
        label: `updateLine(s=${lineId},name/surveyor/date from null + color/direction/quality)`,
      };
    },
  },
  {
    name: 'updateStationComment',
    variant: 'InverseCommand',
    guaranteed: true,
    purpose: 'InverseCommand(SetStationComment { station_ref }) - a positional ref, not an id',
    build(ctx, digest, memo) {
      const lineId = target(memo);
      if (lineId === null) return null;
      const stations = stationsOf(digest, lineId);
      const index = stations[ctx.randInt(0, stations.length - 1)];
      if (index === undefined) return null;
      return {
        action: 'updateStationComment',
        args: [toRef(lineId, index), `persist-station-${ctx.randInt(0, 9999)}`],
        label: `updateStationComment(${lineId}/${index})`,
      };
    },
  },
  {
    name: 'createShotBefore',
    variant: 'CreatedShotSnapshot',
    guaranteed: true,
    purpose:
      'CreatedShotSnapshot with a BEFORE anchor: the id comes from the allocator while the ' +
      'position is 0, so ids stop matching positional order - bug 8s precondition',
    build(ctx, digest, memo) {
      const lineId = target(memo);
      if (lineId === null) return null;
      const shotIds = shotsOf(digest, lineId).sort((a, b) => a - b);
      const beforeId = shotIds[0];
      if (beforeId === undefined) return null;
      return {
        action: 'createShot',
        args: [lineId, nominalShot(ctx), null, beforeId],
        label: `createShot(s=${lineId},before=${beforeId})`,
      };
    },
  },
  {
    name: 'createShotAfter',
    variant: 'CreatedShotSnapshot',
    guaranteed: true,
    purpose: 'CreatedShotSnapshot with an AFTER anchor (the other redo path in undo.rs)',
    build(ctx, digest, memo) {
      const lineId = target(memo);
      if (lineId === null) return null;
      const shotIds = shotsOf(digest, lineId).sort((a, b) => a - b);
      const afterId = shotIds[Math.floor(shotIds.length / 2)];
      if (afterId === undefined) return null;
      return {
        action: 'createShot',
        args: [lineId, nominalShot(ctx), afterId],
        label: `createShot(s=${lineId},after=${afterId})`,
      };
    },
  },
  {
    name: 'updateShot',
    variant: 'PreviousValues(shot)',
    guaranteed: true,
    purpose:
      'PreviousValues { line_id, shot_id: Some(..) } on the DIVERGED line: undo calls ' +
      'update_shot(line, stale_id) and redo re-executes Command::UpdateShot with the same id',
    build(ctx, digest, memo) {
      const lineId = target(memo);
      if (lineId === null) return null;
      const collide = pickCollidingShot(ctx, digest, lineId);
      const shotIds = shotsOf(digest, lineId).sort((a, b) => a - b);
      const shotId = collide?.shotId ?? shotIds[ctx.randInt(0, shotIds.length - 1)];
      if (shotId === undefined) return null;
      return {
        action: 'updateShot',
        args: [
          lineId,
          shotId,
          { length: rounded(ctx, 1, 15, 2), azimuth_in: rounded(ctx, 0, 359.9, 1) },
        ],
        label:
          `updateShot(s=${lineId},shot=${shotId}` +
          `${collide ? `,pos ${collide.position} -> lands on ${collide.lands} after reload` : ',no collision available'})`,
      };
    },
  },
  {
    name: 'setShotKind',
    variant: 'PreviousValues(shot)',
    guaranteed: true,
    purpose:
      'PreviousValues { shot_uuid } from a SetShotKind on the DIVERGED line: the undo ' +
      'restores the previous kind by uuid, and the redo re-executes a Command that still ' +
      'names the shot by its in-session id',
    build(ctx, digest, memo) {
      const lineId = target(memo);
      if (lineId === null) return null;
      const collide = pickCollidingShot(ctx, digest, lineId);
      const shotIds = shotsOf(digest, lineId).sort((a, b) => a - b);
      const shotId = collide?.shotId ?? shotIds[ctx.randInt(0, shotIds.length - 1)];
      if (shotId === undefined) return null;
      // NEVER `Normal`. Every shot the plan creates starts `Normal`, so setting
      // `Normal` would be a forward no-op whose inverse is also a no-op - and an
      // undo that lands on the WRONG shot would then be invisible. `Jump` /
      // `Reference` make the difference observable, which is the whole point.
      const kind = ctx.pick(NON_DEFAULT_SHOT_KINDS);
      return {
        action: 'setShotKind',
        args: [lineId, shotId, kind],
        label:
          `setShotKind(s=${lineId},shot=${shotId},${kind}` +
          `${collide ? `,pos ${collide.position} -> lands on ${collide.lands} after reload` : ',no collision available'})`,
      };
    },
  },
  {
    name: 'updateShotTop',
    variant: 'PreviousValues(shot)',
    guaranteed: true,
    purpose:
      'a SECOND PreviousValues { shot_id } entry, so the id-bearing block at the top of the ' +
      'stack is five deep - one more than MAX_PRE_SAVE_UNDOS. That is what guarantees the ' +
      'reload lands inside the block whatever the pre-save undo count draws',
    build(ctx, digest, memo) {
      const lineId = target(memo);
      if (lineId === null) return null;
      const collide = pickCollidingShot(ctx, digest, lineId);
      const shotIds = shotsOf(digest, lineId).sort((a, b) => a - b);
      const shotId = collide?.shotId ?? shotIds[ctx.randInt(0, shotIds.length - 1)];
      if (shotId === undefined) return null;
      return {
        action: 'updateShot',
        args: [lineId, shotId, { depth_out: rounded(ctx, 0, 30, 2), left: rounded(ctx, 0.1, 4, 2) }],
        label:
          `updateShot(s=${lineId},shot=${shotId},top` +
          `${collide ? `,pos ${collide.position} -> lands on ${collide.lands} after reload` : ',no collision available'})`,
      };
    },
  },
];

/** Every variant the plan promises, in first-appearance order. */
export const PLANNED_VARIANTS: readonly UndoVariant[] = [
  ...new Set(COVERAGE_STEPS.map((step) => step.variant)),
];

/** Variants a guaranteed step promises; a run missing one of these has a broken plan. */
export const GUARANTEED_VARIANTS: readonly UndoVariant[] = [
  ...new Set(COVERAGE_STEPS.filter((step) => step.guaranteed).map((step) => step.variant)),
];

/**
 * True when `shotIds` no longer equals what the LOADER would renumber them to.
 *
 * `line_id * FEATURE_ID_STRIDE + sequence_order` is both what
 * `persistence/sqlite/read.rs` writes on load and where the in-session allocator
 * draws from, so ids that still match positional order would survive a reload
 * unchanged and a stale persisted id would address the same shot it always did.
 * The divergence is the whole precondition, so the scenario asserts it rather
 * than assuming the plan produced it.
 */
export function idsDivergeFromPosition(lineId: number, shotIds: readonly number[]): boolean {
  return shotIds.some((id, index) => id !== lineId * FEATURE_ID_STRIDE + index);
}
