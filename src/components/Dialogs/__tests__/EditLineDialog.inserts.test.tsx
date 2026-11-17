/**
 * Where the Edit Line dialog puts the rows it creates on Save.
 *
 * The dialog edits a LOCAL copy of the line and diffs it on Save, and its
 * create call passed no position at all - so a row inserted between two measured
 * shots (right-click "Insert N shots above/below", or the keyboard insert) was
 * written after the last shot instead. The passage came back rearranged from a
 * save that looked like it had only added a leg.
 *
 * These tests pin the anchoring rule rather than the call: every added row is
 * created before the next SAVED shot, so a run of consecutive added rows keeps
 * its on-screen order without the dialog having to thread the id the backend
 * mints for each one back into the next call.
 *
 * `.tsx` so vitest runs it under jsdom - the module under test is a component.
 */
import { describe, it, expect } from 'vitest';

import { plannedShotInserts } from '../EditLineDialog';
import { blankEditableShot } from '../../common/editableShot';
import type { EditableShot } from '../../common/editableShot';

/** A saved shot as the dialog holds it: real id, no isNew flag. */
function saved(id: number): EditableShot {
  return {
    id,
    azimuth_in: 90,
    azimuth_out: 90,
    length: 10,
    depth_in: 1,
    depth_out: 2,
    pitch_in: null,
    pitch_out: null,
    left: null,
    right: null,
    up: null,
    down: null,
    comment: null,
    kind: 'Normal',
  };
}

/** An added row the user has actually filled in, keyed by its temp id. */
function added(tempId: number): EditableShot {
  return { ...saved(tempId), tempId, isNew: true };
}

/** The plan as `(row id -> anchor)` pairs, in creation order. */
function plan(rows: EditableShot[]): [number, number | undefined][] {
  return plannedShotInserts(rows).map((entry) => [entry.shot.id, entry.beforeShotId]);
}

describe('plannedShotInserts', () => {
  it('anchors a mid-line row to the shot it was inserted above', () => {
    expect(plan([saved(1), added(-1), saved(2), saved(3)])).toEqual([[-1, 2]]);
  });

  it('gives a run of consecutive rows the same anchor, in on-screen order', () => {
    // Each insert lands immediately before shot 2, so the second goes in after
    // the first: 1, -1, -2, 2. Anchoring both to the PRECEDING shot instead
    // would put them in one after the other and reverse the run.
    expect(plan([saved(1), added(-1), added(-2), saved(2)])).toEqual([
      [-1, 2],
      [-2, 2],
    ]);
  });

  it('appends a trailing run, in order', () => {
    // Nothing saved follows them, so their position IS the end of the line.
    expect(plan([saved(1), added(-1), added(-2)])).toEqual([
      [-1, undefined],
      [-2, undefined],
    ]);
  });

  it('anchors leading rows to the first saved shot', () => {
    expect(plan([added(-1), added(-2), saved(1), saved(2)])).toEqual([
      [-1, 1],
      [-2, 1],
    ]);
  });

  it('appends everything when the line has no saved shot left', () => {
    expect(plan([saved(1), added(-1), added(-2)].map((s) => (s.isNew ? s : { ...s, isDeleted: true })))).toEqual([
      [-1, undefined],
      [-2, undefined],
    ]);
  });

  it('skips a deleted shot when picking the anchor', () => {
    // Shot 2 is deleted FIRST on save, so quoting its id would fail the create.
    const rows = [saved(1), added(-1), { ...saved(2), isDeleted: true }, saved(3)];
    expect(plan(rows)).toEqual([[-1, 3]]);
  });

  it('leaves out rows that will not be written', () => {
    const rows = [
      saved(1),
      blankEditableShot(-1), // added and never filled in
      { ...added(-2), isDeleted: true },
      added(-3),
      saved(2),
    ];
    expect(plan(rows)).toEqual([[-3, 2]]);
  });

  it('plans nothing for a line that was only edited', () => {
    expect(plan([saved(1), saved(2)])).toEqual([]);
    expect(plan([])).toEqual([]);
  });

  it('handles several runs at once, each against its own anchor', () => {
    const rows = [
      added(-1),
      saved(1),
      added(-2),
      added(-3),
      saved(2),
      saved(3),
      added(-4),
    ];
    expect(plan(rows)).toEqual([
      [-1, 1],
      [-2, 2],
      [-3, 2],
      [-4, undefined],
    ]);
  });

  it('walks a mixed line into the order it is displayed in', () => {
    // Replay the plan against the saved line to prove the end state matches
    // the dialog's array: each entry is spliced in before its anchor, and an
    // unanchored one is appended.
    const rows = [saved(1), added(-1), added(-2), saved(2), added(-3), saved(3), added(-4)];
    const line = rows.filter((s) => s.isNew !== true).map((s) => s.id);
    for (const { shot, beforeShotId } of plannedShotInserts(rows)) {
      const at = beforeShotId === undefined ? line.length : line.indexOf(beforeShotId);
      line.splice(at, 0, shot.id);
    }
    expect(line).toEqual(rows.map((s) => s.id));
  });
});
