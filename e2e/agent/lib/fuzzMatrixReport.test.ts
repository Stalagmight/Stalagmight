import { describe, expect, it } from 'vitest';

import {
  FAIL_BLOCK_LINES,
  TAIL_LINES,
  cellVerdict,
  failureDetail,
  matrixSummary,
  type CellExit,
  type CellVerdict,
} from './fuzzMatrixReport';

/** A scenario failure as `scenario-run.ts` prints it: FAIL line, then the message. */
const ORACLE_FAILURE = [
  'session ready (debug build fresh)',
  '   . scenario import-dialog-fuzz start (seed=8, iterations=8)',
  'FAIL: import-dialog-fuzz threw:',
  'F3 ACCOUNTING: the receipt does not describe what landed in the project',
  '  receipt:  1 section(s), 7 shot(s)',
  '  project:  0 section(s), 0 shot(s)',
  '  reproduce: pnpm agent:scenario import-dialog-fuzz --seed 8 --iterations 8',
  '  last 2 move(s):',
  '    walls import [receipt]',
  '    INTERJECT: undo mid-flow [receipt]',
  '  trace: /tmp/sessions/x/traces/import-dialog-fuzz-seed8.json',
  '',
];

describe('failureDetail', () => {
  it('keeps the whole FAIL block for a failure that HAS a reproduce line', () => {
    // The regression this file exists for: an oracle failure always carries a
    // `reproduce:` line, and the printer used to keep the tail only when there
    // was none - so the numbers, the moves and the trace path were dropped for
    // exactly the failures that had them.
    const detail = failureDetail({ outLines: ORACLE_FAILURE, allLines: ORACLE_FAILURE });
    expect(detail.split('\n')[0]).toBe('FAIL: import-dialog-fuzz threw:');
    expect(detail).toContain('receipt:  1 section(s), 7 shot(s)');
    expect(detail).toContain('INTERJECT: undo mid-flow [receipt]');
    expect(detail).toContain('trace: /tmp/sessions/x/traces/import-dialog-fuzz-seed8.json');
  });

  it('drops the lines before the FAIL line, and the trailing blank', () => {
    const detail = failureDetail({ outLines: ORACLE_FAILURE, allLines: ORACLE_FAILURE });
    expect(detail).not.toContain('session ready');
    expect(detail.endsWith('.json')).toBe(true);
  });

  it('caps a runaway block', () => {
    const long = ['FAIL: x threw:', ...Array.from({ length: 400 }, (_unused, i) => `line ${i}`)];
    const detail = failureDetail({ outLines: long, allLines: long });
    expect(detail.split('\n')).toHaveLength(FAIL_BLOCK_LINES);
  });

  it('falls back to the tail of ALL output when the child printed no FAIL line', () => {
    // A crash, a timeout or a driver that never attached: stderr is the only
    // account there is, and stdout has no FAIL line to anchor on.
    // The tail is shareable (#576 W23-NF1): stack frames - code locations -
    // print as they are, anything else is counted.
    const outLines = ['session ready', ''];
    const allLines = [...outLines, ...Array.from({ length: 40 }, (_unused, i) => `    at f${i} (/x/y.ts:${i}:1)`)];
    const detail = failureDetail({ outLines, allLines });
    const lines = detail.split('\n');
    expect(lines).toHaveLength(TAIL_LINES);
    expect(lines[lines.length - 1]).toBe('    at f39 (/x/y.ts:39:1)');
    expect(detail).not.toContain('\n\n');
    const text = failureDetail({ outLines, allLines: [...allLines, 'stderr 40', 'stderr 41'] }).split('\n');
    expect(text.slice(-2)).toEqual([
      '    at f39 (/x/y.ts:39:1)',
      "  (2 line(s) withheld: they can quote the app's text - the matrix does not keep them)",
    ]);
  });

  it('anchors on the FIRST FAIL line when a run printed more than one', () => {
    const outLines = ['FAIL: a threw:', 'first', 'FAIL: b threw:', 'second'];
    const detail = failureDetail({ outLines, allLines: outLines });
    expect(detail.split('\n')[0]).toBe('FAIL: a threw:');
    expect(detail).toContain('first');
  });
});

describe('cellVerdict (#576 HFI-7)', () => {
  const exit = (over: Partial<CellExit>): CellExit => ({
    spawnFailed: false,
    timedOut: false,
    code: 0,
    sawPass: false,
    sawFail: false,
    ...over,
  });

  it('calls a cell that died without PASS or FAIL lost, not failed', () => {
    // The HFI-7 shape: the second app of a pair was killed by the single-instance
    // plugin, the harness never attached, and the child exited 1 at ~33 s.
    expect(cellVerdict(exit({ code: 1 }))).toBe('lost');
    expect(cellVerdict(exit({ code: null }))).toBe('lost');
  });

  it('calls a spawn failure or a timed-out cell lost', () => {
    expect(cellVerdict(exit({ spawnFailed: true, code: null }))).toBe('lost');
    expect(cellVerdict(exit({ timedOut: true, code: null, sawPass: true }))).toBe('lost');
  });

  it('keeps a scenario verdict a verdict', () => {
    expect(cellVerdict(exit({ code: 0, sawPass: true }))).toBe('pass');
    expect(cellVerdict(exit({ code: 1, sawFail: true }))).toBe('fail');
    expect(cellVerdict(exit({ code: 1, sawPass: true }))).toBe('fail');
  });
});

describe('matrixSummary (#576 HFI-7)', () => {
  const cells = (verdicts: CellVerdict[], durationMs: number) => verdicts.map((verdict) => ({ verdict, durationMs }));

  it('does not print a speedup for a matrix that lost cells', () => {
    // 9 of 12 lost at ~33 s, 3 that ran ~120 s: the run that printed "speedup 3.91x".
    const run = [...cells(Array<CellVerdict>(9).fill('lost'), 33_000), ...cells(['pass', 'pass', 'pass'], 120_000)];
    const text = matrixSummary(run, 150_000).join('\n');
    expect(text).not.toMatch(/speedup \d/);
    expect(text).toContain('speedup NOT reported: 9 cell(s) LOST');
    expect(text).toContain('0 failed, 9 lost / 12 run(s)');
    expect(text).toContain('MATRIX INCOMPLETE: 9 of 12 cell(s) produced no verdict');
  });

  it('counts failures apart from lost cells', () => {
    const text = matrixSummary([...cells(['fail'], 60_000), ...cells(['lost'], 30_000)], 60_000).join('\n');
    expect(text).toContain('1 failed, 1 lost / 2 run(s)');
  });

  it('still reports the speedup when every cell reached a verdict, failures included', () => {
    const text = matrixSummary(cells(['pass', 'fail', 'pass', 'pass'], 100_000), 100_000).join('\n');
    expect(text).toContain('serial-equivalent 400.0s; speedup 4.00x');
    expect(text).toContain('1 failed, 0 lost / 4 run(s)');
    expect(text).not.toContain('INCOMPLETE');
  });
});
