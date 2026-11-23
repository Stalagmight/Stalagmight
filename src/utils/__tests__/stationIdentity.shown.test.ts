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

  it('checks a rename through a loop end on every line its shown station is on', () => {
    // Renaming through Side 2 renames Main / 3, which Side 2 and Main 0 show:
    // 13 is taken on Main by another station, 1 on Side.
    expect(labels.stationNameTakenOn(side2, '13')).toBe(MAIN);
    expect(labels.stationNameTakenOn(side2, '1')).toBe(SIDE);
    expect(labels.stationNameTakenOn(side2, '30')).toBeNull();
  });
});
