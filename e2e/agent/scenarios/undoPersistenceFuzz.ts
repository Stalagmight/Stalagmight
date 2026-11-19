/**
 * undo-persistence-fuzz: hunts bug 8's SIBLINGS across the save/reload boundary.
 *
 * Where `undo-redo-fuzz` draws random moves and crosses the persistence boundary
 * occasionally, this scenario does one thing on purpose, every iteration:
 * build an undo stack that carries EVERY UndoData variant a live command can
 * produce, make the shot ids diverge from positional order, leave the redo stack
 * non-empty, save/close/reopen, and then walk the whole RESTORED history down to
 * depth 0 and all the way back up - including through the restored REDO entries,
 * which nothing else in the harness has ever exercised.
 *
 * ## Why
 *
 * Bug 8 (issue #413, the data-corruption one) is three facts stacked:
 *
 *   1. shots are RENUMBERED on load to `line_id * FEATURE_ID_STRIDE +
 *      sequence_order` (persistence/sqlite/read.rs:372);
 *   2. undo entries are persisted and reloaded as OPAQUE JSON with no remapping
 *      anywhere on the restore path (persistence/sqlite/undo.rs); and
 *   3. the in-session allocator (`Line::take_shot_id`) hands out ids from the
 *      SAME space, so a stale id does not miss - it COLLIDES.
 *
 * Nothing about that reasoning is specific to `CreatedShotSnapshot`. Every
 * variant that names a shot by id is exposed, and so is every redo that
 * re-executes the ORIGINAL command (`redo_inner`'s catch-all arm,
 * core/state/undo.rs, re-runs `entry.command` for `PreviousValues`,
 * `InverseCommand` and `ZeroDatumInfo`). The main fuzzer rarely unwinds deep into
 * a restored stack and NEVER redoes a restored redo entry, so those siblings were
 * unreachable. This scenario reaches them by construction.
 *
 * ## What one iteration does
 *
 * 1. Open a fresh copy of the fixture with its persisted undo history DELETED
 *    (`loadFixtureWithoutUndoHistory`), so depth is measured from a known floor.
 *    `STALAGMIGHT_FUZZ_FIXTURE` picks the fixture, exactly as the main fuzzer.
 * 2. Run the COVERAGE PLAN (`undoPersistenceCoverage.ts`): a fixed sequence of
 *    mutations, with seeded targets and values, that puts every reachable
 *    `UndoData` variant on the stack. The plan is then CHECKED against
 *    `get_undo_stack`, not assumed.
 * 3. Assert the id-vs-position divergence really happened. A line whose ids
 *    still equal `line_id * STRIDE + position` would survive the reload
 *    unchanged, and the whole collision precondition would be absent.
 * 4. Undo a few entries, so the REDO stack is non-empty when the save runs (both
 *    stacks are persisted, and the redo half restores too).
 * 5. Save to a temp path, close, reopen. Both stack lengths must come back
 *    exactly; the model switches to id-agnostic comparison here, because
 *    `Shot::id` is not persisted.
 * 6. Unwind through EVERY restored undo entry to depth 0, then redo back to the
 *    top - through the restored redo entries as well - verifying the snapshot
 *    recorded at every depth in both directions.
 *
 * Iterations are independent: each one re-opens the fixture and resets the
 * in-page model, because the persisted window (`undo_persist_limit`, 20) means a
 * stack deeper than the plan could not round trip intact.
 *
 * ## Oracles
 *
 * The same three the main fuzzer runs, through the same in-page bridge
 * (`undoRedoFuzzBridge.ts`): the exact canonical-state ROUNDTRIP per depth, the
 * ABSOLUTE invariant suite on every observed state, and the INTEGRITY DELTA.
 * Plus two this scenario adds:
 *
 *   - COVERAGE. Every variant a guaranteed plan step promises must be present on
 *     the restored stack, or the run fails: a coverage claim that is not checked
 *     is not a coverage claim.
 *   - STACK SURVIVAL. `undoLen` and `redoLen` after the reopen must equal what
 *     they were at save time.
 *
 * ## The known bugs that used to fire here by construction - all fixed, all strict
 *
 * This scenario carried four tolerances, because the defects it hunted fired
 * near the TOP of the restored stack and would otherwise have masked everything
 * beneath them. Every one of those defects is fixed, so every tolerance is
 * DELETED and the whole walk is strict: any mismatch, any wedge, any integrity
 * divergence fails the run outright.
 *
 * The one subtraction still in the file is not a tolerance: the documented
 * save/reload attribution, which names on the record every change it absorbs and
 * the largest numeric move among them, resyncs the model at that depth (so the
 * integrity verdict the resync superseded is skipped rather than compared), and
 * is refused outright when the diff came back CAPPED, because a prefix cannot be
 * spoken for.
 *
 * What was removed, and what replaced it:
 *
 *   - FINDING 11 (`pre-save-snapshot-undo-reverts-save-heals`, classifier
 *     deleted; fixed in 94896857, completed by b9d6b14b). A
 *     `ProjectSnapshotPair` recorded BEFORE the save held the pre-heal project in
 *     both halves, so redoing one reverted the save's gauge-anchor capture. This
 *     scenario saves with a stack of pre-save entries under it and then redoes
 *     all of them, so it fired every iteration. The save heals are factored into
 *     `apply_save_heals` and re-applied by every snapshot restore, so a restored
 *     project meets a saved one's standard.
 *     `undo_of_a_pre_save_entry_keeps_the_saves_gauge_anchors` is un-ignored and
 *     green.
 *
 *     The re-apply is a CARRY-FORWARD from the live project, which left a residue
 *     that `undo-redo-fuzz` found and this scenario never could: it cannot
 *     restore anchors to a line the snapshot RE-CREATES, because nothing live
 *     holds them. That shape is out of this plan's reach - its snapshot pairs edit
 *     lines that stay put - which is why it passed while half of
 *     `undo-redo-fuzz`'s seeds did not. b9d6b14b closes it by making snapshot
 *     pairs SELF-REFRESHING walls (an undo re-records the entry's `after` half
 *     from the live project before restoring `before`, and a redo mirrors it), so
 *     pair undo/redo are exact inverses by construction rather than by re-healing
 *     afterwards. Entry 11 in `undoRedoFuzz.ts` carries the 2-step reduction.
 *
 *   - BUG 8 (`bug-8-wrong-shot-delete`, classifier deleted; fixed in b9c9d649).
 *     Undo of a `CreateShot` deleted the wrong shot after a reload, because
 *     `UndoData::CreatedShotSnapshot { shot_id }` addressed a shot by an
 *     in-session id the loader had renumbered. Undo data now carries the target
 *     shot's `Shot::uuid` and every restore arm resolves by it.
 *
 *   - FINDINGS 12 and 13, this scenario's own (see below; both fixed in
 *     b9c9d649). Their classifiers - `stale-id-writes-to-the-wrong-shot` plus
 *     its three derived-geometry siblings, and the
 *     `Shot N not found in line S` wedge signature - are deleted with them.
 *
 *   - CONTAMINATION, deleted with the defects that motivated it. Once a stale-id
 *     defect had fired, the live project no longer matched any snapshot recorded
 *     before the save, so the finding-12 classifiers were extended to the rest of
 *     the walk and, failing those, the ROUNDTRIP and INTEGRITY-DELTA oracles were
 *     downgraded to counters. That was a real reduction in what a walk could
 *     prove, taken because stopping at the first defect proved strictly less.
 *     With nothing corrupting the project mid-walk, both oracles are hard for
 *     every depth in both directions, which is what this scenario should always
 *     have been able to say. The `contaminatedMismatch` /
 *     `contaminatedIntegrityDelta` counters are gone with the mechanism; if a
 *     future defect needs the same treatment, note that the measurement that
 *     justified it was "`contaminatedMismatch` = 0 over 60 fractal iterations" -
 *     i.e. the downgrade never actually fired, every one of the 1054 tolerated
 *     mismatches was still explained by a named classifier.
 *
 * ## Findings
 *
 * Two, both new, both root-caused and now pinned GREEN in
 * `src-tauri/src/core/state/open_undo_bugs_tests.rs`, both written up at
 * <https://github.com/Stalagmight/Stalagmight/issues/413>. They are the same
 * root cause as bug 8 - no id remapping on the undo restore path - in variants
 * and directions nothing had reached before.
 *
 * BOTH ARE FIXED (b9c9d649), together with bugs 8 and 16, and the two tests
 * below are no longer `#[ignore]`d. The undo data now records the target shot's `Shot::uuid`
 * and every restore arm resolves by it, with the in-session id kept only as the
 * legacy fallback for entries persisted before those fields existed - so an id
 * that the loader's renumbering has reassigned can no longer decide which shot
 * an undo hits (see `docs/undo-redo.md`, "Undo data addresses a shot by uuid").
 * `SetShotKind` consequently records `PreviousValues` rather than
 * `InverseCommand`, which is a newtype around `Command` and had nowhere to put a
 * uuid. The mechanisms below are kept as the record of what the scenario found
 * and of what it still guards against.
 *
 * 12. DATA CORRUPTION, quiet. After a save/reload, undo (or redo) of
 *     `SetShotKind` or `UpdateShot` applies to the WRONG SHOT.
 *     `UndoData::InverseCommand(Command::SetShotKind { shot_id })` and
 *     `UndoData::PreviousValues { shot_id }` both name a shot by its in-session
 *     id (undo_capture.rs:172-185 and :94-135); undo re-executes them against the
 *     live project (undo.rs:60-62, :65-88) and `redo_inner`'s catch-all arm
 *     re-executes `entry.command` verbatim (undo.rs:696), which carries the same
 *     id. Nothing is deleted, no uuid changes, no count moves and
 *     `check_project_integrity` is silent: one shot keeps an edit the undo should
 *     have taken away and another gains one it never had.
 *
 *     Needs no delete, unlike bug 8 - one insert-before is enough.
 *
 *     Repro, kept as the regression check and now expected CLEAN:
 *     `--seed 1 --iterations 1` on fractal (what the `agent:scenarios` smoke
 *     entry runs). It used to fail on the first id-bearing entry it unwound:
 *
 *         undo did not restore the state recorded at depth 16
 *           (the entry it consumed was set_shot_kind)
 *           at project.lines[32].shots[0].kind
 *             expected: "Normal"   actual: "Jump"
 *
 *     What is NOT affected, which bounds the fix: `SegmentMoveInfo` and
 *     `ZeroDatumInfo` name lines and line ids ARE persisted; `ClosureInfo`
 *     and every stored station reference use `StationKey`
 *     (`LineStart` / `AfterShot(uuid)`, models/station.rs), which is
 *     uuid-anchored on purpose. `ProjectSnapshotPair` has restore arms but no
 *     live entry holds one - see `UNREACHABLE_VARIANTS`.
 *
 * 13. WEDGED HISTORY. Same mechanism, opposite outcome. A line that has had a
 *     shot DELETED has handed out more ids than it has shots, so the newest
 *     persisted id ran PAST `line_id * STRIDE + (len - 1)` and missed
 *     everything: `delete_shot` / `update_shot` / `set_shot_kind` returned
 *     "Shot N not found in line S", the undo failed, and `undo_inner`'s
 *     epilogue pushed the entry back - so it failed identically forever and
 *     every entry beneath it was unreachable for good.
 *
 *     Bug 8's write-up says a stale id "does not miss - it COLLIDES". True only
 *     while the line still has as many shots as ids were handed out; a delete
 *     breaks that and switched the whole defect into this regime.
 *
 *     Beyond addressing shots by uuid, the wedge itself is now structurally
 *     impossible on a restored stack: a load-time applicability pass DROPS an
 *     entry whose target no longer resolves instead of leaving it to fail
 *     forever, so a history that cannot be applied can no longer be a history
 *     that cannot be got past.
 *
 *     Repro, kept and now expected CLEAN:
 *     `STALAGMIGHT_PERSIST_FUZZ_DELETE_IN_TARGET=1 --seed 1 --iterations 3`
 *     on fractal. That flag is still off by default, but no longer because the
 *     wedge it used to construct was terminal - that wedge is fixed, and a run
 *     with the flag set now walks the whole stack like any other. It stays
 *     opt-in because it is a DIFFERENT id-space shape (the target line's ids
 *     run past the renumbered range, so a stale id misses rather than collides),
 *     and the default arrangement is the one the plan's coverage promises are
 *     written against.
 *
 * ## Found by this scenario on 2026-08-05 (release, HEAD 4579f18f) - FIXED
 *
 * 27. FIXED (b9d6b14b, and the state class removed entirely by f16c96a8).
 *     UNDO DROPPED AN EXACTLY-ZERO `line_offsets` KEY. On naharon, undoing an
 *     `update_shot` removed `project.line_offsets.0`, which the state it was
 *     restoring had as `[0,0]`:
 *
 *         undo did not restore the state recorded at depth 16
 *           (the entry it consumed was update_shot)
 *           at project.line_offsets.0
 *             expected: [0,0]   actual: <absent>
 *
 *     It fired BEFORE the save (`saves=0 reloads=0`), so it was an in-session
 *     defect, not a persistence one, and it was the exact mirror of finding 2.
 *     Finding 2 was "an undo LEAKS an all-zero offset key the line did not
 *     have"; a67dd1b5 fixed it by making adjust-style writes DROP a key that
 *     lands within `Project::OFFSET_DUST_M` of zero. An undo of
 *     `PreviousValues { shot_id }` re-executes `update_shot`, which runs that
 *     same cascade - so a key whose legitimate value IS (0,0) was deleted by the
 *     undo instead of being left alone. Geometrically inert, exactly as finding 2
 *     was, and just as much a non-inverse: the key is serialized into the .stalx.
 *
 *     Fixed on the principle that AN INVERSE RESTORES, IT DOES NOT RE-DERIVE:
 *     `PreviousValues` and `CreatedShotSnapshot` capture the rigid connected set's
 *     `line_offsets` PRE-command (additive serde fields) and the undo puts them
 *     back verbatim instead of running the cascade again (b9d6b14b). f16c96a8 then
 *     removed the underlying state from the model altogether - a dust-zero offset
 *     entry is unrepresentable, normalized by the single writer and re-normalized
 *     at the two boundaries that ingest maps this app did not write (the loader's
 *     zero-trust re-solve, and snapshot restore for legacy payloads) - so the
 *     "was this key legitimately present?" question the two findings kept
 *     disagreeing about no longer exists.
 *
 *     Repro, kept and now expected CLEAN:
 *     `STALAGMIGHT_FUZZ_FIXTURE=naharon.stalx --seed 5 --iterations 2`.
 *     `pnpm agent:fuzz-shrink` reduced it to 5 steps (1-minimal, 2 candidates
 *     rejected as failing differently):
 *
 *         1. createLine(spare,shots=3)          <- line 41
 *         2. mergeStations(41/3,5/3)               <- join it to the graph
 *         3. removeZeroDatum(0/0)                  <- line 0 loses its datum
 *         4. updateShot(s=41,shot=4100001,...)
 *         5. undo
 *
 *     It was never classified and never tolerated: it failed the run, which is
 *     the point.
 *
 * A structural fact worth keeping, found while reaching these: a
 * `ProjectSnapshotPair` entry is a WALL. `Shot::id` is serialized inside a
 * project snapshot (models/shot.rs, no `skip`), so restoring one puts the project
 * back into the PRE-SAVE id space and every stale id beyond it becomes valid
 * again. The exposed window is exactly "from the depth the reload lands on, down
 * to the first `ProjectSnapshotPair` beneath it" - which is why a random fuzzer
 * missed all of this, and why `COVERAGE_STEPS` puts every snapshot-pair step as
 * low in the stack as its preconditions allow.
 *
 * ## Trace record / replay
 *
 * Each iteration records its resolved move list and, on failure, writes it to the
 * session artifacts dir (path named in the message). The recorder RESETS per
 * iteration, because the cycles are independent - so the trace is of the cycle
 * that failed, already minimal in that dimension.
 *
 * `--trace <file>` replays one and `pnpm agent:fuzz-shrink` minimizes it. On a
 * replay the PLAN's own preconditions are skipped - id divergence, the
 * renumbering check, variant coverage, "the plan pushed no entries" - because a
 * shrunk trace is by construction not the plan. Every APP oracle still runs:
 * roundtrip, absolute invariants, integrity delta and stack survival.
 *
 * ## Determinism
 *
 * Every value and every target is drawn from `ctx.rng()`; the plan's order is
 * fixed. There are no secondary streams, because there is no legacy stream to
 * preserve: this scenario is new, and nothing outside it depends on its draws.
 * Equally, it draws from its OWN context and its own generators - it imports no
 * generator from `undoRedoFuzzActions.ts` - so it cannot move `undo-redo-fuzz`'s
 * stream or invalidate any repro seed recorded there.
 */
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import type { Scenario, ScenarioContext } from './framework';
import { DEFAULT_FIXTURE, disableAutoSave, loadFixtureWithoutUndoHistory } from './common';
import {
  RELOAD_RESOLVED_GEOMETRY,
  SAVE_HEALING_MUTATIONS,
  classifyDiffs,
  describeDiffs,
  describeShotSetDelta,
  diffSnapshots,
  maxNumericDelta,
  shotSetDelta,
  type KnownAsymmetry,
  type SnapshotDiff,
} from '../lib/projectSnapshot';
import type { TargetingDigest } from '../lib/fuzzOracleCore';
import {
  COVERAGE_STEPS,
  FEATURE_ID_STRIDE,
  GUARANTEED_VARIANTS,
  PLANNED_VARIANTS,
  UNREACHABLE_VARIANTS,
  VARIANT_BY_COMMAND_TYPE,
  idsDivergeFromPosition,
  newCoverageMemo,
  type CoverageAction,
  type UndoVariant,
} from './undoPersistenceCoverage';
import {
  TraceRecorder,
  defaultTracePath,
  readTrace,
  writeTrace,
  type FuzzTrace,
  type TraceStep,
} from '../lib/fuzzTrace';
import {
  REPLAYABLE_MUTATIONS,
  bumpEpoch,
  enableIdAgnosticModel,
  fetchMismatch,
  initModel,
  injectOracle,
  observeOnly,
  resyncModel,
  runStep,
  type IntegrityVerdict,
  type StepResponse,
} from './undoRedoFuzzBridge';

