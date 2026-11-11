import { describe, it, expect } from 'vitest';
import { displayStationName, formatShotId, formatStationId, formatStationIdWithName } from '../formatters';

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

  it('keeps a name of 0 (an mNemo section start)', () => {
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
