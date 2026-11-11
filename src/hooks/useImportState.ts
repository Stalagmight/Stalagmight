import { useReducer, useCallback, useMemo } from 'react';
import type { DmpFile, DmpShot } from '../types/commands';
import type { GpsCoordinates, StationRef, Coordinates, Section } from '../types/project';
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
import { sectionColorPalette } from '../utils/colors';

/**
 * Configuration from Step 1 (DmpSelectionDialog).
 */
export interface DmpSelectionConfig {
  selectedIndices: Set<number>;
  defaultSurveyor: string | null;
  defaultDate: string | null;
}

/**
 * Placement configuration for a section.
 */
export type PlacementConfig =
  | { type: 'origin' }
  | { type: 'zeroDatum'; coordinates: GpsCoordinates; stationIndex: number }
  | { type: 'fromExisting'; importStation: number; existingStation: StationRef }
  | { type: 'fromImporting'; importStation: number; targetSectionIndex: number; targetStationIndex: number }
  | { type: 'manual'; alignStationIndex: number; position?: Coordinates; snappedToStation?: StationRef | null };

/**
 * Configuration for a single imported section.
 */
export interface SectionConfig {
  sectionIndex: number;
  name: string;
  surveyor: string | null;
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
  previousState: Partial<SectionConfig>;
  description: string;
}

/**
 * Complete import state.
 */
export interface ImportState {
  /** Per-section configuration */
  sectionConfigs: Map<number, SectionConfig>;
  /** Current section index (within the Map keys) */
  currentSectionIndex: number;
  /** Monotonic counter for unique temporary shot IDs */
  nextTempId: number;
  /** Placed sections (user has explicitly set placement) */
  placedSections: Set<number>;
  /** Excluded sections (will not be imported) */
  excludedSections: Set<number>;
  /** Undo stacks per section */
  undoStacks: Map<number, UndoEntry[]>;
  /** Redo stacks per section */
  redoStacks: Map<number, UndoEntry[]>;
  /** Original section values for reset functionality */
  originalValues: Map<number, SectionConfig>;
}

/**
 * Action types for the import state reducer.
 */
type ImportAction =
  | { type: 'NAVIGATE'; sectionIndex: number }
  | { type: 'UPDATE_FIELD'; field: keyof SectionConfig; value: SectionConfig[keyof SectionConfig] }
  | { type: 'UPDATE_SHOT'; shotIndex: number; field: keyof EditableShot; value: EditableShot[keyof EditableShot] }
  | { type: 'ADD_SHOT' }
  | { type: 'DELETE_SHOT'; index: number }
  | { type: 'INSERT_SHOT_BEFORE'; index: number }
  | { type: 'INSERT_SHOT_AFTER'; index: number }
  | { type: 'SET_PLACEMENT'; placement: PlacementConfig }
  | { type: 'MARK_PLACED'; sectionIndex: number }
  | { type: 'UNMARK_PLACED'; sectionIndex: number }
  | { type: 'EXCLUDE_SECTION'; sectionIndex: number }
  | { type: 'INCLUDE_SECTION'; sectionIndex: number }
  | { type: 'UNDO' }
  | { type: 'REDO' }
  | { type: 'RESET_SECTION' }
  | { type: 'RESTORE_STATE'; sectionConfigs: Map<number, SectionConfig>; placedSections: Set<number>; excludedSections: Set<number>; currentSectionIndex: number };

const MAX_UNDO_HISTORY = 50;
const DEFAULT_EXCLUDED_DMP_SHOT_LENGTH = 1;

/**
 * DMP imports skip length-1 standard shots by default.
 *
 * This is also the mNemo station-numbering rule: only the shots that pass it
 * advance the station number (see `mnemoStationNumber`).
 */
export function isImportableDmpShot(shot: DmpShot): boolean {
  if (shot.shot_type !== 'Standard') {
    return false;
  }
  return Math.abs(shot.length - DEFAULT_EXCLUDED_DMP_SHOT_LENGTH) > 1e-9;
}

/**
 * The mNemo station number at the END of the importable shot at `index` (its
 * position among the shots `isImportableDmpShot` keeps). mNemo numbers a
 * section's start 0 and the end of its N-th shot N, so this is `index + 1`.
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
function getNextTempIdFromConfigs(sectionConfigs: Map<number, SectionConfig>): number {
  let maxTempId = 0;
  for (const config of sectionConfigs.values()) {
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
 * Save current section state to undo stack.
 */
