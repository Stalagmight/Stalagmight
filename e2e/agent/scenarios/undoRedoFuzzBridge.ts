/**
 * The webview half of the undo-redo-fuzz oracle: the eval bodies that run a
 * step entirely in-page, and their typed request/response contract.
 *
 * ## Why
 *
 * The scenario used to do two WebDriver round trips per iteration - one to call
 * the store action, one to fetch `get_project_state` - and canonicalize the
 * whole payload in Node. That payload is ~60 KB on the fractal fixture and
 * ~277 KB on naharon, so the bridge, not the app, was the cost.
 *
 * Now one `executeAsync` per step: call the action, read the state and both
 * history stacks, canonicalize, compare against the model, and return a verdict
 * plus a ~20x smaller targeting digest. The full canonical strings cross the
 * bridge ONLY when they disagree, which is the rare path and is where Node's
 * tolerant diff and bug classification live.
 *
 * ## What did NOT change
 *
 * - The mutation and undo/redo still go through
 *   `window.__E2E_TEST__.projectStore.getState()[action](...args)`, exactly as
 *   `callStoreAction` did. That boundary is deliberate: going straight to IPC
 *   would leave the frontend store stale, and it is the boundary that surfaced
 *   the `updateSection`-cannot-clear-null and undo-fails-outright bugs.
 * - Args are still handed to WebDriver as script arguments, so the same JSON
 *   serialization applies (an absent key stays absent - the `ShotUpdates`
 *   double-option distinction survives by construction).
 * - The BEFORE stack lengths are supplied by Node from the previous step's
 *   response rather than re-read, which is exactly what the two-round-trip
 *   version compared against.
 * - The model bookkeeping (truncate on redo-clear, push on a new entry) applies
 *   the same rules in the same order; Node re-derives cursor/top independently
 *   and asserts the page agrees, so a divergence fails loudly instead of
 *   silently weakening the oracle.
 *
 * The authoritative snapshot array lives in ONE place - the page - and is
 * strictly read-only with respect to app state: the injected oracle only ever
 * calls `get_project_state` / `get_undo_stack` / `get_redo_stack`.
 */
import type { ScenarioContext } from './framework';
import { ORACLE_GLOBAL, oracleInjectionSource, type TargetingDigest } from '../lib/fuzzOracleCore';
import { shareableTestId } from '../../../scripts/lib/shareable-selector.mjs';

/**
 * Every `projectStore` action a fuzz step may dispatch, and nothing else.
 *
 * `runStep` reaches an action by NAME - `store.getState()[req.action]` - and on
 * a replay that name comes out of a trace file, which `readTrace` checks only
 * for its schema version and a `steps` array. Without this set a crafted trace
 * calls any method the store exposes (`loadProject` on a path of its choosing,
 * `saveProject` over a file of its choosing) inside a privileged webview with
 * the `__E2E_TEST__` hooks live.
 *
 * Kept HERE, at the dispatch point, rather than in the generators: this is the
 * boundary, and `undoRedoFuzzBridge.test.ts` pins the other direction - every
 * action the generators and the coverage plan can produce must be in this set,
 * so adding a generator without widening the allowlist fails a unit test rather
 * than a fuzz run.
 */
/**
 * Mutations a REPLAYED trace may drive, with arguments the trace supplies.
 *
 * The tighter of the two sets, and the one that matters for a file from
 * elsewhere: every entry here is produced by `undoRedoFuzzActions.ts` or
 * `undoPersistenceCoverage.ts` with generated arguments, so replaying one with
 * the trace's arguments does what the recording did. `saveProject` is
 * deliberately NOT here even though the scenarios call it constantly - it takes
 * a PATH (`projectStore.saveProject(path?)`), and the scenarios pass their own
 * temp path, never a recorded one. A trace step naming it would be choosing
 * where this process writes a .stalx.
 */
