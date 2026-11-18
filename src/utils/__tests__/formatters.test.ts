import { describe, it, expect } from 'vitest';
import { SHOT_ARROW, displayStationName, formatShotLabel, formatStationLabel } from '../formatters';

describe('formatStationLabel', () => {
  it('is the station name alone without line context', () => {
    expect(formatStationLabel('17')).toBe('17');
    expect(formatStationLabel('4a', null)).toBe('4a');
    expect(formatStationLabel('0', '')).toBe('0');
  });

  it('prefixes the line name with a slash', () => {
    expect(formatStationLabel('17', 'Main Line')).toBe('Main Line / 17');
  });

  it('never shows a positional line:index handle', () => {
    expect(formatStationLabel('3', 'Side')).not.toMatch(/\d+:\d+/);
  });

  it('cannot reorder or disguise the label with bidi or control characters', () => {
    expect(formatStationLabel('\u202e9\u202c', 'Main\u2067')).toBe('Main / 9');
    expect(formatStationLabel('1\n2')).toBe('12');
  });
});

describe('formatShotLabel', () => {
  it('joins the two station names with the arrow', () => {
    expect(SHOT_ARROW).toBe('\u2192');
    expect(formatShotLabel('4', '5')).toBe('4 \u2192 5');
  });

  it('prefixes the line name with a colon', () => {
    expect(formatShotLabel('4', '5', 'Main Line')).toBe('Main Line: 4 \u2192 5');
  });

  it('keeps lettered and prefixed names', () => {
    expect(formatShotLabel('A3', '2a', 'Side')).toBe('Side: A3 \u2192 2a');
  });

  it('sanitizes both station names and the line name', () => {
    expect(formatShotLabel('\u202e4', '5\u0007', '\u200fLine')).toBe('Line: 4 \u2192 5');
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
