import { describe, expect, it } from 'vitest';

import {
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
        { path: 'project.sections[2].shots[0].uuid', expected: '"u-1"', actual: '"u-2"' },
        { path: 'project.section_offsets.32', expected: '<absent>', actual: '[0,0]' },
      ],
      [],
    );

    expect(classified.known).toHaveLength(0);
    expect(classified.unknown).toHaveLength(2);
    expect(classified.asymmetries).toHaveLength(0);
  });

  it('does not swallow a neighbouring field on an attributable path', () => {
    const classified = classifyDiffs(
      [{ path: 'project.sections[2].shots[0].length', expected: '5', actual: '6' }],
      SAVE_HEALING_MUTATIONS,
    );

    expect(classified.known).toHaveLength(0);
    expect(classified.unknown).toHaveLength(1);
  });

  it('reports each matching rule once, whatever the diff count', () => {
    const classified = classifyDiffs(
      [
        { path: 'project.sections[2].gauge_shot_uuid', expected: 'null', actual: '"u-1"' },
        { path: 'project.sections[5].gauge_shot_uuid', expected: 'null', actual: '"u-2"' },
      ],
      SAVE_HEALING_MUTATIONS,
    );

    expect(classified.known).toHaveLength(2);
    expect(classified.asymmetries.map((a) => a.id)).toEqual(['save-captures-missing-gauge-anchors']);
  });

  it('exposes every attribution rule with an id and an explanation', () => {
    for (const asymmetry of [...SAVE_HEALING_MUTATIONS, ...RELOAD_RESOLVED_GEOMETRY]) {
      expect(asymmetry.id).toMatch(/^[a-z0-9-]+$/);
      expect(asymmetry.description.length).toBeGreaterThan(40);
    }
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

describe('SAVE_HEALING_MUTATIONS', () => {
  it('accepts the three documented save-time heals, in the documented direction', () => {
    const classified = classifyDiffs(
      [
        { path: 'project.sections[2].gauge_shot_uuid', expected: 'null', actual: '"u-1"' },
        { path: 'project.sections[2].gauge_neighbour_uuid', expected: 'null', actual: '"u-2"' },
        { path: 'project.sections[0].shots[1].uuid', expected: '<absent>', actual: '"u-3"' },
        { path: 'project.section_offsets.9', expected: '[0,0]', actual: '<absent>' },
      ],
      SAVE_HEALING_MUTATIONS,
    );

    expect(classified.unknown).toEqual([]);
    expect(classified.asymmetries).toHaveLength(3);
  });

  it('rejects the same paths moving the WRONG way', () => {
    const classified = classifyDiffs(
      [
        // A gauge anchor changing value (not null -> value) is not a heal.
        { path: 'project.sections[2].gauge_shot_uuid', expected: '"old"', actual: '"new"' },
        // An offset entry APPEARING is not the orphan retain.
        { path: 'project.section_offsets.9', expected: '<absent>', actual: '[1,2]' },
        // A uuid being cleared is not the backfill.
        { path: 'project.sections[0].shots[1].uuid', expected: '"u"', actual: '<absent>' },
      ],
      SAVE_HEALING_MUTATIONS,
    );

    expect(classified.known).toEqual([]);
    expect(classified.unknown).toHaveLength(3);
  });

  it('rejects anything else a save might have touched', () => {
    const classified = classifyDiffs(
      [{ path: 'project.sections[0].shots[1].length', expected: '5', actual: '6' }],
      SAVE_HEALING_MUTATIONS,
    );

    expect(classified.unknown).toHaveLength(1);
  });

  // #576 NF-FZDET-1: the save stamps Section::created_at on a section that has
  // none yet (NF-W14BE-1), and the payload carries it from then on.
  it('accepts the creation time a save stamps on a section, absent -> an RFC 3339 stamp', () => {
    const classified = classifyDiffs(
      [
        { path: 'project.sections[3].created_at', expected: '<absent>', actual: '"2026-11-05T21:57:28.123456789+00:00"' },
        { path: 'project.sections[4].created_at', expected: '<absent>', actual: '"2026-11-05T21:57:28+00:00"' },
      ],
      SAVE_HEALING_MUTATIONS,
    );

    expect(classified.unknown).toEqual([]);
    expect(classified.asymmetries.map((a) => a.id)).toEqual(['save-stamps-section-creation-times']);
  });

  it('rejects a section creation time that changes, goes or is not a stamp', () => {
    const classified = classifyDiffs(
      [
        // A stamp the section had is written back byte for byte (#576 RV14-2):
        // another one - even the same moment in another form - is not the heal.
        { path: 'project.sections[0].created_at', expected: '"2026-01-01T10:00:00+05:00"', actual: '"2026-01-01T05:00:00+00:00"' },
        { path: 'project.sections[1].created_at', expected: '"2026-01-01T10:00:00+00:00"', actual: '"2026-11-05T21:57:28+00:00"' },
        { path: 'project.sections[2].created_at', expected: '"2026-01-01T10:00:00+00:00"', actual: '<absent>' },
        { path: 'project.sections[5].created_at', expected: '<absent>', actual: 'null' },
        { path: 'project.sections[6].created_at', expected: '<absent>', actual: '"2026-11-05"' },
        { path: 'project.sections[7].created_at', expected: '<absent>', actual: '"2026-13-45T25:61:00+00:00"' },
        // The project's own creation time is not stamped by a save.
        { path: 'project.created_at', expected: '"2026-01-01T10:00:00Z"', actual: '"2026-11-05T21:57:28Z"' },
      ],
      SAVE_HEALING_MUTATIONS,
    );

    expect(classified.known).toEqual([]);
    expect(classified.unknown).toHaveLength(7);
  });
});

describe('shotSetDelta', () => {
  const snapshot = (sections: { id: number; uuids: string[] }[]): string =>
    canonicalJson({
      project: {
        sections: sections.map((section) => ({
          id: section.id,
          shots: section.uuids.map((uuid) => ({ uuid })),
        })),
      },
      stations: [],
    });

  it('is empty when the same shots are present on both sides', () => {
    const same = snapshot([{ id: 1, uuids: ['a', 'b'] }]);

    expect(shotSetDelta(same, same)).toEqual([]);
    // Reordered within the section: the same shots moved, none was lost.
    expect(shotSetDelta(same, snapshot([{ id: 1, uuids: ['b', 'a'] }]))).toEqual([]);
  });

  it('names the shot that went missing and the one that took its place', () => {
    const deltas = shotSetDelta(
      snapshot([{ id: 1, uuids: ['a', 'b'] }]),
      snapshot([{ id: 1, uuids: ['a', 'c'] }]),
    );

    expect(deltas).toEqual([
      { sectionId: 1, missing: ['b'], extra: ['c'], expectedCount: 2, actualCount: 2 },
    ]);
  });

  it('counts a section that VANISHED as its whole shot list lost', () => {
    // The regression this guards: iterating only `expected` and skipping ids
    // absent from `actual` reported NO delta for a run that lost a section, and
    // the caller then printed "shot identity is IDENTICAL on both sides".
    const deltas = shotSetDelta(
      snapshot([{ id: 1, uuids: ['a'] }, { id: 7, uuids: ['x', 'y'] }]),
      snapshot([{ id: 1, uuids: ['a'] }]),
    );

    expect(deltas).toEqual([
      { sectionId: 7, missing: ['x', 'y'], extra: [], expectedCount: 2, actualCount: 0 },
    ]);
  });

  it('counts a section that APPEARED as its whole shot list gained', () => {
    const deltas = shotSetDelta(
      snapshot([{ id: 1, uuids: ['a'] }]),
      snapshot([{ id: 1, uuids: ['a'] }, { id: 7, uuids: ['x'] }]),
    );

    expect(deltas).toEqual([
      { sectionId: 7, missing: [], extra: ['x'], expectedCount: 0, actualCount: 1 },
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
