import { useReducer, useCallback, useMemo } from 'react';
import type { DmpFile, DmpShot } from '../types/commands';
import type { GpsCoordinates, StationRef, Coordinates, Line } from '../types/project';
import type { EditableShot } from '../components/common/ShotTableEditor';
import {
  type ImportError,
  validateEditableShot,
  validatePlacement,
  validateMergeTarget,
  hasAzimuthDiscrepancy,
  hasPitchDiscrepancy,
} from '../utils/validation';
import type { ValidationConfig } from '../types/config';
import type { Person } from '../types/ipc.generated';
import { lineColorPalette } from '../utils/colors';

/**
 * Configuration from Step 1 (DmpSelectionDialog).
 */
export interface DmpSelectionConfig {
  selectedIndices: Set<number>;
  defaultSurveyors?: Person[];
  defaultExplorers?: Person[];
  defaultDate: string | null;
}

/**
 * Placement configuration for a line.
 */
export type PlacementConfig =
  | { type: 'origin' }
  | { type: 'zeroDatum'; coordinates: GpsCoordinates; stationIndex: number }
  | { type: 'fromExisting'; importStation: number; existingStation: StationRef }
  | { type: 'fromImporting'; importStation: number; targetLineIndex: number; targetStationIndex: number }
  | { type: 'manual'; alignStationIndex: number; position?: Coordinates; snappedToStation?: StationRef | null };

/**
 * Configuration for a single imported line.
 */
export interface LineConfig {
  lineIndex: number;
  name: string;
  surveyors: Person[];
  explorers: Person[];
  date: string | null;
  color: string;
  direction: 'in' | 'out';
  quality: number;
  shots: EditableShot[];
  placement: PlacementConfig;
}

/**
 * Entry types for undo/redo operations.
 */
export type UndoEntryType =
  | 'shotEdit'
  | 'shotAdd'
  | 'shotDelete'
  | 'metadataEdit'
  | 'placementChange';

/**
 * Undo entry storing previous state for reversal.
 */
export interface UndoEntry {
  type: UndoEntryType;
  previousState: Partial<LineConfig>;
  description: string;
}

/**
 * Complete import state.
 */
export interface ImportState {
  /** Per-line configuration */
  lineConfigs: Map<number, LineConfig>;
  /** Current line index (within the Map keys) */
  currentLineIndex: number;
  /** Monotonic counter for unique temporary shot IDs */
  nextTempId: number;
  /** Placed lines (user has explicitly set placement) */
  placedLines: Set<number>;
  /** Excluded lines (will not be imported) */
  excludedLines: Set<number>;
  /** Undo stacks per line */
  undoStacks: Map<number, UndoEntry[]>;
  /** Redo stacks per line */
  redoStacks: Map<number, UndoEntry[]>;
  /** Original line values for reset functionality */
  originalValues: Map<number, LineConfig>;
}

/**
 * Action types for the import state reducer.
 */
type ImportAction =
  | { type: 'NAVIGATE'; lineIndex: number }
  | { type: 'UPDATE_FIELD'; field: keyof LineConfig; value: LineConfig[keyof LineConfig] }
  | { type: 'UPDATE_SHOT'; shotIndex: number; field: keyof EditableShot; value: EditableShot[keyof EditableShot] }
  | { type: 'ADD_SHOT' }
  | { type: 'DELETE_SHOT'; index: number }
  | { type: 'INSERT_SHOT_BEFORE'; index: number }
  | { type: 'INSERT_SHOT_AFTER'; index: number }
  | { type: 'SET_PLACEMENT'; placement: PlacementConfig }
  | { type: 'MARK_PLACED'; lineIndex: number }
  | { type: 'UNMARK_PLACED'; lineIndex: number }
  | { type: 'EXCLUDE_LINE'; lineIndex: number }
  | { type: 'INCLUDE_LINE'; lineIndex: number }
  | { type: 'UNDO' }
  | { type: 'REDO' }
  | { type: 'RESET_LINE' }
  | { type: 'RESTORE_STATE'; lineConfigs: Map<number, LineConfig>; placedLines: Set<number>; excludedLines: Set<number>; currentLineIndex: number };

const MAX_UNDO_HISTORY = 50;
const DEFAULT_EXCLUDED_DMP_SHOT_LENGTH = 1;

/**
 * DMP imports skip length-1 standard shots by default.
 *
 * This is also the MNemo station-numbering rule: only the shots that pass it
 * advance the station number (see `mnemoStationNumber`).
 */
export function isImportableDmpShot(shot: DmpShot): boolean {
  if (shot.shot_type !== 'Standard') {
    return false;
  }
  return Math.abs(shot.length - DEFAULT_EXCLUDED_DMP_SHOT_LENGTH) > 1e-9;
}

/**
 * The MNemo station number at the END of the importable shot at `index` (its
 * position among the shots `isImportableDmpShot` keeps). MNemo numbers a
 * line's start 0 and the end of its N-th shot N, so this is `index + 1`.
 * ASSUMED, not confirmed against a device: that only the shots the importer
 * keeps advance the device's counter. This is the one place the rule lives: if a
 * device turns out to count other shot kinds too (1 m shots, CsA/CsB), this and
 * the filter it is applied after are what change.
 */
