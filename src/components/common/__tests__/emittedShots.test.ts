/**
 * The predicate and translations that keep the UI's station numbering and the
 * payload's in step.
 *
 * Dropping blank new rows from the payloads created two pictures of a line -
 * the one on screen, which counts a blank row, and the one sent, which does not.
 * Three separate sites converted between them wrongly (zero-datum reanchoring,
 * the DMP station adjustment, the manual segment anchor), each able to land a
 * GPS anchor or merge point on the wrong station. These pin the shared answer.
 */
import { describe, it, expect } from 'vitest';
import {
  isEmittedShot,
  emittedBefore,
  fromEmittedStationIndex,
  toEmittedStationIndex,
  visibleBefore,
  visibleShotCount,
} from '../emittedShots';
import type { EditableShot } from '../editableShot';

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

describe('isEmittedShot', () => {
  it('keeps a measured row', () => {
    expect(isEmittedShot(measured(0))).toBe(true);
  });

  it('drops deleted and wholly blank rows', () => {
    expect(isEmittedShot(deleted(0))).toBe(false);
    expect(isEmittedShot(blank(0))).toBe(false);
  });

  it('keeps a partly filled row - not blank, and the gate blocks it elsewhere', () => {
    expect(isEmittedShot({ ...blank(0), length: 7 } as EditableShot)).toBe(true);
  });

  it('is false for a missing row rather than throwing', () => {
    expect(isEmittedShot(undefined)).toBe(false);
  });
});

describe('emittedBefore', () => {
  const rows = [measured(0), blank(1), deleted(2), measured(3)];

  it('counts only rows that reach the backend', () => {
    expect(emittedBefore(rows, 4)).toBe(2);
    expect(emittedBefore(rows, 1)).toBe(1);
    expect(emittedBefore(rows, 3)).toBe(1); // blank and deleted both skipped
  });

  it('is 0 at the start and clamps past the end', () => {
    expect(emittedBefore(rows, 0)).toBe(0);
    expect(emittedBefore(rows, 99)).toBe(2);
  });
});

describe('toEmittedStationIndex', () => {
  it('pulls a pick back by the blanks that precede it', () => {
    // Visible order (blanks included) vs emitted: picking visible station 2
    // means "after two visible rows", of which only one is emitted.
    const rows = [measured(0), blank(1), measured(2), measured(3)];
    expect(toEmittedStationIndex(rows, 2)).toBe(1);
  });

  it('leaves a pick alone when no blank precedes it', () => {
    const rows = [measured(0), measured(1), blank(2)];
    expect(toEmittedStationIndex(rows, 2)).toBe(2);
  });

  it('clamps a pick past the end of the shortened line', () => {
    // Every row blank: the emitted line has no shots, so the only station
    // left is 0 - never an out-of-range index the backend would reject.
    const rows = [blank(0), blank(1)];
    expect(toEmittedStationIndex(rows, 2)).toBe(0);
  });

  it('ignores deleted rows on both sides of the count', () => {
    const rows = [deleted(0), measured(1), blank(2), measured(3)];
    expect(toEmittedStationIndex(rows, 2)).toBe(1);
  });
});

describe('visibleBefore / visibleShotCount', () => {
  const rows = [measured(0), blank(1), deleted(2), measured(3)];

  it('counts the rows on screen, blanks included and deleted ones not', () => {
    expect(visibleShotCount(rows)).toBe(3);
    expect(visibleBefore(rows, 4)).toBe(3);
    expect(visibleBefore(rows, 3)).toBe(2); // the deleted row occupies no station
    expect(visibleBefore(rows, 0)).toBe(0);
    expect(visibleBefore(rows, 99)).toBe(3);
  });

  it('differs from the emitted count by exactly the blank rows', () => {
    expect(visibleShotCount(rows) - rows.filter(isEmittedShot).length).toBe(1);
  });
});

describe('fromEmittedStationIndex', () => {
  it('pushes a drawn station past the blanks above it', () => {
    // The preview draws only the measured row, so its station 0 is the one the
    // grid calls station 1.
    const rows = [blank(0), measured(1)];
    expect(fromEmittedStationIndex(rows, 0)).toBe(1);
    expect(fromEmittedStationIndex(rows, 1)).toBe(2);
  });

  it('answers with the index it was handed when nothing is blank', () => {
    const rows = [measured(0), measured(1), measured(2)];
    for (const station of [0, 1, 2, 3]) {
      expect(fromEmittedStationIndex(rows, station)).toBe(station);
    }
  });

  it('ends the run at the last emitted row, not after a trailing blank', () => {
    const rows = [measured(0), blank(1)];
    expect(fromEmittedStationIndex(rows, 1)).toBe(1);
  });

  it('clamps a station the preview cannot have drawn', () => {
    expect(fromEmittedStationIndex([measured(0)], 9)).toBe(1);
    expect(fromEmittedStationIndex([blank(0)], 0)).toBe(0);
    expect(fromEmittedStationIndex([], 3)).toBe(0);
  });

  it('round-trips every drawn station back to itself', () => {
    // This is the property the preview click depends on: click a dot, store the
    // visible station, send the emitted one, and land on the dot clicked.
    const rows = [blank(0), measured(1), blank(2), measured(3), measured(4), blank(5)];
    const emitted = rows.filter(isEmittedShot).length;
    for (let station = 0; station <= emitted; station++) {
      expect(toEmittedStationIndex(rows, fromEmittedStationIndex(rows, station))).toBe(station);
    }
  });
});

/**
 * The reverse mirror reflects a station about the midpoint of its line, so it
 * has to count the same shots the payload sends. Counting non-deleted rows while
 * emitting non-blank ones reflected about the wrong midpoint and attached the
 * import to the wrong physical station.
 */
describe('mirroring across an emitted count', () => {
  const mirror = (station: number, shotCount: number) => shotCount - station;

  it('reflects about the emitted midpoint, not the visible one', () => {
    const rows = [measured(0), blank(1), measured(2), measured(3)];
    const emitted = rows.filter(isEmittedShot).length;
    expect(emitted).toBe(3);
    // Station 1 in emitted space mirrors to 2 of 3, not 3 of 4.
    expect(mirror(1, emitted)).toBe(2);
    expect(mirror(1, rows.filter((s) => s.isDeleted !== true).length)).toBe(3);
  });

  it('agrees with the visible count when nothing is blank', () => {
    const rows = [measured(0), measured(1)];
    expect(rows.filter(isEmittedShot).length).toBe(
      rows.filter((s) => s.isDeleted !== true).length
    );
  });
});
