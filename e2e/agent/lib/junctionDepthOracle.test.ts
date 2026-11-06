/**
 * X6c on synthetic surveys: what the Ariane export does to depths and tapes
 * under the junction rule (#576 TML-DEPTH-D1, Q1-D1, Q2-D1, Q3-D1), and what
 * the old re-hang and clamp did, which must now fail.
 *
 * Each case states the original shots and the re-imported ones as the export
 * writes them: `keepRun` is the exporter's refit, `sqrt(h^2 + dz'^2)` rounded
 * up to the next written micrometre.
 */
import { describe, expect, it } from 'vitest';

import { ExportOracleError } from './exportOracles';
import {
  checkJunctionDepths,
  junctionDepthReport,
  type StationedShot,
  type StationMergeWire,
} from './junctionDepthOracle';

const REPRO = 'repro: synthetic';

function shot(
  uuid: string,
  [section, index]: [number, number],
  length: number,
  azimuthIn: number,
  [depthIn, depthOut]: [number, number],
): StationedShot {
  return { uuid, section, index, length, azimuthIn, depthIn, depthOut };
}

const run = (length: number, change: number): number => Math.sqrt(Math.max(length * length - change * change, 0));

/** The exporter's refit of a `length` m shot measured over `measured`, now over `change`. */
const keepRun = (length: number, measured: number, change: number): number =>
  Math.ceil(Math.hypot(run(length, measured), change) * 1e6) / 1e6;

function report(
  before: StationedShot[],
  after: StationedShot[],
  merges: StationMergeWire[] = [],
): ReturnType<typeof junctionDepthReport> {
  return junctionDepthReport(before, { shots: after, merges });
}

describe('X6c an averaged junction', () => {
  // The decided example: Main station 1 reads 10.0, Branch starts at 12.0.
  const before = [
    shot('m1', [0, 0], 4, 0, [9, 10]),
    shot('m2', [0, 1], 3, 90, [10, 9.5]),
    shot('m3', [0, 2], 2, 90, [9.5, 10]),
    shot('b1', [1, 0], 2, 180, [12, 12.5]),
    shot('b2', [1, 1], 3, 200, [12.5, 13]),
  ];
  const merges: StationMergeWire[] = [[10, 1, 11, 0]];

  it('passes the junction at the mean, its touching shots refitted, the rest true', () => {
    const after = [
      shot('m1', [10, 0], keepRun(4, 1, 2), 0, [9, 11]),
      shot('m2', [10, 1], keepRun(3, -0.5, -1.5), 90, [11, 9.5]),
      shot('m3', [10, 2], 2, 90, [9.5, 10]),
      shot('b1', [11, 0], keepRun(2, 0.5, 1.5), 180, [11, 12.5]),
      shot('b2', [11, 1], 3, 200, [12.5, 13]),
    ];
    const got = report(before, after, merges);
    expect(got.failures).toEqual([]);
    expect(got).toMatchObject({ matched: 5, unmatched: 0, averagedJunctions: 1, refittedShots: 3, brokenShots: 0 });
    expect(() => checkJunctionDepths(got, REPRO)).not.toThrow();
  });

  it('fails a junction left at the main line, the branch refitted onto it', () => {
    const after = [
      shot('m1', [10, 0], 4, 0, [9, 10]),
      shot('m2', [10, 1], 3, 90, [10, 9.5]),
      shot('m3', [10, 2], 2, 90, [9.5, 10]),
      shot('b1', [11, 0], keepRun(2, 0.5, 2.5), 180, [10, 12.5]),
      shot('b2', [11, 1], 3, 200, [12.5, 13]),
    ];
    const got = report(before, after, merges);
    expect(got.failures).toHaveLength(1);
    expect(got.failures[0]).toMatch(/mean with one reading per section/);
    expect(() => checkJunctionDepths(got, REPRO)).toThrow(ExportOracleError);
    expect(() => checkJunctionDepths(got, REPRO)).toThrow(/X6c/);
  });

  it('fails the old re-hang, which shifted the whole branch', () => {
    const after = [
      shot('m1', [10, 0], 4, 0, [9, 10]),
      shot('m2', [10, 1], 3, 90, [10, 9.5]),
      shot('m3', [10, 2], 2, 90, [9.5, 10]),
      shot('b1', [11, 0], 2, 180, [10, 10.5]),
      shot('b2', [11, 1], 3, 200, [10.5, 11]),
    ];
    const got = report(before, after, merges);
    // The junction, the station between b1 and b2, and b2's end.
    expect(got.failures).toHaveLength(3);
  });

  it('counts a section merged part-way along once, however many pieces it has there', () => {
    // Mid is merged at its station 1 onto Main station 1, so it comes back as
    // a forward piece (m22, m23) and a reversed one (m21 walked back). One
    // reading per section: (10 + 12) / 2 = 11, not (10 + 12 + 12) / 3.
    const mid = [
      shot('m1', [0, 0], 5, 0, [9, 10]),
      shot('m2', [0, 1], 5, 0, [10, 10]),
      shot('m21', [1, 0], 3, 90, [11, 12]),
      shot('m22', [1, 1], 3, 90, [12, 11.5]),
      shot('m23', [1, 2], 3, 90, [11.5, 11]),
    ];
    const pieces: StationMergeWire[] = [
      [10, 1, 11, 0],
      [10, 1, 12, 0],
    ];
    const at = (junction: number): StationedShot[] => [
      shot('m1', [10, 0], keepRun(5, 1, junction - 9), 0, [9, junction]),
      shot('m2', [10, 1], keepRun(5, 0, 10 - junction), 0, [junction, 10]),
      shot('m22', [11, 0], keepRun(3, -0.5, 11.5 - junction), 90, [junction, 11.5]),
      shot('m23', [11, 1], 3, 90, [11.5, 11]),
      // The reversed leg: heading through 180, depths swapped.
      shot('m21', [12, 0], keepRun(3, 1, 11 - junction), 270, [junction, 11]),
    ];
    const once = report(mid, at(11), pieces);
    expect(once.failures).toEqual([]);
    expect(once.averagedJunctions).toBe(1);
    const twice = report(mid, at(11.333333), pieces);
    expect(twice.failures).toHaveLength(1);
    expect(twice.failures[0]).toMatch(/mean with one reading per section/);
  });
});

