/**
 * sdk#7: SpeleoDB sends revision `authored_date` as an ISO-8601 UTC instant, and
 * before its ISO revision dates as New York wall time with no zone
 * (`YYYY/MM/DD HH:MM`). Either must become the right instant (DST-aware), be shown
 * in the viewer's zone with its GMT offset, and give correct relative times.
 */
import { describe, it, expect } from 'vitest';
import { formatRelativeTime } from '../../i18n/format';
import { formatRevisionTime, gmtOffsetLabel, parseRevisionDate } from '../speleoRevisionTime';

const iso = (raw: string) => parseRevisionDate(raw)?.toISOString() ?? null;

describe('parseRevisionDate', () => {
  it('reads the server wall time as New York time, standard and daylight', () => {
    expect(iso('2026/11/09 04:15')).toBe('2026-11-09T09:15:00.000Z'); // EST, UTC-5
    expect(iso('2026/07/03 06:00')).toBe('2026-07-03T10:00:00.000Z'); // EDT, UTC-4
    expect(iso('2026/01/01 00:00')).toBe('2026-01-01T05:00:00.000Z');
  });

  it('follows the US DST switch dates, not the viewer zone', () => {
    // 2026-03-08: 02:00 EST jumps to 03:00 EDT.
    expect(iso('2026/03/08 01:30')).toBe('2026-03-08T06:30:00.000Z');
    expect(iso('2026/03/08 03:30')).toBe('2026-03-08T07:30:00.000Z');
    // Between the US (Mar 8) and EU (Mar 29) switches New York is already on EDT.
    expect(iso('2026/03/20 12:00')).toBe('2026-03-20T16:00:00.000Z');
    // 2026-11-01: 01:00-02:00 happens twice; the earlier (EDT) instant is used.
    expect(iso('2026/11/01 01:30')).toBe('2026-11-01T05:30:00.000Z');
    expect(iso('2026/11/01 02:30')).toBe('2026-11-01T07:30:00.000Z');
  });

  it('keeps an explicit offset as sent and rejects what it cannot place', () => {
    expect(iso('2026-07-03T10:00:00+00:00')).toBe('2026-07-03T10:00:00.000Z');
    expect(iso('2026-07-03T12:00:00+02:00')).toBe('2026-07-03T10:00:00.000Z');
    expect(iso('2026-07-03T10:00:00Z')).toBe('2026-07-03T10:00:00.000Z');
    expect(iso('2026/07/03 06:00:30')).toBe('2026-07-03T10:00:30.000Z');
    expect(parseRevisionDate('yesterday')).toBeNull();
    expect(parseRevisionDate('')).toBeNull();
  });

  it('places no wall time that is not a real date and time, rather than rolling it over', () => {
    for (const raw of ['2026/02/30 10:00', '2026/13/45 25:99', '2026/11/10 24:00', '2026/11/10 10:60']) {
      expect(parseRevisionDate(raw)).toBeNull();
    }
    expect(iso('2028/02/29 10:00')).toBe('2028-02-29T15:00:00.000Z'); // leap day, EST
  });

  it('reads the UTC instant the backend hands on, milliseconds included', () => {
    expect(iso('2026-11-10T08:18:23.123Z')).toBe('2026-11-10T08:18:23.123Z');
    expect(iso('2026-11-10T08:18:23.000Z')).toBe('2026-11-10T08:18:23.000Z');
    // The same minute from a pre-ISO server (03:18 EST) is the same instant, to the minute.
    expect(iso('2026/11/10 03:18')).toBe('2026-11-10T08:18:00.000Z');
  });
});

describe('gmtOffsetLabel', () => {
  it('states the offset of the zone the time is shown in', () => {
    const winter = new Date('2026-01-15T12:00:00Z');
    const summer = new Date('2026-07-15T12:00:00Z');
    expect(gmtOffsetLabel(winter, 'Europe/Paris')).toBe('GMT+1');
    expect(gmtOffsetLabel(summer, 'Europe/Paris')).toBe('GMT+2');
    expect(gmtOffsetLabel(winter, 'America/New_York')).toBe('GMT-5');
    expect(gmtOffsetLabel(winter, 'Asia/Kolkata')).toBe('GMT+5:30');
    expect(gmtOffsetLabel(winter, 'America/St_Johns')).toBe('GMT-3:30');
    expect(gmtOffsetLabel(winter, 'UTC')).toBe('GMT+0');
  });

  it('defaults to the viewer zone', () => {
    const date = new Date('2026-01-15T12:00:00Z');
    const minutes = -date.getTimezoneOffset();
    const label = gmtOffsetLabel(date);
    expect(label.startsWith(minutes < 0 ? 'GMT-' : 'GMT+')).toBe(true);
  });
});

describe('formatRevisionTime', () => {
  it('shows a just-made commit at the viewer local time, with the offset', () => {
    // 04:15 in New York is 10:15 in Paris (CET) on 9 November.
    const date = parseRevisionDate('2026/11/09 04:15');
    expect(date).not.toBeNull();
    const text = formatRevisionTime(date as Date, 'Europe/Paris');
    expect(text).toContain('Nov 9, 2026');
    expect(text).toContain('10:15');
    expect(text.endsWith(' GMT+1')).toBe(true);
    // March 20: New York is on EDT, Paris still on CET.
    const spring = formatRevisionTime(parseRevisionDate('2026/03/20 12:00') as Date, 'Europe/Paris');
    expect(spring).toContain('05:00');
    expect(spring.endsWith(' GMT+1')).toBe(true);
  });

  it('shows a UTC instant in the zone and offset in force at that instant', () => {
    // Paris: CET (GMT+1) in winter, CEST (GMT+2) in summer, switching 2026-03-29 01:00 UTC.
    const winter = formatRevisionTime(parseRevisionDate('2026-03-29T00:59:00.000Z') as Date, 'Europe/Paris');
    expect(winter).toContain('Mar 29, 2026');
    expect(winter).toContain('01:59');
    expect(winter.endsWith(' GMT+1')).toBe(true);
    const summer = formatRevisionTime(parseRevisionDate('2026-03-29T01:00:00.000Z') as Date, 'Europe/Paris');
    expect(summer).toContain('03:00');
    expect(summer.endsWith(' GMT+2')).toBe(true);
    // A half-hour zone, and a viewer west of New York.
    const kolkata = formatRevisionTime(parseRevisionDate('2026-11-10T08:18:23.123Z') as Date, 'Asia/Kolkata');
    expect(kolkata).toContain('01:48');
    expect(kolkata.endsWith(' GMT+5:30')).toBe(true);
    const la = formatRevisionTime(parseRevisionDate('2026-11-10T08:18:23.123Z') as Date, 'America/Los_Angeles');
    expect(la).toContain('Nov 10, 2026');
    expect(la).toContain('12:18');
    expect(la.endsWith(' GMT-8')).toBe(true);
  });

  it('gives a correct relative time for a just-made commit', () => {
    // The bug: parsed as viewer-local, a commit made a minute ago read hours off
    // (in the future for a viewer west of New York). As an instant it is "now".
    const date = parseRevisionDate('2026/11/09 04:15') as Date;
    const now = new Date('2026-11-09T09:30:00Z');
    expect(formatRelativeTime(date, 'en', now)).toBe('15 minutes ago');
  });
});
