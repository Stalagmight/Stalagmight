/**
 * The client-side name rules (SPEC 4.1/4.2): a line name is required and
 * unique in the project, a station name required and unique in its line; the
 * defaults a dialog offers never clash.
 */
import { describe, it, expect } from 'vitest';
import '../../i18n';
import {
  defaultSplitNames,
  hasForbiddenNameChars,
  isLineNameRefusal,
  lineNameProblem,
  lineNameProblemMessage,
  nextFreeLineName,
  splitNameProblems,
  stationNameProblem,
  stationNameProblemMessage,
  uniqueLineName,
} from '../nameValidation';

const lines = (...names: Array<string | null>) => names.map((name, i) => ({ id: i + 1, name }));

describe('lineNameProblem', () => {
  it('refuses an empty or blank name', () => {
    expect(lineNameProblem('', lines('Main'))).toBe('empty');
    expect(lineNameProblem('   ', lines('Main'))).toBe('empty');
  });

  it("refuses another line's name, compared trimmed and exactly", () => {
    expect(lineNameProblem('Main', lines('Main', 'Side'))).toBe('duplicate');
    expect(lineNameProblem('  Main ', lines('Main', 'Side'))).toBe('duplicate');
    expect(lineNameProblem('Main', lines(' Main ', 'Side'))).toBe('duplicate');
    // Exact: case and inner spacing count.
    expect(lineNameProblem('main', lines('Main'))).toBeNull();
    expect(lineNameProblem('Main  Line', lines('Main Line'))).toBeNull();
    expect(lineNameProblem('Upstream', lines('Main', 'Side'))).toBeNull();
  });

  it("accepts the renamed line's own name", () => {
    expect(lineNameProblem('Side', lines('Main', 'Side'), 2)).toBeNull();
    expect(lineNameProblem('Main', lines('Main', 'Side'), 2)).toBe('duplicate');
  });

  it('treats no project as no clash, and ignores unnamed lines', () => {
    expect(lineNameProblem('Main', null)).toBeNull();
    expect(lineNameProblem('Main', undefined)).toBeNull();
    expect(lineNameProblem('Main', lines(null))).toBeNull();
  });
});

describe('stationNameProblem', () => {
  it('refuses an empty name and a name another station of the line holds', () => {
    const names = ['0', '1', '2a', '3'];
    expect(stationNameProblem(' ', names, 1)).toBe('empty');
    expect(stationNameProblem('3', names, 1)).toBe('duplicate');
    expect(stationNameProblem(' 2a ', names, 1)).toBe('duplicate');
  });

  it("accepts the station's own name and a fresh one", () => {
    const names = ['0', '1', '2'];
    expect(stationNameProblem('1', names, 1)).toBeNull();
    expect(stationNameProblem('1a', names, 1)).toBeNull();
  });
});

describe('nextFreeLineName', () => {
  it('gives the smallest free Line N', () => {
    expect(nextFreeLineName([])).toBe('Line 1');
    expect(nextFreeLineName(null)).toBe('Line 1');
    expect(nextFreeLineName(lines('Line 1', 'Line 2'))).toBe('Line 3');
    expect(nextFreeLineName(lines('Line 2', 'Main'))).toBe('Line 1');
    expect(nextFreeLineName(lines('Line 1', 'Line 3'))).toBe('Line 2');
    expect(nextFreeLineName(lines(' Line 1 '))).toBe('Line 2');
  });
});

describe('uniqueLineName', () => {
  it('keeps a free base, else chains (2), (3), ...', () => {
    expect(uniqueLineName('Main', lines('Side'))).toBe('Main');
    expect(uniqueLineName('Main', lines('Main'))).toBe('Main (2)');
    expect(uniqueLineName('Main', lines('Main', 'Main (2)'))).toBe('Main (3)');
    expect(uniqueLineName('Main', lines('Main', 'Main (3)'))).toBe('Main (2)');
  });

  it('also avoids the extra names it is given, and trims', () => {
    expect(uniqueLineName('Main', lines('Side'), ['Main'])).toBe('Main (2)');
    expect(uniqueLineName(' Main ', lines('Main'), ['Main (2)'])).toBe('Main (3)');
  });
});

