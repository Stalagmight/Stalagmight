import { create } from 'zustand';
import { invokeIpc } from '../utils/ipc';
import { clampRound } from '../utils/clamp';
import {
  PANEL_SIZE_BOUNDS,
  WIZARD_MAP_PANEL_DEFAULT_WIDTH,
} from '../components/SingleSystemView/layoutConstants';
import { trackAction, checkDialogCancel, trackDialogComplete, trackDisplayState, setDwellState, advanceProjectSeq, trackNav, trackPanelToggle, trackPanelMode, trackSelect, trackSelectAdd, clearProjectActionSource, isPersistSuppressed } from '../utils/telemetry';
import type { Coordinates, DistanceField, PathShot, SectionId, ShotId, StationRef, SectionData } from '../types/project';

export interface CommentTarget {
  type: 'station' | 'shot';
  stationRef?: StationRef;
  sectionId?: SectionId;
  shotId?: ShotId;
  screenPos: { x: number; y: number };
  currentComment: string;
}
import type { AppearanceConfig, PanelsConfig, SyntacticHighlightingConfig, SyntacticHighlightLevel } from '../types/config';
import type { DisplaySettings } from '../types/loading';
import type { Person } from '../types/ipc.generated';
import {
  DISPLAY_DEFAULTS,
  isWarningsVisitActive,
  selectEffectiveColorMode,
  type WarningsVisitOverride,
} from './displaySelectors';
import { useGasPlanStore } from './gasPlanStore';
import { useExportStore } from './exportStore';
import { useColorScaleStore } from './colorScaleStore';

// Import tool utilities for internal use and re-export for convenience
import { NON_INTERACTIVE_TOOLS, isNonInteractiveTool } from '../utils/toolUtils';
import { MAX_VISIBLE_TOASTS, clampToastCapacity } from '../utils/toastCapacity';
import { getGroupForTool } from '../utils/toolGroups';
export { NON_INTERACTIVE_TOOLS, isNonInteractiveTool };

export type ResolvedTheme = 'dark' | 'light' | 'cave' | 'dracula' | 'one_dark' | 'solarized_light' | 'catppuccin_latte' | 'nord' | 'catppuccin_mocha';

export type ViewType = 'global' | 'single';

/**
 * Cave geometry display mode.
 * - `measured`: raw survey geometry (the source of truth).
 * - `closureAdjusted`: loop-closure-corrected geometry, derived non-destructively
 *   on the backend. This view is read-only - edits are made in `measured`.
 */
export type GeometryMode = 'measured' | 'closureAdjusted';

/**
 * Top-level workspace mode (#38 follow-up, the workspace-mode reframe). Survey, Data and
 * Plan are picked from the top-middle carousel when a project is open; Export is entered
 * from the project menu (or Ctrl+E) and left with its Close export button:
 *  - `survey` - the editable survey workspace: map, Sections, Details, docked data table.
 *  - `data`   - the editable Data workspace: one table of every section's stations and
 *               shots, with an optional map pane beside it.
 *  - `plan`   - the gas planner: read-only closure map + gas overlay, Plan-setup
 *               sidebar, tabbed bottom tables.
 *  - `export` - read-only closure map + the export panel and preview.
 * Survey and Data edit ({@link isEditableMode}); Plan and Export are read-only and force
 * the closure-adjusted map. Runtime only (never persisted) and meaningful only in
 * single-project view.
 */
export type AppMode = 'survey' | 'data' | 'plan' | 'export';
export const APP_MODE_DEFAULT: AppMode = 'survey';

/** Geometry mode used before any saved preference exists (first run). */
export const GEOMETRY_MODE_DEFAULT: GeometryMode = 'measured';

/**
 * The two workspaces that edit the survey: Survey and Data. Plan and Export are
 * read-only. Mode entry/exit snapshots (see `setAppMode`), display persistence,
 * undo/redo and the Import button all key on this, so a new editable mode is one
 * line here rather than a hunt for every `appMode === 'survey'`.
 */
export function isEditableMode(mode: AppMode): boolean {
  return mode === 'survey' || mode === 'data';
}

/**
 * The survey map is read-only whenever the closure-adjusted geometry is shown OR
 * we're in a read-only workspace (Plan, Export). Both terms are load-bearing:
 * the read-only modes force `closureAdjusted`, but that auto-reverts to `measured`
 * on a project with no eligible closures (see MapCanvas), so `appMode` is the robust
 * authority - a `geometryMode`-only check leaks mutations (import, undo) into the
 * read-only workspaces (#243, the #225 bug class). One predicate so the many
 * gate sites can't drift. False only in Survey or Data on the measured geometry.
 */
export function isMapReadOnly(state: Pick<UIStore, 'geometryMode' | 'appMode'>): boolean {
  return state.geometryMode === 'closureAdjusted' || !isEditableMode(state.appMode);
}

/**
 * Whether the map draws the "Closures Computed View is read-only" notice: only in an
 * editable workspace (the read-only modes force that geometry, so the notice would
 * explain nothing), and only when no placement or SpeleoDB badge already explains it.
 */
export function showsClosureAdjustedNotice(args: {
  geometryMode: GeometryMode;
  appMode: AppMode;
  placementMode: boolean;
  speleoReadOnly: boolean;
}): boolean {
  return (
    args.geometryMode === 'closureAdjusted' &&
    !args.placementMode &&
    isEditableMode(args.appMode) &&
    !args.speleoReadOnly
  );
}

export type DmpImportWizardPhase = 'init' | 'suggesting' | 'manual_placing' | 'placing' | 'done';

export type DetailPanelTab = 'details' | 'warnings';

// The single-select colour mode lives in ./colorMode (shared with the display
// selectors without an import cycle); re-exported so existing imports keep working.
export { colorModeOf, colorModeFlags, type ColorMode, type ColorOverlayFlags } from './colorMode';
import { colorModeFlags, type ColorMode } from './colorMode';
import { visitColorPatch, visitEndPatch, visitErrorsPatch, visitToggleColorPatch } from './warningsVisit';

/** Reveal behavior for a side/data panel.
 *  - `pin`: click to open; stays open (reflows the map) until closed.
 *  - `hover`: pops open on hover (floats over the map); hides on mouse-leave. */
export type PanelMode = 'hover' | 'pin';

/** Identifies one of the three in-project panels (matches telemetry PanelName). */
export type PanelId = 'sections' | 'detail' | 'data-table';

/**
 * A panel whose SIZE persists. Wider than {@link PanelId}: the import wizards'
 * map column (#404) is dragged and remembered like the others, but it has no
 * open/hover state - it is shown for as long as its wizard step is - so it is
 * not a `PanelId` and takes no part in `setPanelOpen`/`setPanelMode`.
 */
export type SizedPanelId = PanelId | 'wizard-map';

/**
 * A transient "ghost" geometry overlay shown on the map (in survey meters),
 * used to preview a proposed edit before it is committed - e.g. as the user
 * tries different warning-resolution options (issue #57). Structurally matches
 * the map's GhostSectionData so MapCanvas can hand it straight to the survey
 * layer's setGhostSection.
 */
export interface GeometryPreview {
  shots: Array<{ from: { x: number; y: number }; to: { x: number; y: number } }>;
  stations: Array<{ x: number; y: number }>;
  color: string;
  mergeStation?: { x: number; y: number };
}

export type ToolType =
  | 'select'
  | 'pan'
  | 'zoom'
  | 'merge'
  | 'measure'
  | 'move_station'
  | 'move_segment'
  | 'move_shot'
  | 'comment'
  | 'move_comment';

/**
 * One endpoint of the ruler/measure tool (#26). A raw map `point` yields only the
 * direct (straight-line) distance; a `station` or `shotPoint` also enables the
 * along-cave-line distance + path tint. `coords` is the 3D position (meters) used
 * for the direct distance; for a `shotPoint` it's interpolated along the shot.
 */
export type MeasureEndpoint =
  | { kind: 'point'; x: number; y: number; z: number }
  | { kind: 'station'; section_id: SectionId; station_index: number; coords: Coordinates }
  | { kind: 'shotPoint'; section_id: SectionId; shot_id: ShotId; t: number; coords: Coordinates };

/** Resolved along-cave-line query result for the current measurement. */
export interface MeasureAlongLine {
  /** Shortest along-cave-line distance, or null when the endpoints are disconnected. */
  alongLine: number | null;
  /** Shots traversed on the path (for tinting). */
  pathShots: PathShot[];
  /** Stations traversed on the path (tinted alongside the shots). */
  pathStations: StationRef[];
}

/** Live shot-snap candidate while the measure tool hovers a shot (not a station).
 *  The on-shot 3D coords are derived from `t` at click time (geometry-mode aware). */
export interface MeasureSnapShot {
  section_id: SectionId;
  shot_id: ShotId;
  t: number;
}

/**
 * Full reset of all ruler/measure state - the single funnel spread by
 * `clearMeasure` and every other reset trigger (tool switch, dialog open,
 * geometry-mode change, project load…).
 */
const MEASURE_CLEARED = {
  measureStart: null,
  measureEnd: null,
  measureDirect: null,
  measureAlongLine: null,
  measureAlongLinePending: false,
  measureSnapShot: null,
};

/**
 * Placement result returned when placement mode completes.
 */
export interface PlacementResult {
  /** Final cursor position in map coordinates (meters) */
  position: Coordinates;
  /** If snapped to an existing station, its reference */
  snappedToStation: StationRef | null;
}

/**
 * Placement mode state for manually placing a newly imported section on the map.
 * The section preview follows the cursor until the user clicks to confirm placement.
 */
/** Additional section to render alongside the primary placement section. */
export interface AdditionalPlacementSection {
  sectionData: SectionData;
  color: string;
  offsetX: number;  // meters relative to primary section station 0
  offsetY: number;
}

export interface PlacementModeState {
  /** The section data to be created (not yet in the project) */
  sectionData: SectionData;
  /** Which station index of the new section aligns to cursor (0 = first, N = last) */
  alignStationIndex: number;
  /** Current cursor position in map coordinates (meters) */
  cursorPosition: Coordinates | null;
  /** Snapped station reference (if cursor is near an existing station) */
  snappedToStation: StationRef | null;
  /** Callback when placement is confirmed */
  onComplete?: (result: PlacementResult) => void;
  /** Callback when placement is canceled */
  onCancel?: () => void;
  /** Additional sections to render alongside primary (for segment placement) */
  additionalSections?: AdditionalPlacementSection[];
}

export interface Selection {
  type: 'station' | 'shot' | 'section';
  stationRef?: StationRef;
  sectionId?: SectionId;
  shotId?: ShotId;
}

// Helper to compare selections
export function selectionsEqual(a: Selection, b: Selection): boolean {
  if (a.type !== b.type) return false;
  if (a.type === 'station' && b.type === 'station') {
    return (
      a.stationRef?.section_id === b.stationRef?.section_id &&
      a.stationRef?.station_index === b.stationRef?.station_index
    );
  }
  if (a.type === 'shot' && b.type === 'shot') {
    return a.sectionId === b.sectionId && a.shotId === b.shotId;
  }
  if (a.type === 'section' && b.type === 'section') {
    return a.sectionId === b.sectionId;
  }
  return false;
}

/**
 * Toast severity - `error` (failed, won't self-heal), `warning` (succeeded with
 * caveats, or blocked), `success` (succeeded, result off-screen/non-obvious), or
 * `info` (FYI, no outcome attached - rare). Mandatory at every callsite; see
 * docs/notifications.md. The runtime `showToast` implementation still defaults
 * to `'info'` for safety, but the type below forbids relying on it.
 */
export type ToastKind = 'info' | 'success' | 'warning' | 'error';

/** One entry in the toast stack. Ordered oldest-first in `UIStore.toasts`. */
export interface ToastItem {
  /** Process-unique, monotonically increasing. React key and timer identity. */
  id: number;
  message: string;
  /** De-duplication key. At most one non-leaving toast per key is on screen. */
  key: string;
  kind: ToastKind;
  /**
   * When this toast was last pushed/refreshed, on the MONOTONIC clock
   * (`performance.now()`), which is the only thing the de-duplication window
   * below is measured against.
   *
   * Not wall-clock, which it used to be: `Date.now()` can step backwards (an NTP
   * correction, a manual clock change), and a negative age passes `< WINDOW_MS`
   * just as happily as a fresh one - so every repeat of a key would collapse into
   * the toast already on screen for as long as the skew lasted, however far apart
   * the events really were.
   */
  refreshedAt: number;
  /**
   * Bumped every time the same key refreshes this toast. ToastStack keys its
   * dwell/fade clock on (id, revision), so a refresh restarts the timer without
   * remounting the element (which would replay the enter animation).
   */
  revision: number;
  /**
   * True once the toast is on its way out (evicted by the stack cap, or the user
   * hit dismiss). Leaving toasts are excluded from de-duplication and from the
   * visible-count cap, and ToastStack drops them once their exit finishes.
   */
  leaving: boolean;
}

/**
 * Hard ceiling on how many toasts stay fully live at once. A push past the
 * effective cap evicts the OLDEST (marks it leaving, so it animates out instead
 * of blinking away).
 *
 * The EFFECTIVE cap is usually smaller: it is "whatever fits in 20% of the
 * window height", which only the DOM can answer, so ToastStack measures it and
 * feeds it back through `setToastCapacity`. Until something reports a real
 * measurement the store uses this maximum, which keeps it deterministic on its
 * own (and correct in unit tests of unrelated components, where no stack is
 * mounted to measure anything).
 */
export { MAX_VISIBLE_TOASTS };

/**
 * Hard ceiling on the array, live + leaving. Exit animations are driven by
 * ToastStack, so if it is not mounted (unit tests of unrelated components)
 * nothing would ever reap the leaving entries. Anything past this is dropped
 * outright rather than allowed to grow without bound.
 */
