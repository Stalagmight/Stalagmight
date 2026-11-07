/**
 * The undo-model plumbing `undoRedoFuzz.ts` and `undoPersistenceFuzz.ts` both
 * run on top of the in-page bridge (`scenarios/undoRedoFuzzBridge.ts`): the
 * fused step and its timing metrics, the model / invariant / stack / integrity
 * assertions, the epoch-attributed mismatch resolution, and the save, close,
 * reopen boundary.
 *
 * Each scenario used to carry its own copy, the persistence one written as a
 * transcription of the main one. The texts, metric names and the order of every
 * bridge call are unchanged; only the home moved.
 *
 * THE SCENARIO STILL OWNS THE MODEL. `cursor`, `top`, the stack lengths and the
 * save/reload counters stay `let`s in the scenario body, which reads and writes
 * them exactly where it always did; this module sees them through
 * `UndoModelState`, a getter/setter view the scenario builds over its own
 * variables. Nothing here draws from any PRNG or decides a move, so moving it
 * cannot shift a seed's action sequence.
 */
import type { ScenarioContext } from '../scenarios/framework';
import { disableAutoSave } from '../scenarios/common';
import {
  REPLAYABLE_MUTATIONS,
  bumpEpoch,
  enableIdAgnosticModel,
  fetchMismatch,
  observeOnly,
  resyncModel,
  runStep,
  type IntegrityVerdict,
  type StepRequest,
  type StepResponse,
} from '../scenarios/undoRedoFuzzBridge';
import type { MoveLog } from './fuzzHarness';
import { detProbe } from './fuzzDeterminismProbe';
import { checkSnapshot, type SnapshotCheckLimits } from './fuzzSnapshotCheck';
import type { FuzzTrace, TraceStep } from './fuzzTrace';
import { RELOAD_RESOLVED_GEOMETRY, SAVE_HEALING_MUTATIONS } from './projectSnapshot';

// ---------------------------------------------------------------------------
// Run setup both undo fuzzers do before the model exists.
// ---------------------------------------------------------------------------

/** Turn auto-save off, or refuse to run: a save the model never saw would desync it. */
export async function requireAutoSaveOff(ctx: ScenarioContext): Promise<void> {
  if (!(await disableAutoSave(ctx))) {
    throw new Error(
      'could not disable auto-save; a save mutates live state with no undo entry and would ' +
        'desync the model (see disableAutoSave in scenarios/common.ts)',
    );
  }
}

/**
 * `config.general.undo_persist_limit`, or 20 when the config does not say.
 * Only the newest that many entries survive a save, so a deeper history would
 * come back truncated - both scenarios bound what they let cross the boundary.
 */
export async function readUndoPersistLimit(ctx: ScenarioContext): Promise<number> {
  return ctx.eval<number>(`
    const config = window.__E2E_TEST__.projectStore.getState().config;
    return config && config.general && typeof config.general.undo_persist_limit === 'number'
      ? config.general.undo_persist_limit
      : 20;
  `);
}

// ---------------------------------------------------------------------------
// Replaying a recorded trace through the store-driven undo model.
// ---------------------------------------------------------------------------

/** The history-move members of `TraceStep`. */
export type HistoryTraceStep = Extract<TraceStep, { kind: 'undo' | 'redo' | 'undoTo' | 'redoTo' }>;

/**
 * Refuse a recorded mutation this harness may not replay.
 *
 * A trace supplies both the action NAME and its arguments, so the replay path
 * is narrower than the dispatch path: only the mutations a generator can
 * produce, never a scenario-driven control action like `saveProject` (which
 * takes a path the scenario would then be choosing).
 */
export function assertReplayableMutation(action: string): void {
  if (!REPLAYABLE_MUTATIONS.has(action)) {
    // The name is read back from a trace, which a forged copy writes, so it goes
    // on a later line, which the runners withhold (#576 W51).
    throw new Error(
      `trace step names an action that is not a replayable mutation. ` +
        `A recorded trace may only replay the actions the generators produce.\n  action: ${action}`,
    );
  }
}

