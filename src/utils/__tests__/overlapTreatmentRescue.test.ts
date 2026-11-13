/**
 * The rescue that stops a section disappearing when overlaps at both ends
 * delete every one of its shots.
 *
 * It has to count the rows that will be EMITTED: counting merely undeleted rows
 * let one blank added row - alive on screen, absent from the payload - stand in
 * for a whole section, so the rescue skipped, the batch emitted nothing for it,
 * and the section vanished with no report anywhere.
 */
import { describe, it, expect } from 'vitest';
import { rescueSectionsWithNoEmittedShots } from '../overlapTreatment';
import type { SectionEditState } from '../batchImportHelpers';
import type { EditableShot } from '../../components/common/ShotTableEditor';

const measured = (id: number, overrides: Partial<EditableShot> = {}): EditableShot =>
  ({
    id,
    azimuth_in: 10,
    azimuth_out: 12,
    length: 5,
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
    ...overrides,
  }) as EditableShot;

/** A row added in the grid and never filled in. */
const blank = (id: number): EditableShot =>
  measured(id, {
    isNew: true,
    length: NaN,
    azimuth_in: NaN,
    azimuth_out: NaN,
    depth_in: NaN,
    depth_out: NaN,
  });

const edits = (shots: EditableShot[]): SectionEditState => ({
  name: 'S',
  date: '',
  color: '',
  direction: 'in',
  quality: 1,
  reverse: false,
  shots,
});

describe('rescueSectionsWithNoEmittedShots', () => {
  it('un-deletes a section that would lose every shot, as reference copies', () => {
    const map = new Map([[0, edits([measured(0, { isDeleted: true }), measured(1, { isDeleted: true })])]]);
    expect(rescueSectionsWithNoEmittedShots(map)).toEqual([0]);
    const shots = map.get(0)!.shots;
    expect(shots.every((s) => s.isDeleted !== true)).toBe(true);
    expect(shots.every((s) => s.kind === 'Reference')).toBe(true);
  });

  it('fires when only a blank added row survives the deletions', () => {
    const map = new Map([
      [0, edits([measured(0, { isDeleted: true }), measured(1, { isDeleted: true }), blank(-1)])],
    ]);
    expect(rescueSectionsWithNoEmittedShots(map)).toEqual([0]);
    const shots = map.get(0)!.shots;
    expect(shots.filter((s) => s.kind === 'Reference')).toHaveLength(2);
    // The blank row is left exactly as it was - it is still an invitation.
    expect(shots[2]).toEqual(blank(-1));
  });

  it('leaves a section that still emits something alone', () => {
    const map = new Map([[0, edits([measured(0, { isDeleted: true }), measured(1)])]]);
    expect(rescueSectionsWithNoEmittedShots(map)).toEqual([]);
    expect(map.get(0)!.shots[0]!.isDeleted).toBe(true);
    expect(map.get(0)!.shots[1]!.kind).toBe('Normal');
  });

  it('has nothing to rescue when a section is empty for other reasons', () => {
    // No deletions to convert - the section simply has no measured rows, which
    // the emit loop reports rather than silently patches.
    const map = new Map([[0, edits([blank(-1)])], [1, edits([])]]);
    expect(rescueSectionsWithNoEmittedShots(map)).toEqual([]);
  });

  it('rescues each affected section independently', () => {
    const map = new Map([
      [0, edits([measured(0, { isDeleted: true })])],
      [1, edits([measured(0)])],
      [2, edits([measured(0, { isDeleted: true }), blank(-1)])],
    ]);
    expect(rescueSectionsWithNoEmittedShots(map)).toEqual([0, 2]);
  });
});