const MAX_TOTAL_TOASTS = MAX_VISIBLE_TOASTS + 4;

/**
 * Apply the live-toast cap: mark the oldest overflow `leaving` (so it animates
 * away rather than blinking out), then trim the array to its hard ceiling.
 */
function capToasts(next: ToastItem[], capacity: number): ToastItem[] {
  const live = next.filter((t) => !t.leaving);
  const overflow = live.length - capacity;
  const evicted = overflow > 0 ? new Set(live.slice(0, overflow).map((t) => t.id)) : null;
  const marked = evicted ? next.map((t) => (evicted.has(t.id) ? { ...t, leaving: true } : t)) : next;
  return marked.slice(-MAX_TOTAL_TOASTS);
}

/**
 * How long after a toast appears a repeat of its key still counts as the SAME
 * event rather than a new one.
 *
 * Deliberately far shorter than a toast's visible life. Matching for the whole
 * lifetime turned each key into a single slot, so repeating an action - lock,
 * unlock, lock, unlock - had every new toast steal the previous one of its kind
 * and the stack could never grow past one per key. Inside a second a repeat is a
 * burst (a double-click, a loop firing the same warning) where collapsing is
 * both wanted and imperceptible; past that it is something the user did again
 * and it earns its own line.
 */
export const TOAST_DEDUP_WINDOW_MS = 1000;

/** Source of `ToastItem.id`. Module-scoped so ids never repeat within a session. */
let nextToastId = 1;

/** The people a section was last created or imported with, this session. */
export interface LastUsedCredit {
  surveyors: Person[];
  explorers: Person[];
}

export interface UIStore {
  // View state
  view: ViewType;
  setView: (view: ViewType) => void;

  // Tool state
  activeTool: ToolType;
  lastSubtoolByGroup: Record<string, ToolType>;
  setActiveTool: (tool: ToolType) => void;
  resetToolTransientsForProjectSwitch: () => void;

  // Selection state
  selection: Selection | null;
  multiSelection: Selection[];
  setSelection: (selection: Selection | null) => void;
  addToSelection: (selection: Selection) => void;
  removeFromSelection: (selection: Selection) => void;
  toggleSelection: (selection: Selection) => void;
  clearSelection: () => void;
  clearSelectionByType: (type: 'station' | 'shot' | 'section') => void;
  filterSelection: (predicate: (s: Selection) => boolean) => void;
  selectAll: (selections: Selection[]) => void;
  isSelected: (selection: Selection) => boolean;

  // Tool-specific state
  mergeStationA: StationRef | null;
  mergeStationB: StationRef | null;
  setMergeStationA: (ref: StationRef | null) => void;
  setMergeStationB: (ref: StationRef | null) => void;
  clearMergeStations: () => void;

  measureStart: MeasureEndpoint | null;
  measureEnd: MeasureEndpoint | null;
  /** Direct (3D straight-line) distance + depth diff between the two endpoints. */
  measureDirect: { distance: number; depthDiff: number } | null;
  /** Along-cave-line result (null = not computed / not applicable). */
  measureAlongLine: MeasureAlongLine | null;
  /** True while the along-line IPC query is in flight. */
  measureAlongLinePending: boolean;
  /** Live shot-snap candidate (when the measure cursor is over a shot, not a station). */
  measureSnapShot: MeasureSnapShot | null;
  setMeasureStart: (point: MeasureEndpoint | null) => void;
  setMeasureEnd: (point: MeasureEndpoint | null) => void;
  setMeasureDirect: (direct: { distance: number; depthDiff: number } | null) => void;
  setMeasureAlongLine: (result: MeasureAlongLine | null) => void;
  setMeasureAlongLinePending: (pending: boolean) => void;
  setMeasureSnapShot: (snap: MeasureSnapShot | null) => void;
  /** Reset every piece of measure state (endpoints, results, tint, snap). */
  clearMeasure: () => void;

  // Left workspace navigation is session-only and never changes lock ownership.
  leftWorkspace: 'sections' | 'speleodb';
  surveyLeftWorkspace: 'sections' | 'speleodb';
  speleoWorkspaceSection: 'history' | null;
  speleoRevealNonce: number;
  speleoOutsideSurvey: boolean;
  leftWorkspaceVisible: boolean;
  selectLeftWorkspace: (tab: 'sections' | 'speleodb') => void;
  openSpeleoWorkspace: (section?: 'history') => void;
  resetLeftWorkspace: () => void;

  // Panel state
  sectionsPanelOpen: boolean;
  sectionsPanelMode: PanelMode;
  sectionsPanelWidth: number;
  detailPanelOpen: boolean;
  detailPanelMode: PanelMode;
  detailPanelWidth: number;
  detailPanelTab: DetailPanelTab;
  dataTableOpen: boolean;
  dataTableMode: PanelMode;
  dataTableHeight: number;
  /** Width of the import wizards' map column (#404). Lives here rather than in
   *  each wizard's local state so a drag in one is honoured by the next, and so
   *  `initPanels` can seed it from the persisted config. */
  wizardMapPanelWidth: number;
  /** Bumped to transiently reveal the Details panel: in hover mode SingleSystemView
   *  floats it open (like a hover, with an auto-close) instead of pinning, so a
   *  programmatic reveal (e.g. jump-to-warning) never clobbers the user's persisted
   *  panel mode. Session-only, never persisted. */
  detailRevealNonce: number;
  /** Reveal the Details panel (e.g. the "Project Details" menu item) WITHOUT
   *  clobbering the user's persisted reveal mode: pinned → open it; hover → float
   *  it open transiently via detailRevealNonce. Unlike toggleDetailPanel this never
   *  forces 'pin' or persists a mode change just to view content. */
  revealDetailPanel: () => void;
  toggleSectionsPanel: () => void;
  toggleDetailPanel: () => void;
  toggleDataTable: () => void;
  /** Set a panel's open state (pinned). Tracks open/close + persists. */
  setPanelOpen: (panel: PanelId, open: boolean) => void;
  /** Set a panel's reveal mode. Pin → open; un-pin (hover) → closed at rest. */
  setPanelMode: (panel: PanelId, mode: PanelMode) => void;
  setDetailPanelTab: (tab: DetailPanelTab) => void;
  setSectionsPanelWidth: (width: number) => void;
  setDetailPanelWidth: (width: number) => void;
  setDataTableHeight: (height: number) => void;
  /** Live width of the import wizards' map column (#404). One value for every
   *  wizard that shows one, so the map does not resize as the user steps
   *  between them; persisted on divider release by `WizardMapPanel`. */
  setWizardMapPanelWidth: (width: number) => void;
  initPanels: (panels: PanelsConfig) => void;
  savePanelSizes: (panel: SizedPanelId, size: number) => Promise<void>;
  /** Persist the full per-panel reveal layout (open + mode) globally. */
  savePanelLayout: () => Promise<void>;

  // Data-table fullscreen mode: the table overlays the whole window below the
  // toolbar. Session-only - never persisted, so the app always starts windowed.
  dataTableFullScreen: boolean;
  /** Raw sqlite table browsed read-only while fullscreen (null = the normal
   *  Sections-backed view). Only reachable from the fullscreen toolbar. */
  rawTableSelection: string | null;
  enterDataTableFullScreen: () => void;
  exitDataTableFullScreen: () => void;
  setRawTableSelection: (table: string | null) => void;

  // Section focus mode (#50): double-clicking a section collapses both side
  // panels and opens the data table so the screen is dedicated to one section.
  // Exiting restores the layout captured at entry.
  focusedSectionId: SectionId | null;
  enterSectionFocus: (sectionId: SectionId, dataTableHeight: number) => void;
  exitSectionFocus: () => void;
  /** While focused, follow the selected section so the heading tracks the table
   *  (does not re-snapshot layout or re-fit the map). */
  setFocusedSection: (sectionId: SectionId) => void;

  // Map state
  /** Camera zoom, NOT written per frame. Two writers only, both in MapCanvas:
   *  the 'zoomend' handler (once per gesture) and the map-teardown cleanup,
   *  which captures the final zoom when we unmount mid-gesture and 'zoomend'
   *  never fires. The only reader is MapCanvas restoring the initial camera when
   *  the map is recreated. It is therefore stale mid-gesture - a live-zoom
   *  consumer must subscribe to the map (`map.on('zoom')`), not to this field. */
  mapZoom: number;
  setMapZoom: (zoom: number) => void;
  requestFitToBounds: false | 'animate' | 'snap';
  triggerFitToBounds: (options?: { snap?: boolean }) => void;
  clearFitToBoundsRequest: () => void;
  requestZoomToSection: SectionId | null;
  triggerZoomToSection: (sectionId: SectionId) => void;
  clearZoomToSectionRequest: () => void;
  requestPanToShot: { sectionId: SectionId; shotId: ShotId } | null;
  triggerPanToShot: (sectionId: SectionId, shotId: ShotId) => void;
  clearPanToShotRequest: () => void;
  requestCenterOnStation: StationRef | null;
  triggerCenterOnStation: (ref: StationRef) => void;
  clearCenterOnStationRequest: () => void;
  requestCenterOnShot: { sectionId: SectionId; shotId: ShotId } | null;
  triggerCenterOnShot: (sectionId: SectionId, shotId: ShotId) => void;
  clearCenterOnShotRequest: () => void;

  /** Transient ghost-geometry preview on the map (null = none). */
  geometryPreview: GeometryPreview | null;
  setGeometryPreview: (preview: GeometryPreview | null) => void;

  /** Request to open the Warnings tab and start resolving a shot's issue
   *  (e.g. clicking the warning indicator in the data table). */
  requestResolveWarning: { sectionId: SectionId; shotId: ShotId } | null;
  triggerResolveWarning: (sectionId: SectionId, shotId: ShotId) => void;
  clearResolveWarningRequest: () => void;
  /** Show the project view's Details panel on the Warnings tab, listing every
   *  issue, without starting to resolve one (the SpeleoDB upload dialog's
   *  "Resolve now"). Leaves Plan/Export for Survey, where that panel lives. */
  openWarningsTab: () => void;

  /** Progressive zoom-in centered on a shot: each request recenters and zooms
   *  one step closer (used by the resolve view's zoom button, #57). */
  requestZoomToShot: { sectionId: SectionId; shotId: ShotId } | null;
  triggerZoomToShot: (sectionId: SectionId, shotId: ShotId) => void;
  clearZoomToShotRequest: () => void;

  // Display toggles
  showStations: boolean;
  showShots: boolean;
  showSectionNames: boolean;
  showStationLabels: boolean;
  /** Station names as the station labels; exclusive with showStationLabels. */
  showStationNames: boolean;
  showShotData: boolean;
  /** The "Errors" labels: each shot's reading deltas and each loop closure's
   *  error. Persisted per project like showShotData. */
  showShotErrors: boolean;
  /** Mirror of `config.map.highlight_warnings_on_tab_switch`: whether the
   *  Warnings tab turns on Survey Issues + Errors while it is on screen. */
  warningsAutoDisplay: boolean;
  setWarningsAutoDisplay: (on: boolean) => void;
  /** Changes made during the current Warnings-tab visit (see
   *  `displaySelectors.isWarningsVisitActive`); null outside a visit and until
   *  the user changes something. Never persisted. */
  warningsVisitOverride: WarningsVisitOverride | null;
  showReferenceShots: boolean;
  /** #27: jumps hide independently of references. */
  showJumps: boolean;
  showZeroDatum: boolean;
  showGrid: boolean;
  showSatellite: boolean;
  showDepthGradient: boolean;
  showStationComments: boolean;
  showShotComments: boolean;
  showDevStats: boolean;
  showMemoryStats: boolean;
  showMapStats: boolean;
  showZoomStats: boolean;
  gridSpacing: number;
  toggleDisplay: (key: DisplayToggleKey) => void;
  setGridSpacing: (spacing: number) => void;
  saveDisplaySetting: (key: DisplayToggleKey, value: boolean) => Promise<void>;
  resetDisplaySettings: () => void;
  /** Apply display settings from backend (merges with existing state using ??) */
  applyDisplaySettings: (settings: DisplaySettings) => void;

  // Distance-gradient overlay (#34). Runtime-only - deliberately NOT a
  // DisplayToggleKey: it's a transient analytical overlay (defaults to the
  // entrance each session), so it is never persisted via saveDisplaySetting.
  showDistanceGradient: boolean;
  /** Chosen origin station; `null` = the zero-datum entrance default. */
  distanceGradientOrigin: StationRef | null;
  /** Latest per-station distance field from the backend (null until fetched). */
  distanceField: DistanceField | null;
  /** Toggle the overlay; turning it off clears the origin + cached field. */
  toggleDistanceGradient: () => void;
  setDistanceGradientOrigin: (ref: StationRef | null) => void;
  setDistanceField: (field: DistanceField | null) => void;

  // Top-level workspace mode (#38 follow-up): Survey / Data / Plan / Export. Runtime-only
  // (never persisted), meaningful only in single-project view. `plan` subsumes the
  // former gas sub-mode - the survey layer, the carousel, and the panel swaps all read
  // `appMode`. Plan and Export force the read-only closure-adjusted map (see setAppMode).
  appMode: AppMode;
  /** Switch workspace mode. Leaving an editable mode (Survey, Data) for a read-only one
   *  (Plan, Export) snapshots the geometry mode, display flags and docks, then forces the
   *  closure-adjusted map and suppresses the survey color overlays; coming back to an
   *  editable mode restores the snapshot. Survey <-> Data keeps everything as it is.
   *  The gas store survives every switch (#244). */
  setAppMode: (mode: AppMode) => void;
  /** Idempotent "leave a read-only workspace" - used by the setView() cleanup funnel (and
   *  the color-overlay actions) so a stale Plan or Export workspace can't outlive a
   *  project/view switch or coexist with a survey overlay. Returns to Survey; a no-op in
   *  any editable mode (Survey, Data), so picking a colour in Data's map pane stays in Data. */
  exitGasPlan: () => void;

