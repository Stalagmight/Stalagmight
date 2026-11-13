/**
 * Find in the Data workspace: compile the query once, then match it against
 * the display text of each searchable cell (`DataColumnDef.text`), so what is
 * highlighted is exactly what the cell shows.
 *
 * - Literal mode escapes the query; regex mode takes a JavaScript RegExp
 *   (tried with the `u` flag first, so `\p{L}` and astral characters work,
 *   then without it for patterns only the legacy syntax accepts).
 * - Case-insensitive unless "match case" is on (Unicode simple case folding).
 * - "Whole word" keeps a match only when it does not continue a word on either
 *   side. Word characters are Unicode letters, marks, digits and `_`, so it
 *   works for "Höhle" or "洞". A match that itself starts (or ends) with a
 *   non-word character is fine on that side, as in VS Code ("-3" is a whole
 *   word in "A-3").
 * - Zero-length regex matches (`^`, `a*` between letters) are skipped, never
 *   looped on.
 */
import type { DataColumnDef, DataColumnKey, DataEntityRow, DataModelCtx, SearchState } from '../types';
import { sectionLabelOf } from './ctx';

/** Hard cap on the matches one search collects (memory and highlight cost). */
export const MAX_SEARCH_MATCHES = 100_000;

export interface CompiledSearch {
  /** Global regex to run, or null when there is nothing to search. */
  regex: RegExp | null;
  /** The engine's message for an invalid regex (then `regex` is null). */
  error: string | null;
  wholeWord: boolean;
}

export type SearchPattern = Pick<SearchState, 'query' | 'matchCase' | 'wholeWord' | 'regex'>;

const escapeRegExp = (s: string): string => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

/** Compile the find options. An empty query compiles to "nothing to search". */
export function compileSearch(p: SearchPattern): CompiledSearch {
  if (p.query === '') return { regex: null, error: null, wholeWord: p.wholeWord };
  const flags = p.matchCase ? 'g' : 'gi';
  if (!p.regex) {
    return { regex: new RegExp(escapeRegExp(p.query), `${flags}u`), error: null, wholeWord: p.wholeWord };
  }
  try {
    return { regex: new RegExp(p.query, `${flags}u`), error: null, wholeWord: p.wholeWord };
  } catch {
    try {
      return { regex: new RegExp(p.query, flags), error: null, wholeWord: p.wholeWord };
    } catch (e) {
      return { regex: null, error: e instanceof Error ? e.message : String(e), wholeWord: p.wholeWord };
    }
  }
}

const WORD_RE = /[\p{L}\p{M}\p{N}_]/u;

/** The code point that ends just before `i` (handles surrogate pairs). */
function codePointBefore(text: string, i: number): string {
  if (i <= 0) return '';
  const low = text.charCodeAt(i - 1);
  if (low >= 0xdc00 && low <= 0xdfff && i >= 2) {
    const high = text.charCodeAt(i - 2);
    if (high >= 0xd800 && high <= 0xdbff) return text.slice(i - 2, i);
  }
  return text[i - 1]!;
}

function codePointAt(text: string, i: number): string {
  const cp = text.codePointAt(i);
  return cp === undefined ? '' : String.fromCodePoint(cp);
}

const isWord = (ch: string): boolean => ch !== '' && WORD_RE.test(ch);

/** Is [start, end) a whole word in `text`? */
export function isWholeWord(text: string, start: number, end: number): boolean {
  const leftOk = !isWord(codePointBefore(text, start)) || !isWord(codePointAt(text, start));
  if (!leftOk) return false;
  return !isWord(codePointAt(text, end)) || !isWord(codePointBefore(text, end));
}

/** Index one code point past `i` (two code units for an astral character
 *  under the `u` flag, which must never resume inside a surrogate pair). */
function stepPast(text: string, i: number, re: RegExp): number {
  const cp = text.codePointAt(i) ?? 0;
  return i + (cp > 0xffff && re.unicode ? 2 : 1);
}

/**
 * Walk the reportable matches of `compiled` in `text`: non-empty, and whole
 * words when that option is on. `visit` returns false to stop.
 */
function scan(text: string, compiled: CompiledSearch, visit: (m: RegExpExecArray) => boolean): void {
  const re = compiled.regex;
  if (!re || text === '') return;
  re.lastIndex = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) {
    const start = m.index;
    const end = start + m[0].length;
    if (end === start) {
      // Zero-length: step over one code point so the loop can't stall.
      if (start >= text.length) return;
      re.lastIndex = stepPast(text, start, re);
      continue;
    }
    if (compiled.wholeWord && !isWholeWord(text, start, end)) {
      // Another alignment may still be a whole word ("aa" in "aaa aa").
      re.lastIndex = stepPast(text, start, re);
      continue;
    }
    if (!visit(m)) return;
  }
}

