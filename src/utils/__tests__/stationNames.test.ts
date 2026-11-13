/**
 * mNemo station numbers -> station names, from the DMP rows the wizard sends.
 *
 * mNemo numbers a section's start 0 and the end of its N-th shot N. Only the
 * shots the importer keeps (`isImportableDmpShot`: Standard, not 1 m) advance
 * the number, so the numbers are fixed when the file is read and every later
 * wizard edit - a deleted row, an overlap trim, an added row - must leave each
 * surviving station on the number the diver's notes use. All data here is
 * synthetic.
 */
import { describe, it, expect } from 'vitest';
import { convertToEditableShots, mnemoStationNumber } from '../../hooks/useImportState';
import { buildZeroDatumSectionConfig, stationNamesForEmittedShots } from '../importSectionConfig';
import { isEmittedShot } from '../../components/common/emittedShots';
import { blankEditableShot, type EditableShot } from '../../components/common/editableShot';
import type { DmpShot } from '../../types/commands';
import type { ZeroDatumConfigResult } from '../../components/Dialogs/ZeroDatumConfigDialog';

function dmpShot(length: number, shot_type: DmpShot['shot_type'] = 'Standard'): DmpShot {
  return {
    shot_type,
    heading_in: 90,
    heading_out: 90,
    length,
    depth_in: 0,
    depth_out: 0,
    pitch_in: 0,
    pitch_out: 0,
    left: 0,
    right: 0,
    up: 0,
    down: 0,
    temperature: 0,
    hour: 0,
    minute: 0,
    second: 0,
    marker_index: 0,
  };
}

/** `count` importable 4 m shots, numbered 1..count by the importer. */
function rows(count: number): EditableShot[] {
  return convertToEditableShots(Array.from({ length: count }, () => dmpShot(4)));
}

const names = (shots: EditableShot[]) => stationNamesForEmittedShots(shots.filter(isEmittedShot));

const deleted = (shot: EditableShot): EditableShot => ({ ...shot, isDeleted: true });

/** A user-added row with readings filled in (a blank one is never emitted). */
function added(tempId: number): EditableShot {
  return { ...blankEditableShot(tempId), azimuth_in: 0, azimuth_out: 0, length: 3, depth_in: 0, depth_out: 0 };
}

describe('mNemo station numbering', () => {
  it('counts only the shots the importer keeps', () => {
    const shots = convertToEditableShots([
      dmpShot(4),
      dmpShot(1), // the 1 m shot the importer skips
      dmpShot(5),
      dmpShot(0, 'CsA'),
      dmpShot(6),
      dmpShot(0, 'EndOfSurvey'),
    ]);
    expect(shots.map((s) => s.mnemoStation)).toEqual([1, 2, 3]);
    expect(mnemoStationNumber(0)).toBe(1);
  });

  it('names every station of an untouched section 0..N', () => {
    expect(names(rows(3))).toEqual([
      [0, '0'],
      [1, '1'],
      [2, '2'],
      [3, '3'],
    ]);
  });

  it('a deleted interior row takes only its end station with it', () => {
    const shots = rows(4);
    shots[1] = deleted(shots[1]!); // mNemo 1 -> 2
    expect(names(shots)).toEqual([
      [0, '0'],
      [1, '1'],
      [2, '3'],
      [3, '4'],
    ]);
  });

  it('an overlap trim of the first rows starts the section at its real number', () => {
    const shots = rows(4);
    shots[0] = deleted(shots[0]!);
    shots[1] = deleted(shots[1]!);
    expect(names(shots)).toEqual([
      [0, '2'],
      [1, '3'],
      [2, '4'],
    ]);
  });

  it('a row the user added leaves its new station unnamed', () => {
    const [a, b] = rows(2);
    expect(names([a!, added(-1), b!])).toEqual([
      [0, '0'],
      [1, '1'],
      // station 2 is the end of the added row: no mNemo number
      [3, '2'],
    ]);
    // Prepended: the new start is unnamed, the old start keeps 0.
    expect(names([added(-2), a!, b!])).toEqual([
      [1, '0'],
      [2, '1'],
      [3, '2'],
    ]);
  });

  it('keeps numbering through rows kept as Reference shots', () => {
    const shots = rows(3).map((s, i) => (i === 1 ? { ...s, kind: 'Reference' as const } : s));
    expect(names(shots).map(([, name]) => name)).toEqual(['0', '1', '2', '3']);
  });

  it('a blank added row is not emitted and shifts nothing', () => {
    const [a, b] = rows(2);
    expect(names([a!, blankEditableShot(-1), b!])).toEqual([
      [0, '0'],
      [1, '1'],
      [2, '2'],
    ]);
  });

  it('rows without numbers produce no names', () => {
    expect(names([added(-1), added(-2)])).toEqual([]);
  });
});

describe('buildZeroDatumSectionConfig station names', () => {
  it('sends names in the emitted, pre-reverse order', () => {
    const shots = rows(3);
    shots[1] = deleted(shots[1]!);
    const config: ZeroDatumConfigResult = {
      sectionIndex: 0,
      stationIndex: 0,
      gps: { latitude: 20.2, longitude: -87.4, depth_reference: 0 },
      metadata: {
        name: 'A01',
        surveyors: [],
        explorers: [],
        date: null,
        color: '#ffffff',
        direction: 'in',
        quality: 1,
        reverse: true,
      },
      shots,
    };

    const built = buildZeroDatumSectionConfig(config);

    expect(built.shots).toHaveLength(2);
    // Not mirrored here: the backend mirrors names for a reversed import.
    expect(built.station_names).toEqual([
      [0, '0'],
      [1, '1'],
      [2, '3'],
    ]);
  });
});
