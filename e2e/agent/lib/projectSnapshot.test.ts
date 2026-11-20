import { describe, expect, it } from 'vitest';

import {
  type KnownAsymmetry,
  LOOP_CLOSURES_PATH,
  RELOAD_RESOLVED_GEOMETRY,
  SAVE_HEALING_MUTATIONS,
  canonicalJson,
  classifyDiffs,
  describeDiffs,
  diffJson,
  diffSnapshots,
  isListPermutationOnly,
  shotSetDelta,
} from './projectSnapshot';

// The canonical serializer and its normalization rules are tested in
// fuzzOracleCore.test.ts, which owns them. This file covers the Node-only
// half: the tolerant diff, the epoch attribution, and the permutation detector.
//
// The known-open-bug classifiers this file used to cover
// (`KNOWN_UNDO_ASYMMETRIES`, `LOOP_CLOSURE_REORDER`) are deleted along with the
// issue-413 findings they named, so their tests are gone with them. What is
// tested here is what remains: attribution for behaviour a save or a reload is
// DESIGNED to have, which must stay narrow by path AND by direction, and must
// never match an ordinary field.

describe('diffJson', () => {
  it('names the first differing path with both values', () => {
    const diffs = diffJson({ a: { b: [1, 2, 3] } }, { a: { b: [1, 9, 3] } });

    expect(diffs).toHaveLength(1);
    expect(diffs[0]?.path).toBe('a.b[1]');
    expect(diffs[0]?.expected).toBe('2');
    expect(diffs[0]?.actual).toBe('9');
  });

  it('reports array length changes and missing keys', () => {
    expect(diffJson({ a: [1, 2] }, { a: [1] })[0]).toEqual({
      path: 'a.length',
      expected: '2',
      actual: '1',
    });
    expect(diffJson({ a: 1 }, {})[0]).toEqual({ path: 'a', expected: '1', actual: '<absent>' });
    expect(diffJson({}, { a: 1 })[0]).toEqual({ path: 'a', expected: '<absent>', actual: '1' });
  });

  it('honours the diff limit, and asking for one more REVEALS the overflow', () => {
    const expected = { a: 1, b: 2, c: 3, d: 4 };
    const actual = { a: 9, b: 9, c: 9, d: 9 };

    expect(diffJson(expected, actual, 2)).toHaveLength(2);
    // How the scenarios detect truncation: they request LIMIT + 1 and treat a
    // list longer than LIMIT as "this is a prefix, tolerate nothing". Both
    // directions matter - at exactly the limit the list is complete, and an
    // off-by-one either way silently disarms that guard.
    expect(diffJson(expected, actual, 4)).toHaveLength(4);
    expect(diffJson(expected, actual, 5)).toHaveLength(4);
    expect(diffJson(expected, actual, 3 + 1)).toHaveLength(4);
    expect(diffJson(expected, actual, 2 + 1).length > 2).toBe(true);
  });

  it('returns nothing for equal structures', () => {
    expect(diffJson({ a: [1, { b: null }] }, { a: [1, { b: null }] })).toEqual([]);
  });

  it('absorbs re-derived-geometry dust around a true zero', () => {
    // The exact pair the fuzzer hit: undo of a moveStation gave back a
    // coordinate on the other side of zero, 3.6 femtometres away.
    const expected = { stations: [{ coordinates: { x: -3.5527136788e-15 } }] };
    const actual = { stations: [{ coordinates: { x: -7.1054273576e-15 } }] };

    expect(diffJson(expected, actual)).toEqual([]);
    expect(diffJson(expected, actual, 6, null)).toHaveLength(1);
  });

  it('still reports a difference a real geometry regression would produce', () => {
    // A nanometre is the absolute floor; a micrometre is already a failure, and
    // anything a bug could plausibly cause is millimetres or more.
    expect(diffJson({ x: 12.5 }, { x: 12.5 + 1e-6 })).toHaveLength(1);
    expect(diffJson({ x: 12.5 }, { x: 12.501 })).toHaveLength(1);
    expect(diffJson({ x: 0 }, { x: 1e-9 })).toEqual([]);
    expect(diffJson({ x: 0 }, { x: 1e-7 })).toHaveLength(1);
  });

  it('keeps the relative bound tight on large coordinates', () => {
    expect(diffJson({ x: 1e6 }, { x: 1e6 + 1e-9 })).toEqual([]);
    expect(diffJson({ x: 1e6 }, { x: 1e6 + 0.001 })).toHaveLength(1);
  });

  it('does not treat non-finite or non-numeric values as close', () => {
    expect(diffJson({ x: 1 }, { x: '1' })).toHaveLength(1);
    expect(diffJson({ x: 1 }, { x: null })).toHaveLength(1);
  });

  it('diffs canonical snapshot strings', () => {
    const diffs = diffSnapshots('{"a":1}', '{"a":2}');

    expect(diffs[0]?.path).toBe('a');
    expect(describeDiffs(diffs)).toContain('at a');
  });
});

