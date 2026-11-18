import { describe, expect, it } from 'vitest';

import { FAIL_BLOCK_LINES, TAIL_LINES, failureDetail } from './fuzzMatrixReport';

/** A scenario failure as `scenario-run.ts` prints it: FAIL line, then the message. */
const ORACLE_FAILURE = [
  'session ready (debug build fresh)',
  '   . scenario import-dialog-fuzz start (seed=8, iterations=8)',
  'FAIL: import-dialog-fuzz threw:',
  'F3 ACCOUNTING: the receipt does not describe what landed in the project',
  '  receipt:  1 line(s), 7 shot(s)',
  '  project:  0 line(s), 0 shot(s)',
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
    expect(detail).toContain('receipt:  1 line(s), 7 shot(s)');
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
    const outLines = ['session ready', ''];
    const allLines = [...outLines, ...Array.from({ length: 40 }, (_unused, i) => `stderr ${i}`)];
    const detail = failureDetail({ outLines, allLines });
    const lines = detail.split('\n');
    expect(lines).toHaveLength(TAIL_LINES);
    expect(lines[lines.length - 1]).toBe('stderr 39');
    expect(detail).not.toContain('\n\n');
  });

  it('anchors on the FIRST FAIL line when a run printed more than one', () => {
    const outLines = ['FAIL: a threw:', 'first', 'FAIL: b threw:', 'second'];
    const detail = failureDetail({ outLines, allLines: outLines });
    expect(detail.split('\n')[0]).toBe('FAIL: a threw:');
    expect(detail).toContain('first');
  });
});
