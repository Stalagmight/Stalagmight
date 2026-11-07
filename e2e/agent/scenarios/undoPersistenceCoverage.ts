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
 * different sections, stations, geometry and text while every run makes the same
 * coverage promise. The scenario then re-reads `get_undo_stack` and ASSERTS the
 * promised variants are really there.
 *
 * ## Which variants the app can actually produce
 *
 * `AppState::generate_undo_data` / `finalize_undo_data`
 * (src-tauri/src/core/state/undo_capture.rs) plus the three paths that push
 * their own entry (`execute_batch`, `execute_move_comment`,
 * `repair_open_project_integrity`) are the ONLY producers. Measured against
 * them, six `UndoData` variants are dead as far as a freshly-stripped project is
 * concerned - see `UNREACHABLE_VARIANTS`. They survive in `undo()` as restore
 * arms for entries persisted by older builds, so a scenario that strips the
 * fixture's history can never construct one.
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
 * `section_id * FEATURE_ID_STRIDE + sequence_order`
 * (persistence/sqlite/read.rs), and the in-session allocator hands out ids from
 * the same space - which is why a stale persisted id COLLIDES rather than
 * missing. Mirrored here so the scenario can prove, before it saves, that the
 * ids it is about to persist really have diverged from positional order.
 */
export const FEATURE_ID_STRIDE = 100_000;

/** The `UndoData` variants a live command can produce. */
export type UndoVariant =
  | 'CreatedSectionSnapshot'
  | 'CreatedShotSnapshot'
  | 'PreviousValues(shot)'
  | 'PreviousValues(section)'
  | 'InverseCommand'
  | 'ClosureInfo'
  | 'ZeroDatumInfo'
  | 'SegmentMoveInfo'
  | 'ProjectSnapshotPair';

/**
 * `UndoHistoryItem.commandType` -> the `UndoData` variant that command's entry
 * carries. Transcribed from `undo_capture.rs`; every row is a `match` arm there.
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
 *   - `reverse_section` produces a `ProjectSnapshotPair` only for a section with
 *     shots; a shot-less section reverses to itself and pushes nothing at all
 *     (so no row is created to be mapped). The plan only ever reverses a
 *     populated section.
 */
export const VARIANT_BY_COMMAND_TYPE: Readonly<Record<string, UndoVariant>> = {
  create_section: 'CreatedSectionSnapshot',
  create_shot: 'CreatedShotSnapshot',
  update_shot: 'PreviousValues(shot)',
  update_section_metadata: 'PreviousValues(section)',
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
  remove_zero_datum: 'ZeroDatumInfo',
  move_section: 'SegmentMoveInfo',
  move_segment: 'SegmentMoveInfo',
  delete_section: 'ProjectSnapshotPair',
  delete_shot: 'ProjectSnapshotPair',
  reverse_section: 'ProjectSnapshotPair',
  merge_stations: 'ProjectSnapshotPair',
  unmerge_stations: 'ProjectSnapshotPair',
  unmerge_station_pair: 'ProjectSnapshotPair',
  move_station: 'ProjectSnapshotPair',
  move_comment: 'ProjectSnapshotPair',
  batch: 'ProjectSnapshotPair',
  repair_integrity: 'ProjectSnapshotPair',
};

/**
 * `UndoData` variants that exist in `src-tauri/src/core/undo.rs` and have a
 * restore arm in `undo()`/`redo()`, but which NO live command produces any more.
 *
 * Verified by grepping every construction site outside `#[cfg(test)]`: only
 * `undo_capture.rs` and `app_state.rs` build `UndoData`, and between them they
 * emit exactly the nine variants above plus `None`. Each name below was replaced
 * by a `ProjectSnapshotPair` (the capture comments in `undo_capture.rs` say why:
 * a lightweight record could not restore the cascades the command triggers) or,
 * for `ShotSnapshot`, is documented in `undo()` as serving "legacy persisted
 * entries" only.
 *
 * This matters for the scope of this scenario: a stale-id defect in one of these
 * arms is unreachable from a project whose persisted history was stripped, so
 * the hunt is bounded by this list rather than by the enum.
 */