describe('X6c a broken shot goes out vertical', () => {
  // c drops 3.0001 m on a 3 m tape: broken data (TML-DEPTH-Q2-D1).
  const before = [
    shot('a', [0, 0], 10, 0, [0, 10]),
    shot('b', [0, 1], 3, 0, [10, 13]),
    shot('c', [0, 2], 3, 45, [13, 16.0001]),
    shot('d', [0, 3], 2, 45, [16.0001, 17.0001]),
  ];

  it('passes both depths kept and the drop as its length', () => {
    const after = [
      shot('a', [5, 0], 10, 0, [0, 10]),
      shot('b', [5, 1], 3, 0, [10, 13]),
      shot('c', [5, 2], 3.0001, 45, [13, 16.0001]),
      shot('d', [5, 3], 2, 45, [16.0001, 17.0001]),
    ];
    const got = report(before, after);
    expect(got.failures).toEqual([]);
    expect(got).toMatchObject({ averagedJunctions: 0, refittedShots: 1, brokenShots: 1 });
  });

  it('fails the old clamp, which moved the station it ends at', () => {
    const after = [
      shot('a', [5, 0], 10, 0, [0, 10]),
      shot('b', [5, 1], 3, 0, [10, 13]),
      shot('c', [5, 2], 3, 45, [13, 16]),
      shot('d', [5, 3], keepRun(2, 1, 1.0001), 45, [16, 17.0001]),
    ];
    const got = report(before, after);
    expect(got.failures).toHaveLength(1);
    expect(got.failures[0]).toMatch(/came back at c=16, d=16; its readings are section 0: 16.0001/);
  });

  it('fails a broken shot written as measured: the length bound does not hold', () => {
    const got = report(before, before.map((s) => ({ ...s, section: 5 })));
    expect(got.failures).toEqual([expect.stringMatching(/shot c .*the length bound does not hold/)]);
  });
});

describe('X6c a second reading inside a section is refitted, not averaged', () => {
  // b starts at 6.3 where a ended at 6.0: two readings of station 1, one section.
  const before = [shot('a', [0, 0], 4, 0, [5, 6]), shot('b', [0, 1], 3, 90, [6.3, 7])];

  it('passes the shot refitted from its parent, its end true', () => {
    const after = [shot('a', [3, 0], 4, 0, [5, 6]), shot('b', [3, 1], keepRun(3, 0.7, 1), 90, [6, 7])];
    const got = report(before, after);
    expect(got.failures).toEqual([]);
    expect(got).toMatchObject({ averagedJunctions: 0, refittedShots: 1 });
  });

  it('passes the second reading kept, where its shot holds the bound', () => {
    const got = report(before, before.map((s) => ({ ...s, section: 3 })));
    expect(got.failures).toEqual([]);
    expect(got.refittedShots).toBe(0);
  });

  it('fails a refit that moved its end instead of its tape', () => {
    const after = [shot('a', [3, 0], 4, 0, [5, 6]), shot('b', [3, 1], 3, 90, [6, 6.7])];
    const got = report(before, after);
    expect(got.failures).toHaveLength(1);
    expect(got.failures[0]).toMatch(/b=6.7; its readings are section 0: 7/);
  });
});

describe('X6c the plan', () => {
  const before = [shot('a', [0, 0], 5, 30, [1, 1])];

  it('fails a tape that changed on a shot nothing touched', () => {
    const got = report(before, [shot('a', [1, 0], 5.1, 30, [1, 1])]);
    expect(got.failures).toEqual([expect.stringMatching(/horizontal run 5 m came back as 5.1 m .*the plan moved/)]);
  });

  it('fails a heading that changed, and reads a reversed leg as the same shot', () => {
    expect(report(before, [shot('a', [1, 0], 5, 40, [1, 1])]).failures).toEqual([
      expect.stringMatching(/heading 30 came back as 40/),
    ]);
    expect(report(before, [shot('a', [1, 0], 5, 210, [1, 1])]).failures).toEqual([]);
  });

  it('leaves a shot it cannot match by uuid to X6, and counts it', () => {
    const got = report([shot('', [0, 0], 5, 30, [1, 1])], [shot('', [1, 0], 9, 0, [4, 4])]);
    expect(got).toMatchObject({ matched: 0, unmatched: 1, failures: [] });
  });
});
