/**
 * Mirror the store's persisted view slice (column order / hidden / widths, tint,
 * map pane open + width, section headers) to config.toml `[panels]`
 * `data_workspace_*` via `update_data_workspace_view`.
 *
 * Same contract as `useDataTableViewPersistence` (the four behaviours #357 paid
 * for): a sorted, normalized snapshot so applying the config never schedules a
 * save of state nobody touched; an optimistic publish into the cached config
 * before the IPC; rollback + toast when the IPC rejects, so a later change
 * retries; and a flush of a still-pending save on unmount.
 *
 * Two differences, because the view lives in a GLOBAL store that outlives this
 * hook (it survives mode switches) rather than in the host's own state:
 *
 *  - The "last known config" baseline is module state, not a ref, so a remount
 *    knows which store values are already saved and which are not.
 *  - Config is applied on EVERY arrival (Settings > Reload from disk, the
 *    re-sync after a save, the backend's own clamping), not once. It is merged
 *    per field: a field the config changed is applied unless the user has an
 *    unsaved local edit of that same field, which wins and is saved next.
 */
import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { invokeIpc } from '../../utils/ipc';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { stableWidths } from '../SingleSystemView/useDataTableViewPersistence';
import { DATA_COLUMN_BY_KEY, DATA_DEFAULT_HIDDEN } from './columnsMeta';
import {
  DEFAULT_VIEW,
  MAP_PANE_DEFAULT_WIDTH,
  MAP_PANE_MIN_WIDTH,
  useDataWorkspaceStore,
  type DataWorkspaceViewState,
} from './store';
import { TINT_MODES, type DataColumnKey, type TintMode } from './types';

/** Widest map pane the config keeps; mirrors the clamp in Rust
 *  `PanelsConfig::normalize` (pinned by configBoundsMirror.test.ts). */
export const MAP_PANE_MAX_WIDTH = 4000;

/** Debounce between the last view change and its save. */
export const DATA_WORKSPACE_VIEW_SAVE_DELAY_MS = 500;

type ViewKey = keyof DataWorkspaceViewState;
const VIEW_KEYS: readonly ViewKey[] = [
  'columnOrder',
  'hiddenColumns',
  'columnWidths',
  'tintMode',
  'mapPaneOpen',
  'mapPaneWidth',
  'showSectionHeaders',
];

type Snapshot = Record<ViewKey, string>;

const isColumnKey = (key: unknown): key is DataColumnKey =>
  typeof key === 'string' && DATA_COLUMN_BY_KEY.has(key as DataColumnKey);

function columnKeys(raw: unknown, fallback: readonly DataColumnKey[]): DataColumnKey[] {
  if (!Array.isArray(raw)) return [...fallback];
  return [...new Set(raw.filter(isColumnKey))];
}

function widths(raw: unknown): Record<string, number> {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return {};
  const out: Record<string, number> = {};
  for (const [key, value] of Object.entries(raw as Record<string, unknown>)) {
    if (typeof value === 'number' && Number.isFinite(value) && value > 0) out[key] = Math.round(value);
  }
  return out;
}

/**
 * A view as the config would hold it. Used on BOTH sides: on what arrives from
 * `get_config` (which, against an older backend, may lack these fields - only
 * the Rust `#[serde(default)]` guarantees them) and on what the store holds
 * (whose setters do not clamp everything), so the two snapshots compare like
 * for like. Unknown column keys and tint modes fall back; ABSENT hidden columns
 * mean the defaults, while an empty list means "show all".
 */
export function normalizeDataWorkspaceView(raw: Partial<Record<ViewKey, unknown>>): DataWorkspaceViewState {
  const width = raw.mapPaneWidth;
  return {
    columnOrder: columnKeys(raw.columnOrder, []),
    hiddenColumns: columnKeys(raw.hiddenColumns, DATA_DEFAULT_HIDDEN),
    columnWidths: widths(raw.columnWidths),
    tintMode: TINT_MODES.includes(raw.tintMode as TintMode) ? (raw.tintMode as TintMode) : DEFAULT_VIEW.tintMode,
    mapPaneOpen: typeof raw.mapPaneOpen === 'boolean' ? raw.mapPaneOpen : DEFAULT_VIEW.mapPaneOpen,
    mapPaneWidth:
      typeof width === 'number' && Number.isFinite(width)
        ? Math.min(MAP_PANE_MAX_WIDTH, Math.max(MAP_PANE_MIN_WIDTH, Math.round(width)))
        : MAP_PANE_DEFAULT_WIDTH,
    showSectionHeaders:
      typeof raw.showSectionHeaders === 'boolean' ? raw.showSectionHeaders : DEFAULT_VIEW.showSectionHeaders,
  };
}

