import { create } from 'zustand';
import { invokeIpc } from '../utils/ipc';
import { clampRound } from '../utils/clamp';
import { PANEL_SIZE_BOUNDS } from '../components/SingleSystemView/layoutConstants';
import { trackAction, checkDialogCancel, trackDialogComplete, trackDisplayState, setDwellState, advanceProjectSeq, trackNav, trackPanelToggle, trackPanelMode, trackSelect, trackSelectAdd, clearProjectActionSource, isPersistSuppressed } from '../utils/telemetry';
import type { Coordinates, DistanceField, PathShot, SectionId, ShotId, StationRef, SectionData, Marker } from '../types/project';

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
import { DISPLAY_DEFAULTS } from './displaySelectors';
import { useGasPlanStore } from './gasPlanStore';

// Import tool utilities for internal use and re-export for convenience
import { NON_INTERACTIVE_TOOLS, isNonInteractiveTool } from '../utils/toolUtils';
import { getGroupForTool } from '../utils/toolGroups';
export { NON_INTERACTIVE_TOOLS, isNonInteractiveTool };

export type ResolvedTheme = 'dark' | 'light' | 'cave' | 'dracula' | 'one_dark' | 'solarized_light' | 'catppuccin_latte' | 'nord' | 'catppuccin_mocha';

export type ViewType = 'global' | 'single';

/**
 * Cave geometry display mode.
 * - `measured`: raw survey geometry (the source of truth).
 * - `closureAdjusted`: loop-closure-corrected geometry, derived non-destructively
 *   on the backend. This view is read-only — edits are made in `measured`.
 */
export type GeometryMode = 'measured' | 'closureAdjusted';

/**
 * Top-level workspace mode (#38 follow-up, the workspace-mode reframe). Selected from the
 * top-middle carousel when a project is open:
 *  - `survey` — the editable survey workspace (everything as it was).
 *  - `plan`   — the gas planner: read-only closure map + gas overlay, Plan-setup
 *               sidebar, tabbed bottom tables.
 *  - `export` — read-only closure map + an export stub (SVG/JPG, future work).
 * Plan/Export force the read-only closure-adjusted map; only Survey edits. Runtime
 * only (never persisted) and meaningful only in single-project view.
 */
export type AppMode = 'survey' | 'plan' | 'export';
export const APP_MODE_DEFAULT: AppMode = 'survey';

/** Geometry mode used before any saved preference exists (first run). */
export const GEOMETRY_MODE_DEFAULT: GeometryMode = 'measured';

/**
 * The survey map is read-only whenever the closure-adjusted geometry is shown OR
 * we're in a non-survey workspace (Plan/Export). Both terms are load-bearing:
 * Plan/Export force `closureAdjusted`, but that auto-reverts to `measured` on a
 * project with no eligible closures (see MapCanvas), so `appMode` is the robust
 * authority — a `geometryMode`-only check leaks mutations (import, undo) into the
 * read-only workspaces (#243, the #225 bug class). One predicate so the many
 * gate sites can't drift.
 */
export function isMapReadOnly(state: Pick<UIStore, 'geometryMode' | 'appMode'>): boolean {
  return state.geometryMode === 'closureAdjusted' || state.appMode !== 'survey';
}

export type DmpImportWizardPhase = 'init' | 'suggesting' | 'manual_placing' | 'placing' | 'done';

export type DetailPanelTab = 'details' | 'warnings';

/**
 * Shot/station coloring mode. The three overlay flags (showDepthGradient /
 * showDistanceGradient / showIssueSeverity) are mutually exclusive — all paint the
 * shots/stations layer — so this enum is the single-select view of them. `section`
 * = none of the overlays (the default per-section coloring). Read via
 * {@link colorModeOf}; set via the store's `setColorMode` (the one place that owns
 * the select-one mapping).
 */
export type ColorMode = 'section' | 'depth' | 'distance' | 'issues';

/** Derive the single-select coloring mode from the three mutually-exclusive
 *  overlay flags. Use as a zustand selector: `useUIStore(colorModeOf)`. */
export function colorModeOf(
  s: Pick<UIStore, 'showIssueSeverity' | 'showDistanceGradient' | 'showDepthGradient'>,
): ColorMode {
  return s.showIssueSeverity
    ? 'issues'
    : s.showDistanceGradient
      ? 'distance'
      : s.showDepthGradient
        ? 'depth'
        : 'section';
}

