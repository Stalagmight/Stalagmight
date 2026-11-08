import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import {
  MoveLog,
  RERUN_WITH,
  RERUN_WITH_IDS,
  describeRunError,
  failureText,
  isWebDriverStall,
  openReplay,
  throwFailure,
  writeRunTrace,
} from './fuzzHarness';
import { TraceRecorder, failureSignature, readTrace, writeTrace } from './fuzzTrace';
import { foldLocalPaths, shareableFailureText } from './shareableFailure';
import { moveTraceStep } from '../scenarios/importDialogTrace';

// Shared per-run plumbing of the four fuzz scenarios. The failure texts below
// are pinned VERBATIM, because the shrinker compares failure signatures taken
// from them and the operator reads them: the extraction promised both stay
// exactly what each scenario printed before.

const ENV = ['STALAGMIGHT_FUZZ_REPLAY', 'STALAGMIGHT_FUZZ_TRACE'] as const;
let saved: Record<string, string | undefined> = {};
let dir = '';

beforeEach(() => {
  saved = Object.fromEntries(ENV.map((key) => [key, process.env[key]]));
  for (const key of ENV) delete process.env[key];
  dir = fs.mkdtempSync(path.join(os.tmpdir(), 'fuzz-harness-test-'));
});

afterEach(() => {
  for (const key of ENV) {
    const value = saved[key];
    if (value === undefined) delete process.env[key];
    else process.env[key] = value;
  }
  fs.rmSync(dir, { recursive: true, force: true });
});

function recordedTrace(scenario: string, fixture: string): string {
  const recorder = new TraceRecorder();
  recorder.record({ kind: 'undo' });
  return writeTrace(
    path.join(dir, `${scenario}.json`),
    recorder.build({ scenario, seed: 7, iterations: 3, fixture, stream: 'v2', failure: null }),
  );
}

describe('openReplay', () => {
  it('draws its own moves when nothing is set', () => {
    expect(openReplay('scenario-a', 'CaveA.stalx', RERUN_WITH_IDS)).toEqual({
      replayPath: '',
      traceOut: '',
      replay: null,
    });
  });

  it('reports the forced trace path without replaying', () => {
    process.env.STALAGMIGHT_FUZZ_TRACE = path.join(dir, 'out.json');

    const setup = openReplay('scenario-a', 'CaveA.stalx', RERUN_WITH);

    expect(setup.traceOut).toBe(path.join(dir, 'out.json'));
    expect(setup.replay).toBeNull();
  });

  it('reads the trace when it matches this scenario and fixture', () => {
    const file = recordedTrace('scenario-a', 'CaveA.stalx');
    process.env.STALAGMIGHT_FUZZ_REPLAY = file;

    const setup = openReplay('scenario-a', 'CaveA.stalx', RERUN_WITH_IDS);

    expect(setup.replayPath).toBe(file);
    expect(setup.replay?.steps).toEqual([{ kind: 'undo' }]);
    expect(setup.replay?.seed).toBe(7);
  });

  it('refuses a trace another scenario recorded', () => {
    const file = recordedTrace('scenario-b', 'CaveA.stalx');
    process.env.STALAGMIGHT_FUZZ_REPLAY = file;

    expect(() => openReplay('scenario-a', 'CaveA.stalx', RERUN_WITH_IDS)).toThrow(
      new Error(`trace ${foldLocalPaths(file)} was recorded by another scenario; run it with that scenario\n  recorded by: scenario-b`),
    );
  });

  it('refuses a trace recorded on another fixture, in the undo fuzzers\' wording', () => {
    const file = recordedTrace('scenario-a', 'CaveB.stalx');
    process.env.STALAGMIGHT_FUZZ_REPLAY = file;

    expect(() => openReplay('scenario-a', 'CaveA.stalx', RERUN_WITH_IDS)).toThrow(
      new Error(
        `trace ${foldLocalPaths(file)} was recorded on another fixture but this run is on CaveA.stalx ` +
          '(pnpm agent:scenario --trace restores it for you)\n' +
          '  its steps carry ids resolved against that project. Re-run with STALAGMIGHT_FUZZ_FIXTURE=CaveB.stalx',
      ),
    );
  });

  it('refuses a trace recorded on another fixture, in the UI fuzzers\' wording', () => {
    const file = recordedTrace('scenario-a', 'CaveB.stalx');
    process.env.STALAGMIGHT_FUZZ_REPLAY = file;

    expect(() => openReplay('scenario-a', 'CaveA.stalx', RERUN_WITH)).toThrow(
      new Error(
        `trace ${foldLocalPaths(file)} was recorded on another fixture but this run is on CaveA.stalx ` +
          '(pnpm agent:scenario --trace restores it for you)\n' +
          '  re-run with STALAGMIGHT_FUZZ_FIXTURE=CaveB.stalx',
      ),
    );
  });

  it("keeps the trace's own scenario and fixture off the line a runner prints (#576 W51)", () => {
    // recordedTrace names the file after its scenario, so the forged one is
    // written under a neutral name: only the header carries the token.
    const forged = path.join(dir, 'forged.json');
    fs.copyFileSync(recordedTrace('qzForgedScenario', 'qzCaveB.stalx'), forged);
    process.env.STALAGMIGHT_FUZZ_REPLAY = forged;
    const headline = (): string => {
      try {
        openReplay('scenario-a', 'CaveA.stalx', RERUN_WITH);
      } catch (err) {
        return shareableFailureText((err as Error).message).split('\n')[0] ?? '';
      }
      return '';
    };
    expect(headline()).toBe(`trace ${foldLocalPaths(forged)} was recorded by another scenario; run it with that scenario`);
    process.env.STALAGMIGHT_FUZZ_REPLAY = recordedTrace('scenario-a', 'qzCaveB.stalx');
    expect(headline()).toContain('was recorded on another fixture');
    expect(headline()).not.toContain('qzCaveB');
  });
});