/** The `panels.data_workspace_*` fields, keyed like the store's view slice. */
function viewFromPanels(panels: Record<string, unknown>): DataWorkspaceViewState {
  return normalizeDataWorkspaceView({
    columnOrder: panels.data_workspace_column_order,
    hiddenColumns: panels.data_workspace_hidden_columns,
    columnWidths: panels.data_workspace_column_widths,
    tintMode: panels.data_workspace_tint_mode,
    mapPaneOpen: panels.data_workspace_map_pane_open,
    mapPaneWidth: panels.data_workspace_map_pane_width,
    showSectionHeaders: panels.data_workspace_section_headers,
  });
}

/** One comparable string per field. The hidden list is a set and the widths a
 *  map, so both are sorted: the backend hands them back sorted (BTreeMap). */
function snapshotOf(view: DataWorkspaceViewState): Snapshot {
  return {
    columnOrder: JSON.stringify(view.columnOrder),
    hiddenColumns: JSON.stringify([...view.hiddenColumns].sort()),
    columnWidths: JSON.stringify(stableWidths(view.columnWidths)),
    tintMode: view.tintMode,
    mapPaneOpen: String(view.mapPaneOpen),
    mapPaneWidth: String(view.mapPaneWidth),
    showSectionHeaders: String(view.showSectionHeaders),
  };
}

const sameSnapshot = (a: Snapshot, b: Snapshot) => VIEW_KEYS.every((key) => a[key] === b[key]);

const storeView = () => normalizeDataWorkspaceView(useDataWorkspaceStore.getState());

/**
 * What the config is known to hold for each field: the view last applied from
 * it or last written to it. `null` until the config first arrives. Module
 * state, deliberately - see the file comment.
 */
let baseline: Snapshot | null = null;

/**
 * Saves whose IPC has not settled. While one is in flight, a config that
 * arrives (the optimistic publish, or an earlier save's re-sync racing a later
 * save) is not merged: it can predate the newest write and would flick the
 * store back to an older value for one round trip. The last save's own
 * re-sync, issued once nothing is in flight, brings the final state.
 */
let savesInFlight = 0;

/** Test-only: forget the baseline, as a fresh app start would. */
export function resetDataWorkspaceViewPersistenceForTests(): void {
  baseline = null;
  savesInFlight = 0;
}

/** Merge a config arrival into the store (see the file comment). */
function applyFromConfig(panels: Record<string, unknown>): void {
  if (baseline !== null && savesInFlight > 0) return;
  const incoming = viewFromPanels(panels);
  const next = snapshotOf(incoming);
  const store = useDataWorkspaceStore.getState();
  const current = snapshotOf(storeView());
  // Before the first arrival, the store's defaults stand in for "what the
  // config held", so a value the user set before config loaded is kept too.
  const known = baseline ?? snapshotOf(normalizeDataWorkspaceView(DEFAULT_VIEW));
  const patch: Partial<DataWorkspaceViewState> = {};
  for (const key of VIEW_KEYS) {
    // An unsaved local edit of this field: it wins, and the save path writes it.
    if (current[key] !== known[key]) continue;
    if (current[key] !== next[key]) (patch as Record<ViewKey, unknown>)[key] = incoming[key];
  }
  // Recorded BEFORE applying: `applyView` notifies the save subscription
  // synchronously, which must already see these values as saved.
  baseline = next;
  if (Object.keys(patch).length > 0 || !store.viewLoaded) store.applyView(patch, { fromConfig: true });
}