export const REPLAYABLE_MUTATIONS: ReadonlySet<string> = new Set([
  'createLoopClosure',
  'createSection',
  'createSectionAndMerge',
  'createShot',
  'deleteLastShotAndSection',
  'deleteSection',
  'deleteShot',
  'deleteShotAndSplit',
  'deleteShotRangeShift',
  'deleteShotRangeSplit',
  'executeBatch',
  'importSections',
  // ---- v3: the import group (undoRedoFuzzImportActions.ts) ----------------
  // The four file importers and the two file-taking probes DO take a path, so
  // the `saveProject` argument above would seem to exclude them. It does not,
  // and the difference is where the path can point: `FILE_ARG_ACTIONS`
  // (lib/fuzzTrace.ts) makes `readTrace` refuse any of these whose `args[0]` is
  // absolute or escapes the session directory, and `undoRedoFuzz.ts` resolves
  // what is left against a directory IT chose. A trace can pick a name inside
  // the replayer's own scratch space and nothing else - whereas `saveProject`
  // has no such gate and would be choosing where a .stalx gets written.
  //
  // `saveAfterImport` takes NO arguments at all, so a trace choosing it chooses
  // nothing; with `importContext` null (which this scenario asserts at startup)
  // it only marks the frontend dirty.
  'calculateFileHash',
  'checkDuplicateImport',
  'checkSectionHashes',
  'executeCompassImport',
  'executeGpxImport',
  'executeTmlImport',
  'executeWallsImport',
  'importDmp',
  'saveAfterImport',
  'suggestImportPlacement',
  // ------------------------------------------------------------------------
  'mergeStations',
  'moveComment',
  'moveSection',
  'moveSegment',
  'moveStation',
  'removeLoopClosure',
  'removeZeroDatum',
  'reverseSection',
  'setShotKind',
  'setZeroDatum',
  'splitShotAtDistance',
  'unmergeStationPair',
  'updateSection',
  'updateShot',
  'updateStationComment',
]);

/**
 * Every `projectStore` action a fuzz step may dispatch at all: the replayable
 * mutations, plus the control moves the SCENARIO drives with its own arguments
 * (history moves, the save/reload boundary, Repair, and the re-observe
 * sentinel). `runStep` reaches an action by NAME, so without this set a step
 * could call any method the store exposes.
 *
 * The two-set split is the point: `runStep` checks this one, and the replay
 * path checks `REPLAYABLE_MUTATIONS`, so an action the scenario may drive is
 * not thereby an action a FILE may drive.
 *
 * `undoRedoFuzzBridge` owns both, and `lib/fuzzableActions.test.ts` pins the
 * other direction - every action the generators can produce must be replayable,
 * so adding a generator without widening the set fails a unit test rather than
 * a fuzz run.
 */
export const FUZZABLE_ACTIONS: ReadonlySet<string> = new Set([
  ...REPLAYABLE_MUTATIONS,
  // Pure re-observation: no action is dispatched at all (used after a reload).
  'noop:observe',
  // History moves.
  'undo',
  'redo',
  'undoTo',
  'redoTo',
  // The persistence boundary and the repair pass - scenario-driven, with the
  // scenario's own arguments. Never replayable from a trace.
  'saveProject',
  'repairProjectIntegrity',
]);

/** What Node asks the page to do for one step. */
export interface StepRequest {
  /**
   * `undoTo`/`redoTo` are MULTI-step jumps through the history-dropdown path
   * (store `undoTo`/`redoTo` -> IPC `undo_to`/`redo_to`). They move the cursor by
   * `steps`, and the state they land on must equal the snapshot single steps
   * recorded at that depth - that equivalence is the whole point of testing them.
   */
  kind: 'mutate' | 'undo' | 'redo' | 'undoTo' | 'redoTo' | 'save' | 'reload';
  /** projectStore action name. */
  action: string;
  args: unknown[];
  /** Node's current undo depth. */
  cursor: number;
  /** Undo-stack length before this step (from the previous response). */
  undo0: number;
  /** History moves only: how many entries this move crosses (1 for singles). */
  steps?: number;
}

/** `check_project_integrity`, normalized for comparison across depths. */
export interface IntegrityVerdict {
  saveBlocked: boolean;
  /** Sorted, so message order can never masquerade as a difference. */
  messages: string[];
}