/** Reveal behavior for a side/data panel.
 *  - `pin`: click to open; stays open (reflows the map) until closed.
 *  - `hover`: pops open on hover (floats over the map); hides on mouse-leave. */
export type PanelMode = 'hover' | 'pin';

/** Identifies one of the three in-project panels (matches telemetry PanelName). */
export type PanelId = 'sections' | 'detail' | 'data-table';

/**
 * A transient "ghost" geometry overlay shown on the map (in survey meters),
 * used to preview a proposed edit before it is committed — e.g. as the user
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
 * Full reset of all ruler/measure state — the single funnel spread by
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

interface UIStore {
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
  initPanels: (panels: PanelsConfig) => void;
  savePanelSizes: (panel: PanelId, size: number) => Promise<void>;
  /** Persist the full per-panel reveal layout (open + mode) globally. */
  savePanelLayout: () => Promise<void>;

  // Data-table fullscreen mode: the table overlays the whole window below the
  // toolbar. Session-only — never persisted, so the app always starts windowed.
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
  mapCenter: [number, number];
  mapZoom: number;
  setMapCenter: (center: [number, number]) => void;
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
  showShotData: boolean;
  showReferenceShots: boolean;
  showZeroDatum: boolean;
  showGrid: boolean;
  showSatellite: boolean;
  showDepthGradient: boolean;
  showStationComments: boolean;
  showShotComments: boolean;
  showDevStats: boolean;
  gridSpacing: number;
  toggleDisplay: (key: DisplayToggleKey) => void;
  setGridSpacing: (spacing: number) => void;
  loadDisplaySettings: () => Promise<void>;
  saveDisplaySetting: (key: DisplayToggleKey, value: boolean) => Promise<void>;
  resetDisplaySettings: () => void;
  /** Apply display settings from backend (merges with existing state using ??) */
  applyDisplaySettings: (settings: DisplaySettings) => void;

  // Distance-gradient overlay (#34). Runtime-only — deliberately NOT a
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

  // Top-level workspace mode (#38 follow-up): Survey / Plan / Export. Runtime-only
  // (never persisted), meaningful only in single-project view. `plan` subsumes the
  // former gas sub-mode — the survey layer, the carousel, and the panel swaps all read
  // `appMode`. Plan/Export force the read-only closure-adjusted map (see setAppMode).
  appMode: AppMode;
  /** Switch workspace mode. Entering Plan/Export forces the closure-adjusted (read-only)
   *  map and suppresses the survey color overlays; returning to Survey restores the prior
   *  geometry mode. Entering Plan reveals the Plan-setup sidebar; leaving Plan resets
   *  gasPlanStore. */
  setAppMode: (mode: AppMode) => void;
  /** Idempotent "return to Survey" — used by the setView() cleanup funnel (and the
   *  color-overlay actions) so a stale Plan/Export workspace can't outlive a project/view
   *  switch or coexist with a survey overlay. No-op when already in Survey. */
  exitGasPlan: () => void;

  // Issue-severity coloring overlay (#22). Runtime-only — like
  // showDistanceGradient it is NOT a DisplayToggleKey and is never persisted
  // (off on every load; the Warnings-tab auto-switch drives it transiently).
  showIssueSeverity: boolean;
  /** Toggle issue-severity coloring (mutually exclusive with depth/distance). */
  toggleIssueSeverity: () => void;
  /** Set issue-severity coloring explicitly (used by the Warnings-tab
   *  auto-switch). Enabling turns off both gradients. */
  setIssueSeverity: (on: boolean) => void;
  /** Select the active coloring mode (single-select over the three mutually-
   *  exclusive overlays). The one place that owns the "which overlay for which
   *  mode" mapping — delegates to the per-overlay actions so persistence/telemetry/
   *  cleanup stay identical. Read the current mode via {@link colorModeOf}. */
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

  // Marker preview — temporary marker shown on the map while the marker form is open.
  // Cleared when the form closes or the shot changes.
  previewMarker: Marker | null;
  setPreviewMarker: (marker: Marker | null) => void;

  // When set, ShotMarkerPanel for this shot should auto-open its form.
  // Cleared by ShotMarkerPanel after it consumes the signal.
  pendingMarkerOpen: { sectionId: number; shotId: number } | null;
  setPendingMarkerOpen: (v: { sectionId: number; shotId: number } | null) => void;

  // Toast state (per-key throttling)
  toast: { message: string; key: string; timestamp: number } | null;
  lastToastByKey: Record<string, number>;
  showToast: (message: string, key: string) => void;
  clearToast: () => void;

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

  // Import flow context
  importContext: 'new-project' | 'existing-project' | null;
  setImportContext: (context: 'new-project' | 'existing-project' | null) => void;

  // Session surveyor memory (not persisted to config)
  lastUsedSurveyor: string;
  setLastUsedSurveyor: (surveyor: string) => void;

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
  | 'showShotData'
  | 'showReferenceShots'
  | 'showZeroDatum'
  | 'showGrid'
  | 'showSatellite'
  | 'showDepthGradient'
  | 'showStationComments'
  | 'showShotComments'
  | 'showDevStats';

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
 * The survey geometry mode captured when entering Plan/Export, restored on return to
 * Survey. Plan/Export force `closureAdjusted` (the read-only closure map); this remembers
 * what the user had so Survey comes back to it. Module-level bookkeeping (the UI never
 * reads it), like {@link focusLayoutSnapshot}.
 */