describe('MoveLog', () => {
  it('keeps the last N lines, indented, and every line when asked', () => {
    const log = new MoveLog();
    const { note } = log;
    for (const line of ['#1 a', '#2 b', '#3 c']) note(line);

    expect(log.history).toEqual(['#1 a', '#2 b', '#3 c']);
    expect(log.tail(2)).toEqual(['    #2 b', '    #3 c']);
    expect(log.tail(2, true)).toEqual(['    #1 a', '    #2 b', '    #3 c']);
  });

  it('names a move that never returned, after the tail, until it is disarmed', () => {
    const log = new MoveLog();
    const { note, startingMove, finishedMove } = log;
    note('#1 undo -> depth 0');
    startingMove('#2 redo');

    expect(log.tail(5)).toEqual(['    #1 undo -> depth 0', '    #2 redo -> IN FLIGHT (had not returned)']);
    // The in-flight line is EXTRA: the limit counts history lines only.
    expect(log.tail(1)).toEqual(['    #1 undo -> depth 0', '    #2 redo -> IN FLIGHT (had not returned)']);

    finishedMove();
    expect(log.tail(5)).toEqual(['    #1 undo -> depth 0']);
  });
});

describe('writeRunTrace', () => {
  const meta = { scenario: 'scenario-a', iterations: 4, fixture: 'CaveA.stalx', stream: 'plan' };

  function recorder(): TraceRecorder {
    const out = new TraceRecorder();
    out.record({ kind: 'save' });
    return out;
  }

  it('writes to the explicit path when one is given, with the run header', () => {
    const target = path.join(dir, 'explicit.json');

    const written = writeRunTrace(
      { artifactsDir: path.join(dir, 'artifacts'), seed: 9 },
      recorder(),
      meta,
      { message: 'boom' },
      target,
    );

    expect(written).toBe(target);
    const trace = readTrace(target);
    expect(trace.scenario).toBe('scenario-a');
    expect(trace.seed).toBe(9);
    expect(trace.iterations).toBe(4);
    expect(trace.fixture).toBe('CaveA.stalx');
    expect(trace.stream).toBe('plan');
    expect(trace.failure?.message).toBe('boom');
    expect(trace.steps).toEqual([{ kind: 'save' }]);
    expect(trace.noProjectEpisodes).toBeUndefined();
    expect(fs.existsSync(path.join(dir, 'artifacts'))).toBe(false);
  });

  it('falls back to a timestamped file under the artifacts dir', () => {
    const artifacts = path.join(dir, 'artifacts');

    const written = writeRunTrace({ artifactsDir: artifacts, seed: 9 }, recorder(), meta, null, '');

    expect(written).not.toBeNull();
    expect(path.dirname(written ?? '')).toBe(path.join(artifacts, 'traces'));
    expect(path.basename(written ?? '')).toMatch(/^scenario-a-seed9-.*\.json$/);
    expect(readTrace(written ?? '').failure).toBeNull();
  });

  it('answers null, without throwing, when there is nowhere to write or the write fails', () => {
    expect(writeRunTrace({ artifactsDir: null, seed: 9 }, recorder(), meta, null, '')).toBeNull();
    // A directory where the file should go: the write throws, the helper must not.
    expect(writeRunTrace({ artifactsDir: null, seed: 9 }, recorder(), meta, null, dir)).toBeNull();
  });

  it('carries the no-project episodes when there are any', () => {
    const target = path.join(dir, 'episodes.json');

    writeRunTrace({ artifactsDir: null, seed: 9 }, recorder(), { ...meta, noProjectEpisodes: [0, 2] }, null, target);

    expect(readTrace(target).noProjectEpisodes).toEqual([0, 2]);
  });
});

