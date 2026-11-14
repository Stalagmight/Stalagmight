import { describe, it, expect } from 'vitest';
import {
  parseLocaleNumber,
  getLocaleSeparators,
  serializeCell,
  serializeRangeToTSV,
  parseTSV,
} from '../gridClipboard';

describe('parseLocaleNumber', () => {
  it('parses plain en-US numbers', () => {
    expect(parseLocaleNumber('1.5', 'en-US')).toBe(1.5);
    expect(parseLocaleNumber('1,234.5', 'en-US')).toBe(1234.5);
    expect(parseLocaleNumber('1,234', 'en-US')).toBe(1234);
    expect(parseLocaleNumber('-42', 'en-US')).toBe(-42);
  });

  it('parses comma-decimal locales (the parseFloat bug)', () => {
    // The bug this fixes: parseFloat("1,5") === 1.
    expect(parseLocaleNumber('1,5', 'de-DE')).toBe(1.5);
    expect(parseLocaleNumber('1234,5', 'de-DE')).toBe(1234.5);
    expect(parseLocaleNumber('1.234,5', 'de-DE')).toBe(1234.5);
    expect(parseLocaleNumber('-3,5', 'de-DE')).toBe(-3.5);
  });

  it('strips group separators only for valid groups of three', () => {
    expect(parseLocaleNumber('1.234', 'de-DE')).toBe(1234);
    expect(parseLocaleNumber('1.234.567,89', 'de-DE')).toBe(1234567.89);
  });

  it('dot-fallback: a plain "1.5" typed in a comma-decimal locale parses as 1.5', () => {
    // The dot is NOT grouping here - "1.5" must never become 15.
    expect(parseLocaleNumber('1.5', 'de-DE')).toBe(1.5);
    // Too many fraction digits for a group → also the dot-fallback.
    expect(parseLocaleNumber('1.2345', 'de-DE')).toBe(1.2345);
  });

  it('rejects comma in en-US unless it is valid grouping (never 15)', () => {
    expect(parseLocaleNumber('1,5', 'en-US')).toBeUndefined();
    expect(parseLocaleNumber('12,34', 'en-US')).toBeUndefined();
  });

  it('accepts space-like group separators (fr-FR)', () => {
    expect(parseLocaleNumber('1\u202F234,5', 'fr-FR')).toBe(1234.5); // narrow NBSP
    expect(parseLocaleNumber('1\u00A0234,5', 'fr-FR')).toBe(1234.5); // NBSP
    expect(parseLocaleNumber('1 234,5', 'fr-FR')).toBe(1234.5); // regular space
    expect(parseLocaleNumber('1,5', 'fr-FR')).toBe(1.5);
  });

  it('returns undefined for empty / non-numeric (callers must skip, never write 0)', () => {
    expect(parseLocaleNumber('', 'en-US')).toBeUndefined();
    expect(parseLocaleNumber('   ', 'en-US')).toBeUndefined();
    expect(parseLocaleNumber('abc', 'en-US')).toBeUndefined();
    expect(parseLocaleNumber('', 'de-DE')).toBeUndefined();
    expect(parseLocaleNumber('abc', 'de-DE')).toBeUndefined();
  });

  it('parses back a negative Intl formatted with a Unicode minus (sv-SE)', () => {
    const shown = new Intl.NumberFormat('sv-SE').format(-1234.5);
    expect(shown.charCodeAt(0)).toBe(0x2212);
    expect(parseLocaleNumber(shown, 'sv-SE')).toBe(-1234.5);
    expect(parseLocaleNumber('\u22123,5', 'sv-SE')).toBe(-3.5);
    expect(parseLocaleNumber('\uFE632,25', 'sv-SE')).toBe(-2.25);
    expect(parseLocaleNumber('\uFF0D7', 'en-US')).toBe(-7);
    expect(parseLocaleNumber('\u22121.5', 'en-US')).toBe(-1.5);
  });

  it('trims surrounding whitespace', () => {
    expect(parseLocaleNumber('  2.5 ', 'en-US')).toBe(2.5);
  });
});

describe('getLocaleSeparators', () => {
  it('reports en separators', () => {
    expect(getLocaleSeparators('en-US')).toEqual({ decimal: '.', group: ',' });
  });
  it('falls back to en defaults on a bogus locale', () => {
    expect(getLocaleSeparators('not-a-locale')).toEqual({ decimal: '.', group: ',' });
  });
});

describe('serializeCell', () => {
  it('leaves plain values unquoted', () => {
    expect(serializeCell('hello')).toBe('hello');
    expect(serializeCell('1.5')).toBe('1.5');
  });
  it('quotes and escapes tabs, newlines, and quotes', () => {
    expect(serializeCell('a\tb')).toBe('"a\tb"');
    expect(serializeCell('line1\nline2')).toBe('"line1\nline2"');
    expect(serializeCell('say "hi"')).toBe('"say ""hi"""');
  });
});

describe('serializeRangeToTSV / parseTSV round-trip', () => {
  it('serializes a matrix to TSV', () => {
    expect(serializeRangeToTSV([['a', 'b'], ['c', 'd']])).toBe('a\tb\nc\td');
  });

  it('parses simple TSV', () => {
    expect(parseTSV('a\tb\nc\td')).toEqual([['a', 'b'], ['c', 'd']]);
  });

  it('drops a single trailing newline (no phantom empty row)', () => {
    expect(parseTSV('a\tb\n')).toEqual([['a', 'b']]);
  });

  it('handles CRLF row separators (Excel on Windows)', () => {
    expect(parseTSV('a\tb\r\nc\td')).toEqual([['a', 'b'], ['c', 'd']]);
  });

  it('parses quoted fields with embedded tabs, newlines, and doubled quotes', () => {
    const tsv = '"a\tb"\t"multi\nline"\n"say ""hi"""\tplain';
    expect(parseTSV(tsv)).toEqual([
      ['a\tb', 'multi\nline'],
      ['say "hi"', 'plain'],
    ]);
  });

  it('round-trips a comment with a newline', () => {
    const matrix = [['note one\nsecond line', '42']];
    expect(parseTSV(serializeRangeToTSV(matrix))).toEqual(matrix);
  });

  it('parses a single cell', () => {
    expect(parseTSV('solo')).toEqual([['solo']]);
  });

  it('keeps quotes that appear mid-field in unquoted text (inch marks)', () => {
    // A quote only opens a quoted field at the field start; elsewhere it is a
    // literal, so a plainly-pasted comment keeps its inch marks / quotes.
    expect(parseTSV('say "hi"\tplain')).toEqual([['say "hi"', 'plain']]);
    expect(parseTSV('6" pipe')).toEqual([['6" pipe']]);
    expect(parseTSV('a"b\tc"d')).toEqual([['a"b', 'c"d']]);
  });
});
