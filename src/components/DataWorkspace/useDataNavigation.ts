/**
 * Navigation out of (and back into) the Data workspace: "Show on map" (open
 * the map pane, select, centre), "Open in Survey" (switch mode, select, zoom,
 * leave a Back-to-Data chip), go-to-row (focus, revealing a filtered-out row),
 * and the two-way map follow: the active row selects + pans on the map pane,
 * and a selection made on the map focuses its row. No feedback loop: a
 * selection this workspace set itself is recognised and ignored.
 */
import { createContext, useCallback, useContext, useEffect, useMemo, useRef } from 'react';
import { useUIStore, type Selection } from '../../store/uiStore';
import { setSource } from '../../utils/telemetry';
import { useDataWorkspaceStore } from './store';
import type { DataModel } from './DataModelContext';
import type { DataColumnKey, DataRow, DataView } from './types';
import { rowName } from './model/ctx';
import { parseRowKey, sectionRowKey, shotRowKey, stationRowKey, type ParsedRowKey } from './model/rows';
import { shotKeyAt } from './model/links';

export interface DataNav {
  /** Focus a row; a row the filters hide is revealed (and marked) first. */
  goToRow: (rowKey: string, colKey?: DataColumnKey) => void;
  showOnMap: (row: DataRow) => void;
  openInSurvey: (row: DataRow) => void;
  copyRowNames: (rows: readonly DataRow[]) => void;
  filterToSection: (sectionId: number) => void;
  /** Highlight the rows linked to the hovered station (null clears). */
  setHoveredPartners: (keys: readonly string[] | null) => void;
}

export const DataNavContext = createContext<DataNav | null>(null);
export function useDataNav(): DataNav | null {
  return useContext(DataNavContext);
}

/** The app selection a row (or a parsed row key) stands for. */
export function selectionForRow(row: DataRow | ParsedRowKey): Selection {
  if (row.kind === 'station') {
    return { type: 'station', stationRef: { section_id: row.sectionId, station_index: row.stationIndex } };
  }
  if (row.kind === 'shot') return { type: 'shot', sectionId: row.sectionId, shotId: row.shotId };
  return { type: 'section', sectionId: row.sectionId };
}

/**
 * The row an app selection points at (null for none / multi). A section maps
 * to its header when the view shows it, else to the section's first displayed
 * row (sorted, or the header dropped), else to its station 0 - an entity row,
 * which a focus request can reveal; a header never is, so returning one that
 * is not on screen would leave the request pending. Without a view, the header
 * counts as shown when the model has one.
 */
export function rowKeyForSelection(sel: Selection | null, model: DataModel | null, view?: DataView | null): string | null {
  if (!sel) return null;
  if (sel.type === 'station' && sel.stationRef) return stationRowKey(sel.stationRef.section_id, sel.stationRef.station_index);
  if (sel.type === 'shot' && sel.sectionId !== undefined && sel.shotId !== undefined) return shotRowKey(sel.sectionId, sel.shotId);
  if (sel.type === 'section' && sel.sectionId !== undefined) {
    const sectionId = sel.sectionId;
    const header = sectionRowKey(sectionId);
    if (view) {
      const first = view.rows.find((r) => r.sectionId === sectionId);
      if (first) return first.key;
    } else if (model?.rowByKey.has(header)) {
      return header;
    }
    return stationRowKey(sectionId, 0);
  }
  return null;
}

/** Centre (and zoom) the map on a row: a station at point zoom, a shot on its
 *  midpoint, a section fitted whole. */
function centerOn(row: DataRow | ParsedRowKey): void {
  const ui = useUIStore.getState();
  if (row.kind === 'station') ui.triggerCenterOnStation({ section_id: row.sectionId, station_index: row.stationIndex });
  else if (row.kind === 'shot') ui.triggerCenterOnShot(row.sectionId, row.shotId);
  else ui.triggerZoomToSection(row.sectionId);
}

/**
 * "Open in Survey": remember the row for the Back-to-Data chip, switch to
 * Survey, select the row there and zoom the map to it. Store-only, so the map
 * pane header (outside the workspace's providers) can call it too. Returns the
 * selection it set (null for a key that names nothing). `beforeSelect` sees
 * that selection BEFORE it is set: the selection subscribers run
 * synchronously inside `setSelection`, so a caller that must recognise its own
 * selection has to know it by then.
 */
export function openRowInSurvey(rowKey: string, beforeSelect?: (sel: Selection) => void): Selection | null {
  const parsed = parseRowKey(rowKey);
  if (!parsed) return null;
  useDataWorkspaceStore.getState().setReturnToData({ rowKey });
  const ui = useUIStore.getState();
  ui.setAppMode('survey');
  const sel = selectionForRow(parsed);
  beforeSelect?.(sel);
  setSource('data-table');
  ui.setSelection(sel);
  centerOn(parsed);
  return sel;
}

const FOLLOW_THROTTLE_MS = 160;
/** A pane that just opened is still being laid out and MapLibre resizes its
 *  canvas on the next observer tick; panning before that centres the row on
 *  the OLD (full-width) viewport, i.e. at the pane's right edge. Same settle
 *  idea as SECTION_FOCUS_REFIT_DELAY_MS in SingleSystemView. */
const PANE_OPEN_SETTLE_MS = 320;