/**
 * EVERY env knob below is read at CALL time, never frozen into a module-level
 * const - the rule, and the reason, are spelled out in `undoRedoFuzz.ts`:
 * `pnpm agent:scenario --trace` and `pnpm agent:fuzz-shrink` both set these
 * variables from inside their own `main()`, after this module was imported.
 */

/**
 * INERT, and kept for the same reason the main fuzzer keeps it: every finding
 * this scenario reproduced (bug 1, bug 8, findings 11/12/13) is fixed and every
 * classifier is deleted, so there is nothing left to tolerate. Still read, and
 * still named in the `reproduce:` line, so a tolerant-looking invocation cannot
 * be mistaken for one that actually relaxed anything.
 */
function allowKnownBugs(): boolean {
  return process.env.STALAGMIGHT_FUZZ_ALLOW_KNOWN_BUGS === '1';
}

/** Which `.stalx` under `e2e/fixtures/` to fuzz. Basename only, as the main fuzzer. */
function fuzzFixture(): string {
  return process.env.STALAGMIGHT_FUZZ_FIXTURE || DEFAULT_FIXTURE;
}

/** Diffs shown in a failure message (more are counted, not printed). */
const SHOWN_DIFFS = 6;
/**
 * Differences the slow re-compare collects before it gives up.
 *
 * A capped list can only ever support a NEGATIVE conclusion. `checkSnapshot`
 * asks for one more than this and lets nothing speak for the mismatch - not even
 * the epoch attribution that would otherwise RESYNC - when the extra one comes
 * back.
 */
