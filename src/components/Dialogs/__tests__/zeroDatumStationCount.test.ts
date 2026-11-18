/**
 * The dialog's station domain must match the payload's.
 *
 * `buildZeroDatumLineConfig` drops blank new rows before serializing. The
 * dialog counted them, so inserting a blank row before the chosen zero-datum
 * station left the picker, the preview and the re-anchor one ahead of the
 * backend - and the GPS anchor was written to a different station than the one
 * the user selected. These pin the shared predicate both sides now use.
 */
import { describe, it, expect } from 'vitest';
import { isBlankNewShot } from '../../common/editableShotStatus';
import type { EditableShot } from '../../common/editableShot';

const measured = (id: number): EditableShot =>
  ({
    id,
    length: 5,
    azimuth_in: 10,
    azimuth_out: 10,
    depth_in: 0,
    depth_out: 0,
    pitch_in: null,
    pitch_out: null,
    left: null,
    right: null,
    up: null,
    down: null,
    comment: null,
    kind: 'Normal',
  }) as unknown as EditableShot;

const blank = (id: number): EditableShot =>
  ({
    ...measured(id),
    isNew: true,
    length: NaN,
    azimuth_in: NaN,
    azimuth_out: NaN,
    depth_in: NaN,
    depth_out: NaN,
  }) as unknown as EditableShot;

const deleted = (id: number): EditableShot =>
  ({ ...measured(id), isDeleted: true }) as unknown as EditableShot;

/** The predicate the dialog counts by, mirrored from ZeroDatumConfigDialog. */
const isCounted = (s: EditableShot | undefined): boolean =>
  s !== undefined && s.isDeleted !== true && !isBlankNewShot(s);

describe('zero-datum station counting', () => {
  it('ignores a blank new row, as the payload does', () => {
    const shots = [measured(0), blank(1), measured(2)];
    expect(shots.filter(isCounted)).toHaveLength(2);
  });

  it('ignores deleted rows too', () => {
    expect([measured(0), deleted(1)].filter(isCounted)).toHaveLength(1);
  });

  it('counts a partly filled row - it is not blank, and the gate blocks it', () => {
    const partial = { ...blank(1), length: 7 } as EditableShot;
    expect([measured(0), partial].filter(isCounted)).toHaveLength(2);
  });

  it('puts the station index back in step when a blank precedes the pick', () => {
    // Station N sits after N counted shots. With a blank row inserted at index 1
    // the user's "station 2" must still mean the same physical station.
    const withBlank = [measured(0), blank(9), measured(1), measured(2)];
    const before = (arrayIndex: number) =>
      withBlank.slice(0, arrayIndex).filter(isCounted).length;
    expect(before(2)).toBe(1); // the blank did not advance the count
    expect(before(4)).toBe(3);
  });
});
