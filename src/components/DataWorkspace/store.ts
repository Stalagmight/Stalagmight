/**
 * Data workspace UI state. Runtime only, except the "view" slice, which
 * `useDataWorkspaceViewPersistence` mirrors to `config.toml` `[panels]`.
 *
 * Survives mode switches (leaving Data and coming back keeps filters, search
 * and the active row); `resetForProject()` clears the transient slices when a
 * project is opened or closed.
 */
import { create } from 'zustand';
import { DATA_DEFAULT_HIDDEN } from './columnsMeta';
import type {
  DataColumnKey,
  FilterCondition,
  FilterState,
  FocusRequest,
  QuickFilters,
  SearchState,
  SortState,
  TintMode,
} from './types';

export const MAP_PANE_MIN_WIDTH = 360;
export const MAP_PANE_DEFAULT_WIDTH = 480;

export const EMPTY_QUICK_FILTERS: QuickFilters = {
  issues: false,
  comments: false,
  links: false,
  frozen: false,
  virtualShots: false,
};

export const EMPTY_FILTERS: FilterState = {
  rowTypes: 'all',
  sectionIds: null,
  quick: EMPTY_QUICK_FILTERS,
  conditions: [],
};

export const EMPTY_SEARCH: SearchState = {
  open: false,
  replaceOpen: false,
  query: '',
  replacement: '',
  matchCase: false,
  wholeWord: false,
  regex: false,
  rowScope: 'visible',
  columnScope: 'visible',
  columns: [],
  onlyMatchingRows: false,
};

/** The persisted part (mirrors `panels.data_workspace_*`). */
export interface DataWorkspaceViewState {
  columnOrder: DataColumnKey[];
  hiddenColumns: DataColumnKey[];
  columnWidths: Record<string, number>;
  tintMode: TintMode;
  mapPaneOpen: boolean;
  mapPaneWidth: number;
  showSectionHeaders: boolean;
}

export const DEFAULT_VIEW: DataWorkspaceViewState = {
  columnOrder: [],
  hiddenColumns: [...DATA_DEFAULT_HIDDEN],
  columnWidths: {},
  tintMode: 'section',
  mapPaneOpen: false,
  mapPaneWidth: MAP_PANE_DEFAULT_WIDTH,
  showSectionHeaders: true,
};

export interface DataWorkspaceState extends DataWorkspaceViewState {
  /** True once the persisted view has been applied from config. */
  viewLoaded: boolean;
  filters: FilterState;
  sort: SortState | null;
  /** Sections whose rows are collapsed under their header. */
  collapsedSections: number[];
  search: SearchState;
  /** Index into the current view's `matches` (-1 = none). */
  currentMatch: number;
  /** Row the grid's active cell is on (survives mode switches / rebuilds). */
  activeRowKey: string | null;
  /** One-shot request for the grid to scroll to + activate a row. */
  focusRequest: FocusRequest | null;
  /** Rows the user navigated to (a link chip, the map, Back to Data) that the
   *  filters / collapse would hide: shown anyway, marked as revealed, until
   *  the filters change or the user dismisses them. */
  revealRowKeys: string[];
  /** Follow the active row in the map pane (centre the map on it). */
  mapFollow: boolean;
  /** Set by "Open in Survey": Survey shows a "Back to Data" chip that
   *  returns here and re-focuses this row. */
  returnToData: { rowKey: string } | null;

  // view
  applyView: (view: Partial<DataWorkspaceViewState>, opts?: { fromConfig?: boolean }) => void;
  setColumnOrder: (order: DataColumnKey[]) => void;
  setHiddenColumns: (hidden: DataColumnKey[]) => void;
  setColumnWidths: (widths: Record<string, number>) => void;
  setTintMode: (mode: TintMode) => void;
  setMapPaneOpen: (open: boolean) => void;
  setMapPaneWidth: (width: number) => void;
  setShowSectionHeaders: (on: boolean) => void;
  // filters / sort
  setFilters: (patch: Partial<FilterState>) => void;
  setQuickFilter: (key: keyof QuickFilters, on: boolean) => void;
  addCondition: (c: FilterCondition) => void;
  updateCondition: (id: string, patch: Partial<FilterCondition>) => void;
  removeCondition: (id: string) => void;
  clearFilters: () => void;
  setSort: (sort: SortState | null) => void;
  toggleSectionCollapsed: (sectionId: number) => void;
  setCollapsedSections: (ids: number[]) => void;
  // search
  setSearch: (patch: Partial<SearchState>) => void;
  setCurrentMatch: (index: number) => void;
  closeSearch: () => void;
  // navigation
  setActiveRowKey: (key: string | null) => void;
  requestFocus: (req: Omit<FocusRequest, 'nonce'>) => void;
  /** Reveal a row the filters hide (see revealRowKeys) and focus it. */
  revealAndFocus: (req: Omit<FocusRequest, 'nonce'>) => void;
  clearRevealed: () => void;
  setMapFollow: (on: boolean) => void;
  setReturnToData: (v: { rowKey: string } | null) => void;
  /** Clear everything project-specific (filters, search, sort, collapse,
   *  active row, return chip). The persisted view is kept. */
  resetForProject: () => void;
}