  // Issue-severity coloring overlay (#22). Runtime-only - like
  // showDistanceGradient it is NOT a DisplayToggleKey and is never persisted
  // (off on every load; the Warnings-tab auto-switch drives it transiently).
  showIssueSeverity: boolean;
  /** Toggle issue-severity coloring (mutually exclusive with depth/distance). */
  toggleIssueSeverity: () => void;
  /** Set issue-severity coloring explicitly (used by the Warnings-tab
   *  auto-switch). Enabling turns off both gradients. */
  setIssueSeverity: (on: boolean) => void;
  /** Select the active coloring mode (single-select over the three mutually-
   *  exclusive overlays). Authoritative: it writes the whole flag set from
   *  {@link colorModeFlags}, so it lands on the requested mode from ANY starting
   *  combination, while depth keeps its persistence/telemetry. Read the current
   *  mode via {@link colorModeOf}. */
  setColorMode: (mode: ColorMode) => void;

  // Geometry display mode (measured vs non-destructive closure-adjusted)
  geometryMode: GeometryMode;
  /** Switch the geometry display mode; persists across sessions (remember last used). */
  setGeometryMode: (mode: GeometryMode, source?: string) => void;

  // Hover state
  hoveredStation: StationRef | null;
  hoveredShot: { sectionId: SectionId; shotId: ShotId } | null;
  hoveredSectionId: SectionId | null;
  setHoveredStation: (ref: StationRef | null) => void;
  setHoveredShot: (shot: { sectionId: SectionId; shotId: ShotId } | null) => void;
  setHoveredSectionId: (sectionId: SectionId | null) => void;

  // Toast stack (oldest first, newest last - see src/components/common/ToastStack.tsx)
  toasts: ToastItem[];
  /**
   * How many toasts may be live at once. Reported by ToastStack from measured
   * geometry (see `fitToastCapacity`); defaults to `MAX_VISIBLE_TOASTS` so the
   * store behaves sanely with no stack mounted.
   */
  toastCapacity: number;
  /**
   * Push a toast onto the stack. `kind` is optional and defaults to `'info'`, so
   * every pre-existing two-argument call site keeps the neutral look it had.
   *
   * `key` is the de-duplication key: if a toast with the same key is already on
   * screen (and not already leaving), it is REFRESHED in place - newest wording
   * and kind win, its timer restarts, and it moves to the newest slot - instead
   * of stacking a duplicate.
   */
  showToast: (message: string, key: string, kind: ToastKind) => void;
  /**
   * Set the live-toast cap from measured geometry. Clamped into
   * [MIN_VISIBLE_TOASTS, MAX_VISIBLE_TOASTS]; shrinking it evicts the oldest
   * toasts immediately, so nothing can sit off-screen with a running timer.
   */
  setToastCapacity: (capacity: number) => void;
  /** Begin one toast's exit animation (the per-toast dismiss button). */
  dismissToast: (id: number) => void;
  /** Drop one toast outright. ToastStack calls this when a toast's life ends. */
  removeToast: (id: number) => void;
  /**
   * Drop every toast immediately - no exit animation, no eviction bookkeeping.
   *
   * A TEST-HARNESS reset, and deliberately nothing else: no production code path
   * calls it, and none should. The app dismisses toasts one at a time (the stack
   * owns the dwell/fade clock and calls `removeToast`), so a bulk wipe would take
   * a toast off screen mid-animation with its ToastStack timer still running.
   * It stays because the toast tests - including `ToastStack.test.tsx`'s teardown
   * - reset the stack through it between cases.
   */
  clearToast: () => void;
  /**
   * Extra clearance (px) the toast stack keeps above its default `bottom-6`
   * anchor, keyed by the owner that requested it. A dialog with its own
   * bottom-anchored footer near the toast's corner (e.g. the DMP wizard's
   * Cancel/Back row) registers its footer height here for as long as it is
   * mounted, so a lingering toast (F8: `dmp-manual-placement-hint`) is
   * raised above the footer instead of covering it. Keyed rather than a
   * single number so two owners never stomp each other's clearance; the
   * stack uses the largest value currently registered.
   */
  toastBottomOffsets: Record<string, number>;
  registerToastBottomOffset: (owner: string, px: number) => void;
  unregisterToastBottomOffset: (owner: string) => void;

  // Snap state
  snappedStation: StationRef | null;
  setSnappedStation: (ref: StationRef | null) => void;

  // Drag state
  isDragging: boolean;
  dragStart: [number, number] | null;
  dragCurrent: [number, number] | null;
  setDragState: (isDragging: boolean, start?: [number, number], current?: [number, number]) => void;

  // Dialog state
  activeDialog: string | null;
  dialogData: Record<string, unknown>;
  dmpImportWizardPhase: DmpImportWizardPhase | null;
  openDialog: (dialog: string, data?: Record<string, unknown>) => void;
  closeDialog: () => void;
  setDmpImportWizardPhase: (phase: DmpImportWizardPhase | null) => void;

  // Screen-occupancy registry (#445 review): a count of independent top-layer
  // overlays that render OUTSIDE `activeDialog` (TelemetryOptInDialog,
  // HeldLockResumeDialog, SpeleoDumpPickerDialog) and
  // are currently actually on screen. Register via `useScreenOccupant` from the
  // overlay itself; check via `isScreenBusy()`. A counter, not a boolean, because
  // more than one can be mounted at once (e.g. a login made from inside Settings
  // can raise the held-lock prompt over it).
  screenOccupants: number;
  addScreenOccupant: () => void;
  removeScreenOccupant: () => void;

  // Import flow context
  importContext: 'new-project' | 'existing-project' | null;
  setImportContext: (context: 'new-project' | 'existing-project' | null) => void;

  // Session memory of the last surveyors / explorers a section was created or
  // imported with (not persisted; the cross-project autocomplete is
  // `store/recentPeople`). New Section and the mNemo wizard prefill from it.
  lastUsedCredit: LastUsedCredit;
  setLastUsedCredit: (credit: LastUsedCredit) => void;

  // Placement mode state (for manual map placement of imported sections)
  placementMode: PlacementModeState | null;
  startPlacementMode: (
    sectionData: SectionData,
    alignStationIndex: number,
    onComplete?: (result: PlacementResult) => void,
    onCancel?: () => void,
    additionalSections?: AdditionalPlacementSection[],
  ) => void;
  updatePlacementCursor: (position: Coordinates | null) => void;
  updatePlacementSnap: (stationRef: StationRef | null) => void;
  cancelPlacementMode: () => void;
  confirmPlacement: () => PlacementModeState | null;

  // Map viewport center in project coordinates (x, y in meters)
  // Used for "Place at center of screen" in import dialog
  mapViewportCenterProject: { x: number; y: number } | null;
  setMapViewportCenterProject: (center: { x: number; y: number } | null) => void;
  getMapViewportCenter: () => { x: number; y: number };

  // Theme state
  resolvedTheme: ResolvedTheme;
  setResolvedTheme: (theme: ResolvedTheme) => void;
  /** Temporary theme override for live preview in Settings dialog */
  themePreview: AppearanceConfig | null;
  setThemePreview: (appearance: AppearanceConfig | null) => void;

  // Syntactic highlighting state
  /** Resolved highlighting level, mirrored from config by useSyntacticHighlighting. */
  syntacticLevel: SyntacticHighlightLevel;
  setSyntacticLevel: (level: SyntacticHighlightLevel) => void;
  /** Temporary highlighting override for live preview in Settings dialog */
  syntacticHighlightPreview: SyntacticHighlightingConfig | null;
  setSyntacticHighlightPreview: (config: SyntacticHighlightingConfig | null) => void;

  // Comment tool state
  commentTarget: CommentTarget | null;
  setCommentTarget: (target: CommentTarget | null) => void;
  clearCommentTarget: () => void;

  // Move comment tool state
  moveCommentSource: {
    type: 'station' | 'shot';
    stationRef?: StationRef;
    sectionId?: SectionId;
    shotId?: ShotId;
    comment: string;
    /** Screen position of the click that picked up the comment (for cursor tag) */
    screenPos?: { x: number; y: number };
  } | null;
  setMoveCommentSource: (source: UIStore['moveCommentSource']) => void;
  clearMoveCommentSource: () => void;

  // ARIA selection announcements for screen readers
  selectionAnnouncement: string;
  announceSelection: (message: string) => void;
}

export type DisplayToggleKey =
  | 'showStations'
  | 'showShots'
  | 'showSectionNames'
  | 'showStationLabels'
  | 'showStationNames'
  | 'showShotData'
  | 'showShotErrors'
  | 'showReferenceShots'
  | 'showJumps'
  | 'showZeroDatum'
  | 'showGrid'
  | 'showSatellite'
  | 'showDepthGradient'
  | 'showStationComments'
  | 'showShotComments'
  | 'showDevStats'
  | 'showMemoryStats'
  | 'showMapStats'
  | 'showZoomStats';

/**
 * The hamburger menu's Debug submenu toggles are session-only developer
 * overlays (see the showDevStats/showMemoryStats/showMapStats/showZoomStats
 * resets in applyDisplaySettings and setView('global') below) - they're
 * never restored from the backend, so persisting them via saveDisplaySetting
 * would only ever write orphan rows to the project's display-settings table.
 * toggleDisplay skips the IPC write for any key in this set.
 */
const SESSION_ONLY_DISPLAY_TOGGLES: ReadonlySet<DisplayToggleKey> = new Set([
  'showDevStats',
  'showMemoryStats',
  'showMapStats',
  'showZoomStats',
]);

/**
 * Toggles that are one choice between two labels: the map's station label is
 * either the station ID or the station name, never both. Turning one on turns
 * its partner off; turning one off leaves the partner alone, so both off (no
 * station labels) is reachable.
 */
const EXCLUSIVE_DISPLAY_PARTNER: Partial<Record<DisplayToggleKey, DisplayToggleKey>> = {
  showStationLabels: 'showStationNames',
  showStationNames: 'showStationLabels',
};

/**
 * Layout captured when entering section-focus mode (#50), restored on exit.
 * Module-level (not store state) since it's internal bookkeeping the UI never
 * reads. Re-focusing a different section while already focused keeps the
 * original snapshot, so exiting always returns to the pre-focus layout.
 */
let focusLayoutSnapshot:
  | {
      sectionsPanelOpen: boolean;
      sectionsPanelMode: PanelMode;
      detailPanelOpen: boolean;
      detailPanelMode: PanelMode;
      dataTableOpen: boolean;
      dataTableMode: PanelMode;
      dataTableHeight: number;
    }
  | null = null;

/**
 * The geometry mode captured when leaving an editable mode (Survey, Data) for a
 * read-only one (Plan, Export), restored when an editable mode is entered again.
 * The read-only modes force `closureAdjusted` (the read-only closure map); this
 * remembers what the user had so Survey or Data comes back to it. Module-level
 * bookkeeping (the UI never reads it), like {@link focusLayoutSnapshot}. The
 * `prePlan` names predate Data and Export; the snapshots cover both read-only modes.
 */
let prePlanGeometryMode: GeometryMode | null = null;

/**
 * The editable mode (Survey or Data) a read-only one was entered from, captured with
 * the snapshots below, so `exitGasPlan()` (Shift+D in Plan, a view switch) returns
 * the user to the workspace they left rather than always to Survey.
 */
let preReadOnlyMode: 'survey' | 'data' | null = null;

/**
 * Display/overlay flags captured on the same editable -> read-only edge, restored on
 * the way back - so Display changes made while planning/exporting are scoped to that
 * mode and never mutate (or persist to) the editable view. Includes the color overlays
 * that Plan force-clears on entry, so entering Plan no longer silently turns a survey
 * depth/distance gradient off for good. Module-level bookkeeping like
 * {@link prePlanGeometryMode}; the persistence skip lives in `toggleDisplay` /
 * `setGridSpacing` (they only write to the saved config in an editable mode).
 */
const PREPLAN_DISPLAY_KEYS = [
  'showStations',
  'showShots',
  'showReferenceShots',
  'showJumps',
  'showZeroDatum',
  'showSectionNames',
  'showStationLabels',
  'showStationNames',
  'showStationComments',
  'showShotComments',
  'showShotData',
  'showShotErrors',
  'showGrid',
  'showSatellite',
  'gridSpacing',
  'showDepthGradient',
  'showDistanceGradient',
  'showIssueSeverity',
  'distanceGradientOrigin',
  'distanceField',
] as const;
type PrePlanDisplay = Pick<UIStore, (typeof PREPLAN_DISPLAY_KEYS)[number]>;
let prePlanDisplay: PrePlanDisplay | null = null;
const snapshotDisplay = (s: UIStore): PrePlanDisplay => {
  const snap: Partial<PrePlanDisplay> = {};
  for (const k of PREPLAN_DISPLAY_KEYS) {
    (snap as Record<string, unknown>)[k] = s[k];
  }
  return snap as PrePlanDisplay;
};

/**
 * The right Dock (detail panel) and bottom Dock (data table) open/mode captured
 * on the editable -> read-only edge, restored on the way back (#241). Plan and
 * Export force both docks open (`revealDetailPanel` + the Plan-only pinned data
 * table) so the setup/Cylinders panels surface without hunting for a closed dock;
 * without this snapshot a dock the user had closed stayed force-opened after
 * exiting back to Survey. Data renders neither dock but leaves their flags alone,
 * so a snapshot taken in Data is Survey's own dock state. Module-level bookkeeping
 * like {@link prePlanGeometryMode} - restored via a plain `set()` (not
 * `toggleDetailPanel`/`toggleDataTable`), so the restore itself is never persisted,
 * mirroring the fullscreen-table snapshot.
 */