const DIFF_LIMIT = 400;
/** Actions shown in the failure message's history tail. */
const HISTORY_TAIL = 14;
/** `STALAGMIGHT_FUZZ_DUMP_HISTORY=1` prints the WHOLE iteration history instead. */
function dumpFullHistory(): boolean {
  return process.env.STALAGMIGHT_FUZZ_DUMP_HISTORY === '1';
}

/** Most entries undone before the save, so the redo stack is non-empty there. */
const MAX_PRE_SAVE_UNDOS = 4;

/**
 * Replay a recorded trace instead of running the coverage plan, and where to
 * force-write the trace of a run that passed.
 *
 * Read at RUN time, not at import time: `pnpm agent:scenario --trace` parses its
 * flags inside `main()`, after this module is imported, so a module-level const
 * would capture the empty string and silently fall back to generating.
 *
 * Replay is a MODE of this scenario, never a separate one - the fused step, the
 * three oracles and the epoch attribution are all the same code on both paths.
 */
function replayTracePath(): string {
  return process.env.STALAGMIGHT_FUZZ_REPLAY ?? '';
}

function traceOutPath(): string {
  return process.env.STALAGMIGHT_FUZZ_TRACE ?? '';
}

interface SnapshotCheck {
  equal: boolean;
  floatNoise: boolean;
  attributed: KnownAsymmetry[];
  text: string;
}

const EQUAL: SnapshotCheck = {
  equal: true,
  floatNoise: false,
  attributed: [],
  text: '',
};

/**
 * The SLOW path, reached only after the page reported a mismatch: re-compare with
 * numeric tolerance, subtract the documented save/reload families when the
 * comparison crosses an epoch, and render whatever is left as a failure.
 *
 * No `known` list and no bug-8 gate any more. This function used to take both,
 * because bug 1, bug 8 and findings 11/12/13 all fired here by construction;
 * they are fixed, their classifiers are deleted, and a mismatch the epoch
 * attribution does not explain now ends the run.
 */
