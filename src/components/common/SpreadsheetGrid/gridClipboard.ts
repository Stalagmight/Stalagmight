/**
 * Clipboard + locale helpers for SpreadsheetGrid. Pure and unit-tested.
 *
 * Clean-room: standard TSV (tab-separated columns, newline-separated rows) with
 * RFC-4180-style quoting for fields containing tabs/newlines/quotes - the same
 * shape Excel and Google Sheets put on the clipboard's `text/plain`. No grid
 * library source referenced.
 */

/** Decimal + group separators for a locale, derived from Intl. */
export function getLocaleSeparators(locale: string): { decimal: string; group: string } {
  let decimal = '.';
  let group = ',';
  try {
    for (const part of new Intl.NumberFormat(locale).formatToParts(11111.1)) {
      if (part.type === 'decimal') decimal = part.value;
      else if (part.type === 'group') group = part.value;
    }
  } catch {
    /* unknown locale → keep en defaults */
  }
  return { decimal, group };
}

const SPACE_RE = /[\s  ]/g; // incl. NBSP / narrow NBSP used as group sep

const SPACE_CLASS = '[\\s\\u00A0\\u202F]'; // same set, for building patterns

/** U+2212 MINUS SIGN, U+FE63 SMALL HYPHEN-MINUS, U+FF0D FULLWIDTH HYPHEN-MINUS. */
const UNICODE_MINUS_RE = /[\u2212\uFE63\uFF0D]/g;

const escapeRegExp = (s: string): string => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

/**
 * Parse a typed/pasted string into a number using the active locale's
 * separators. Returns `undefined` when the input is empty or not a finite
 * number (callers MUST treat `undefined` as "skip", never as 0).
 *
 * This replaces the bare `parseFloat` in the edit paths, which silently
 * corrupts comma-decimal locales (`parseFloat("1,5") === 1`).
 *
 * The group separator is only stripped when the string is a genuinely grouped
 * number (groups of three digits, e.g. "1.234,5" in de-DE) - blindly stripping
 * it would turn a pasted dot-decimal "1.5" into 15. Otherwise a single locale
 * decimal separator is swapped to '.', and finally a plain dot-decimal
 * fallback covers "1.5" typed in a comma-decimal locale.
 */
export function parseLocaleNumber(raw: string, locale: string): number | undefined {
  // Intl formats negatives with U+2212 MINUS SIGN in sv-SE, nb-NO, fi-FI...;
  // the small and fullwidth hyphen-minus also arrive in pasted text. Number()
  // reads only the ASCII '-', so a value the grid itself displayed would not
  // parse back.
  const s = raw.trim().replace(UNICODE_MINUS_RE, '-');
  if (s === '') return undefined;
  const { decimal, group } = getLocaleSeparators(locale);
  // Space-like group separators (fr-FR uses NBSP / narrow NBSP) accept any space.
  const groupPat = /^\s$/.test(group) ? SPACE_CLASS : escapeRegExp(group);
  const grouped = new RegExp(
    `^[+-]?\\d{1,3}(?:${groupPat}\\d{3})+(?:${escapeRegExp(decimal)}\\d+)?$`
  );
  if (grouped.test(s)) {
    let normalized = s.replace(new RegExp(groupPat, 'g'), '');
    if (decimal !== '.') normalized = normalized.split(decimal).join('.');
    const n = Number(normalized);
    if (Number.isFinite(n)) return n;
  }
  // Ungrouped: swap a single locale decimal separator to '.'.
  if (decimal !== '.' && s.split(decimal).length === 2) {
    const n = Number(s.replace(decimal, '.'));
    if (Number.isFinite(n)) return n;
  }
  // Dot-fallback: a plain "1.5" pasted in a comma-decimal locale.
  const fallback = Number(s.replace(SPACE_RE, ''));
  return Number.isFinite(fallback) ? fallback : undefined;
}

/** Quote a single cell for TSV if it contains a tab, newline, or quote. */
export function serializeCell(value: string): string {
  if (/[\t\n\r"]/.test(value)) {
    return `"${value.replace(/"/g, '""')}"`;
  }
  return value;
}

/** Serialize a 2D matrix of display strings to clipboard TSV. */
export function serializeRangeToTSV(grid: string[][]): string {
  return grid.map((row) => row.map(serializeCell).join('\t')).join('\n');
}

/**
 * Parse clipboard text (TSV from Excel/Sheets, or our own) into a 2D matrix.
 * State machine: honours quoted fields containing tabs/newlines and doubled
 * quotes; treats CRLF and LF as row separators; drops a single trailing newline
 * so a copied block doesn't gain a phantom empty row.
 */
export function parseTSV(text: string): string[][] {
  const rows: string[][] = [];
  let row: string[] = [];
  let field = '';
  let inQuotes = false;
  let i = 0;
  const n = text.length;
  while (i < n) {
    const ch = text[i];
    if (inQuotes) {
      if (ch === '"') {
        if (text[i + 1] === '"') {
          field += '"';
          i += 2;
          continue;
        }
        inQuotes = false;
        i++;
        continue;
      }
      field += ch;
      i++;
      continue;
    }
    // A quote only opens a quoted field at the FIELD START (nothing accumulated
    // yet). A quote in the middle of an unquoted field is a literal character -
    // pasting `say "hi"` must keep the inch marks, not strip them (RFC-4180).
    if (ch === '"' && field === '') {
      inQuotes = true;
      i++;
      continue;
    }
    if (ch === '\t') {
      row.push(field);
      field = '';
      i++;
      continue;
    }
    if (ch === '\n' || ch === '\r') {
      row.push(field);
      field = '';
      rows.push(row);
      row = [];
      i += ch === '\r' && text[i + 1] === '\n' ? 2 : 1;
      continue;
    }
    field += ch;
    i++;
  }
  row.push(field);
  rows.push(row);
  // Drop a trailing empty row produced by a terminal newline.
  if (rows.length > 1) {
    const last = rows[rows.length - 1];
    if (last && last.length === 1 && last[0] === '') rows.pop();
  }
  return rows;
}