let prePlanDocks:
  | {
      detailPanelOpen: boolean;
      detailPanelMode: PanelMode;
      dataTableOpen: boolean;
      dataTableMode: PanelMode;
    }
  | null = null;

/**
 * The data table's open/mode captured when entering fullscreen, restored on exit.
 * Module-level bookkeeping (the UI never reads it). Fullscreen forces the table
 * open+pinned; without this, exiting would strand a hover-mode or closed table
 * as pinned-open.
 */
let fullScreenTableSnapshot:
  | { dataTableOpen: boolean; dataTableMode: PanelMode }
  | null = null;

/**
 * Show the Details panel on its Warnings tab, making sure it will actually mount
 * and be visible: any mode but Survey has no Details panel, and section-focus and
 * data-table fullscreen hide or cover the
 * panel, and the panel may simply be collapsed. WarningsContent only consumes a
 * resolve request while mounted (detailPanelOpen && tab === 'warnings'), so
 * without this a request would be a no-op that sat stale and fired on a later
 * tab switch.
 *
 * It reveals the panel without changing the user's saved panel mode. Pinned: just
 * ensure it is open. Hover: bump the reveal nonce so SingleSystemView floats it
 * open like a hover (with a timed auto-close, since no pointer-leave fires for a
 * programmatic reveal), so revealing never silently leaves a hover-mode user
 * pinned. Opening a pinned-but-closed panel is a real (persistent) open, just
 * like toggleDetailPanel, so it is persisted: otherwise the in-memory open flag
 * would diverge from the saved layout (re-closing the panel next session, or
 * being overwritten by any later savePanelLayout).
 */
function revealWarningsTab(
  set: (partial: Partial<UIStore>) => void,
  extra: Partial<UIStore>,
): void {
  let s = useUIStore.getState();
  // The Details panel exists only in Survey: Data renders no right dock, and Plan
  // and Export put panels of their own there. Switch first, or the request would
  // sit stale with nothing mounted to consume it.
  if (s.appMode !== 'survey') {
    s.setAppMode('survey');
    s = useUIStore.getState();
  }
  if (s.focusedSectionId !== null) s.exitSectionFocus();
  if (s.dataTableFullScreen) s.exitDataTableFullScreen();
  // Re-read: the exits above can change the panel's mode/open.
  s = useUIStore.getState();
  const pinOpening = s.detailPanelMode === 'pin' && !s.detailPanelOpen;
  set({
    ...extra,
    detailPanelTab: 'warnings',
    ...(s.detailPanelMode === 'pin'
      ? { detailPanelOpen: true }
      : { detailRevealNonce: s.detailRevealNonce + 1 }),
  });
  if (pinOpening) void useUIStore.getState().savePanelLayout();
}

/**
 * Apply a panel mutation that also exits section-focus (#50). Restores the
 * pre-focus layout snapshot (a no-op when not focused, since the snapshot is
 * null), then layers the caller's panel-specific overrides on top and clears
 * focusedSectionId. The spread order is load-bearing: snapshot first (the OTHER
 * panels' real pre-focus values), the caller's overrides second (this panel's
 * intended new state wins), focusedSectionId:null last. Shared by the five
 * focus-dropping mutators so the snapshot ritual lives in exactly one place.
 */
function exitFocusMerge(
  set: (partial: Partial<UIStore>) => void,
  overrides: Partial<UIStore>,
): void {
  const snap = focusLayoutSnapshot;
  focusLayoutSnapshot = null;
  set({ ...(snap ?? {}), ...overrides, focusedSectionId: null });
}

/**
 * Maps each {@link PanelId} to its three per-panel store fields, so the panel
 * actions (setPanelOpen/setPanelMode) and the reveal selectors read one entry
 * instead of repeating a 3-way ternary. Exported for usePanelReveal.
 */
export const PANEL_FIELDS = {
  sections: { open: 'sectionsPanelOpen', mode: 'sectionsPanelMode', width: 'sectionsPanelWidth' },
  detail: { open: 'detailPanelOpen', mode: 'detailPanelMode', width: 'detailPanelWidth' },
  'data-table': { open: 'dataTableOpen', mode: 'dataTableMode', width: 'dataTableHeight' },
} as const satisfies Record<PanelId, { open: string; mode: string; width: string }>;

/**
 * The store/config field each SIZED panel's width persists through. The three
 * reveal panels reuse `PANEL_FIELDS`' own `width` so there is still one name per
 * panel; the wizard map column has no reveal state and so appears only here.
 */
export const PANEL_WIDTH_FIELDS = {
  sections: PANEL_FIELDS.sections.width,
  detail: PANEL_FIELDS.detail.width,
  'data-table': PANEL_FIELDS['data-table'].width,
  'wizard-map': 'wizardMapPanelWidth',
} as const satisfies Record<SizedPanelId, string>;

/**
 * Does a modal own the screen right now - the single `activeDialog` slot, a
 * placement flow, or an independent top-layer overlay registered via
 * `useScreenOccupant`? The one predicate for "is it safe to open something
 * else without stacking a dialog over a surface the user is mid-answering" -
 * `screenBusy()` in launchWithFile.ts today, and whatever else asks tomorrow.
 * Callable as a plain function (`isScreenBusy(useUIStore.getState())`) or as
 * a selector, same shape as `isMapReadOnly` above.
 */
export function isScreenBusy(
  state: Pick<UIStore, 'activeDialog' | 'placementMode' | 'screenOccupants'>,
): boolean {
  return state.activeDialog !== null || state.placementMode !== null || state.screenOccupants > 0;
}

