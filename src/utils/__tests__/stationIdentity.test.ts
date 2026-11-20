import { describe, expect, it } from 'vitest';
import '../../i18n';
import {
  identityLabels,
  lineDisplayName,
  lineStationNamesOf,
  shotIndexOf,
  stationGlobalIndexOf,
  warningMessageBody,
  type IdentityData,
} from '../stationIdentity';
import type { Station } from '../../types/project';

/**
 * Two lines laid out the way the binary loader does: line 7 ("Main Line",
 * stations "4" "4a" "5") then line 2 ("Side", stations "A0" "A1"), with
 * Station IDs that are NOT the indexes - so a label that used the position
 * would show up here.
 */
function data(): IdentityData {
  return {
    stationCount: 5,
    stationLineIds: new Uint32Array([7, 7, 7, 2, 2]),
    stationIndices: new Uint32Array([0, 1, 2, 0, 1]),
    stationNames: new Map([
      [0, '4'],
      [1, '4a'],
      [2, '5'],
      [3, 'A0'],
      [4, 'A1'],
    ]),
    stationIds: new Uint32Array([31, 30, 44, 12, 13]),
    lines: [
      { id: 7, name: 'Main Line', color: '#f00', direction: 'In', quality: 1, next_shot_id: 30, shotOffset: 0, shotCount: 2 },
      { id: 2, name: 'Side', color: '#0f0', direction: 'In', quality: 1, next_shot_id: 5, shotOffset: 2, shotCount: 1 },
    ],
    shotIds: new Int32Array([21, 20, 4]),
  } as unknown as IdentityData;
}

describe('station identity labels', () => {
  const labels = identityLabels({ surveyData: data() });

  it('names a station by its name, with the line on request', () => {
    expect(labels.station({ line_id: 7, station_index: 1 })).toBe('4a');
    expect(labels.station({ line_id: 7, station_index: 1 }, true)).toBe('Main Line / 4a');
    expect(labels.station({ line_id: 2, station_index: 0 }, true)).toBe('Side / A0');
  });

  it('reads the Station ID, not the index', () => {
    expect(labels.stationId({ line_id: 7, station_index: 0 })).toBe(31);
    expect(labels.stationId({ line_id: 2, station_index: 1 })).toBe(13);
    expect(labels.stationId({ line_id: 9, station_index: 0 })).toBeNull();
  });

  it('names a shot from its two stations, by position or by id', () => {
    expect(labels.shotAt(7, 0)).toBe('4 → 4a');
    expect(labels.shotAt(7, 1, true)).toBe('Main Line: 4a → 5');
    // Shot id 20 is the line's SECOND shot: ids are handles, not positions.
    expect(labels.shot(7, 20, true)).toBe('Main Line: 4a → 5');
    expect(labels.shot(7, 99)).toBeNull();
  });

  it('falls back to the index text for a station it cannot find', () => {
    expect(labels.stationName({ line_id: 9, station_index: 3 })).toBe('3');
  });

  it('finds global indexes and a line\'s names by index', () => {
    const d = data();
    expect(stationGlobalIndexOf(d, { line_id: 2, station_index: 1 })).toBe(4);
    expect(lineStationNamesOf({ surveyData: d }, 7)).toEqual(['4', '4a', '5']);
    expect(shotIndexOf({ surveyData: d }, 2, 4)).toBe(0);
  });

  it('works from a station list when there is no SurveyData', () => {
    const stations = [
      { station_ref: { line_id: 1, station_index: 0 }, name: 'X', id: 8 },
      { station_ref: { line_id: 1, station_index: 1 }, name: 'Y', id: 9 },
    ] as Station[];
    const fromList = identityLabels({ stations, lines: [{ id: 1, name: 'L', shots: [{ id: 5 }] }] });
    expect(fromList.station({ line_id: 1, station_index: 1 }, true)).toBe('L / Y');
    expect(fromList.stationId({ line_id: 1, station_index: 0 })).toBe(8);
    expect(fromList.shot(1, 5, true)).toBe('L: X → Y');
    expect(fromList.lineStationNames(1)).toEqual(['X', 'Y']);
  });

  it('names a line by its name, never its id', () => {
    expect(labels.lineName(2)).toBe('Side');
    expect(labels.lineName(99)).toBe('Unnamed line');
    expect(lineDisplayName('  ')).toBe('Unnamed line');
    expect(lineDisplayName('‮Main')).toBe('Main');
  });
});