export function useDataWorkspaceViewPersistence(): void {
  const { t } = useTranslation();
  const tRef = useRef(t);
  useEffect(() => {
    tRef.current = t;
  }, [t]);

  const panels = useProjectStore((s) => s.config?.panels) as Record<string, unknown> | undefined;
  useEffect(() => {
    if (panels) applyFromConfig(panels);
  }, [panels]);

  // The save that is scheduled but has not fired yet, so unmount can flush it.
  const pendingSaveRef = useRef<(() => void) | null>(null);

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null;

    const save = () => {
      timer = null;
      pendingSaveRef.current = null;
      if (baseline === null) return;
      const view = storeView();
      const snapshot = snapshotOf(view);
      if (sameSnapshot(snapshot, baseline)) return;
      // Marked saved up front so a change mid-flight is diffed against what is
      // being written, but remembered: if the IPC rejects, the state never
      // reached disk and the next change must retry it.
      const previouslySaved = baseline;
      baseline = snapshot;
      const hiddenColumns = [...view.hiddenColumns].sort();
      // Publish into the IN-MEMORY config before the IPC (see
      // useDataTableViewPersistence): a remount that races the flush must not
      // see the pre-save values. `setState`, not the `setConfig` action, which
      // would also drop the closure-adjusted geometry for no reason.
      const config = useProjectStore.getState().config;
      if (config) {
        useProjectStore.setState({
          config: {
            ...config,
            panels: {
              ...config.panels,
              data_workspace_column_order: view.columnOrder,
              data_workspace_hidden_columns: hiddenColumns,
              data_workspace_column_widths: view.columnWidths,
              data_workspace_tint_mode: view.tintMode,
              data_workspace_map_pane_open: view.mapPaneOpen,
              data_workspace_map_pane_width: view.mapPaneWidth,
              data_workspace_section_headers: view.showSectionHeaders,
            },
          },
        });
      }
      savesInFlight += 1;
      void invokeIpc(
        'update_data_workspace_view',
        {
          columnOrder: view.columnOrder,
          hiddenColumns,
          columnWidths: view.columnWidths,
          tintMode: view.tintMode,
          mapPaneOpen: view.mapPaneOpen,
          mapPaneWidth: view.mapPaneWidth,
          sectionHeaders: view.showSectionHeaders,
        },
        { purpose: 'persist Data workspace view' },
      ).then(
        () => {
          savesInFlight -= 1;
          // Re-sync the cached config with what the backend normalized and stored.
          void useProjectStore.getState().loadConfig();
        },
        (e: unknown) => {
          savesInFlight -= 1;
          if (baseline && sameSnapshot(baseline, snapshot)) {
            baseline = previouslySaved;
            // Undo the optimistic publish from DISK. The store keeps the user's
            // values (they are now unsaved local edits again), so nothing on
            // screen reverts, and the next change retries the write.
            void useProjectStore.getState().loadConfig();
          }
          console.error('Failed to save Data workspace view:', e);
          useUIStore
            .getState()
            .showToast(tRef.current('dataTable.workspaceViewSaveFailed'), TOAST_KEYS.DATA_WORKSPACE_VIEW_SAVE, 'error');
        },
      );
    };

    const schedule = () => {
      if (timer !== null) clearTimeout(timer);
      timer = null;
      pendingSaveRef.current = null;
      if (baseline === null || sameSnapshot(snapshotOf(storeView()), baseline)) return;
      pendingSaveRef.current = save;
      timer = setTimeout(save, DATA_WORKSPACE_VIEW_SAVE_DELAY_MS);
    };

    const unsubscribe = useDataWorkspaceStore.subscribe((state, prev) => {
      // `viewLoaded` flipping is the config's first arrival: a value the user
      // changed before it (kept by `applyFromConfig`) is saved from there.
      if (state.viewLoaded !== prev.viewLoaded || VIEW_KEYS.some((key) => state[key] !== prev[key])) {
        schedule();
      }
    });
    // A value left unsaved by an earlier mount (its save failed) is retried.
    schedule();
    return () => {
      unsubscribe();
      if (timer !== null) clearTimeout(timer);
      // Flush instead of dropping: leaving Data mode (or closing the app view)
      // inside the debounce window must still persist the last change.
      pendingSaveRef.current?.();
    };
  }, []);
}
