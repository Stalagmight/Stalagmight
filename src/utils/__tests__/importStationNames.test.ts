/**
 * The import wizards' port of the backend's station-naming rules
 * (`station_naming.rs`): the cases mirror `station_naming_tests.rs`, so a
 * drift between the two shows up here.
 */
import { describe, it, expect } from 'vitest';
import {
  dmpFileStationLabel,
  dmpFileStationName,
  fillLineStationNames,
  importLineStationNames,
  nameWithFreeLetter,
  settleLineStationNames,
  suggestionTargetStationLabel,
} from '../importStationNames';
import type { EditableShot } from '../../components/common/editableShot';

const row = (mnemoStation?: number): EditableShot =>
  ({
    id: mnemoStation ?? -1,
    azimuth_in: 0,
    azimuth_out: 180,
    length: 5,
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
    ...(mnemoStation === undefined ? { isNew: true } : { mnemoStation }),
  }) as EditableShot;

describe('station naming port', () => {
  it('settles: seeds by index and letters the later repeat', () => {
    const names = ['5', '5', null, '1'];
    settleLineStationNames(names);
    expect(names).toEqual(['5', '5a', '2', '1']);

    const taken = ['1', null];
    settleLineStationNames(taken);
    expect(taken).toEqual(['1', '1a']);

    const blank = ['  ', 'x'];
    settleLineStationNames(blank);
    expect(blank).toEqual(['0', 'x']);
  });

  it('fills a partly named line by the creation rules', () => {
    // Mid letters inside, End numbers at the tail, Prepend backwards at the head.
    const names = [null, '5', null, null, '6', null, null];
    fillLineStationNames(names);
    expect(names).toEqual(['9', '5', '5a', '5b', '6', '7', '8']);

    const none: Array<string | null> = [null, null];
    fillLineStationNames(none);
    expect(none).toEqual([null, null]);
  });

  it('keeps a zero-padded or lettered family when it continues it', () => {
    const padded = ['007', null];
    fillLineStationNames(padded);
    expect(padded).toEqual(['007', '008']);

    const lettered = [null, 'A0', 'A1', 'A2', 'A3'];
    fillLineStationNames(lettered);
    expect(lettered[0]).toBe('A4');
  });

  it('hyphenates the Mid suffix of a name with no digit', () => {
    expect(nameWithFreeLetter('Junction', new Set(['Junction']))).toBe('Junction-a');
    expect(nameWithFreeLetter('Junction', new Set(['Junction', 'Junction-a']))).toBe('Junction-b');
    expect(nameWithFreeLetter('2a', new Set())).toBe('2aa');
    expect(nameWithFreeLetter('A3', new Set())).toBe('A3a');
    expect(nameWithFreeLetter('Sump2', new Set())).toBe('Sump2a');

    const repeats = ['Sump', 'Sump', 'Sump'];
    settleLineStationNames(repeats);
    expect(repeats).toEqual(['Sump', 'Sump-a', 'Sump-b']);

    const run = ['Junction', null, null, '6'];
    fillLineStationNames(run);
    expect(run).toEqual(['Junction', 'Junction-a', 'Junction-b', '6']);
  });

  it('continues past z with two-letter suffixes', () => {
    const names: Array<string | null> = ['4', ...Array.from({ length: 27 }, () => null), '5'];
    fillLineStationNames(names);
    expect(names[26]).toBe('4z');
    expect(names[27]).toBe('4aa');
  });
});

describe('importLineStationNames', () => {
  it('names an unedited DMP line by its MNemo numbers', () => {
    expect(importLineStationNames([row(1), row(2), row(3)])).toEqual(['0', '1', '2', '3']);
  });

  it('names the stations a wizard edit added by the creation rules', () => {
    // A row inserted after MNemo 1 ends at a station the file does not name: Mid.
    expect(importLineStationNames([row(1), row(), row(2)])).toEqual(['0', '1', '1a', '2']);
    // A row appended at the end: End.
    expect(importLineStationNames([row(1), row(2), row()])).toEqual(['0', '1', '2', '3']);
    // A row prepended: its start is not named by any DMP row, so it is
    // Prepend-named backwards from the first named station.
    expect(importLineStationNames([row(), row(1)])).toEqual(['2', '0', '1']);
  });

  it('starts a trimmed line at the MNemo number of its first surviving station', () => {
    expect(importLineStationNames([row(3), row(4)])).toEqual(['2', '3', '4']);
  });

  it('seeds a line with no names from its indexes', () => {
    expect(importLineStationNames([row(), row()])).toEqual(['0', '1', '2']);
  });

  it('fills a reversed line in its final order but returns the file order', () => {
    // Reversed, the appended row's end becomes the first station: Prepend.
    expect(importLineStationNames([row(1), row(2), row()], true)).toEqual(['0', '1', '2', '3']);
    expect(importLineStationNames([row(1), row(), row(2)], true)).toEqual(['0', '1', '2a', '2']);
  });
});

describe('dmpFileStationName', () => {
  it('is the MNemo number: station k of the file is named k', () => {
    expect(dmpFileStationName(0)).toBe('0');
    expect(dmpFileStationName(1)).toBe('1');
    expect(dmpFileStationName(12)).toBe('12');
  });
});

describe('DMP wizard station labels', () => {
  const labels = { stationName: () => 'A7' };

  it('names an incoming station with its line', () => {
    expect(dmpFileStationLabel(4, 'Upstream')).toBe('Upstream / 4');
  });

  it('names an existing target station by the project, an incoming one by the file', () => {
    expect(
      suggestionTargetStationLabel(labels, { targetType: 'existing', existingLineId: 5, existingLineName: 'Main Line' }, 1)
    ).toBe('Main Line / A7');
    expect(
      suggestionTargetStationLabel(labels, { targetType: 'importing', existingLineId: null, existingLineName: 'Dive 2' }, 3)
    ).toBe('Dive 2 / 3');
  });

  it("names an incoming target line by the user's rename in the wizard, the file's name until then", () => {
    const importing = { targetType: 'importing' as const, existingLineId: null, existingLineName: 'Dive 2', targetImportIndex: 4 };
    const renamed = (i: number) => (i === 4 ? 'Upper Sump' : undefined);
    expect(suggestionTargetStationLabel(labels, importing, 3, renamed)).toBe('Upper Sump / 3');
    expect(suggestionTargetStationLabel(labels, importing, 3, () => undefined)).toBe('Dive 2 / 3');
    // A project line is the project's, whatever the wizard holds.
    expect(
      suggestionTargetStationLabel(labels, { targetType: 'existing', existingLineId: 5, existingLineName: 'Main Line' }, 1, () => 'X'),
    ).toBe('Main Line / A7');
  });
});