export function mnemoStationNumber(index: number): number {
  return index + 1;
}

/**
 * Convert DMP shots to EditableShots.
 */
export function convertToEditableShots(shots: DmpShot[]): EditableShot[] {
  return shots
    .filter(isImportableDmpShot)
    .map((shot, index) => ({
      id: index,
      mnemoStation: mnemoStationNumber(index),
      azimuth_in: shot.heading_in,
      azimuth_out: shot.heading_out,
      length: shot.length,
      depth_in: shot.depth_in,
      depth_out: shot.depth_out,
      pitch_in: shot.pitch_in,
      pitch_out: shot.pitch_out,
      left: shot.left ?? null,
      right: shot.right ?? null,
      up: shot.up ?? null,
      down: shot.down ?? null,
      comment: shot.comment || null,
      kind: 'Normal' as const,
    }));
}

/**
 * Get the next available temp ID based on existing shots.
 */
function getNextTempIdFromConfigs(lineConfigs: Map<number, LineConfig>): number {
  let maxTempId = 0;
  for (const config of lineConfigs.values()) {
    for (const shot of config.shots) {
      const candidate = shot.tempId ?? (shot.id < 0 ? Math.abs(shot.id) : 0);
      if (candidate > maxTempId) {
        maxTempId = candidate;
      }
    }
  }
  return maxTempId + 1;
}

/**
 * Deep copy a shots array.
 */
function deepCopyShots(shots: EditableShot[]): EditableShot[] {
  return shots.map((s) => ({ ...s }));
}

/**
 * Create a new blank shot.
 */
function createBlankShot(tempId: number): EditableShot {
  return {
    id: -tempId,
    azimuth_in: 0,
    azimuth_out: 0,
    length: 0,
    depth_in: 0,
    depth_out: 0,
    pitch_in: null,
    pitch_out: null,
    left: null,
    right: null,
    up: null,
    down: null,
    comment: null,
    kind: 'Normal' as const,
    isNew: true,
    tempId,
  };
}

/**
 * Save current line state to undo stack.
 */
function pushToUndoStack(
  state: ImportState,
  entryType: UndoEntryType,
  description: string,
  previousPartialState: Partial<LineConfig>
): ImportState {
  const currentConfig = state.lineConfigs.get(state.currentLineIndex);
  if (!currentConfig) return state;

  const entry: UndoEntry = {
    type: entryType,
    previousState: previousPartialState,
    description,
  };

  const undoStack = state.undoStacks.get(state.currentLineIndex) || [];
  const newUndoStack = [...undoStack, entry];

  // Limit undo stack size
  if (newUndoStack.length > MAX_UNDO_HISTORY) {
    newUndoStack.shift();
  }

  // Clear redo stack when new action is taken
  const newUndoStacks = new Map(state.undoStacks);
  newUndoStacks.set(state.currentLineIndex, newUndoStack);

  const newRedoStacks = new Map(state.redoStacks);
  newRedoStacks.set(state.currentLineIndex, []);

  return {
    ...state,
    undoStacks: newUndoStacks,
    redoStacks: newRedoStacks,
  };
}

/**
 * Import state reducer.
 */