let prePlanGeometryMode: GeometryMode | null = null;

/**
 * Display/overlay flags captured when entering Plan/Export, restored on return to
 * Survey — so Display changes made while planning/exporting are scoped to that mode
 * and never mutate (or persist to) the Survey view. Includes the color overlays that
 * Plan force-clears on entry, so entering Plan no longer silently turns a survey
 * depth/distance gradient off for good. Module-level bookkeeping like
 * {@link prePlanGeometryMode}; the persistence skip lives in `toggleDisplay` /
 * `setGridSpacing` (they only write to the saved config while in Survey).
 */
const PREPLAN_DISPLAY_KEYS = [
  'showStations',
  'showShots',
  'showReferenceShots',
  'showZeroDatum',
  'showSectionNames',
  'showStationLabels',
  'showStationComments',
  'showShotComments',
  'showShotData',
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
 * when entering Plan/Export, restored on return to Survey (#241). Plan/Export
 * force both docks open (`revealDetailPanel` + the Plan-only pinned data table)
 * so the setup/Cylinders panels surface without hunting for a closed dock;
 * without this snapshot a dock the user had closed stayed force-opened after
 * exiting back to Survey. Module-level bookkeeping like {@link prePlanGeometryMode}
 * - restored via a plain `set()` (not `toggleDetailPanel`/`toggleDataTable`), so
 * the restore itself is never persisted, mirroring the fullscreen-table snapshot.
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

export const useUIStore = create<UIStore>((set) => ({
  // View state
  view: 'global',
  setView: (view) => {
    // Section-focus mode (#50) is single-view, single-project scoped: drop it on
    // any view switch so a stale focus can't outlive the view or project that
    // opened it. No-op when not focused; restores the pre-focus panel layout.
    useUIStore.getState().exitSectionFocus();
    // Fullscreen table is single-view scoped too — a view switch can't strand it.
    useUIStore.getState().exitDataTableFullScreen();
    // Gas-planning sub-mode (#38) is single-view scoped too — drop it on any view
    // switch so a stale reachable overlay + input panel can't outlive the view.
    useUIStore.getState().exitGasPlan();
    setDwellState(view === 'global' ? 'global-view' : `tool.${useUIStore.getState().activeTool}`);
    if (view === 'single') {
      void advanceProjectSeq();
      // A project open/create/import flow has landed — the entry-point source
      // has been consumed, so clear it to prevent bleed into a later open.
      clearProjectActionSource();
    }
    // Dev stats is a single-view developer overlay — the Display toggle that
    // controls it lives only in SingleSystemView, but <MemoryProbe/> is mounted
    // app-wide. Reset it on the way back to GlobalView so the probe (and its 2 s
    // memory-IPC poll) can't strand there with no in-view way to hide it.
    //
    // The ruler readout/tint now lives in this global store, but is a single-view,
    // map-only interaction — clear it on any view switch so it can't be resurrected
    // when MapCanvas remounts on return (#26, Codex).
    set(view === 'global'
      ? { view, showDevStats: DISPLAY_DEFAULTS.showDevStats, ...MEASURE_CLEARED }
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
      // No backend restore needed — pickup doesn't mutate backend
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
  sectionsPanelOpen: true,
  sectionsPanelMode: 'pin',
  sectionsPanelWidth: 250,
  detailPanelOpen: true,
  detailPanelMode: 'pin',
  detailPanelWidth: 300,
  detailPanelTab: 'details',
  dataTableOpen: false,
  dataTableMode: 'pin',
  dataTableHeight: 200,
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
    // open transiently via the nonce — the same non-persisting idiom as
    // triggerResolveWarning — so viewing details never overwrites the user's hover
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
  initPanels: (panels: PanelsConfig) => {
    const asMode = (m: string): PanelMode => (m === 'hover' ? 'hover' : 'pin');
    set({
      sectionsPanelWidth: panels.sections_panel_width,
      detailPanelWidth: panels.detail_panel_width,
      dataTableHeight: panels.data_table_height,
      // Default the open booleans defensively. The Rust config supplies them via
      // serde defaults, but a partial config object that bypasses serde (the e2e
      // mock, or an older persisted shape) would otherwise write `undefined`,
      // leaving the side panels falsy-closed at rest. asMode() already defaults
      // the mode strings, so only the booleans need this guard.
      sectionsPanelOpen: panels.sections_panel_open ?? true,
      sectionsPanelMode: asMode(panels.sections_panel_mode),
      detailPanelOpen: panels.detail_panel_open ?? true,
      detailPanelMode: asMode(panels.detail_panel_mode),
      dataTableOpen: panels.data_table_open ?? false,
      dataTableMode: asMode(panels.data_table_mode),
    });
  },
  savePanelSizes: async (panel, size) => {
    // This persists as a Rust `u32`. clampRound guarantees the contract regardless
    // of caller: it rounds to a whole pixel (sub-pixel sizes under the webview UI
    // zoom / HiDPI fail serde and throw), folds a non-finite size to the minimum,
    // AND clamps to the panel's resize bounds so a degenerate 0 can never be
    // persisted (which would reload the panel collapsed — there is no "unset"
    // sentinel, the stored u32 is taken at face value).
    const { min, max } = PANEL_SIZE_BOUNDS[panel];
    const fields = { [PANEL_FIELDS[panel].width]: clampRound(size, min, max) };
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
    // — otherwise entering fullscreen from a hover-revealed (or closed) table
    // would leave it stuck pinned-open afterward, silently losing the user's
    // hover/closed preference.
    if (!s.dataTableFullScreen) {
      // Snapshot the table's prior open/mode so exit can restore it. While section-
      // focused (#50), the live dataTableOpen/Mode are focus-forced (open+'pin'),
      // NOT the user's real preference — that lives in focusLayoutSnapshot. Capture
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
    // — re-apply that instead of restoring the pre-fullscreen snapshot (exit-focus
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
    // measured geometry — guaranteeing the focused data table is editable.
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
  mapCenter: [0, 0],
  mapZoom: 15,
  setMapCenter: (mapCenter) => set({ mapCenter }),
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
    // Make sure the resolve view will actually mount and be visible before
    // requesting it: section-focus and data-table fullscreen hide/cover the
    // detail panel, and the panel may simply be collapsed. WarningsContent only
    // consumes the request while mounted (detailPanelOpen && tab === 'warnings'),
    // so open the panel on the Warnings tab here. (Otherwise the click would be
    // a no-op and the request would sit stale and fire on a later tab switch.)
    let s = useUIStore.getState();
    if (s.focusedSectionId !== null) s.exitSectionFocus();
    if (s.dataTableFullScreen) s.exitDataTableFullScreen();
    // Re-read: the exits above can change the panel's mode/open.
    s = useUIStore.getState();
    // Reveal the Details panel on the Warnings tab so WarningsContent mounts and
    // consumes the request — without mutating the user's persisted panel mode.
    // Pinned: just ensure it's open. Hover: bump the reveal nonce so SingleSystemView
    // floats it open like a hover (with a timed auto-close, since no pointer-leave
    // fires for a programmatic reveal), so resolving a warning never silently leaves
    // a hover-mode user pinned.
    // For a pinned-but-closed panel, opening it is a real (persistent) open just
    // like toggleDetailPanel — persist it so the in-memory open flag doesn't
    // diverge from the saved layout (which would re-close the panel next session
    // and could be clobbered by any later savePanelLayout).
    const pinOpening = s.detailPanelMode === 'pin' && !s.detailPanelOpen;
    set({
      requestResolveWarning: { sectionId, shotId },
      detailPanelTab: 'warnings',
      ...(s.detailPanelMode === 'pin'
        ? { detailPanelOpen: true }
        : { detailRevealNonce: s.detailRevealNonce + 1 }),
    });
    if (pinOpening) void useUIStore.getState().savePanelLayout();
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
  showShotData: DISPLAY_DEFAULTS.showShotData,
  showReferenceShots: DISPLAY_DEFAULTS.showReferenceShots,
  showZeroDatum: DISPLAY_DEFAULTS.showZeroDatum,
  showGrid: DISPLAY_DEFAULTS.showGrid,
  showSatellite: DISPLAY_DEFAULTS.showSatellite,
  showDepthGradient: DISPLAY_DEFAULTS.showDepthGradient,
  showStationComments: DISPLAY_DEFAULTS.showStationComments,
  showShotComments: DISPLAY_DEFAULTS.showShotComments,
  showDevStats: DISPLAY_DEFAULTS.showDevStats,
  gridSpacing: DISPLAY_DEFAULTS.gridSpacing,
  // Distance-gradient overlay (#34) — runtime-only state, never persisted.
  showDistanceGradient: false,
  distanceGradientOrigin: null,
  distanceField: null,
  toggleDistanceGradient: () => {
    const on = !useUIStore.getState().showDistanceGradient;
    // Distance and depth gradients are mutually exclusive — both paint the shots
    // layer, so enabling one suppresses the other (#34, report of mixed
    // depth-colored shots + distance-colored stations). We flip only depth's
    // *runtime* flag, not its persisted setting, so distance acts as a transient
    // overlay and the depth preference returns next session (distance is always
    // off on load). Turning the overlay off clears the origin + cached field, so
    // re-enabling starts fresh at the entrance (zero-datum) default.
    // Gas reachability (#38) paints the same shots/stations layer — enabling a
    // color overlay must also leave the gas sub-mode (no-op when gas is off). This
    // closes the reverse hole: setColorMode exits gas, but this action is also
    // reachable directly (e.g. a keyboard shortcut) without going through it.
    if (on) useUIStore.getState().exitGasPlan();
    set(
      on
        ? { showDistanceGradient: true, showDepthGradient: false, showIssueSeverity: false }
        : { showDistanceGradient: false, distanceGradientOrigin: null, distanceField: null }
    );
  },
  // Issue-severity coloring (#22) — runtime-only, never persisted.
  showIssueSeverity: false,
  toggleIssueSeverity: () => {
    const on = !useUIStore.getState().showIssueSeverity;
    // Mutually exclusive with both gradients — all three paint the shots/stations
    // layer. Flip depth's *runtime* flag only (don't persist), and clear the
    // distance overlay's origin/field like toggleDistanceGradient does.
    // Also leave the gas sub-mode (#38) — it owns the same layer paint.
    if (on) useUIStore.getState().exitGasPlan();
    set(
      on
        ? {
            showIssueSeverity: true,
            showDepthGradient: false,
            showDistanceGradient: false,
            distanceGradientOrigin: null,
            distanceField: null,
          }
        : { showIssueSeverity: false }
    );
  },
  setIssueSeverity: (on) => {
    if (useUIStore.getState().showIssueSeverity === on) return;
    // Enabling issue-severity leaves the gas sub-mode (#38) — shared layer paint.
    if (on) useUIStore.getState().exitGasPlan();
    set(
      on
        ? {
            showIssueSeverity: true,
            showDepthGradient: false,
            showDistanceGradient: false,
            distanceGradientOrigin: null,
            distanceField: null,
          }
        : { showIssueSeverity: false }
    );
  },
  setColorMode: (mode) => {
    const s = useUIStore.getState();
    // Selecting any color overlay leaves the gas sub-mode — both own the shots
    // layer paint, so they can't coexist (gas entry is the toolbar toggle,
    // outside this menu). Done before the no-op check so picking 'section' while
    // gas is active still exits it. No-op when gas is off.
    s.exitGasPlan();
    if (colorModeOf(s) === mode) return;
    // Single entry point for the mutually-exclusive coloring overlays. Delegates to
    // the per-overlay actions, which each turn the others off and handle their own
    // persistence (depth is a saved DisplayToggleKey), telemetry, and cleanup (the
    // distance overlay clears its origin/field) — so the "select one of N" mapping
    // lives here in the store instead of being duplicated in the DisplayMenu pills.
    if (mode === 'depth') {
      if (!s.showDepthGradient) s.toggleDisplay('showDepthGradient');
    } else if (mode === 'distance') {
      if (!s.showDistanceGradient) s.toggleDistanceGradient();
    } else if (mode === 'issues') {
      s.setIssueSeverity(true);
    } else {
      // 'section' = none: turn off whichever single overlay is currently on.
      if (s.showDepthGradient) s.toggleDisplay('showDepthGradient');
      else if (s.showDistanceGradient) s.toggleDistanceGradient();
      else if (s.showIssueSeverity) s.setIssueSeverity(false);
    }
  },
  setDistanceGradientOrigin: (distanceGradientOrigin) =>
    // Clear the cached field so stale colors from the previous origin aren't shown
    // under the new origin's legend/marker while the new field is being fetched.
    set({ distanceGradientOrigin, distanceField: null }),
  setDistanceField: (distanceField) => set({ distanceField }),
  // Top-level workspace mode (#38 follow-up) — runtime-only, never persisted.
  appMode: APP_MODE_DEFAULT,
  setAppMode: (mode) => {
    const s = useUIStore.getState();
    if (s.appMode === mode) return;
    if (mode === 'survey') {
      // Returning to Survey: restore the geometry mode we had before entering
      // Plan/Export. The gas overlay + markers are appMode-gated (they clear when
      // appMode leaves 'plan'), so we do NOT reset the gas store here — the loaded
      // plan / working inputs PERSIST across mode switches within a session (#244).
      // A new session / project switch still empties it (the reset on project load).
      const restore = prePlanGeometryMode ?? GEOMETRY_MODE_DEFAULT;
      prePlanGeometryMode = null;
      // Restore the Survey display flags captured on entry so Plan/Export tweaks
      // (and the forced color-overlay clear) don't bleed into the survey view.
      const displayRestore = prePlanDisplay ?? {};
      prePlanDisplay = null;
      // Restore the docks Plan/Export force-opened (#241) - a dock the user had
      // closed before entering must come back closed, not stay stranded open.
      const dockRestore = prePlanDocks ?? {};
      prePlanDocks = null;
      set({
        appMode: 'survey',
        geometryMode: restore,
        ...displayRestore,
        ...dockRestore,
        ...MEASURE_CLEARED,
      });
      return;
    }
    // Entering Plan or Export. Both show the read-only closure map, so:
    //  - drop the survey-editing modes (section focus, fullscreen table) that
    //    assume an editable measured view;
    //  - remember the survey geometry mode (only when coming FROM survey — a
    //    plan↔export switch must not overwrite the saved survey value with
    //    'closureAdjusted'), then force closureAdjusted. Set directly (not via
    //    setGeometryMode) so this forced display isn't persisted as the user's
    //    "remember last used" preference;
    //  - suppress the three color overlays (they paint the same shots/stations
    //    layer the gas overlay uses, and are a survey concern).
    s.exitSectionFocus();
    s.exitDataTableFullScreen();
    if (s.appMode === 'survey') {
      prePlanGeometryMode = s.geometryMode;
      prePlanDisplay = snapshotDisplay(s);
      // Snapshot the docks AFTER the exits above, which may themselves have just
      // restored the user's real pre-focus/pre-fullscreen dock state - capturing
      // stale pre-exit values here would restore the wrong (focus-forced) state
      // on the way back out of Plan/Export.
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
      showDepthGradient: false,
      showDistanceGradient: false,
      showIssueSeverity: false,
      distanceGradientOrigin: null,
      distanceField: null,
      ...MEASURE_CLEARED,
    });
    // Both Plan (setup sidebar) and Export (export stub) render in the right Dock —
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
    if (mode === 'plan') {
      // Plan also pins the bottom Dock open so the Cylinders/Profile tabs are visible
      // without hunting for a closed dock.
      set({ dataTableOpen: true, dataTableMode: 'pin' });
    }
    // Export shows the read-only map + export stub. We do NOT reset the gas store on
    // entry: the overlay is appMode-gated (hidden outside Plan) and a loaded plan must
    // survive a Plan -> Export -> Plan round-trip within a session (#244). The gas store
    // is emptied only on project load.
  },
  exitGasPlan: () => {
    if (useUIStore.getState().appMode === 'survey') return;
    useUIStore.getState().setAppMode('survey');
  },
  toggleDisplay: (key) => {
    trackAction(`toggle.${key}`);
    const state = useUIStore.getState();
    const newValue = !state[key];
    // Enabling the depth gradient turns off the (runtime-only) distance gradient
    // and issue-severity overlays — all three paint the shots layer (#34, #22).
    const exclusive =
      key === 'showDepthGradient' && newValue
        ? {
            showDistanceGradient: false,
            distanceGradientOrigin: null,
            distanceField: null,
            showIssueSeverity: false,
          }
        : {};
    // Enabling the depth gradient also leaves the gas sub-mode (#38) — it owns the
    // same shots/stations paint. Critical for the Shift+D keyboard shortcut, which
    // calls toggleDisplay directly (bypassing setColorMode's gas-exit).
    if (key === 'showDepthGradient' && newValue) useUIStore.getState().exitGasPlan();
    set({ [key]: newValue, ...exclusive });
    // Persist only Survey display changes. In Plan/Export the toggles are scoped to
    // that mode (snapshotted on entry, restored on exit), so they must not write to
    // the saved survey config. Re-read appMode: the showDepthGradient branch above
    // calls exitGasPlan() first, so a Shift+D from Plan lands (and persists) in Survey.
    if (useUIStore.getState().appMode === 'survey') {
      state.saveDisplaySetting(key, newValue);
    }
  },
  setGridSpacing: (spacing) => {
    set({ gridSpacing: spacing });
    // Scoped to Plan/Export while there (restored on exit) — don't persist it.
    if (useUIStore.getState().appMode !== 'survey') return;
    // Persist the spacing setting
    invokeIpc(
      'save_display_setting',
      { key: 'gridSpacing', value: String(spacing) },
      { purpose: 'persist display setting', fields: { key: 'gridSpacing' } }
    ).catch((e) => {
      console.warn('Failed to save grid spacing:', e);
    });
  },
  loadDisplaySettings: async () => {
    try {
      const settings = await invokeIpc<Record<string, string>>('load_display_settings', undefined, {
        purpose: 'load display settings for UI',
      });
      const displayState: Partial<Record<DisplayToggleKey, boolean>> = {};
      let gridSpacing: number | undefined;
      let geometryMode: GeometryMode | undefined;
      for (const [key, value] of Object.entries(settings)) {
        if (key.startsWith('show')) {
          displayState[key as DisplayToggleKey] = value === 'true';
        } else if (key === 'gridSpacing') {
          const parsed = parseFloat(value);
          if (!isNaN(parsed) && parsed > 0) {
            gridSpacing = parsed;
          }
        } else if (key === 'geometryMode') {
          // "Remember last used": restore the persisted geometry mode, ignoring
          // any unrecognized value.
          if (value === 'measured' || value === 'closureAdjusted') {
            geometryMode = value;
          }
        }
      }
      // Migration fallback: if we loaded some settings but showShots wasn't among them,
      // default it to true (older saved settings won't have this newer key)
      const hasLoadedSettings =
        Object.keys(displayState).length > 0 || gridSpacing !== undefined || geometryMode !== undefined;
      if (hasLoadedSettings && displayState.showShots === undefined) {
        displayState.showShots = true;
      }
      // geometryMode is per-project ("remember last used"): restore the saved value
      // or reset to the measured default. Applied unconditionally so a project with
      // no saved preference (or no display settings at all) doesn't inherit the
      // previous project's mode.
      set({ geometryMode: geometryMode ?? GEOMETRY_MODE_DEFAULT });
      if (hasLoadedSettings) {
        set({
          ...displayState,
          ...(gridSpacing !== undefined ? { gridSpacing } : {}),
        });
        trackDisplayState(useUIStore.getState());
      }
    } catch (e) {
      // No project open or other error - use defaults
      console.warn('Failed to load display settings:', e);
    }
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
      showShotData: DISPLAY_DEFAULTS.showShotData,
      showReferenceShots: DISPLAY_DEFAULTS.showReferenceShots,
      showZeroDatum: DISPLAY_DEFAULTS.showZeroDatum,
      showGrid: DISPLAY_DEFAULTS.showGrid,
      showSatellite: DISPLAY_DEFAULTS.showSatellite,
      showDepthGradient: DISPLAY_DEFAULTS.showDepthGradient,
  showStationComments: DISPLAY_DEFAULTS.showStationComments,
      showShotComments: DISPLAY_DEFAULTS.showShotComments,
      showDevStats: DISPLAY_DEFAULTS.showDevStats,
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
    // load so a previous project's plan — and especially its advanced-preset ack — can't leak
    // into the next (mirrors the showDevStats/showIssueSeverity per-load resets below).
    useGasPlanStore.getState().reset();
    // Apply display settings from backend, merging with existing state.
    // Uses ?? to only apply fields that are defined, preserving current state for undefined fields.
    // EXACT 13 persisted fields matching uiStore - NO colorBy (not persisted).
    // showShots/showShotComments joined in #275 B2: they were always saved by
    // toggleDisplay but never restored (the previous project's in-memory value
    // silently won over the saved one).
    set((state) => ({
      appMode: APP_MODE_DEFAULT,
      showStations: settings.showStations ?? state.showStations,
      showShots: settings.showShots ?? state.showShots,
      showShotComments: settings.showShotComments ?? state.showShotComments,
      showSectionNames: settings.showSectionNames ?? state.showSectionNames,
      showStationLabels: settings.showStationLabels ?? state.showStationLabels,
      showShotData: settings.showShotData ?? state.showShotData,
      showReferenceShots: settings.showReferenceShots ?? state.showReferenceShots,
      showZeroDatum: settings.showZeroDatum ?? state.showZeroDatum,
      showGrid: settings.showGrid ?? state.showGrid,
      showSatellite: settings.showSatellite ?? state.showSatellite,
      showDepthGradient: settings.showDepthGradient ?? state.showDepthGradient,
      showStationComments: settings.showStationComments ?? state.showStationComments,
      gridSpacing: settings.gridSpacing ?? state.gridSpacing,
      // Dev stats is a session-only developer overlay (memory probe + map dev
      // stats), not a persisted per-project preference and absent from the
      // backend payload. Reset it on every load so enabling it for one project
      // can't leak the overlay — and its 2 s memory-IPC poll — into the next.
      showDevStats: DISPLAY_DEFAULTS.showDevStats,
      // Issue-severity coloring is a runtime-only overlay (never persisted); reset
      // it on every load so a manual enable in one project doesn't leak the
      // traffic-light coloring into the next (issue #57 / Codex). The Warnings-tab
      // auto-switch re-derives it transiently via effectiveIssueSeverity.
      showIssueSeverity: false,
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

  // Marker preview
  previewMarker: null,
  setPreviewMarker: (previewMarker) => set({ previewMarker }),

  pendingMarkerOpen: null,
  setPendingMarkerOpen: (pendingMarkerOpen) => set({ pendingMarkerOpen }),

  // Toast state (per-key throttling)
  toast: null,
  lastToastByKey: {},
  showToast: (message, key) => {
    const now = Date.now();
    const lastTime = useUIStore.getState().lastToastByKey[key] || 0;

    // Throttle: only show if no recent toast for this key (5 second cooldown)
    if (now - lastTime > 5000) {
      set({
        toast: { message, key, timestamp: now },
        lastToastByKey: { ...useUIStore.getState().lastToastByKey, [key]: now },
      });
      // Auto-clear after 3 seconds
      setTimeout(() => {
        const t = useUIStore.getState().toast;
        if (t && t.timestamp === now) {
          set({ toast: null });
        }
      }, 3000);
    }
  },
  clearToast: () => set({ toast: null }),

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
    const prev = useUIStore.getState().activeDialog;
    if (prev) {
      // Dialog chaining — previous dialog transitions, not cancelled
      trackDialogComplete(prev);
      checkDialogCancel(prev);
    }
    trackAction(`dialog.${activeDialog}`);
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

  // Import flow context
  importContext: null,
  setImportContext: (importContext) => set({ importContext }),

  // Session surveyor memory (not persisted to config)
  lastUsedSurveyor: '',
  setLastUsedSurveyor: (lastUsedSurveyor) => set({ lastUsedSurveyor }),

  // Placement mode state
  placementMode: null,
  startPlacementMode: (sectionData, alignStationIndex, onComplete, onCancel, additionalSections) => {
    // Placement mode bypasses the setView() cleanup funnel, so leave the gas sub-mode
    // (#38) here too — its overlay/panel must not outlive entry into placement
    // (idempotent no-op when gas is off).
    useUIStore.getState().exitGasPlan();
    setDwellState('placement');
    set({
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