export const UNREACHABLE_VARIANTS: Readonly<Record<string, string>> = {
  MergeInfo: 'MergeStations captures a ProjectSnapshotPair instead (undo_capture.rs)',
  UnmergeInfo: 'UnmergeStations/UnmergeStationPair capture a ProjectSnapshotPair instead',
  StationMoveInfo: 'MoveStation captures a ProjectSnapshotPair instead',
  ShotSnapshot: 'DeleteShot captures a ProjectSnapshotPair; this arm serves legacy persisted entries',
  SectionSnapshot: 'DeleteSection captures a ProjectSnapshotPair; no live producer',
  ProjectSnapshot: 'superseded by ProjectSnapshotPair; no live producer',
};

/** One `projectStore` action plus its args, exactly as the bridge dispatches it. */
export interface CoverageAction {
  action: string;
  args: unknown[];
  /** Short label for the action history in a failure message. */
  label: string;
}

/**
 * `STALAGMIGHT_PERSIST_FUZZ_DELETE_IN_TARGET=1` moves the plan's mid-section
 * delete onto the TARGET section instead of the spare one.
 *
 * That single change was the repro for finding 13 (the stale-id WEDGE): a
 * section that has had a shot deleted has handed out more ids than it has shots,
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
 * against: with the delete on the SPARE section the target's ids stay inside the
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
   * The section every shot-level step targets.
   *
   * Deliberately a FRESH section rather than one of the fixture's: it is
   * disconnected, has no zero datum and is in no loop, so `delete_shot`'s
   * "part of a loop / anchored to a zero datum" guard cannot refuse the undo of
   * a shot insert. That guard was open bug 6 (fixed in 72595cc6 - an undo now
   * routes through the guard-free `remove_created_shot`), and the choice of a
   * fresh, unanchored section is kept: it is what makes the plan's promises hold
   * on ANY fixture, rather than only on one whose topology happens to cooperate.
   *
   * It also never has a shot DELETED from it (unless `deleteInTarget()` is set),
   * which is what keeps every id it hands out inside the range the loader
   * renumbers into - see that constant for why that matters.
   */
  createdSectionId: number | null;
  /**
   * A second fresh section, whose only job is to be the victim of the plan's
   * mid-section delete without disturbing the target's id space.
   */
  spareSectionId: number | null;
  /** Closure ids present before the plan's `createLoopClosure` step. */
  closureIdsBefore: number[];
}

