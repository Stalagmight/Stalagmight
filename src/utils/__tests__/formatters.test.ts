import { describe, it, expect } from 'vitest';
import {
  MAX_STATION_NAME_CHARS,
  displayStationName,
  formatShotId,
  formatStationId,
  formatStationIdWithName,
  sanitizeStationName,
} from '../formatters';

describe('formatShotId', () => {
  it('formats section and shot position', () => {
    expect(formatShotId(5, 3)).toBe('5:3');
  });

  it('formats first shot in first section', () => {
    expect(formatShotId(1, 1)).toBe('1:1');
  });

  it('formats larger numbers', () => {
    expect(formatShotId(123, 456)).toBe('123:456');
  });

  it('handles single digit values', () => {
    expect(formatShotId(1, 2)).toBe('1:2');
    expect(formatShotId(9, 9)).toBe('9:9');
  });

  it('formats zero values (edge case, though typically 1-indexed)', () => {
    expect(formatShotId(0, 0)).toBe('0:0');
  });
});

describe('formatStationId', () => {
  it('formats section and station index', () => {
    expect(formatStationId(5, 3)).toBe('5:3');
  });

  it('formats first station in first section (0-indexed)', () => {
    expect(formatStationId(1, 0)).toBe('1:0');
  });

  it('formats larger numbers', () => {
    expect(formatStationId(123, 456)).toBe('123:456');
  });

  it('handles single digit values', () => {
    expect(formatStationId(1, 2)).toBe('1:2');
    expect(formatStationId(9, 9)).toBe('9:9');
  });

  it('handles zero values', () => {
    expect(formatStationId(0, 0)).toBe('0:0');
  });
});

describe('formatStationIdWithName', () => {
  it('appends the name beside the positional id', () => {
    expect(formatStationIdWithName(5, 3, '17')).toBe('5:3 · 17');
  });

  it('is the plain id for a station without a name', () => {
    expect(formatStationIdWithName(5, 3)).toBe('5:3');
    expect(formatStationIdWithName(5, 3, null)).toBe('5:3');
    expect(formatStationIdWithName(5, 3, '')).toBe('5:3');
  });

  it('keeps a name of 0 (an MNemo section start)', () => {
    expect(formatStationIdWithName(2, 0, '0')).toBe('2:0 · 0');
  });

  it('cannot reorder or disguise the id with bidi or control characters', () => {
    // A right-to-left override followed by a fake id, from a hostile file.
    expect(formatStationIdWithName(5, 3, '\u202e9:9\u202c')).toBe('5:3 · 9:9');
    expect(formatStationIdWithName(5, 3, '1\n2\u2067\u200f')).toBe('5:3 · 12');
    // A name of nothing but control characters shows as no name.
    expect(formatStationIdWithName(5, 3, '\u202e\u0007')).toBe('5:3');
  });
});

describe('displayStationName', () => {
  it('passes ordinary names, including non-Latin text, unchanged', () => {
    expect(displayStationName('17')).toBe('17');
    expect(displayStationName('A12 · east')).toBe('A12 · east');
    expect(displayStationName('נקודה 3')).toBe('נקודה 3');
  });

  it('is empty for no name', () => {
    expect(displayStationName(undefined)).toBe('');
    expect(displayStationName(null)).toBe('');
  });
});

// The same cases as the Rust tests of `sanitize_station_name`
// (src-tauri/crates/models/src/station_name.rs), which this mirrors.
describe('sanitizeStationName', () => {
  it('keeps an ordinary name and trims it', () => {
    expect(sanitizeStationName('17')).toBe('17');
    expect(sanitizeStationName('  A12 east \t')).toBe('A12 east');
    expect(sanitizeStationName('\u05e0\u05e7\u05d5\u05d3\u05d4 3')).toBe('\u05e0\u05e7\u05d5\u05d3\u05d4 3');
  });

  it('empty and whitespace-only mean remove', () => {
    expect(sanitizeStationName('')).toBeNull();
    expect(sanitizeStationName('   \t\n ')).toBeNull();
    expect(sanitizeStationName('\u202e\u200f\u0007')).toBeNull();
  });

  it('strips controls and bidi overrides; a newline or tab becomes a space', () => {
    expect(sanitizeStationName('A\u202eB\u2066C\u0000D\ufffe')).toBe('ABCD');
    expect(sanitizeStationName('Sump\nentrance')).toBe('Sump entrance');
    expect(sanitizeStationName('a\u0085b')).toBe('a b'); // NEL: a whitespace control
    expect(sanitizeStationName('a\u009bb')).toBe('ab'); // a C1 control that is not whitespace
  });

  it('trims what Rust trims, and only that', () => {
    expect(sanitizeStationName('\u3000\u00a0x\u2028')).toBe('x');
    // U+FEFF is not Unicode White_Space: Rust keeps it, JS trim() would not.
    expect(sanitizeStationName('\ufeffx')).toBe('\ufeffx');
  });

  it('clamps to the char limit on a code point boundary', () => {
    const long = '\u00e9'.repeat(MAX_STATION_NAME_CHARS + 50);
    expect(Array.from(sanitizeStationName(long)!)).toHaveLength(MAX_STATION_NAME_CHARS);
    const astral = '\u{1f600}'.repeat(MAX_STATION_NAME_CHARS + 1);
    expect(Array.from(sanitizeStationName(astral)!)).toHaveLength(MAX_STATION_NAME_CHARS);
    // A cut that lands after a space does not leave trailing whitespace.
    const spaced = `${'x'.repeat(MAX_STATION_NAME_CHARS - 1)} tail`;
    expect(sanitizeStationName(spaced)).toBe('x'.repeat(MAX_STATION_NAME_CHARS - 1));
  });

  it('is idempotent', () => {
    for (const raw of ['  a\tb  ', 'A\u202eB', 'z'.repeat(400), '17', ' x \n y ']) {
      const once = sanitizeStationName(raw)!;
      expect(sanitizeStationName(once)).toBe(once);
    }
  });
});