function checkSnapshot(
  expected: string,
  actual: string,
  attributable: readonly KnownAsymmetry[],
): SnapshotCheck {
  if (expected === actual) return EQUAL;
  // ONE MORE than we report: `truncated` is what tells "exactly DIFF_LIMIT
  // differences" from "at least that many, and the rest were never looked at".
  // The walk visits root keys in sorted order, so a wide `project.*` difference
  // can spend the whole budget before `stations` or `stats` is examined once -
  // and the ATTRIBUTED resync below reads "nothing unexplained is left" off this
  // list.
  const all = diffSnapshots(expected, actual, DIFF_LIMIT + 1);
  if (all.length === 0) return { ...EQUAL, floatNoise: true };
  const truncated = all.length > DIFF_LIMIT;
  if (truncated) all.length = DIFF_LIMIT;

  let diffs = all;
  let attributed: KnownAsymmetry[] = [];
  let absorbed: SnapshotDiff[] = [];
  if (attributable.length > 0) {
    const split = classifyDiffs(all, attributable);
    if (split.known.length > 0) {
      diffs = split.unknown;
      attributed = split.asymmetries;
      absorbed = split.known.map((hit) => hit.diff);
    }
  }
  if (diffs.length === 0) {
    // The ATTRIBUTED shape is what the caller RESYNCS on, so it may only be
    // returned for a complete list. Every path we saw was attributable, but on
    // a truncated diff the ones past the cap were never looked at.
    if (truncated) {
      return {
        equal: false,
        floatNoise: false,
        attributed,
        text:
          `  every path in the first ${DIFF_LIMIT} was a documented save/reload change, but the\n` +
          '  diff was CAPPED there - so this is a statement about a PREFIX, not about the mismatch.',
      };
    }
    // The absorbed MAGNITUDE goes on the record. The re-derived-geometry
    // families are bounded by shape, not by size (see RELOAD_RESOLVED_GEOMETRY),
    // which is only defensible while the number is visible.
    const worst = maxNumericDelta(absorbed);
    return {
      equal: false,
      floatNoise: false,
      attributed,
      text: attributed
        .map((entry) => `  ATTRIBUTED [${entry.id}]: ${entry.description}`)
        .concat(worst === null ? [] : [`  largest absorbed numeric change: ${worst.toExponential(3)}`])
        .join('\n'),
    };
  }

  const lines = [describeDiffs(diffs.slice(0, SHOWN_DIFFS))];
  if (diffs.length > SHOWN_DIFFS) lines.push(`  ...and ${diffs.length - SHOWN_DIFFS} more differing path(s)`);
  if (truncated) {
    lines.push(
      `  THE DIFF WAS CAPPED at ${DIFF_LIMIT} path(s): everything above is a PREFIX and nothing`,
      '  past it was examined, so no classifier can speak for this mismatch and none is allowed to.',
    );
  }

  // Shot identity by uuid, which survives the renumbering, is the only thing
  // that can tell "the same shots moved" from "a different shot was deleted".
  const deltas = shotSetDelta(expected, actual);
  if (deltas.length > 0) {
    lines.push('  SHOT IDENTITY DELTA (by uuid, which survives save/reload):');
    lines.push(describeShotSetDelta(deltas));
  }
  for (const entry of attributed) {
    lines.push(`  (plus attributable change(s) [${entry.id}], subtracted before this diff)`);
  }

  return {
    equal: false,
    floatNoise: false,
    attributed,
    text: lines.join('\n'),
  };
}

/** `get_undo_stack` / `get_redo_stack` rows, as the frontend sees them. */
interface HistoryItem {
  index: number;
  description: string;
  commandType: string;
}

async function readStack(ctx: ScenarioContext, which: 'getUndoStack' | 'getRedoStack'): Promise<HistoryItem[]> {
  const res = (await ctx.callAction('project', which, [])) as { ok: boolean; result?: unknown };
  return Array.isArray(res.result) ? (res.result as HistoryItem[]) : [];
}

export const undoPersistenceFuzz: Scenario = {
  name: 'undo-persistence-fuzz',
  description:
    'Builds an undo stack covering every reachable UndoData variant, saves/reopens it, then ' +
    'walks the whole restored history down and back up - hunting bug 8s stale-id siblings.',
  defaultIterations: 12,
  async run(ctx, { iterations }) {
    // Resolved once per RUN, not once per process - see the env-knob rule above.
    const fixture = fuzzFixture();
    ctx.setMetric(`fixture:${fixture}`, 1);
    ctx.setMetric('plannedVariants', PLANNED_VARIANTS.length);
    const saveDir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-persist-fuzz-'));

    // Not reachable from a stripped project, so out of scope by construction.
    // Logged once per run rather than asserted: the day a command starts
    // producing one of these again, the coverage report is where it will show up.
    ctx.log(
      `UndoData variants no live command produces (out of scope): ` +
        Object.entries(UNREACHABLE_VARIANTS)
          .map(([name, why]) => `${name} (${why})`)
          .join('; '),
    );

    const seen = new Map<UndoVariant, number>();
    let totalEntries = 0;

    try {
      for (let iteration = 1; iteration <= iterations; iteration++) {
        const covered = await runIteration(ctx, iteration, iterations, saveDir);
        for (const [variant, count] of covered) {
          seen.set(variant, (seen.get(variant) ?? 0) + count);
          totalEntries += count;
        }
      }
    } finally {
      try {
        fs.rmSync(saveDir, { recursive: true, force: true });
      } catch {
        // Best-effort: a leftover temp dir is not a scenario failure.
      }
    }

    for (const [variant, count] of seen) ctx.setMetric(`variant:${variant}`, count);
    ctx.log(
      `fixture=${fixture} seed=${ctx.seed}: ${iterations} save/reload cycle(s), ` +
        `${totalEntries} persisted undo entries walked in both directions; variant coverage ` +
        `${[...seen.entries()].map(([v, c]) => `${v}=${c}`).sort().join(' ')}`,
    );
  },
};

/**
 * One complete cycle: open, build, diverge, undo a few, save/reopen, unwind,
 * rewind. Returns how many entries of each variant the restored stack held.
 */
