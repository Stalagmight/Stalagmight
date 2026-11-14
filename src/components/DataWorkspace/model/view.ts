/**
 * The view pipeline: filters -> collapse -> search (matches, reveal) ->
 * navigation reveals -> "only rows with matches" -> sort.
 *
 * Definitions the toolbar, the grid and the status bar rely on:
 * - A section is collapsed only while it has a header row (`showSectionHeaders`)
 *   and the view is in survey order; with no header on screen there is nothing
 *   to expand it from, so collapse is ignored (headers off, or sorted).
 * - The find searches only while the find bar is open (`search.open`) and the
 *   query compiles. Row scope 'visible' searches the rows on screen (filters,
 *   collapse and navigation reveals applied); 'all' searches every entity row
 *   and shows the hidden ones that match (`revealedRowKeys`).
 * - Navigation reveals (`revealRowKeys`) are shown even under "only rows with
 *   matches": the user asked to see that row.
 * - A section header is kept (survey order only) when its section shows at
 *   least one row, or when it is collapsed and has rows passing the filters
 *   (so it can be expanded again).
 * - `filteredCount` counts the entity rows that pass the filters; collapse,
 *   reveals and search narrowing don't change it.
 * - Matches are listed in display order: row, then column display order, then
 *   offset. Hidden columns holding a match under the 'all' / 'custom' column
 *   scopes are listed in `revealedColumns` and sit at their place in the
 *   column order.
 * - The row name (`ROW_NAME_KEY`: the gutter's row ID + the section's
 *   full name) is searched like an always-shown first column under the
 *   'visible' and 'all' column scopes (not 'custom', which lists real
 *   columns): at most one read-only match per row, before its cell matches.
 * - A row's issues (`ROW_ISSUES_KEY`: its unresolved warnings, the gutter's
 *   issue icon) are searched under the same scopes, right after the row name.
 * - A station's links (`ROW_LINKS_KEY`: its chip labels, one per line) are
 *   searched under the same scopes, and listed where the shot-only block
 *   starts (`linksColumnPosition`): their chips span it.
 */
import type {
  DataColumnDef,
  DataColumnKey,
  DataEntityRow,
  DataModelCtx,
  DataRow,
  DataView,
  SearchColKey,
  SearchMatch,
} from '../types';
import { ROW_ISSUES_KEY, ROW_LINKS_KEY, ROW_NAME_KEY } from '../types';
import type { DataWorkspaceState } from '../store';
import { applyFilters } from './filter';
import { compileSearch, findInText, MAX_SEARCH_MATCHES, orderColumns, rowNameMatch, searchColumnsInScope } from './search';
import { sortRows } from './sort';
import { linksSearchText, stationLinkItems } from './linkItems';
import { issuesText, rowWarnings } from './columns';

export type DataViewInput = Pick<
  DataWorkspaceState,
  'filters' | 'sort' | 'collapsedSections' | 'search' | 'hiddenColumns' | 'columnOrder' | 'revealRowKeys'
>;

const appliesTo = (col: DataColumnDef, row: DataEntityRow): boolean =>
  col.appliesTo === 'both' || col.appliesTo === row.kind;

/**
 * Where a station's links sit among the columns, as a display index: just
 * before the first shot-only column (their chips span the shot-only block),
 * or after every column when there is none. Find lists and steps through
 * the links matches there.
 */
export function linksColumnPosition(ordered: readonly Pick<DataColumnDef, 'appliesTo'>[]): number {
  const at = ordered.findIndex((c) => c.appliesTo === 'shot');
  return at < 0 ? ordered.length : at;
}

