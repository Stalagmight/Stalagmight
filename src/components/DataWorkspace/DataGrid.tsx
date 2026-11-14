/**
 * The Data workspace's SpreadsheetGrid, wired: column defs -> grid columns,
 * visibility (+ columns revealed by search), order / widths / sort from the
 * store, find highlights, row tints, the rail gutter, section header bands,
 * the section named in the header corner once its header scrolls away,
 * focus requests (scrollTo) and keeping the active row across rebuilds,
 * a station's link chips spanning the shown shot-only columns (or counted in
 * the gutter when none is shown), context-menu actions, the header menu, and
 * the selection summary.
 */
import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';
import { SpreadsheetGrid } from '../common/SpreadsheetGrid';
import type { CellMarks, GridExtraMenuItem, GridScrollRequest } from '../common/SpreadsheetGrid/types';
import { useGridLabels } from '../../hooks/useGridLabels';
import { useUIStore } from '../../store/uiStore';
import { useDataWorkspaceStore } from './store';
import { ROW_ISSUES_KEY, ROW_LINKS_KEY, ROW_NAME_KEY, type DataColumnDef, type DataColumnKey, type DataRow, type DataView, type SearchColKey } from './types';
import type { DataWorkspaceModel } from './useDataWorkspaceModel';
import type { DataEditing } from './useDataEditing';
import type { DataNav } from './useDataNavigation';
import type { DataTint } from './useDataTint';
import { buildGridColumns } from './dataGridColumns';
import { buildRail } from './model/links';
import { stationLinkItems } from './model/linkItems';
import { shotBlockSpan } from './model/columnGroups';
import { StationLinkChips } from './StationLinkChips';
import { orderColumns } from './model/search';
import { isEntityRow, parseRowKey, sectionRowKey } from './model/rows';
import { DataRowLabel, DATA_GUTTER_WIDTH, GUTTER_RAIL_LANE, type RevealReason } from './DataRowLabel';
import { DataSectionCorner, DataSectionHeader } from './DataSectionHeader';
import { DataHeaderMenu, type HeaderMenuState } from './DataHeaderMenu';
import { dataCellTestId, dataRowTestId } from './dataTestIds';
import { selectionRectFromDom, summarizeSelection, type GridSelectionInfo, type SelectionSummary } from './selectionSummary';

export const DATA_GRID_PREFIX = 'data-grid';

/**
 * Where the active cell starts on a first entry (nothing remembered): the first
 * station / shot row - never a section header band - in the first shown column
 * that applies to that row's kind, so the cell means something (Comment on a
 * station, as Depth is hidden by default; Length on a shot), never a blank
 * cell. When no shown column applies to that kind (Comment hidden too), the
 * first row of the other kind that has one. null when there is no entity row.
 */
export function initialActiveCell(
  rows: readonly DataRow[],
  orderedColumns: readonly Pick<DataColumnDef, 'key' | 'appliesTo'>[],
  hidden: readonly DataColumnKey[],
): { row: number; colKey: DataColumnKey | undefined } | null {
  const first = rows.findIndex(isEntityRow);
  if (first < 0) return null;
  const hiddenSet = new Set(hidden);
  const shown = orderedColumns.filter((c) => !hiddenSet.has(c.key));
  const pickFor = (kind: DataRow['kind']) => shown.find((c) => c.appliesTo === 'both' || c.appliesTo === kind)?.key;
  const firstKind = rows[first]!.kind;
  const own = pickFor(firstKind);
  if (own !== undefined) return { row: first, colKey: own };
  const other = rows.findIndex((r) => isEntityRow(r) && r.kind !== firstKind);
  const otherCol = other >= 0 ? pickFor(rows[other]!.kind) : undefined;
  if (otherCol !== undefined) return { row: other, colKey: otherCol };
  return { row: first, colKey: shown[0]?.key };
}

/** FNV-1a over the row keys: changes whenever the rows or their order do. */
export function rowsVersionOf(rows: readonly DataRow[]): string {
  let h = 0x811c9dc5;
  for (const r of rows) {
    const k = r.key;
    for (let i = 0; i < k.length; i++) {
      h ^= k.charCodeAt(i);
      h = Math.imul(h, 0x01000193);
    }
    h ^= 0x7c;
  }
  return `${rows.length}:${(h >>> 0).toString(36)}`;
}