async function runIteration(
  ctx: ScenarioContext,
  iteration: number,
  iterations: number,
  saveDir: string,
): Promise<Map<UndoVariant, number>> {
  const tag = `#${iteration}`;
  const fixture = fuzzFixture();
  const history: string[] = [];
  const note = (line: string): void => {
    history.push(line);
  };

  /**
   * The move that is RUNNING RIGHT NOW - `null` between moves.
   *
   * A `note` line reports what a move DID to the stacks, so it can only be
   * written once the move RETURNS. A move that throws instead then left the
   * tail ending on the PREVIOUS move, which reads as an attribution and is one:
   * it names the wrong move. The same gotcha in the Rust tier mis-read two of
   * issue #464's defects during triage.
   *
   * Armed before each step and disarmed after, and rendered by `fail` only, so
   * a run that does not fail behaves exactly as before and the trace is
   * untouched.
   */
  let inFlight: string | null = null;
  const startingMove = (label: string): void => {
    inFlight = label;
  };
  const finishedMove = (): void => {
    inFlight = null;
  };

  /**
   * The resolved move list for THIS cycle. Reset per iteration on purpose: the
   * iterations are independent (each re-opens the fixture from a known floor), so
   * a trace of the failing cycle is already minimal in that dimension and a
   * replay reproduces it without replaying the ones before it.
   *
   * Observe-only - `record` runs after the decision, draws nothing and reads no
   * app state.
   */
  const recorder = new TraceRecorder();
  const replayPath = replayTracePath();
  const traceOut = traceOutPath();
  const replay: FuzzTrace | null = replayPath ? readTrace(replayPath) : null;
  if (replay && replay.scenario !== 'undo-persistence-fuzz') {
    throw new Error(
      `trace ${replayPath} was recorded by ${replay.scenario}; run it with that scenario`,
    );
  }
  // The recorded steps carry RESOLVED line and shot ids, so replaying them
  // against a different project drives moves at ids that do not exist there:
  // an invented failure, or a green run, either way not the recorded bug.
  if (replay && replay.fixture !== fixture) {
    throw new Error(
      `trace ${replayPath} was recorded on ${replay.fixture} but this run is on ${fixture}; ` +
        `its steps carry ids resolved against that project. Re-run with ` +
        `STALAGMIGHT_FUZZ_FIXTURE=${replay.fixture} (pnpm agent:scenario --trace restores it for you).`,
    );
  }

  const dumpTrace = (failure: { message: string } | null, explicitPath: string): string | null => {
    const target =
      explicitPath ||
      (ctx.artifactsDir ? defaultTracePath(ctx.artifactsDir, 'undo-persistence-fuzz', ctx.seed) : '');
    if (!target) return null;
    try {
      return writeTrace(
        target,
        recorder.build({
          scenario: 'undo-persistence-fuzz',
          seed: ctx.seed,
          iterations,
          fixture,
          stream: 'plan',
          failure,
        }),
      );
    } catch {
      return null;
    }
  };

  const { stripped } = await loadFixtureWithoutUndoHistory(ctx, fixture);
  if (!(await disableAutoSave(ctx))) {
    throw new Error(
      'could not disable auto-save; a save mutates live state with no undo entry and would ' +
        'desync the model (see disableAutoSave in scenarios/common.ts)',
    );
  }

  const persistLimit = await ctx.eval<number>(`
    const config = window.__E2E_TEST__.projectStore.getState().config;
    return config && config.general && typeof config.general.undo_persist_limit === 'number'
      ? config.general.undo_persist_limit
      : 20;
  `);

  await injectOracle(ctx);
  const init = await initModel(ctx);
  const strippedRows = Object.values(stripped.cleared).reduce((sum, n) => sum + n, 0);
  ctx.setMetric('strippedUndoRows', strippedRows);

  let cursor = 0;
  let top = 0;
  let undoLen = init.undoLen;
  let redoLen = init.redoLen;
  let digest: TargetingDigest = init.digest;
  let savesDone = 0;
  let reloadsDone = 0;
  /**
   * `entryType[d]` is the commandType of the entry whose execution took the
   * project from depth d-1 to depth d. Read once off both restored stacks, so a
   * failure can name the entry the move consumed - which is what separates bug 1
   * from finding 11, and what narrows the bug-8 tolerance to its own mechanism.
   */
  const entryType: (string | null)[] = [null];

  /**
   * Index the two stacks by DEPTH. Index 0 of each is the entry the next undo /
   * redo would consume, so the undo stack counts down from the current depth and
   * the redo stack counts up from it.
   */
  const fillEntryTypes = (undo: readonly HistoryItem[], redo: readonly HistoryItem[]): void => {
    for (let i = 0; i < undo.length; i++) entryType[cursor - i] = undo[i]?.commandType ?? null;
    for (let i = 0; i < redo.length; i++) entryType[cursor + 1 + i] = redo[i]?.commandType ?? null;
  };

  // Explicitly typed so TypeScript's control-flow analysis treats a call as
  // never-returning and narrows what follows it (an inferred `never` return on a
  // const arrow does not).
  const fail: (headline: string, detail?: string) => never = (headline, detail = '') => {
    const tail = (dumpFullHistory() ? history : history.slice(-HISTORY_TAIL)).map((line) => `    ${line}`);
    // A move that has not RETURNED has no history line yet, so name it here -
    // without this the tail ends on the move BEFORE the one that failed.
    if (inFlight !== null) tail.push(`    ${inFlight} -> IN FLIGHT (had not returned)`);
    const message = [
      headline,
      `  reproduce: pnpm agent:scenario undo-persistence-fuzz --seed ${ctx.seed} ` +
        `--iterations ${iterations} --release` +
        `${fixture === DEFAULT_FIXTURE ? '' : ` (STALAGMIGHT_FUZZ_FIXTURE=${fixture})`}` +
        `${allowKnownBugs() ? ' (STALAGMIGHT_FUZZ_ALLOW_KNOWN_BUGS=1 - INERT: nothing left to allow)' : ''}` +
        `${replayPath ? ` (REPLAY of ${replayPath})` : ''}`,
      `  iteration=${iteration} fixture=${fixture} depth=${cursor} top=${top} ` +
        `saves=${savesDone} reloads=${reloadsDone}`,
      ...(detail ? [detail] : []),
      `  last ${tail.length} action(s) this iteration:`,
      ...tail,
    ].join('\n');
    const written = dumpTrace({ message }, traceOut);
    throw new Error(written ? `${message}\n  trace: ${written}` : message);
  };

  if (init.undoLen !== 0 || init.redoLen !== 0) {
    fail(
      `${fixture} opened with inherited history after stripping ${strippedRows} row(s)`,
      `  undoLen=${init.undoLen} redoLen=${init.redoLen} ` +
        `(cleared ${JSON.stringify(stripped.cleared)}, absent ${JSON.stringify(stripped.missing)})`,
    );
  }
  if (init.invariants.length > 0) {
    fail(
      `${fixture} violates project invariants BEFORE any mutation`,
      init.invariants.map((violation) => `  INVARIANT: ${violation}`).join('\n'),
    );
  }

  const step = async (
    kind: 'mutate' | 'undo' | 'redo' | 'save' | 'reload',
    action: string,
    args: unknown[],
  ): Promise<StepResponse> => {
    const started = Date.now();
    const res = await runStep(ctx, { kind, action, args, cursor, undo0: undoLen, steps: 1 });
    ctx.metric('fusedStepMs', Date.now() - started);
    ctx.metric('actionMs', res.timing.actionMs);
    ctx.metric('observeMs', res.timing.observeMs);
    ctx.metric('canonicalMs', res.timing.canonicalMs);
    ctx.metric('integrityMs', res.timing.integrityMs);
    ctx.metric('steps');
    return res;
  };

  const record = (res: StepResponse): void => {
    undoLen = res.undoLen;
    redoLen = res.redoLen;
    digest = res.digest;
  };

  const assertModel = (res: StepResponse, headline: string): void => {
    if (res.cursorAfter !== cursor || res.top !== top) {
      fail(
        headline,
        `  page model says cursor=${res.cursorAfter} top=${res.top}, Node says cursor=${cursor} top=${top}`,
      );
    }
  };

  const assertInvariants = (res: StepResponse, what: string): void => {
    if (res.invariants.length === 0) return;
    fail(
      `project invariant(s) violated after ${what}`,
      res.invariants.map((violation) => `  INVARIANT: ${violation}`).join('\n'),
    );
  };

  const assertStacks = (headline: string, res: StepResponse): void => {
    const wantRedo = top - cursor;
    if (res.undoLen !== cursor || res.redoLen !== wantRedo) {
      fail(
        headline,
        `  expected undoLen=${cursor} redoLen=${wantRedo}, got undoLen=${res.undoLen} redoLen=${res.redoLen}`,
      );
    }
  };

  const sameIntegrity = (a: IntegrityVerdict, b: IntegrityVerdict): boolean =>
    a.saveBlocked === b.saveBlocked && a.messages.join('\u0000') === b.messages.join('\u0000');

  const assertIntegrityDelta = (res: StepResponse, what: string, resynced = false): void => {
    const expected = res.integrityExpected;
    if (expected === null) return;
    // `resolveMismatch` just replaced the model's snapshot at this depth, and
    // `resyncModel` moves the recorded integrity verdict with it. `expected`
    // was computed IN THE PAGE at step time, i.e. against the snapshot that has
    // since been superseded, so comparing it now would fail on precisely the
    // difference the resync accepted.
    if (resynced) {
      ctx.metric('integrityResynced');
      return;
    }
    ctx.metric('integrityCompared');
    if (sameIntegrity(expected, res.integrity)) return;
    const format = (verdict: IntegrityVerdict): string =>
      `saveBlocked=${verdict.saveBlocked} messages=${JSON.stringify(verdict.messages)}`;
    fail(
      `${what} restored the state but NOT the integrity verdict recorded at depth ${cursor}`,
      `  expected: ${format(expected)}\n  actual:   ${format(res.integrity)}`,
    );
  };

  /**
   * Resolve a mismatch the page reported. `consumed` is the commandType of the
   * entry the move applied (null for a save, a reload or a mutation); it is now
   * only ever reported, never used to pick a tolerance - there are none left.
   *
   * Returns true when the model was RESYNCED, which the caller has to know:
   * every expectation the page computed for this step - the integrity verdict
   * included - was computed against the snapshot the resync replaced.
   */
  const resolveMismatch = async (
    headline: string,
    res: StepResponse,
    consumed: string | null,
  ): Promise<boolean> => {
    const pair = await fetchMismatch(ctx);
    // Only an epoch-crossing comparison may subtract the documented undo-less
    // changes. Within an epoch the oracle stays strict.
    const attributable = res.crossesEpoch
      ? [
          ...(savesDone > 0 ? SAVE_HEALING_MUTATIONS : []),
          ...(reloadsDone > 0 ? RELOAD_RESOLVED_GEOMETRY : []),
        ]
      : [];
    const check = checkSnapshot(pair.expected ?? '', pair.actual ?? '', attributable);
    if (check.equal) {
      if (check.floatNoise) ctx.metric('floatNoise');
      return false;
    }
    if (check.attributed.length > 0 && check.text.startsWith('  ATTRIBUTED')) {
      ctx.metric('resyncs');
      for (const entry of check.attributed) ctx.metric(`resync:${entry.id}`);
      top = await resyncModel(ctx, pair.index, res.integrity);
      return true;
    }
    fail(headline, check.text);
  };

  /**
   * Apply ONE mutation and run the whole case analysis on what the stacks did.
   * Lifted out of the plan loop unchanged so a REPLAY can drive it with a
   * recorded action instead of a planned one; the plan path still hands it
   * exactly what `CoverageStep.build` returned.
   */
  const applyMutation = async (built: CoverageAction, stepName: string): Promise<number> => {
    recorder.record({
      kind: 'mutate',
      action: built.action,
      args: built.args,
      label: built.label,
    });
    const beforeUndo = undoLen;
    const beforeRedo = redoLen;
    startingMove(`${tag} ${stepName}: ${built.label}`);
    const res = await step('mutate', built.action, built.args);
    finishedMove();
    const accepted = res.actionOk && res.commandSuccess !== false;
    const pushed = res.undoLen - beforeUndo;
    note(
      `${tag} ${stepName}: ${built.label} -> ${accepted ? 'ok' : 'rejected'} ` +
        `(undo ${beforeUndo}->${res.undoLen}, redo ${beforeRedo}->${res.redoLen})` +
        `${accepted ? '' : ` [${res.commandError ?? res.actionError ?? 'unknown'}]`}`,
    );
    assertInvariants(res, built.label);

    if (!accepted) {
      ctx.metric(`rejected:${stepName}`);
      if (pushed !== 0 || res.redoLen !== beforeRedo) {
        fail(
          'a REJECTED command changed the history stacks',
          `  error: ${res.commandError ?? res.actionError ?? 'unknown'}\n` +
            `  undo ${beforeUndo}->${res.undoLen}, redo ${beforeRedo}->${res.redoLen}`,
        );
      }
      assertModel(res, 'a rejected command moved the page model');
      if (res.verdict !== 'equal') {
        await resolveMismatch(
          `a REJECTED command (${built.action}) mutated the project: ` +
            `${res.commandError ?? res.actionError ?? 'unknown'}`,
          res,
          null,
        );
      }
      record(res);
      return pushed;
    }

    if (pushed === 0) {
      // Succeeded without pushing an entry. Permitted (a no-op command), but it
      // means the plan did not deliver this variant; the coverage assertion is
      // what reports that.
      ctx.metric(`noop:${stepName}`);
      if (res.redoLen === 0) top = cursor;
      assertModel(res, 'a no-op command moved the page model');
      if (res.verdict !== 'equal') {
        await resolveMismatch(`a no-op command (${built.action}) changed the project state`, res, null);
      }
      record(res);
      return pushed;
    }
    if (pushed !== 1) {
      fail(`${stepName} pushed ${pushed} undo entries (expected 0 or 1)`, `  after ${built.label}`);
    }
    if (res.redoLen !== 0) {
      fail('a successful command did not clear the redo stack', `  after ${built.label}`);
    }
    cursor += 1;
    top = cursor;
    entryType[cursor] = null; // filled from the restored stacks after the reload
    assertModel(res, 'the page recorded the new snapshot at a different depth than Node expected');
    if (res.verdict !== 'recorded') {
      fail(`the page did not record a snapshot for a pushed entry (verdict=${res.verdict})`);
    }
    record(res);
    ctx.metric(`step:${stepName}`);
    return pushed;
  };

  // -----------------------------------------------------------------------
  // 1. Build the covering stack.
  // -----------------------------------------------------------------------
  const memo = newCoverageMemo();
  // Empty when replaying: the trace supplies every move, and running the plan as
  // well would make the replay something other than the cycle it came from.
  for (const plan of replay ? [] : COVERAGE_STEPS) {
    const lineIdsBefore = plan.remembers ? digest.s.map((entry) => entry[0]) : [];
    const built = plan.build(ctx, digest, memo);
    if (built === null) {
      note(`${tag} ${plan.name} -> NOT BUILDABLE`);
      ctx.metric(`unbuildable:${plan.name}`);
      if (plan.guaranteed) {
        fail(
          `coverage step ${plan.name} could not be built, so the plan cannot promise ` +
            `${plan.variant}`,
          `  purpose: ${plan.purpose}`,
        );
      }
      continue;
    }
    const pushed = await applyMutation(built, plan.name);

    // Only after a step that actually pushed an entry - which is what the
    // `continue`s in the extracted body used to guarantee positionally.
    if (plan.remembers && pushed === 1) {
      const fresh = digest.s.map((entry) => entry[0]).filter((id) => !lineIdsBefore.includes(id));
      const created = fresh[0];
      if (fresh.length !== 1 || created === undefined) {
        fail(
          `${plan.name} added ${fresh.length} line(s); the plan needs exactly one to hold on to`,
          `  before=${JSON.stringify(lineIdsBefore.slice(-8))} ` +
            `after=${JSON.stringify(digest.s.map((entry) => entry[0]).slice(-8))}`,
        );
      }
      if (plan.remembers === 'target') memo.createdLineId = created;
      else memo.spareLineId = created;
    }
  }

  if (!replay && top === 0) fail('the coverage plan pushed no undo entries at all');
  if (!replay && top > persistLimit) {
    fail(
      `the coverage plan built ${top} entries but only the newest ${persistLimit} are persisted ` +
        `(undo_persist_limit), so the restored stack could not be modelled`,
    );
  }

  // -----------------------------------------------------------------------
  // 2. Prove the collision precondition: ids must no longer equal positions.
  // -----------------------------------------------------------------------
  // Everything in this block asserts that the PLAN did its job. A replayed trace
  // is by construction not the plan - a shrink is allowed to remove the very
  // steps that produced the divergence - so these are skipped there. No app
  // oracle is skipped: the roundtrip, invariant, integrity and stack-survival
  // checks all run on both paths.
  const targetLine = memo.createdLineId;
  if (!replay && targetLine === null) fail('the plan never identified its target line');
  const targetShots =
    targetLine === null ? [] : digest.s.find((entry) => entry[0] === targetLine)?.[1] ?? [];
  if (!replay && targetLine !== null && !idsDivergeFromPosition(targetLine, targetShots)) {
    fail(
      'the plan did not make shot ids diverge from positional order, so a stale persisted id ' +
        'would address the same shot after the reload and the whole precondition is absent',
      `  line ${targetLine} ids=${JSON.stringify(targetShots)} (the loader will renumber ` +
        `them to ${JSON.stringify(targetShots.map((_, i) => targetLine * FEATURE_ID_STRIDE + i))})`,
    );
  }
  if (!replay) ctx.metric('divergedLines');

  // -----------------------------------------------------------------------
  // 3. Undo a few, so the REDO stack is non-empty when the save runs.
  // -----------------------------------------------------------------------
  // Read the live stack first, so the pre-save undos can also name the entry they
  // consumed; the authoritative read is the one after the reload.
  fillEntryTypes(await readStack(ctx, 'getUndoStack'), []);
  const wanted = replay ? 0 : ctx.randInt(1, Math.max(1, Math.min(MAX_PRE_SAVE_UNDOS, top - 1)));

  /**
   * One history move, verified against the snapshot recorded at the depth it
   * lands on. Throws on anything that is not a clean move.
   *
   * It used to return false for one tolerated case: finding 13's WEDGE, where a
   * persisted shot id ran past the renumbered range, `delete_shot` /
   * `update_shot` / `set_shot_kind` returned "Shot N not found in line S",
   * `undo_inner` pushed the entry back, and every entry beneath it was
   * unreachable for good. That is fixed (undo data addresses shots by uuid, and
   * a load-time applicability pass drops an entry whose target no longer
   * resolves), so the signature match and the early return are deleted: a
   * refused history move fails the run.
   */
  const walk = async (direction: 'undo' | 'redo', label: string): Promise<void> => {
    const back = direction === 'undo';
    const consumed = entryType[back ? cursor : cursor + 1] ?? null;
    const beforeUndo = undoLen;
    const beforeRedo = redoLen;
    startingMove(`${tag} ${label} ${direction}[${consumed ?? '?'}]`);
    const res = await step(direction, direction, []);
    finishedMove();
    if (!res.actionOk || res.commandSuccess === false) {
      const error = res.commandError ?? res.actionError ?? 'unknown';
      note(`${tag} ${label} ${direction}[${consumed ?? '?'}] -> FAILED [${error}]`);
      fail(
        `${direction} at depth ${cursor} failed outright (entry ${consumed ?? 'unknown'})`,
        `  error: ${error}`,
      );
    }
    cursor += back ? -1 : 1;
    note(
      `${tag} ${label} ${direction}[${consumed ?? '?'}] -> depth ${cursor} ` +
        `(undo ${beforeUndo}->${res.undoLen}, redo ${beforeRedo}->${res.redoLen})`,
    );
    assertInvariants(res, direction);
    assertModel(res, `${direction} left the page model where Node did not expect it`);
    assertStacks(`${direction} left the history stacks in an unexpected shape`, res);
    const resynced =
      res.verdict !== 'equal' &&
      (await resolveMismatch(
        `${direction} did not restore the state recorded at depth ${cursor} ` +
          `(the entry it consumed was ${consumed ?? 'unknown'})`,
        res,
        consumed,
      ));
    assertIntegrityDelta(res, direction, resynced);
    record(res);
    ctx.metric(direction === 'undo' ? 'undos' : 'redos');
  };

  for (let i = 0; i < wanted; i++) {
    recorder.record({ kind: 'undo' });
    await walk('undo', 'pre-save');
  }
  if (!replay && redoLen === 0) {
    fail('the pre-save undos left the redo stack empty, so the reload could not restore one');
  }
  ctx.setMetric('preSaveRedoDepth', top - cursor);

  /**
   * Save to a temp path, close, reopen: the persistence boundary, as one
   * indivisible move. Splitting it would let a shrink produce a candidate that
   * saved without reopening, which is not a state the app can be in.
   */
  const crossBoundary = async (): Promise<void> => {
    const savePath = path.join(saveDir, `cycle-${iteration}.stalx`);
    await bumpEpoch(ctx);
    startingMove(`${tag} save`);
    const saveRes = await step('save', 'saveProject', [savePath]);
    finishedMove();
    if (!saveRes.actionOk || saveRes.commandSuccess === false) {
      fail('saveProject failed', `  error: ${saveRes.commandError ?? saveRes.actionError ?? 'unknown'}`);
    }
    note(`${tag} save -> ok (depth ${cursor}, undo ${saveRes.undoLen}, redo ${saveRes.redoLen})`);
    assertInvariants(saveRes, 'save');
    assertStacks('save changed the history stacks', saveRes);
    savesDone += 1;
    if (saveRes.verdict !== 'equal') {
      await resolveMismatch(
        'save mutated the project in a way SAVE_HEALING_MUTATIONS does not account for',
        saveRes,
        null,
      );
    }
    record(saveRes);
    ctx.metric('saves');

    startingMove(`${tag} close/reopen`);
    const closed = (await ctx.callAction('project', 'closeProject', [])) as {
      ok: boolean;
      error?: string;
      result?: { success?: boolean; error?: string };
    };
    if (!closed.ok || closed.result?.success === false) {
      fail('closeProject failed before reopen', `  error: ${closed.result?.error ?? closed.error ?? 'unknown'}`);
    }
    const opened = (await ctx.callAction('project', 'loadProject', [savePath])) as {
      ok: boolean;
      error?: string;
      result?: { success?: boolean; error?: string };
    };
    if (!opened.ok || opened.result?.success === false) {
      fail('reopening the saved project failed', `  error: ${opened.result?.error ?? opened.error ?? 'unknown'}`);
    }
    finishedMove();

    await enableIdAgnosticModel(ctx);
    await bumpEpoch(ctx);
    reloadsDone += 1;
    const reloaded = await observeOnly(ctx, cursor);
    note(`${tag} reload -> depth ${cursor} (undo ${reloaded.undoLen}, redo ${reloaded.redoLen})`);
    assertInvariants(reloaded, 'reload');
    assertStacks('the persisted history did not come back intact', reloaded);
    const resynced =
      reloaded.verdict !== 'equal' &&
      (await resolveMismatch(
        'reopening the saved project did not restore the state it was saved at',
        reloaded,
        null,
      ));
    assertIntegrityDelta(reloaded, 'reload', resynced);
    record(reloaded);
    ctx.metric('reloads');
  };

  // -----------------------------------------------------------------------
  // 4. Save, close, reopen.
  // -----------------------------------------------------------------------
  /**
   * Apply one recorded move. Same rule as `undo-redo-fuzz`: a history move whose
   * precondition no longer holds is SKIPPED and a save with nothing to persist is
   * skipped too, which is what makes an arbitrary subsequence of a trace runnable
   * - the property the shrinker needs. Skipping asserts nothing, so it cannot
   * turn a failing candidate green by hiding something.
   */
  const applyTraceStep = async (traceStep: TraceStep, label: string): Promise<void> => {
    if (traceStep.kind === 'mutate') {
      // A trace supplies both the action NAME and its arguments, so the replay
      // path is narrower than the dispatch path: only the mutations a generator
      // can produce, never a scenario-driven control action like `saveProject`
      // (which takes a path this file would then be choosing).
      if (!REPLAYABLE_MUTATIONS.has(traceStep.action)) {
        throw new Error(
          `trace step names ${traceStep.action}, which is not a replayable mutation. ` +
            `A recorded trace may only replay the actions the generators produce.`,
        );
      }
      await applyMutation(
        { action: traceStep.action, args: traceStep.args, label: traceStep.label },
        `replay:${traceStep.action}`,
      );
      return;
    }
    if (traceStep.kind === 'save') {
      if (top === 0 || top > persistLimit || savesDone > 0) {
        ctx.metric('replaySkipped:save');
        return;
      }
      recorder.record({ kind: 'save' });
      await crossBoundary();
      return;
    }
    if (traceStep.kind === 'repair') {
      ctx.metric('replaySkipped:repair');
      return;
    }
    // Schema v2 added the import/export step vocabulary. This scenario drives
    // the store, not the UI, so an unrunnable kind must be named rather than
    // fall through into the history walk below and replay as an undo.
    if (
      traceStep.kind !== 'undo' &&
      traceStep.kind !== 'redo' &&
      traceStep.kind !== 'undoTo' &&
      traceStep.kind !== 'redoTo'
    ) {
      throw new Error(
        `trace step kind ${traceStep.kind} is not one undo-persistence-fuzz replays; ` +
          `it belongs to a scenario that drives the import/export UI.`,
      );
    }
    const back = traceStep.kind === 'undo' || traceStep.kind === 'undoTo';
    const reach = back ? cursor : top - cursor;
    if (reach < 1) {
      ctx.metric(`replaySkipped:${traceStep.kind}`);
      return;
    }
    recorder.record(back ? { kind: 'undo' } : { kind: 'redo' });
    await walk(back ? 'undo' : 'redo', label);
  };

  if (replay) {
    ctx.setMetric('replaySteps', replay.steps.length);
    for (let index = 0; index < replay.steps.length; index++) {
      await applyTraceStep(replay.steps[index] as TraceStep, `T${index + 1}`);
    }
  } else {
    recorder.record({ kind: 'save' });
    await crossBoundary();
  }

  // The renumbering is what makes a stale id collide rather than miss. Prove it
  // happened, so a build that stopped renumbering would fail here loudly instead
  // of quietly turning this scenario into a no-op.
  const renumbered =
    targetLine === null ? [] : digest.s.find((entry) => entry[0] === targetLine)?.[1] ?? [];
  if (!replay && targetLine !== null && idsDivergeFromPosition(targetLine, renumbered)) {
    fail(
      'the reload did NOT renumber shot ids to line_id * FEATURE_ID_STRIDE + position, so the ' +
        'stale-id collision this scenario tests for cannot arise',
      `  line ${targetLine} ids after reload = ${JSON.stringify(renumbered)}`,
    );
  }
  if (!replay) ctx.metric('renumberedLines');

  // -----------------------------------------------------------------------
  // 5. Read the restored stacks: which entry sits at which depth.
  // -----------------------------------------------------------------------
  const restoredUndo = await readStack(ctx, 'getUndoStack');
  const restoredRedo = await readStack(ctx, 'getRedoStack');
  if (restoredUndo.length !== cursor || restoredRedo.length !== top - cursor) {
    fail(
      'the restored history items do not match the restored stack lengths',
      `  get_undo_stack=${restoredUndo.length} get_redo_stack=${restoredRedo.length}, ` +
        `expected ${cursor} and ${top - cursor}`,
    );
  }
  fillEntryTypes(restoredUndo, restoredRedo);

  const covered = new Map<UndoVariant, number>();
  const unmapped: string[] = [];
  for (let depth = 1; depth <= top; depth++) {
    const commandType = entryType[depth];
    if (!commandType) fail(`no restored history item for depth ${depth}`);
    const variant = VARIANT_BY_COMMAND_TYPE[commandType];
    if (!variant) {
      unmapped.push(commandType);
      continue;
    }
    covered.set(variant, (covered.get(variant) ?? 0) + 1);
  }
  if (unmapped.length > 0) {
    fail(
      `restored undo entries carry commandType(s) VARIANT_BY_COMMAND_TYPE does not know: ` +
        `${JSON.stringify([...new Set(unmapped)])}`,
      '  the map is transcribed from src-tauri/src/core/state/undo_capture.rs; re-check it',
    );
  }
  // The COVERAGE oracle asserts the PLAN's promise, so it does not apply to a
  // replayed subsequence - a shrink's whole job is to remove steps it can.
  const missingGuaranteed = replay
    ? []
    : GUARANTEED_VARIANTS.filter((variant) => !covered.has(variant));
  if (missingGuaranteed.length > 0) {
    fail(
      `the restored stack is missing UndoData variant(s) the plan guarantees: ` +
        `${JSON.stringify(missingGuaranteed)}`,
      `  present: ${JSON.stringify([...covered.keys()])}\n` +
        '  a guaranteed step either failed or no-opped; see the action history above',
    );
  }
  for (const variant of PLANNED_VARIANTS) {
    if (covered.has(variant)) continue;
    ctx.metric(`variantMissing:${variant}`);
    ctx.log(`${tag} variant ${variant} was not reachable on ${fixture} this iteration`);
  }
  note(
    `${tag} restored stacks: undo=${restoredUndo.length} redo=${restoredRedo.length}, ` +
      `entries=${JSON.stringify(entryType.slice(1))}`,
  );

  // -----------------------------------------------------------------------
  // 6. Walk the whole RESTORED history: all the way down, all the way back up.
  // -----------------------------------------------------------------------
  // No wedge escape any more: `walk` throws on a refused move, so the unwind
  // reaches depth 0 and the rewind reaches the summit, or the run fails. The
  // `unwindWedged` / `deepestWedge` / `rewindWedged` metrics went with finding
  // 13's tolerance.
  const summit = top;
  while (cursor > 0) await walk('undo', 'unwind');
  while (cursor < summit) await walk('redo', 'rewind');
  if (cursor !== summit) {
    fail(`the roundtrip ended at depth ${cursor}, expected ${summit}`);
  }
  ctx.metric('entriesUnwound', summit);
  ctx.metric('cycles');
  ctx.setMetric('traceSteps', recorder.length);
  // A cycle that PASSED only writes a trace when asked for by name. The path is
  // shared across iterations on purpose: the last cycle wins, and a run whose
  // cycles are independent has nothing useful in the earlier ones.
  if (traceOut) dumpTrace(null, traceOut);

  return covered;
}
