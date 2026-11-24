/**
 * RS-PROSE: the import wizards' connection descriptions as prose. Each token
 * reads, word for word, the plain label the wizard writes elsewhere (select
 * options, the alternatives' buttons): an existing slot as its coloured pill,
 * an incoming line's station as a neutral pill (no line id: it is not in the
 * project yet) with that line's wizard name as its context.
 * Injections: a line id on the incoming part fails "neutral"; reading the
 * file's line name over the wizard rename fails the rename case.
 */
import { describe, expect, it } from 'vitest';
import '../../i18n';
import { dmpFileStationLabel, suggestionTargetStationLabel } from '../importStationNames';
import { dmpFileStationPart, suggestionTargetStationProse } from '../importStationProse';
import { proseParts, proseToPlain, proseTokens } from '../prose';
import { identityLabels } from '../stationIdentity';
import type { Station } from '../../types/project';

// Main Line (5): station 1 is its own A7; station 2 shows Upstream's 12
// through a merge (a foreign slot, RS 6).
const src = {
  stations: [
    { station_ref: { line_id: 5, station_index: 1 }, id: 9, name: 'A7' },
    {
      station_ref: { line_id: 5, station_index: 2 },
      id: 12,
      name: '12',
      shown: { id: 12, home_line: 6, foreign: true, loop_end: false },
    },
  ] as Station[],
  lines: [
    { id: 5, name: 'Main Line' },
    { id: 6, name: 'Upstream' },
  ],
};
const labels = identityLabels(src);
const prose = proseTokens(src);

const existing = { targetType: 'existing' as const, existingLineId: 5, existingLineName: 'Main Line' };
const importing = { targetType: 'importing' as const, existingLineId: null, existingLineName: 'Dive 2', targetImportIndex: 4 };

describe('import connection prose', () => {
  it('names an incoming station by a neutral pill that reads as its plain label', () => {
    const part = dmpFileStationPart(4, 'Upstream');
    expect(part).toEqual({ kind: 'station', text: 'Upstream / 4', name: '4', context: 'Upstream' });
    expect(part.text).toBe(dmpFileStationLabel(4, 'Upstream'));
    expect('line_id' in part).toBe(false);
  });

  it('names an existing target slot by its pill, a foreign one by the line it belongs to (RS-OWNERSHIP)', () => {
    for (const index of [1, 2]) {
      const token = suggestionTargetStationProse(prose, existing, index);
      expect(proseToPlain(token)).toBe(suggestionTargetStationLabel(labels, existing, index));
      const [part] = proseParts(token);
      expect(part).toMatchObject({ kind: 'station', line_id: 5 });
    }
    expect(proseParts(suggestionTargetStationProse(prose, existing, 1))[0]).toMatchObject({ context: 'Main Line' });
    // The foreign slot's pill is coloured and named by the line its station belongs to.
    expect(proseParts(suggestionTargetStationProse(prose, existing, 2))[0]).toMatchObject({
      name: '12',
      home_line: 6,
      context: 'Upstream',
    });
  });

  it("names an incoming target by the user's rename in the wizard, the file's name until then", () => {
    const renamed = (i: number) => (i === 4 ? 'Upper Sump' : undefined);
    for (const name of [renamed, () => undefined]) {
      const token = suggestionTargetStationProse(prose, importing, 3, name);
      expect(proseToPlain(token)).toBe(suggestionTargetStationLabel(labels, importing, 3, name));
      expect('line_id' in proseParts(token)[0]!).toBe(false);
    }
    expect(proseToPlain(suggestionTargetStationProse(prose, importing, 3, renamed))).toBe('Upper Sump / 3');
    // A project line is the project's, whatever the wizard holds.
    expect(proseToPlain(suggestionTargetStationProse(prose, existing, 1, () => 'X'))).toBe('Main Line / A7');
  });
});