/**
 * Refuse a step kind a store-driven scenario cannot run, so that everything
 * after this call is a history move.
 *
 * Schema v2 added the import/export step vocabulary (`ui`, `export`, ...). The
 * undo fuzzers drive the store, not the UI, so they cannot run those - and they
 * must say so rather than let one fall through into the history-move arm and
 * replay as an undo, which is the exact bug the `kind` allow-list in
 * `validateStep` was added for.
 */
export function assertHistoryStep(step: TraceStep, scenario: string): asserts step is HistoryTraceStep {
  if (step.kind !== 'undo' && step.kind !== 'redo' && step.kind !== 'undoTo' && step.kind !== 'redoTo') {
    throw new Error(
      `trace step kind ${step.kind} is not one ${scenario} replays; ` +
        `it belongs to a scenario that drives the import/export UI.`,
    );
  }
}

/**
 * How far a recorded history move can reach from here, or null - counted as
 * `replaySkipped:<kind>` - when it cannot move at all.
 *
 * A move whose precondition no longer holds is SKIPPED rather than failed:
 * that is what makes an arbitrary subsequence of a trace runnable, which is
 * what the shrinker needs. Skipping asserts nothing, so it can never turn a
 * failing candidate green by hiding something.
 */
export function historyReach(
  ctx: ScenarioContext,
  step: HistoryTraceStep,
  cursor: number,
  top: number,
): { back: boolean; reach: number } | null {
  const back = step.kind === 'undo' || step.kind === 'undoTo';
  const reach = back ? cursor : top - cursor;
  if (reach < 1) {
    ctx.metric(`replaySkipped:${step.kind}`);
    return null;
  }
  return { back, reach };
}

/** Apply every recorded step, in order, tagged `T1`, `T2`, ... */
export async function replayTrace(
  ctx: ScenarioContext,
  replay: FuzzTrace,
  apply: (step: TraceStep, tag: string) => Promise<void>,
): Promise<void> {
  ctx.setMetric('replaySteps', replay.steps.length);
  for (let index = 0; index < replay.steps.length; index++) {
    await apply(replay.steps[index] as TraceStep, `T${index + 1}`);
  }
}

/** A live view of the scenario's own model variables. */
export interface UndoModelState {
  /** Node's current undo depth. */
  readonly cursor: number;
  /** Node's mirror of the page's `snapshots.length - 1`. A resync moves it. */
  top: number;
  /** Undo-stack length after the previous step: the next step's `undo0`. */
  readonly undoLen: number;
  /** Saves this run (or cycle) has taken. Any one makes the save heals attributable. */
  savesDone: number;
  /** Reloads likewise, for the reload re-solve. */
  reloadsDone: number;
}

export interface UndoHarnessOptions {
  /** The scenario's `fail`: throws with its own reproduce line and move tail. */
  fail: (headline: string, detail?: string) => never;
  /** The scenario's move log; the boundary crossing arms and notes on it. */
  log: MoveLog;
  /** The scenario's per-step bookkeeping (stack lengths, digest, its own maxima). */
  record: (res: StepResponse) => void;
  /**
   * The undo-stack length depth 0 sits on. Both scenarios assert it is 0 after
   * stripping the fixture's history; `undoRedoFuzz` still passes its measured
   * value so the bound reads as "from wherever the stack actually starts".
   */
  baseUndo: number;
  /** The scenario's slow re-compare budget (see `SnapshotCheckLimits`). */
  limits: SnapshotCheckLimits;
  /**
   * Note a REFUSED save in the move log before failing on it. `undoRedoFuzz`
   * always did and `undoPersistenceFuzz` never did; kept per scenario so neither
   * failure message changes.
   */
  noteFailedSave: boolean;
}