export function buildDataView(
  rows: readonly DataRow[],
  columns: readonly DataColumnDef[],
  ctx: DataModelCtx,
  state: DataViewInput,
): DataView {
  const n = rows.length;

  // 1. Filters.
  const pass = applyFilters(rows, columns, ctx, state.filters);
  let totalCount = 0;
  let filteredCount = 0;
  const headerSections = new Set<number>();
  for (let i = 0; i < n; i++) {
    const row = rows[i]!;
    if (row.kind === 'section') headerSections.add(row.sectionId);
    else {
      totalCount++;
      filteredCount += pass[i]!;
    }
  }

  // 2. Collapse: only sections that have a header to expand from, and never
  // while sorted (the headers are dropped then, so nothing could expand them).
  const sortCol = state.sort ? columns.find((c) => c.key === state.sort!.colKey) : undefined;
  const sorted = state.sort !== null && sortCol !== undefined;
  const collapsed = new Set(sorted ? [] : state.collapsedSections.filter((id) => headerSections.has(id)));
  const shown = new Uint8Array(n);
  for (let i = 0; i < n; i++) {
    if (pass[i] && !collapsed.has(rows[i]!.sectionId)) shown[i] = 1;
  }

  // Navigation reveals (applied before search so a revealed row is searched
  // like any other row on screen).
  const navKeys = new Set(state.revealRowKeys);
  const navReveal = new Uint8Array(n);
  if (navKeys.size > 0) {
    for (let i = 0; i < n; i++) {
      const row = rows[i]!;
      if (row.kind !== 'section' && !shown[i] && navKeys.has(row.key)) navReveal[i] = 1;
    }
  }

  // 3. Search.
  const search = state.search;
  const compiled = search.open ? compileSearch(search) : null;
  const searching = compiled !== null && compiled.regex !== null;
  const ordered = orderColumns(columns, state.columnOrder);
  const scopeCols = searching ? searchColumnsInScope(ordered, search, state.hiddenColumns) : [];
  const hidden = new Set(state.hiddenColumns);
  const revealedColumns = new Set<DataColumnKey>();
  const rowMatches: (SearchMatch[] | undefined)[] = new Array<SearchMatch[] | undefined>(n);
  let matchTotal = 0;
  let matchLimitReached = false;
  if (searching) {
    const spans: number[] = [];
    // The gutter's row name and a station's link chips are always on screen
    // (no column to hide): searched like shown columns, but not under the
    // 'custom' scope, which lists real columns.
    const searchPseudo = search.columnScope !== 'custom';
    const sectionHit = new Map<number, [number, number] | null>();
    // A station's links sit where the shot-only block starts (the chips span
    // it): before the first column at or after it in display order.
    const linksAt = linksColumnPosition(ordered);
    const orderIndex = new Map(ordered.map((c, k) => [c.key, k]));
    /** Push the matches of one text; true when the limit is reached. */
    const collect = (i: number, row: DataEntityRow, colKey: SearchColKey, text: string, lockReason: SearchMatch['lockReason']) => {
      spans.length = 0;
      const found = findInText(text, compiled, spans, MAX_SEARCH_MATCHES - matchTotal);
      if (found === 0) return false;
      let list = rowMatches[i];
      if (!list) rowMatches[i] = list = [];
      for (let k = 0; k < spans.length; k += 2) {
        list.push({ rowKey: row.key, colKey, start: spans[k]!, end: spans[k + 1]!, lockReason });
      }
      matchTotal += found;
      return matchTotal >= MAX_SEARCH_MATCHES;
    };
    outer: for (let i = 0; i < n; i++) {
      const row = rows[i]!;
      if (row.kind === 'section') continue;
      if (search.rowScope === 'visible' && !shown[i] && !navReveal[i]) continue;
      if (searchPseudo) {
        const hit = rowNameMatch(ctx, row, compiled, sectionHit);
        if (hit) {
          rowMatches[i] = [{ rowKey: row.key, colKey: ROW_NAME_KEY, start: hit[0], end: hit[1], lockReason: 'computed' }];
          if (++matchTotal >= MAX_SEARCH_MATCHES) {
            matchLimitReached = true;
            break outer;
          }
        }
        // The issue icon at the end of the gutter name.
        if (collect(i, row, ROW_ISSUES_KEY, issuesText(rowWarnings(ctx, row)), 'computed')) {
          matchLimitReached = true;
          break outer;
        }
      }
      let linksText = searchPseudo && row.kind === 'station' ? linksSearchText(stationLinkItems(ctx, row)) : '';
      for (const col of scopeCols) {
        if (linksText !== '' && orderIndex.get(col.key)! >= linksAt) {
          const full = collect(i, row, ROW_LINKS_KEY, linksText, 'computed');
          linksText = '';
          if (full) {
            matchLimitReached = true;
            break outer;
          }
        }
        if (!appliesTo(col, row)) continue;
        const before = matchTotal;
        const full = collect(i, row, col.key, col.text(row, ctx), col.lockReason(row, ctx));
        if (matchTotal > before && hidden.has(col.key)) revealedColumns.add(col.key);
        if (full) {
          matchLimitReached = true;
          break outer;
        }
      }
      if (linksText !== '' && collect(i, row, ROW_LINKS_KEY, linksText, 'computed')) {
        matchLimitReached = true;
        break outer;
      }
    }
  }

  // Which entity rows display, and which only because of a reveal.
  const display = new Uint8Array(n);
  const revealedRowKeys = new Set<string>();
  const narrow = searching && search.onlyMatchingRows;
  const sectionShowsRow = new Set<number>();
  const sectionHasPassing = new Set<number>();
  for (let i = 0; i < n; i++) {
    const row = rows[i]!;
    if (row.kind === 'section') continue;
    if (pass[i]) sectionHasPassing.add(row.sectionId);
    const searchReveal = searching && search.rowScope === 'all' && !shown[i] && rowMatches[i] !== undefined;
    let on = shown[i] === 1 || navReveal[i] === 1 || searchReveal;
    if (on && narrow && rowMatches[i] === undefined && !navKeys.has(row.key)) on = false;
    if (!on) continue;
    display[i] = 1;
    sectionShowsRow.add(row.sectionId);
    if (!shown[i]) revealedRowKeys.add(row.key);
  }

  // 6. Order: survey order with headers, or sorted entity rows.
  let viewRows: DataRow[];
  if (sorted && state.sort) {
    const entity: DataEntityRow[] = [];
    for (let i = 0; i < n; i++) if (display[i]) entity.push(rows[i] as DataEntityRow);
    viewRows = sortRows(entity, sortCol!, state.sort.dir, ctx);
  } else {
    viewRows = [];
    for (let i = 0; i < n; i++) {
      const row = rows[i]!;
      if (row.kind === 'section') {
        const keep =
          sectionShowsRow.has(row.sectionId) ||
          (!narrow && collapsed.has(row.sectionId) && sectionHasPassing.has(row.sectionId));
        if (keep) viewRows.push(row);
      } else if (display[i]) viewRows.push(row);
    }
  }

  // 7. Matches in display order + index.
  const matches: SearchMatch[] = [];
  const matchIndex = new Map<string, Map<SearchColKey, number[]>>();
  if (matchTotal > 0) {
    const indexOfRow = new Map<string, number>();
    for (let i = 0; i < n; i++) if (rowMatches[i]) indexOfRow.set(rows[i]!.key, i);
    for (const row of viewRows) {
      const i = indexOfRow.get(row.key);
      if (i === undefined) continue;
      const byCol = new Map<SearchColKey, number[]>();
      for (const m of rowMatches[i]!) {
        let list = byCol.get(m.colKey);
        if (!list) byCol.set(m.colKey, (list = []));
        list.push(matches.length);
        matches.push(m);
      }
      matchIndex.set(row.key, byCol);
    }
  }

  return {
    rows: viewRows,
    matches,
    matchIndex,
    revealedRowKeys,
    revealedColumns,
    filteredCount,
    totalCount,
    surveyOrder: !sorted,
    matchLimitReached,
    searchNarrowing: narrow,
  };
}
