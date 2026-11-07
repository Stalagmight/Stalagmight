import { describe, expect, it } from 'vitest';

import { EQUAL, checkSnapshot, type SnapshotCheckLimits } from './fuzzSnapshotCheck';
import { RELOAD_RESOLVED_GEOMETRY, SAVE_HEALING_MUTATIONS } from './projectSnapshot';

// The slow half of the two undo fuzzers' roundtrip oracle. Every fixture here is
// a hand-built canonical snapshot (sorted keys, the shape `get_project_state`
// canonicalizes to), so each verdict shape is pinned without the app.

const LIMITS: SnapshotCheckLimits = { diffLimit: 200, shownDiffs: 6 };

interface Shot {
  uuid: string;
  length: number;
}

function section(id: number, shots: Shot[], gauge: string | null = null): Record<string, unknown> {
  return { gauge_neighbour_uuid: gauge, gauge_shot_uuid: gauge, id, shots };
}

function snap(project: Record<string, unknown>): string {
  return JSON.stringify({ project, stations: [] });
}

const SHOTS: Shot[] = [
  { uuid: 'shot-a', length: 1 },
  { uuid: 'shot-b', length: 2 },
];

describe('checkSnapshot', () => {
  it('returns EQUAL itself for byte-identical strings', () => {
    const text = snap({ sections: [section(0, SHOTS)] });

    expect(checkSnapshot(text, text, [], LIMITS)).toBe(EQUAL);
  });

  it('calls a difference inside the numeric tolerance float noise, not a mismatch', () => {
    const expected = snap({ sections: [section(0, [{ uuid: 'shot-a', length: 1 }])] });
    const actual = snap({ sections: [section(0, [{ uuid: 'shot-a', length: 1 + 1e-13 }])] });

    expect(checkSnapshot(expected, actual, [], LIMITS)).toEqual({
      equal: true,
      floatNoise: true,
      attributed: [],
      text: '',
    });
  });

  it('renders an unexplained difference with no attribution', () => {
    const expected = snap({ name: 'CaveA', sections: [] });
    const actual = snap({ name: 'CaveB', sections: [] });

    const check = checkSnapshot(expected, actual, [], LIMITS);

    expect(check.equal).toBe(false);
    expect(check.attributed).toEqual([]);
    expect(check.text).toBe('  at project.name\n    expected: "CaveA"\n    actual:   "CaveB"');
  });

  it('stays strict when nothing is attributable, even on an attributable path', () => {
    // Within one epoch the caller passes []: a gauge anchor appearing is then a
    // plain difference, never a save heal.
    const expected = snap({ sections: [section(0, SHOTS, null)] });
    const actual = snap({ sections: [section(0, SHOTS, 'gauge-1')] });

    const check = checkSnapshot(expected, actual, [], LIMITS);

    expect(check.attributed).toEqual([]);
    expect(check.text.startsWith('  ATTRIBUTED')).toBe(false);
    expect(check.text).toContain('at project.sections[0].gauge_shot_uuid');
  });

  it('returns the ATTRIBUTED shape when every difference is a documented save heal', () => {
    const expected = snap({ sections: [section(0, SHOTS, null)] });
    const actual = snap({ sections: [section(0, SHOTS, 'gauge-1')] });

    const check = checkSnapshot(expected, actual, SAVE_HEALING_MUTATIONS, LIMITS);

    expect(check.equal).toBe(false);
    expect(check.attributed.map((entry) => entry.id)).toEqual(['save-captures-missing-gauge-anchors']);
    expect(check.text.startsWith('  ATTRIBUTED [save-captures-missing-gauge-anchors]: ')).toBe(true);
    // Nothing numeric was absorbed, so no magnitude line.
    expect(check.text).not.toContain('largest absorbed numeric change');
  });

  it('attributes the creation time a save stamps on a section, and not one that changes (#576 NF-FZDET-1)', () => {
    const before = snap({ sections: [section(0, SHOTS)] });
    const stamped = snap({ sections: [{ created_at: '2026-11-05T21:57:28.5+00:00', ...section(0, SHOTS) }] });
    const restamped = snap({ sections: [{ created_at: '2026-11-05T21:58:00+00:00', ...section(0, SHOTS) }] });

    const first = checkSnapshot(before, stamped, SAVE_HEALING_MUTATIONS, LIMITS);
    const again = checkSnapshot(stamped, restamped, SAVE_HEALING_MUTATIONS, LIMITS);

    expect(first.attributed.map((entry) => entry.id)).toEqual(['save-stamps-section-creation-times']);
    expect(first.text.startsWith('  ATTRIBUTED [save-stamps-section-creation-times]: ')).toBe(true);
    expect(again.attributed).toEqual([]);
    expect(again.text).toContain('at project.sections[0].created_at');
  });

  it('puts the absorbed MAGNITUDE of a reload re-solve on the record', () => {
    const expected = snap({ section_offsets: { 3: [0, 0] }, sections: [] });
    const actual = snap({ section_offsets: { 3: [1.5, 0] }, sections: [] });

    const check = checkSnapshot(expected, actual, RELOAD_RESOLVED_GEOMETRY, LIMITS);

    expect(check.text.split('\n')).toEqual([
      `  ATTRIBUTED [reload-resolves-section-offsets]: ${RELOAD_RESOLVED_GEOMETRY[0]?.description}`,
      '  largest absorbed numeric change: 1.500e+0',
    ]);
  });

  it('still attributes a list of EXACTLY diffLimit differences (complete, not capped)', () => {
    const expected = snap({ sections: [section(0, SHOTS, null)] });
    const actual = snap({ sections: [section(0, SHOTS, 'gauge-1')] });

    // Two differing paths (both gauge anchors) and a limit of two.
    const check = checkSnapshot(expected, actual, SAVE_HEALING_MUTATIONS, { diffLimit: 2, shownDiffs: 6 });

    expect(check.text.startsWith('  ATTRIBUTED')).toBe(true);
  });

  it('never returns the ATTRIBUTED (resync) shape for a CAPPED diff', () => {
    // Four attributable paths, cap of three: everything the walk saw was a save
    // heal, but the fourth was never looked at.
    const expected = snap({ sections: [section(0, SHOTS, null), section(1, SHOTS, null)] });
    const actual = snap({ sections: [section(0, SHOTS, 'gauge-1'), section(1, SHOTS, 'gauge-2')] });

    const check = checkSnapshot(expected, actual, SAVE_HEALING_MUTATIONS, { diffLimit: 3, shownDiffs: 6 });

    expect(check.equal).toBe(false);
    expect(check.attributed.map((entry) => entry.id)).toEqual(['save-captures-missing-gauge-anchors']);
    expect(check.text.startsWith('  ATTRIBUTED')).toBe(false);
    expect(check.text).toBe(
      '  every path in the first 3 was a documented save/reload change, but the\n' +
        '  diff was CAPPED there - so this is a statement about a PREFIX, not about the mismatch.',
    );
  });

  it('says a capped failure is a PREFIX that no attribution may speak for', () => {
    const many = (value: number): Record<string, number> =>
      Object.fromEntries(Array.from({ length: 5 }, (_, index) => [`k${index}`, value]));
    const expected = snap({ meta: many(1), sections: [] });
    const actual = snap({ meta: many(2), sections: [] });

    const check = checkSnapshot(expected, actual, [], { diffLimit: 4, shownDiffs: 2 });
    const lines = check.text.split('\n');

    expect(lines).toContain('  ...and 2 more differing path(s)');
    expect(lines).toContain('  THE DIFF WAS CAPPED at 4 path(s): everything above is a PREFIX and nothing');
    expect(lines).toContain(
      '  past it was examined, so no attribution can speak for this mismatch and none is allowed to.',
    );
  });

  it('counts the differences it does not print', () => {
    const expected = snap({ a: 1, b: 1, c: 1, sections: [] });
    const actual = snap({ a: 2, b: 2, c: 2, sections: [] });

    const check = checkSnapshot(expected, actual, [], { diffLimit: 200, shownDiffs: 1 });

    expect(check.text).toBe(
      '  at project.a\n    expected: 1\n    actual:   2\n  ...and 2 more differing path(s)',
    );
  });

  it('says the shot identity is IDENTICAL when shots differ only field by field', () => {
    const expected = snap({ sections: [section(0, SHOTS)] });
    const actual = snap({
      sections: [
        section(0, [
          { uuid: 'shot-a', length: 9 },
          { uuid: 'shot-b', length: 2 },
        ]),
      ],
    });

    const check = checkSnapshot(expected, actual, [], LIMITS);

    expect(check.text).toContain('at project.sections[0].shots[0].length');
    expect(check.text).toContain(
      '  (shot identity by uuid is IDENTICAL on both sides - the shots moved, none was lost)',
    );
    expect(check.text).not.toContain('SHOT IDENTITY DELTA');
  });

  it('names the shot that went MISSING when the uuid set changed', () => {
    const expected = snap({ sections: [section(0, SHOTS)] });
    const actual = snap({ sections: [section(0, [{ uuid: 'shot-a', length: 1 }])] });

    const check = checkSnapshot(expected, actual, [], LIMITS);

    expect(check.text).toContain('  SHOT IDENTITY DELTA (by uuid, which survives save/reload):');
    expect(check.text).toContain('  section 0: 2 shot(s) expected, 1 present');
    expect(check.text).toContain('    shots MISSING (present before, gone now): ["shot-b"]');
    expect(check.text).not.toContain('IDENTICAL');
  });

  it('gates the shot-identity report on a reported shots path', () => {
    // A shot really was lost, but the cap stops the walk at the loop-closure
    // diff (`loop_closures` sorts before `sections`), so no reported path is a
    // shots path - and the report must not speak about shots it did not show.
    const expected = snap({ loop_closures: [{ id: 1 }], sections: [section(0, SHOTS)] });
    const actual = snap({ loop_closures: [{ id: 2 }], sections: [section(0, [{ uuid: 'shot-a', length: 1 }])] });

    const check = checkSnapshot(expected, actual, [], { diffLimit: 1, shownDiffs: 6 });

    expect(check.text).toContain('at project.loop_closures[0].id');
    expect(check.text).not.toContain('SHOT IDENTITY');
    expect(check.text).not.toContain('IDENTICAL');
  });

  it('lists the attributable families it subtracted before an unexplained remainder', () => {
    const expected = snap({ name: 'CaveA', sections: [section(0, SHOTS, null)] });
    const actual = snap({ name: 'CaveB', sections: [section(0, SHOTS, 'gauge-1')] });

    const check = checkSnapshot(expected, actual, SAVE_HEALING_MUTATIONS, LIMITS);

    expect(check.attributed.map((entry) => entry.id)).toEqual(['save-captures-missing-gauge-anchors']);
    expect(check.text.startsWith('  ATTRIBUTED')).toBe(false);
    expect(check.text).toContain('at project.name');
    expect(check.text).not.toContain('gauge_shot_uuid');
    expect(check.text).toContain(
      '  (plus attributable change(s) [save-captures-missing-gauge-anchors], subtracted before this diff)',
    );
  });

  it('explains a pure loop_closures reorder in one sentence', () => {
    const one = { from: 'A1', id: 1, to: 'A2' };
    const two = { from: 'B1', id: 2, to: 'B2' };
    const expected = snap({ loop_closures: [one, two], sections: [] });
    const actual = snap({ loop_closures: [two, one], sections: [] });

    const check = checkSnapshot(expected, actual, [], LIMITS);

    expect(check.equal).toBe(false);
    expect(check.text.split('\n').slice(-2)).toEqual([
      '  (every difference above is explained by project.loop_closures being REORDERED:',
      '   same closures, same ids, same metrics, different positions)',
    ]);
  });

  it('does not claim a reorder on a capped diff', () => {
    const one = { from: 'A1', id: 1, to: 'A2' };
    const two = { from: 'B1', id: 2, to: 'B2' };
    const expected = snap({ loop_closures: [one, two], sections: [] });
    const actual = snap({ loop_closures: [two, one], sections: [] });

    const check = checkSnapshot(expected, actual, [], { diffLimit: 2, shownDiffs: 6 });

    expect(check.text).toContain('THE DIFF WAS CAPPED at 2 path(s)');
    expect(check.text).not.toContain('REORDERED');
  });
});