describe('failureText', () => {
  it('is the headline, the context, the detail, then the counted tail', () => {
    expect(
      failureText('undo failed', ['  reproduce: x', '  depth=1'], '  error: boom', 'action(s)', ['    #1 a', '    #2 b']),
    ).toBe(
      ['undo failed', '  reproduce: x', '  depth=1', '  error: boom', '  last 2 action(s):', '    #1 a', '    #2 b'].join(
        '\n',
      ),
    );
  });

  it('leaves out an empty detail rather than printing a blank line', () => {
    expect(failureText('h', ['  c'], '', 'move(s)', [])).toBe('h\n  c\n  last 0 move(s):');
  });
});

describe('throwFailure', () => {
  it('records the message VERBATIM, then throws it with the trace path appended', () => {
    const seen: string[] = [];

    expect(() =>
      throwFailure('the message', (failure) => {
        seen.push(failure.message);
        return '/traces/t.json';
      }),
    ).toThrow(new Error('the message\n  trace: /traces/t.json'));
    expect(seen).toEqual(['the message']);
  });

  it('throws the bare message when no trace was written', () => {
    expect(() => throwFailure('the message', () => null)).toThrow(new Error('the message'));
  });
});

describe('a run that dies outside its own fail (#576 HFI-9)', () => {
  /**
   * What webdriver 9 throws when a fused in-page step outruns the 30 s request
   * budget: `WebDriverError.computeErrorMessage` wraps the DOMException text
   * with the command and the in-page script.
   */
  const timeout = (): Error =>
    new Error(
      'WebDriverError: The operation was aborted due to timeout when running "execute/async" with method ' +
        '"POST" and args "async function fusedStep(args) { const done = arguments[arguments.length - 1]; ..."',
    );

  it('recognises every spelling of a stalled page, and nothing else', () => {
    expect(isWebDriverStall(timeout())).toBe(true);
    expect(isWebDriverStall(new Error('Request timed out! Consider increasing the "connectionRetryTimeout" option.'))).toBe(
      true,
    );
    expect(isWebDriverStall(new Error('script timeout: the script did not finish within 30000 ms'))).toBe(true);
    expect(isWebDriverStall(new Error('could not turn auto-save off; a timer firing mid-run would change state'))).toBe(
      false,
    );
    expect(isWebDriverStall(new Error('S4 UNEXPECTED ERROR DIALOG: a move with no business failing raised one'))).toBe(
      false,
    );
  });

  it('turns the bare timeout into a STALL that names the move that never returned', () => {
    const report = describeRunError(timeout(), 'click shot-editor-ctx-cut - cut the selection');
    expect(report.stall).toBe(true);
    expect(report.headline).toBe(
      'STALL: execute/async did not return within the WebDriver budget - the page stopped answering',
    );
    expect(report.detail).toContain('  stalled move: click shot-editor-ctx-cut - cut the selection');
    expect(report.detail).toContain('  error: WebDriverError: The operation was aborted due to timeout');
  });

  it('gives every stall of one command one signature, whatever the seed or the move', () => {
    const a = describeRunError(timeout(), 'FORCED advance on wiz-manual');
    const b = describeRunError(timeout(), 'select row 41');
    const sign = (report: { headline: string; detail: string }, seed: number) =>
      failureSignature(
        failureText(report.headline, [`  reproduce: pnpm agent:scenario x --seed ${seed}`], report.detail, 'move(s)', []),
      );
    expect(sign(a, 1).headline).toBe(sign(b, 32).headline);
  });

  it('keeps any other error under its own first line', () => {
    const report = describeRunError(new Error('E2 INTEGRITY: x.stalx opened SAVE-BLOCKED\n["a"]'), null);
    expect(report.stall).toBe(false);
    expect(report.headline).toBe('E2 INTEGRITY: x.stalx opened SAVE-BLOCKED');
    expect(report.detail).toBe('  in flight: (none)\n["a"]');
  });

  it('writes the trace up to and including the stalled move, with the reproduce line', () => {
    // The scenario's catch, from the same helpers it calls: the steps recorded
    // so far, then the in-flight step the dispatch left behind, then `fail`.
    const recorder = new TraceRecorder();
    recorder.record({ kind: 'ui', testid: 'import-wizard-next', label: 'next' });
    const stalled = moveTraceStep({ kind: 'ui', testid: 'shot-editor-ctx-cut', label: 'cut the selection' });
    expect(stalled).not.toBeNull();
    const log = new MoveLog();
    log.note('next [wiz-manual]');
    log.startingMove('cut the selection');
    expect(log.inFlight()).toBe('cut the selection');

    const report = describeRunError(timeout(), log.inFlight());
    if (stalled !== null) recorder.record(stalled);
    const message = failureText(
      report.headline,
      ['  reproduce: pnpm agent:scenario importDialogFuzz --seed 19 --iterations 8'],
      report.detail,
      'move(s)',
      log.tail(25),
    );
    const meta = { scenario: 'importDialogFuzz', iterations: 8, fixture: 'f.stalx', stream: 'import-v3' };
    let thrown = '';
    try {
      throwFailure(message, (failure) => writeRunTrace({ artifactsDir: dir, seed: 19 }, recorder, meta, failure, ''));
    } catch (err) {
      thrown = err instanceof Error ? err.message : String(err);
    }
    expect(thrown.split('\n')[0]).toMatch(/^STALL: /);
    expect(thrown).toContain('  reproduce: pnpm agent:scenario importDialogFuzz --seed 19');
    expect(thrown).toContain('    cut the selection -> IN FLIGHT (had not returned)');
    const tracePath = /\n {2}trace: (.+)$/.exec(thrown)?.[1] ?? '';
    const trace = readTrace(tracePath);
    expect(trace.steps.at(-1)).toEqual(stalled);
    expect(trace.failure?.message.split('\n')[0]).toMatch(/^STALL: /);
  });
});