/**
 * Every match of `compiled` in `text`, as `[start, end)` pairs pushed onto
 * `out` (flat: start0, end0, start1, end1, ...). Returns the number added;
 * `limit` stops early. The hot path of a search (once per cell), so the loop is
 * written out rather than going through `scan`'s callback.
 */
export function findInText(text: string, compiled: CompiledSearch, out: number[], limit = Infinity): number {
  const re = compiled.regex;
  if (!re || text === '' || limit <= 0) return 0;
  re.lastIndex = 0;
  let added = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) {
    const start = m.index;
    const end = start + m[0].length;
    if (end === start) {
      if (start >= text.length) break;
      re.lastIndex = stepPast(text, start, re);
      continue;
    }
    if (compiled.wholeWord && !isWholeWord(text, start, end)) {
      re.lastIndex = stepPast(text, start, re);
      continue;
    }
    out.push(start, end);
    if (++added >= limit) break;
  }
  return added;
}

/** Every reportable exec result in `text`, keyed by start offset (replace
 *  needs the capture groups). */
export function execAll(text: string, compiled: CompiledSearch): Map<number, RegExpExecArray> {
  const out = new Map<number, RegExpExecArray>();
  scan(text, compiled, (m) => {
    out.set(m.index, m);
    return true;
  });
  return out;
}

/**
 * Columns in their display order: the persisted `columnOrder` first (unknown
 * keys dropped), then every remaining column in default order.
 */
export function orderColumns<C extends { key: DataColumnKey }>(
  columns: readonly C[],
  columnOrder: readonly DataColumnKey[],
): C[] {
  if (columnOrder.length === 0) return [...columns];
  const byKey = new Map(columns.map((c) => [c.key, c]));
  const out: C[] = [];
  const seen = new Set<DataColumnKey>();
  for (const k of columnOrder) {
    const c = byKey.get(k);
    if (c && !seen.has(k)) {
      out.push(c);
      seen.add(k);
    }
  }
  for (const c of columns) if (!seen.has(c.key)) out.push(c);
  return out;
}

/**
 * The searchable columns a search looks in, in display order:
 * - 'visible': the shown ones;
 * - 'all': every searchable column (hidden ones too);
 * - 'custom': the chosen ones (`search.columns`), shown or not.
 */
export function searchColumnsInScope(
  orderedColumns: readonly DataColumnDef[],
  search: Pick<SearchState, 'columnScope' | 'columns'>,
  hiddenColumns: readonly DataColumnKey[],
): DataColumnDef[] {
  const hidden = new Set(hiddenColumns);
  const chosen = new Set(search.columns);
  return orderedColumns.filter((c) => {
    if (!c.searchable) return false;
    if (search.columnScope === 'visible') return !hidden.has(c.key);
    if (search.columnScope === 'custom') return chosen.has(c.key);
    return true;
  });
}

/** The gutter's row name: "<label>:<station index>" / "<label>:<shot position>". */
export function rowNameText(ctx: DataModelCtx, row: DataEntityRow): string {
  return `${sectionLabelOf(ctx, row.sectionId)}:${row.kind === 'station' ? row.stationIndex : row.position}`;
}

/** The row-name pseudo-column's text (`ROW_NAME_KEY`): the row name, a line
 *  break (no literal query spans it), then the section's full name. */
export function rowNameSearchText(ctx: DataModelCtx, row: DataEntityRow): string {
  return `${rowNameText(ctx, row)}\n${ctx.sectionNames.get(row.sectionId) ?? sectionLabelOf(ctx, row.sectionId)}`;
}

/**
 * The row-name match of a row, or null: the first match in its row name, else
 * in its section's full name (offsets into `rowNameSearchText`). One per row,
 * so next / previous never step twice through the same gutter name.
 * `sectionHit` caches the full-name test per section (it is the same for every
 * row of the section).
 */
export function rowNameMatch(
  ctx: DataModelCtx,
  row: DataEntityRow,
  compiled: CompiledSearch,
  sectionHit: Map<number, [number, number] | null>,
): [number, number] | null {
  const spans: number[] = [];
  const name = rowNameText(ctx, row);
  if (findInText(name, compiled, spans, 1) > 0) return [spans[0]!, spans[1]!];
  let hit = sectionHit.get(row.sectionId);
  if (hit === undefined) {
    const full = ctx.sectionNames.get(row.sectionId) ?? sectionLabelOf(ctx, row.sectionId);
    hit = findInText(full, compiled, spans, 1) > 0 ? [spans[0]!, spans[1]!] : null;
    sectionHit.set(row.sectionId, hit);
  }
  return hit ? [name.length + 1 + hit[0], name.length + 1 + hit[1]] : null;
}