/** Everything Node needs to run its case analysis, and nothing else. */
export interface StepResponse {
  /** False when the action threw (the `callStoreAction` `ok:false` case). */
  actionOk: boolean;
  actionError: string | null;
  /** The store outcome's `success`, or null when the action returned no such field. */
  commandSuccess: boolean | null;
  commandError: string | null;
  undoLen: number;
  redoLen: number;
  /** Canonical snapshot length in bytes (the string never crosses the bridge). */
  bytes: number;
  /**
   * `recorded` - a new entry was pushed and its snapshot stored.
   * `equal` / `differ` - compared against the model at `comparedIndex`.
   * `unexpected` - the command pushed something other than 0 or 1 entries;
   *   the page changed nothing and Node fails.
   */
  verdict: 'recorded' | 'equal' | 'differ' | 'unexpected';
  /** Index the comparison used (-1 when nothing was compared). */
  comparedIndex: number;
  cursorAfter: number;
  /** `snapshots.length - 1` after the step. */
  top: number;
  digest: TargetingDigest;
  /**
   * ABSOLUTE well-formedness violations in the observed state (empty = sound).
   * Independent of the roundtrip comparison: these fire even when every snapshot
   * matches, which is the case a corrupting command with a faithful undo hits.
   */
  invariants: string[];
  /** This step's integrity verdict, and the one recorded at the depth landed on. */
  integrity: IntegrityVerdict;
  /** Null when the model has no verdict recorded for this depth yet. */
  integrityExpected: IntegrityVerdict | null;
  /**
   * True when the snapshot being compared against was recorded in an EARLIER
   * epoch - i.e. before a save or reload made an undo-less change to live state.
   * Only such a comparison may subtract the documented save-heal / reload-resolve
   * families; within one epoch the comparison stays strict, so a geometry bug
   * cannot hide behind "a reload re-solves offsets".
   */
  crossesEpoch: boolean;
  /**
   * In-page attribution for the step, in ms. Not diagnostics for their own
   * sake: they are what says whether the remaining cost is the bridge, the
   * store action's own work (`flushPlanAnchors` + the IPC command +
   * `refreshProjectState` + `checkProjectIntegrity`), the three read-only IPC
   * calls, or the canonicalization - so the next optimization is aimed at
   * evidence rather than at a guess.
   */
  timing: { actionMs: number; observeMs: number; canonicalMs: number; integrityMs: number };
}

/** The initial observation, taken before any action. */
export interface InitResponse {
  undoLen: number;
  redoLen: number;
  bytes: number;
  digest: TargetingDigest;
  invariants: string[];
  integrity: IntegrityVerdict;
}

const ORACLE = `globalThis.${ORACLE_GLOBAL}`;

/** Shared preamble: resolve the oracle and the two app handles it reads through. */
const PREAMBLE = `
  const O = ${ORACLE};
  if (!O) throw new Error('fuzz oracle is not injected');
  const core = window.__TAURI__ && window.__TAURI__.core;
  if (!core || typeof core.invoke !== 'function') throw new Error('window.__TAURI__.core.invoke unavailable');
`;

/** Read state + both stack lengths, canonicalize, digest. Read-only. */
const OBSERVE = `
  const observeStarted = Date.now();
  const project = await core.invoke('get_project_state');
  if (project === null || project === undefined) {
    throw new Error('get_project_state returned null: the project closed mid-run');
  }
  const undoLen = (await core.invoke('get_undo_stack')).length;
  const redoLen = (await core.invoke('get_redo_stack')).length;
  const observeMs = Date.now() - observeStarted;
  const canonicalStarted = Date.now();
  const canonical = O.canonical(project, O.idAgnosticMode);
  const digest = O.digest(project);
  const invariants = O.invariants(project);
  const canonicalMs = Date.now() - canonicalStarted;
  // Measured separately: it is a whole extra IPC command doing O(N) structural
  // work, and whether it is worth paying every step is a numbers question.
  const integrityStarted = Date.now();
  const integrityRaw = await core.invoke('check_project_integrity');
  const integrity = {
    saveBlocked: !!(integrityRaw && integrityRaw.save_blocked),
    messages: (integrityRaw && integrityRaw.messages ? integrityRaw.messages.slice() : []).sort(),
  };
  const integrityMs = Date.now() - integrityStarted;
`;

/** Install the oracle (and reset its model) for this run. */
export async function injectOracle(ctx: ScenarioContext): Promise<void> {
  await ctx.eval<boolean>(`${oracleInjectionSource()}\nreturn true;`);
}

/** Record `snapshots[0]` and report the opening observation. */
export async function initModel(ctx: ScenarioContext): Promise<InitResponse> {
  return ctx.eval<InitResponse>(`
    ${PREAMBLE}
    ${OBSERVE}
    O.snapshots = [canonical];
    O.integrity = [integrity];
    O.epoch = 0;
    O.epochs = [0];
    O.lastExpected = null;
    O.lastActual = null;
    O.mismatchIndex = -1;
    return {
      undoLen: undoLen,
      redoLen: redoLen,
      bytes: canonical.length,
      digest: digest,
      invariants: invariants,
      integrity: integrity,
    };
  `);
}