export function newCoverageMemo(): CoverageMemo {
  return { createdSectionId: null, spareSectionId: null, closureIdsBefore: [] };
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
  /** Set when the step creates a section the plan needs to keep hold of. */
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

/** A valid `SectionData` with 3 shots (enough for a mid-section insert AND a delete). */
function nominalSection(ctx: ScenarioContext): Record<string, unknown> {
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

/** Sections by ascending id, so a pick never depends on `Vec` order. */
function sortedSections(digest: TargetingDigest): [number, number[]][] {
  return [...digest.s].sort((a, b) => a[0] - b[0]);
}

function shotsOf(digest: TargetingDigest, sectionId: number): number[] {
  const found = digest.s.find((entry) => entry[0] === sectionId);
  return found ? [...found[1]] : [];
}

/**
 * A shot whose id will address a DIFFERENT shot after the reload, together with
 * the position it will then land on.
 *
 * The digest lists a section's shot ids in payload (i.e. positional) order, and
 * the loader renumbers position `i` to `sectionId * FEATURE_ID_STRIDE + i`. So a
 * persisted id `ids[i]` addresses position `ids[i] - sectionId * STRIDE` after
 * the reload, and it is a COLLISION exactly when that number is a different, but
 * still valid, position.
 *
 * Targeting such a shot deliberately is what turns "a stale id might hit the
 * wrong shot" into a property the plan guarantees. Picking uniformly instead
 * leaves it to luck: a run where every id happens to sit at its own position
 * exercises the same code and observes nothing, which is how a coverage claim
 * quietly becomes vacuous.
 *
 * Returns null when the section has no colliding shot; callers fall back to any
 * shot rather than dropping the step, since the entry is still worth having.
 */
function pickCollidingShot(
  ctx: ScenarioContext,
  digest: TargetingDigest,
  sectionId: number,
): { shotId: number; position: number; lands: number } | null {
  const ids = shotsOf(digest, sectionId);
  const colliding = ids
    .map((id, position) => ({ shotId: id, position, lands: id - sectionId * FEATURE_ID_STRIDE }))
    .filter((entry) => entry.lands !== entry.position && entry.lands >= 0 && entry.lands < ids.length);
  if (colliding.length === 0) return null;
  return colliding[ctx.randInt(0, colliding.length - 1)] ?? null;
}

/** Station refs inside one section, ascending by index. */
function stationsOf(digest: TargetingDigest, sectionId: number): number[] {
  return digest.st
    .filter((ref) => ref[0] === sectionId)
    .map((ref) => ref[1])
    .sort((a, b) => a - b);
}

function toRef(sectionId: number, stationIndex: number): Record<string, number> {
  return { section_id: sectionId, station_index: stationIndex };
}

/** The created section, or null when `createSection` did not land. */
function target(memo: CoverageMemo): number | null {
  return memo.createdSectionId;
}

// ---------------------------------------------------------------------------
// The plan. The ORDER is the most load-bearing thing in this file, and it is not
// the obvious one. It was corrected after a 60-iteration run showed the first
// arrangement could not expose a single stale-id defect.
//
// ## Why a ProjectSnapshotPair entry is a wall
//
// `Shot::id` IS serialized inside a project snapshot (models/shot.rs has no
// `skip` on it), and `UndoData::ProjectSnapshotPair` restores the whole project
// verbatim (`restore_snapshot_into`, core/state/undo.rs). So the moment a walk
// crosses ONE snapshot-pair entry in either direction, the project is back in
// the PRE-SAVE id space - and every stale id in every entry beyond it is
// suddenly valid again. A snapshot-pair entry heals the very corruption this
// scenario is looking for.
//
// The exposed window is therefore exactly: from the depth the reload lands on,
// down to (not including) the first `ProjectSnapshotPair` beneath it. In the
// first arrangement every snapshot-pair step sat ABOVE the id-bearing ones, so
// that window was two or three entries wide and contained none of them. 60
// iterations of it reported only bugs already known.
//
// ## The arrangement that follows from that
//
//   1-2   the fresh target section and the spare one the delete uses.
//   3-8   every ProjectSnapshotPair step, as LOW in the stack as their
//         preconditions allow, so they are the last thing a walk reaches.
//   9-14  the graph and metadata entries, whose undo data names sections and
//         uuid-anchored stations rather than shot ids (`StationKey` is
//         `SectionStart` / `AfterShot(uuid)`, models/station.rs), so they are
//         id-safe and belong outside the window too. Two of them are the
//         `updateSection` PAIR: one clears name/surveyor/date to explicit null,
//         the next sets all three from null, so both directions of
//         `SectionUpdates`' double_option cross the save/reload boundary
//         (issue #413 finding 5).
//   15-19 EVERY id-bearing entry, contiguous, at the top: two `CreateShot`s
//         (bug 8's own variant, both anchor forms), two `UpdateShot`s
//         (`PreviousValues { shot_id }`, whose undo calls
//         `update_shot(section, stale_id)` and whose redo re-executes
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
    name: 'createSection',
    variant: 'CreatedSectionSnapshot',
    guaranteed: true,
    purpose: 'CreatedSectionSnapshot { section_id, section_data }, and the fresh target section',
    remembers: 'target',
    build(ctx) {
      const data = nominalSection(ctx);
      return { action: 'createSection', args: [data], label: 'createSection(target,shots=3)' };
    },
  },
  {
    name: 'createSectionSpare',
    variant: 'CreatedSectionSnapshot',
    guaranteed: true,
    purpose:
      'a second fresh section, so the DeleteShot pair path has a victim that is NOT the target. ' +
      'A delete leaves the section with fewer shots than ids handed out, and the plan needs the ' +
      'targets ids to stay inside the range the loader renumbers into (see deleteInTarget)',
    remembers: 'spare',
    build(ctx) {
      const data = nominalSection(ctx);
      return { action: 'createSection', args: [data], label: 'createSection(spare,shots=3)' };
    },
  },
  {
    name: 'deleteShotMid',
    variant: 'ProjectSnapshotPair',
    guaranteed: true,
    purpose:
      'the DeleteShot ProjectSnapshotPair path. Runs on the SPARE section by default; ' +
      'STALAGMIGHT_PERSIST_FUZZ_DELETE_IN_TARGET=1 moves it onto the target, which is the ' +
      'repro for finding 13 (the stale-id wedge)',
    build(ctx, digest, memo) {
      const sectionId = deleteInTarget() ? target(memo) : memo.spareSectionId;
      if (sectionId === null) return null;
      const shotIds = shotsOf(digest, sectionId);
      if (shotIds.length < 3) return null;
      // A middle shot, never an end one: removing an end shot leaves the
      // remaining ids in positional order again on one side.
      const shotId = shotIds[1 + ctx.randInt(0, shotIds.length - 3)];
      if (shotId === undefined) return null;
      return {
        action: 'deleteShot',
        args: [sectionId, shotId],
        label: `deleteShot(s=${sectionId},shot=${shotId})`,
      };
    },
  },
  {
    name: 'moveStation',
    variant: 'ProjectSnapshotPair',
    guaranteed: true,
    purpose:
      'a ProjectSnapshotPair recorded BEFORE the save: undoing or redoing it restores a whole ' +
      'pre-save project, which is finding 11s mechanism and also puts the PRE-RENUMBER shot ids ' +
      'back into a project the entries above it were captured against',
    build(ctx, digest, memo) {
      const sectionId = target(memo);
      if (sectionId === null) return null;
      const stations = stationsOf(digest, sectionId);
      const index = stations[ctx.randInt(0, stations.length - 1)];
      if (index === undefined) return null;
      return {
        action: 'moveStation',
        args: [toRef(sectionId, index), nominalOffset(ctx)],
        label: `moveStation(${sectionId}/${index})`,
      };
    },
  },
  {
    name: 'reverseSection',
    variant: 'ProjectSnapshotPair',
    guaranteed: true,
    purpose: 'the ReverseSection pair path, and it re-orders the diverged section once more',
    build(ctx, digest, memo) {
      const sectionId = target(memo);
      if (sectionId === null) return null;
      if (shotsOf(digest, sectionId).length === 0) return null;
      const swap = ctx.rng() < 0.5;
      return {
        action: 'reverseSection',
        args: [sectionId, swap],
        label: `reverseSection(s=${sectionId},swapLR=${swap})`,
      };
    },
  },
  {
    name: 'executeBatch',
    variant: 'ProjectSnapshotPair',
    guaranteed: true,
    purpose: 'one entry covering several sub-commands (execute_batch pushes its own pair)',
    build(ctx, digest, memo) {
      const sectionId = target(memo);
      if (sectionId === null) return null;
      const shotIds = shotsOf(digest, sectionId).sort((a, b) => a - b);
      const shotId = shotIds[ctx.randInt(0, shotIds.length - 1)];
      if (shotId === undefined) return null;
      return {
        action: 'executeBatch',
        args: [
          [
            {
              type: 'update_shot',
              section_id: sectionId,
              shot_id: shotId,
              updates: { length: rounded(ctx, 1, 15, 2) },
            },
            { type: 'move_section', section_id: sectionId, offset: nominalOffset(ctx) },
          ],
        ],
        label: `executeBatch(updShot+mvSec,s=${sectionId})`,
      };
    },
  },
  {
    name: 'moveSection',
    variant: 'SegmentMoveInfo',
    guaranteed: true,
    purpose: 'SegmentMoveInfo { affected_sections, previous_offsets } - section ids, which DO persist',
    build(ctx, _digest, memo) {
      // Runs before the merge and before the zero datum, both of which would
      // refuse it: `Cannot move a segment whose rigid group holds a zero datum`
      // (core/state/command_move.rs) applies to the whole rigid group, so once
      // the fresh section is merged into a fixture component that carries one
      // (naharon ships several) the move is rejected and the variant is lost.
      const sectionId = target(memo);
      if (sectionId === null) return null;
      return {
        action: 'moveSection',
        args: [sectionId, nominalOffset(ctx)],
        label: `moveSection(s=${sectionId})`,
      };
    },
  },
  {
    name: 'mergeStations',
    variant: 'ProjectSnapshotPair',
    guaranteed: true,
    purpose:
      'the MergeStations pair path, and the connectivity createLoopClosure needs. One side is ' +
      'always the FRESH section, so "stations are already connected" cannot refuse it',
    build(ctx, digest, memo) {
      const sectionId = target(memo);
      if (sectionId === null) return null;
      const own = stationsOf(digest, sectionId);
      const ownIndex = own[own.length - 1];
      const others = sortedSections(digest).filter(
        (entry) => entry[0] !== sectionId && entry[1].length > 0,
      );
      const other = others[ctx.randInt(0, others.length - 1)];
      if (ownIndex === undefined || other === undefined) return null;
      const otherStations = stationsOf(digest, other[0]);
      const otherIndex = otherStations[ctx.randInt(0, otherStations.length - 1)];
      if (otherIndex === undefined) return null;
      return {
        action: 'mergeStations',
        args: [toRef(sectionId, ownIndex), toRef(other[0], otherIndex)],
        label: `mergeStations(${sectionId}/${ownIndex},${other[0]}/${otherIndex})`,
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
      const sectionId = target(memo);
      if (sectionId === null) return null;
      const own = stationsOf(digest, sectionId);
      // Station 0 of the fresh section: the merge above used its LAST station,
      // so this pair is connected but not itself merged.
      const ownIndex = own[0];
      const merged = [...digest.m].sort(
        (a, b) => a[0] - b[0] || a[1] - b[1] || a[2] - b[2] || a[3] - b[3],
      );
      const link = merged.find((entry) => entry[0] === sectionId || entry[2] === sectionId);
      if (ownIndex === undefined || link === undefined) return null;
      const otherSection = link[0] === sectionId ? link[2] : link[0];
      const otherMergedIndex = link[0] === sectionId ? link[3] : link[1];
      const otherStations = stationsOf(digest, otherSection).filter(
        (index) => index !== otherMergedIndex,
      );
      const otherIndex = otherStations[ctx.randInt(0, otherStations.length - 1)];
      if (otherIndex === undefined) return null;
      return {
        action: 'createLoopClosure',
        args: [toRef(sectionId, ownIndex), toRef(otherSection, otherIndex), 'Move'],
        label: `createLoopClosure(${sectionId}/${ownIndex},${otherSection}/${otherIndex})`,
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
      // section back to being a pendant of the survey graph, so the shot inserts
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
      const sectionId = target(memo);
      if (sectionId === null) return null;
      const gps = {
        latitude: rounded(ctx, 20.2, 20.5, 6),
        longitude: rounded(ctx, -87.5, -87.2, 6),
        depth_reference: rounded(ctx, 0, 5, 2),
      };
      return {
        action: 'setZeroDatum',
        args: [toRef(sectionId, 0), gps, { fitToBounds: false }],
        label: `setZeroDatum(${sectionId}/0)`,
      };
    },
  },
  {
    name: 'updateSectionClear',
    variant: 'PreviousValues(section)',
    guaranteed: true,
    purpose:
      'PreviousValues { section_id, shot_id: None } that CLEARS name/surveyor/date to explicit ' +
      'null. Forward: Some(None) must serialize as an explicit null in the persisted command, so ' +
      'the redo re-clears after a reload. Undo: the capture is Some(Some(old)) and must put all ' +
      'three back. Issue #413 finding 5 broke both halves',
    build(ctx, _digest, memo) {
      const sectionId = target(memo);
      if (sectionId === null) return null;
      // Explicit nulls, deliberately. `createSection` gave the target a
      // non-null name/surveyor/date (`nominalSection`), so this step is the
      // value -> null direction and the step after it is the null -> value one.
      // Together they cover both sides of SectionUpdates' double_option.
      return {
        action: 'updateSection',
        args: [sectionId, { name: null, surveyor: null, date: null, quality: ctx.randInt(1, 5) }],
        label: `updateSection(s=${sectionId},clear name/surveyor/date)`,
      };
    },
  },
  {
    name: 'updateSection',
    variant: 'PreviousValues(section)',
    guaranteed: true,
    purpose:
      'PreviousValues { section_id, shot_id: None } over EVERY updatable field, on a section ' +
      'whose name/surveyor/date the step before it set to null - so the capture is Some(None) ' +
      'and the undo has to restore a previously-null value, which is finding 5 exactly',
    build(ctx, _digest, memo) {
      const sectionId = target(memo);
      if (sectionId === null) return null;
      // All six fields, including the three nullable ones. They were EXCLUDED
      // here while issue #413 finding 5 was open: `SectionUpdates` carried only
      // `#[serde(default)]` on name/surveyor/date, so a captured `Some(None)`
      // ("this section had none, restore that") serialized to the same `null` as
      // `None` ("leave it alone") and read back as the latter - the undo left the
      // new value in place. Every iteration would have failed on that in-session
      // defect before the persistence walk it exists to run had started.
      //
      // Fixed in b9c9d649 (double_option + skip_serializing_if, schema v17), so
      // the restriction is lifted: this is the harness-tier, end-to-end proof
      // that a Some(None) section field survives capture -> save -> reload ->
      // undo -> redo. The oracle was never relaxed for it - an unexpected change
      // to any of these fields always failed - only the INPUT was narrowed.
      return {
        action: 'updateSection',
        args: [
          sectionId,
          {
            name: `persist-renamed-${ctx.randInt(1000, 9999)}`,
            surveyor: `persist-surveyor-${ctx.randInt(1, 9)}`,
            date: `20${ctx.randInt(10, 26)}-0${ctx.randInt(1, 9)}-1${ctx.randInt(0, 9)}`,
            color: ctx.pick(COLORS),
            direction: ctx.pick(SURVEY_DIRECTIONS),
            quality: ctx.randInt(1, 5),
          },
        ],
        label: `updateSection(s=${sectionId},name/surveyor/date from null + color/direction/quality)`,
      };
    },
  },
  {
    name: 'updateStationComment',
    variant: 'InverseCommand',
    guaranteed: true,
    purpose: 'InverseCommand(SetStationComment { station_ref }) - a positional ref, not an id',
    build(ctx, digest, memo) {
      const sectionId = target(memo);
      if (sectionId === null) return null;
      const stations = stationsOf(digest, sectionId);
      const index = stations[ctx.randInt(0, stations.length - 1)];
      if (index === undefined) return null;
      return {
        action: 'updateStationComment',
        args: [toRef(sectionId, index), `persist-station-${ctx.randInt(0, 9999)}`],
        label: `updateStationComment(${sectionId}/${index})`,
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
      const sectionId = target(memo);
      if (sectionId === null) return null;
      const shotIds = shotsOf(digest, sectionId).sort((a, b) => a - b);
      const beforeId = shotIds[0];
      if (beforeId === undefined) return null;
      return {
        action: 'createShot',
        args: [sectionId, nominalShot(ctx), null, beforeId],
        label: `createShot(s=${sectionId},before=${beforeId})`,
      };
    },
  },
  {
    name: 'createShotAfter',
    variant: 'CreatedShotSnapshot',
    guaranteed: true,
    purpose: 'CreatedShotSnapshot with an AFTER anchor (the other redo path in undo.rs)',
    build(ctx, digest, memo) {
      const sectionId = target(memo);
      if (sectionId === null) return null;
      const shotIds = shotsOf(digest, sectionId).sort((a, b) => a - b);
      const afterId = shotIds[Math.floor(shotIds.length / 2)];
      if (afterId === undefined) return null;
      return {
        action: 'createShot',
        args: [sectionId, nominalShot(ctx), afterId],
        label: `createShot(s=${sectionId},after=${afterId})`,
      };
    },
  },
  {
    name: 'updateShot',
    variant: 'PreviousValues(shot)',
    guaranteed: true,
    purpose:
      'PreviousValues { section_id, shot_id: Some(..) } on the DIVERGED section: undo calls ' +
      'update_shot(section, stale_id) and redo re-executes Command::UpdateShot with the same id',
    build(ctx, digest, memo) {
      const sectionId = target(memo);
      if (sectionId === null) return null;
      const collide = pickCollidingShot(ctx, digest, sectionId);
      const shotIds = shotsOf(digest, sectionId).sort((a, b) => a - b);
      const shotId = collide?.shotId ?? shotIds[ctx.randInt(0, shotIds.length - 1)];
      if (shotId === undefined) return null;
      return {
        action: 'updateShot',
        args: [
          sectionId,
          shotId,
          { length: rounded(ctx, 1, 15, 2), azimuth_in: rounded(ctx, 0, 359.9, 1) },
        ],
        label:
          `updateShot(s=${sectionId},shot=${shotId}` +
          `${collide ? `,pos ${collide.position} -> lands on ${collide.lands} after reload` : ',no collision available'})`,
      };
    },
  },
  {
    name: 'setShotKind',
    variant: 'PreviousValues(shot)',
    guaranteed: true,
    purpose:
      'PreviousValues { shot_uuid } from a SetShotKind on the DIVERGED section: the undo ' +
      'restores the previous kind by uuid, and the redo re-executes a Command that still ' +
      'names the shot by its in-session id',
    build(ctx, digest, memo) {
      const sectionId = target(memo);
      if (sectionId === null) return null;
      const collide = pickCollidingShot(ctx, digest, sectionId);
      const shotIds = shotsOf(digest, sectionId).sort((a, b) => a - b);
      const shotId = collide?.shotId ?? shotIds[ctx.randInt(0, shotIds.length - 1)];
      if (shotId === undefined) return null;
      // NEVER `Normal`. Every shot the plan creates starts `Normal`, so setting
      // `Normal` would be a forward no-op whose inverse is also a no-op - and an
      // undo that lands on the WRONG shot would then be invisible. `Jump` /
      // `Reference` make the difference observable, which is the whole point.
      const kind = ctx.pick(NON_DEFAULT_SHOT_KINDS);
      return {
        action: 'setShotKind',
        args: [sectionId, shotId, kind],
        label:
          `setShotKind(s=${sectionId},shot=${shotId},${kind}` +
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
      const sectionId = target(memo);
      if (sectionId === null) return null;
      const collide = pickCollidingShot(ctx, digest, sectionId);
      const shotIds = shotsOf(digest, sectionId).sort((a, b) => a - b);
      const shotId = collide?.shotId ?? shotIds[ctx.randInt(0, shotIds.length - 1)];
      if (shotId === undefined) return null;
      return {
        action: 'updateShot',
        args: [sectionId, shotId, { depth_out: rounded(ctx, 0, 30, 2), left: rounded(ctx, 0.1, 4, 2) }],
        label:
          `updateShot(s=${sectionId},shot=${shotId},top` +
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
 * `section_id * FEATURE_ID_STRIDE + sequence_order` is both what
 * `persistence/sqlite/read.rs` writes on load and where the in-session allocator
 * draws from, so ids that still match positional order would survive a reload
 * unchanged and a stale persisted id would address the same shot it always did.
 * The divergence is the whole precondition, so the scenario asserts it rather
 * than assuming the plan produced it.
 */
export function idsDivergeFromPosition(sectionId: number, shotIds: readonly number[]): boolean {
  return shotIds.some((id, index) => id !== sectionId * FEATURE_ID_STRIDE + index);
}
