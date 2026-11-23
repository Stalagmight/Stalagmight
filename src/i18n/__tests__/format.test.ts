import { describe, it, expect, beforeEach } from 'vitest';
import {
  formatFixed,
  formatPercent,
  formatNumber,
  formatDate,
  formatRelativeTime,
  setNumberFormatLocale,
  setDateFormatLocale,
  getCurrentLocale,
} from '../format';

// Reset module-level locale overrides before each test
beforeEach(() => {
  setNumberFormatLocale('locale');
  setDateFormatLocale('locale');
});

// ---------------------------------------------------------------------------
// getCurrentLocale
// ---------------------------------------------------------------------------

describe('getCurrentLocale', () => {
  it('returns the explicit locale when provided and not "system"', () => {
    expect(getCurrentLocale('de')).toBe('de');
    expect(getCurrentLocale('fr-FR')).toBe('fr-FR');
  });

  it('falls back to i18n language when "system" is passed', () => {
    const result = getCurrentLocale('system');
    // Should resolve to something, typically 'en' in test environment
    expect(typeof result).toBe('string');
    expect(result.length).toBeGreaterThan(0);
  });

  it('falls back to i18n language when no explicit locale', () => {
    const result = getCurrentLocale();
    expect(typeof result).toBe('string');
    expect(result.length).toBeGreaterThan(0);
  });
});

// ---------------------------------------------------------------------------
// formatFixed
// ---------------------------------------------------------------------------

describe('formatFixed', () => {
  it('formats with default precision (1 decimal)', () => {
    expect(formatFixed(12.345, 'en')).toBe('12.3');
  });

  it('formats with precision 0', () => {
    expect(formatFixed(12.9, 'en', 0)).toBe('13');
  });

  it('formats with precision 2', () => {
    expect(formatFixed(3.14159, 'en', 2)).toBe('3.14');
  });

  it('pads with trailing zeros', () => {
    expect(formatFixed(5, 'en', 3)).toBe('5.000');
  });

  // RS-FX6, P3-19: Station details showed "Y: -0.00 m" for a station a hair
  // south of the origin. A value that rounds to zero has no sign; one that
  // rounds away from it keeps its own. Injection: format `value` as is.
  it('writes a value that rounds to zero without a sign', () => {
    expect(formatFixed(-0.0004, 'en', 2)).toBe('0.00');
    expect(formatFixed(-0, 'en', 1)).toBe('0.0');
    expect(formatFixed(-0.04, 'en', 1)).toBe('0.0');
    expect(formatFixed(-0.06, 'en', 1)).toBe('-0.1');
    expect(formatFixed(-12.3, 'en', 1)).toBe('-12.3');
  });

  it('clamps precision to minimum 0', () => {
    expect(formatFixed(12.5, 'en', -1)).toBe('13');
  });

  it('clamps precision to maximum 6', () => {
    // precision 10 should be clamped to 6
    const result = formatFixed(1.123456789, 'en', 10);
    expect(result).toBe('1.123457');
  });

  it('handles floor for non-integer precision', () => {
    // precision 2.9 should be floored to 2
    expect(formatFixed(3.14159, 'en', 2.9)).toBe('3.14');
  });

  it('handles negative numbers', () => {
    expect(formatFixed(-42.567, 'en', 2)).toBe('-42.57');
  });

  it('handles zero', () => {
    expect(formatFixed(0, 'en', 2)).toBe('0.00');
  });
});

// ---------------------------------------------------------------------------
// formatPercent
// ---------------------------------------------------------------------------

describe('formatPercent', () => {
  it('appends % suffix', () => {
    expect(formatPercent(12.34, 'en', 1)).toBe('12.3%');
  });

  it('uses default precision of 1', () => {
    expect(formatPercent(99.999, 'en')).toBe('100.0%');
  });

  it('handles zero', () => {
    expect(formatPercent(0, 'en', 2)).toBe('0.00%');
  });

  it('clamps precision same as formatFixed', () => {
    expect(formatPercent(5.5, 'en', -5)).toBe('6%');
    expect(formatPercent(5.123456789, 'en', 10)).toBe('5.123457%');
  });
});

// ---------------------------------------------------------------------------
// formatNumber
// ---------------------------------------------------------------------------