export function useDataNavigation(
  model: DataModel | null,
  view: DataView | null,
  setPartners: (keys: readonly string[] | null) => void,
): DataNav {
  const viewRef = useRef(view);
  viewRef.current = view;
  const modelRef = useRef(model);
  modelRef.current = model;
  /** The last selection this workspace pushed to the app: map-side changes
   *  that are just that echo are ignored. */
  const ownSelection = useRef<Selection | null>(null);

  const select = useCallback((row: DataRow) => {
    const sel = selectionForRow(row);
    ownSelection.current = sel;
    setSource('data-table');
    useUIStore.getState().setSelection(sel);
  }, []);

  const goToRow = useCallback((rowKey: string, colKey?: DataColumnKey) => {
    const store = useDataWorkspaceStore.getState();
    const shown = viewRef.current?.rows.some((r) => r.key === rowKey) ?? false;
    const req = { rowKey, align: 'center' as const, ...(colKey ? { colKey } : {}) };
    if (shown) store.requestFocus(req);
    else store.revealAndFocus(req);
  }, []);

  const showOnMap = useCallback(
    (row: DataRow) => {
      useDataWorkspaceStore.getState().setMapPaneOpen(true);
      select(row);
      centerOn(row);
    },
    [select],
  );

  const openInSurvey = useCallback((row: DataRow) => {
    // The guard must be set before the selection fires the subscription below,
    // or it leaves a focus request that jumps the grid on the next entry.
    openRowInSurvey(row.key, (sel) => {
      ownSelection.current = sel;
    });
  }, []);

  const copyRowNames = useCallback((rows: readonly DataRow[]) => {
    const m = modelRef.current;
    if (!m || rows.length === 0) return;
    const text = rows.map((r) => rowName(m.ctx, r)).join('\n');
    void navigator.clipboard?.writeText(text).catch(() => undefined);
  }, []);

  const filterToSection = useCallback((sectionId: number) => {
    useDataWorkspaceStore.getState().setFilters({ sectionIds: [sectionId] });
  }, []);

  // ── Map follow: active row -> map selection + pan (throttled) ───────────
  const followTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pendingFollow = useRef<string | null>(null);
  const lastFollowAt = useRef(0);
  /** The row a map selection just focused: following it would echo the
   *  selection back (collapsing a multi-selection, turning a section into its
   *  station 0) and pan a map the user is already looking at. */
  const fromMap = useRef<string | null>(null);
  const runFollow = useCallback(() => {
    followTimer.current = null;
    const key = pendingFollow.current;
    pendingFollow.current = null;
    const m = modelRef.current;
    const row = key ? m?.rowByKey.get(key) : undefined;
    if (!row || !m) return;
    const echo = fromMap.current === key;
    fromMap.current = null;
    const ui = useUIStore.getState();
    if (echo || ui.multiSelection.length > 1 || rowKeyForSelection(ui.selection, m, viewRef.current) === key) return;
    lastFollowAt.current = Date.now();
    select(row);
    // Pan, never zoom: the user keeps the zoom they chose. A station pans to
    // the shot that leaves it (or arrives, at a section end).
    if (row.kind === 'shot') ui.triggerPanToShot(row.sectionId, row.shotId);
    else if (row.kind === 'station') {
      const key2 = shotKeyAt(m.ctx, row.sectionId, row.stationIndex) ?? shotKeyAt(m.ctx, row.sectionId, row.stationIndex - 1);
      const shot = key2 ? m.rowByKey.get(key2) : undefined;
      if (shot?.kind === 'shot') ui.triggerPanToShot(shot.sectionId, shot.shotId);
    }
  }, [select]);

  useEffect(() => {
    let prevKey = useDataWorkspaceStore.getState().activeRowKey;
    let prevOpen = useDataWorkspaceStore.getState().mapPaneOpen;
    let prevFollow = useDataWorkspaceStore.getState().mapFollow;
    const unsub = useDataWorkspaceStore.subscribe((s) => {
      // Follow fires when the active row changes, and also when the map pane
      // opens or Follow is switched on: the map then shows the row the user is
      // on instead of wherever it was last left.
      const opened = s.mapPaneOpen && !prevOpen;
      const followOn = s.mapFollow && !prevFollow;
      prevOpen = s.mapPaneOpen;
      prevFollow = s.mapFollow;
      if (s.activeRowKey === prevKey && !opened && !followOn) return;
      prevKey = s.activeRowKey;
      if (!s.mapPaneOpen || !s.mapFollow || !s.activeRowKey) return;
      pendingFollow.current = s.activeRowKey;
      // A just-opened pane is still resizing: pan once it has settled.
      if (opened) {
        if (followTimer.current) clearTimeout(followTimer.current);
        followTimer.current = setTimeout(runFollow, PANE_OPEN_SETTLE_MS);
        return;
      }
      if (followTimer.current) return;
      const wait = Math.max(0, FOLLOW_THROTTLE_MS - (Date.now() - lastFollowAt.current));
      followTimer.current = setTimeout(runFollow, wait);
    });
    return () => {
      unsub();
      if (followTimer.current) clearTimeout(followTimer.current);
    };
  }, [runFollow]);

  // ── Map selection -> focus its row ──────────────────────────────────────
  useEffect(() => {
    return useUIStore.subscribe((s, prev) => {
      if (s.selection === prev.selection || !s.selection) return;
      if (s.selection === ownSelection.current) return;
      // Data is being left (Open in Survey from the map pane header, or any
      // switch that selects on its way out): a request now would sit in the
      // store and jump the grid when Data is entered again.
      if (s.appMode !== 'data') return;
      const key = rowKeyForSelection(s.selection, modelRef.current, viewRef.current);
      if (!key || !modelRef.current?.rowByKey.has(key)) return;
      if (useDataWorkspaceStore.getState().activeRowKey === key) return;
      fromMap.current = key;
      goToRow(key);
    });
  }, [goToRow]);

  return useMemo(
    () => ({ goToRow, showOnMap, openInSurvey, copyRowNames, filterToSection, setHoveredPartners: setPartners }),
    [goToRow, showOnMap, openInSurvey, copyRowNames, filterToSection, setPartners],
  );
}
