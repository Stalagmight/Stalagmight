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
 */
import type {
  DataColumnDef,
  DataColumnKey,
  DataEntityRow,
  DataModelCtx,
  DataRow,
  DataView,
  SearchMatch,
} from '../types';
import type { DataWorkspaceState } from '../store';
import { applyFilters } from './filter';
import { compileSearch, findInText, MAX_SEARCH_MATCHES, orderColumns, searchColumnsInScope } from './search';
import { sortRows } from './sort';

export type DataViewInput = Pick<
  DataWorkspaceState,
  'filters' | 'sort' | 'collapsedSections' | 'search' | 'hiddenColumns' | 'columnOrder' | 'revealRowKeys'
>;

const appliesTo = (col: DataColumnDef, row: DataEntityRow): boolean =>
  col.appliesTo === 'both' || col.appliesTo === row.kind;

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
    outer: for (let i = 0; i < n; i++) {
      const row = rows[i]!;
      if (row.kind === 'section') continue;
      if (search.rowScope === 'visible' && !shown[i] && !navReveal[i]) continue;
      for (const col of scopeCols) {
        if (!appliesTo(col, row)) continue;
        spans.length = 0;
        const found = findInText(col.text(row, ctx), compiled, spans, MAX_SEARCH_MATCHES - matchTotal);
        if (found === 0) continue;
        const lockReason = col.lockReason(row, ctx);
        let list = rowMatches[i];
        if (!list) rowMatches[i] = list = [];
        for (let k = 0; k < spans.length; k += 2) {
          list.push({ rowKey: row.key, colKey: col.key, start: spans[k]!, end: spans[k + 1]!, lockReason });
        }
        matchTotal += found;
        if (hidden.has(col.key)) revealedColumns.add(col.key);
        if (matchTotal >= MAX_SEARCH_MATCHES) {
          matchLimitReached = true;
          break outer;
        }
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
  const matchIndex = new Map<string, Map<DataColumnKey, number[]>>();
  if (matchTotal > 0) {
    const indexOfRow = new Map<string, number>();
    for (let i = 0; i < n; i++) if (rowMatches[i]) indexOfRow.set(rows[i]!.key, i);
    for (const row of viewRows) {
      const i = indexOfRow.get(row.key);
      if (i === undefined) continue;
      const byCol = new Map<DataColumnKey, number[]>();
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
