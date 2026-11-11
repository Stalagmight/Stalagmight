/**
 * sdk#7: SpeleoDB revision timestamps.
 *
 * SpeleoDB sends a commit's `authored_date` as an ISO-8601 UTC instant (since its
 * a679010c6, 2026-11-10), which the backend hands on as UTC with milliseconds,
 * `2026-11-10T08:18:23.123Z`.
 *
 * An older server sends wall-clock time in ITS zone, `YYYY/MM/DD HH:MM`, with no
 * offset: the instance runs with `TIME_ZONE = "America/New_York"` and its revision
 * serializer formatted the date without one. `new Date()` reads such a string as
 * the VIEWER's local time, which put every revision up to a day off (a just-made
 * commit read "in 4 hours" for a viewer west of New York). The backend passes that
 * string through unchanged, and it is converted here, in the frontend, rather than
 * in Rust: the webview ships the full IANA time-zone database through `Intl`, so
 * the New York DST rules (past and future) come for free, while the backend has no
 * tz data and would need a new crate to get it.
 */
import { formatDate } from '../i18n/format';

/** The zone a SpeleoDB instance renders zone-less revision dates in. */
export const SPELEODB_SERVER_TIME_ZONE = 'America/New_York';

/** `2026/11/09 04:15`, optionally with seconds (or `-` / `T` separators). */
const WALL_TIME = /^(\d{4})[/-](\d{1,2})[/-](\d{1,2})[ T](\d{1,2}):(\d{2})(?::(\d{2}))?$/;
/** An ISO-8601 instant that names its own offset. */
const ZONED = /(?:Z|[+-]\d{2}:?\d{2})$/i;

const partsFormatters = new Map<string, Intl.DateTimeFormat>();

/** `timeZone`'s offset from UTC at `instant`, in milliseconds (east positive). */
function zoneOffsetMs(instant: number, timeZone: string): number {
  let fmt = partsFormatters.get(timeZone);
  if (!fmt) {
    fmt = new Intl.DateTimeFormat('en-US', {
      timeZone,
      hourCycle: 'h23',
      year: 'numeric',
      month: 'numeric',
      day: 'numeric',
      hour: 'numeric',
      minute: 'numeric',
      second: 'numeric',
    });
    partsFormatters.set(timeZone, fmt);
  }
  const part: Partial<Record<Intl.DateTimeFormatPartTypes, number>> = {};
  for (const p of fmt.formatToParts(new Date(instant))) {
    if (p.type !== 'literal') part[p.type] = Number(p.value);
  }
  const get = (type: Intl.DateTimeFormatPartTypes) => part[type] ?? 0;
  // Some engines still answer 24 for midnight despite `h23`.
  const hour = get('hour') % 24;
  const wall = Date.UTC(get('year'), get('month') - 1, get('day'), hour, get('minute'), get('second'));
  return wall - Math.floor(instant / 1000) * 1000;
}

/**
 * The instant a wall-clock time in `timeZone` denotes. Two passes settle the DST
 * edge: the offset is re-read at the first estimate. In the repeated autumn hour
 * the earlier (summer-time) instant wins - the server string carries nothing that
 * could tell the two apart.
 */
function wallTimeToInstant(wallAsUtc: number, timeZone: string): number {
  const estimate = wallAsUtc - zoneOffsetMs(wallAsUtc, timeZone);
  return wallAsUtc - zoneOffsetMs(estimate, timeZone);
}

/**
 * Parse a revision `authored_date` into the instant it denotes, or `null` when it
 * is neither the server's zone-less wall time nor an ISO-8601 string with an
 * explicit offset.
 */
export function parseRevisionDate(raw: string, serverTimeZone = SPELEODB_SERVER_TIME_ZONE): Date | null {
  const text = raw.trim();
  const wall = WALL_TIME.exec(text);
  if (wall) {
    const [, y, mo, d, h, mi, s] = wall;
    const wallAsUtc = Date.UTC(Number(y), Number(mo) - 1, Number(d), Number(h), Number(mi), Number(s ?? 0));
    // Not a real date and time (`2026/02/30`, `25:99`): `Date.UTC` would roll it over
    // into another day, so place nothing rather than the wrong instant.
    const check = new Date(wallAsUtc);
    if (
      check.getUTCFullYear() !== Number(y) || check.getUTCMonth() !== Number(mo) - 1 || check.getUTCDate() !== Number(d) ||
      check.getUTCHours() !== Number(h) || check.getUTCMinutes() !== Number(mi) || check.getUTCSeconds() !== Number(s ?? 0)
    ) {
      return null;
    }
    const instant = new Date(wallTimeToInstant(wallAsUtc, serverTimeZone));
    return Number.isNaN(instant.getTime()) ? null : instant;
  }
  if (!ZONED.test(text)) return null;
  const instant = new Date(text);
  return Number.isNaN(instant.getTime()) ? null : instant;
}

/**
 * `GMT+1`, `GMT-5`, `GMT+5:30`, `GMT+0`: the offset of the zone `date` is shown in.
 * `timeZone` defaults to the viewer's own zone.
 */
export function gmtOffsetLabel(date: Date, timeZone?: string): string {
  const minutes = timeZone
    ? Math.round(zoneOffsetMs(date.getTime(), timeZone) / 60_000)
    : -date.getTimezoneOffset();
  const sign = minutes < 0 ? '-' : '+';
  const abs = Math.abs(minutes);
  const rest = abs % 60;
  return `GMT${sign}${Math.floor(abs / 60)}${rest ? `:${String(rest).padStart(2, '0')}` : ''}`;
}

/**
 * A revision instant as the viewer's local date and time with its GMT offset
 * stated, e.g. `Nov 9, 2026, 10:15 AM GMT+1`. `timeZone` is for tests; the app
 * always shows the viewer's own zone.
 */
export function formatRevisionTime(date: Date, timeZone?: string): string {
  const local = formatDate(date, undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
    ...(timeZone ? { timeZone } : {}),
  });
  return `${local} ${gmtOffsetLabel(date, timeZone)}`;
}