export function importReducer(state: ImportState, action: ImportAction): ImportState {
  const currentConfig = state.lineConfigs.get(state.currentLineIndex);

  switch (action.type) {
    case 'NAVIGATE': {
      return {
        ...state,
        currentLineIndex: action.lineIndex,
      };
    }

    case 'UPDATE_FIELD': {
      if (!currentConfig) return state;

      // Save to undo stack
      const previousPartial: Partial<LineConfig> = {
        [action.field]: currentConfig[action.field],
      };
      let stateWithUndo = pushToUndoStack(state, 'metadataEdit', `Edit ${String(action.field)}`, previousPartial);

      // Update the config
      const newConfigs = new Map(stateWithUndo.lineConfigs);
      newConfigs.set(state.currentLineIndex, {
        ...currentConfig,
        [action.field]: action.value,
      });

      return {
        ...stateWithUndo,
        lineConfigs: newConfigs,
      };
    }

    case 'UPDATE_SHOT': {
      if (!currentConfig) return state;

      const shot = currentConfig.shots[action.shotIndex];
      if (!shot) return state;

      // Save to undo stack (only store the changed shot's previous state)
      const previousPartial: Partial<LineConfig> = {
        shots: deepCopyShots(currentConfig.shots),
      };
      let stateWithUndo = pushToUndoStack(state, 'shotEdit', `Edit shot ${action.shotIndex} ${String(action.field)}`, previousPartial);

      // Update the shot
      const newShots = [...currentConfig.shots];
      newShots[action.shotIndex] = {
        ...shot,
        [action.field]: action.value,
      };

      const newConfigs = new Map(stateWithUndo.lineConfigs);
      newConfigs.set(state.currentLineIndex, {
        ...currentConfig,
        shots: newShots,
      });

      return {
        ...stateWithUndo,
        lineConfigs: newConfigs,
      };
    }

    case 'ADD_SHOT': {
      if (!currentConfig) return state;

      // Save to undo stack
      const previousPartial: Partial<LineConfig> = {
        shots: deepCopyShots(currentConfig.shots),
      };
      let stateWithUndo = pushToUndoStack(state, 'shotAdd', 'Add shot', previousPartial);

      // Add new shot
      const newShot = createBlankShot(state.nextTempId);
      const newShots = [...currentConfig.shots, newShot];

      const newConfigs = new Map(stateWithUndo.lineConfigs);
      newConfigs.set(state.currentLineIndex, {
        ...currentConfig,
        shots: newShots,
      });

      return {
        ...stateWithUndo,
        lineConfigs: newConfigs,
        nextTempId: state.nextTempId + 1,
      };
    }

    case 'DELETE_SHOT': {
      if (!currentConfig) return state;

      const shot = currentConfig.shots[action.index];
      if (!shot) return state;

      // Save to undo stack
      const previousPartial: Partial<LineConfig> = {
        shots: deepCopyShots(currentConfig.shots),
      };
      let stateWithUndo = pushToUndoStack(state, 'shotDelete', `Delete shot ${action.index}`, previousPartial);

      // Mark shot as deleted
      const newShots = [...currentConfig.shots];
      newShots[action.index] = { ...shot, isDeleted: true };

      const newConfigs = new Map(stateWithUndo.lineConfigs);
      newConfigs.set(state.currentLineIndex, {
        ...currentConfig,
        shots: newShots,
      });

      return {
        ...stateWithUndo,
        lineConfigs: newConfigs,
      };
    }

    case 'INSERT_SHOT_BEFORE': {
      if (!currentConfig) return state;

      // Save to undo stack
      const previousPartial: Partial<LineConfig> = {
        shots: deepCopyShots(currentConfig.shots),
      };
      let stateWithUndo = pushToUndoStack(state, 'shotAdd', `Insert shot before ${action.index}`, previousPartial);

      // Insert new shot
      const newShot = createBlankShot(state.nextTempId);
      const newShots = [...currentConfig.shots];
      newShots.splice(action.index, 0, newShot);

      const newConfigs = new Map(stateWithUndo.lineConfigs);
      newConfigs.set(state.currentLineIndex, {
        ...currentConfig,
        shots: newShots,
      });

      return {
        ...stateWithUndo,
        lineConfigs: newConfigs,
        nextTempId: state.nextTempId + 1,
      };
    }

    case 'INSERT_SHOT_AFTER': {
      if (!currentConfig) return state;

      // Save to undo stack
      const previousPartial: Partial<LineConfig> = {
        shots: deepCopyShots(currentConfig.shots),
      };
      let stateWithUndo = pushToUndoStack(state, 'shotAdd', `Insert shot after ${action.index}`, previousPartial);

      // Insert new shot
      const newShot = createBlankShot(state.nextTempId);
      const newShots = [...currentConfig.shots];
      newShots.splice(action.index + 1, 0, newShot);

      const newConfigs = new Map(stateWithUndo.lineConfigs);
      newConfigs.set(state.currentLineIndex, {
        ...currentConfig,
        shots: newShots,
      });

      return {
        ...stateWithUndo,
        lineConfigs: newConfigs,
        nextTempId: state.nextTempId + 1,
      };
    }

    case 'SET_PLACEMENT': {
      if (!currentConfig) return state;

      // Save to undo stack
      const previousPartial: Partial<LineConfig> = {
        placement: currentConfig.placement,
      };
      let stateWithUndo = pushToUndoStack(state, 'placementChange', 'Change placement', previousPartial);

      // Update placement (does NOT mark line as placed - that only happens via MARK_PLACED)
      const newConfigs = new Map(stateWithUndo.lineConfigs);
      newConfigs.set(state.currentLineIndex, {
        ...currentConfig,
        placement: action.placement,
      });

      return {
        ...stateWithUndo,
        lineConfigs: newConfigs,
      };
    }

    case 'MARK_PLACED': {
      const newPlaced = new Set(state.placedLines);
      newPlaced.add(action.lineIndex);
      return {
        ...state,
        placedLines: newPlaced,
      };
    }

    case 'UNMARK_PLACED': {
      const newPlaced = new Set(state.placedLines);
      newPlaced.delete(action.lineIndex);
      return {
        ...state,
        placedLines: newPlaced,
      };
    }

    case 'EXCLUDE_LINE': {
      const newExcluded = new Set(state.excludedLines);
      newExcluded.add(action.lineIndex);
      // Also remove from placed if it was placed
      const newPlaced = new Set(state.placedLines);
      newPlaced.delete(action.lineIndex);
      return {
        ...state,
        excludedLines: newExcluded,
        placedLines: newPlaced,
      };
    }

    case 'INCLUDE_LINE': {
      const newExcluded = new Set(state.excludedLines);
      newExcluded.delete(action.lineIndex);
      return {
        ...state,
        excludedLines: newExcluded,
      };
    }

    case 'UNDO': {
      const undoStack = state.undoStacks.get(state.currentLineIndex) || [];
      if (undoStack.length === 0 || !currentConfig) return state;

      // Pop from undo stack
      const entry = undoStack[undoStack.length - 1];
      if (!entry) return state;

      const newUndoStack = undoStack.slice(0, -1);

      // Push current state to redo stack
      const redoStack = state.redoStacks.get(state.currentLineIndex) || [];
      const redoEntry: UndoEntry = {
        type: entry.type,
        previousState: {},
        description: entry.description,
      };

      // Build redo state from current config
      for (const key of Object.keys(entry.previousState) as (keyof LineConfig)[]) {
        if (key === 'shots') {
          redoEntry.previousState.shots = deepCopyShots(currentConfig.shots);
        } else {
          // Copy the current value for this field
          (redoEntry.previousState as Record<string, unknown>)[key] = currentConfig[key];
        }
      }

      const newRedoStack = [...redoStack, redoEntry];

      // Restore previous state
      const restoredConfig = { ...currentConfig };
      for (const [key, value] of Object.entries(entry.previousState)) {
        if (key === 'shots') {
          restoredConfig.shots = deepCopyShots(value as EditableShot[]);
        } else {
          (restoredConfig as Record<string, unknown>)[key] = value;
        }
      }

      const newConfigs = new Map(state.lineConfigs);
      newConfigs.set(state.currentLineIndex, restoredConfig);

      const newUndoStacks = new Map(state.undoStacks);
      newUndoStacks.set(state.currentLineIndex, newUndoStack);

      const newRedoStacks = new Map(state.redoStacks);
      newRedoStacks.set(state.currentLineIndex, newRedoStack);

      return {
        ...state,
        lineConfigs: newConfigs,
        undoStacks: newUndoStacks,
        redoStacks: newRedoStacks,
      };
    }

    case 'REDO': {
      const redoStack = state.redoStacks.get(state.currentLineIndex) || [];
      if (redoStack.length === 0 || !currentConfig) return state;

      // Pop from redo stack
      const entry = redoStack[redoStack.length - 1];
      if (!entry) return state;

      const newRedoStack = redoStack.slice(0, -1);

      // Push current state to undo stack
      const undoStack = state.undoStacks.get(state.currentLineIndex) || [];
      const undoEntry: UndoEntry = {
        type: entry.type,
        previousState: {},
        description: entry.description,
      };

      // Build undo state from current config
      for (const key of Object.keys(entry.previousState) as (keyof LineConfig)[]) {
        if (key === 'shots') {
          undoEntry.previousState.shots = deepCopyShots(currentConfig.shots);
        } else {
          (undoEntry.previousState as Record<string, unknown>)[key] = currentConfig[key];
        }
      }

      const newUndoStack = [...undoStack, undoEntry];

      // Restore redo state
      const restoredConfig = { ...currentConfig };
      for (const [key, value] of Object.entries(entry.previousState)) {
        if (key === 'shots') {
          restoredConfig.shots = deepCopyShots(value as EditableShot[]);
        } else {
          (restoredConfig as Record<string, unknown>)[key] = value;
        }
      }

      const newConfigs = new Map(state.lineConfigs);
      newConfigs.set(state.currentLineIndex, restoredConfig);

      const newUndoStacks = new Map(state.undoStacks);
      newUndoStacks.set(state.currentLineIndex, newUndoStack);

      const newRedoStacks = new Map(state.redoStacks);
      newRedoStacks.set(state.currentLineIndex, newRedoStack);

      return {
        ...state,
        lineConfigs: newConfigs,
        undoStacks: newUndoStacks,
        redoStacks: newRedoStacks,
      };
    }

    case 'RESET_LINE': {
      const original = state.originalValues.get(state.currentLineIndex);
      if (!original) return state;

      // Reset to original values (deep copy)
      const resetConfig: LineConfig = {
        ...original,
        shots: deepCopyShots(original.shots),
        placement: { type: 'origin' },
      };

      const newConfigs = new Map(state.lineConfigs);
      newConfigs.set(state.currentLineIndex, resetConfig);

      // Remove from placed lines
      const newPlaced = new Set(state.placedLines);
      newPlaced.delete(state.currentLineIndex);

      // Clear undo/redo stacks for this line
      const newUndoStacks = new Map(state.undoStacks);
      newUndoStacks.set(state.currentLineIndex, []);

      const newRedoStacks = new Map(state.redoStacks);
      newRedoStacks.set(state.currentLineIndex, []);

      return {
        ...state,
        lineConfigs: newConfigs,
        placedLines: newPlaced,
        undoStacks: newUndoStacks,
        redoStacks: newRedoStacks,
      };
    }

    case 'RESTORE_STATE': {
      const nextTempId = Math.max(
        state.nextTempId,
        getNextTempIdFromConfigs(action.lineConfigs)
      );
      return {
        ...state,
        lineConfigs: action.lineConfigs,
        placedLines: action.placedLines,
        excludedLines: action.excludedLines,
        currentLineIndex: action.currentLineIndex,
        nextTempId,
      };
    }

    default:
      return state;
  }
}

