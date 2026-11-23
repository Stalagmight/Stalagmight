/**
 * RS 6: every display reads the identity a slot SHOWS. A slot keeps its node
 * id (groupings, moves, selections), and a foreign slot or a loop end carries
 * `shown`: the Station ID shown, its home line and the loop mark. Plain text
 * says whose station a foreign slot shows ("Side / 12 (Main)", U-P3-8) and
 * what a loop end is shown as ("Main / 3 (loop end on Side)").
 * Injection: reading `stationIds` for the displayed ID fails "reads the
 * shown Station ID".
 */
import { describe, expect, it } from 'vitest';
import '../../i18n';
import { identityLabels, type IdentityData } from '../stationIdentity';
import type { ShownSlot } from '../../types/ipc/ShownSlot';

const MAIN = 1;
const SIDE = 2;

/** Main 0..2 and Side 0..2. Main 1 and Side 0 are one junction (node 11)
 *  that keeps Main's 12, so Side's slot is foreign; Side 2 (node 22) is a
 *  loop end shown as Main / 3 (Station ID 3). */
function data(): IdentityData {
  const shown = new Map<number, ShownSlot>([
    [3, { id: 11, home_line: MAIN, foreign: true, loop_end: false }],
    [5, { id: 3, home_line: MAIN, foreign: true, loop_end: true, loop_closure: 9n }],
  ]);
  return {
    stationCount: 6,
    stationLineIds: new Uint32Array([MAIN, MAIN, MAIN, SIDE, SIDE, SIDE]),
    stationIndices: new Uint32Array([0, 1, 2, 0, 1, 2]),
    stationNames: new Map([
      [0, '3'],
      [1, '12'],
      [2, '13'],
      [3, '12'],
      [4, '1'],
      [5, '3'],
    ]),
    stationIds: new Uint32Array([3, 11, 13, 11, 21, 22]),
    stationShown: shown,
    lines: [
      { id: MAIN, name: 'Main', shotOffset: 0, shotCount: 2 },
      { id: SIDE, name: 'Side', shotOffset: 2, shotCount: 2 },
    ],
    shotIds: new Int32Array([1, 2, 3, 4]),
  } as unknown as IdentityData;
}

describe('shown identity labels (RS 6)', () => {
  const labels = identityLabels({ surveyData: data() });
  const main1 = { line_id: MAIN, station_index: 1 };
  const side0 = { line_id: SIDE, station_index: 0 };
  const side2 = { line_id: SIDE, station_index: 2 };

  it('reads the shown Station ID for display, the node id for groupings', () => {
    expect(labels.shownStationId(side2)).toBe(3);
    expect(labels.stationId(side2)).toBe(22);
    expect(labels.shownStationId(side0)).toBe(11);
    expect(labels.shownStationId(main1)).toBe(11);
  });

  it('gives a foreign slot the dot of its home line, a native slot none', () => {
    expect(labels.homeMark(side0)).toBe(MAIN);
    expect(labels.homeMark(main1)).toBeNull();
    expect(labels.shown(side2)?.loop_end).toBe(true);
    expect(labels.shown(main1)).toBeUndefined();
  });

  it('says whose station a foreign slot shows in plain text (U-P3-8)', () => {
    expect(labels.station(side0, true)).toBe('Side / 12 (Main)');
    expect(labels.station(main1, true)).toBe('Main / 12');
    // Without the line, the name alone.
    expect(labels.station(side0)).toBe('12');
  });

  it('names a loop end by what it is shown as and where it is', () => {
    expect(labels.station(side2, true)).toBe('Main / 3 (loop end on Side)');
  });

  it('checks a rename through a loop end on the lines its shown station is native on (RS-FX5)', () => {
    // Renaming through Side 2 renames Main / 3, which Main 0 shows natively
    // and Side 2 as a loop end: 13 is taken on Main by another station; Side
    // is not checked, so its own 1 is free.
    expect(labels.stationNameTakenOn(side2, '13')).toBe(MAIN);
    expect(labels.stationNameTakenOn(side2, '1')).toBeNull();
    expect(labels.stationNameTakenOn(side2, '30')).toBeNull();
  });

  it('checks a rename through a foreign slot on the kept station\'s native lines only (RS-FX5)', () => {
    // Main / 12 is shown on Side through the foreign slot Side 0: renaming it
    // (through either slot) to Side's own 1 is free, to Main's 13 is not.
    expect(labels.stationNameTakenOn(side0, '1')).toBeNull();
    expect(labels.stationNameTakenOn(main1, '1')).toBeNull();
    expect(labels.stationNameTakenOn(side0, '13')).toBe(MAIN);
    expect(labels.stationNameTakenOn(main1, '3')).toBe(MAIN);
  });

  it('lets a native station take a name only dotted slots show on its line (RS-FX5)', () => {
    // Side 1 is Side's own station: the foreign 12 and the loop end's 3 on
    // Side count for no uniqueness.
    const side1 = { line_id: SIDE, station_index: 1 };
    expect(labels.stationNameTakenOn(side1, '12')).toBeNull();
    expect(labels.stationNameTakenOn(side1, '3')).toBeNull();
  });
});

/**
 * The user's example (RS-FX5): Main / 12 is kept at Branch's start, so
 * Branch's slot 0 shows "12" with Main's dot, and Branch's own station 2 keeps
 * its surveyed name "12". Neither counts against the other.
 */
describe('dotted slots do not collide (RS-FX5)', () => {
  const BRANCH = 3;
  const d = {
    stationCount: 5,
    stationLineIds: new Uint32Array([MAIN, MAIN, BRANCH, BRANCH, BRANCH]),
    stationIndices: new Uint32Array([0, 1, 0, 1, 2]),
    stationNames: new Map([
      [0, '11'],
      [1, '12'],
      [2, '12'],
      [3, '1'],
      [4, '12'],
    ]),
    stationIds: new Uint32Array([10, 12, 12, 31, 32]),
    stationShown: new Map<number, ShownSlot>([[2, { id: 12, home_line: MAIN, foreign: true, loop_end: false }]]),
    lines: [
      { id: MAIN, name: 'Main', shotOffset: 0, shotCount: 1 },
      { id: BRANCH, name: 'Branch', shotOffset: 1, shotCount: 2 },
    ],
    shotIds: new Int32Array([1, 2, 3]),
  } as unknown as IdentityData;
  const labels = identityLabels({ surveyData: d });
  const branch0 = { line_id: BRANCH, station_index: 0 };
  const branch2 = { line_id: BRANCH, station_index: 2 };

  it("keeps Branch's own 12 renamable to anything Branch's native slots do not show", () => {
    expect(labels.stationNameTakenOn(branch2, '12')).toBeNull();
    expect(labels.stationNameTakenOn(branch2, '1')).toBe(BRANCH);
    expect(labels.stationNameTakenOn(branch2, '11')).toBeNull();
  });

  it("checks Main / 12's rename on Main only", () => {
    expect(labels.stationNameTakenOn(branch0, '1')).toBeNull();
    expect(labels.stationNameTakenOn(branch0, '11')).toBe(MAIN);
  });
});