export interface UndoHarness {
  /** One fused round trip, timed. */
  step(kind: StepRequest['kind'], action: string, args: unknown[], steps?: number): Promise<StepResponse>;
  /** The page applied the same bookkeeping rules; prove it landed in the same place. */
  assertModel(res: StepResponse, headline: string): void;
  /** ABSOLUTE well-formedness of the observed state. */
  assertInvariants(res: StepResponse, what: string): void;
  /** Both stack lengths must be what the model says. */
  assertStacks(headline: string, res: StepResponse): void;
  /** The integrity verdict recorded at this depth must be restored with the state. */
  assertIntegrityDelta(res: StepResponse, what: string, resynced?: boolean): void;
  /**
   * Resolve a mismatch the page reported: re-baseline a documented save/reload
   * change, or fail with the diff. True when the model was RESYNCED.
   */
  resolveMismatch(headline: string, res: StepResponse): Promise<boolean>;
  /** Save to `savePath`, close, reopen: the persistence boundary, as one move. */
  saveAndReload(tag: string, savePath: string): Promise<void>;
}

const sameIntegrity = (a: IntegrityVerdict, b: IntegrityVerdict): boolean =>
  a.saveBlocked === b.saveBlocked && a.messages.join('\u0000') === b.messages.join('\u0000');

/** The store-action result shape `closeProject` / `loadProject` answer with. */
interface ActionResult {
  ok: boolean;
  error?: string;
  result?: { success?: boolean; error?: string };
}