describe('defaultSplitNames', () => {
  it('names the halves <name> A and <name> B', () => {
    expect(defaultSplitNames('Main', lines('Main', 'Side'), 1)).toEqual({ a: 'Main A', b: 'Main B' });
  });

  it('makes each unique against the other lines and against each other', () => {
    const project = lines('Main', 'Main A', 'Main B', 'Main B (2)');
    const names = defaultSplitNames('Main', project, 1);
    expect(names).toEqual({ a: 'Main A (2)', b: 'Main B (3)' });
    expect(names.a).not.toBe(names.b);
    expect(splitNameProblems(names.a, names.b, project, 1)).toEqual({ a: null, b: null });
  });

  it("frees the split line's own name for half A", () => {
    // Splitting "Main A" (line 1): its own name is not a clash for either half.
    const names = defaultSplitNames('Main A', lines('Main A', 'Main A A'), 1);
    expect(names).toEqual({ a: 'Main A A (2)', b: 'Main A B' });
  });

  it('falls back to "Line" for an unnamed line', () => {
    expect(defaultSplitNames(null, lines(null), 1)).toEqual({ a: 'Line A', b: 'Line B' });
    expect(defaultSplitNames('  ', lines('Line A'), 1)).toEqual({ a: 'Line A', b: 'Line B' });
  });
});

describe('splitNameProblems', () => {
  const project = lines('Main', 'Side');

  it('accepts two free names, and the line keeping its own', () => {
    expect(splitNameProblems('Main', 'Main tail', project, 1)).toEqual({ a: null, b: null });
    // Half A takes a new name; B may take the old one (the line gives it up).
    expect(splitNameProblems('Main head', 'Main', project, 1)).toEqual({ a: null, b: null });
  });

  it('flags an empty name, another line, and the two halves alike', () => {
    expect(splitNameProblems('', 'B', project, 1)).toEqual({ a: 'empty', b: null });
    expect(splitNameProblems('A', 'Side', project, 1)).toEqual({ a: null, b: 'duplicate' });
    expect(splitNameProblems('Same', ' Same ', project, 1)).toEqual({ a: null, b: 'same' });
  });
});

describe('lineNameProblemMessage', () => {
  it('says what is wrong, naming the taken name', () => {
    expect(lineNameProblemMessage('empty', '')).toBe('A line needs a name.');
    expect(lineNameProblemMessage('duplicate', ' Main ')).toBe(
      'A line named "Main" already exists. Line names must be unique in the project.'
    );
    expect(lineNameProblemMessage('same', 'X')).toBe('The two lines need different names.');
  });
});

describe('isLineNameRefusal', () => {
  it("recognises the backend's line-name refusals only", () => {
    expect(isLineNameRefusal('A line name cannot be empty')).toBe(true);
    expect(isLineNameRefusal('A line named "Main" already exists')).toBe(true);
    expect(
      isLineNameRefusal(
        'A line name cannot contain control or invisible formatting characters (U+200E)'
      )
    ).toBe(true);
    expect(isLineNameRefusal('A station name cannot contain control characters')).toBe(false);
    expect(isLineNameRefusal('Line 3 not found')).toBe(false);
    expect(isLineNameRefusal(null)).toBe(false);
    expect(isLineNameRefusal(undefined)).toBe(false);
  });
});

describe('forbidden characters', () => {
  // Cc, LRM/RLM, the bidi embeddings / overrides / isolates, zero-width
  // space and joiners, BOM - the characters the backend refuses in a name.
  const bad = [
    '\u0000', '\u0007', '\u001F', '\u007F', '\u0085', '\u009F',
    '\u200B', '\u200C', '\u200D', '\u200E', '\u200F',
    '\u202A', '\u202B', '\u202C', '\u202D', '\u202E',
    '\u2066', '\u2067', '\u2068', '\u2069', '\uFEFF',
  ];

  it('flags each of them inside a name', () => {
    for (const ch of bad) expect(hasForbiddenNameChars(`a${ch}b`), JSON.stringify(ch)).toBe(true);
  });

  it('lets ordinary names through, and trims a pasted newline away', () => {
    for (const ok of ['Main Line', 'A3', '2a', 'Cueva \u00f1', '\u0644\u0627', 'x \u00b7 y', 'Main\n', '\tSide']) {
      expect(hasForbiddenNameChars(ok), JSON.stringify(ok)).toBe(false);
    }
  });

  it('counts a BOM at the edges, which the JS trim would drop', () => {
    expect(hasForbiddenNameChars('\uFEFFMain')).toBe(true);
    expect(hasForbiddenNameChars('Main\uFEFF')).toBe(true);
  });

  it('is a line- and station-name problem with its own message', () => {
    expect(lineNameProblem('Main\u202E', lines('Side'))).toBe('invalidChars');
    expect(lineNameProblemMessage('invalidChars', 'x')).toBe(
      'A name cannot contain control characters or invisible formatting characters.',
    );
    expect(stationNameProblem('5\u200B', ['4', '5'], 1)).toBe('invalidChars');
    expect(stationNameProblemMessage('invalidChars', 'Main', 'x')).toBe(
      'A name cannot contain control characters or invisible formatting characters.',
    );
    // Empty still wins over everything else.
    expect(stationNameProblem('  ', ['4'], 0)).toBe('empty');
  });
});