export const useUIStore = create<UIStore>((set) => ({
  // View state
  view: 'global',
  setView: (view) => {
    // Section-focus mode (#50) is single-view, single-project scoped: drop it on
    // any view switch so a stale focus can't outlive the view or project that
    // opened it. No-op when not focused; restores the pre-focus panel layout.
    useUIStore.getState().exitSectionFocus();
    // Fullscreen table is single-view scoped too - a view switch can't strand it.
    useUIStore.getState().exitDataTableFullScreen();
    // Gas-planning sub-mode (#38) is single-view scoped too - drop it on any view
    // switch so a stale reachable overlay + input panel can't outlive the view.
    useUIStore.getState().exitGasPlan();
    setDwellState(view === 'global' ? 'global-view' : `tool.${useUIStore.getState().activeTool}`);
    if (view === 'single') {
      void advanceProjectSeq();
      // A project open/create/import flow has landed - the entry-point source
      // has been consumed, so clear it to prevent bleed into a later open.
      clearProjectActionSource();
    }
    // The Debug submenu's four toggles (Dev/Memory/Map/Zoom Stats) are
    // single-view developer overlays - the Display toggles that control them
    // live only in SingleSystemView, but <MemoryProbe/> is mounted app-wide.
    // Reset them on the way back to GlobalView so the overlays (and the
    // memory probe's 2 s memory-IPC poll) can't strand there with no in-view
    // way to hide them.
    //
    // The ruler readout/tint now lives in this global store, but is a single-view,
    // map-only interaction - clear it on any view switch so it can't be resurrected
    // when MapCanvas remounts on return (#26, Codex).
    set(view === 'global'
      ? {
          view,
          showDevStats: DISPLAY_DEFAULTS.showDevStats,
          showMemoryStats: DISPLAY_DEFAULTS.showMemoryStats,
          showMapStats: DISPLAY_DEFAULTS.showMapStats,
          showZoomStats: DISPLAY_DEFAULTS.showZoomStats,
          ...MEASURE_CLEARED,
        }
      : { view, ...MEASURE_CLEARED });
  },

  // Tool state
  activeTool: 'select',
  lastSubtoolByGroup: {},
  setActiveTool: (activeTool) =>
    set((state) => {
      trackAction(`tool.${activeTool}`);
      if (!useUIStore.getState().activeDialog && !useUIStore.getState().placementMode) {
        setDwellState(`tool.${activeTool}`);
      }
      // Track last-used subtool per group
      const group = getGroupForTool(activeTool);
      const lastSubtoolByGroup = group
        ? { ...state.lastSubtoolByGroup, [group.id]: activeTool }
        : state.lastSubtoolByGroup;

      // Clear comment target when switching away from comment tool
      const clearComment = state.commentTarget !== null && activeTool !== 'comment'
        ? { commentTarget: null }
        : {};

      // Clear move comment source when switching away from move_comment tool
      // No backend restore needed - pickup doesn't mutate backend
      const clearMoveComment = state.moveCommentSource !== null && activeTool !== 'move_comment'
        ? { moveCommentSource: null }
        : {};

      // Clear any active ruler measurement when switching to a different tool
      // (#26: "reset … selects a different tool"). The measure tool never holds
      // measure state while another tool is active.
      const clearMeasure = activeTool !== 'measure' &&
        (state.measureStart !== null || state.measureEnd !== null ||
         state.measureAlongLine !== null || state.measureSnapShot !== null)
        ? MEASURE_CLEARED
        : {};

      // Clear hover state when switching to non-interactive tools (pan/zoom)
      // This ensures store consistency regardless of how hover was set
      // Use explicit null checks (not truthiness) because SectionId can be 0
      if (
        isNonInteractiveTool(activeTool) &&
        (state.hoveredStation !== null || state.hoveredShot !== null || state.hoveredSectionId !== null)
      ) {
        return { activeTool, lastSubtoolByGroup, hoveredStation: null, hoveredShot: null, hoveredSectionId: null, ...clearComment, ...clearMoveComment, ...clearMeasure };
      }
      return { activeTool, lastSubtoolByGroup, ...clearComment, ...clearMoveComment, ...clearMeasure };
    }),
  // Project-switch reset for tool state. Deliberately NOT routed through
  // setActiveTool: a load-forced reset is not a user action, so no telemetry or
  // dwell fires and nothing persists (plain set(), per the transient-state rule).
  // Entity refs (moveCommentSource, merge picks, commentTarget) use small
  // per-project integer ids, so a stale ref from project A usually RESOLVES in
  // project B and the next click would mutate the wrong entity - null them all.
  // Measure endpoints carry the same per-project refs, so the canonical reset
  // owns MEASURE_CLEARED itself instead of relying on the setView() call that
  // happens to follow it (MapCanvas's projectSwitchKey effect also clears
  // measure state - a second, independent belt).
  resetToolTransientsForProjectSwitch: () =>
    set({
      activeTool: 'select',
      moveCommentSource: null,
      mergeStationA: null,
      mergeStationB: null,
      commentTarget: null,
      ...MEASURE_CLEARED,
    }),

  // Selection state
  selection: null,
  multiSelection: [],
  setSelection: (selection) => {
    if (selection) trackSelect(selection.type);
    set({ selection, multiSelection: selection ? [selection] : [] });
  },
  addToSelection: (selection) =>
    set((state) => {
      // Don't add duplicates
      const exists = state.multiSelection.some((s) => selectionsEqual(s, selection));
      if (exists) return state;
      trackSelectAdd(selection.type);
      return {
        multiSelection: [...state.multiSelection, selection],
        selection: selection,
      };
    }),
  removeFromSelection: (selection) =>
    set((state) => {
      const newMulti = state.multiSelection.filter((s) => !selectionsEqual(s, selection));
      const lastItem = newMulti[newMulti.length - 1];
      const newSelection: Selection | null = lastItem !== undefined ? lastItem : null;
      return {
        multiSelection: newMulti,
        selection: newSelection,
      };
    }),
  toggleSelection: (selection) =>
    set((state) => {
      const exists = state.multiSelection.some((s) => selectionsEqual(s, selection));
      if (exists) {
        const newMulti = state.multiSelection.filter((s) => !selectionsEqual(s, selection));
        const lastItem = newMulti[newMulti.length - 1];
        const newSelection: Selection | null = lastItem !== undefined ? lastItem : null;
        return {
          multiSelection: newMulti,
          selection: newSelection,
        };
      } else {
        return {
          multiSelection: [...state.multiSelection, selection],
          selection: selection,
        };
      }
    }),
  clearSelection: () => set({ selection: null, multiSelection: [] }),
  clearSelectionByType: (type) =>
    set((state) => {
      const filteredMulti = state.multiSelection.filter((s) => s.type !== type);
      // If current selection is being cleared, update to last remaining item
      const newSelection =
        state.selection?.type === type
          ? (filteredMulti[filteredMulti.length - 1] ?? null)
          : state.selection;
      return {
        selection: newSelection,
        multiSelection: filteredMulti,
      };
    }),
  filterSelection: (predicate) =>
    set((state) => {
      const filteredMulti = state.multiSelection.filter(predicate);
      // If current selection fails predicate, update to last remaining item
      const newSelection =
        state.selection && predicate(state.selection)
          ? state.selection
          : (filteredMulti[filteredMulti.length - 1] ?? null);
      return {
        selection: newSelection,
        multiSelection: filteredMulti,
      };
    }),
  selectAll: (selections: Selection[]) =>
    set({
      multiSelection: selections,
      selection: selections[selections.length - 1] ?? null,
    }),
  isSelected: (selection: Selection): boolean => {
    // Note: This will get the latest state at call time
    return useUIStore.getState().multiSelection.some((s: Selection) => selectionsEqual(s, selection));
  },

  // Tool-specific state
  mergeStationA: null,
  mergeStationB: null,
  setMergeStationA: (mergeStationA) => set({ mergeStationA }),
  setMergeStationB: (mergeStationB) => set({ mergeStationB }),
  clearMergeStations: () => set({ mergeStationA: null, mergeStationB: null }),

  measureStart: null,
  measureEnd: null,
  measureDirect: null,
  measureAlongLine: null,
  measureAlongLinePending: false,
  measureSnapShot: null,
  setMeasureStart: (measureStart) => set({ measureStart }),
  setMeasureEnd: (measureEnd) => set({ measureEnd }),
  setMeasureDirect: (measureDirect) => set({ measureDirect }),
  setMeasureAlongLine: (measureAlongLine) => set({ measureAlongLine, measureAlongLinePending: false }),
  setMeasureAlongLinePending: (measureAlongLinePending) => set({ measureAlongLinePending }),
  setMeasureSnapShot: (measureSnapShot) => set({ measureSnapShot }),
  clearMeasure: () => set({ ...MEASURE_CLEARED }),

  // Panel state
  leftWorkspace: 'sections',
  surveyLeftWorkspace: 'sections',
  speleoWorkspaceSection: null,
  speleoRevealNonce: 0,
  speleoOutsideSurvey: false,
  leftWorkspaceVisible: false,
  resetLeftWorkspace: () => set({ leftWorkspace: 'sections', surveyLeftWorkspace: 'sections', speleoWorkspaceSection: null, speleoOutsideSurvey: false }),
  selectLeftWorkspace: (tab) => set({ leftWorkspace: tab, ...(useUIStore.getState().appMode === 'survey' ? { surveyLeftWorkspace: tab } : {}) }),
  openSpeleoWorkspace: (section) => {
    const state = useUIStore.getState();
    state.exitDataTableFullScreen();
    set({
      leftWorkspace: 'speleodb',
      ...(state.appMode === 'survey' ? { surveyLeftWorkspace: 'speleodb' } : { speleoOutsideSurvey: true }),
      ...(section ? { speleoWorkspaceSection: section } : {}),
      speleoRevealNonce: state.speleoRevealNonce + 1,
    });
  },
  sectionsPanelOpen: true,
  sectionsPanelMode: 'pin',
  sectionsPanelWidth: 250,
  detailPanelOpen: true,
  detailPanelMode: 'pin',
  detailPanelWidth: 300,
  detailPanelTab: 'details',
  // Closed until the config arrives, even though a fresh install ends up OPEN -
  // that default is delivered by initPanels' `?? true` below, which is the path
  // a config with no stored value takes. Starting open instead mounted the
  // heaviest panel in the app on every launch for everyone who keeps it closed,
  // then unmounted it a moment later, with the map briefly sized around it.
  dataTableOpen: false,
  dataTableMode: 'pin',
  dataTableHeight: 200,
  wizardMapPanelWidth: WIZARD_MAP_PANEL_DEFAULT_WIDTH,
  detailRevealNonce: 0,
  // Keyboard shortcuts / menu items are deliberate commands → pin open/close
  // (mode forced to 'pin' so the panel reflows and stays). Manually changing the
  // layout drops section-focus mode (#50) so the toolbar can't keep showing a
  // focused-section name with panels reopened; the new layout is persisted.
  toggleSectionsPanel: () => {
    const s = useUIStore.getState();
    const opening = !s.sectionsPanelOpen;
    trackPanelToggle('sections', opening);
    // Exiting section-focus via a toggle must restore the OTHER panels' pre-focus
    // open/mode (kept in the snapshot) before savePanelLayout persists live state.
    // Otherwise the focus-forced collapse/pin of the untouched panels would be
    // saved as the user's layout, silently overwriting their preferences. `snap`
    // is null when not focused → the spread is a no-op. `opening` is relative to
    // the currently-visible (focus-forced) state so the toggle flips what the user
    // sees; this panel's own values are then overridden on top of the snapshot.
    exitFocusMerge(set, { sectionsPanelOpen: opening, sectionsPanelMode: 'pin' });
    void s.savePanelLayout();
  },
  toggleDetailPanel: () => {
    const s = useUIStore.getState();
    const opening = !s.detailPanelOpen;
    trackPanelToggle('detail', opening);
    // See toggleSectionsPanel: restore the snapshot first so toggling out of focus
    // doesn't persist the other panels' focus-forced state.
    exitFocusMerge(set, { detailPanelOpen: opening, detailPanelMode: 'pin' });
    void s.savePanelLayout();
  },
  revealDetailPanel: () => {
    const s = useUIStore.getState();
    // Hover mode (and NOT section-focused, where the mode is force-pinned): float it
    // open transiently via the nonce - the same non-persisting idiom as
    // triggerResolveWarning - so viewing details never overwrites the user's hover
    // preference with a persisted 'pin'. Every other case keeps the prior behavior
    // (the deliberate open-and-pin command), so only the buggy path changes.
    if (s.detailPanelMode === 'hover' && s.focusedSectionId === null) {
      set({ detailRevealNonce: s.detailRevealNonce + 1 });
    } else if (!s.detailPanelOpen) {
      s.toggleDetailPanel();
    }
  },
  toggleDataTable: () => {
    const s = useUIStore.getState();
    const opening = !s.dataTableOpen;
    trackPanelToggle('data-table', opening);
    // See toggleSectionsPanel: restore the snapshot first so toggling out of focus
    // doesn't persist the other panels' focus-forced state.
    exitFocusMerge(set, { dataTableOpen: opening, dataTableMode: 'pin' });
    void s.savePanelLayout();
  },
  setPanelOpen: (panel, open) => {
    const s = useUIStore.getState();
    trackPanelToggle(panel, open);
    // See toggleSectionsPanel: restore the snapshot first so changing one panel out
    // of focus doesn't persist the other panels' focus-forced state.
    exitFocusMerge(set, { [PANEL_FIELDS[panel].open]: open } as Partial<UIStore>);
    void s.savePanelLayout();
  },
  setPanelMode: (panel, mode) => {
    const s = useUIStore.getState();
    trackPanelMode(panel, mode);
    // See toggleSectionsPanel: restore the snapshot first so changing one panel out
    // of focus doesn't persist the other panels' focus-forced state.
    // Pinning makes the panel a persistent open panel; un-pinning (hover) leaves
    // it closed at rest (visible only while hovered, tracked in local React state).
    const open = mode === 'pin';
    exitFocusMerge(set, {
      [PANEL_FIELDS[panel].mode]: mode,
      [PANEL_FIELDS[panel].open]: open,
    } as Partial<UIStore>);
    void s.savePanelLayout();
  },
  setDetailPanelTab: (detailPanelTab) => set({ detailPanelTab }),
  setSectionsPanelWidth: (sectionsPanelWidth) => set({ sectionsPanelWidth }),
  setDetailPanelWidth: (detailPanelWidth) => set({ detailPanelWidth }),
  setDataTableHeight: (dataTableHeight) => set({ dataTableHeight }),
  setWizardMapPanelWidth: (wizardMapPanelWidth) => set({ wizardMapPanelWidth }),
  initPanels: (panels: PanelsConfig) => {
    const asMode = (m: string): PanelMode => (m === 'hover' ? 'hover' : 'pin');
    set({
      sectionsPanelWidth: panels.sections_panel_width,
      detailPanelWidth: panels.detail_panel_width,
      dataTableHeight: panels.data_table_height,
      // Defaulted defensively for the same reason as the open booleans below:
      // the Rust config supplies it via a serde default, but a partial config
      // object that bypasses serde (the e2e mock, or a config written before
      // #404) would otherwise set the width to `undefined` and collapse the
      // wizard map column to nothing.
      wizardMapPanelWidth: panels.wizard_map_panel_width ?? WIZARD_MAP_PANEL_DEFAULT_WIDTH,
      // Default the open booleans defensively. The Rust config supplies them via
      // serde defaults, but a partial config object that bypasses serde (the e2e
      // mock, or an older persisted shape) would otherwise write `undefined`,
      // leaving the side panels falsy-closed at rest. asMode() already defaults
      // the mode strings, so only the booleans need this guard.
      sectionsPanelOpen: panels.sections_panel_open ?? true,
      sectionsPanelMode: asMode(panels.sections_panel_mode),
      detailPanelOpen: panels.detail_panel_open ?? true,
      detailPanelMode: asMode(panels.detail_panel_mode),
      dataTableOpen: panels.data_table_open ?? true,
      dataTableMode: asMode(panels.data_table_mode),
    });
  },
  savePanelSizes: async (panel, size) => {
    // This persists as a Rust `u32`. clampRound guarantees the contract regardless
    // of caller: it rounds to a whole pixel (sub-pixel sizes under the webview UI
    // zoom / HiDPI fail serde and throw), folds a non-finite size to the minimum,
    // AND clamps to the panel's resize bounds so a degenerate 0 can never be
    // persisted (which would reload the panel collapsed - there is no "unset"
    // sentinel, the stored u32 is taken at face value).
    const { min, max } = PANEL_SIZE_BOUNDS[panel];
    const fields = { [PANEL_WIDTH_FIELDS[panel]]: clampRound(size, min, max) };
    try {
      await invokeIpc('update_panel_sizes', fields, { purpose: 'persist panel layout', fields });
    } catch (e) {
      console.error('Failed to save panel sizes:', e);
    }
  },
  savePanelLayout: async () => {
    // Skip persistence for programmatic panel changes (e.g. onboarding toggles
    // panels transiently via withPersistSuppressed): the user's saved layout
    // must not be overwritten by a temporary onboarding-driven open/close. The
    // toggles call this synchronously, so the suppression flag is still set here.
    // Keyed on persist-suppression specifically (NOT telemetry-suppression) so a
    // caller wrapping a real user action in withTrackingSuppressed still persists.
    if (isPersistSuppressed()) return;
    const s = useUIStore.getState();
    const fields = {
      sectionsPanelOpen: s.sectionsPanelOpen,
      sectionsPanelMode: s.sectionsPanelMode,
      detailPanelOpen: s.detailPanelOpen,
      detailPanelMode: s.detailPanelMode,
      dataTableOpen: s.dataTableOpen,
      dataTableMode: s.dataTableMode,
    };
    try {
      await invokeIpc('update_panel_layout', fields, {
        purpose: 'persist panel reveal layout',
        fields,
      });
    } catch (e) {
      console.error('Failed to save panel layout:', e);
    }
  },

  // Data-table fullscreen mode (session-only, never persisted)
  dataTableFullScreen: false,
  rawTableSelection: null,
  enterDataTableFullScreen: () => {
    const s = useUIStore.getState();
    // The overlay grows out of the docked table, so the table must be open and
    // pinned (fullscreen is a deliberate, persistent state, not a hover peek).
    // Snapshot the table's prior open/mode on FIRST entry so exit can restore it
    // - otherwise entering fullscreen from a hover-revealed (or closed) table
    // would leave it stuck pinned-open afterward, silently losing the user's
    // hover/closed preference.
    if (!s.dataTableFullScreen) {
      // Snapshot the table's prior open/mode so exit can restore it. While section-
      // focused (#50), the live dataTableOpen/Mode are focus-forced (open+'pin'),
      // NOT the user's real preference - that lives in focusLayoutSnapshot. Capture
      // from there so a hover/closed table isn't stranded pinned-open after exiting
      // fullscreen (exit-fullscreen and exit-focus are independent paths).
      fullScreenTableSnapshot =
        s.focusedSectionId !== null && focusLayoutSnapshot
          ? { dataTableOpen: focusLayoutSnapshot.dataTableOpen, dataTableMode: focusLayoutSnapshot.dataTableMode }
          : { dataTableOpen: s.dataTableOpen, dataTableMode: s.dataTableMode };
    }
    set({ dataTableFullScreen: true, dataTableOpen: true, dataTableMode: 'pin' });
  },
  exitDataTableFullScreen: () => {
    if (!useUIStore.getState().dataTableFullScreen) return;
    // Restore the pre-fullscreen open/mode captured on entry. These are transient
    // (fullscreen forces them) and never persisted, so no savePanelLayout call.
    const snap = fullScreenTableSnapshot;
    fullScreenTableSnapshot = null;
    // If section focus is STILL active, its workspace requires the table open+pinned
    // - re-apply that instead of restoring the pre-fullscreen snapshot (exit-focus
    // will restore the user's real state later). Only restore the snapshot when no
    // other override remains, so the table is never stranded pinned-open or wrongly
    // closed while focused.
    const stillFocused = useUIStore.getState().focusedSectionId !== null;
    const restored: Partial<UIStore> = stillFocused
      ? { dataTableOpen: true, dataTableMode: 'pin' }
      : (snap ?? {});
    // Leaving fullscreen also leaves the raw-data view: the picker that could
    // switch back to Sections only exists in the fullscreen toolbar.
    set({ dataTableFullScreen: false, rawTableSelection: null, ...restored });
  },
  setRawTableSelection: (rawTableSelection) => set({ rawTableSelection }),

  // Section focus mode (#50)
  focusedSectionId: null,
  enterSectionFocus: (focusedSectionId, dataTableHeight) => {
    const s = useUIStore.getState();
    // Focus mode is an editable workspace for one section. The closure-adjusted
    // view (#70) is read-only, so entering focus leaves it and returns to the
    // measured geometry - guaranteeing the focused data table is editable.
    if (s.geometryMode !== 'measured') {
      s.setGeometryMode('measured', 'sections-panel');
    }
    // Snapshot the layout only on first entry; re-focusing another section
    // while already focused must not overwrite the pre-focus layout.
    if (s.focusedSectionId === null) {
      focusLayoutSnapshot = {
        sectionsPanelOpen: s.sectionsPanelOpen,
        sectionsPanelMode: s.sectionsPanelMode,
        detailPanelOpen: s.detailPanelOpen,
        detailPanelMode: s.detailPanelMode,
        dataTableOpen: s.dataTableOpen,
        dataTableMode: s.dataTableMode,
        dataTableHeight: s.dataTableHeight,
      };
    }
    // Set the focus layout directly (not via save*) so the temporary focus state
    // is never persisted to the user's panel config. Side panels collapse and the
    // data table pins open (reflow). Only apply the focus height when the table
    // opens from closed: a fresh ResizablePanel mount seeds its (uncontrolled)
    // size from this height. If the table is already open we leave its height
    // alone rather than force a remount that would reset DataTable state.
    set({
      focusedSectionId,
      sectionsPanelOpen: false,
      // Force pin mode while collapsed so a hover-mode side panel can't pop open
      // over the focused workspace. Prior modes are captured in the snapshot above
      // and restored on exit.
      sectionsPanelMode: 'pin',
      detailPanelOpen: false,
      detailPanelMode: 'pin',
      dataTableOpen: true,
      dataTableMode: 'pin',
      ...(s.dataTableOpen ? {} : { dataTableHeight }),
    });
  },
  exitSectionFocus: () => {
    if (useUIStore.getState().focusedSectionId === null) return;
    const snap = focusLayoutSnapshot;
    focusLayoutSnapshot = null;
    // Restores the captured panel layout. The geometry switch to 'measured' on
    // entry is intentionally one-way (per the #50/#70 design resolution): exiting
    // leaves you in measured to keep editing, not back in the read-only view.
    set({ focusedSectionId: null, ...(snap ?? {}) });
  },
  setFocusedSection: (focusedSectionId) => {
    // Only meaningful while already focused (follow the selection). Guarded so it
    // can't enter focus without a layout snapshot.
    if (useUIStore.getState().focusedSectionId === null) return;
    set({ focusedSectionId });
  },

  // Map state
  mapZoom: 15,
  setMapZoom: (mapZoom) => set({ mapZoom }),
  requestFitToBounds: false as false | 'animate' | 'snap',
  triggerFitToBounds: (options?) => {
    trackNav('fit-to-bounds');
    set({ requestFitToBounds: options?.snap ? 'snap' : 'animate' });
  },
  clearFitToBoundsRequest: () => set({ requestFitToBounds: false }),
  requestZoomToSection: null,
  triggerZoomToSection: (sectionId: SectionId) => {
    trackNav('zoom-to-section');
    set({ requestZoomToSection: sectionId });
  },
  clearZoomToSectionRequest: () => set({ requestZoomToSection: null }),
  requestPanToShot: null,
  triggerPanToShot: (sectionId: SectionId, shotId: ShotId) => {
    trackNav('pan-to-shot');
    set({ requestPanToShot: { sectionId, shotId } });
  },
  clearPanToShotRequest: () => set({ requestPanToShot: null }),
  requestCenterOnStation: null,
  triggerCenterOnStation: (ref: StationRef) => {
    trackNav('center-on-station');
    set({ requestCenterOnStation: ref });
  },
  clearCenterOnStationRequest: () => set({ requestCenterOnStation: null }),
  requestCenterOnShot: null,
  geometryPreview: null,
  setGeometryPreview: (geometryPreview) => set({ geometryPreview }),
  requestResolveWarning: null,
  triggerResolveWarning: (sectionId: SectionId, shotId: ShotId) => {
    revealWarningsTab(set, { requestResolveWarning: { sectionId, shotId } });
  },
  openWarningsTab: () => {
    // The Warnings tab lives in the Details panel, which only Survey renders:
    // revealWarningsTab returns to Survey first (from Data, Plan or Export).
    revealWarningsTab(set, {});
  },
  clearResolveWarningRequest: () => set({ requestResolveWarning: null }),
  requestZoomToShot: null,
  triggerZoomToShot: (sectionId: SectionId, shotId: ShotId) =>
    set({ requestZoomToShot: { sectionId, shotId } }),
  clearZoomToShotRequest: () => set({ requestZoomToShot: null }),
  triggerCenterOnShot: (sectionId: SectionId, shotId: ShotId) => {
    trackNav('center-on-shot');
    set({ requestCenterOnShot: { sectionId, shotId } });
  },
  clearCenterOnShotRequest: () => set({ requestCenterOnShot: null }),

  // Display toggles (initial values from DISPLAY_DEFAULTS - single source of truth)
  showStations: DISPLAY_DEFAULTS.showStations,
  showShots: DISPLAY_DEFAULTS.showShots,
  showSectionNames: DISPLAY_DEFAULTS.showSectionNames,
  showStationLabels: DISPLAY_DEFAULTS.showStationLabels,
  showStationNames: DISPLAY_DEFAULTS.showStationNames,
  showShotData: DISPLAY_DEFAULTS.showShotData,
  showShotErrors: DISPLAY_DEFAULTS.showShotErrors,
  warningsAutoDisplay: true,
  setWarningsAutoDisplay: (warningsAutoDisplay) => set({ warningsAutoDisplay }),
  warningsVisitOverride: null,
  showReferenceShots: DISPLAY_DEFAULTS.showReferenceShots,
  showJumps: DISPLAY_DEFAULTS.showJumps,
  showZeroDatum: DISPLAY_DEFAULTS.showZeroDatum,
  showGrid: DISPLAY_DEFAULTS.showGrid,
  showSatellite: DISPLAY_DEFAULTS.showSatellite,
  showDepthGradient: DISPLAY_DEFAULTS.showDepthGradient,
  showStationComments: DISPLAY_DEFAULTS.showStationComments,
  showShotComments: DISPLAY_DEFAULTS.showShotComments,
  showDevStats: DISPLAY_DEFAULTS.showDevStats,
  showMemoryStats: DISPLAY_DEFAULTS.showMemoryStats,
  showMapStats: DISPLAY_DEFAULTS.showMapStats,
  showZoomStats: DISPLAY_DEFAULTS.showZoomStats,
  gridSpacing: DISPLAY_DEFAULTS.gridSpacing,
  // Distance-gradient overlay (#34) - runtime-only state, never persisted.
  showDistanceGradient: false,
  distanceGradientOrigin: null,
  distanceField: null,
  toggleDistanceGradient: () => {
    // Every colour writer honours a Warnings-tab visit (see setColorMode).
    const visit = useUIStore.getState();
    if (isWarningsVisitActive(visit)) {
      set(visitToggleColorPatch(visit, 'distance'));
      return;
    }
    const on = !useUIStore.getState().showDistanceGradient;
    // Distance and depth gradients are mutually exclusive - both paint the shots
    // layer, so enabling one suppresses the other (#34, report of mixed
    // depth-colored shots + distance-colored stations). We flip only depth's
    // *runtime* flag, not its persisted setting, so distance acts as a transient
    // overlay and the depth preference returns next session (distance is always
    // off on load). Turning the overlay off clears the origin + cached field, so
    // re-enabling starts fresh at the entrance (zero-datum) default.
    // Gas reachability (#38) paints the same shots/stations layer - enabling a
    // color overlay must also leave the gas sub-mode (no-op when gas is off). This
    // closes the reverse hole: setColorMode exits gas, but this action is also
    // reachable directly (e.g. a keyboard shortcut) without going through it.
    if (on) useUIStore.getState().exitGasPlan();
    set(
      on
        ? colorModeFlags('distance')
        : { showDistanceGradient: false, distanceGradientOrigin: null, distanceField: null }
    );
  },
  // Issue-severity coloring (#22) - runtime-only, never persisted.
  showIssueSeverity: false,
  toggleIssueSeverity: () => {
    const visit = useUIStore.getState();
    if (isWarningsVisitActive(visit)) {
      set(visitToggleColorPatch(visit, 'issues'));
      return;
    }
    const on = !useUIStore.getState().showIssueSeverity;
    // Mutually exclusive with both gradients - all three paint the shots/stations
    // layer. Flip depth's *runtime* flag only (don't persist), and clear the
    // distance overlay's origin/field like toggleDistanceGradient does.
    // Also leave the gas sub-mode (#38) - it owns the same layer paint.
    if (on) useUIStore.getState().exitGasPlan();
    set(on ? colorModeFlags('issues') : { showIssueSeverity: false });
  },
  setIssueSeverity: (on) => {
    const visit = useUIStore.getState();
    if (isWarningsVisitActive(visit)) {
      if ((selectEffectiveColorMode(visit) === 'issues') !== on) set(visitToggleColorPatch(visit, 'issues'));
      return;
    }
    if (useUIStore.getState().showIssueSeverity === on) return;
    // Enabling issue-severity leaves the gas sub-mode (#38) - shared layer paint.
    if (on) useUIStore.getState().exitGasPlan();
    set(on ? colorModeFlags('issues') : { showIssueSeverity: false });
  },
  setColorMode: (mode) => {
    // Selecting any color overlay leaves the gas sub-mode - both own the shots
    // layer paint, so they can't coexist (gas entry is the toolbar toggle,
    // outside this menu). Done FIRST, before the flags are read: picking 'section'
    // while gas is active must still exit it, and the exit restores the Plan-entry
    // display snapshot - so a state read taken before it would be stale. No-op
    // when gas is off.
    useUIStore.getState().exitGasPlan();
    const s = useUIStore.getState();
    // During a Warnings-tab visit the pick lasts for the visit only: it lands in
    // the visit override, the saved flags (and the project) stay as they were.
    if (isWarningsVisitActive(s)) {
      set(visitColorPatch(s, mode));
      return;
    }
    // This action is AUTHORITATIVE: it drives the three overlay flags straight
    // from the requested mode via colorModeFlags, whatever combination they were
    // in. It used to delegate to the per-overlay actions behind `if (!flag)`
    // guards, which silently no-op'd on an invalid combination - and an invalid
    // combination was reachable, because depth is persisted and distance is not
    // (see colorModeFlags). Depth on + distance on read as 'distance', picking
    // Depth hit `if (!s.showDepthGradient)` = false, and the overlay was stuck.
    //
    // Depth's persistence semantics are unchanged (#34): the distance/issue
    // overlays suppress depth at RUNTIME only, leaving the saved preference to
    // come back next session. So the persisted write (and its telemetry) happens
    // exactly when the user selected or DEselected depth itself - never when
    // another overlay took it over. toggleDisplay still owns that write; the
    // set() below is what makes the whole flag set correct regardless.
    //
    // The runtime flag is NOT a stand-in for the saved one, and the two diverge in
    // exactly the state this branch has to handle: while distance/issues suppresses
    // depth, `showDepthGradient` already reads false while the saved value is still
    // true. Picking 'None' from there is the user DEselecting depth (that is what
    // 'None' means - no overlay, suppressed or not), so it must reach the disk;
    // gating on the runtime flag alone made it a no-op and depth came back next
    // session. toggleDisplay can't do it - there is no runtime flip left to make -
    // so persist directly, with the toggle's telemetry so the pairing above holds.
    // The write is idempotent when the saved value was already false; only this one
    // transition can even reach it.
    if (mode === 'depth' || mode === 'section') {
      const wantDepth = mode === 'depth';
      if (s.showDepthGradient !== wantDepth) {
        s.toggleDisplay('showDepthGradient');
      } else if (!wantDepth && (s.showDistanceGradient || s.showIssueSeverity)) {
        trackAction('toggle.showDepthGradient');
        void s.saveDisplaySetting('showDepthGradient', false);
      }
    }
    set(colorModeFlags(mode));
  },
  setDistanceGradientOrigin: (distanceGradientOrigin) =>
    // Clear the cached field so stale colors from the previous origin aren't shown
    // under the new origin's legend/marker while the new field is being fetched.
    set({ distanceGradientOrigin, distanceField: null }),
  setDistanceField: (distanceField) => set({ distanceField }),
  // Top-level workspace mode (#38 follow-up) - runtime-only, never persisted.
  appMode: APP_MODE_DEFAULT,
  setAppMode: (mode) => {
    const s = useUIStore.getState();
    if (s.appMode === mode) return;
    const fromEditable = isEditableMode(s.appMode);
    const toEditable = isEditableMode(mode);
    // Left dock: Sections is a Survey-only tab. Leaving Survey remembers whether
    // SpeleoDB was showing (it then stays up) and points the dock at SpeleoDB, which
    // Data, Plan and Export otherwise show only on request. Returning to Survey
    // brings back the tab Survey had.
    if (s.appMode === 'survey') {
      set({ speleoOutsideSurvey: s.leftWorkspace === 'speleodb' && s.leftWorkspaceVisible, leftWorkspace: 'speleodb' });
    }
    const leftRestore: Partial<UIStore> =
      mode === 'survey' ? { leftWorkspace: s.surveyLeftWorkspace, speleoOutsideSurvey: false } : {};
    // Every mode but Survey drops the Survey-only sub-states that assume the
    // docked survey layout: section focus and the fullscreen data table. Done
    // before any snapshot below, because both exits may restore the user's real
    // pre-focus / pre-fullscreen dock state.
    if (mode !== 'survey') {
      s.exitSectionFocus();
      s.exitDataTableFullScreen();
    }

    // Survey <-> Data: both edit the same map, so the geometry mode, the display
    // flags, the docks and a pending measurement all carry over untouched - no
    // snapshot is taken or restored. Data renders neither the right nor the bottom
    // dock, but it leaves their flags alone, so Survey gets them back as they were.
    if (fromEditable && toEditable) {
      set({ appMode: mode, ...leftRestore });
      return;
    }

    if (toEditable) {
      // Read-only -> editable (Plan or Export -> Survey or Data): restore the
      // geometry mode, display flags and docks snapshotted on the way in, so the
      // read-only mode's forced closure map, its Display tweaks (and the forced
      // color-overlay clear) and its force-opened docks (#241) don't bleed into the
      // editable view. Data restores exactly what Survey would. The gas overlay +
      // markers are appMode-gated (they clear when appMode leaves 'plan'), so we do
      // NOT reset the gas store here - the loaded plan / working inputs PERSIST
      // across mode switches within a session (#244). A new session / project
      // switch still empties it (the reset on project load).
      const restore = prePlanGeometryMode ?? GEOMETRY_MODE_DEFAULT;
      prePlanGeometryMode = null;
      const displayRestore = prePlanDisplay ?? {};
      prePlanDisplay = null;
      const dockRestore = prePlanDocks ?? {};
      prePlanDocks = null;
      preReadOnlyMode = null;
      set({
        appMode: mode,
        ...leftRestore,
        geometryMode: restore,
        ...displayRestore,
        ...dockRestore,
        // The ruler readout depends on which geometry is active.
        ...MEASURE_CLEARED,
      });
      return;
    }

    // Entering Plan or Export. Both show the read-only closure map, so:
    //  - remember the editable view (only on the editable -> read-only edge - a
    //    plan <-> export switch must not overwrite the saved geometry with
    //    'closureAdjusted'), then force closureAdjusted. Set directly (not via
    //    setGeometryMode) so this forced display isn't persisted as the user's
    //    "remember last used" preference;
    //  - suppress the three color overlays (they paint the same shots/stations
    //    layer the gas overlay uses, and are a survey concern).
    if (fromEditable) {
      preReadOnlyMode = s.appMode === 'data' ? 'data' : 'survey';
      prePlanGeometryMode = s.geometryMode;
      prePlanDisplay = snapshotDisplay(s);
      // Snapshot the docks AFTER the exits above, which may themselves have just
      // restored the user's real pre-focus/pre-fullscreen dock state - capturing
      // stale pre-exit values here would restore the wrong (focus-forced) state
      // on the way back out of the read-only mode.
      const afterExit = useUIStore.getState();
      prePlanDocks = {
        detailPanelOpen: afterExit.detailPanelOpen,
        detailPanelMode: afterExit.detailPanelMode,
        dataTableOpen: afterExit.dataTableOpen,
        dataTableMode: afterExit.dataTableMode,
      };
    }
    set({
      appMode: mode,
      geometryMode: 'closureAdjusted',
      // Through colorModeFlags like every other overlay writer, so the "ONE place
      // that maps a mode back onto the three booleans" claim on it stays true and a
      // fourth overlay can never be added here and forgotten. 'section' = no
      // overlay, and it clears the distance cache the same way.
      ...colorModeFlags('section'),
      ...MEASURE_CLEARED,
    });
    // Both Plan (setup sidebar) and Export (export panel) render in the right Dock -
    // reveal it on entry so the panel surfaces even from a closed dock. Inlined
    // (not revealDetailPanel/toggleDetailPanel) because a mode-forced open is not
    // a user action: it must never persist via savePanelLayout, or the user's
    // real closed-panel preference gets silently overwritten on disk.
    const revealState = useUIStore.getState();
    if (revealState.detailPanelMode === 'hover' && revealState.focusedSectionId === null) {
      set({ detailRevealNonce: revealState.detailRevealNonce + 1 });
    } else if (!revealState.detailPanelOpen) {
      set({ detailPanelOpen: true });
    }
    // Plan no longer pins the bottom Dock here: with staged entry (#395) the
    // Cylinders/DPV/Segments/Profile tabs only exist once an origin is chosen, so
    // SingleSystemView pins it open at that moment instead (revealPlanTables).
    // We do NOT reset the gas store on entry: the overlay is appMode-gated (hidden
    // outside Plan) and a loaded plan must survive a Plan -> Export -> Plan
    // round-trip within a session (#244). The gas store is emptied only on project load.
  },
  exitGasPlan: () => {
    const ui = useUIStore.getState();
    // Survey and Data are both editable workspaces with the full Display menu:
    // a colour pick, Shift+D or an overlay there must never eject the user.
    if (isEditableMode(ui.appMode)) return;
    // Back to the editable mode the read-only one was entered from.
    ui.setAppMode(preReadOnlyMode ?? 'survey');
  },
  toggleDisplay: (key) => {
    trackAction(`toggle.${key}`);
    const state = useUIStore.getState();
    // During a Warnings-tab visit the Errors toggle and Shift+D change only the
    // visit (see setColorMode); nothing is saved.
    if (isWarningsVisitActive(state)) {
      if (key === 'showShotErrors') {
        set(visitErrorsPatch(state));
        return;
      }
      if (key === 'showDepthGradient') {
        set(visitColorPatch(state, selectEffectiveColorMode(state) === 'depth' ? 'section' : 'depth'));
        return;
      }
    }
    const newValue = !state[key];
    // Enabling the depth gradient turns off the (runtime-only) distance gradient
    // and issue-severity overlays - all three paint the shots layer (#34, #22).
    const exclusive =
      key === 'showDepthGradient' && newValue
        ? {
            showDistanceGradient: false,
            distanceGradientOrigin: null,
            distanceField: null,
            showIssueSeverity: false,
          }
        : {};
    // Enabling the depth gradient also leaves the gas sub-mode (#38) - it owns the
    // same shots/stations paint. Critical for the Shift+D keyboard shortcut, which
    // calls toggleDisplay directly (bypassing setColorMode's gas-exit).
    if (key === 'showDepthGradient' && newValue) useUIStore.getState().exitGasPlan();
    // Station IDs and station names are one choice (see EXCLUSIVE_DISPLAY_PARTNER).
    // Only the user's own key is tracked; the partner flips as a consequence.
    const partner = newValue ? EXCLUSIVE_DISPLAY_PARTNER[key] : undefined;
    const clearPartner = partner !== undefined && state[partner];
    set({ [key]: newValue, ...exclusive, ...(clearPartner ? { [partner]: false } : {}) });
    // Session-only overlays (the Debug submenu) are never restored on load, so
    // persisting them would only write orphan rows - skip the IPC call entirely.
    if (SESSION_ONLY_DISPLAY_TOGGLES.has(key)) return;
    // Persist only display changes made in an editable mode (Survey, Data - they
    // share one view). In Plan or Export the toggles are scoped to that mode
    // (snapshotted on entry, restored on exit), so they must not write to the saved
    // config. Re-read appMode: the showDepthGradient branch above calls
    // exitGasPlan() first, so a Shift+D from Plan lands (and persists) in the
    // editable mode Plan was entered from.
    if (isEditableMode(useUIStore.getState().appMode)) {
      // Station IDs / names: write the PAIR as it now stands, not just the key.
      // In memory the pair is never both-on, so this also repairs a stored pair
      // that is (a write that failed, a file edited elsewhere): the load lets
      // the IDs win, and without this a later "IDs off" would leave the stale
      // names row behind to reopen as names. The partner goes first.
      const pairKey = EXCLUSIVE_DISPLAY_PARTNER[key];
      if (pairKey !== undefined) state.saveDisplaySetting(pairKey, useUIStore.getState()[pairKey]);
      state.saveDisplaySetting(key, newValue);
    }
  },
  setGridSpacing: (spacing) => {
    set({ gridSpacing: spacing });
    // Scoped to Plan or Export while there (restored on exit) - don't persist it.
    if (!isEditableMode(useUIStore.getState().appMode)) return;
    // Persist the spacing setting
    invokeIpc(
      'save_display_setting',
      { key: 'gridSpacing', value: String(spacing) },
      { purpose: 'persist display setting', fields: { key: 'gridSpacing' } }
    ).catch((e) => {
      console.warn('Failed to save grid spacing:', e);
    });
  },
  saveDisplaySetting: async (key, value) => {
    try {
      await invokeIpc('save_display_setting', { key, value: String(value) }, {
        purpose: 'persist display setting',
        fields: { key: String(key) },
      });
    } catch (e) {
      console.warn('Failed to save display setting:', e);
    }
  },
  resetDisplaySettings: () => {
    // Reset all display toggles to their default values (from DISPLAY_DEFAULTS - single source of truth)
    set({
      showStations: DISPLAY_DEFAULTS.showStations,
      showShots: DISPLAY_DEFAULTS.showShots,
      showSectionNames: DISPLAY_DEFAULTS.showSectionNames,
      showStationLabels: DISPLAY_DEFAULTS.showStationLabels,
      showStationNames: DISPLAY_DEFAULTS.showStationNames,
      showShotData: DISPLAY_DEFAULTS.showShotData,
      showShotErrors: DISPLAY_DEFAULTS.showShotErrors,
      showReferenceShots: DISPLAY_DEFAULTS.showReferenceShots,
      showJumps: DISPLAY_DEFAULTS.showJumps,
      showZeroDatum: DISPLAY_DEFAULTS.showZeroDatum,
      showGrid: DISPLAY_DEFAULTS.showGrid,
      showSatellite: DISPLAY_DEFAULTS.showSatellite,
      showDepthGradient: DISPLAY_DEFAULTS.showDepthGradient,
  showStationComments: DISPLAY_DEFAULTS.showStationComments,
      showShotComments: DISPLAY_DEFAULTS.showShotComments,
      showDevStats: DISPLAY_DEFAULTS.showDevStats,
      showMemoryStats: DISPLAY_DEFAULTS.showMemoryStats,
      showMapStats: DISPLAY_DEFAULTS.showMapStats,
      showZoomStats: DISPLAY_DEFAULTS.showZoomStats,
      gridSpacing: DISPLAY_DEFAULTS.gridSpacing,
      geometryMode: GEOMETRY_MODE_DEFAULT,
    });
  },

  applyDisplaySettings: (settings: DisplaySettings) => {
    // A load is authoritative: drop any Plan/Export workspace mode left over from the
    // PREVIOUS project before applying this project's saved geometry below. Without this,
    // a later setView()→exitGasPlan()→setAppMode('survey') would restore the prior
    // project's saved `prePlanGeometryMode` and clobber the geometry we set here (the
    // module var outlives the project that set it). Clearing it + forcing survey here
    // makes that restore a no-op (appMode already 'survey').
    prePlanGeometryMode = null;
    preReadOnlyMode = null;
    // Same module-var-outlives-project hazard for the Plan/Export display snapshot:
    // clear it so a load doesn't later restore the previous project's survey display.
    prePlanDisplay = null;
    // Same hazard for the Plan/Export dock snapshot (#241): clear it so a load doesn't
    // later restore the previous project's dock open/mode. A direct project switch mid-Plan
    // intentionally DROPS the snapshot rather than applying it: dock state is app-global
    // (not per-project), so the forced-open docks simply stay live for the new project.
    prePlanDocks = null;
    // The gas/Plan store is per-dive, runtime-only state (origin, diver, cylinders, the
    // staged-reserve `pushyAck` safety acknowledgment). It is normally cleared on sub-mode
    // exit, but a DIRECT project switch sets appMode='survey' here, so the later
    // setView()→exitGasPlan() short-circuits and never reaches reset(). Reset it explicitly on
    // load so a previous project's plan - and especially its advanced-preset ack - can't leak
    // into the next (mirrors the showDevStats/showIssueSeverity per-load resets below).
    useGasPlanStore.getState().reset();
    // Same argument, same funnel: the Export-mode option set (format, DPI,
    // line-weight threshold, label toggles) is a rendering choice made FOR the
    // project that was open, and nothing else cleared it - `setAppMode` says so
    // explicitly. A 600-DPI PNG framed for one survey would otherwise stay
    // selected when the next opened, with the Export-mode preview rendering a
    // different cave at settings nobody chose for it. The modal Export dialog is
    // unaffected by design: it owns local state and always opens on data/CSV.
    useExportStore.getState().reset();
    // The depth / distance Levels ranges are per-project too (session-only; the
    // palette CHOICE persists in config and is untouched).
    useColorScaleStore.getState().resetSession();
    // Apply display settings from backend, merging with existing state.
    // Uses ?? to only apply fields that are defined, preserving current state for undefined fields.
    // Every persisted field matching uiStore - NO colorBy (not persisted).
    // showShots/showShotComments joined in #275 B2: they were always saved by
    // toggleDisplay but never restored (the previous project's in-memory value
    // silently won over the saved one).
    // Station IDs and names are exclusive in the menu; a stored pair that reads
    // both-on (a lost second write, or an edited file) lets the IDs win, as the
    // map and the image export do. Not written back here; the next Station IDs
    // or Station names toggle rewrites the pair (see toggleDisplay).
    const current = useUIStore.getState();
    const showStationLabels = settings.showStationLabels ?? current.showStationLabels;
    const showStationNames = !showStationLabels && (settings.showStationNames ?? current.showStationNames);
    set((state) => ({
      appMode: APP_MODE_DEFAULT,
      showStations: settings.showStations ?? state.showStations,
      showShots: settings.showShots ?? state.showShots,
      showShotComments: settings.showShotComments ?? state.showShotComments,
      showSectionNames: settings.showSectionNames ?? state.showSectionNames,
      showStationLabels,
      showStationNames,
      showShotData: settings.showShotData ?? state.showShotData,
      showShotErrors: settings.showShotErrors ?? state.showShotErrors,
      // A load starts outside any Warnings-tab visit's changes.
      warningsVisitOverride: null,
      showReferenceShots: settings.showReferenceShots ?? state.showReferenceShots,
      showJumps: settings.showJumps ?? state.showJumps,
      showZeroDatum: settings.showZeroDatum ?? state.showZeroDatum,
      showGrid: settings.showGrid ?? state.showGrid,
      showSatellite: settings.showSatellite ?? state.showSatellite,
      showStationComments: settings.showStationComments ?? state.showStationComments,
      gridSpacing: settings.gridSpacing ?? state.gridSpacing,
      // The Debug submenu's four toggles are session-only developer overlays
      // (map dev/map/zoom stats + the memory probe), not a persisted
      // per-project preference and absent from the backend payload. Reset
      // them on every load so enabling one for one project can't leak the
      // overlay - and the memory probe's 2 s memory-IPC poll - into the next.
      showDevStats: DISPLAY_DEFAULTS.showDevStats,
      showMemoryStats: DISPLAY_DEFAULTS.showMemoryStats,
      showMapStats: DISPLAY_DEFAULTS.showMapStats,
      showZoomStats: DISPLAY_DEFAULTS.showZoomStats,
      // The three color overlays are restored as ONE single-select mode, never as
      // three independent booleans: only depth is persisted, so writing the saved
      // depth flag back on its own - while the runtime-only distance/issue overlay
      // from the previous project was still on - produced the invalid pair that
      // wedged the Display menu in distance mode (see colorModeFlags). Going
      // through colorModeFlags also gives the two runtime-only overlays the
      // per-load reset they need anyway: issue-severity coloring must not leak its
      // traffic lights into the next project (issue #57 / Codex; the Warnings-tab
      // auto-switch re-derives it transiently via effectiveIssueSeverity), and the
      // distance overlay's origin/field name a station in the project we just left.
      ...colorModeFlags((settings.showDepthGradient ?? state.showDepthGradient) ? 'depth' : 'section'),
      // Restore the saved geometry mode ("remember last used"). The backend
      // defaults geometryMode to "measured" for projects with no saved
      // preference, so a load is authoritative and never inherits the previous
      // project's mode.
      geometryMode: settings.geometryMode === 'closureAdjusted' ? 'closureAdjusted' : GEOMETRY_MODE_DEFAULT,
    }));
    trackDisplayState(useUIStore.getState());
  },

  // Geometry display mode (measured vs non-destructive closure-adjusted)
  geometryMode: GEOMETRY_MODE_DEFAULT,
  setGeometryMode: (mode, source) => {
    if (useUIStore.getState().geometryMode === mode) return;
    trackAction('toggle.geometryMode', source);
    // Distances depend on the active geometry; a stale ruler readout would lie.
    set({ geometryMode: mode, ...MEASURE_CLEARED });
    // Persist for "remember last used" across sessions (string value, like gridSpacing).
    invokeIpc(
      'save_display_setting',
      { key: 'geometryMode', value: mode },
      { purpose: 'persist display setting', fields: { key: 'geometryMode' } }
    ).catch((e) => {
      console.warn('Failed to save geometry mode:', e);
    });
  },

  // Hover state
  hoveredStation: null,
  hoveredShot: null,
  hoveredSectionId: null,
  setHoveredStation: (hoveredStation) => set({ hoveredStation }),
  setHoveredShot: (hoveredShot) => set({ hoveredShot }),
  setHoveredSectionId: (hoveredSectionId) => set({ hoveredSectionId }),

  // Toast stack. Pure data: no timers live here. The dwell/fade clock (and with
  // it hover/focus pausing, which only the DOM knows about) belongs to
  // ToastStack, which calls removeToast when a toast's life ends.
  toasts: [],
  toastCapacity: MAX_VISIBLE_TOASTS,
  showToast: (message, key, kind = 'info') => {
    const prev = useUIStore.getState().toasts;

    // Same key fired again WITHIN THE BURST WINDOW: refresh in place rather than
    // stacking a duplicate. This replaces the old 5s per-key throttle - which
    // swallowed the repeat outright, so a *changed* message under a reused key
    // was lost.
    //
    // The window matters. Matching for the toast's whole visible life made a key
    // a single slot, so a user repeating an action (lock, unlock, lock, unlock)
    // saw each new toast STEAL the earlier one of its kind and the stack could
    // never grow past one per key. Those are distinct things the user did and
    // each deserves its own line. A repeat inside a second is a burst - a
    // double-click, or a loop firing the same warning - where collapsing is both
    // wanted and imperceptible, since the two would overlap anyway.
    // `performance.now()`, not `Date.now()`: a wall clock that steps backwards
    // makes the age negative, which passes this test and collapses unrelated
    // repeats for as long as the skew lasts (see `ToastItem.refreshedAt`).
    const now = performance.now();
    const existing = prev.find(
      (t) => t.key === key && !t.leaving && now - t.refreshedAt < TOAST_DEDUP_WINDOW_MS,
    );
    if (existing) {
      const refreshed: ToastItem = {
        ...existing,
        message,
        kind,
        refreshedAt: now,
        revision: existing.revision + 1,
      };
      // Move to the newest slot (end of the array = nearest the bottom edge).
      set({ toasts: [...prev.filter((t) => t.id !== existing.id), refreshed] });
      return;
    }

    const pushed: ToastItem = {
      id: nextToastId++,
      message,
      key,
      kind,
      refreshedAt: now,
      revision: 0,
      leaving: false,
    };
    set({ toasts: capToasts([...prev, pushed], useUIStore.getState().toastCapacity) });
  },
  setToastCapacity: (capacity) => {
    const clamped = clampToastCapacity(capacity);
    const state = useUIStore.getState();
    if (state.toastCapacity === clamped) return;
    // Evict on the way down as well as on push: a toast the window no longer has
    // room for must not linger invisibly with its timer still running.
    set({ toastCapacity: clamped, toasts: capToasts(state.toasts, clamped) });
  },
  dismissToast: (id) =>
    set((s) => ({ toasts: s.toasts.map((t) => (t.id === id ? { ...t, leaving: true } : t)) })),
  removeToast: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),
  clearToast: () => set({ toasts: [] }),
  toastBottomOffsets: {},
  registerToastBottomOffset: (owner, px) =>
    set((s) => ({ toastBottomOffsets: { ...s.toastBottomOffsets, [owner]: px } })),
  unregisterToastBottomOffset: (owner) =>
    set((s) => {
      if (!(owner in s.toastBottomOffsets)) return s;
      const next = { ...s.toastBottomOffsets };
      delete next[owner];
      return { toastBottomOffsets: next };
    }),

  // Snap state
  snappedStation: null,
  setSnappedStation: (snappedStation) => set({ snappedStation }),

  // Drag state
  isDragging: false,
  dragStart: null,
  dragCurrent: null,
  setDragState: (isDragging, start, current) =>
    set({
      isDragging,
      dragStart: start ?? null,
      dragCurrent: current ?? null,
    }),

  // Dialog state
  activeDialog: null,
  dialogData: {},
  dmpImportWizardPhase: null,
  openDialog: (activeDialog, dialogData = {}) => {
    if (activeDialog === 'speleodb-project') {
      useUIStore.getState().closeDialog();
      useUIStore.getState().openSpeleoWorkspace('history');
      return;
    }
    const prev = useUIStore.getState().activeDialog;
    // Re-opening the dialog already on screen is a data refresh, not a chain and
    // not a new open, so gate the BOOKKEEPING - never the state. `dialogData` is
    // still replaced below, because every reachable repeat carries a different
    // payload: a background re-verify moving `speleodb-divergence` on, a second
    // async failure re-messaging `error`, `openExitPrompt` re-raising a quit
    // prompt over itself. `context-menu` - the one dialog opened straight off a
    // pointer gesture - is kept out of this branch only by the mousedown dismissal
    // and its own full-screen backdrop, neither of which is a contract worth
    // betting a frozen menu on.
    //
    // What a repeat must NOT do is bookkeeping. Chaining is A -> B
    // (docs/telemetry.md), so completing A against itself only clobbers a pending
    // completion; and a refresh is not a second `dialog.<name>` open, which
    // double-counted the action AND burned the 50 ms `setSource` hint through
    // `consumeSource`, mis-attributing whatever the user did next.
    // `setDwellState` stays outside the gate: it already self-no-ops on an
    // unchanged key, so the dwell clock keeps running across the refresh.
    if (prev !== activeDialog) {
      if (prev) {
        // Dialog chaining - previous dialog transitions, not cancelled
        trackDialogComplete(prev);
        checkDialogCancel(prev);
      }
      trackAction(`dialog.${activeDialog}`);
    }
    setDwellState(`dialog.${activeDialog}`);
    // A ruler readout/tint shouldn't linger behind a dialog (#26 reset).
    set({ activeDialog, dialogData, dmpImportWizardPhase: null, ...MEASURE_CLEARED });
  },
  closeDialog: () => {
    const { activeDialog: dialogName, activeTool, placementMode } = useUIStore.getState();
    if (dialogName) checkDialogCancel(dialogName);
    setDwellState(placementMode ? 'placement' : `tool.${activeTool}`);
    set({ activeDialog: null, dialogData: {}, dmpImportWizardPhase: null });
  },
  setDmpImportWizardPhase: (dmpImportWizardPhase) => set({ dmpImportWizardPhase }),

  screenOccupants: 0,
  addScreenOccupant: () => set((state) => ({ screenOccupants: state.screenOccupants + 1 })),
  removeScreenOccupant: () =>
    set((state) => ({ screenOccupants: Math.max(0, state.screenOccupants - 1) })),

  // Import flow context
  importContext: null,
  setImportContext: (importContext) => set({ importContext }),

  // Session surveyors / explorers memory (not persisted to config)
  lastUsedCredit: { surveyors: [], explorers: [] },
  setLastUsedCredit: (lastUsedCredit) => set({ lastUsedCredit }),

  // Placement mode state
  placementMode: null,
  startPlacementMode: (sectionData, alignStationIndex, onComplete, onCancel, additionalSections) => {
    // Placement mode bypasses the setView() cleanup funnel, so leave the gas sub-mode
    // (#38) here too - its overlay/panel must not outlive entry into placement
    // (idempotent no-op when gas is off).
    useUIStore.getState().exitGasPlan();
    // Placement is a full-map flow: Data's table covers the map (or squeezes it into
    // a side pane), so place on Survey's map. Survey <-> Data keeps every display
    // flag and the geometry, so this costs the user nothing but the table view.
    if (useUIStore.getState().appMode === 'data') useUIStore.getState().setAppMode('survey');
    setDwellState('placement');
    set({
      // Placement REQUIRES visible stations (#518). `MapCanvas` cancels an active
      // placement the moment `showStations` goes false - it needs them to snap
      // to - so starting one with the toggle off produced a placement that died
      // before the first click, and the caller's `onCancel` put the user back
      // where they started with no explanation. Turning it on here is the same
      // shape as the `exitGasPlan()` above and the `MEASURE_CLEARED` below:
      // entering a mode settles the state that mode needs, in this one funnel
      // every placement passes through.
      //
      // NOT restored on exit, and NOT persisted. Not restored because no sibling
      // does - gas exit, the measure clear and the dwell state are all one-way -
      // and because re-hiding the stations the instant a section lands would
      // take away the thing the user just placed it against. Not persisted
      // because this is a prerequisite of a transient mode, not a preference:
      // `set` writes session state only, where `toggleDisplay` would also call
      // `saveDisplaySetting` and silently rewrite the project's saved config.
      showStations: true,
      placementMode: {
        sectionData,
        alignStationIndex,
        cursorPosition: null,
        snappedToStation: null,
        ...(onComplete !== undefined && { onComplete }),
        ...(onCancel !== undefined && { onCancel }),
        ...(additionalSections !== undefined && { additionalSections }),
      },
      // Placement and the ruler readout both occupy top-center (#26 reset).
      ...MEASURE_CLEARED,
    });
  },
  updatePlacementCursor: (position) =>
    set((state) =>
      state.placementMode
        ? {
            placementMode: {
              ...state.placementMode,
              cursorPosition: position,
            },
          }
        : state
    ),
  updatePlacementSnap: (stationRef) =>
    set((state) =>
      state.placementMode
        ? {
            placementMode: {
              ...state.placementMode,
              snappedToStation: stationRef,
            },
          }
        : state
    ),
  cancelPlacementMode: () => {
    setDwellState(`tool.${useUIStore.getState().activeTool}`);
    set({ placementMode: null });
  },
  confirmPlacement: (): PlacementModeState | null => {
    const currentPlacement = useUIStore.getState().placementMode;
    // Clear placement mode and return the state for the caller to use
    setDwellState(`tool.${useUIStore.getState().activeTool}`);
    set({ placementMode: null });
    return currentPlacement;
  },

  // Map viewport center in project coordinates
  mapViewportCenterProject: null,
  setMapViewportCenterProject: (center) => set({ mapViewportCenterProject: center }),
  getMapViewportCenter: (): { x: number; y: number } => {
    // Return stored viewport center, or default to origin (0, 0)
    const center = useUIStore.getState().mapViewportCenterProject;
    return center ?? { x: 0, y: 0 };
  },

  // Theme state
  resolvedTheme: 'dark' as ResolvedTheme,
  setResolvedTheme: (resolvedTheme: ResolvedTheme) => set({ resolvedTheme }),
  themePreview: null,
  setThemePreview: (themePreview) => set({ themePreview }),

  // Syntactic highlighting state (opt-out default: full)
  syntacticLevel: 'full' as SyntacticHighlightLevel,
  setSyntacticLevel: (syntacticLevel) => set({ syntacticLevel }),
  syntacticHighlightPreview: null,
  setSyntacticHighlightPreview: (syntacticHighlightPreview) => set({ syntacticHighlightPreview }),

  // Comment tool state
  commentTarget: null,
  setCommentTarget: (commentTarget) => set({ commentTarget }),
  clearCommentTarget: () => set({ commentTarget: null }),

  // Move comment tool state
  moveCommentSource: null,
  setMoveCommentSource: (moveCommentSource) => set({ moveCommentSource }),
  clearMoveCommentSource: () => set({ moveCommentSource: null }),

  // ARIA selection announcements for screen readers
  selectionAnnouncement: '',
  announceSelection: (message) => set({ selectionAnnouncement: message }),
}));

// Drop the Warnings-tab visit's changes as soon as the visit ends - whatever
// ended it (another tab, closing the panel, section focus, Plan / Export, the
// Global view, a full-screen table, the setting turned off). A subscription
// sees every one of those paths, including direct setState calls.
useUIStore.subscribe((s) => {
  const patch = visitEndPatch(s);
  if (patch) useUIStore.setState(patch);
});