describe('classifyDiffs', () => {
  it('leaves every diff unknown when no rule is passed', () => {
    // The `known` argument is required precisely so this is the default shape:
    // an empty rule list classifies nothing, which is what a strict scenario
    // wants when a comparison does not cross a save/reload epoch.
    const classified = classifyDiffs(
      [
        { path: 'project.lines[2].shots[0].uuid', expected: '"u-1"', actual: '"u-2"' },
        { path: 'stations[3].coordinates.x', expected: '1', actual: '2' },
      ],
      [],
    );

    expect(classified.known).toHaveLength(0);
    expect(classified.unknown).toHaveLength(2);
    expect(classified.asymmetries).toHaveLength(0);
  });

  /** A rule shaped like the families the scenarios pass (a fill-in only). */
  const FILL_IN: KnownAsymmetry = {
    id: 'test-fills-in-a-label',
    pathPattern: /^project\.lines\[\d+\]\.label$/,
    accepts: (diff) => diff.expected === 'null' && diff.actual !== 'null',
    description: 'A test-only rule: a line label may go from null to a value, nothing else.',
  };

  it('does not swallow a neighbouring field on an attributable path', () => {
    const classified = classifyDiffs(
      [{ path: 'project.lines[2].shots[0].length', expected: '5', actual: '6' }],
      [FILL_IN],
    );

    expect(classified.known).toHaveLength(0);
    expect(classified.unknown).toHaveLength(1);
  });

  it('reports each matching rule once, whatever the diff count', () => {
    const classified = classifyDiffs(
      [
        { path: 'project.lines[2].label', expected: 'null', actual: '"a"' },
        { path: 'project.lines[5].label', expected: 'null', actual: '"b"' },
      ],
      [FILL_IN],
    );

    expect(classified.known).toHaveLength(2);
    expect(classified.asymmetries.map((a) => a.id)).toEqual(['test-fills-in-a-label']);
  });

  it('honours a rule\'s direction', () => {
    const classified = classifyDiffs(
      [{ path: 'project.lines[2].label', expected: '"a"', actual: '"b"' }],
      [FILL_IN],
    );

    expect(classified.known).toEqual([]);
    expect(classified.unknown).toHaveLength(1);
  });
});

// DIAGNOSTIC ONLY since the finding-4 classifier was deleted: it never made a
// comparison pass, and now it only shapes the failure message. Still tested,
// because a message that mis-explains a failure is worse than one that does not
// try.
describe('isListPermutationOnly', () => {
  const withClosures = (closures: unknown[], extra: Record<string, unknown> = {}): string =>
    canonicalJson({ project: { name: 'p', loop_closures: closures, ...extra }, stations: [] });

  const a = { id: 1, path_length: 10 };
  const b = { id: 2, path_length: 20 };
  const c = { id: 3, path_length: 30 };

  it('recognises the closure that was pushed back on instead of reinserted', () => {
    // Remove id 2 from [1,2,3] then undo: it lands at the end.
    const before = withClosures([a, b, c]);
    const afterUndo = withClosures([a, c, b]);

    expect(isListPermutationOnly(before, afterUndo, LOOP_CLOSURES_PATH)).toBe(true);
  });

  it('is false when a closure actually changed', () => {
    const before = withClosures([a, b, c]);
    const mutated = withClosures([a, c, { id: 2, path_length: 999 }]);

    expect(isListPermutationOnly(before, mutated, LOOP_CLOSURES_PATH)).toBe(false);
  });

  it('is false when a closure was lost or gained', () => {
    expect(isListPermutationOnly(withClosures([a, b, c]), withClosures([a, c]), LOOP_CLOSURES_PATH)).toBe(false);
    expect(isListPermutationOnly(withClosures([a, b]), withClosures([a, b, c]), LOOP_CLOSURES_PATH)).toBe(false);
  });

  it('is false when something OUTSIDE the list also differs', () => {
    const before = withClosures([a, b, c], { name_extra: 'x' });
    const after = withClosures([a, c, b], { name_extra: 'y' });

    expect(isListPermutationOnly(before, after, LOOP_CLOSURES_PATH)).toBe(false);
  });

  it('is false when the order is already identical', () => {
    const same = withClosures([a, b, c]);

    expect(isListPermutationOnly(same, same, LOOP_CLOSURES_PATH)).toBe(false);
  });

  it('is false when the path does not hold a list', () => {
    expect(isListPermutationOnly('{"project":{}}', '{"project":{}}', LOOP_CLOSURES_PATH)).toBe(false);
    expect(isListPermutationOnly('not json', 'not json', LOOP_CLOSURES_PATH)).toBe(false);
  });

  it('does not mistake a permutation for equality elsewhere in the snapshot', () => {
    // Reordered AND a station moved: not explainable by the reorder alone.
    const before = canonicalJson({ project: { loop_closures: [a, b] }, stations: [{ x: 1 }] });
    const after = canonicalJson({ project: { loop_closures: [b, a] }, stations: [{ x: 2 }] });

    expect(isListPermutationOnly(before, after, LOOP_CLOSURES_PATH)).toBe(false);
  });
});