export function undoFuzzHarness(
  ctx: ScenarioContext,
  model: UndoModelState,
  options: UndoHarnessOptions,
): UndoHarness {
  const { log, record, baseUndo, limits, noteFailedSave } = options;
  // Re-declared WITH its type, not destructured: TypeScript only treats a call
  // as never-returning (and narrows what follows it) through an explicitly
  // annotated binding.
  const fail: UndoHarnessOptions['fail'] = options.fail;

  const step: UndoHarness['step'] = async (kind, action, args, steps = 1) => {
    const started = Date.now();
    const res = await runStep(ctx, {
      kind,
      action,
      args,
      cursor: model.cursor,
      undo0: model.undoLen,
      steps,
    });
    ctx.metric('fusedStepMs', Date.now() - started);
    // Attribution, so the next round of tuning aims at evidence: the store
    // action's own work, the three read-only IPC calls, the in-page
    // canonicalization, and (by subtraction from fusedStepMs) the bridge.
    ctx.metric('actionMs', res.timing.actionMs);
    ctx.metric('observeMs', res.timing.observeMs);
    ctx.metric('canonicalMs', res.timing.canonicalMs);
    ctx.metric('integrityMs', res.timing.integrityMs);
    ctx.metric('steps');
    // Determinism mode: this step's answer and the state it left (no-op when off).
    await detProbe()?.step(ctx, { kind, action, args, steps }, res);
    return res;
  };

  const assertModel: UndoHarness['assertModel'] = (res, headline) => {
    const { cursor, top } = model;
    if (res.cursorAfter !== cursor || res.top !== top) {
      fail(
        headline,
        `  page model says cursor=${res.cursorAfter} top=${res.top}, Node says cursor=${cursor} top=${top}`,
      );
    }
  };

  /**
   * Independent of the roundtrip comparison: a forward command that corrupts
   * the project and an undo that faithfully restores the corruption match at
   * every depth, so only this fires.
   */
  const assertInvariants: UndoHarness['assertInvariants'] = (res, what) => {
    if (res.invariants.length === 0) return;
    fail(
      `project invariant(s) violated after ${what}`,
      res.invariants.map((violation) => `  INVARIANT: ${violation}`).join('\n'),
    );
  };

  const assertStacks: UndoHarness['assertStacks'] = (headline, res) => {
    const wantUndo = baseUndo + model.cursor;
    const wantRedo = model.top - model.cursor;
    if (res.undoLen !== wantUndo || res.redoLen !== wantRedo) {
      fail(
        headline,
        `  expected undoLen=${wantUndo} redoLen=${wantRedo}, got undoLen=${res.undoLen} redoLen=${res.redoLen}`,
      );
    }
  };

  /**
   * The app runs `check_project_integrity` after every undo/redo anyway, so
   * its verdict is part of the state a history move is supposed to restore. A
   * state that roundtrips while its integrity verdict does not is either a bug
   * in the checker or a divergence the snapshot cannot see.
   */
  const assertIntegrityDelta: UndoHarness['assertIntegrityDelta'] = (res, what, resynced = false) => {
    const expected = res.integrityExpected;
    if (expected === null) return;
    // `resolveMismatch` just replaced the model's snapshot at this depth, and
    // `resyncModel` moves the recorded integrity verdict with it - that is its
    // documented job. `expected` was computed IN THE PAGE at step time, against
    // the snapshot that has since been superseded, so comparing it now would
    // fail on precisely the difference the resync accepted. That is how a run
    // could log a documented save/reload re-baseline and then immediately fail
    // naming a different oracle. (The only resync left is that one:
    // `STALAGMIGHT_FUZZ_ALLOW_KNOWN_BUGS` is inert and tolerates nothing.)
    if (resynced) {
      ctx.metric('integrityResynced');
      return;
    }
    // Counted so the oracle can be shown to be doing work rather than passing
    // vacuously on a model that never recorded a verdict.
    ctx.metric('integrityCompared');
    if (sameIntegrity(expected, res.integrity)) return;
    const format = (verdict: IntegrityVerdict): string =>
      `saveBlocked=${verdict.saveBlocked} messages=${JSON.stringify(verdict.messages)}`;
    fail(
      `${what} restored the state but NOT the integrity verdict recorded at depth ${model.cursor}`,
      `  expected: ${format(expected)}\n  actual:   ${format(res.integrity)}`,
    );
  };

  /**
   * Fetches both canonical strings (the only time they cross the bridge) and
   * either RE-BASELINES a documented save/reload change or fails with the full
   * diff. Those two outcomes are all there are - no known-bug tolerance
   * survives.
   *
   * Returns true when the model was RESYNCED, which the caller has to know:
   * every expectation the page computed for this step - the integrity verdict
   * included - was computed against the snapshot the resync replaced.
   */
  const resolveMismatch: UndoHarness['resolveMismatch'] = async (headline, res) => {
    const pair = await fetchMismatch(ctx);
    // ONLY a comparison that crosses an epoch boundary may subtract the
    // documented undo-less changes. Within an epoch the oracle stays strict -
    // otherwise "a reload re-solves offsets" would permanently hide every
    // section_offsets bug the fuzzers exist to find (undoRedoFuzz bugs 3 and 7).
    const attributable = res.crossesEpoch
      ? [
          ...(model.savesDone > 0 ? SAVE_HEALING_MUTATIONS : []),
          ...(model.reloadsDone > 0 ? RELOAD_RESOLVED_GEOMETRY : []),
        ]
      : [];
    const check = checkSnapshot(pair.expected ?? '', pair.actual ?? '', attributable, limits);
    if (check.equal) {
      // Counted, never resynced: keeping the ORIGINAL expectation means the
      // tolerance can never ratchet, only ever absorb the same dust again.
      if (check.floatNoise) ctx.metric('floatNoise');
      return false;
    }
    // Every difference was a documented save heal or reload re-solve: record
    // the new baseline and carry on. Nothing else is tolerated here.
    if (check.attributed.length > 0 && check.text.startsWith('  ATTRIBUTED')) {
      ctx.metric('resyncs');
      for (const entry of check.attributed) ctx.metric(`resync:${entry.id}`);
      model.top = await resyncModel(ctx, pair.index, res.integrity);
      return true;
    }
    // No known-bug branch: every issue-413 finding is fixed and every
    // classifier deleted, so anything the attribution did not explain fails.
    fail(headline, check.text);
  };

  /**
   * Undo history lives in the .stalx (both stacks, sequence renumbered), so
   * until a run crosses this boundary nothing below it is exercised at all.
   * Three separate oracles run here:
   *
   *   1. SAVE IS ATTRIBUTABLE. `save_project` heals live state with no undo
   *      entry - `ensure_shot_uuids`, `ensure_unique_shot_uuids`,
   *      `capture_missing_gauge_anchors`, the orphan `section_offsets` retain,
   *      non-finite sanitize. Every path that changes must match
   *      `SAVE_HEALING_MUTATIONS` by path AND direction; anything else fails.
   *   2. THE STACKS SURVIVE. Both lengths must come back exactly.
   *   3. THE RESTORED STACK IS THE SAME HISTORY. From here the model compares
   *      id-agnostically (the loader renumbers `Shot::id` to
   *      `section_id * FEATURE_ID_STRIDE + sequence_order`), and every later
   *      walk compares the RESTORED stack against the snapshots recorded
   *      before the save.
   *
   * One indivisible move: splitting it would let a shrink produce a candidate
   * that saved without reopening, which is not a state the app can be in.
   */
  const saveAndReload: UndoHarness['saveAndReload'] = async (tag, savePath) => {
    const { note, startingMove, finishedMove } = log;
    // The save is about to make undo-less changes, so everything recorded so
    // far belongs to the previous epoch.
    await bumpEpoch(ctx);
    startingMove(`${tag} save`);
    const saveRes = await step('save', 'saveProject', [savePath]);
    finishedMove();
    if (!saveRes.actionOk || saveRes.commandSuccess === false) {
      if (noteFailedSave) note(`${tag} save -> FAILED`);
      fail('saveProject failed', `  error: ${saveRes.commandError ?? saveRes.actionError ?? 'unknown'}`);
    }
    note(`${tag} save -> ok (depth ${model.cursor}, undo ${saveRes.undoLen}, redo ${saveRes.redoLen})`);
    // Determinism mode: the saved file, every table row by row (no-op when off).
    detProbe()?.stalx(`${tag} saved`, savePath);
    assertInvariants(saveRes, 'save');
    assertStacks('save changed the history stacks', saveRes);
    model.savesDone += 1;
    if (saveRes.verdict !== 'equal') {
      await resolveMismatch('save mutated the project in a way SAVE_HEALING_MUTATIONS does not account for', saveRes);
    }
    record(saveRes);
    ctx.metric('saves');

    startingMove(`${tag} close/reopen`);
    const closed = (await ctx.callAction('project', 'closeProject', [])) as ActionResult;
    if (!closed.ok || closed.result?.success === false) {
      fail('closeProject failed before reopen', `  error: ${closed.result?.error ?? closed.error ?? 'unknown'}`);
    }
    const opened = (await ctx.callAction('project', 'loadProject', [savePath])) as ActionResult;
    if (!opened.ok || opened.result?.success === false) {
      fail('reopening the saved project failed', `  error: ${opened.result?.error ?? opened.error ?? 'unknown'}`);
    }
    finishedMove();

    // Switch BEFORE re-observing so both sides of the comparison are in the
    // same form. Idempotent, so later reloads are a no-op.
    await enableIdAgnosticModel(ctx);
    await bumpEpoch(ctx);
    model.reloadsDone += 1;
    const res = await observeOnly(ctx, model.cursor);
    await detProbe()?.step(ctx, { kind: 'reload', action: 'noop:observe' }, res);
    note(`${tag} reload -> depth ${model.cursor} (undo ${res.undoLen}, redo ${res.redoLen})`);
    assertInvariants(res, 'reload');
    assertStacks('the persisted history did not come back intact', res);
    const resynced =
      res.verdict !== 'equal' &&
      (await resolveMismatch('reopening the saved project did not restore the state it was saved at', res));
    assertIntegrityDelta(res, 'reload', resynced);
    record(res);
    ctx.metric('reloads');
  };

  return { step, assertModel, assertInvariants, assertStacks, assertIntegrityDelta, resolveMismatch, saveAndReload };
}
