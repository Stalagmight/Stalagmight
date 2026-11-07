import { describe, expect, it } from 'vitest';

import type { ScenarioContext } from '../scenarios/framework';
import type { IntegrityVerdict, StepResponse } from '../scenarios/undoRedoFuzzBridge';
import { MoveLog } from './fuzzHarness';
import type { FuzzTrace, TraceStep } from './fuzzTrace';
import {
  assertHistoryStep,
  assertReplayableMutation,
  historyReach,
  readUndoPersistLimit,
  replayTrace,
  requireAutoSaveOff,
  undoFuzzHarness,
  type UndoModelState,
} from './undoFuzzHarness';

// The undo-model plumbing both undo fuzzers share, driven against a FAKE page:
// every bridge call is answered from a script and logged, so the tests can pin
// what each helper sends (cursor, undo0, args), in what ORDER, and what it
// does with the answer - the properties the extraction had to keep identical.

const CLEAN: IntegrityVerdict = { saveBlocked: false, messages: [] };

function response(overrides: Partial<StepResponse> = {}): StepResponse {
  return {
    actionOk: true,
    actionError: null,
    commandSuccess: true,
    commandError: null,
    undoLen: 0,
    redoLen: 0,
    bytes: 10,
    verdict: 'equal',
    comparedIndex: 0,
    cursorAfter: 0,
    top: 0,
    digest: { s: [] } as unknown as StepResponse['digest'],
    invariants: [],
    integrity: CLEAN,
    integrityExpected: null,
    crossesEpoch: false,
    timing: { actionMs: 1, observeMs: 2, canonicalMs: 3, integrityMs: 4 },
    ...overrides,
  };
}

interface FakePage {
  ctx: ScenarioContext;
  calls: string[];
  stepRequests: Record<string, unknown>[];
  resyncArgs: unknown[];
  metrics: Record<string, number>;
}

/** A page that answers the bridge from scripted queues and logs every call. */
function fakePage(script: {
  steps?: StepResponse[];
  mismatch?: { expected: string; actual: string; index: number };
  resyncTop?: number;
  actions?: Record<string, { ok: boolean; error?: string; result?: unknown }>;
}): FakePage {
  const calls: string[] = [];
  const stepRequests: Record<string, unknown>[] = [];
  const resyncArgs: unknown[] = [];
  const metrics: Record<string, number> = {};
  const steps = [...(script.steps ?? [])];
  const evalFn = async (body: string, args: unknown[] = []): Promise<unknown> => {
    const first = args[0] as Record<string, unknown> | undefined;
    if (first && typeof first.kind === 'string' && typeof first.action === 'string') {
      calls.push(`step:${first.kind}:${first.action}`);
      stepRequests.push(first);
      const next = steps.shift();
      if (!next) throw new Error(`no scripted response for ${first.action}`);
      return next;
    }
    if (body.includes('O.lastExpected')) {
      calls.push('fetchMismatch');
      return script.mismatch;
    }
    if (body.includes('O.snapshots[req.index] = O.lastActual')) {
      calls.push('resyncModel');
      resyncArgs.push(first);
      return script.resyncTop ?? 0;
    }
    if (body.includes('O.idAgnosticMode = true')) {
      calls.push('enableIdAgnosticModel');
      return 1;
    }
    if (body.includes('O.epoch += 1')) {
      calls.push('bumpEpoch');
      return 1;
    }
    throw new Error(`unexpected eval: ${body.slice(0, 80)}`);
  };
  const ctx = {
    eval: evalFn,
    callAction: async (store: string, action: string, args: unknown[] = []) => {
      calls.push(`action:${store}.${action}(${JSON.stringify(args)})`);
      return script.actions?.[action] ?? { ok: true, result: { success: true } };
    },
    metric: (key: string, delta = 1) => {
      metrics[key] = (metrics[key] ?? 0) + delta;
    },
    metrics,
  } as unknown as ScenarioContext;
  return { ctx, calls, stepRequests, resyncArgs, metrics };
}

function model(initial: Partial<UndoModelState> = {}): UndoModelState {
  return { cursor: 0, top: 0, undoLen: 0, savesDone: 0, reloadsDone: 0, ...initial };
}

/** A `fail` that throws its two arguments so a test can read both. */
const fail = (headline: string, detail = ''): never => {
  throw new Error(`${headline}\n${detail}`);
};