describe('the save and reload families on the station-graph model', () => {
  it('are empty: a v2 save never heals and a v2 load is pure', () => {
    expect(SAVE_HEALING_MUTATIONS).toEqual([]);
    expect(RELOAD_RESOLVED_GEOMETRY).toEqual([]);
  });

  it('attribute nothing a save or a reload could change', () => {
    // The SQLite heals and the line-offset re-solve these used to attribute:
    // on v2 every one of them is a hard failure.
    const classified = classifyDiffs(
      [
        { path: 'project.lines[2].gauge_shot_uuid', expected: 'null', actual: '"u-1"' },
        { path: 'project.lines[0].shots[1].uuid', expected: '<absent>', actual: '"u-3"' },
        { path: 'stations[4].coordinates.x', expected: '1.5', actual: '1.75' },
      ],
      [...SAVE_HEALING_MUTATIONS, ...RELOAD_RESOLVED_GEOMETRY],
    );

    expect(classified.known).toEqual([]);
    expect(classified.unknown).toHaveLength(3);
  });
});

describe('shotSetDelta', () => {
  const snapshot = (lines: { id: number; uuids: string[] }[]): string =>
    canonicalJson({
      project: {
        lines: lines.map((line) => ({
          id: line.id,
          shots: line.uuids.map((uuid) => ({ uuid })),
        })),
      },
      stations: [],
    });

  it('is empty when the same shots are present on both sides', () => {
    const same = snapshot([{ id: 1, uuids: ['a', 'b'] }]);

    expect(shotSetDelta(same, same)).toEqual([]);
    // Reordered within the line: the same shots moved, none was lost.
    expect(shotSetDelta(same, snapshot([{ id: 1, uuids: ['b', 'a'] }]))).toEqual([]);
  });

  it('names the shot that went missing and the one that took its place', () => {
    const deltas = shotSetDelta(
      snapshot([{ id: 1, uuids: ['a', 'b'] }]),
      snapshot([{ id: 1, uuids: ['a', 'c'] }]),
    );

    expect(deltas).toEqual([
      { lineId: 1, missing: ['b'], extra: ['c'], expectedCount: 2, actualCount: 2 },
    ]);
  });

  it('counts a line that VANISHED as its whole shot list lost', () => {
    // The regression this guards: iterating only `expected` and skipping ids
    // absent from `actual` reported NO delta for a run that lost a line, and
    // the caller then printed "shot identity is IDENTICAL on both sides".
    const deltas = shotSetDelta(
      snapshot([{ id: 1, uuids: ['a'] }, { id: 7, uuids: ['x', 'y'] }]),
      snapshot([{ id: 1, uuids: ['a'] }]),
    );

    expect(deltas).toEqual([
      { lineId: 7, missing: ['x', 'y'], extra: [], expectedCount: 2, actualCount: 0 },
    ]);
  });

  it('counts a line that APPEARED as its whole shot list gained', () => {
    const deltas = shotSetDelta(
      snapshot([{ id: 1, uuids: ['a'] }]),
      snapshot([{ id: 1, uuids: ['a'] }, { id: 7, uuids: ['x'] }]),
    );

    expect(deltas).toEqual([
      { lineId: 7, missing: [], extra: ['x'], expectedCount: 0, actualCount: 1 },
    ]);
  });
});

// The bug-8 shot-swap tolerance (`isBug8ShotSwapOnly` and its `isShotIdentitySwap`
// uuid predicate) was tested here, at length, because it was the one place a
// mismatch could be walked past. It was deleted with the bug it tolerated - issue
// #413 finding 8, an undo after a save/reload deleting the WRONG shot - so there
// is nothing left to cover: a swapped shot identity is now an ordinary,
// unattributable failure, and `shotSetDelta` above is all that survives of it (it
// names which shot went missing, which is reporting, not tolerating).
