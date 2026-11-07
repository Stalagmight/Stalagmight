import { useEffect, useCallback, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../store/uiStore';
import { useProjectStore } from '../store/projectStore';
import { useGasPlanStore } from '../store/gasPlanStore';
import { useShallow } from 'zustand/react/shallow';
import { isEditingBlockedNow } from './useEditingBlocked';
import { isNonInteractiveTool, navigationToolsEnabled } from '../utils/toolUtils';
import { normalizeShortcut, parseKeyEvent } from '../utils/shortcuts';
import { surfaceSaveFailure } from '../utils/saveFeedback';
import { runFileShortcutAction } from '../utils/fileShortcutActions';
import { saveProjectAs } from '../utils/projectActions';
import type { ToolType, Selection, DisplayToggleKey } from '../store/uiStore';
import { setSource, trackNav } from '../utils/telemetry';
import type { ShortcutsConfig, ShortcutFeedback } from '../types/config';
import { defaultConfig } from '../types/config';
import { TOAST_KEYS, shortcutToastKey } from '../utils/toastKeys';

/**
 * Action types that can be triggered by keyboard shortcuts.
 */
type ShortcutAction =
  // Tools
  | 'select'
  | 'pan'
  | 'zoom'
  | 'merge'
  | 'measure'
  | 'move_station'
  | 'move_segment'
  | 'move_shot'
  | 'comment_tool'
  | 'move_comment_tool'
  // Edit
  | 'undo'
  | 'redo'
  // View
  | 'toggle_depth_gradient'
  | 'toggle_grid'
  | 'toggle_satellite'
  | 'toggle_station_labels'
  | 'toggle_shot_labels'
  | 'toggle_section_labels'
  | 'fit_to_bounds'
  | 'zoom_in'
  | 'zoom_out'
  // Panels
  | 'toggle_sections_panel'
  | 'toggle_detail_panel'
  | 'toggle_data_table'
  // File
  | 'new_file'
  | 'open_file'
  | 'save'
  | 'save_as'
  | 'import_dmp'
  | 'export'
  | 'quit';

/**
 * Map of action names to the corresponding tool types.
 */
const TOOL_ACTIONS: Partial<Record<ShortcutAction, ToolType>> = {
  select: 'select',
  pan: 'pan',
  zoom: 'zoom',
  merge: 'merge',
  measure: 'measure',
  move_station: 'move_station',
  move_segment: 'move_segment',
  move_shot: 'move_shot',
  comment_tool: 'comment',
  move_comment_tool: 'move_comment',
};

/**
 * Map of action names to display toggle keys.
 */
const DISPLAY_TOGGLE_ACTIONS: Partial<Record<ShortcutAction, DisplayToggleKey>> = {
  toggle_depth_gradient: 'showDepthGradient',
  toggle_grid: 'showGrid',
  toggle_satellite: 'showSatellite',
  toggle_station_labels: 'showStationLabels',
  toggle_shot_labels: 'showShotData',
  toggle_section_labels: 'showSectionNames',
};

/**
 * Actions that are "non-obvious" - view toggles and panel toggles.
 * These show toasts when feedback is set to 'non_obvious' or 'all'.
 */
const NON_OBVIOUS_ACTIONS: Set<ShortcutAction> = new Set([
  // View toggles
  'toggle_depth_gradient',
  'toggle_grid',
  'toggle_satellite',
  'toggle_station_labels',
  'toggle_shot_labels',
  'toggle_section_labels',
  // Panel toggles
  'toggle_sections_panel',
  'toggle_detail_panel',
  'toggle_data_table',
]);

/**
 * Human-readable labels for shortcut actions (used in toast messages).
 */
const ACTION_LABELS: Record<ShortcutAction, string> = {
  select: 'Select tool',
  pan: 'Pan tool',
  zoom: 'Zoom tool',
  merge: 'Merge tool',
  measure: 'Measure tool',
  move_station: 'Move station tool',
  move_segment: 'Move segment tool',
  move_shot: 'Move shot tool',
  comment_tool: 'Comment tool',
  move_comment_tool: 'Move comment tool',
  undo: 'Undo',
  redo: 'Redo',
  toggle_depth_gradient: 'Depth gradient',
  toggle_grid: 'Grid',
  toggle_satellite: 'Satellite',
  toggle_station_labels: 'Station labels',
  toggle_shot_labels: 'Shot labels',
  toggle_section_labels: 'Section labels',
  fit_to_bounds: 'Fit to bounds',
  zoom_in: 'Zoom in',
  zoom_out: 'Zoom out',
  toggle_sections_panel: 'Sections panel',
  toggle_detail_panel: 'Detail panel',
  toggle_data_table: 'Data table',
  new_file: 'New',
  open_file: 'Open',
  save: 'Save',
  save_as: 'Save As',
  // Matches the visible Import button and menu row. Deliberately not
  // "...survey data": with a project open the action takes `.dmp` only (#508),
  // and this one label is shown in both contexts.
  import_dmp: 'Import',
  export: 'Export',
  quit: 'Quit',
};

/**
 * During placement mode, only keep display toggles and map navigation shortcuts active.
 */
const PLACEMENT_MODE_ALLOWED_SHORTCUTS: Set<ShortcutAction> = new Set([
  'toggle_depth_gradient',
  'toggle_grid',
  'toggle_satellite',
  'toggle_station_labels',
  'toggle_shot_labels',
  'toggle_section_labels',
  'fit_to_bounds',
  'zoom_in',
  'zoom_out',
  // Quitting the app is global and must work in every mode (matches the menu Quit).
  'quit',
]);

/**
 * While the data table is fullscreen, only save/undo/redo stay active. Escape
 * and the `toggle_data_table` action exit the mode; every other shortcut is
 * blocked so stray tool/import keys can't act on the hidden map. Grid-internal
 * keys are unaffected - the grid stops propagation of what it handles.
 */
const FULLSCREEN_TABLE_ALLOWED_SHORTCUTS: Set<ShortcutAction> = new Set([
  'save',
  'undo',
  'redo',
  // Quitting the app is global and must work in every mode (matches the menu Quit).
  'quit',
]);

/**
 * Determine if a toast should be shown for an action based on feedback preference.
 */
function shouldShowToast(action: ShortcutAction, feedback: ShortcutFeedback): boolean {
  if (feedback === 'none') return false;
  if (feedback === 'all') return true;
  // 'non_obvious' - only show for view/panel toggles
  return NON_OBVIOUS_ACTIONS.has(action);
}

/**
 * Build a map from normalized shortcut strings to their action names.
 * This allows O(1) lookup during keydown handling.
 */
export function buildShortcutMap(shortcuts: ShortcutsConfig): Map<string, ShortcutAction> {
  const map = new Map<string, ShortcutAction>();

  const actions: (keyof ShortcutsConfig)[] = [
    // Tools
    'select',
    'pan',
    'zoom',
    'merge',
    'measure',
    'move_station',
    'move_segment',
    'move_shot',
    'comment_tool',
    'move_comment_tool',
    // Edit
    'undo',
    'redo',
    // View
    'toggle_depth_gradient',
    'toggle_grid',
    'toggle_satellite',
    'toggle_station_labels',
    'toggle_shot_labels',
    'toggle_section_labels',
    'fit_to_bounds',
    'zoom_in',
    'zoom_out',
    // Panels
    'toggle_sections_panel',
    'toggle_detail_panel',
    'toggle_data_table',
    // File
    'new_file',
    'open_file',
    'save',
    'save_as',
    'import_dmp',
    'export',
    'quit',
  ];

  for (const action of actions) {
    const bindings = shortcuts[action];
    if (!Array.isArray(bindings)) continue;

    for (const binding of bindings) {
      const normalized = normalizeShortcut(binding);
      if (normalized) {
        map.set(normalized, action as ShortcutAction);
      }
    }
  }

  return map;
}

/**
 * Get global map reference from window.
 */
function getMap(): {
  getZoom: () => number;
  getMinZoom: () => number;
  getMaxZoom: () => number;
  zoomTo: (zoom: number, options?: { duration?: number }) => void;
} | null {
  return (
    window as unknown as {
      __stalagmight_map__?: {
        getZoom: () => number;
        getMinZoom: () => number;
        getMaxZoom: () => number;
        zoomTo: (zoom: number, options?: { duration?: number }) => void;
      };
    }
  ).__stalagmight_map__ ?? null;
}

/**
 * Handle zoom in action with configurable step and limits.
 */
function handleZoomIn(zoomStep: number, zoomMax: number): void {
  const map = getMap();
  if (!map) return;

  const currentZoom = map.getZoom();
  const maxZoom = Math.min(zoomMax, map.getMaxZoom());
  const targetZoom = Math.min(maxZoom, currentZoom + zoomStep);

  if (targetZoom !== currentZoom) {
    map.zoomTo(targetZoom, { duration: 300 });
  }
}

/**
 * Handle zoom out action with configurable step and limits.
 */
function handleZoomOut(zoomStep: number, zoomMin: number): void {
  const map = getMap();
  if (!map) return;

  const currentZoom = map.getZoom();
  const minZoom = Math.max(zoomMin, map.getMinZoom());
  const targetZoom = Math.max(minZoom, currentZoom - zoomStep);

  if (targetZoom !== currentZoom) {
    map.zoomTo(targetZoom, { duration: 300 });
  }
}

export function useKeyboardShortcuts() {
  const { t } = useTranslation();
  const {
    setActiveTool,
    toggleSectionsPanel,
    toggleDetailPanel,
    toggleDataTable,
    toggleDisplay,
    clearSelection,
    selectAll,
    triggerFitToBounds,
    selection,
    openDialog,
    clearMergeStations,
    clearMeasure,
    showStations,
    showToast,
  } = useUIStore(useShallow((s) => ({
    setActiveTool: s.setActiveTool,
    toggleSectionsPanel: s.toggleSectionsPanel,
    toggleDetailPanel: s.toggleDetailPanel,
    toggleDataTable: s.toggleDataTable,
    toggleDisplay: s.toggleDisplay,
    clearSelection: s.clearSelection,
    selectAll: s.selectAll,
    triggerFitToBounds: s.triggerFitToBounds,
    selection: s.selection,
    openDialog: s.openDialog,
    clearMergeStations: s.clearMergeStations,
    clearMeasure: s.clearMeasure,
    showStations: s.showStations,
    showToast: s.showToast,
  })));
  const { undo, redo, saveProject, project, stations, config } = useProjectStore(
    useShallow((s) => ({
      undo: s.undo,
      redo: s.redo,
      saveProject: s.saveProject,
      project: s.project,
      stations: s.stations,
      config: s.config,
    })),
  );

  // Ref for shortcut map - uses ref to avoid stale closures in event handlers
  // Initialize with defaults to handle first render before useEffect runs
  const shortcutMapRef = useRef<Map<string, ShortcutAction>>(buildShortcutMap(defaultConfig.shortcuts));

  // Update ref when config changes - use defaultConfig as fallback
  useEffect(() => {
    const shortcuts = config?.shortcuts ?? defaultConfig.shortcuts;
    shortcutMapRef.current = buildShortcutMap(shortcuts);
  }, [config?.shortcuts]);

  const handleKeyDown = useCallback(
    (event: KeyboardEvent) => {
      // Honor a key already consumed by a focused component. This window-level
      // handler runs after React's bubble phase, so a grid-owned shortcut (the
      // docked data-table grid's Ctrl/Cmd+A select-all, type-to-edit `?`, etc.)
      // has already called preventDefault; without this it would ALSO fire the
      // matching global shortcut (e.g. select-all-stations, switching the table
      // to Stations). Mirrors the existing defaultPrevented guard in the Escape
      // branch, generalized to every reserved key.
      if (event.defaultPrevented) return;
      const { activeDialog, screenOccupants } = useUIStore.getState();
      // #576 D39(c): a prompt outside the `activeDialog` slot (held-lock, telemetry
      // opt-in, storage choice, dump picker - `useScreenOccupant`) owns the keyboard
      // like a dialog, Escape included: its own frame answers Escape, and the branch
      // below would otherwise clear the selection or close a dialog beneath it.
      if (screenOccupants > 0) return;
      if (activeDialog && event.key !== 'Escape') {
        // Allow ? to toggle the keyboard shortcuts overlay closed
        if (!(event.key === '?' && activeDialog === 'keyboard-shortcuts')) {
          return;
        }
      }

      setSource('keyboard');

      const placementModeActive = useUIStore.getState().placementMode !== null;
      const dataTableFullScreen = useUIStore.getState().dataTableFullScreen;

      // Handle Cmd/Ctrl+, to open Settings - works even when focus is in an input
      // Must be BEFORE the input guard check
      if ((event.metaKey || event.ctrlKey) && event.key === ',') {
        if (placementModeActive || dataTableFullScreen) {
          event.preventDefault();
          return;
        }
        event.preventDefault();
        // Only open if Settings is not already open
        if (activeDialog !== 'settings') {
          openDialog('settings');
        }
        return;
      }

      // Handle Escape FIRST - works even when focus is in an input
      if (event.key === 'Escape') {
        // #395: the one-shot "Pick on map" origin picker disarms on Escape, the same as any
        // other one-shot map-click mode, and takes it over the same way placement mode does -
        // but only when no dialog is open, or it would swallow the dialog's own Escape (an
        // open dialog's close button wins; the picker stays armed underneath it).
        if (!useUIStore.getState().activeDialog && useGasPlanStore.getState().originPickArmed) {
          useGasPlanStore.getState().setOriginPickArmed(false);
          return;
        }

        // Placement mode owns Escape behavior (handled in MapCanvas).
        if (placementModeActive) {
          return;
        }

        // Skip if already handled (e.g., by a dropdown closing)
        if (event.defaultPrevented) {
          return;
        }

        // Skip if a dropdown is open - let the dropdown handle Escape first
        const openDropdown = document.querySelector('[data-dropdown-open]');
        if (openDropdown) {
          return;
        }

        // Close any open dialog
        const { activeDialog, closeDialog } = useUIStore.getState();
        if (activeDialog) {
          event.preventDefault();
          closeDialog();
          // IMPORTANT: Don't return - continue to clear tool state
        } else if (dataTableFullScreen) {
          // Fullscreen data table: with no dialog to close, Escape exits the
          // mode (grid-internal Escape already won via its preventDefault).
          event.preventDefault();
          useUIStore.getState().exitDataTableFullScreen();
          return;
        }

        // Always clear tool state (existing behavior)
        clearSelection();
        clearMergeStations();
        clearMeasure();
        useUIStore.getState().clearCommentTarget();
        // Cancel move comment - no backend restore needed (pickup doesn't mutate backend)
        useUIStore.getState().clearMoveCommentSource();
        return;
      }

      // Ignore if typing in an input (for all other keys)
      if (
        event.target instanceof HTMLInputElement ||
        event.target instanceof HTMLTextAreaElement ||
        event.target instanceof HTMLSelectElement ||
        (event.target instanceof HTMLElement && event.target.isContentEditable)
      ) {
        return;
      }

      const isCtrlOrCmd = event.ctrlKey || event.metaKey;

      // ===== RESERVED SHORTCUTS (hardcoded) =====
      // These cannot be overridden by config

      // Handle Delete/Backspace - delete selected items
      if (event.key === 'Delete' || event.key === 'Backspace') {
        if (placementModeActive || dataTableFullScreen) {
          event.preventDefault();
          return;
        }
        // GUARD: read-only - no deletes in the closure-adjusted view, in Plan/Export
        // (which can fall back to geometryMode 'measured' on a no-closures project, so
        // the appMode check is load-bearing), or on a locked remote SpeleoDB project.
        if (isEditingBlockedNow()) return;
        // GUARD: No delete for pan/zoom tools
        const currentToolDel = useUIStore.getState().activeTool;
        if (isNonInteractiveTool(currentToolDel)) return;

        if (selection) {
          if (selection.type === 'shot' && selection.sectionId !== undefined && selection.shotId !== undefined) {
            openDialog('confirm-delete', {
              type: 'shot',
              sectionId: selection.sectionId,
              shotId: selection.shotId,
            });
          } else if (selection.type === 'station' && selection.stationRef) {
            openDialog('confirm-delete', {
              type: 'station',
              stationRef: selection.stationRef,
            });
          } else if (selection.type === 'section' && selection.sectionId !== undefined) {
            openDialog('confirm-delete', {
              type: 'section',
              sectionId: selection.sectionId,
            });
          }
        }
        return;
      }

      // Handle Ctrl+A - Select all (reserved)
      if (isCtrlOrCmd && event.key.toLowerCase() === 'a') {
        // ALWAYS prevent browser select-all
        event.preventDefault();
        if (placementModeActive || dataTableFullScreen) {
          return;
        }
        // GUARD: No select all for pan/zoom tools
        const currentToolA = useUIStore.getState().activeTool;
        if (isNonInteractiveTool(currentToolA)) return;
        // GUARD: Block select all when stations are hidden
        if (!showStations) {
          showToast(t('map.toasts.stationsHidden'), TOAST_KEYS.STATIONS_HIDDEN, 'warning');
          return;
        }
        if (stations.length > 0) {
          const allStations: Selection[] = stations.map((station) => ({
            type: 'station' as const,
            stationRef: station.station_ref,
          }));
          selectAll(allStations);
        }
        return;
      }

      // Toggle keyboard shortcuts overlay
      if (event.key === '?') {
        event.preventDefault();
        if (placementModeActive || dataTableFullScreen) {
          return;
        }
        const { activeDialog: currentDialog } = useUIStore.getState();
        if (currentDialog === 'keyboard-shortcuts') {
          useUIStore.getState().closeDialog();
        } else if (!currentDialog) {
          openDialog('keyboard-shortcuts');
        }
        return;
      }

      // ===== CONFIGURABLE SHORTCUTS =====
      // Parse the key event into normalized form for lookup
      const normalizedKey = parseKeyEvent(event);
      if (!normalizedKey || normalizedKey === 'Ctrl' || normalizedKey === 'Alt' || normalizedKey === 'Shift') {
        // Modifier-only press, ignore
        return;
      }

      const shortcutMap = shortcutMapRef.current;
      const action = shortcutMap.get(normalizedKey);

      if (!action) {
        // No action mapped to this shortcut
        return;
      }

      if (placementModeActive && !PLACEMENT_MODE_ALLOWED_SHORTCUTS.has(action)) {
        event.preventDefault();
        return;
      }

      if (dataTableFullScreen && !FULLSCREEN_TABLE_ALLOWED_SHORTCUTS.has(action)) {
        event.preventDefault();
        // Exit is keyed to the toggle_data_table ACTION (not a literal key),
        // so rebinding the shortcut keeps it working. Everything else -
        // including import_dmp/export - is swallowed.
        if (action === 'toggle_data_table') {
          useUIStore.getState().exitDataTableFullScreen();
        }
        return;
      }

      // Get config values for zoom actions
      const currentConfig = useProjectStore.getState().config;
      const zoomStep = currentConfig?.map.zoom_step ?? 1.0;
      const zoomMin = currentConfig?.map.zoom_min ?? 0;
      const zoomMax = currentConfig?.map.zoom_max ?? 22;
      const shortcutFeedback = currentConfig?.general.shortcut_feedback ?? 'non_obvious';

      // Helper to show toast if appropriate for this action
      const maybeShowToast = (actionName: ShortcutAction, stateInfo?: string) => {
        if (shouldShowToast(actionName, shortcutFeedback)) {
          const label = ACTION_LABELS[actionName];
          const message = stateInfo ? `${label}: ${stateInfo}` : label;
          showToast(message, shortcutToastKey(actionName), 'info');
        }
      };

      // Handle action
      switch (action) {
        // Tool actions
        case 'select':
        case 'pan':
        case 'merge':
        case 'measure':
        case 'move_station':
        case 'move_segment':
        case 'move_shot':
        case 'comment_tool':
        case 'move_comment_tool': {
          // P does nothing while the navigate group is hidden (#409). The guard is
          // BEFORE preventDefault and the toast on purpose: a key bound to a tool
          // that does not exist has to behave exactly like an unbound key, not like
          // a key that was pressed and swallowed.
          if (action === 'pan' && !navigationToolsEnabled(currentConfig)) break;
          const tool = TOOL_ACTIONS[action];
          if (tool) {
            event.preventDefault();
            setActiveTool(tool);
            maybeShowToast(action);
          }
          break;
        }

        case 'zoom':
          // Same as 'pan' above: hidden tool, unbound key.
          if (!navigationToolsEnabled(currentConfig)) break;
          // Regular toggle to zoom tool
          event.preventDefault();
          setActiveTool('zoom');
          maybeShowToast(action);
          break;

        // Edit actions
        // GUARD: undo/redo are survey-only (#243) - parity with the toolbar buttons,
        // which are hidden in Plan/Export. Only appMode is checked (not geometryMode):
        // the buttons stay available in the closure-adjusted survey view, so the
        // shortcuts do too.
        case 'undo':
          event.preventDefault();
          if (useUIStore.getState().appMode !== 'survey') break;
          undo();
          maybeShowToast(action);
          break;

        case 'redo':
          event.preventDefault();
          if (useUIStore.getState().appMode !== 'survey') break;
          redo();
          maybeShowToast(action);
          break;

        // View toggle actions
        case 'toggle_depth_gradient':
        case 'toggle_grid':
        case 'toggle_satellite':
        case 'toggle_station_labels':
        case 'toggle_shot_labels':
        case 'toggle_section_labels': {
          const toggleKey = DISPLAY_TOGGLE_ACTIONS[action];
          if (toggleKey) {
            event.preventDefault();
            // Get current state before toggle to report new state
            const currentState = useUIStore.getState()[toggleKey];
            toggleDisplay(toggleKey);
            maybeShowToast(action, currentState ? 'Off' : 'On');
          }
          break;
        }

        case 'fit_to_bounds':
          event.preventDefault();
          triggerFitToBounds();
          maybeShowToast(action);
          break;

        case 'zoom_in':
          event.preventDefault();
          trackNav('zoom-in', 'keyboard');
          handleZoomIn(zoomStep, zoomMax);
          maybeShowToast(action);
          break;

        case 'zoom_out':
          event.preventDefault();
          trackNav('zoom-out', 'keyboard');
          handleZoomOut(zoomStep, zoomMin);
          maybeShowToast(action);
          break;

        // Panel actions
        case 'toggle_sections_panel': {
          event.preventDefault();
          const sectionsPanelOpen = useUIStore.getState().sectionsPanelOpen;
          toggleSectionsPanel();
          maybeShowToast(action, sectionsPanelOpen ? 'Hidden' : 'Visible');
          break;
        }

        case 'toggle_detail_panel': {
          event.preventDefault();
          const detailPanelOpen = useUIStore.getState().detailPanelOpen;
          toggleDetailPanel();
          maybeShowToast(action, detailPanelOpen ? 'Hidden' : 'Visible');
          break;
        }

        case 'toggle_data_table': {
          event.preventDefault();
          const dataTableOpen = useUIStore.getState().dataTableOpen;
          toggleDataTable();
          maybeShowToast(action, dataTableOpen ? 'Hidden' : 'Visible');
          break;
        }

        // File actions
        case 'new_file':
          event.preventDefault();
          // Routes through the shared helper so the unsaved-changes guard (a dirty
          // open project) is applied here too, not just on the menu "New" path.
          runFileShortcutAction('new_file');
          maybeShowToast(action);
          break;

        case 'quit':
          event.preventDefault();
          runFileShortcutAction('quit');
          break;

        case 'open_file':
          event.preventDefault();
          runFileShortcutAction('open_file');
          break;

        case 'save':
          event.preventDefault();
          if (project) {
            void saveProject().then((result) => {
              if (result && !result.success) {
                // A save blocked by integrity issues is not a "real" error -
                // route to Review & Repair instead of an error window.
                surfaceSaveFailure(result, openDialog, useProjectStore.getState());
              }
            });
            maybeShowToast(action);
          }
          break;

        case 'save_as':
          event.preventDefault();
          if (useProjectStore.getState().project) {
            // Shared with the menu "Save As" path: the Save-As + overwrite-confirm
            // flow (and its failure handling) lives in saveProjectAs so the two
            // can't drift.
            void saveProjectAs();
            maybeShowToast(action);
          }
          break;

        case 'import_dmp':
          event.preventDefault();
          // Shared routing (toolbar Import + drag-drop + GlobalView Ctrl+I): the
          // helper applies the read-only-map guard (closure-adjusted view or
          // Plan/Export, #243) and scopes the picker to DMP-only while a project
          // is open.
          runFileShortcutAction('import_dmp');
          break;

        case 'export': {
          event.preventDefault();
          const currentProject = useProjectStore.getState().project;
          if (currentProject) {
            openDialog('export');
          }
          break;
        }
      }
    },
    [
      clearSelection,
      clearMergeStations,
      clearMeasure,
      selection,
      openDialog,
      project,
      showStations,
      showToast,
      stations,
      selectAll,
      setActiveTool,
      undo,
      redo,
      saveProject,
      toggleDisplay,
      triggerFitToBounds,
      toggleSectionsPanel,
      toggleDetailPanel,
      toggleDataTable,
    ]
  );

  useEffect(() => {
    window.addEventListener('keydown', handleKeyDown);
    return () => {
      window.removeEventListener('keydown', handleKeyDown);
    };
  }, [handleKeyDown]);
}