let focusNonce = 0;

export const useDataWorkspaceStore = create<DataWorkspaceState>((set) => ({
  ...DEFAULT_VIEW,
  viewLoaded: false,
  filters: EMPTY_FILTERS,
  sort: null,
  collapsedSections: [],
  search: EMPTY_SEARCH,
  currentMatch: -1,
  activeRowKey: null,
  focusRequest: null,
  revealRowKeys: [],
  mapFollow: true,
  returnToData: null,

  applyView: (view, opts) => set({ ...view, ...(opts?.fromConfig ? { viewLoaded: true } : {}) }),
  setColumnOrder: (columnOrder) => set({ columnOrder }),
  setHiddenColumns: (hiddenColumns) => set({ hiddenColumns }),
  setColumnWidths: (columnWidths) => set({ columnWidths }),
  setTintMode: (tintMode) => set({ tintMode }),
  setMapPaneOpen: (mapPaneOpen) => set({ mapPaneOpen }),
  setMapPaneWidth: (w) => set({ mapPaneWidth: Math.max(MAP_PANE_MIN_WIDTH, Math.round(w)) }),
  setShowSectionHeaders: (showSectionHeaders) => set({ showSectionHeaders }),

  setFilters: (patch) => set((s) => ({ filters: { ...s.filters, ...patch } })),
  setQuickFilter: (key, on) => set((s) => ({ filters: { ...s.filters, quick: { ...s.filters.quick, [key]: on } } })),
  addCondition: (c) => set((s) => ({ filters: { ...s.filters, conditions: [...s.filters.conditions, c] } })),
  updateCondition: (id, patch) =>
    set((s) => ({
      filters: {
        ...s.filters,
        conditions: s.filters.conditions.map((c) => (c.id === id ? { ...c, ...patch } : c)),
      },
    })),
  removeCondition: (id) =>
    set((s) => ({ filters: { ...s.filters, conditions: s.filters.conditions.filter((c) => c.id !== id) } })),
  clearFilters: () => set({ filters: EMPTY_FILTERS }),
  setSort: (sort) => set({ sort }),
  toggleSectionCollapsed: (id) =>
    set((s) => ({
      collapsedSections: s.collapsedSections.includes(id)
        ? s.collapsedSections.filter((x) => x !== id)
        : [...s.collapsedSections, id],
    })),
  setCollapsedSections: (collapsedSections) => set({ collapsedSections }),

  setSearch: (patch) => set((s) => ({ search: { ...s.search, ...patch } })),
  setCurrentMatch: (currentMatch) => set({ currentMatch }),
  closeSearch: () => set((s) => ({ search: { ...s.search, open: false, replaceOpen: false }, currentMatch: -1 })),

  setActiveRowKey: (activeRowKey) => set({ activeRowKey }),
  requestFocus: (req) => set({ focusRequest: { ...req, nonce: ++focusNonce } }),
  revealAndFocus: (req) =>
    set((s) => ({
      revealRowKeys: s.revealRowKeys.includes(req.rowKey) ? s.revealRowKeys : [...s.revealRowKeys, req.rowKey],
      focusRequest: { ...req, nonce: ++focusNonce },
    })),
  clearRevealed: () => set({ revealRowKeys: [] }),
  setMapFollow: (mapFollow) => set({ mapFollow }),
  setReturnToData: (returnToData) => set({ returnToData }),
  resetForProject: () =>
    set({
      filters: EMPTY_FILTERS,
      sort: null,
      collapsedSections: [],
      search: EMPTY_SEARCH,
      currentMatch: -1,
      activeRowKey: null,
      focusRequest: null,
      revealRowKeys: [],
      returnToData: null,
    }),
}));
