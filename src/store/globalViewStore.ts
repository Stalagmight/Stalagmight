import { create } from 'zustand';
import { invokeIpc } from '../utils/ipc';
import { getErrorMessage } from '../utils/commandError';
import type { SurveyGeometry } from '../types/survey';

/**
 * Loading state for a system's geometry.
 */
export type GeometryLoadingState = 'idle' | 'loading' | 'loaded' | 'error';

/**
 * Per-system geometry state.
 */
export interface SystemGeometryState {
  /** Loading state of the geometry */
  state: GeometryLoadingState;
  /** The loaded geometry data (when state === 'loaded') */
  geometry?: SurveyGeometry;
  /** Error message (when state === 'error') */
  error?: string;
}

interface GlobalViewStore {
  /**
   * Generation counter for cancellation.
   * Incremented on clearGeometryCache to invalidate in-flight requests.
   */
  generation: number;

  /**
   * Geometry state keyed by file_path.
   * Systems not in the map are in 'idle' state.
   */
  geometryStates: Map<string, SystemGeometryState>;

  /**
   * Loaded geometry data for rendering.
   * Map of file_path -> SurveyGeometry.
   */
  geometryData: Map<string, SurveyGeometry>;

  /**
   * Selected system IDs for multi-select.
   * Empty set means no selection.
   */
  selectedSystemIds: Set<string>;

  /**
   * Load geometry for a system by file path.
   * Skips if already loading/loaded.
   */
  loadGeometry: (filePath: string) => Promise<void>;

  /**
   * Load geometry for multiple systems sequentially.
   * Stops if generation changes (cache cleared).
   *
   * A system whose load FAILED is skipped unless `retryErrors` is set (#576
   * D10-D1): the known causes (no GPS reference, an anchor that no longer
   * resolves, an unreadable file) do not heal on their own, so retrying on every
   * pass only re-reads the file. The Global View sets it for the user's own
   * actions - reloading the recent list, switching back to the survey view.
   */
  loadGeometryBatch: (filePaths: string[], options?: { retryErrors?: boolean }) => Promise<void>;

  /**
   * Get the loading state for a system.
   */
  getLoadingState: (filePath: string) => GeometryLoadingState;

  /**
   * Clear all cached geometry and cancel pending loads.
   * Call when entering single-system view.
   */
  clearGeometryCache: () => void;

  /**
   * Cancel in-flight geometry loads without clearing already-loaded data.
   * Increments generation to invalidate pending requests while preserving
   * geometryData and geometryStates for already-loaded systems.
   * Call when switching to markers-only view.
   */
  cancelPendingLoads: () => void;

  /**
   * Select a single system (clears existing selection).
   */
  selectSystem: (filePath: string | null) => void;

  /**
   * Toggle a system's selection (for Ctrl+click).
   * If already selected, removes from selection.
   * If not selected, adds to selection.
   */
  toggleSystemSelection: (filePath: string) => void;

  /**
   * Set multiple systems as selected (replaces existing selection).
   * Used for Ctrl+A select all.
   */
  setSelectedSystems: (filePaths: Set<string>) => void;

  /**
   * Clear all selection.
   */
  clearSelection: () => void;
}

export const useGlobalViewStore = create<GlobalViewStore>((set, get) => ({
  generation: 0,
  geometryStates: new Map(),
  geometryData: new Map(),
  selectedSystemIds: new Set(),

  loadGeometry: async (filePath: string) => {
    const startGeneration = get().generation;
    const { geometryStates } = get();

    // Skip if already loading or loaded
    const currentState = geometryStates.get(filePath);
    if (currentState?.state === 'loading' || currentState?.state === 'loaded') {
      return;
    }

    // Mark as loading
    set((state) => ({
      geometryStates: new Map(state.geometryStates).set(filePath, {
        state: 'loading',
      }),
    }));

    try {
      const geometry = await invokeIpc('get_survey_geometry_for_file', {
        path: filePath,
      }, { purpose: 'load survey geometry for file' });

      // Check if generation changed while in flight (cache was cleared)
      if (get().generation !== startGeneration) {
        return; // Discard result, don't update state
      }

      // Success - update state and cache geometry
      set((state) => ({
        geometryStates: new Map(state.geometryStates).set(filePath, {
          state: 'loaded',
          geometry,
        }),
        geometryData: new Map(state.geometryData).set(filePath, geometry),
      }));
    } catch (e) {
      // Check if generation changed while in flight
      if (get().generation !== startGeneration) {
        return; // Discard error, don't update state
      }

      // The command's CommandError (or a transport failure): its display message.
      const errorMessage = getErrorMessage(e, 'Failed to load geometry');

      set((state) => ({
        geometryStates: new Map(state.geometryStates).set(filePath, {
          state: 'error',
          error: errorMessage,
        }),
      }));
    }
  },

  loadGeometryBatch: async (filePaths: string[], options?: { retryErrors?: boolean }) => {
    const startGeneration = get().generation;
    const { getLoadingState } = get();

    // Filter out already loading/loaded systems, and failed ones unless asked.
    const toLoad = filePaths.filter((fp) => {
      const state = getLoadingState(fp);
      return state === 'idle' || (state === 'error' && options?.retryErrors === true);
    });

    // Load sequentially to prevent overwhelming backend
    for (const filePath of toLoad) {
      // Stop if generation changed (cache cleared)
      if (get().generation !== startGeneration) {
        return;
      }
      await get().loadGeometry(filePath);
    }
  },

  getLoadingState: (filePath: string) => {
    return get().geometryStates.get(filePath)?.state ?? 'idle';
  },

  clearGeometryCache: () => {
    set((state) => ({
      generation: state.generation + 1,
      geometryStates: new Map(),
      geometryData: new Map(),
    }));
  },

  cancelPendingLoads: () => {
    // Bump generation to invalidate in-flight requests
    // but preserve geometryData and loaded states
    set((state) => {
      // Remove 'loading' entries from geometryStates (reset to idle)
      // Keep 'loaded' and 'error' states intact
      const newStates = new Map<string, SystemGeometryState>();
      for (const [key, value] of state.geometryStates) {
        if (value.state !== 'loading') {
          newStates.set(key, value);
        }
      }
      return {
        generation: state.generation + 1,
        geometryStates: newStates,
      };
    });
  },

  selectSystem: (filePath: string | null) => {
    if (filePath === null) {
      set({ selectedSystemIds: new Set() });
    } else {
      set({ selectedSystemIds: new Set([filePath]) });
    }
  },

  toggleSystemSelection: (filePath: string) => {
    set((state) => {
      const newSelection = new Set(state.selectedSystemIds);
      if (newSelection.has(filePath)) {
        newSelection.delete(filePath);
      } else {
        newSelection.add(filePath);
      }
      return { selectedSystemIds: newSelection };
    });
  },

  setSelectedSystems: (filePaths: Set<string>) => {
    set({ selectedSystemIds: new Set(filePaths) });
  },

  clearSelection: () => {
    set({ selectedSystemIds: new Set() });
  },
}));