/**
 * One fused step. The case analysis here MIRRORS the scenario's - Node
 * re-derives `cursorAfter`/`top` and asserts they match, so the two cannot
 * drift apart unnoticed.
 */
export async function runStep(ctx: ScenarioContext, request: StepRequest): Promise<StepResponse> {
  // Checked in NODE, before the name can reach the page at all. A replayed
  // trace is the untrusted input here (see `FUZZABLE_ACTIONS`), and this is a
  // hard throw rather than an `actionOk: false` response because a step naming
  // something outside the set is not a failed move - it is not a fuzz step.
  if (!FUZZABLE_ACTIONS.has(request.action)) {
    throw new Error(
      `refusing to dispatch projectStore.${shareableTestId(request.action)}: not in the fuzzable action set. ` +
        `A recorded trace can only replay the actions the generators produce.`,
    );
  }
  return ctx.eval<StepResponse>(
    `
    const req = args[0];
    ${PREAMBLE}
    const store = window.__E2E_TEST__ && window.__E2E_TEST__.projectStore;
    if (!store) throw new Error('window.__E2E_TEST__.projectStore unavailable');

    let actionOk = true;
    let actionError = null;
    let commandSuccess = null;
    let commandError = null;
    const actionStarted = Date.now();
    const action = req.action === 'noop:observe' ? null : store.getState()[req.action];
    if (req.action === 'noop:observe') {
      // Pure re-observation (used right after a reload): no action is dispatched.
      commandSuccess = true;
    } else if (typeof action !== 'function') {
      actionOk = false;
      actionError = 'action ' + req.action + ' is not a function';
    } else {
      try {
        const result = await Promise.resolve(action.apply(undefined, req.args));
        if (result && typeof result === 'object') {
          if ('success' in result) commandSuccess = !!result.success;
          if (result.error !== null && result.error !== undefined) commandError = String(result.error);
        }
      } catch (err) {
        actionOk = false;
        actionError = String(err);
      }
    }
    const actionMs = Date.now() - actionStarted;
    const accepted = actionOk && commandSuccess !== false;

    ${OBSERVE}

    const steps = req.steps === undefined ? 1 : req.steps;
    let cursorAfter = req.cursor;
    let comparedIndex = -1;
    let verdict = null;
    if (req.kind === 'mutate') {
      const pushed = undoLen - req.undo0;
      if (!accepted) {
        comparedIndex = req.cursor;
      } else if (pushed === 1) {
        O.snapshots.length = req.cursor + 1;
        O.snapshots.push(canonical);
        O.integrity.length = req.cursor + 1;
        O.integrity.push(integrity);
        O.epochs.length = req.cursor + 1;
        O.epochs.push(O.epoch);
        cursorAfter = req.cursor + 1;
        verdict = 'recorded';
      } else if (pushed === 0) {
        // A no-op command may still have cleared the redo stack, which
        // truncates the model's forward branch BEFORE the comparison.
        if (redoLen === 0) {
          O.snapshots.length = req.cursor + 1;
          O.integrity.length = req.cursor + 1;
          O.epochs.length = req.cursor + 1;
        }
        comparedIndex = req.cursor;
      } else {
        verdict = 'unexpected';
      }
    } else if (req.kind === 'save' || req.kind === 'reload') {
      // Neither moves the history cursor. Both are compared against the depth
      // they were taken at: a save should be state-preserving apart from its
      // documented healing mutations, and a reload should restore exactly what
      // was saved (modulo the shot-id renumbering the id-agnostic mode absorbs).
      comparedIndex = req.cursor;
    } else {
      const back = req.kind === 'undo' || req.kind === 'undoTo';
      cursorAfter = req.cursor + (back ? -steps : steps);
      comparedIndex = cursorAfter;
    }

    if (verdict === null) {
      const expected = O.snapshots[comparedIndex];
      verdict = expected === canonical ? 'equal' : 'differ';
      if (verdict === 'differ') {
        O.lastExpected = expected === undefined ? null : expected;
        O.lastActual = canonical;
        O.mismatchIndex = comparedIndex;
      }
    }

    return {
      actionOk: actionOk,
      actionError: actionError,
      commandSuccess: commandSuccess,
      commandError: commandError,
      undoLen: undoLen,
      redoLen: redoLen,
      bytes: canonical.length,
      verdict: verdict,
      comparedIndex: comparedIndex,
      cursorAfter: cursorAfter,
      top: O.snapshots.length - 1,
      digest: digest,
      invariants: invariants,
      integrity: integrity,
      integrityExpected: comparedIndex >= 0 && O.integrity[comparedIndex] !== undefined
        ? O.integrity[comparedIndex]
        : null,
      crossesEpoch: comparedIndex >= 0 && (O.epochs[comparedIndex] === undefined || O.epochs[comparedIndex] < O.epoch),
      timing: {
        actionMs: actionMs,
        observeMs: observeMs,
        canonicalMs: canonicalMs,
        integrityMs: integrityMs,
      },
    };
  `,
    [request],
  );
}

