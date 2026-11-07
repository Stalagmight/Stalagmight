/**
 * #576 RV19b-P2: a failure line is kept whole only in a shape the harness
 * writes, and each shape is strict enough that the app's own text cannot take
 * it. A `trace:` line is one trace file, a `reproduce:` line a re-run command,
 * an `at` line a diff path of field names and indices.
 *
 * Everything here is synthetic: token names (`Qz...`) stand in for a cave, a
 * section and a station.
 */
import { describe, expect, it } from 'vitest';

import { shareableFailureText, valueShape } from './shareableFailure';

describe('kept line shapes take only the lines the harness writes (#576 RV19b-P2)', () => {
  it("withholds the app's text shaped like a trace, reproduce or diff-path line", () => {
    const shared = shareableFailureText(
      [
        'FAIL: label - failed: boom',
        '  trace: Qzcafé Nord passage',
        '  reproduce: station Qzbig Room',
        '  at Qzentrance.Sump_2',
        '  trace: Qzpassage',
        '  reproduce: pnpm Qzroom',
        '  at qzentrance',
        '  at qzentrance sump',
      ].join('\n'),
    );
    expect(shared).not.toMatch(/qz/i);
    expect(shared.split('\n').slice(1)).toEqual([
      "  (7 line(s) withheld: they can quote the app's text - the full failure is in the session's actions.jsonl)",
    ]);
  });

  it('keeps the trace, reproduce and diff-path lines the harness writes', () => {
    const lines = [
      '  reproduce: pnpm agent:scenario undo-persistence-fuzz --seed 3 --iterations 5 --release',
      '  reproduce: pnpm agent:scenario import-dialog-fuzz --seed 19 --iterations 8 with STALAGMIGHT_FUZZ_FIXTURE=<corpus #ab12c>',
      '  reproduce: run_scenario("undo-redo-fuzz", seed=1, iterations=2)',
      'reproduce: pnpm agent:scenario export-roundtrip-fuzz --seed 1 --iterations 1',
      '  reproduce: STALAGMIGHT_FUZZ_FIXTURE=~/fixtures/a.stalx pnpm agent:scenario s --seed 3',
      '  trace: <repo>/.agent-artifacts/sessions/s1/traces/undo-redo-fuzz-seed1-20261107.json',
      '  trace: /traces/t.json',
      '  at project.sections[3].shots[1].uuid',
      '  at stations[2].coordinates.x',
      '  at project.section_offsets.12',
      '  at project.sections.length',
    ];
    expect(shareableFailureText(['headline', ...lines].join('\n')).split('\n')).toEqual(['headline', ...lines]);
  });
});

describe('a value read back from a file prints as its shape (#576 W51)', () => {
  it('keeps numbers, booleans, null and undefined, and gives text only as a size', () => {
    expect([1.5, -5, 0, true, null, undefined].map(valueShape)).toEqual(['1.5', '-5', '0', 'true', 'null', 'undefined']);
    expect(valueShape('')).toBe('an empty string');
    expect(valueShape('qzCaveA')).toBe('a string of 7 character(s)');
    expect(valueShape('qzCaveA\n  at project.sections[1]')).toBe('a string of 32 character(s)');
    expect(valueShape(['qzCaveA', 'StationA1'])).toBe('an array of 2 item(s)');
    expect(valueShape({ name: 'qzCaveA' })).toBe('an object');
    for (const forged of ['qzCaveA', ['qzCaveA'], { qzCaveA: 1 }]) expect(valueShape(forged)).not.toContain('qzCaveA');
  });
});
