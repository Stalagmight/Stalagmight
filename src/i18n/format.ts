import i18n from './index';

type RelativeUnit = 'second' | 'minute' | 'hour' | 'day' | 'week' | 'month' | 'year';

// Module-level format locale overrides (default: follow language setting)
let numberFormatLocale = 'locale';
let dateFormatLocale = 'locale';

// Cache for Intl.NumberFormat instances - construction is expensive (~0.05ms each in WebKit).
// With hundreds of lines rendering per frame, this adds up to 20-40ms.
const numberFormatCache = new Map<string, Intl.NumberFormat>();

function getCachedNumberFormat(locale: string, options?: Intl.NumberFormatOptions): Intl.NumberFormat {
  const key = options ? `${locale}|${JSON.stringify(options)}` : locale;
  let fmt = numberFormatCache.get(key);
  if (!fmt) {
    fmt = new Intl.NumberFormat(locale, options);
    numberFormatCache.set(key, fmt);
    // Evict if cache grows too large (unlikely in practice)
    if (numberFormatCache.size > 100) {
      const first = numberFormatCache.keys().next().value;
      if (first !== undefined) numberFormatCache.delete(first);
    }
  }
  return fmt;
}

export function setNumberFormatLocale(locale: string): void {
  numberFormatLocale = locale;
}

export function setDateFormatLocale(locale: string): void {
  dateFormatLocale = locale;
}

export function getCurrentLocale(explicitLocale?: string): string {
  if (explicitLocale && explicitLocale !== 'system') {
    return explicitLocale;
  }
  return i18n.resolvedLanguage || i18n.language || 'en';
}

/** Resolve the effective locale for number formatting. */
function getNumberLocale(explicitLocale?: string): string {
  if (explicitLocale) return getCurrentLocale(explicitLocale);
  if (numberFormatLocale !== 'locale') return numberFormatLocale;
  return getCurrentLocale();
}

/** The effective BCP-47 locale used for number formatting - always a valid Intl
 *  tag (never the sentinel 'locale'/'system'). Pass this to consumers (e.g. the
 *  data-table grid) that must PARSE numbers with the same separators they're
 *  displayed in. */
export function getNumberFormatLocale(): string {
  return getNumberLocale();
}

/** Resolve the effective locale for date formatting. */
function getDateLocale(explicitLocale?: string): string {
  if (explicitLocale) return getCurrentLocale(explicitLocale);
  if (dateFormatLocale !== 'locale') return dateFormatLocale;
  return getCurrentLocale();
}

export function formatNumber(
  value: number,
  locale?: string,
  options?: Intl.NumberFormatOptions
): string {
  const resolvedLocale = getNumberLocale(locale);
  return getCachedNumberFormat(resolvedLocale, options).format(value);
}

/**
 * `value` with exactly `precision` decimals. A value that rounds to zero is
 * written without a sign: "-0.00 m" (a Y of -0.0004 m in Station details)
 * reads as a negative reading that is not there (RS-FX6, P3-19).
 */
export function formatFixed(
  value: number,
  locale?: string,
  precision: number = 1
): string {
  const resolvedLocale = getNumberLocale(locale);
  const clampedPrecision = Math.max(0, Math.min(6, Math.floor(precision)));
  const roundsToZero = Math.abs(value) * 10 ** clampedPrecision < 0.5;
  return getCachedNumberFormat(resolvedLocale, {
    minimumFractionDigits: clampedPrecision,
    maximumFractionDigits: clampedPrecision,
  }).format(roundsToZero ? 0 : value);
}

export function formatPercent(
  value: number,
  locale?: string,
  precision: number = 1
): string {
  const resolvedLocale = getNumberLocale(locale);
  const clampedPrecision = Math.max(0, Math.min(6, Math.floor(precision)));
  const formatted = getCachedNumberFormat(resolvedLocale, {
    minimumFractionDigits: clampedPrecision,
    maximumFractionDigits: clampedPrecision,
  }).format(value);
  return `${formatted}%`;
}

export function formatDate(
  date: Date,
  locale?: string,
  options?: Intl.DateTimeFormatOptions
): string {
  const resolvedLocale = getDateLocale(locale);
  return new Intl.DateTimeFormat(resolvedLocale, options).format(date);
}

export function formatRelativeTime(
  date: Date,
  locale?: string,
  now: Date = new Date()
): string {
  const resolvedLocale = getCurrentLocale(locale);
  const diffMs = date.getTime() - now.getTime();
  const diffSeconds = Math.round(diffMs / 1000);
  const absSeconds = Math.abs(diffSeconds);

  let value = diffSeconds;
  let unit: RelativeUnit = 'second';

  if (absSeconds >= 60 && absSeconds < 3600) {
    value = Math.round(diffSeconds / 60);
    unit = 'minute';
  } else if (absSeconds >= 3600 && absSeconds < 86400) {
    value = Math.round(diffSeconds / 3600);
    unit = 'hour';
  } else if (absSeconds >= 86400 && absSeconds < 604800) {
    value = Math.round(diffSeconds / 86400);
    unit = 'day';
  } else if (absSeconds >= 604800 && absSeconds < 2629800) {
    value = Math.round(diffSeconds / 604800);
    unit = 'week';
  } else if (absSeconds >= 2629800 && absSeconds < 31557600) {
    value = Math.round(diffSeconds / 2629800);
    unit = 'month';
  } else if (absSeconds >= 31557600) {
    value = Math.round(diffSeconds / 31557600);
    unit = 'year';
  }

  const rtf = new Intl.RelativeTimeFormat(resolvedLocale, { numeric: 'auto' });
  return rtf.format(value, unit);
}