/**
 * Where a focus request for a section header (`sec:N`) lands: the header when
 * it is displayed, else the section's first displayed row (sorted, headers
 * off, the header dropped). null when nothing of the section is on screen: a
 * header is never revealed (model/view.ts), so such a request is dropped
 * rather than left pending. Other keys are returned as they are.
 */
export function focusTargetKey(
  rowKey: string,
  rows: readonly DataRow[],
  rowIndexByKey: ReadonlyMap<string, number>,
): string | null {
  if (rowIndexByKey.has(rowKey)) return rowKey;
  const parsed = parseRowKey(rowKey);
  if (parsed?.kind !== 'section') return rowKey;
  return rows.find((r) => r.sectionId === parsed.sectionId)?.key ?? null;
}

let scrollNonce = 0;
/** The last store focus request acted on. Module-level, not per mount: the
 *  request stays in the store, and a remount (leaving and re-entering Data)
 *  must not replay it. */
let handledFocusNonce: number | null = null;

interface Props {
  model: DataWorkspaceModel;
  view: DataView;
  height: number;
  nav: DataNav;
  editing: DataEditing;
  tint: DataTint;
  partners: ReadonlySet<string>;
  onSelectionSummary: (summary: SelectionSummary | null) => void;
}

export function DataGrid({ model, view, height, nav, editing, tint, partners, onSelectionSummary }: Props) {
  const { t } = useTranslation();
  const { ctx } = model;
  const containerRef = useRef<HTMLDivElement | null>(null);
  const {
    hiddenColumns,
    columnOrder,
    columnWidths,
    sort,
    focusRequest,
    collapsedSections,
    currentMatch,
    revealRowKeys,
    setColumnOrder,
    setColumnWidths,
    setSort,
    setActiveRowKey,
  } = useDataWorkspaceStore(
    useShallow((s) => ({
      hiddenColumns: s.hiddenColumns,
      columnOrder: s.columnOrder,
      columnWidths: s.columnWidths,
      sort: s.sort,
      focusRequest: s.focusRequest,
      collapsedSections: s.collapsedSections,
      currentMatch: s.currentMatch,
      revealRowKeys: s.revealRowKeys,
      setColumnOrder: s.setColumnOrder,
      setColumnWidths: s.setColumnWidths,
      setSort: s.setSort,
      setActiveRowKey: s.setActiveRowKey,
    })),
  );

  const baseLabels = useGridLabels('dataTable.shotNoun');
  const labels = useMemo(
    () => ({ ...baseLabels, rowNoun: (count: number) => t('dataWorkspace.rowNoun', { count }) }),
    [baseLabels, t],
  );

  // The gutter's issue icon on a shot: Survey opens on it with the resolver.
  const onResolveWarning = useCallback(
    (row: DataRow) => {
      if (row.kind !== 'shot') return;
      nav.openInSurvey(row);
      useUIStore.getState().triggerResolveWarning(row.sectionId, row.shotId);
    },
    [nav],
  );

  const columns = useMemo(
    () =>
      buildGridColumns(model.columns, {
        t,
        ctx,
        view,
        onCycleKind: editing.cycleKind,
        onOpenCredit: editing.openCreditEditor,
      }),
    [model.columns, t, ctx, view, editing.cycleKind, editing.openCreditEditor],
  );

  const gridHidden = useMemo(
    () => hiddenColumns.filter((k) => !view.revealedColumns.has(k)),
    [hiddenColumns, view.revealedColumns],
  );

  const rail = useMemo(() => buildRail(view.rows, ctx, view.surveyOrder), [view.rows, view.surveyOrder, ctx]);
  const viewRows = view.rows;
  const rowIndexByKey = useMemo(() => new Map(viewRows.map((r, i) => [r.key, i])), [viewRows]);
  const rowsVersion = useMemo(() => rowsVersionOf(view.rows), [view.rows]);
  const collapsed = useMemo(() => new Set(collapsedSections), [collapsedSections]);
  const navRevealed = useMemo(() => new Set(revealRowKeys), [revealRowKeys]);

  // ── Focus requests + keeping the active row across rebuilds ─────────────
  const lastActive = useRef<{ key: string; idx: number; colKey: string } | null>(null);
  const [scrollReq, setScrollReq] = useState<GridScrollRequest | null>(null);

  // One effect, so a pending focus request always beats the re-anchoring of the
  // previous active row (entering Data from "Open in Data" / "Back to Data").
  useEffect(() => {
    if (focusRequest && handledFocusNonce !== focusRequest.nonce) {
      const targetKey = focusTargetKey(focusRequest.rowKey, viewRows, rowIndexByKey);
      // A section with nothing on screen: drop the request (see focusTargetKey).
      if (targetKey === null) {
        handledFocusNonce = focusRequest.nonce;
        return;
      }
      const idx = rowIndexByKey.get(targetKey);
      if (idx === undefined) {
        // A row that no longer exists: drop the request.
        if (!model.rowByKey.has(targetKey)) {
          handledFocusNonce = focusRequest.nonce;
          return;
        }
        // Hidden by the filters / collapse: reveal it (once), then this effect
        // runs again with the row in the view. Meanwhile, don't re-anchor.
        const store = useDataWorkspaceStore.getState();
        if (!store.revealRowKeys.includes(targetKey)) {
          const { colKey, align, takeFocus } = focusRequest;
          store.revealAndFocus({
            rowKey: targetKey,
            ...(colKey ? { colKey } : {}),
            ...(align ? { align } : {}),
            ...(takeFocus !== undefined ? { takeFocus } : {}),
          });
        }
        return;
      }
      handledFocusNonce = focusRequest.nonce;
      const colKey = focusRequest.colKey ?? lastActive.current?.colKey;
      lastActive.current = { key: targetKey, idx, colKey: colKey ?? '' };
      setActiveRowKey(targetKey);
      setScrollReq({
        row: idx,
        ...(colKey ? { colKey } : {}),
        align: focusRequest.align ?? 'auto',
        // Find navigation while typing sends takeFocus:false - the find box keeps
        // focus; anything else uses the grid's default (focus only when the grid
        // or the body already has it).
        ...(focusRequest.takeFocus === false ? { focus: false } : focusRequest.takeFocus ? { focus: true } : {}),
        nonce: ++scrollNonce,
      });
      return;
    }
    // Keep the active row across rebuilds (filters, sort, data refresh) by key.
    const key = useDataWorkspaceStore.getState().activeRowKey;
    if (!key) {
      // First entry (or a new project): start on the first data row, not on the
      // section header band the grid's own default would pick.
      const start = initialActiveCell(viewRows, orderColumns(model.columns, columnOrder), gridHidden);
      if (!start) return;
      const startKey = viewRows[start.row]!.key;
      lastActive.current = { key: startKey, idx: start.row, colKey: start.colKey ?? '' };
      setActiveRowKey(startKey);
      setScrollReq({ row: start.row, ...(start.colKey ? { colKey: start.colKey } : {}), align: 'auto', nonce: ++scrollNonce });
      return;
    }
    const idx = rowIndexByKey.get(key);
    if (idx === undefined || idx === lastActive.current?.idx) return;
    const colKey = lastActive.current?.colKey || undefined;
    // First mount (entering Data): bring the remembered row to the middle.
    const align = lastActive.current ? 'auto' : 'center';
    lastActive.current = { key, idx, colKey: colKey ?? '' };
    setScrollReq({ row: idx, ...(colKey ? { colKey } : {}), align, nonce: ++scrollNonce });
  }, [focusRequest, viewRows, rowIndexByKey, setActiveRowKey, model.rowByKey, model.columns, columnOrder, gridHidden]);

  const onActiveCellChange = useCallback(
    (rowIndex: number, colKey: string) => {
      const row = view.rows[rowIndex];
      if (!row) return;
      lastActive.current = { key: row.key, idx: rowIndex, colKey };
      setActiveRowKey(row.key);
    },
    [view.rows, setActiveRowKey],
  );

  // ── Find highlights ────────────────────────────────────────────────────
  const cellMarks = useCallback(
    (row: DataRow, _i: number, colKey: string): CellMarks | undefined => {
      const ids = view.matchIndex.get(row.key)?.get(colKey as SearchColKey);
      if (!ids || ids.length === 0) return undefined;
      const ranges = ids.map((i) => [view.matches[i]!.start, view.matches[i]!.end] as const);
      const cur = ids.includes(currentMatch) ? view.matches[currentMatch] : undefined;
      return cur ? { ranges, current: [cur.start, cur.end] } : { ranges };
    },
    [view.matchIndex, view.matches, currentMatch],
  );

  // ── Row look ───────────────────────────────────────────────────────────
  const rowStyle = useCallback(
    (row: DataRow): CSSProperties | undefined => {
      const color = tint.colorOf(row);
      return color ? ({ '--sg-row-tint': color } as CSSProperties) : undefined;
    },
    [tint],
  );
  const revealReason = useCallback(
    (row: DataRow): RevealReason => (navRevealed.has(row.key) ? 'nav' : view.revealedRowKeys.has(row.key) ? 'search' : null),
    [navRevealed, view.revealedRowKeys],
  );
  const rowClassName = useCallback(
    (row: DataRow) => {
      let cls = `dw-row dw-row-${row.kind}`;
      if (partners.has(row.key)) cls += ' dw-row-partner';
      if (revealReason(row)) cls += ' dw-row-revealed';
      return cls;
    },
    [partners, revealReason],
  );
  // ── Station links: chips spanning the shown shot-only block ────────────
  const shotSpan = useMemo(
    () => shotBlockSpan(orderColumns(model.columns, columnOrder).map((c) => c.key), new Set(gridHidden)),
    [model.columns, columnOrder, gridHidden],
  );
  const spanCells = useCallback(
    (row: DataRow, i: number) => {
      if (row.kind !== 'station' || !shotSpan) return undefined;
      const items = stationLinkItems(ctx, row);
      if (items.length === 0) return undefined;
      return {
        from: shotSpan.from,
        to: shotSpan.to,
        content: <StationLinkChips row={row} items={items} ctx={ctx} nav={nav} marks={cellMarks(row, i, ROW_LINKS_KEY)} />,
      };
    },
    [shotSpan, ctx, nav, cellMarks],
  );

  const rowLabel = useCallback(
    (row: DataRow, i: number) => (
      <DataRowLabel
        row={row}
        ctx={ctx}
        rail={rail?.[i] ?? null}
        tint={tint.colorOf(row)}
        revealed={revealReason(row)}
        nav={nav}
        nameMarks={cellMarks(row, i, ROW_NAME_KEY)}
        // No shot-only column shown: the chips have no room, the gutter counts them.
        gutterLinks={row.kind === 'station' && !shotSpan ? stationLinkItems(ctx, row) : null}
        linkMarks={cellMarks(row, i, ROW_LINKS_KEY)}
        issueMarks={cellMarks(row, i, ROW_ISSUES_KEY)}
        onResolveWarning={onResolveWarning}
      />
    ),
    [ctx, rail, tint, revealReason, nav, cellMarks, shotSpan, onResolveWarning],
  );
  // The section the top rows belong to, in the header corner, once its header
  // row has scrolled away (or with headers off). Nothing while sorted: the
  // rows then mix sections.
  const headerCorner = useCallback(
    (firstVisibleRow: number) => {
      if (!view.surveyOrder) return null;
      const row = viewRows[firstVisibleRow];
      if (!row || row.kind === 'section') return null;
      return <DataSectionCorner sectionId={row.sectionId} ctx={ctx} />;
    },
    [view.surveyOrder, viewRows, ctx],
  );
  const groupRowContent = useCallback(
    (row: DataRow) =>
      row.kind === 'section' ? (
        <DataSectionHeader
          sectionId={row.sectionId}
          ctx={ctx}
          stats={model.sectionStats.get(row.sectionId)}
          collapsed={collapsed.has(row.sectionId)}
          nav={nav}
        />
      ) : undefined,
    [ctx, model.sectionStats, collapsed, nav],
  );

  // ── Map hover <-> row hover ──────────────────────────────────────────────
  const onRowHover = useCallback(
    (rowIndex: number | null) => {
      const row = rowIndex === null ? undefined : view.rows[rowIndex];
      const ui = useUIStore.getState();
      ui.setHoveredShot(row?.kind === 'shot' ? { sectionId: row.sectionId, shotId: row.shotId } : null);
      ui.setHoveredStation(row?.kind === 'station' ? { section_id: row.sectionId, station_index: row.stationIndex } : null);
    },
    [view.rows],
  );
  useEffect(
    () => () => {
      useUIStore.getState().setHoveredShot(null);
      useUIStore.getState().setHoveredStation(null);
    },
    [],
  );

  // ── Context menus ──────────────────────────────────────────────────────
  const extraContextMenuItems = useCallback(
    ({ fromRow, toRow }: { fromRow: number; toRow: number }): GridExtraMenuItem[] => {
      const row = view.rows[fromRow];
      if (!row) return [];
      const range = view.rows.slice(fromRow, toRow + 1);
      const isCollapsed = collapsed.has(row.sectionId);
      // Collapse works only in survey order with the section's header in the
      // model (model/view.ts); anywhere else the toggle would do nothing now
      // and surprise later, so it is not offered.
      const canCollapse = view.surveyOrder && model.rowByKey.has(sectionRowKey(row.sectionId));
      const items: GridExtraMenuItem[] = [
        { label: t('dataWorkspace.menu.showOnMap'), onClick: () => nav.showOnMap(row), testId: 'data-ctx-show-on-map', separatorBefore: true },
        { label: t('dataWorkspace.menu.openInSurvey'), onClick: () => nav.openInSurvey(row), testId: 'data-ctx-open-in-survey' },
        { label: t('dataWorkspace.menu.copyRowName'), onClick: () => nav.copyRowNames(range), testId: 'data-ctx-copy-row-name' },
        {
          label: t('dataWorkspace.menu.filterToSection'),
          onClick: () => nav.filterToSection(row.sectionId),
          testId: 'data-ctx-filter-section',
          separatorBefore: true,
        },
      ];
      if (canCollapse) {
        items.push({
          label: t(isCollapsed ? 'dataWorkspace.menu.expandSection' : 'dataWorkspace.menu.collapseSection'),
          onClick: () => useDataWorkspaceStore.getState().toggleSectionCollapsed(row.sectionId),
          testId: 'data-ctx-toggle-section',
        });
      }
      return items;
    },
    [view.rows, view.surveyOrder, model.rowByKey, collapsed, nav, t],
  );

  const [headerMenu, setHeaderMenu] = useState<HeaderMenuState | null>(null);
  const closeHeaderMenu = useCallback(() => setHeaderMenu(null), []);

  // ── Selection summary ─────────────────────────────────────────────────
  const selRef = useRef<GridSelectionInfo | null>(null);
  const onSelectionChange = useCallback(
    (sel: GridSelectionInfo) => {
      selRef.current = sel;
      const cells = sel.rows * sel.cols;
      if (cells <= 1) {
        onSelectionSummary(null);
        return;
      }
      // Read the marks after the grid has painted them.
      requestAnimationFrame(() => {
        if (selRef.current !== sel) return;
        const rect = selectionRectFromDom(containerRef.current, DATA_GRID_PREFIX, sel, view.rows.length);
        onSelectionSummary(summarizeSelection(view.rows, rect, model.columnByKey, ctx, cells));
      });
    },
    [onSelectionSummary, view.rows, model.columnByKey, ctx],
  );

  return (
    <div ref={containerRef} className="h-full w-full" onKeyDownCapture={editing.onKeyDownCapture}>
      {editing.creditEditor}
      <SpreadsheetGrid<DataRow>
        columns={columns}
        rows={view.rows}
        rowKey={(r) => r.key}
        rowTestId={dataRowTestId}
        cellTestId={(r, _i, colKey) => (isEntityRow(r) ? dataCellTestId(r, colKey) : undefined)}
        testIdPrefix={DATA_GRID_PREFIX}
        locale={ctx.locale}
        height={height}
        onEdit={editing.onEdit}
        onBatchEdit={editing.onBatchEdit}
        onPasteReport={editing.onPasteReport}
        onActiveCellChange={onActiveCellChange}
        onRowHover={onRowHover}
        onSelectionChange={onSelectionChange}
        rowLabel={rowLabel}
        gutterWidth={DATA_GUTTER_WIDTH}
        labels={labels}
        hiddenColumns={gridHidden}
        columnOrder={columnOrder}
        onColumnOrderChange={(order) => setColumnOrder(order as DataColumnKey[])}
        columnWidths={columnWidths}
        onColumnWidthsChange={setColumnWidths}
        sort={sort}
        onSortChange={(colKey, dir) => setSort(dir ? { colKey: colKey as DataColumnKey, dir } : null)}
        cellMarks={cellMarks}
        rowStyle={rowStyle}
        rowClassName={rowClassName}
        rowsVersion={rowsVersion}
        groupRowContent={groupRowContent}
        spanCells={spanCells}
        groupContentStart="gutter"
        groupContentInset={GUTTER_RAIL_LANE}
        headerCorner={headerCorner}
        scrollTo={scrollReq}
        extraContextMenuItems={extraContextMenuItems}
        onHeaderContextMenu={(colKey, pos) => setHeaderMenu({ ...pos, colKey: colKey as DataColumnKey })}
      />
      {headerMenu && (
        <DataHeaderMenu
          menu={headerMenu}
          def={model.columnByKey.get(headerMenu.colKey)}
          revealed={view.revealedColumns.has(headerMenu.colKey)}
          onClose={closeHeaderMenu}
          anchorScope={containerRef}
        />
      )}
    </div>
  );
}