function harness(page: FakePage, state: UndoModelState, overrides: { baseUndo?: number; noteFailedSave?: boolean } = {}) {
  const log = new MoveLog();
  const recorded: StepResponse[] = [];
  const h = undoFuzzHarness(page.ctx, state, {
    fail,
    log,
    record: (res) => recorded.push(res),
    baseUndo: overrides.baseUndo ?? 0,
    limits: { diffLimit: 200, shownDiffs: 6 },
    noteFailedSave: overrides.noteFailedSave ?? true,
  });
  return { h, log, recorded };
}

describe('undoFuzzHarness.step', () => {
  it('sends the model cursor and undo length AT CALL TIME, and times the step', async () => {
    const page = fakePage({ steps: [response(), response()] });
    const state = model({ cursor: 2, undoLen: 5 });
    const { h } = harness(page, state);

    await h.step('undo', 'undo', []);
    // The view is live: a later step sees the scenario's updated variables.
    Object.assign(state, { cursor: 1, undoLen: 4 });
    await h.step('undoTo', 'undoTo', [0], 3);

    expect(page.stepRequests).toEqual([
      { kind: 'undo', action: 'undo', args: [], cursor: 2, undo0: 5, steps: 1 },
      { kind: 'undoTo', action: 'undoTo', args: [0], cursor: 1, undo0: 4, steps: 3 },
    ]);
    expect(page.metrics).toMatchObject({ actionMs: 2, observeMs: 4, canonicalMs: 6, integrityMs: 8, steps: 2 });
    expect(page.metrics.fusedStepMs).toBeGreaterThanOrEqual(0);
  });
});

describe('undoFuzzHarness assertions', () => {
  it('assertStacks measures depth from baseUndo', () => {
    const { h } = harness(fakePage({}), model({ cursor: 2, top: 3 }), { baseUndo: 1 });

    expect(() => h.assertStacks('ok', response({ undoLen: 3, redoLen: 1 }))).not.toThrow();
    expect(() => h.assertStacks('stacks moved', response({ undoLen: 2, redoLen: 1 }))).toThrow(
      new Error('stacks moved\n  expected undoLen=3 redoLen=1, got undoLen=2 redoLen=1'),
    );
  });

  it('assertModel compares the page model with Node', () => {
    const { h } = harness(fakePage({}), model({ cursor: 1, top: 2 }));

    expect(() => h.assertModel(response({ cursorAfter: 1, top: 2 }), 'ok')).not.toThrow();
    expect(() => h.assertModel(response({ cursorAfter: 0, top: 2 }), 'drift')).toThrow(
      new Error('drift\n  page model says cursor=0 top=2, Node says cursor=1 top=2'),
    );
  });

  it('assertInvariants lists every violation', () => {
    const { h } = harness(fakePage({}), model());

    expect(() => h.assertInvariants(response({ invariants: ['a', 'b'] }), 'undo')).toThrow(
      new Error('project invariant(s) violated after undo\n  INVARIANT: a\n  INVARIANT: b'),
    );
  });

  it('assertIntegrityDelta compares, skips a resynced step, and counts both', () => {
    const page = fakePage({});
    const { h } = harness(page, model({ cursor: 4 }));
    const blocked: IntegrityVerdict = { saveBlocked: true, messages: ['m'] };

    h.assertIntegrityDelta(response(), 'undo');
    expect(page.metrics.integrityCompared).toBeUndefined();

    h.assertIntegrityDelta(response({ integrityExpected: CLEAN }), 'undo');
    expect(page.metrics.integrityCompared).toBe(1);

    h.assertIntegrityDelta(response({ integrityExpected: CLEAN, integrity: blocked }), 'undo', true);
    expect(page.metrics.integrityResynced).toBe(1);

    expect(() => h.assertIntegrityDelta(response({ integrityExpected: CLEAN, integrity: blocked }), 'redo')).toThrow(
      new Error(
        'redo restored the state but NOT the integrity verdict recorded at depth 4\n' +
          '  expected: saveBlocked=false messages=[]\n  actual:   saveBlocked=true messages=["m"]',
      ),
    );
  });
});