/**
 * Fetch the mismatching pair. The ONLY place a full canonical string crosses
 * the bridge, and only after the page has already said they differ.
 */
export async function fetchMismatch(
  ctx: ScenarioContext,
): Promise<{ expected: string | null; actual: string | null; index: number }> {
  return ctx.eval(`
    const O = ${ORACLE};
    if (!O) throw new Error('fuzz oracle is not injected');
    return { expected: O.lastExpected, actual: O.lastActual, index: O.mismatchIndex };
  `);
}

/**
 * Accept the actual state as the model's expectation at `index`.
 * Only ever called for a mismatch every one of whose differences was
 * ATTRIBUTED to a documented undo-less change across an epoch boundary - a
 * save-time heal or a reload re-solve - so the new baseline is recorded once
 * instead of failing the same explained difference at every later comparison.
 * There is no known-bug branch left in either fuzz scenario:
 * `STALAGMIGHT_FUZZ_ALLOW_KNOWN_BUGS` is inert and tolerates nothing.
 */
export async function resyncModel(
  ctx: ScenarioContext,
  index: number,
  integrity: IntegrityVerdict | null,
): Promise<number> {
  return ctx.eval<number>(
    `
    const req = args[0];
    const O = ${ORACLE};
    if (!O) throw new Error('fuzz oracle is not injected');
    if (O.mismatchIndex !== req.index) {
      throw new Error('resync index ' + req.index + ' does not match the recorded mismatch ' + O.mismatchIndex);
    }
    if (O.lastActual === null) throw new Error('no recorded mismatch to resync from');
    O.snapshots[req.index] = O.lastActual;
    // The integrity verdict recorded at that depth belongs to the snapshot being
    // replaced, so it has to move with it or the delta oracle would compare the
    // resynced state against a verdict from the state it superseded.
    if (req.integrity !== null) O.integrity[req.index] = req.integrity;
    O.epochs[req.index] = O.epoch;
    return O.snapshots.length - 1;
  `,
    [{ index, integrity }],
  );
}


/**
 * Switch the model to id-agnostic comparison and re-express every recorded
 * snapshot in that form.
 *
 * Called once, on the first reload of a run. `Shot::id` is not persisted - the
 * loader renumbers - so from the first reload onwards a byte-exact comparison
 * against snapshots recorded pre-reload would report the renumbering as drift.
 * Everything else stays compared exactly, and comparisons BEFORE any reload stay
 * byte-exact, so a run that never saves is unaffected.
 */
export async function enableIdAgnosticModel(ctx: ScenarioContext): Promise<number> {
  return ctx.eval<number>(`
    const O = ${ORACLE};
    if (!O) throw new Error('fuzz oracle is not injected');
    if (O.idAgnosticMode) return O.snapshots.length;
    O.idAgnosticMode = true;
    for (let i = 0; i < O.snapshots.length; i++) {
      O.snapshots[i] = O.idAgnostic(O.snapshots[i]);
    }
    return O.snapshots.length;
  `);
}

/** Re-observe without acting, e.g. straight after a reload. */
export async function observeOnly(ctx: ScenarioContext, cursor: number): Promise<StepResponse> {
  return runStep(ctx, { kind: 'save', action: 'noop:observe', args: [], cursor, undo0: 0 });
}


/**
 * Start a new epoch: something changed live state with no undo entry (a save's
 * healing mutations, or a reload's zero-trust re-solve of derived geometry).
 * Snapshots recorded in an earlier epoch may then differ by exactly those
 * documented families; snapshots recorded since may not.
 */
export async function bumpEpoch(ctx: ScenarioContext): Promise<number> {
  return ctx.eval<number>(`
    const O = ${ORACLE};
    if (!O) throw new Error('fuzz oracle is not injected');
    O.epoch += 1;
    return O.epoch;
  `);
}