/**
 * Initialize import state from DMP file and selection config.
 */
function initializeState(
  dmpFile: DmpFile,
  selectionConfig: DmpSelectionConfig,
  savedState?: {
    lineConfigs: Array<[number, LineConfig]>;
    placedLines: number[];
    excludedLines?: number[];
    currentLineIndex: number;
  },
  resolvedTheme = 'dark'
): ImportState {
  const selectedIndices = Array.from(selectionConfig.selectedIndices).sort((a, b) => a - b);
  const firstLineIndex = selectedIndices[0];
  const palette = lineColorPalette(resolvedTheme);

  // Build original values (these never change)
  const originalValues = new Map<number, LineConfig>();
  selectedIndices.forEach((lineIdx, arrayIdx) => {
    const line = dmpFile.lines[lineIdx];
    if (!line) return;

    const dateStr = line.date?.split(/[T ]/)[0] ?? null;

    originalValues.set(lineIdx, {
      lineIndex: lineIdx,
      name: line.name,
      // DMP lines name nobody - the people come from the Step 1 defaults
      surveyors: [],
      explorers: [],
      date: dateStr,
      color: palette[arrayIdx % palette.length] ?? '#4ecdc4',
      direction: line.direction,
      quality: 1.0,
      shots: convertToEditableShots(line.shots),
      placement: { type: 'origin' },
    });
  });

  // If restoring from saved state
  if (savedState) {
    const lineConfigs = new Map(
      savedState.lineConfigs.map(([idx, cfg]) => [
        idx,
        { ...cfg, quality: cfg.quality ?? 1.0 },
      ])
    );
    return {
      lineConfigs,
      currentLineIndex: savedState.currentLineIndex,
      nextTempId: getNextTempIdFromConfigs(lineConfigs),
      placedLines: new Set(savedState.placedLines),
      excludedLines: new Set(savedState.excludedLines ?? []),
      undoStacks: new Map(),
      redoStacks: new Map(),
      originalValues,
    };
  }

  // Build initial line configs
  const lineConfigs = new Map<number, LineConfig>();
  selectedIndices.forEach((lineIdx, arrayIdx) => {
    const line = dmpFile.lines[lineIdx];
    if (!line) return;

    const direction = line.direction;
    // The people from step 1 (never anything derived from the line name)
    const surveyors = selectionConfig.defaultSurveyors ?? [];
    const explorers = selectionConfig.defaultExplorers ?? [];
    const dateStr = selectionConfig.defaultDate ?? line.date?.split(/[T ]/)[0] ?? null;

    lineConfigs.set(lineIdx, {
      lineIndex: lineIdx,
      name: line.name,
      surveyors,
      explorers,
      date: dateStr,
      color: palette[arrayIdx % palette.length] ?? '#4ecdc4',
      direction,
      quality: 1.0,
      shots: convertToEditableShots(line.shots),
      placement: { type: 'origin' },
    });
  });

  // No auto-placement - all lines start as unplaced
  const placedLines = new Set<number>();
  const excludedLines = new Set<number>();

  const nextTempId = getNextTempIdFromConfigs(lineConfigs);

  return {
    lineConfigs,
    currentLineIndex: firstLineIndex ?? 0,
    nextTempId,
    placedLines,
    excludedLines,
    undoStacks: new Map(),
    redoStacks: new Map(),
    originalValues,
  };
}