describe('undoFuzzHarness.resolveMismatch', () => {
  const before = JSON.stringify({ project: { sections: [{ gauge_neighbour_uuid: null, gauge_shot_uuid: null, id: 0 }] } });
  const after = JSON.stringify({ project: { sections: [{ gauge_neighbour_uuid: 'g', gauge_shot_uuid: 'g', id: 0 }] } });

  it('RESYNCS on a documented save heal across an epoch, moving top and the integrity verdict', async () => {
    const page = fakePage({ mismatch: { expected: before, actual: after, index: 1 }, resyncTop: 3 });
    const state = model({ top: 1, savesDone: 1 });
    const { h } = harness(page, state);
    const blocked: IntegrityVerdict = { saveBlocked: true, messages: ['m'] };

    const resynced = await h.resolveMismatch('headline', response({ crossesEpoch: true, integrity: blocked }));

    expect(resynced).toBe(true);
    expect(state.top).toBe(3);
    expect(page.calls).toEqual(['fetchMismatch', 'resyncModel']);
    expect(page.resyncArgs).toEqual([{ index: 1, integrity: blocked }]);
    expect(page.metrics).toMatchObject({ resyncs: 1, 'resync:save-captures-missing-gauge-anchors': 1 });
  });

  it('stays STRICT within an epoch, and when no save has happened', async () => {
    for (const [crossesEpoch, savesDone] of [
      [false, 1],
      [true, 0],
    ] as const) {
      const page = fakePage({ mismatch: { expected: before, actual: after, index: 1 } });
      const { h } = harness(page, model({ savesDone }));

      await expect(h.resolveMismatch('undo did not restore', response({ crossesEpoch }))).rejects.toThrow(
        /^undo did not restore\n {2}at project\.sections\[0\]\.gauge_neighbour_uuid/,
      );
      expect(page.calls).toEqual(['fetchMismatch']);
    }
  });

  it('counts float noise and keeps the original expectation', async () => {
    const page = fakePage({ mismatch: { expected: '{"x":1}', actual: '{"x":1.0000000000001}', index: 0 } });
    const { h } = harness(page, model());

    expect(await h.resolveMismatch('h', response())).toBe(false);
    expect(page.metrics.floatNoise).toBe(1);
    expect(page.calls).toEqual(['fetchMismatch']);
  });
});

describe('undoFuzzHarness.saveAndReload', () => {
  it('bumps, saves, closes, reopens, goes id-agnostic, bumps, re-observes - in that order', async () => {
    const page = fakePage({
      steps: [response({ undoLen: 2, redoLen: 1 }), response({ undoLen: 2, redoLen: 1 })],
    });
    const state = model({ cursor: 2, top: 3, undoLen: 2 });
    const { h, log, recorded } = harness(page, state);

    await h.saveAndReload('#3', '/tmp/cycle.stalx');

    expect(page.calls).toEqual([
      'bumpEpoch',
      'step:save:saveProject',
      'action:project.closeProject([])',
      'action:project.loadProject(["/tmp/cycle.stalx"])',
      'enableIdAgnosticModel',
      'bumpEpoch',
      'step:save:noop:observe',
    ]);
    expect(page.stepRequests[0]).toEqual({
      kind: 'save',
      action: 'saveProject',
      args: ['/tmp/cycle.stalx'],
      cursor: 2,
      undo0: 2,
      steps: 1,
    });
    // observeOnly sends undo0: 0 and no steps, exactly as the bridge always did.
    expect(page.stepRequests[1]).toEqual({ kind: 'save', action: 'noop:observe', args: [], cursor: 2, undo0: 0 });
    expect(state.savesDone).toBe(1);
    expect(state.reloadsDone).toBe(1);
    expect(recorded).toHaveLength(2);
    expect(log.history).toEqual([
      '#3 save -> ok (depth 2, undo 2, redo 1)',
      '#3 reload -> depth 2 (undo 2, redo 1)',
    ]);
    // [#576 RV7a-7] Read through the rendering the failure text uses: no move left armed.
    expect(log.tail(log.history.length).some((line) => line.endsWith('-> IN FLIGHT (had not returned)'))).toBe(false);
    expect(page.metrics).toMatchObject({ saves: 1, reloads: 1 });
  });

  it('notes a refused save only when the scenario asked for it', async () => {
    for (const noteFailedSave of [true, false]) {
      const page = fakePage({ steps: [response({ commandSuccess: false, commandError: 'disk full' })] });
      const { h, log } = harness(page, model(), { noteFailedSave });

      await expect(h.saveAndReload('#1', '/tmp/s.stalx')).rejects.toThrow(
        new Error('saveProject failed\n  error: disk full'),
      );
      expect(log.history).toEqual(noteFailedSave ? ['#1 save -> FAILED'] : []);
    }
  });

  it('leaves close/reopen IN FLIGHT when the reopen fails', async () => {
    const page = fakePage({
      steps: [response()],
      actions: { loadProject: { ok: true, result: { success: false, error: 'locked' } } },
    });
    const { h, log } = harness(page, model());

    await expect(h.saveAndReload('#1', '/tmp/s.stalx')).rejects.toThrow(
      new Error('reopening the saved project failed\n  error: locked'),
    );
    expect(log.tail(1).at(-1)).toBe('    #1 close/reopen -> IN FLIGHT (had not returned)');
  });
});