describe('survey warning text', () => {
  const labels = identityLabels({ surveyData: data() });
  const warning = { lineId: 7, shotIndex: 1, message: 'Shot Main Line: 4a \u2192 5 has zero length' };

  it('drops the named shot the backend opens the message with', () => {
    expect(warningMessageBody(labels, warning)).toBe('has zero length');
  });

  it('leaves a message that does not open with the shot as it is', () => {
    expect(warningMessageBody(labels, { ...warning, message: 'GPS fix is far away' })).toBe('GPS fix is far away');
    // A positional prefix is no longer produced, and never parsed.
    expect(warningMessageBody(labels, { ...warning, message: 'Shot 7:2 has zero length' })).toBe(
      'Shot 7:2 has zero length',
    );
  });
});

/**
 * A junction on the station graph: line 2 ("Side") starts ON Main Line's
 * station "4a", so the two slots share Station ID 30 and the name (plan 9.2).
 */
function junction(): IdentityData {
  const d = data();
  d.stationIds[3] = 30;
  d.stationNames.set(3, '4a');
  return d;
}

describe('a junction', () => {
  const labels = identityLabels({ surveyData: junction() });

  it('lists every slot of the station, on every line through it', () => {
    expect(labels.stationSlots({ line_id: 2, station_index: 0 })).toEqual([
      { line_id: 7, station_index: 1 },
      { line_id: 2, station_index: 0 },
    ]);
    expect(labels.stationSlots({ line_id: 7, station_index: 2 })).toEqual([{ line_id: 7, station_index: 2 }]);
  });

  it("refuses a rename to a name taken on ANY of the junction's lines, naming that line", () => {
    // "A1" is free on Main Line but taken on Side, which also passes the junction.
    expect(labels.stationNameTakenOn({ line_id: 7, station_index: 1 }, 'A1')).toBe(2);
    expect(labels.stationNameTakenOn({ line_id: 7, station_index: 1 }, '5')).toBe(7);
    // Its own name, shown on both of its slots, is not a collision.
    expect(labels.stationNameTakenOn({ line_id: 2, station_index: 0 }, '4a')).toBeNull();
    expect(labels.stationNameTakenOn({ line_id: 7, station_index: 1 }, 'fresh')).toBeNull();
  });

  it('checks only its own line for a station on one line', () => {
    expect(labels.stationNameTakenOn({ line_id: 7, station_index: 2 }, 'A1')).toBeNull();
    expect(labels.stationNameTakenOn({ line_id: 7, station_index: 2 }, '4')).toBe(7);
  });

  it('reads the same off the Station list when there is no SurveyData', () => {
    const station = (line_id: number, station_index: number, id: number, name: string): Station =>
      ({ station_ref: { line_id, station_index }, id, name, merged_with: [] }) as unknown as Station;
    const fromList = identityLabels({
      stations: [station(7, 0, 31, '4'), station(7, 1, 30, '4a'), station(2, 0, 30, '4a'), station(2, 1, 13, 'A1')],
    });
    expect(fromList.stationSlots({ line_id: 7, station_index: 1 })).toHaveLength(2);
    expect(fromList.stationNameTakenOn({ line_id: 7, station_index: 1 }, 'A1')).toBe(2);
  });
});

describe('onePerStation', () => {
  it("keeps one slot of each station, so a junction's move is sent once", () => {
    const labels = identityLabels({ surveyData: junction() });
    expect(
      labels.onePerStation([
        { line_id: 2, station_index: 0 },
        { line_id: 7, station_index: 1 },
        { line_id: 7, station_index: 2 },
        { line_id: 9, station_index: 0 },
      ]),
    ).toEqual([
      { line_id: 2, station_index: 0 },
      { line_id: 7, station_index: 2 },
      // Unknown to the SoA: kept, the backend decides.
      { line_id: 9, station_index: 0 },
    ]);
  });
});

describe('datumOfStation', () => {
  it("finds a junction's datum from any of its slots, its own slot first", () => {
    const labels = identityLabels({ surveyData: junction() });
    const onMain = { station_ref: { line_id: 7, station_index: 1 }, tag: 'main' };
    const onSide = { station_ref: { line_id: 2, station_index: 0 }, tag: 'side' };
    expect(labels.datumOf([onMain], { line_id: 2, station_index: 0 })?.tag).toBe('main');
    expect(labels.datumOf([onMain, onSide], { line_id: 2, station_index: 0 })?.tag).toBe('side');
    expect(labels.datumOf([onMain], { line_id: 7, station_index: 2 })).toBeUndefined();
    expect(labels.datumOf(undefined, { line_id: 7, station_index: 1 })).toBeUndefined();
  });
});