/**
 * Hook for managing import state with undo/redo functionality.
 * Provides per-line state tracking, undo/redo stacks, and reset functionality.
 */
export function useImportState(
  dmpFile: DmpFile,
  selectionConfig: DmpSelectionConfig,
  savedState?: {
    lineConfigs: Array<[number, LineConfig]>;
    placedLines: number[];
    excludedLines?: number[];
    currentLineIndex: number;
  },
  resolvedTheme = 'dark'
) {
  const [state, dispatch] = useReducer(
    importReducer,
    { dmpFile, selectionConfig, savedState, resolvedTheme },
    ({ dmpFile, selectionConfig, savedState, resolvedTheme }) =>
      initializeState(dmpFile, selectionConfig, savedState, resolvedTheme)
  );

  // Get selected indices in order
  const selectedIndices = useMemo(() => {
    return Array.from(selectionConfig.selectedIndices).sort((a, b) => a - b);
  }, [selectionConfig.selectedIndices]);

  // Current array index (position in selectedIndices)
  const currentArrayIndex = useMemo(() => {
    return selectedIndices.indexOf(state.currentLineIndex);
  }, [selectedIndices, state.currentLineIndex]);

  // Current config
  const currentConfig = state.lineConfigs.get(state.currentLineIndex);

  // Can undo/redo for current line
  const canUndo = useMemo(() => {
    const stack = state.undoStacks.get(state.currentLineIndex);
    return (stack?.length ?? 0) > 0;
  }, [state.undoStacks, state.currentLineIndex]);

  const canRedo = useMemo(() => {
    const stack = state.redoStacks.get(state.currentLineIndex);
    return (stack?.length ?? 0) > 0;
  }, [state.redoStacks, state.currentLineIndex]);

  // Actions
  const navigateToLine = useCallback((lineIndex: number) => {
    dispatch({ type: 'NAVIGATE', lineIndex });
  }, []);

  const navigateToArrayIndex = useCallback((arrayIndex: number) => {
    const lineIndex = selectedIndices[arrayIndex];
    if (lineIndex !== undefined) {
      dispatch({ type: 'NAVIGATE', lineIndex });
    }
  }, [selectedIndices]);

  const updateLineField = useCallback(<K extends keyof LineConfig>(
    field: K,
    value: LineConfig[K]
  ) => {
    dispatch({ type: 'UPDATE_FIELD', field, value });
  }, []);

  const updateShot = useCallback((
    shotIndex: number,
    field: keyof EditableShot,
    value: EditableShot[keyof EditableShot]
  ) => {
    dispatch({ type: 'UPDATE_SHOT', shotIndex, field, value });
  }, []);

  const addShot = useCallback(() => {
    dispatch({ type: 'ADD_SHOT' });
  }, []);

  const deleteShot = useCallback((index: number) => {
    dispatch({ type: 'DELETE_SHOT', index });
  }, []);

  const insertShotBefore = useCallback((index: number) => {
    dispatch({ type: 'INSERT_SHOT_BEFORE', index });
  }, []);

  const insertShotAfter = useCallback((index: number) => {
    dispatch({ type: 'INSERT_SHOT_AFTER', index });
  }, []);

  const setPlacement = useCallback((placement: PlacementConfig) => {
    dispatch({ type: 'SET_PLACEMENT', placement });
  }, []);

  const markPlaced = useCallback((lineIndex: number) => {
    dispatch({ type: 'MARK_PLACED', lineIndex });
  }, []);

  const unmarkPlaced = useCallback((lineIndex: number) => {
    dispatch({ type: 'UNMARK_PLACED', lineIndex });
  }, []);

  const excludeLine = useCallback((lineIndex: number) => {
    dispatch({ type: 'EXCLUDE_LINE', lineIndex });
  }, []);

  const includeLine = useCallback((lineIndex: number) => {
    dispatch({ type: 'INCLUDE_LINE', lineIndex });
  }, []);

  const undo = useCallback(() => {
    dispatch({ type: 'UNDO' });
  }, []);

  const redo = useCallback(() => {
    dispatch({ type: 'REDO' });
  }, []);

  const resetLine = useCallback(() => {
    dispatch({ type: 'RESET_LINE' });
  }, []);

  const restoreState = useCallback((
    lineConfigs: Map<number, LineConfig>,
    placedLines: Set<number>,
    excludedLines: Set<number>,
    currentLineIndex: number
  ) => {
    dispatch({ type: 'RESTORE_STATE', lineConfigs, placedLines, excludedLines, currentLineIndex });
  }, []);

  // Serialization helpers for state persistence
  const serializeLineConfigs = useCallback(() => {
    return Array.from(state.lineConfigs.entries());
  }, [state.lineConfigs]);

  const serializePlacedLines = useCallback(() => {
    return Array.from(state.placedLines);
  }, [state.placedLines]);

  const serializeExcludedLines = useCallback(() => {
    return Array.from(state.excludedLines);
  }, [state.excludedLines]);

  /**
   * Get the next unplaced line index (not placed and not excluded).
   * Returns undefined if all lines are placed or excluded.
   *
   * @param afterIndex - Line index to start searching after
   * @param excludeLine - Optional line index to exclude from search (used when
   *                         the line is being marked as placed but state hasn't updated yet)
   */
  const getNextUnplacedLine = useCallback((afterIndex?: number, excludeLine?: number): number | undefined => {
    const startIdx = afterIndex !== undefined
      ? selectedIndices.indexOf(afterIndex) + 1
      : 0;

    // First check from startIdx to end
    for (let i = startIdx; i < selectedIndices.length; i++) {
      const lineIndex = selectedIndices[i];
      if (lineIndex === undefined) continue;
      // Skip the line being placed (state not yet updated)
      if (lineIndex === excludeLine) continue;
      if (!state.placedLines.has(lineIndex) && !state.excludedLines.has(lineIndex)) {
        return lineIndex;
      }
    }

    // Then check from beginning to startIdx (wrap around)
    for (let i = 0; i < startIdx; i++) {
      const lineIndex = selectedIndices[i];
      if (lineIndex === undefined) continue;
      // Skip the line being placed (state not yet updated)
      if (lineIndex === excludeLine) continue;
      if (!state.placedLines.has(lineIndex) && !state.excludedLines.has(lineIndex)) {
        return lineIndex;
      }
    }

    return undefined;
  }, [selectedIndices, state.placedLines, state.excludedLines]);

  /**
   * Check if all lines are either placed or excluded.
   */
  const allLinesHandled = useMemo(() => {
    for (const lineIndex of selectedIndices) {
      if (!state.placedLines.has(lineIndex) && !state.excludedLines.has(lineIndex)) {
        return false;
      }
    }
    return true;
  }, [selectedIndices, state.placedLines, state.excludedLines]);

  /**
   * Check if all lines are excluded.
   */
  const allLinesExcluded = useMemo(() => {
    for (const lineIndex of selectedIndices) {
      if (!state.excludedLines.has(lineIndex)) {
        return false;
      }
    }
    return true;
  }, [selectedIndices, state.excludedLines]);

  // Get line status - now considers validation errors and excluded status
  const getLineStatus = useCallback((lineIndex: number): 'placed' | 'not_placed' | 'errors' | 'excluded' => {
    // Check excluded first
    if (state.excludedLines.has(lineIndex)) return 'excluded';

    const config = state.lineConfigs.get(lineIndex);
    if (!config) return 'not_placed';

    const validShots = config.shots.filter((s) => !s.isDeleted);
    if (validShots.length === 0) return 'errors';

    // Check for shot validation errors
    for (let i = 0; i < config.shots.length; i++) {
      const shot = config.shots[i];
      if (!shot || shot.isDeleted) continue;
      const shotErrors = validateEditableShot(shot, i, lineIndex);
      if (shotErrors.length > 0) return 'errors';
    }

    if (state.placedLines.has(lineIndex)) return 'placed';

    return 'not_placed';
  }, [state.lineConfigs, state.placedLines, state.excludedLines]);

  // Get all configs for import (excludes excluded lines)
  const getAllConfigs = useCallback(() => {
    return Array.from(state.lineConfigs.values())
      .filter((config) => !state.excludedLines.has(config.lineIndex))
      .map((config) => ({
        ...config,
        shots: config.shots.filter((s) => !s.isDeleted),
      }));
  }, [state.lineConfigs, state.excludedLines]);

  /**
   * Get configs for lines that have been placed (for use as targets in "From existing station").
   * Returns placed lines with their names and station counts.
   */
  const getPlacedLineConfigs = useCallback(() => {
    const placedConfigs: Array<{ lineIndex: number; name: string; stationCount: number }> = [];
    for (const lineIndex of state.placedLines) {
      const config = state.lineConfigs.get(lineIndex);
      if (config) {
        const validShots = config.shots.filter((s) => !s.isDeleted);
        placedConfigs.push({
          lineIndex,
          name: config.name,
          stationCount: validShots.length + 1, // shots + 1 = stations
        });
      }
    }
    return placedConfigs;
  }, [state.placedLines, state.lineConfigs]);

  /**
   * Get all errors for a specific line.
   * Includes shot validation errors, placement errors, and merge conflicts.
   */
  const getLineErrors = useCallback((
    lineIndex: number,
    existingLines: Line[]
  ): ImportError[] => {
    const config = state.lineConfigs.get(lineIndex);
    if (!config) return [];

    const errors: ImportError[] = [];

    // Validate each shot
    config.shots.forEach((shot, shotIndex) => {
      if (!shot.isDeleted) {
        const shotErrors = validateEditableShot(shot, shotIndex, lineIndex);
        errors.push(...shotErrors);
      }
    });

    // Check for empty line (no valid shots)
    const validShots = config.shots.filter((s) => !s.isDeleted);
    if (validShots.length === 0) {
      errors.push({
        type: 'invalidData',
        lineIndex,
        message: 'Line has no valid shots',
      });
    }

    // Validate placement
    const isPlaced = state.placedLines.has(lineIndex);
    const placementError = validatePlacement(isPlaced, lineIndex);
    if (placementError) {
      errors.push(placementError);
    }

    // Validate merge target if using fromExisting placement
    if (config.placement.type === 'fromExisting') {
      const existingLineIds = new Set(existingLines.map((s) => s.id));
      // Get target line id from placement config
      const targetLineId = config.placement.existingStation.line_id;

      // Get set of line indices that have already been placed in this import
      // (these could be valid merge targets)
      const placedLineIndices = state.placedLines;

      const mergeError = validateMergeTarget(
        config.placement.type,
        targetLineId,
        existingLineIds,
        placedLineIndices,
        lineIndex
      );
      if (mergeError) {
        errors.push(mergeError);
      }
    }

    return errors;
  }, [state.lineConfigs, state.placedLines]);

  /**
   * Get all errors for all lines.
   * Returns a map of line index to array of errors.
   */
  const getAllLineErrors = useCallback((
    existingLines: Line[]
  ): Map<number, ImportError[]> => {
    const allErrors = new Map<number, ImportError[]>();

    for (const lineIndex of selectedIndices) {
      const errors = getLineErrors(lineIndex, existingLines);
      if (errors.length > 0) {
        allErrors.set(lineIndex, errors);
      }
    }

    return allErrors;
  }, [selectedIndices, getLineErrors]);

  /**
   * Get the error count for a specific line.
   */
  const getLineErrorCount = useCallback((
    lineIndex: number,
    existingLines: Line[]
  ): number => {
    return getLineErrors(lineIndex, existingLines).length;
  }, [getLineErrors]);

  /**
   * Get the data error count for a specific line (excluding placement errors).
   * This is used to determine if the Place button should be enabled.
   */
  const getLineDataErrorCount = useCallback((
    lineIndex: number,
    existingLines: Line[]
  ): number => {
    const errors = getLineErrors(lineIndex, existingLines);
    // Filter out missingPlacement errors - those are expected before placing
    return errors.filter((e) => e.type !== 'missingPlacement').length;
  }, [getLineErrors]);

  /**
   * Get the warning count for a specific line (azimuth/pitch discrepancies).
   * Counts discrepancy warnings across non-deleted shots; a single shot can
   * contribute up to 2 warnings (one azimuth, one pitch).
   * A threshold of 0 means that warning type is disabled.
   */
  const getLineWarningCount = useCallback((
    lineIndex: number,
    valConfig: Pick<ValidationConfig, 'azimuth_diff_threshold' | 'pitch_diff_threshold'>
  ): number => {
    const lineConfig = state.lineConfigs.get(lineIndex);
    if (!lineConfig) return 0;
    let warnings = 0;
    for (const shot of lineConfig.shots) {
      if (shot.isDeleted) continue;
      if (valConfig.azimuth_diff_threshold > 0 && hasAzimuthDiscrepancy(shot.azimuth_in, shot.azimuth_out, valConfig.azimuth_diff_threshold)) warnings++;
      if (valConfig.pitch_diff_threshold > 0 && hasPitchDiscrepancy(shot.pitch_in, shot.pitch_out, valConfig.pitch_diff_threshold)) warnings++;
    }
    return warnings;
  }, [state.lineConfigs]);

  /**
   * Get total error count across all lines.
   */
  const getTotalErrorCount = useCallback((
    existingLines: Line[]
  ): number => {
    let total = 0;
    for (const lineIndex of selectedIndices) {
      total += getLineErrors(lineIndex, existingLines).length;
    }
    return total;
  }, [selectedIndices, getLineErrors]);

  /**
   * Get total data error count across all lines (excluding placement errors).
   * Used for the error summary display.
   */
  const getTotalDataErrorCount = useCallback((
    existingLines: Line[]
  ): number => {
    let total = 0;
    for (const lineIndex of selectedIndices) {
      total += getLineDataErrorCount(lineIndex, existingLines);
    }
    return total;
  }, [selectedIndices, getLineDataErrorCount]);

  /**
   * Check if all lines can be imported (no errors and all placed or excluded).
   */
  const canImport = useCallback((
    existingLines: Line[]
  ): boolean => {
    // If all lines are excluded, we can't import (nothing to import)
    if (state.excludedLines.size === selectedIndices.length) {
      return false;
    }

    for (const lineIndex of selectedIndices) {
      // Skip excluded lines - they don't need to be validated
      if (state.excludedLines.has(lineIndex)) continue;

      const config = state.lineConfigs.get(lineIndex);
      if (!config) return false;

      // Check for valid shots
      const validShots = config.shots.filter((s) => !s.isDeleted);
      if (validShots.length === 0) return false;

      // Check for shot errors
      for (let i = 0; i < config.shots.length; i++) {
        const shot = config.shots[i];
        if (!shot || shot.isDeleted) continue;
        const shotErrors = validateEditableShot(shot, i, lineIndex);
        if (shotErrors.length > 0) return false;
      }

      // Check if placed
      if (!state.placedLines.has(lineIndex)) return false;

      // Check merge target validity
      if (config.placement.type === 'fromExisting') {
        const existingLineIds = new Set(existingLines.map((s) => s.id));
        const targetLineId = config.placement.existingStation.line_id;
        const mergeError = validateMergeTarget(
          config.placement.type,
          targetLineId,
          existingLineIds,
          state.placedLines,
          lineIndex
        );
        if (mergeError) return false;
      }
    }
    return true;
  }, [selectedIndices, state.lineConfigs, state.placedLines, state.excludedLines]);

  return {
    // State
    state,
    currentConfig,
    currentLineIndex: state.currentLineIndex,
    currentArrayIndex,
    selectedIndices,
    lineConfigs: state.lineConfigs,
    placedLines: state.placedLines,
    excludedLines: state.excludedLines,
    canUndo,
    canRedo,
    allLinesHandled,
    allLinesExcluded,

    // Navigation
    navigateToLine,
    navigateToArrayIndex,
    getNextUnplacedLine,

    // Line field updates
    updateLineField,

    // Shot operations
    updateShot,
    addShot,
    deleteShot,
    insertShotBefore,
    insertShotAfter,

    // Placement
    setPlacement,
    markPlaced,
    unmarkPlaced,

    // Exclude/Include
    excludeLine,
    includeLine,

    // Undo/Redo
    undo,
    redo,

    // Reset
    resetLine,

    // State restoration
    restoreState,

    // Serialization
    serializeLineConfigs,
    serializePlacedLines,
    serializeExcludedLines,

    // Status
    getLineStatus,

    // Export
    getAllConfigs,
    getPlacedLineConfigs,

    // Error validation
    getLineErrors,
    getAllLineErrors,
    getLineErrorCount,
    getLineDataErrorCount,
    getLineWarningCount,
    getTotalErrorCount,
    getTotalDataErrorCount,
    canImport,
  };
}

export type UseImportStateReturn = ReturnType<typeof useImportState>;