function pushToUndoStack(
  state: ImportState,
  entryType: UndoEntryType,
  description: string,
  previousPartialState: Partial<SectionConfig>
): ImportState {
  const currentConfig = state.sectionConfigs.get(state.currentSectionIndex);
  if (!currentConfig) return state;

  const entry: UndoEntry = {
    type: entryType,
    previousState: previousPartialState,
    description,
  };

  const undoStack = state.undoStacks.get(state.currentSectionIndex) || [];
  const newUndoStack = [...undoStack, entry];

  // Limit undo stack size
  if (newUndoStack.length > MAX_UNDO_HISTORY) {
    newUndoStack.shift();
  }

  // Clear redo stack when new action is taken
  const newUndoStacks = new Map(state.undoStacks);
  newUndoStacks.set(state.currentSectionIndex, newUndoStack);

  const newRedoStacks = new Map(state.redoStacks);
  newRedoStacks.set(state.currentSectionIndex, []);

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
  const currentConfig = state.sectionConfigs.get(state.currentSectionIndex);

  switch (action.type) {
    case 'NAVIGATE': {
      return {
        ...state,
        currentSectionIndex: action.sectionIndex,
      };
    }

    case 'UPDATE_FIELD': {
      if (!currentConfig) return state;

      // Save to undo stack
      const previousPartial: Partial<SectionConfig> = {
        [action.field]: currentConfig[action.field],
      };
      let stateWithUndo = pushToUndoStack(state, 'metadataEdit', `Edit ${String(action.field)}`, previousPartial);

      // Update the config
      const newConfigs = new Map(stateWithUndo.sectionConfigs);
      newConfigs.set(state.currentSectionIndex, {
        ...currentConfig,
        [action.field]: action.value,
      });

      return {
        ...stateWithUndo,
        sectionConfigs: newConfigs,
      };
    }

    case 'UPDATE_SHOT': {
      if (!currentConfig) return state;

      const shot = currentConfig.shots[action.shotIndex];
      if (!shot) return state;

      // Save to undo stack (only store the changed shot's previous state)
      const previousPartial: Partial<SectionConfig> = {
        shots: deepCopyShots(currentConfig.shots),
      };
      let stateWithUndo = pushToUndoStack(state, 'shotEdit', `Edit shot ${action.shotIndex} ${String(action.field)}`, previousPartial);

      // Update the shot
      const newShots = [...currentConfig.shots];
      newShots[action.shotIndex] = {
        ...shot,
        [action.field]: action.value,
      };

      const newConfigs = new Map(stateWithUndo.sectionConfigs);
      newConfigs.set(state.currentSectionIndex, {
        ...currentConfig,
        shots: newShots,
      });

      return {
        ...stateWithUndo,
        sectionConfigs: newConfigs,
      };
    }

    case 'ADD_SHOT': {
      if (!currentConfig) return state;

      // Save to undo stack
      const previousPartial: Partial<SectionConfig> = {
        shots: deepCopyShots(currentConfig.shots),
      };
      let stateWithUndo = pushToUndoStack(state, 'shotAdd', 'Add shot', previousPartial);

      // Add new shot
      const newShot = createBlankShot(state.nextTempId);
      const newShots = [...currentConfig.shots, newShot];

      const newConfigs = new Map(stateWithUndo.sectionConfigs);
      newConfigs.set(state.currentSectionIndex, {
        ...currentConfig,
        shots: newShots,
      });

      return {
        ...stateWithUndo,
        sectionConfigs: newConfigs,
        nextTempId: state.nextTempId + 1,
      };
    }

    case 'DELETE_SHOT': {
      if (!currentConfig) return state;

      const shot = currentConfig.shots[action.index];
      if (!shot) return state;

      // Save to undo stack
      const previousPartial: Partial<SectionConfig> = {
        shots: deepCopyShots(currentConfig.shots),
      };
      let stateWithUndo = pushToUndoStack(state, 'shotDelete', `Delete shot ${action.index}`, previousPartial);

      // Mark shot as deleted
      const newShots = [...currentConfig.shots];
      newShots[action.index] = { ...shot, isDeleted: true };

      const newConfigs = new Map(stateWithUndo.sectionConfigs);
      newConfigs.set(state.currentSectionIndex, {
        ...currentConfig,
        shots: newShots,
      });

      return {
        ...stateWithUndo,
        sectionConfigs: newConfigs,
      };
    }

    case 'INSERT_SHOT_BEFORE': {
      if (!currentConfig) return state;

      // Save to undo stack
      const previousPartial: Partial<SectionConfig> = {
        shots: deepCopyShots(currentConfig.shots),
      };
      let stateWithUndo = pushToUndoStack(state, 'shotAdd', `Insert shot before ${action.index}`, previousPartial);

      // Insert new shot
      const newShot = createBlankShot(state.nextTempId);
      const newShots = [...currentConfig.shots];
      newShots.splice(action.index, 0, newShot);

      const newConfigs = new Map(stateWithUndo.sectionConfigs);
      newConfigs.set(state.currentSectionIndex, {
        ...currentConfig,
        shots: newShots,
      });

      return {
        ...stateWithUndo,
        sectionConfigs: newConfigs,
        nextTempId: state.nextTempId + 1,
      };
    }

    case 'INSERT_SHOT_AFTER': {
      if (!currentConfig) return state;

      // Save to undo stack
      const previousPartial: Partial<SectionConfig> = {
        shots: deepCopyShots(currentConfig.shots),
      };
      let stateWithUndo = pushToUndoStack(state, 'shotAdd', `Insert shot after ${action.index}`, previousPartial);

      // Insert new shot
      const newShot = createBlankShot(state.nextTempId);
      const newShots = [...currentConfig.shots];
      newShots.splice(action.index + 1, 0, newShot);

      const newConfigs = new Map(stateWithUndo.sectionConfigs);
      newConfigs.set(state.currentSectionIndex, {
        ...currentConfig,
        shots: newShots,
      });

      return {
        ...stateWithUndo,
        sectionConfigs: newConfigs,
        nextTempId: state.nextTempId + 1,
      };
    }

    case 'SET_PLACEMENT': {
      if (!currentConfig) return state;

      // Save to undo stack
      const previousPartial: Partial<SectionConfig> = {
        placement: currentConfig.placement,
      };
      let stateWithUndo = pushToUndoStack(state, 'placementChange', 'Change placement', previousPartial);

      // Update placement (does NOT mark section as placed - that only happens via MARK_PLACED)
      const newConfigs = new Map(stateWithUndo.sectionConfigs);
      newConfigs.set(state.currentSectionIndex, {
        ...currentConfig,
        placement: action.placement,
      });

      return {
        ...stateWithUndo,
        sectionConfigs: newConfigs,
      };
    }

    case 'MARK_PLACED': {
      const newPlaced = new Set(state.placedSections);
      newPlaced.add(action.sectionIndex);
      return {
        ...state,
        placedSections: newPlaced,
      };
    }

    case 'UNMARK_PLACED': {
      const newPlaced = new Set(state.placedSections);
      newPlaced.delete(action.sectionIndex);
      return {
        ...state,
        placedSections: newPlaced,
      };
    }

    case 'EXCLUDE_SECTION': {
      const newExcluded = new Set(state.excludedSections);
      newExcluded.add(action.sectionIndex);
      // Also remove from placed if it was placed
      const newPlaced = new Set(state.placedSections);
      newPlaced.delete(action.sectionIndex);
      return {
        ...state,
        excludedSections: newExcluded,
        placedSections: newPlaced,
      };
    }

    case 'INCLUDE_SECTION': {
      const newExcluded = new Set(state.excludedSections);
      newExcluded.delete(action.sectionIndex);
      return {
        ...state,
        excludedSections: newExcluded,
      };
    }

    case 'UNDO': {
      const undoStack = state.undoStacks.get(state.currentSectionIndex) || [];
      if (undoStack.length === 0 || !currentConfig) return state;

      // Pop from undo stack
      const entry = undoStack[undoStack.length - 1];
      if (!entry) return state;

      const newUndoStack = undoStack.slice(0, -1);

      // Push current state to redo stack
      const redoStack = state.redoStacks.get(state.currentSectionIndex) || [];
      const redoEntry: UndoEntry = {
        type: entry.type,
        previousState: {},
        description: entry.description,
      };

      // Build redo state from current config
      for (const key of Object.keys(entry.previousState) as (keyof SectionConfig)[]) {
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

      const newConfigs = new Map(state.sectionConfigs);
      newConfigs.set(state.currentSectionIndex, restoredConfig);

      const newUndoStacks = new Map(state.undoStacks);
      newUndoStacks.set(state.currentSectionIndex, newUndoStack);

      const newRedoStacks = new Map(state.redoStacks);
      newRedoStacks.set(state.currentSectionIndex, newRedoStack);

      return {
        ...state,
        sectionConfigs: newConfigs,
        undoStacks: newUndoStacks,
        redoStacks: newRedoStacks,
      };
    }

    case 'REDO': {
      const redoStack = state.redoStacks.get(state.currentSectionIndex) || [];
      if (redoStack.length === 0 || !currentConfig) return state;

      // Pop from redo stack
      const entry = redoStack[redoStack.length - 1];
      if (!entry) return state;

      const newRedoStack = redoStack.slice(0, -1);

      // Push current state to undo stack
      const undoStack = state.undoStacks.get(state.currentSectionIndex) || [];
      const undoEntry: UndoEntry = {
        type: entry.type,
        previousState: {},
        description: entry.description,
      };

      // Build undo state from current config
      for (const key of Object.keys(entry.previousState) as (keyof SectionConfig)[]) {
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

      const newConfigs = new Map(state.sectionConfigs);
      newConfigs.set(state.currentSectionIndex, restoredConfig);

      const newUndoStacks = new Map(state.undoStacks);
      newUndoStacks.set(state.currentSectionIndex, newUndoStack);

      const newRedoStacks = new Map(state.redoStacks);
      newRedoStacks.set(state.currentSectionIndex, newRedoStack);

      return {
        ...state,
        sectionConfigs: newConfigs,
        undoStacks: newUndoStacks,
        redoStacks: newRedoStacks,
      };
    }

    case 'RESET_SECTION': {
      const original = state.originalValues.get(state.currentSectionIndex);
      if (!original) return state;

      // Reset to original values (deep copy)
      const resetConfig: SectionConfig = {
        ...original,
        shots: deepCopyShots(original.shots),
        placement: { type: 'origin' },
      };

      const newConfigs = new Map(state.sectionConfigs);
      newConfigs.set(state.currentSectionIndex, resetConfig);

      // Remove from placed sections
      const newPlaced = new Set(state.placedSections);
      newPlaced.delete(state.currentSectionIndex);

      // Clear undo/redo stacks for this section
      const newUndoStacks = new Map(state.undoStacks);
      newUndoStacks.set(state.currentSectionIndex, []);

      const newRedoStacks = new Map(state.redoStacks);
      newRedoStacks.set(state.currentSectionIndex, []);

      return {
        ...state,
        sectionConfigs: newConfigs,
        placedSections: newPlaced,
        undoStacks: newUndoStacks,
        redoStacks: newRedoStacks,
      };
    }

    case 'RESTORE_STATE': {
      const nextTempId = Math.max(
        state.nextTempId,
        getNextTempIdFromConfigs(action.sectionConfigs)
      );
      return {
        ...state,
        sectionConfigs: action.sectionConfigs,
        placedSections: action.placedSections,
        excludedSections: action.excludedSections,
        currentSectionIndex: action.currentSectionIndex,
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
    sectionConfigs: Array<[number, SectionConfig]>;
    placedSections: number[];
    excludedSections?: number[];
    currentSectionIndex: number;
  },
  resolvedTheme = 'dark'
): ImportState {
  const selectedIndices = Array.from(selectionConfig.selectedIndices).sort((a, b) => a - b);
  const firstSectionIndex = selectedIndices[0];
  const palette = sectionColorPalette(resolvedTheme);

  // Build original values (these never change)
  const originalValues = new Map<number, SectionConfig>();
  selectedIndices.forEach((sectionIdx, arrayIdx) => {
    const section = dmpFile.sections[sectionIdx];
    if (!section) return;

    const dateStr = section.date?.split(/[T ]/)[0] ?? null;

    originalValues.set(sectionIdx, {
      sectionIndex: sectionIdx,
      name: section.name,
      surveyor: null, // DMP sections don't have surveyor - use defaultSurveyor from Step 1
      date: dateStr,
      color: palette[arrayIdx % palette.length] ?? '#4ecdc4',
      direction: section.direction,
      quality: 1.0,
      shots: convertToEditableShots(section.shots),
      placement: { type: 'origin' },
    });
  });

  // If restoring from saved state
  if (savedState) {
    const sectionConfigs = new Map(
      savedState.sectionConfigs.map(([idx, cfg]) => [
        idx,
        { ...cfg, quality: cfg.quality ?? 1.0 },
      ])
    );
    return {
      sectionConfigs,
      currentSectionIndex: savedState.currentSectionIndex,
      nextTempId: getNextTempIdFromConfigs(sectionConfigs),
      placedSections: new Set(savedState.placedSections),
      excludedSections: new Set(savedState.excludedSections ?? []),
      undoStacks: new Map(),
      redoStacks: new Map(),
      originalValues,
    };
  }

  // Build initial section configs
  const sectionConfigs = new Map<number, SectionConfig>();
  selectedIndices.forEach((sectionIdx, arrayIdx) => {
    const section = dmpFile.sections[sectionIdx];
    if (!section) return;

    const direction = section.direction;
    // Use default surveyor from step 1, or null if not provided (don't fall back to section name)
    const surveyor = selectionConfig.defaultSurveyor || null;
    const dateStr = selectionConfig.defaultDate ?? section.date?.split(/[T ]/)[0] ?? null;

    sectionConfigs.set(sectionIdx, {
      sectionIndex: sectionIdx,
      name: section.name,
      surveyor,
      date: dateStr,
      color: palette[arrayIdx % palette.length] ?? '#4ecdc4',
      direction,
      quality: 1.0,
      shots: convertToEditableShots(section.shots),
      placement: { type: 'origin' },
    });
  });

  // No auto-placement - all sections start as unplaced
  const placedSections = new Set<number>();
  const excludedSections = new Set<number>();

  const nextTempId = getNextTempIdFromConfigs(sectionConfigs);

  return {
    sectionConfigs,
    currentSectionIndex: firstSectionIndex ?? 0,
    nextTempId,
    placedSections,
    excludedSections,
    undoStacks: new Map(),
    redoStacks: new Map(),
    originalValues,
  };
}

/**
 * Hook for managing import state with undo/redo functionality.
 * Provides per-section state tracking, undo/redo stacks, and reset functionality.
 */
export function useImportState(
  dmpFile: DmpFile,
  selectionConfig: DmpSelectionConfig,
  savedState?: {
    sectionConfigs: Array<[number, SectionConfig]>;
    placedSections: number[];
    excludedSections?: number[];
    currentSectionIndex: number;
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
    return selectedIndices.indexOf(state.currentSectionIndex);
  }, [selectedIndices, state.currentSectionIndex]);

  // Current config
  const currentConfig = state.sectionConfigs.get(state.currentSectionIndex);

  // Can undo/redo for current section
  const canUndo = useMemo(() => {
    const stack = state.undoStacks.get(state.currentSectionIndex);
    return (stack?.length ?? 0) > 0;
  }, [state.undoStacks, state.currentSectionIndex]);

  const canRedo = useMemo(() => {
    const stack = state.redoStacks.get(state.currentSectionIndex);
    return (stack?.length ?? 0) > 0;
  }, [state.redoStacks, state.currentSectionIndex]);

  // Actions
  const navigateToSection = useCallback((sectionIndex: number) => {
    dispatch({ type: 'NAVIGATE', sectionIndex });
  }, []);

  const navigateToArrayIndex = useCallback((arrayIndex: number) => {
    const sectionIndex = selectedIndices[arrayIndex];
    if (sectionIndex !== undefined) {
      dispatch({ type: 'NAVIGATE', sectionIndex });
    }
  }, [selectedIndices]);

  const updateSectionField = useCallback(<K extends keyof SectionConfig>(
    field: K,
    value: SectionConfig[K]
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

  const markPlaced = useCallback((sectionIndex: number) => {
    dispatch({ type: 'MARK_PLACED', sectionIndex });
  }, []);

  const unmarkPlaced = useCallback((sectionIndex: number) => {
    dispatch({ type: 'UNMARK_PLACED', sectionIndex });
  }, []);

  const excludeSection = useCallback((sectionIndex: number) => {
    dispatch({ type: 'EXCLUDE_SECTION', sectionIndex });
  }, []);

  const includeSection = useCallback((sectionIndex: number) => {
    dispatch({ type: 'INCLUDE_SECTION', sectionIndex });
  }, []);

  const undo = useCallback(() => {
    dispatch({ type: 'UNDO' });
  }, []);

  const redo = useCallback(() => {
    dispatch({ type: 'REDO' });
  }, []);

  const resetSection = useCallback(() => {
    dispatch({ type: 'RESET_SECTION' });
  }, []);

  const restoreState = useCallback((
    sectionConfigs: Map<number, SectionConfig>,
    placedSections: Set<number>,
    excludedSections: Set<number>,
    currentSectionIndex: number
  ) => {
    dispatch({ type: 'RESTORE_STATE', sectionConfigs, placedSections, excludedSections, currentSectionIndex });
  }, []);

  // Serialization helpers for state persistence
  const serializeSectionConfigs = useCallback(() => {
    return Array.from(state.sectionConfigs.entries());
  }, [state.sectionConfigs]);

  const serializePlacedSections = useCallback(() => {
    return Array.from(state.placedSections);
  }, [state.placedSections]);

  const serializeExcludedSections = useCallback(() => {
    return Array.from(state.excludedSections);
  }, [state.excludedSections]);

  /**
   * Get the next unplaced section index (not placed and not excluded).
   * Returns undefined if all sections are placed or excluded.
   *
   * @param afterIndex - Section index to start searching after
   * @param excludeSection - Optional section index to exclude from search (used when
   *                         the section is being marked as placed but state hasn't updated yet)
   */
  const getNextUnplacedSection = useCallback((afterIndex?: number, excludeSection?: number): number | undefined => {
    const startIdx = afterIndex !== undefined
      ? selectedIndices.indexOf(afterIndex) + 1
      : 0;

    // First check from startIdx to end
    for (let i = startIdx; i < selectedIndices.length; i++) {
      const sectionIndex = selectedIndices[i];
      if (sectionIndex === undefined) continue;
      // Skip the section being placed (state not yet updated)
      if (sectionIndex === excludeSection) continue;
      if (!state.placedSections.has(sectionIndex) && !state.excludedSections.has(sectionIndex)) {
        return sectionIndex;
      }
    }

    // Then check from beginning to startIdx (wrap around)
    for (let i = 0; i < startIdx; i++) {
      const sectionIndex = selectedIndices[i];
      if (sectionIndex === undefined) continue;
      // Skip the section being placed (state not yet updated)
      if (sectionIndex === excludeSection) continue;
      if (!state.placedSections.has(sectionIndex) && !state.excludedSections.has(sectionIndex)) {
        return sectionIndex;
      }
    }

    return undefined;
  }, [selectedIndices, state.placedSections, state.excludedSections]);

  /**
   * Check if all sections are either placed or excluded.
   */
  const allSectionsHandled = useMemo(() => {
    for (const sectionIndex of selectedIndices) {
      if (!state.placedSections.has(sectionIndex) && !state.excludedSections.has(sectionIndex)) {
        return false;
      }
    }
    return true;
  }, [selectedIndices, state.placedSections, state.excludedSections]);

  /**
   * Check if all sections are excluded.
   */
  const allSectionsExcluded = useMemo(() => {
    for (const sectionIndex of selectedIndices) {
      if (!state.excludedSections.has(sectionIndex)) {
        return false;
      }
    }
    return true;
  }, [selectedIndices, state.excludedSections]);

  // Get section status - now considers validation errors and excluded status
  const getSectionStatus = useCallback((sectionIndex: number): 'placed' | 'not_placed' | 'errors' | 'excluded' => {
    // Check excluded first
    if (state.excludedSections.has(sectionIndex)) return 'excluded';

    const config = state.sectionConfigs.get(sectionIndex);
    if (!config) return 'not_placed';

    const validShots = config.shots.filter((s) => !s.isDeleted);
    if (validShots.length === 0) return 'errors';

    // Check for shot validation errors
    for (let i = 0; i < config.shots.length; i++) {
      const shot = config.shots[i];
      if (!shot || shot.isDeleted) continue;
      const shotErrors = validateEditableShot(shot, i, sectionIndex);
      if (shotErrors.length > 0) return 'errors';
    }

    if (state.placedSections.has(sectionIndex)) return 'placed';

    return 'not_placed';
  }, [state.sectionConfigs, state.placedSections, state.excludedSections]);

  // Get all configs for import (excludes excluded sections)
  const getAllConfigs = useCallback(() => {
    return Array.from(state.sectionConfigs.values())
      .filter((config) => !state.excludedSections.has(config.sectionIndex))
      .map((config) => ({
        ...config,
        shots: config.shots.filter((s) => !s.isDeleted),
      }));
  }, [state.sectionConfigs, state.excludedSections]);

  /**
   * Get configs for sections that have been placed (for use as targets in "From existing station").
   * Returns placed sections with their names and station counts.
   */
  const getPlacedSectionConfigs = useCallback(() => {
    const placedConfigs: Array<{ sectionIndex: number; name: string; stationCount: number }> = [];
    for (const sectionIndex of state.placedSections) {
      const config = state.sectionConfigs.get(sectionIndex);
      if (config) {
        const validShots = config.shots.filter((s) => !s.isDeleted);
        placedConfigs.push({
          sectionIndex,
          name: config.name,
          stationCount: validShots.length + 1, // shots + 1 = stations
        });
      }
    }
    return placedConfigs;
  }, [state.placedSections, state.sectionConfigs]);

  /**
   * Get all errors for a specific section.
   * Includes shot validation errors, placement errors, and merge conflicts.
   */
  const getSectionErrors = useCallback((
    sectionIndex: number,
    existingSections: Section[]
  ): ImportError[] => {
    const config = state.sectionConfigs.get(sectionIndex);
    if (!config) return [];

    const errors: ImportError[] = [];

    // Validate each shot
    config.shots.forEach((shot, shotIndex) => {
      if (!shot.isDeleted) {
        const shotErrors = validateEditableShot(shot, shotIndex, sectionIndex);
        errors.push(...shotErrors);
      }
    });

    // Check for empty section (no valid shots)
    const validShots = config.shots.filter((s) => !s.isDeleted);
    if (validShots.length === 0) {
      errors.push({
        type: 'invalidData',
        sectionIndex,
        message: 'Section has no valid shots',
      });
    }

    // Validate placement
    const isPlaced = state.placedSections.has(sectionIndex);
    const placementError = validatePlacement(isPlaced, sectionIndex);
    if (placementError) {
      errors.push(placementError);
    }

    // Validate merge target if using fromExisting placement
    if (config.placement.type === 'fromExisting') {
      const existingSectionIds = new Set(existingSections.map((s) => s.id));
      // Get target section id from placement config
      const targetSectionId = config.placement.existingStation.section_id;

      // Get set of section indices that have already been placed in this import
      // (these could be valid merge targets)
      const placedSectionIndices = state.placedSections;

      const mergeError = validateMergeTarget(
        config.placement.type,
        targetSectionId,
        existingSectionIds,
        placedSectionIndices,
        sectionIndex
      );
      if (mergeError) {
        errors.push(mergeError);
      }
    }

    return errors;
  }, [state.sectionConfigs, state.placedSections]);

  /**
   * Get all errors for all sections.
   * Returns a map of section index to array of errors.
   */
  const getAllSectionErrors = useCallback((
    existingSections: Section[]
  ): Map<number, ImportError[]> => {
    const allErrors = new Map<number, ImportError[]>();

    for (const sectionIndex of selectedIndices) {
      const errors = getSectionErrors(sectionIndex, existingSections);
      if (errors.length > 0) {
        allErrors.set(sectionIndex, errors);
      }
    }

    return allErrors;
  }, [selectedIndices, getSectionErrors]);

  /**
   * Get the error count for a specific section.
   */
  const getSectionErrorCount = useCallback((
    sectionIndex: number,
    existingSections: Section[]
  ): number => {
    return getSectionErrors(sectionIndex, existingSections).length;
  }, [getSectionErrors]);

  /**
   * Get the data error count for a specific section (excluding placement errors).
   * This is used to determine if the Place button should be enabled.
   */
  const getSectionDataErrorCount = useCallback((
    sectionIndex: number,
    existingSections: Section[]
  ): number => {
    const errors = getSectionErrors(sectionIndex, existingSections);
    // Filter out missingPlacement errors - those are expected before placing
    return errors.filter((e) => e.type !== 'missingPlacement').length;
  }, [getSectionErrors]);

  /**
   * Get the warning count for a specific section (azimuth/pitch discrepancies).
   * Counts discrepancy warnings across non-deleted shots; a single shot can
   * contribute up to 2 warnings (one azimuth, one pitch).
   * A threshold of 0 means that warning type is disabled.
   */
  const getSectionWarningCount = useCallback((
    sectionIndex: number,
    valConfig: Pick<ValidationConfig, 'azimuth_diff_threshold' | 'pitch_diff_threshold'>
  ): number => {
    const sectionConfig = state.sectionConfigs.get(sectionIndex);
    if (!sectionConfig) return 0;
    let warnings = 0;
    for (const shot of sectionConfig.shots) {
      if (shot.isDeleted) continue;
      if (valConfig.azimuth_diff_threshold > 0 && hasAzimuthDiscrepancy(shot.azimuth_in, shot.azimuth_out, valConfig.azimuth_diff_threshold)) warnings++;
      if (valConfig.pitch_diff_threshold > 0 && hasPitchDiscrepancy(shot.pitch_in, shot.pitch_out, valConfig.pitch_diff_threshold)) warnings++;
    }
    return warnings;
  }, [state.sectionConfigs]);

  /**
   * Get total error count across all sections.
   */
  const getTotalErrorCount = useCallback((
    existingSections: Section[]
  ): number => {
    let total = 0;
    for (const sectionIndex of selectedIndices) {
      total += getSectionErrors(sectionIndex, existingSections).length;
    }
    return total;
  }, [selectedIndices, getSectionErrors]);

  /**
   * Get total data error count across all sections (excluding placement errors).
   * Used for the error summary display.
   */
  const getTotalDataErrorCount = useCallback((
    existingSections: Section[]
  ): number => {
    let total = 0;
    for (const sectionIndex of selectedIndices) {
      total += getSectionDataErrorCount(sectionIndex, existingSections);
    }
    return total;
  }, [selectedIndices, getSectionDataErrorCount]);

  /**
   * Check if all sections can be imported (no errors and all placed or excluded).
   */
  const canImport = useCallback((
    existingSections: Section[]
  ): boolean => {
    // If all sections are excluded, we can't import (nothing to import)
    if (state.excludedSections.size === selectedIndices.length) {
      return false;
    }

    for (const sectionIndex of selectedIndices) {
      // Skip excluded sections - they don't need to be validated
      if (state.excludedSections.has(sectionIndex)) continue;

      const config = state.sectionConfigs.get(sectionIndex);
      if (!config) return false;

      // Check for valid shots
      const validShots = config.shots.filter((s) => !s.isDeleted);
      if (validShots.length === 0) return false;

      // Check for shot errors
      for (let i = 0; i < config.shots.length; i++) {
        const shot = config.shots[i];
        if (!shot || shot.isDeleted) continue;
        const shotErrors = validateEditableShot(shot, i, sectionIndex);
        if (shotErrors.length > 0) return false;
      }

      // Check if placed
      if (!state.placedSections.has(sectionIndex)) return false;

      // Check merge target validity
      if (config.placement.type === 'fromExisting') {
        const existingSectionIds = new Set(existingSections.map((s) => s.id));
        const targetSectionId = config.placement.existingStation.section_id;
        const mergeError = validateMergeTarget(
          config.placement.type,
          targetSectionId,
          existingSectionIds,
          state.placedSections,
          sectionIndex
        );
        if (mergeError) return false;
      }
    }
    return true;
  }, [selectedIndices, state.sectionConfigs, state.placedSections, state.excludedSections]);

  return {
    // State
    state,
    currentConfig,
    currentSectionIndex: state.currentSectionIndex,
    currentArrayIndex,
    selectedIndices,
    sectionConfigs: state.sectionConfigs,
    placedSections: state.placedSections,
    excludedSections: state.excludedSections,
    canUndo,
    canRedo,
    allSectionsHandled,
    allSectionsExcluded,

    // Navigation
    navigateToSection,
    navigateToArrayIndex,
    getNextUnplacedSection,

    // Section field updates
    updateSectionField,

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
    excludeSection,
    includeSection,

    // Undo/Redo
    undo,
    redo,

    // Reset
    resetSection,

    // State restoration
    restoreState,

    // Serialization
    serializeSectionConfigs,
    serializePlacedSections,
    serializeExcludedSections,

    // Status
    getSectionStatus,

    // Export
    getAllConfigs,
    getPlacedSectionConfigs,

    // Error validation
    getSectionErrors,
    getAllSectionErrors,
    getSectionErrorCount,
    getSectionDataErrorCount,
    getSectionWarningCount,
    getTotalErrorCount,
    getTotalDataErrorCount,
    canImport,
  };
}

export type UseImportStateReturn = ReturnType<typeof useImportState>;
