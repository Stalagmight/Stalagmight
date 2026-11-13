/**
 * The Data workspace's SpreadsheetGrid, wired: column defs -> grid columns,
 * visibility (+ columns revealed by search), order / widths / sort from the
 * store, find highlights, row tints, the rail gutter, section header bands,
 * focus requests (scrollTo) and keeping the active row across rebuilds,
 * context-menu actions, the header menu, and the selection summary.
 */
import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';
import { SpreadsheetGrid } from '../common/SpreadsheetGrid';
import type { CellMarks, GridExtraMenuItem, GridScrollRequest } from '../common/SpreadsheetGrid/types';
import { useGridLabels } from '../../hooks/useGridLabels';
import { useUIStore } from '../../store/uiStore';
import { useDataWorkspaceStore } from './store';
import type { DataColumnKey, DataRow, DataView } from './types';
import type { DataWorkspaceModel } from './useDataWorkspaceModel';
import type { DataEditing } from './useDataEditing';
import type { DataNav } from './useDataNavigation';
import type { DataTint } from './useDataTint';
import { buildGridColumns } from './dataGridColumns';
import { buildRail } from './model/links';
import { isEntityRow } from './model/rows';
import { DataRowLabel, DATA_GUTTER_WIDTH, type RevealReason } from './DataRowLabel';
import { DataSectionHeader } from './DataSectionHeader';
import { DataHeaderMenu, type HeaderMenuState } from './DataHeaderMenu';
import { dataCellTestId, dataRowTestId } from './dataTestIds';
import { selectionRectFromDom, summarizeSelection, type GridSelectionInfo, type SelectionSummary } from './selectionSummary';

export const DATA_GRID_PREFIX = 'data-grid';

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

let scrollNonce = 0;

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
        nav,
        onCycleKind: editing.cycleKind,
        onOpenCredit: editing.openCreditEditor,
        onResolveWarning,
      }),
    [model.columns, t, ctx, view, nav, editing.cycleKind, editing.openCreditEditor, onResolveWarning],
  );

  const gridHidden = useMemo(
    () => hiddenColumns.filter((k) => !view.revealedColumns.has(k)),
    [hiddenColumns, view.revealedColumns],
  );

  const rail = useMemo(() => buildRail(view.rows, ctx, view.surveyOrder), [view.rows, view.surveyOrder, ctx]);
  const rowIndexByKey = useMemo(() => new Map(view.rows.map((r, i) => [r.key, i])), [view.rows]);
  const rowsVersion = useMemo(() => rowsVersionOf(view.rows), [view.rows]);
  const collapsed = useMemo(() => new Set(collapsedSections), [collapsedSections]);
  const navRevealed = useMemo(() => new Set(revealRowKeys), [revealRowKeys]);

  // ── Focus requests + keeping the active row across rebuilds ─────────────
  const lastActive = useRef<{ key: string; idx: number; colKey: string } | null>(null);
  const handledNonce = useRef<number | null>(null);
  const [scrollReq, setScrollReq] = useState<GridScrollRequest | null>(null);

  useEffect(() => {
    if (!focusRequest || handledNonce.current === focusRequest.nonce) return;
    const idx = rowIndexByKey.get(focusRequest.rowKey);
    if (idx === undefined) return; // not in the view yet (a reveal is on its way)
    handledNonce.current = focusRequest.nonce;
    const colKey = focusRequest.colKey ?? lastActive.current?.colKey;
    setScrollReq({ row: idx, ...(colKey ? { colKey } : {}), align: focusRequest.align ?? 'auto', nonce: ++scrollNonce });
  }, [focusRequest, rowIndexByKey]);

  useEffect(() => {
    const key = useDataWorkspaceStore.getState().activeRowKey;
    if (!key) return;
    const idx = rowIndexByKey.get(key);
    if (idx === undefined || idx === lastActive.current?.idx) return;
    const colKey = lastActive.current?.colKey;
    // First mount (entering Data): bring the remembered row to the middle.
    setScrollReq({ row: idx, ...(colKey ? { colKey } : {}), align: lastActive.current ? 'auto' : 'center', nonce: ++scrollNonce });
  }, [rowIndexByKey]);

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
      const ids = view.matchIndex.get(row.key)?.get(colKey as DataColumnKey);
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
  const rowLabel = useCallback(
    (row: DataRow, i: number) => (
      <DataRowLabel row={row} ctx={ctx} rail={rail?.[i] ?? null} tint={tint.colorOf(row)} revealed={revealReason(row)} nav={nav} />
    ),
    [ctx, rail, tint, revealReason, nav],
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
      return [
        { label: t('dataWorkspace.menu.showOnMap'), onClick: () => nav.showOnMap(row), testId: 'data-ctx-show-on-map', separatorBefore: true },
        { label: t('dataWorkspace.menu.openInSurvey'), onClick: () => nav.openInSurvey(row), testId: 'data-ctx-open-in-survey' },
        { label: t('dataWorkspace.menu.copyRowName'), onClick: () => nav.copyRowNames(range), testId: 'data-ctx-copy-row-name' },
        {
          label: t('dataWorkspace.menu.filterToSection'),
          onClick: () => nav.filterToSection(row.sectionId),
          testId: 'data-ctx-filter-section',
          separatorBefore: true,
        },
        {
          label: t(isCollapsed ? 'dataWorkspace.menu.expandSection' : 'dataWorkspace.menu.collapseSection'),
          onClick: () => useDataWorkspaceStore.getState().toggleSectionCollapsed(row.sectionId),
          testId: 'data-ctx-toggle-section',
        },
      ];
    },
    [view.rows, collapsed, nav, t],
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
        const rect = selectionRectFromDom(containerRef.current, DATA_GRID_PREFIX, sel);
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
        scrollTo={scrollReq}
        extraContextMenuItems={extraContextMenuItems}
        onHeaderContextMenu={(colKey, pos) => setHeaderMenu({ ...pos, colKey: colKey as DataColumnKey })}
      />
      {headerMenu && (
        <DataHeaderMenu
          menu={headerMenu}
          def={model.columnByKey.get(headerMenu.colKey)}
          onClose={closeHeaderMenu}
          anchorScope={containerRef}
        />
      )}
    </div>
  );
}