describe('run setup', () => {
  /** A page whose every eval answers `answer`, logging the bodies it was sent. */
  function evalPage(answer: unknown): { ctx: ScenarioContext; bodies: string[] } {
    const bodies: string[] = [];
    const ctx = {
      eval: async (body: string) => {
        bodies.push(body);
        return answer;
      },
    } as unknown as ScenarioContext;
    return { ctx, bodies };
  }

  it('requireAutoSaveOff passes when the page turned auto-save off, and refuses otherwise', async () => {
    await expect(requireAutoSaveOff(evalPage(true).ctx)).resolves.toBeUndefined();
    await expect(requireAutoSaveOff(evalPage(false).ctx)).rejects.toThrow(
      new Error(
        'could not disable auto-save; a save mutates live state with no undo entry and would ' +
          'desync the model (see disableAutoSave in scenarios/common.ts)',
      ),
    );
  });

  it('readUndoPersistLimit returns what the page read from config.general.undo_persist_limit', async () => {
    const page = evalPage(17);

    expect(await readUndoPersistLimit(page.ctx)).toBe(17);
    expect(page.bodies[0]).toContain('config.general.undo_persist_limit');
    expect(page.bodies[0]).toContain(': 20;');
  });
});

describe('replay-step rules', () => {
  it('assertReplayableMutation refuses a control action and accepts a generator mutation', () => {
    expect(() => assertReplayableMutation('createSection')).not.toThrow();
    expect(() => assertReplayableMutation('saveProject')).toThrow(
      new Error(
        'trace step names an action that is not a replayable mutation. ' +
          'A recorded trace may only replay the actions the generators produce.\n  action: saveProject',
      ),
    );
    // The name a trace carries stays off the line a runner prints (#576 W51).
    expect(() => assertReplayableMutation('## qzCaveA')).toThrow(/^trace step names an action that[^\n]*\n {2}action: ## qzCaveA$/);
  });

  it('assertHistoryStep lets only the four history kinds through, naming the scenario otherwise', () => {
    const history = [{ kind: 'undo' }, { kind: 'redo' }, { kind: 'undoTo', steps: 2 }, { kind: 'redoTo', steps: 1 }];
    for (const step of history) {
      expect(() => assertHistoryStep(step as TraceStep, 'scenario-a')).not.toThrow();
    }
    for (const step of [{ kind: 'save' }, { kind: 'repair' }, { kind: 'ui', testid: 't', label: 'l' }]) {
      expect(() => assertHistoryStep(step as TraceStep, 'scenario-a')).toThrow(
        new Error(
          `trace step kind ${step.kind} is not one scenario-a replays; ` +
            'it belongs to a scenario that drives the import/export UI.',
        ),
      );
    }
  });

  it('historyReach measures undo reach from the cursor and redo reach to the top', () => {
    const page = fakePage({});

    expect(historyReach(page.ctx, { kind: 'undoTo', steps: 9 }, 3, 5)).toEqual({ back: true, reach: 3 });
    expect(historyReach(page.ctx, { kind: 'redo' }, 3, 5)).toEqual({ back: false, reach: 2 });
    expect(page.metrics).toEqual({});
  });

  it('historyReach SKIPS a move with nowhere to go, and counts it by kind', () => {
    const page = fakePage({});

    expect(historyReach(page.ctx, { kind: 'undo' }, 0, 4)).toBeNull();
    expect(historyReach(page.ctx, { kind: 'redoTo', steps: 1 }, 4, 4)).toBeNull();
    expect(historyReach(page.ctx, { kind: 'redoTo', steps: 1 }, 4, 4)).toBeNull();
    expect(page.metrics).toEqual({ 'replaySkipped:undo': 1, 'replaySkipped:redoTo': 2 });
  });

  it('replayTrace applies every step in order, tagged T1.., and counts them', async () => {
    const metrics: Record<string, number> = {};
    const ctx = {
      setMetric: (key: string, value: number) => {
        metrics[key] = value;
      },
    } as unknown as ScenarioContext;
    const seen: string[] = [];
    const steps: TraceStep[] = [{ kind: 'undo' }, { kind: 'save' }, { kind: 'redoTo', steps: 2 }];

    await replayTrace(ctx, { steps } as FuzzTrace, async (step, tag) => {
      seen.push(`${tag}:${step.kind}`);
    });

    expect(seen).toEqual(['T1:undo', 'T2:save', 'T3:redoTo']);
    expect(metrics).toEqual({ replaySteps: 3 });
  });
});