describe('formatNumber', () => {
  it('formats a basic number', () => {
    const result = formatNumber(1234.5, 'en');
    expect(result).toBe('1,234.5');
  });

  it('passes options through', () => {
    const result = formatNumber(0.75, 'en', { style: 'percent' });
    expect(result).toBe('75%');
  });
});

// ---------------------------------------------------------------------------
// formatDate
// ---------------------------------------------------------------------------

describe('formatDate', () => {
  it('formats a date', () => {
    const date = new Date(2025, 0, 15); // Jan 15, 2025
    const result = formatDate(date, 'en');
    expect(result).toContain('2025');
    expect(result).toContain('15');
  });

  it('passes options through', () => {
    const date = new Date(2025, 5, 1); // June 1, 2025
    const result = formatDate(date, 'en', { year: 'numeric', month: 'long', day: 'numeric' });
    expect(result).toContain('June');
    expect(result).toContain('2025');
  });
});

// ---------------------------------------------------------------------------
// formatRelativeTime
// ---------------------------------------------------------------------------

describe('formatRelativeTime', () => {
  const now = new Date('2025-06-15T12:00:00Z');

  it('formats seconds ago', () => {
    const date = new Date('2025-06-15T11:59:30Z'); // 30 seconds before
    const result = formatRelativeTime(date, 'en', now);
    expect(result).toContain('30');
    expect(result).toContain('second');
  });

  it('formats minutes ago', () => {
    const date = new Date('2025-06-15T11:55:00Z'); // 5 minutes before
    const result = formatRelativeTime(date, 'en', now);
    expect(result).toContain('5');
    expect(result).toContain('minute');
  });

  it('formats hours ago', () => {
    const date = new Date('2025-06-15T09:00:00Z'); // 3 hours before
    const result = formatRelativeTime(date, 'en', now);
    expect(result).toContain('3');
    expect(result).toContain('hour');
  });

  it('formats days ago', () => {
    const date = new Date('2025-06-12T12:00:00Z'); // 3 days before
    const result = formatRelativeTime(date, 'en', now);
    expect(result).toContain('3');
    expect(result).toContain('day');
  });

  it('formats weeks ago', () => {
    const date = new Date('2025-05-25T12:00:00Z'); // ~3 weeks before
    const result = formatRelativeTime(date, 'en', now);
    expect(result).toContain('week');
  });

  it('formats months ago', () => {
    const date = new Date('2025-03-15T12:00:00Z'); // ~3 months before
    const result = formatRelativeTime(date, 'en', now);
    expect(result).toContain('month');
  });

  it('formats years ago', () => {
    const date = new Date('2023-06-15T12:00:00Z'); // 2 years before
    const result = formatRelativeTime(date, 'en', now);
    expect(result).toContain('2');
    expect(result).toContain('year');
  });

  it('formats future times (positive diff)', () => {
    const date = new Date('2025-06-15T15:00:00Z'); // 3 hours in the future
    const result = formatRelativeTime(date, 'en', now);
    expect(result).toContain('3');
    expect(result).toContain('hour');
    // Should say "in 3 hours" not "3 hours ago"
    expect(result).toContain('in');
  });

  it('handles just now (0 seconds)', () => {
    const result = formatRelativeTime(now, 'en', now);
    // 0 seconds relative - Intl.RelativeTimeFormat with numeric:'auto' may say "now"
    expect(typeof result).toBe('string');
  });
});

// ---------------------------------------------------------------------------
// Locale override functions
// ---------------------------------------------------------------------------

describe('locale overrides', () => {
  it('setNumberFormatLocale affects formatting', () => {
    setNumberFormatLocale('de');
    const result = formatFixed(1234.5, undefined, 1);
    // German uses comma as decimal separator
    expect(result).toContain(',');
  });

  it('setDateFormatLocale affects date formatting', () => {
    setDateFormatLocale('de');
    const date = new Date(2025, 0, 15);
    const result = formatDate(date);
    // Should be formatted in German locale
    expect(typeof result).toBe('string');
  });

  it('explicit locale parameter overrides module-level setting', () => {
    setNumberFormatLocale('de');
    // Explicit 'en' should override the module-level 'de'
    const result = formatFixed(1234.5, 'en', 1);
    expect(result).toBe('1,234.5');
  });
});
