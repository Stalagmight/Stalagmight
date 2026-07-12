import { create } from 'zustand';
import i18n from '../i18n';
import { invokeIpc } from '../utils/ipc';
import { trackCommand, setSource, peekProjectActionSource, clearProjectActionSource } from '../utils/telemetry';
import { debugLog } from '../utils/debugLog';
import { extractFilename } from '../utils/path';
import { isPermissionError } from '../utils/saveErrors';
import {
  isEditDisabledError,
  isReverifyDivergedError,
  READ_ONLY_EDIT_NOTICE,
} from '../utils/editErrors';
import { hasEligibleClosures } from '../utils/closures';
import { useUIStore } from './uiStore';
import { surfaceSaveFailure } from '../utils/saveFeedback';
import { useGlobalViewStore } from './globalViewStore';
import { useSpeleoStore } from './speleoStore';
import type {
  Project,
  ProjectState,
  ProjectMetadata,
  Station,
  ProjectStats,
  SectionData,
  SectionUpdates,
  ShotData,
  ShotUpdates,
  SectionId,
  ShotId,
  StationRef,
  GpsCoordinates,
  ClosureStrategy,
  Coordinates,
  FrozenShotRef,
  MarkerType,
  MarkerDirection,
} from '../types/project';
import type { MarkerUpdates } from '../types/ipc/MarkerUpdates';
import type { Config } from '../types/config';
import type { CommandResult, CommentHolder, ConnectivityResult, DmpFile, TmlFile, CompassFile, WallsFile, GpxFile, GpxSectionPlacement, ExportImageOptions, ProjectWarning, BatchCommand, UndoHistoryItem, ImportSectionConfig, PlacementSuggestion, CollabState, CollabChangeSet, IntegrityRepairResult, IntegrityCheckResult } from '../types/commands';
import type { CreateProjectFullResult } from '../types/loading';
import type { SurveyData } from '../types/survey-data';
import { emptySurveyData, surveyDataFromLegacy } from '../types/survey-data';

type ShotUpdatesPatch = Partial<ShotUpdates>;

/**
 * Structured loading state for project load/create operations.
 * Tracks progress phases, current/total counts, and error states.
 */
export interface LoadingState {
  /**
   * Backend loading ID for filtering stale progress events.
   * Set from the first progress event or result.loadingId.
   * Value of 0 means "accept events until we learn the backend ID".
   */
  loadingId: bigint;
  /**
   * Client operation ID for race detection (local to frontend).
   * Incremented on each load start. Used to detect if a newer load started.
   */
  clientOpId: number;
  /** Whether a load operation is currently in progress */
  isLoading: boolean;
  /** Name of the project being loaded */
  projectName?: string | undefined;
  /** Path to the project file (for retry functionality) */
  projectPath?: string | undefined;
  /** Current phase enum value (snake_case from backend) */
  phase?: string | undefined;
  /** Index of current phase (0-based) */
  phaseIndex?: number | undefined;
  /** Total number of phases */
  phaseCount?: number | undefined;
  /** Current phase progress percentage (0-100), undefined for indeterminate */
  phaseProgress?: number | undefined;
  /** Current item count in phase */
  phaseCurrent?: number | undefined;
  /** Total items in phase */
  phaseTotal?: number | undefined;
  /** Overall loading progress percentage (0-100) */
  overallProgress: number;
  /** Error information if load failed */
  error?: { message: string; details?: string | undefined } | undefined;
  /** Timestamp of last event (for timeout detection) */
  lastEventTime?: number | undefined;
}

const initialLoadingState: LoadingState = {
  loadingId: 0n,
  clientOpId: 0,
  isLoading: false,
  overallProgress: 0,
};

// Separate client operation ID for race detection (independent of backend loading ID)
let clientOpIdCounter = 0;

// Monotonic sequence so overlapping fire-and-forget integrity checks are
// last-issued-wins: an earlier check that resolves after a newer one was started
// must not clobber the newer result (e.g. a manual save's check racing a Repair's
// check on the same project, where the project-identity guard can't tell them apart).
let integrityCheckSeq = 0;

/**
 * Parsed error from Tauri backend.
 */
interface LoadError {
  code: string;
  message: string;
  details?: string | undefined;
}

interface CreateProjectOptions {
  savePath?: string;
  saveDirectory?: string;
  deferInitialSave?: boolean;
}

function emptyCommandResult(
  success: boolean,
  error?: string | null,
  warnings: CommandResult['warnings'] = []
): CommandResult {
  return {
    success,
    warnings,
    error: success ? null : (error ?? null),
    section_ids: [],
  };
}

/**
 * Parse error from Tauri backend.
 * Tauri may serialize errors as strings or objects.
 */
function parseLoadError(e: unknown): LoadError {
  // Tauri serializes errors as strings - try parsing as JSON
  if (typeof e === 'string') {
    try {
      return JSON.parse(e) as LoadError;
    } catch {
      return { code: 'UNKNOWN', message: e };
    }
  }
  // Already an object
  if (typeof e === 'object' && e !== null) {
    const obj = e as Record<string, unknown>;
    return {
      code: (obj.code as string) || 'UNKNOWN',
      message: (obj.message as string) || String(e),
      details: obj.details as string | undefined,
    };
  }
  return { code: 'UNKNOWN', message: String(e) };
}

interface ProjectStore {
  // State
  project: Project | null;
  /**
   * Whether the open project is backed by an on-disk database. A new project
   * created with a deferred initial save reserves `project.file_path` but leaves
   * the backend database unopened until the first save, so DB-backed actions
   * (e.g. persisting an ignored warning) can't run yet — gate them on this, not
   * on `file_path` alone (issue #57 / Codex).
   */
  databaseOpen: boolean;
  stations: Station[];
  /**
   * Struct-of-Arrays representation of the loaded project's bulk
   * data (shots + stations). Lives alongside `project` and `stations`
   * during the Phase-A migration; once consumers are migrated this
   * becomes the primary representation and `project.sections[].shots`
   * + `stations` go away.
   */
  surveyData: SurveyData | null;
  /**
   * Closure-adjusted geometry for the read-only adjusted display mode, derived
   * non-destructively on the backend and fetched lazily. `null` until fetched
   * and after any edit (which invalidates it); rendering falls back to the
   * measured `stations`/`surveyData` while null. See geometrySelectors.ts.
   */
  stationsAdjusted: Station[] | null;
  surveyDataAdjusted: SurveyData | null;
  /** True while the closure-adjusted geometry is being solved/fetched. */
  adjustedLoading: boolean;
  stats: ProjectStats | null;
  frozenShots: FrozenShotRef[];
  warnings: ProjectWarning[];
  recentProjects: ProjectMetadata[];
  config: Config | null;
  /** Structured loading state with progress tracking */
  loading: LoadingState;
  error: string | null;
  canUndo: boolean;
  canRedo: boolean;
  isDirty: boolean;
  /** Guard to prevent concurrent close operations (rapid Back clicks) */
  isClosing: boolean;

  // Actions
  loadConfig: () => Promise<void>;
  updateConfig: (config: Config) => Promise<CommandResult>;
  /** Update config state without persisting to disk */
  setConfig: (config: Config) => void;

  // Loading state actions
  /** Start a new loading operation, returns the loadingId */
  startLoading: (projectName: string, projectPath: string) => number;
  /** Update loading progress state */
  setLoadingProgress: (progress: Partial<LoadingState>) => void;
  /** Reset loading state to initial */
  resetLoading: () => void;
  /** Set loading error (keeps modal open) */
  setLoadingError: (message: string, details?: string) => void;
  /** Cancel any in-progress loading operation */
  cancelLoading: () => Promise<void>;

  /** Load a project using the consolidated load_project_full command */
  loadProject: (path: string) => Promise<{ success: boolean }>;
  /** Create a new project (legacy, still supported) */
  createProject: (name: string, projectLocation?: string, options?: CreateProjectOptions) => Promise<CommandResult>;
  /**
   * Save the project.  `isAutosave` should be set to `true` from the
   * autosave hook so the Rust side can honour
   * `CollabState.write_tml_on_autosave` and skip the sibling `.tml`
   * write when the user has disabled it.  Manual / Save-As flows
   * leave it `false` (the default) so the .tml is always written.
   */
  saveProject: (path?: string, isAutosave?: boolean) => Promise<CommandResult>;
  /**
   * Persist the project after a DMP import completes (#92). Imports bypass the
   * dirty/save machinery, so the change would otherwise be lost. For a NEW
   * project (`importContext === 'new-project'`) the initial save is deferred —
   * only a real save writes the `.stalx` and registers it in Recent Projects —
   * so this saves. For an import into an EXISTING project it only marks dirty, so
   * the user's unrelated pending edits aren't silently committed; the normal
   * unsaved-changes / autosave flow then persists it.
   *
   * Returns `true` when the caller may proceed to close the dialog / navigate,
   * and `false` when a save failed and an error/permission dialog is now showing
   * (the caller must leave it visible rather than dismiss it with `closeDialog`).
   */
  saveAfterImport: () => Promise<boolean>;
  /**
   * Close the open project. By default (`releaseLock` unset/true) the backend
   * best-effort releases a held SpeleoDB write lock first so the close can't strand
   * the server mutex (fix 1); the close dialog's "Keep lock" path passes
   * `{ releaseLock: false }` to retain it.
   */
  closeProject: (opts?: { releaseLock?: boolean }) => Promise<CommandResult>;
  refreshProjectState: () => Promise<void>;
  /**
   * Mark the project dirty (unsaved changes) without running a command. The DMP
   * import wizard applies sections via direct `import_sections` IPC calls that
   * bypass `executeCommand`, so they don't set `isDirty` themselves; the wizard
   * calls this after each successful batch so the imported sections register as
   * unsaved — powering autosave and the project-close guard (`handleCloseProject`).
   * (Window-close during the wizard is guarded separately, by `isImportInProgress`
   * in App.tsx.) (#110)
   */
  markDirty: () => void;
  /**
   * Fetch closure-adjusted geometry for the adjusted display mode (lazy, cached).
   * Resolves true on success / no-op, false only on a genuine fetch failure (so
   * the caller can fall back to the measured view rather than getting stuck).
   */
  loadAdjustedGeometry: () => Promise<boolean>;
  /** Check if a shot (by section_id + shot index) is frozen */
  isShotFrozen: (sectionId: number, shotIndex: number) => boolean;
  /** Structural-integrity issues found in the open project (empty = sound). */
  integrityIssues: string[];
  /** True once the user dismissed the integrity banner for the CURRENT issue set.
   * Suppresses the banner so a re-check after a save (or a failed autosave) can't
   * resurrect a banner the user explicitly dismissed; reset automatically when the
   * issue set changes (genuinely new issues re-show). Ignored while
   * `integritySaveBlocked` is true (a save-blocking project can't hide Repair). */
  integrityDismissed: boolean;
  /** True when the open project has unresolvable cross-references — the exact
   * condition that refuses both manual and auto-save. While true the integrity
   * banner stays shown and non-dismissable so the Repair entry point can't be
   * hidden on a project that can't be saved. */
  integritySaveBlocked: boolean;
  /** Whether the "Review & Repair" integrity dialog is open. Lifted here (not
   * App-local) so a blocked manual save can route the user straight to Repair
   * instead of showing a generic error window. */
  integrityDialogOpen: boolean;
  /** Open the Review & Repair integrity dialog. */
  openIntegrityDialog: () => void;
  /** Close the Review & Repair integrity dialog. */
  closeIntegrityDialog: () => void;
  /** Re-check the open project's structural integrity, updating integrityIssues. */
  checkProjectIntegrity: () => Promise<string[]>;
  /** Repair structural integrity; returns {fixed, remaining} and refreshes
   * integrityIssues from the true post-repair state (remaining>0 = detect-only
   * issues that need manual attention). */
  repairProjectIntegrity: () => Promise<IntegrityRepairResult>;
  /** Dismiss the integrity warning without repairing. */
  dismissIntegrityIssues: () => void;
  refreshWarnings: () => Promise<void>;
  resolveWarning: (sectionId: number, shotId: number, warningCode: string, comment?: string) => Promise<boolean>;
  unresolveWarning: (sectionId: number, shotId: number, warningCode: string) => Promise<void>;
  loadRecentProjects: () => Promise<void>;
  removeFromRecentProjects: (path: string) => Promise<CommandResult>;
  deleteProject: (path: string, deleteCollabSibling: boolean) => Promise<CommandResult>;
  mergeProjects: (sourcePaths: string[], outputPath: string, outputName: string, archiveOriginals: boolean) => Promise<CommandResult>;

  createSection: (data: SectionData) => Promise<CommandResult>;
  createSectionAndMerge: (
    data: SectionData,
    importStationIndex: number,
    targetStation: StationRef,
    closureStrategy: ClosureStrategy
  ) => Promise<CommandResult>;
  updateSection: (sectionId: SectionId, updates: SectionUpdates) => Promise<CommandResult>;
  deleteSection: (sectionId: SectionId) => Promise<CommandResult>;

  createShot: (sectionId: SectionId, shot: ShotData, afterShotId?: ShotId, beforeShotId?: ShotId) => Promise<CommandResult>;
  updateShot: (sectionId: SectionId, shotId: ShotId, updates: ShotUpdatesPatch) => Promise<CommandResult>;
  deleteShot: (sectionId: SectionId, shotId: ShotId) => Promise<CommandResult>;
  splitShotAtDistance: (sectionId: SectionId, shotId: ShotId, distance: number) => Promise<CommandResult>;
  deleteShotAndSplit: (sectionId: SectionId, shotId: ShotId, nameA: string, nameB: string) => Promise<CommandResult>;
  deleteLastShotAndSection: (sectionId: SectionId, shotId: ShotId) => Promise<CommandResult>;
  deleteShotRangeSplit: (sectionId: SectionId, startShotId: ShotId, endShotId: ShotId, nameA: string, nameB: string) => Promise<CommandResult>;
  deleteShotRangeShift: (sectionId: SectionId, startShotId: ShotId, endShotId: ShotId) => Promise<CommandResult>;
  setShotReference: (sectionId: SectionId, shotId: ShotId, isReference: boolean) => Promise<CommandResult>;

  mergeStations: (stationA: StationRef, stationB: StationRef) => Promise<CommandResult>;
  unmergeStations: (stationRef: StationRef) => Promise<CommandResult>;
  unmergeStationPair: (stationA: StationRef, stationB: StationRef) => Promise<CommandResult>;
  checkMergeConnectivity: (stationA: StationRef, stationB: StationRef) => Promise<ConnectivityResult>;
  createLoopClosure: (stationA: StationRef, stationB: StationRef, closureStrategy: ClosureStrategy) => Promise<CommandResult>;
  removeLoopClosure: (closureId: bigint) => Promise<CommandResult>;
  addMarker: (sectionId: number, shotId: number, position: number, markerType: MarkerType, color: string, text: string, direction: MarkerDirection | null) => Promise<CommandResult>;
  updateMarker: (markerId: bigint, updates: MarkerUpdates) => Promise<CommandResult>;
  deleteMarker: (markerId: bigint) => Promise<CommandResult>;
  updateStationComment: (stationRef: StationRef, comment: string | null) => Promise<CommandResult>;
  moveComment: (source: CommentHolder, target: CommentHolder, swap: boolean) => Promise<CommandResult>;
  setZeroDatum: (stationRef: StationRef, coordinates: GpsCoordinates, options?: { fitToBounds?: boolean }) => Promise<CommandResult>;
  removeZeroDatum: (stationRef: StationRef) => Promise<CommandResult>;
  moveSection: (sectionId: SectionId, offset: Coordinates) => Promise<CommandResult>;
  moveSegment: (sectionId: SectionId, offset: Coordinates) => Promise<CommandResult>;
  moveStation: (stationRef: StationRef, newPosition: Coordinates) => Promise<CommandResult>;
  getConnectedSectionIds: (sectionId: SectionId) => Promise<number[]>;
  getAnchoredSectionIds: () => Promise<number[]>;

  undo: () => Promise<CommandResult>;
  redo: () => Promise<CommandResult>;
  undoTo: (index: number) => Promise<CommandResult>;
  redoTo: (index: number) => Promise<CommandResult>;
  getUndoStack: () => Promise<UndoHistoryItem[]>;
  getRedoStack: () => Promise<UndoHistoryItem[]>;
  refreshUndoState: () => Promise<void>;

  // Batch execution - groups multiple commands into a single undo/redo unit
  executeBatch: (commands: BatchCommand[]) => Promise<CommandResult>;

  importDmp: (path: string) => Promise<DmpFile>;
  parseTml: (path: string) => Promise<TmlFile>;
  executeTmlImport: (path: string, fileHash?: string | null, includeExcluded?: boolean, commentsOnStations?: boolean) => Promise<CommandResult>;
  parseCompass: (path: string) => Promise<CompassFile>;
  executeCompassImport: (path: string, fileHash?: string | null, includeExcluded?: boolean, applyCorrections?: boolean, commentsOnStations?: boolean) => Promise<CommandResult>;
  parseWalls: (path: string) => Promise<WallsFile>;
  executeWallsImport: (path: string, fileHash?: string | null, commentsOnStations?: boolean, includeSplays?: boolean, selectedSurveys?: number[] | null) => Promise<CommandResult>;
  // Per-project settings (#57): name + validation thresholds, stored in the .stalx
  updateProjectSettings: (name: string, validation: Config['validation']) => Promise<CommandResult>;
  // Collaboration mode
  setCollabEnabled: (enabled: boolean) => Promise<CommandResult>;
  setCollabEnabledForPath: (path: string, enabled: boolean) => Promise<CommandResult>;
  /** Rename a recent project's display name by path (project need not be open). */
  setProjectNameForPath: (path: string, name: string) => Promise<CommandResult>;
  getCollabState: () => Promise<CollabState | null>;
  detectCollabChanges: () => Promise<CollabChangeSet | null>;
  applyCollabChanges: () => Promise<CommandResult>;
  checkCollabSibling: () => Promise<string | null>;
  parseGpx: (path: string) => Promise<GpxFile>;
  executeGpxImport: (path: string, fileHash: string | null, importWaypoints: boolean, importTracks: boolean, importRoutes: boolean, sectionPlacements: GpxSectionPlacement[], depthInverted: boolean) => Promise<CommandResult>;
  calculateFileHash: (path: string) => Promise<string>;
  checkDuplicateImport: (hash: string) => Promise<boolean>;
  trackImportedFile: (hash: string) => Promise<void>;
  checkSectionHashes: (hashes: string[]) => Promise<boolean[]>;
  importSections: (sections: ImportSectionConfig[], fileHash?: string | null, sectionHashes?: string[] | null) => Promise<CommandResult>;
  suggestImportPlacement: (shots: ShotData[]) => Promise<PlacementSuggestion[]>;
  exportCsv: (path: string) => Promise<void>;
  exportJson: (path: string) => Promise<void>;
  exportTml: (path: string) => Promise<void>;
  exportImage: (path: string, options: ExportImageOptions) => Promise<void>;
}

/**
 * Detect partial failure in multi-step undo/redo.
 * Backend returns "Undo failed at step N of M: reason" (or "Redo failed...")
 * when some steps succeeded before the failure. If step > 1, state was mutated.
 */
function isPartialMultiStepFailure(error: string | null): boolean {
  if (!error) return false;
  const match = error.match(/failed at step (\d+) of (\d+)/);
  if (!match || !match[1]) return false;
  return parseInt(match[1], 10) > 1;
}

/**
 * Tracks in-flight `loadProject(path)` calls so concurrent requests for the
 * same path share one execution. Each entry is removed when the underlying
 * promise settles. See loadProject for rationale.
 */
const inFlightLoads = new Map<string, Promise<{ success: boolean }>>();

export const useProjectStore = create<ProjectStore>((set, get) => {
  /** Shared helper: invoke IPC command, refresh state on success, mark dirty. */
  const executeCommand = async (
    command: string,
    args: Record<string, unknown> | undefined,
    meta: { purpose: string; fields?: Record<string, unknown> }
  ): Promise<CommandResult> => {
    try {
      const result = await invokeIpc<CommandResult>(command, args, meta);
      if (result.success) {
        await get().refreshProjectState();
        set({ isDirty: true });
        // #204: mirror the backend's mark_remote_dirty so the SpeleoDB action shell
        // knows there are un-pushed edits (drives the button's push&release-with-
        // comment flow). No-op for a local project (no link).
        useSpeleoStore.getState().markDirty();
        // A command can RESOLVE (or surface) an integrity issue — e.g. removing a
        // drifted loop closure clears its banner entry — so re-check, mirroring
        // executeUndoRedo. refreshProjectState doesn't re-check, and the next save
        // may be an autosave (which skips its own post-save re-check to avoid churn),
        // so without this the banner/dialog can keep showing an already-fixed issue
        // until a manual save or reload.
        await get().checkProjectIntegrity();
      } else {
        // executeCommand used to be SILENT on a rejected CommandResult (no refresh,
        // no feedback). That left optimistic UI desynced from backend truth — the
        // #191 read-only edit gate's "shots vanish on drag" bug: a committed drag
        // preview stayed rendered because the success-only refresh never fired.
        // ALWAYS re-sync the view to backend truth, discarding the phantom edit.
        await get().refreshProjectState();
        // Surface a read-only rejection (EDIT_DISABLED) as a single friendly notice
        // across ALL edit paths (drag, dialogs, shortcuts) — a blocked drag was
        // otherwise a silent no-op. Other failures keep their existing handling
        // (dialogs show an inline error), so validation errors aren't double-toasted.
        // Full edit-state UX (suppress this, offer "Start editing") is owned by #204.
        // #202: a re-verify divergence (lost lock / moved tip) routes to the
        // divergence prompt instead of the plain read-only notice - the dialog is
        // the feedback, and the backend already dropped its edit gate.
        if (isReverifyDivergedError(result.error)) {
          useSpeleoStore.getState().notifyReverifyDivergence();
        } else if (isEditDisabledError(result.error)) {
          useUIStore.getState().showToast(READ_ONLY_EDIT_NOTICE, 'edit-disabled');
        }
      }
      return result;
    } catch (e) {
      // A genuine IPC/transport rejection (rare — distinct from a returned
      // {success:false}) would otherwise skip the re-sync above and strand an
      // optimistic edit (a committed drag preview) hidden forever — the same
      // "vanishing shots" failure. Re-sync the view first (the re-render drains
      // the scheduled drag-preview restore), THEN re-throw so every caller's
      // existing error handling (dialog catch blocks, etc.) is unchanged.
      await get().refreshProjectState();
      throw e;
    }
  };

  /** Shared helper for undo/redo operations: invoke, toast on failure, always refresh. */
  const executeUndoRedo = async (
    command: string,
    args: Record<string, unknown> | undefined,
    meta: { purpose: string; fields?: Record<string, unknown> },
    toastId: string,
    checkPartialFailure: boolean,
  ): Promise<CommandResult> => {
    let result: CommandResult;
    try {
      result = await invokeIpc<CommandResult>(command, args, meta);
      if (result.success) {
        set({ isDirty: true });
        // #204: an undo/redo mutates the remote project too - mirror the backend
        // dirty bit into the SpeleoDB store (no-op for a local project).
        useSpeleoStore.getState().markDirty();
      } else {
        if (checkPartialFailure && isPartialMultiStepFailure(result.error)) {
          set({ isDirty: true });
          useSpeleoStore.getState().markDirty();
        }
        // A read-only rejection (EDIT_DISABLED) gets the same friendly notice +
        // dedup key as executeCommand, not the raw coded string — mashing Ctrl+Z
        // on a read-only remote project shows one tidy toast, not "EDIT_DISABLED:…".
        // A #202 re-verify divergence routes to the divergence prompt instead.
        if (isReverifyDivergedError(result.error)) {
          useSpeleoStore.getState().notifyReverifyDivergence();
        } else if (isEditDisabledError(result.error)) {
          useUIStore.getState().showToast(READ_ONLY_EDIT_NOTICE, 'edit-disabled');
        } else {
          useUIStore.getState().showToast(result.error ?? 'Unknown error', toastId);
        }
      }
    } catch (e) {
      result = emptyCommandResult(false, String(e));
      useUIStore.getState().showToast(result.error ?? 'Unknown error', toastId);
    } finally {
      await get().refreshProjectState();
      // Undo/redo can change which cross-references resolve — e.g. undoing a
      // Repair restores the dropped dangling refs, redoing a delete removes a
      // station an offset anchored on — so re-check integrity, otherwise the
      // banner and save-block go stale (showing clean while the project isn't).
      await get().checkProjectIntegrity();
    }
    return result;
  };

  return {
  // Initial state
  project: null,
  databaseOpen: false,
  stations: [],
  surveyData: null,
  stationsAdjusted: null,
  surveyDataAdjusted: null,
  adjustedLoading: false,
  stats: null,
  frozenShots: [],
  warnings: [],
  recentProjects: [],
  integrityIssues: [],
  integrityDismissed: false,
  integritySaveBlocked: false,
  integrityDialogOpen: false,
  config: null,
  loading: { ...initialLoadingState },
  error: null,
  canUndo: false,
  canRedo: false,
  isDirty: false,
  isClosing: false,

  // Config
  loadConfig: async () => {
    try {
      const config = await invokeIpc<Config>('get_config', undefined, {
        purpose: 'load user configuration',
      });
      set({ config });
    } catch (e) {
      console.error('Failed to load config:', e);
    }
  },

  updateConfig: async (config: Config) => {
    try {
      // Rust command returns Result<(), String> - success returns nothing, error throws
      await invokeIpc('update_config', { config }, { purpose: 'persist user settings' });
      // Re-fetch config to get backend-normalized values (e.g., zoom_step clamped to valid range)
      await get().loadConfig();
      // The closure-adjustment sigmas feed the adjusted solve, so a settings change
      // can change the adjusted geometry. Drop the cached set; the map re-fetches
      // lazily (the backend memo is keyed on the config, so a stale config would
      // otherwise keep rendering the old weighted solve until an unrelated edit).
      set({ stationsAdjusted: null, surveyDataAdjusted: null });
      return emptyCommandResult(true, null);
    } catch (e) {
      return emptyCommandResult(false, String(e));
    }
  },

  setConfig: (config: Config) => {
    // Replacing the config (e.g. Settings ▸ Advanced "Reload from disk") can change
    // the closure-adjustment sigmas, so drop the cached adjusted geometry here too —
    // this is the shared config-replacement path that `updateConfig` doesn't use.
    set({ config, stationsAdjusted: null, surveyDataAdjusted: null });
  },

  // Loading state actions
  startLoading: (projectName: string, projectPath: string) => {
    clientOpIdCounter += 1;
    const newClientOpId = clientOpIdCounter;
    set({
      loading: {
        ...initialLoadingState,
        loadingId: 0n, // Will be set from first progress event or result
        clientOpId: newClientOpId,
        isLoading: true,
        projectName,
        projectPath,
        phase: 'opening_database',
        lastEventTime: Date.now(),
      },
      error: null, // Clear any stale error state
    });
    return newClientOpId;
  },

  setLoadingProgress: (progress: Partial<LoadingState>) =>
    set((state) => ({
      loading: {
        ...state.loading,
        ...progress,
        lastEventTime: Date.now(),
      },
    })),

  resetLoading: () =>
    set((state) => ({
      loading: { ...initialLoadingState, clientOpId: state.loading.clientOpId },
    })),

  setLoadingError: (message: string, details?: string) =>
    set((state) => ({
      loading: {
        ...state.loading,
        isLoading: true, // Keep modal open for error display
        error: { message, details },
      },
    })),

  cancelLoading: async () => {
    // Safe to call even when no load is active - backend is idempotent
    try {
      await invokeIpc('cancel_loading', undefined, {
        purpose: 'cancel project loading',
      });
    } catch {
      // Ignore errors - might be called when no load active
    }
    // Always reset loading state
    get().resetLoading();
  },

  // Load project using the consolidated load_project_full command
  //
  // Concurrent calls for the same path share one execution. Without this,
  // a fast double-click on a SystemCard (or React StrictMode dev re-mount)
  // fires loadProject multiple times; each call decodes ~600 KB of shot
  // floats + 350 KB of station coords and rebuilds 240 k MapLibre
  // features in two workers, even though all but the last result are
  // discarded by the clientOpId race check. The thrown-away allocations
  // churn JSC's bmalloc slabs and leave committed (but unused) memory
  // behind. The dedup map ensures the heavy path runs exactly once.
  loadProject: (path: string) => {
    const existing = inFlightLoads.get(path);
    if (existing) return existing;

    // Drop in-flight entries for OTHER paths: this load is about to
    // call `cancelLoading()` which will bump the clientOpId, dooming
    // any in-flight load for a different path to return
    // `{success: false}` from its clientOpId check.  If we kept those
    // entries around, a user re-click for one of those paths would
    // receive the now-doomed promise instead of starting a fresh load.
    // The same-path entry below would have short-circuited above; this
    // only clears stale ones.
    for (const key of [...inFlightLoads.keys()]) {
      if (key !== path) inFlightLoads.delete(key);
    }

    const promise: Promise<{ success: boolean }> = (async () => {
      // [perf-debug] Trace each post-IPC step distinctly so we know whether
      // the visible "stuck progress bar" freeze is JSON.parse on the IPC
      // resolution, the Zustand set, the view switch / MapCanvas mount, or
      // something else. Each checkpoint is logged to the MemoryProbe panel.
      const { recordPerfTiming } = await import('../utils/memoryProbe');
      const tStart = performance.now();

      // Clear GlobalView geometry cache to cancel pending loads and free memory
      useGlobalViewStore.getState().clearGeometryCache();

      // ALWAYS cancel first - NO isLoading guard
      // cancel_loading is idempotent on backend
      await get().cancelLoading();

      // Use cross-platform path splitting for project name extraction
      const initialName = extractFilename(path, /\.stalx$/i) || 'Project';
      const clientOpId = get().startLoading(initialName, path);

      try {
        const tIpcStart = performance.now();
        // Binary IPC: msgpack envelope + typed-array byte buffers for shots
        // and stations. The decoder reconstructs the same shape as the legacy
        // `load_project_full` JSON response so the rest of the load pipeline
        // doesn't need to change. See utils/loadProjectBinary.ts.
        const { loadProjectFullBinary } = await import('../utils/loadProjectBinary');
        const result = await loadProjectFullBinary(path);
        // The await above includes both the Rust handler runtime AND the
        // JS-side msgpack decode + typed-array materialization.
        const tIpcDone = performance.now();
        recordPerfTiming('load_project_full IPC+parse', tIpcDone - tIpcStart);

        // Check clientOpId hasn't changed (cancelled/new load started)
        if (get().loading.clientOpId !== clientOpId) {
          clearProjectActionSource();
          return { success: false };
        }

        // Count the open with its entry-point source now that the load has
        // actually committed (past the cancellation check). loadProject uses the
        // raw binary IPC (untracked by invokeIpc), so the command event is emitted
        // here; counting before this point over-reported superseded/cancelled
        // loads. The source is still armed (setView clears it below). Deduped
        // same-path calls returned before the async body ran, so this fires once
        // per actual load.
        trackCommand('load_project_full', peekProjectActionSource());

        // Set backend loadingId from result for event correlation
        get().setLoadingProgress({ loadingId: result.loadingId });

        // Update loading title with actual project name from result
        if (result.project.name !== initialName) {
          get().setLoadingProgress({ projectName: result.project.name });
        }

        // Apply ALL state from single response
        const tSetStart = performance.now();
        set({
          project: result.project,
          stations: result.stations,
          surveyData: result.surveyData,
          // Adjusted geometry is lazily (re)fetched by the map when the adjusted
          // mode is active; invalidate any stale set from the previous project.
          stationsAdjusted: null,
          surveyDataAdjusted: null,
          stats: result.stats,
          warnings: result.warnings,
          // Populate from the load result so the resolve flow's frozen-shot gating
          // is correct immediately, not stale from the previous project until the
          // first command triggers refreshProjectState (issue #57 / Codex).
          frozenShots: result.frozen_shots,
          // A loaded project is backed by its on-disk database.
          databaseOpen: true,
          isDirty: false,
          // Reset integrity state on project switch so a dismissal from the
          // previous project can never carry over and suppress this one's banner.
          // The post-load integrity-check effect in App.tsx (keyed on
          // project.file_path + loading.isLoading) repopulates it for the new
          // project once this load settles.
          integrityIssues: [],
          integrityDismissed: false,
          integritySaveBlocked: false,
          integrityDialogOpen: false,
        });
        const tSetDone = performance.now();
        recordPerfTiming('store set(project+stations)', tSetDone - tSetStart);

        // Apply display settings to UI store (merge, don't overwrite)
        useUIStore.getState().applyDisplaySettings(result.displaySettings);
        useUIStore.getState().triggerFitToBounds();
        // Reset the Details panel to the project-level view on open (#54): a
        // stale shot/station/section selection from a previous project/session
        // must not carry over.
        useUIStore.getState().clearSelection();
        // Same hazard for tool state (#275 B1): an armed move-comment pickup or
        // merge station from the previous project would resolve against this
        // project's ids and mutate the wrong entities on the next click.
        // Ordering constraint: this must stay in the SUCCESS path, after the
        // authoritative set() above committed the new project. On a failed
        // load the old project is still displayed, so its armed tool refs are
        // still valid and must not be cleared.
        useUIStore.getState().resetToolTransientsForProjectSwitch();
        const tViewStart = performance.now();
        useUIStore.getState().setView('single');
        const tViewDone = performance.now();
        recordPerfTiming('setView(single)', tViewDone - tViewStart);

        // Update recent projects list
        await get().loadRecentProjects();

        get().resetLoading();
        await get().refreshUndoState();
        recordPerfTiming('loadProject total', performance.now() - tStart);

        // #204: seed the SpeleoDB action-shell edit-state from the freshly-loaded
        // project (is_remote / edit gate / permission / lock holder). Best-effort
        // and non-blocking - a local project resolves to `link = null` (shell
        // hidden); a remote one seeds read-only/editing. Covers reopen-from-recents
        // (where the browser's optimistic setLinkOnOpen never ran) and the
        // fast-forward reload path (which routes back through loadProject).
        void useSpeleoStore.getState().refreshLinkStatus();
        // Also refresh who we are, so the action button can tell our own held lock
        // from someone else's (otherwise a reopen-from-recents has no session and
        // every held lock reads as "locked by other").
        void useSpeleoStore.getState().refreshSession();

        // Collaboration mode (non-blocking, best-effort): if the project
        // has collab on AND the sibling .tml was edited externally
        // (Ariane), offer to reconcile.  When collab is off but a
        // companion .tml exists, warn the user that Ariane edits won't
        // flow in and saves won't update it.  Both paths fire
        // asynchronously so the load returns immediately, and both
        // re-check `clientOpId` / `loading` state inside `.then` so a
        // user who navigated away (closeProject, switched projects)
        // before the IPC resolved doesn't get a dialog or toast for a
        // project that's no longer open.
        const detectOpId = clientOpId;
        const isStillCurrentLoad = (): boolean => {
          const state = get();
          return (
            state.project !== null &&
            state.loading.clientOpId === detectOpId
          );
        };
        if (result.project.collab?.enabled) {
          void get()
            .detectCollabChanges()
            .then((cs) => {
              if (!isStillCurrentLoad()) return;
              if (
                cs &&
                cs.shots_added.length +
                  cs.shots_edited.length +
                  cs.shots_removed.length +
                  cs.sections_added.length +
                  cs.section_meta_changed.length >
                  0
              ) {
                useUIStore.getState().openDialog('collab-review', { changeSet: cs });
              }
            })
            .catch(() => {
              /* detection is best-effort; ignore errors */
            });
        } else {
          void get()
            .checkCollabSibling()
            .then((siblingName) => {
              if (!isStillCurrentLoad()) return;
              if (siblingName) {
                useUIStore
                  .getState()
                  .showToast(
                    i18n.t('dialogs.collabReview.siblingWarning', { siblingName }),
                    'collab-sibling-warning'
                  );
              }
            })
            .catch(() => {
              /* best-effort; ignore errors */
            });
        }

        return { success: true };
      } catch (e) {
        const error = parseLoadError(e);

        // Handle ALREADY_OPEN specially - not an error, just switch view
        if (error.code === 'ALREADY_OPEN') {
          useUIStore.getState().setView('single');
          get().resetLoading();
          await get().refreshUndoState();
          return { success: true };
        }

        // Handle PERMISSION - folder-access guidance, not a corruption/error.
        // Checked before CORRUPT so a denial doesn't masquerade as corruption.
        if (error.code === 'PERMISSION') {
          get().resetLoading();
          useUIStore.getState().openDialog('save-permission', {
            detail: error.message,
            context: 'open',
          });
          clearProjectActionSource();
          return { success: false };
        }

        // Handle CORRUPT - open recovery dialog instead of generic error
        if (error.code === 'CORRUPT' && error.details) {
          get().resetLoading();
          useUIStore.getState().openDialog('corruption-recovery', {
            corruptPath: error.details,
            errorMessage: error.message,
          });
          clearProjectActionSource();
          return { success: false };
        }

        // Only set error if this is still the current load operation
        if (get().loading.clientOpId === clientOpId) {
          get().setLoadingError(error.message, error.details);
        }
        clearProjectActionSource();
        return { success: false };
      }
    })();

    inFlightLoads.set(path, promise);
    void promise.finally(() => {
      if (inFlightLoads.get(path) === promise) inFlightLoads.delete(path);
    });
    return promise;
  },

  // Project operations - uses consolidated create_project_full command
  createProject: async (name: string, projectLocation?: string, options?: CreateProjectOptions) => {
    // ALWAYS cancel first - NO isLoading guard
    await get().cancelLoading();

    const clientOpId = get().startLoading(name, '');
    // A deferred initial save reserves file_path but leaves the backend database
    // unopened, so DB-backed actions (e.g. ignoring a warning) can't run yet.
    const deferred = options?.deferInitialSave ?? !options?.savePath;

    try {
      // Attribute the create to its entry point (peek: a DMP import-into-new
      // flow also fires import_sections, which peeks the same source).
      const actionSource = peekProjectActionSource();
      if (actionSource) setSource(actionSource);
      const result = await invokeIpc<CreateProjectFullResult>(
        'create_project_full',
        {
          name,
          projectLocation,
          saveDirectory: options?.saveDirectory,
          savePath: options?.savePath,
          deferInitialSave: deferred,
        },
        { purpose: 'create new project workspace', fields: { name } }
      );

      // Check clientOpId hasn't changed
      if (get().loading.clientOpId !== clientOpId) {
        // Superseded/cancelled — drop the entry-point source so it can't
        // mis-attribute a later, unrelated project command (it has no auto-reset;
        // only a completed flow (setView) or this clear drops it). Mirrors
        // loadProject's clear on every early-return.
        clearProjectActionSource();
        return emptyCommandResult(false, 'Cancelled');
      }

      // Set backend loadingId from result for event correlation
      get().setLoadingProgress({ loadingId: result.loadingId });

      // Apply project state from result. Fresh project has no shots /
      // stations yet, so `surveyData` starts as an empty SoA.
      set({
        project: result.project,
        // A deferred new project has no on-disk database until the first save.
        databaseOpen: !deferred,
        stations: [],
        stationsAdjusted: null,
        surveyDataAdjusted: null,
        surveyData: emptySurveyData(),
        stats: null,
        frozenShots: [],
        warnings: [],
        isDirty: false,
        // Reset integrity state on project switch (see loadProject).
        integrityIssues: [],
        integrityDismissed: false,
        integritySaveBlocked: false,
        integrityDialogOpen: false,
      });

      // Apply display settings from backend (defaults for new project)
      useUIStore.getState().applyDisplaySettings(result.displaySettings);
      // Reset the Details panel to the project-level view on open (#54).
      useUIStore.getState().clearSelection();
      // Stale tool state must not carry into the new project either (#275 B1).
      useUIStore.getState().resetToolTransientsForProjectSwitch();
      useUIStore.getState().setView('single');
      await get().loadRecentProjects();
      get().resetLoading();

      return emptyCommandResult(true, null);
    } catch (e) {
      // Create failed — drop the entry-point source (no auto-reset) so it can't
      // mis-attribute a later, unrelated project command. Mirrors loadProject.
      clearProjectActionSource();
      const error = parseLoadError(e);
      if (get().loading.clientOpId === clientOpId) {
        get().setLoadingError(error.message, error.details);
      }
      return emptyCommandResult(false, error.message);
    }
  },

  saveProject: async (path?: string, isAutosave?: boolean) => {
    // The Rust save path heals duplicate shot UUIDs / orphan section offsets /
    // non-finite geometry in memory before persisting, which mutates the backend
    // project — so afterward the frontend's mirror can be stale. Those heals only
    // ever fire on a project the integrity check already flags, so capture that here
    // as a cheap signal to refresh below, avoiding a get_project_state on every
    // clean save / autosave.
    const hadIntegrityIssues = get().integrityIssues.length > 0;
    const result = await invokeIpc<CommandResult>(
      'save_project',
      { path, isAutosave: isAutosave ?? false },
      { purpose: 'persist project changes', fields: { path } }
    );
    if (result.success) {
      // Update file_path if the backend returned it (first save / save-as)
      if (result.saved_path) {
        const project = get().project;
        if (project) {
          set({ project: { ...project, file_path: result.saved_path } });
          // On the first save / Save As the backend derives the sibling
          // name from the final .stalx stem (e.g. `cave.stalx` →
          // `cave.tml`).  The in-memory `project.collab.sibling_tml_relative`
          // is still the pre-save placeholder (e.g. `collaboration.tml`)
          // until we refresh — the Project Details panel would otherwise
          // mislead the user about which companion file was actually
          // written.  Refresh the collab state when collab is on.
          if (project.collab?.enabled) {
            try {
              const fresh = await get().getCollabState();
              const current = get().project;
              if (fresh && current) {
                set({ project: { ...current, collab: fresh } });
              }
            } catch {
              /* best-effort refresh; ignore IPC errors */
            }
          }
        }
      }
      // Lightweight: just refresh undo/redo availability
      await get().refreshUndoState();
      // The first successful save opens the on-disk database for a deferred project
      // — UNLESS the backend saved the file but couldn't reopen the DB (it clears
      // the handle and flags DATABASE_NOT_REOPENED). In that case keep
      // databaseOpen=false so DB-backed actions stay disabled (issue #57 / Codex).
      const dbNotReopened = result.warnings?.some((w) => w.code === 'DATABASE_NOT_REOPENED') ?? false;
      set({ isDirty: false, databaseOpen: !dbNotReopened });
      // If the save healed the project in memory (re-minted duplicate UUIDs,
      // dropped orphan section offsets, sanitized non-finite geometry), the
      // frontend's shots/stations are now stale vs the saved file — refresh so the
      // rendered state matches. Gated on hadIntegrityIssues so a clean save/autosave
      // skips the extra get_project_state fetch and re-render.
      if (hadIntegrityIssues) {
        await get().refreshProjectState();
      }
      // Don't let geometric corruption persist silently: re-check after save so
      // drifted merges surface in the integrity banner this session, not only on
      // the next load. Skip on a clean autosave to avoid churn — BUT still re-check
      // when the project entered the save flagged (hadIntegrityIssues), because the
      // save path auto-heals some issues (duplicate UUIDs / orphan offsets) on
      // autosave too; without this the banner/dialog would stay stale after an
      // autosave silently cleared them.
      if (!isAutosave || hadIntegrityIssues) {
        void get().checkProjectIntegrity();
      }
    } else {
      const permission = isPermissionError(result.error);
      // Never surface the raw "FS_PERMISSION_DENIED:" marker in error state
      // (a status indicator/toast may render it, e.g. on autosave).
      set({
        error: permission
          ? 'Folder access denied — check Privacy & Security settings'
          : (result.error ?? 'Failed to save project'),
      });
      // A filesystem permission denial (e.g. macOS forgot folder access after
      // an update) needs actionable guidance, not a raw error. Skip on autosave
      // so we don't reopen the dialog on every debounced retry.
      if (permission && !isAutosave) {
        useUIStore.getState().openDialog('save-permission', {
          detail: result.error,
          context: 'save',
        });
      }
      // A save is also blocked when the project has unresolvable cross-references;
      // re-check so they surface in the integrity banner (the repair entry point)
      // and the user can Repair and re-save. Awaited (not fire-and-forget) so
      // `integritySaveBlocked` is up to date by the time saveProject resolves —
      // the auto-save hook reads it to decide whether to pause, and
      // surfaceSaveFailure reads it to route a blocked save to Review & Repair.
      // (On a permission error this is a harmless no-op: the project is
      // structurally fine, so integritySaveBlocked stays false.)
      await get().checkProjectIntegrity();
    }
    return result;
  },

  markDirty: () => set({ isDirty: true }),

  saveAfterImport: async (): Promise<boolean> => {
    // Import into an existing project: just mark dirty so we don't silently
    // commit the user's unrelated pending edits. The existing project already
    // has a file + Recent Projects entry; the normal unsaved-changes / autosave
    // flow persists the import, and the close-guard prevents silent loss.
    if (useUIStore.getState().importContext !== 'new-project') {
      set({ isDirty: true });
      // #204/fix 8: an import into an existing remote project is an un-pushed edit -
      // mirror the SpeleoDB dirty bit so the status button offers push (no-op for a
      // local project). The import command usually already marks it via
      // executeCommand; this keeps the two dirty flags in lockstep on every path.
      useSpeleoStore.getState().markDirty();
      return true;
    }
    // New project: the initial save is deferred, so the import is invisible until
    // a real save writes the .stalx and adds it to Recent Projects. Do that now.
    // (The backend import_sections command also resolves this as a durable
    // safety net; this gives the immediate, complete save with correct dirty state.)
    let result: CommandResult;
    try {
      result = await get().saveProject();
    } catch (e) {
      // A reject (e.g. IPC transport failure) means saveProject never ran its
      // integrity recheck, so `integritySaveBlocked` may be stale — routing through
      // surfaceSaveFailure could wrongly open Review & Repair. Show the generic
      // save error directly instead, and keep the project dirty (close-guarded).
      console.error('[import] save after import failed:', e);
      set({ isDirty: true });
      useUIStore.getState().openDialog('error', {
        message: e instanceof Error ? e.message : String(e),
        title: 'Save Failed',
      });
      return false;
    }
    if (!result.success) {
      // The import isn't on disk: reflect the unsaved state (not a phantom
      // "saved") and surface the actionable error. Signal the caller NOT to close
      // the dialog, so it doesn't dismiss the error/permission guidance.
      set({ isDirty: true });
      surfaceSaveFailure(
        result.error ?? 'Failed to save the imported project.',
        useUIStore.getState().openDialog,
      );
      return false;
    }
    return true;
  },

  closeProject: async (opts) => {
    // Terminate the feature-build worker on close so its JSC heap (and any
    // bmalloc slabs it committed during the last load) goes back to the OS.
    // The worker is a singleton lazily recreated on the next project open;
    // start-up cost is small (~50 ms) compared to keeping ~1 GB of committed
    // slabs alive across the global view. The terminate() helper rejects
    // any pending build promises with a clear error.
    try {
      const { terminateFeaturesWorker } = await import('../components/Map/surveyLayer/featuresWorkerClient');
      terminateFeaturesWorker();
    } catch {
      // Best-effort; the worker module might not have been loaded if the
      // user never opened a project (unlikely path but harmless).
    }

    // Guard against concurrent close operations (rapid Back clicks or double Discard)
    if (get().isClosing) {
      debugLog('[projectStore] closeProject ignored - already closing');
      return emptyCommandResult(false, 'Already closing');
    }

    set({ isClosing: true });
    try {
      const result = await invokeIpc<CommandResult>(
        'close_project',
        // Default: release a held SpeleoDB lock so the close doesn't strand it
        // (fix 1). The keep-lock close path passes { releaseLock: false }.
        { releaseLock: opts?.releaseLock ?? true },
        { purpose: 'close project and release resources' },
      );
      if (result.success) {
        set({ project: null, databaseOpen: false, stations: [], surveyData: null, stationsAdjusted: null, surveyDataAdjusted: null, stats: null, frozenShots: [], isDirty: false, integrityIssues: [], integrityDismissed: false, integritySaveBlocked: false, integrityDialogOpen: false });
        // Closing a project bypasses the setView() cleanup funnel, so drop the gas
        // sub-mode (#38) here too — a stale overlay/panel must not outlive the project.
        useUIStore.getState().exitGasPlan();
        // #204: drop the SpeleoDB action-shell edit-state - no project is open.
        useSpeleoStore.getState().clearLink();
      }
      return result;
    } finally {
      set({ isClosing: false });
    }
  },

  isShotFrozen: (sectionId: number, shotIndex: number) => {
    return get().frozenShots.some(
      (f) => f.section_id === sectionId && f.shot_index === shotIndex
    );
  },

  refreshProjectState: async () => {
    try {
      debugLog('[projectStore] refreshProjectState: invoking get_project_state...');
      const state = await invokeIpc<ProjectState | null>('get_project_state', undefined, {
        purpose: 'refresh project state in UI',
      });
      debugLog('[projectStore] refreshProjectState: got state, sections:', state?.project?.sections?.length);
      if (state) {
        // Build the SoA layout alongside the legacy shapes for the
        // duration of the Phase-A migration. After Phase A.6 this is
        // the only shape stored; project loses .sections[i].shots.
        const surveyData = surveyDataFromLegacy(state.project.sections, state.stations);
        set({
          project: state.project,
          stations: state.stations,
          surveyData,
          // Any edit changes measured geometry → the cached adjusted view is stale.
          // Drop it; the map re-fetches lazily if the adjusted mode is active.
          stationsAdjusted: null,
          surveyDataAdjusted: null,
          stats: state.stats,
          frozenShots: state.frozen_shots,
        });
      }
      debugLog('[projectStore] refreshProjectState: calling refreshUndoState...');
      await get().refreshUndoState();
      // Also refresh warnings
      await get().refreshWarnings();
      debugLog('[projectStore] refreshProjectState: done');
    } catch (e) {
      console.error('Failed to refresh project state:', e);
    }
  },

  refreshWarnings: async () => {
    try {
      const warnings = await invokeIpc<ProjectWarning[]>('get_project_warnings', undefined, {
        purpose: 'refresh validation warnings',
      });
      set({ warnings });
    } catch (e) {
      console.error('Failed to refresh warnings:', e);
      set({ warnings: [] });
    }
  },

  resolveWarning: async (sectionId: number, shotId: number, warningCode: string, comment?: string): Promise<boolean> => {
    try {
      await invokeIpc(
        'resolve_warning',
        { sectionId, shotId, warningCode, comment },
        { purpose: 'acknowledge validation warning', fields: { sectionId, shotId, warningCode } }
      );
      await get().refreshWarnings();
      return true;
    } catch (e) {
      // Surface failure to the caller (e.g. the shot no longer exists) so the
      // resolve flow doesn't report a suppression that never persisted (#57).
      console.error('Failed to resolve warning:', e);
      return false;
    }
  },

  unresolveWarning: async (sectionId: number, shotId: number, warningCode: string) => {
    try {
      await invokeIpc(
        'unresolve_warning',
        { sectionId, shotId, warningCode },
        { purpose: 'reopen validation warning', fields: { sectionId, shotId, warningCode } }
      );
      await get().refreshWarnings();
    } catch (e) {
      console.error('Failed to unresolve warning:', e);
    }
  },

  loadRecentProjects: async () => {
    try {
      // [perf] Hot path on every navigation to GlobalView. Surfaced in the
      // memory probe panel to confirm the SQL-aggregate fast path stays fast.
      const t0 = performance.now();
      const recent = await invokeIpc<ProjectMetadata[]>('list_recent_projects', undefined, {
        purpose: 'load recent projects list',
      });
      const ms = performance.now() - t0;
      const { recordPerfTiming } = await import('../utils/memoryProbe');
      recordPerfTiming('list_recent_projects IPC', ms);
      set({ recentProjects: recent });
    } catch (e) {
      console.error('Failed to load recent projects:', e);
    }
  },

  checkProjectIntegrity: async () => {
    // The check is fire-and-forget (the post-load App effect, the after-every-
    // command wrapper, a failed save) and the backend scans whatever project is
    // currently loaded with no path argument. Correlate the result with the
    // project that was active when it STARTED: if the user switches or closes
    // projects while the check is in flight, a late-resolving result must not
    // overwrite the new project's freshly-reset banner/save-block state.
    // clientOpId bumps on every load (catches a switch); file_path catches a
    // close (project → null).
    const pathAtStart = get().project?.file_path ?? null;
    const opAtStart = get().loading.clientOpId;
    const mySeq = ++integrityCheckSeq;
    try {
      const result = await invokeIpc<IntegrityCheckResult>('check_project_integrity', undefined, {
        purpose: 'check project structural integrity after load',
      });
      const cur = get();
      if (
        mySeq !== integrityCheckSeq ||
        cur.project === null ||
        (cur.project.file_path ?? null) !== pathAtStart ||
        cur.loading.clientOpId !== opAtStart
      ) {
        // Drop this result if it has been superseded. Two ways:
        // 1. A NEWER check was issued after this one started (last-issued-wins) —
        //    so an earlier check can't clobber a fresher result even on the SAME
        //    project (e.g. a save's fire-and-forget check racing a Repair's check,
        //    which the project-identity guard below can't tell apart).
        // 2. The project switched/closed mid-check. The explicit project===null
        //    check is needed for the UNSAVED case: file_path stays null and
        //    closeProject doesn't bump clientOpId, so without it a late result would
        //    repopulate the banner with no project loaded (leaking onto the global view).
        return [];
      }
      const issues = result.messages;
      set((state) => {
        // Keep a dismissal sticky across re-checks (e.g. after every save) as long
        // as the issue SET is unchanged; reset it when the set genuinely changes so
        // newly-appeared issues re-show the banner. Compare order-independently:
        // the backend's message order isn't stable (orphan offset/comment messages
        // come from HashMap iteration), so a positional compare could spuriously
        // reset a dismissal — and thus resurface a banner — on a mere reorder.
        const prev = [...state.integrityIssues].sort();
        const next = [...issues].sort();
        const unchanged =
          prev.length === next.length && prev.every((v, i) => v === next[i]);
        return {
          integrityIssues: issues,
          integritySaveBlocked: result.save_blocked,
          integrityDismissed: unchanged ? state.integrityDismissed : false,
          // Keep the Review & Repair dialog flag consistent with what's actually
          // rendered (the dialog gate is integrityDialogOpen && issues.length > 0).
          // When the last issue clears — via Repair or a command that resolved it —
          // close the dialog explicitly, so the flag can't linger true-but-hidden
          // and pop the dialog back open if a later issue reappears.
          integrityDialogOpen: issues.length === 0 ? false : state.integrityDialogOpen,
        };
      });
      return issues;
    } catch (e) {
      console.error('Failed to check project integrity:', e);
      return [];
    }
  },

  repairProjectIntegrity: async () => {
    try {
      const result = await invokeIpc<IntegrityRepairResult>('repair_project_integrity', undefined, {
        purpose: 'repair project structural integrity',
      });
      // Repair mutates the project; refresh derived state so the UI reflects it.
      await get().refreshProjectState();
      // Repair records a snapshot-pair undo entry, so refresh undo/redo
      // availability — the Undo button must enable so the user can revert it.
      await get().refreshUndoState();
      // Re-check rather than assuming clean: some issues are detect-only and not
      // auto-repairable (a drifted aligned loop closure — recompute/remove it;
      // non-finite shot geometry — fix the data), so reflect the TRUE post-repair
      // state instead of clearing the banner on issues that still exist.
      await get().checkProjectIntegrity();
      // A repair that actually changed the project leaves unsaved edits: mark the
      // store dirty so auto-save runs and the close prompt fires. Otherwise a
      // project that was clean (isDirty false) when the user clicked Repair still
      // looks "saved", and the fix is silently lost on close/reopen.
      //
      // Key on `result.changed`, NOT `result.fixed > 0`: the repair always
      // re-solves offsets, which heals sub-tolerance drift that never counted as a
      // violation, so a genuine fix can report fixed === 0. `changed` is the
      // backend's honest "the snapshot was mutated" signal (the same condition
      // under which it records the undo entry), so it dirties on those silent
      // fixes too while still leaving a true no-op repair clean.
      if (result.changed) {
        set({ isDirty: true });
        // #204/fix 8: a repair that actually mutated the project makes a remote
        // SpeleoDB project un-pushed-dirty too - mirror the backend dirty bit (as
        // executeCommand/executeUndoRedo do) so the status button offers push &
        // prompts for a commit message rather than reading "editing clean". No-op
        // for a local project (no link).
        useSpeleoStore.getState().markDirty();
      }
      return result;
    } catch (e) {
      // Do NOT fabricate a successful no-op { fixed: 0, remaining: 0 }: a thrown
      // repair (an IPC rejection during window/app teardown, a serialization
      // failure, or a backend panic — note the command itself is infallible, so a
      // normal backend Err is not a trigger) must be distinguishable from a genuine
      // clean repair. Otherwise the caller shows a fake "Repaired 0 issues" success
      // toast and swallows the error. Re-throw so the caller surfaces the failure
      // and keeps the Repair entry point open. (integrityIssues / integritySaveBlocked
      // are untouched here, so the banner and save-block stay correct.)
      console.error('Failed to repair project integrity:', e);
      throw e;
    }
  },

  // Hide the banner without clearing integrityIssues, so a re-check after a save
  // (which re-detects the same set) can't resurrect it; the flag is reset when the
  // issue set actually changes. Keeping the issues populated also lets the
  // Review & Repair dialog still open them.
  dismissIntegrityIssues: () => set({ integrityDismissed: true }),

  openIntegrityDialog: () => set({ integrityDialogOpen: true }),
  closeIntegrityDialog: () => set({ integrityDialogOpen: false }),

  removeFromRecentProjects: async (path: string) => {
    try {
      const result = await invokeIpc<CommandResult>(
        'remove_from_recent_projects',
        { path },
        { purpose: 'update recent projects list', fields: { path } }
      );
      if (result.success) {
        // Refresh both the loaded list and the config after removal. The config
        // refresh matters for "missing" recents (paths in config.general
        // .recent_projects that didn't load): those rows are derived from config,
        // so without this the removed path lingers and the row re-renders.
        await Promise.all([get().loadRecentProjects(), get().loadConfig()]);
      }
      return result;
    } catch (e) {
      console.error('Failed to remove from recent projects:', e);
      return emptyCommandResult(false, String(e));
    }
  },

  // Permanently delete a project's files from disk (destructive — distinct from
  // removeFromRecentProjects, which only forgets the entry). The backend hands the
  // files to the OS delete; on success we refresh both the loaded list and the
  // config so the project disappears from the available AND "missing" recent rows.
  deleteProject: async (path: string, deleteCollabSibling: boolean) => {
    try {
      const result = await invokeIpc<CommandResult>(
        'delete_project',
        { path, deleteCollabSibling },
        { purpose: 'permanently delete project', fields: { path } }
      );
      if (result.success) {
        await Promise.all([get().loadRecentProjects(), get().loadConfig()]);
      }
      return result;
    } catch (e) {
      console.error('Failed to delete project:', e);
      return emptyCommandResult(false, String(e));
    }
  },

  // Section operations
  createSection: async (data: SectionData) => {
    return executeCommand(
      'create_section',
      {
        name: data.name,
        surveyor: data.surveyor,
        date: data.date,
        color: data.color,
        direction: data.direction,
        quality: data.quality,
        shots: data.shots,
      },
      {
        purpose: 'add section to project',
        fields: { name: data.name ?? 'unnamed', shot_count: data.shots?.length ?? 0 },
      }
    );
  },

  createSectionAndMerge: async (
    data: SectionData,
    importStationIndex: number,
    targetStation: StationRef,
    closureStrategy: ClosureStrategy
  ) => {
    return executeCommand(
      'create_section_and_merge',
      {
        name: data.name,
        surveyor: data.surveyor,
        date: data.date,
        color: data.color,
        direction: data.direction,
        quality: data.quality,
        shots: data.shots,
        importStationIndex,
        targetStation,
        closureStrategy,
      },
      {
        purpose: 'add section and merge stations',
        fields: { name: data.name ?? 'unnamed', shot_count: data.shots?.length ?? 0 },
      }
    );
  },

  updateSection: async (sectionId: SectionId, updates: SectionUpdates) => {
    return executeCommand(
      'update_section',
      { sectionId, updates },
      { purpose: 'update section metadata', fields: { sectionId } }
    );
  },

  deleteSection: async (sectionId: SectionId) => {
    return executeCommand(
      'delete_section',
      { sectionId },
      { purpose: 'remove section from project', fields: { sectionId } }
    );
  },

  // Shot operations
  createShot: async (sectionId: SectionId, shot: ShotData, afterShotId?: ShotId, beforeShotId?: ShotId) => {
    return executeCommand(
      'create_shot',
      {
        sectionId,
        shot,
        afterShotId: afterShotId ?? null,
        beforeShotId: beforeShotId ?? null,
      },
      {
        purpose: 'add shot to section',
        fields: { sectionId, afterShotId: afterShotId ?? null, beforeShotId: beforeShotId ?? null },
      }
    );
  },

  updateShot: async (sectionId: SectionId, shotId: ShotId, updates: ShotUpdatesPatch) => {
    return executeCommand(
      'update_shot',
      { sectionId, shotId, updates },
      { purpose: 'update shot measurements', fields: { sectionId, shotId } }
    );
  },

  deleteShot: async (sectionId: SectionId, shotId: ShotId) => {
    return executeCommand(
      'delete_shot',
      { sectionId, shotId },
      { purpose: 'delete shot from section', fields: { sectionId, shotId } }
    );
  },

  splitShotAtDistance: async (sectionId: SectionId, shotId: ShotId, distance: number) => {
    return executeCommand(
      'split_shot_at_distance',
      { sectionId, shotId, distance },
      { purpose: 'insert station mid-shot', fields: { sectionId, shotId } }
    );
  },

  deleteShotAndSplit: async (sectionId: SectionId, shotId: ShotId, nameA: string, nameB: string) => {
    return executeCommand(
      'delete_shot_and_split',
      { sectionId, shotId, nameA, nameB },
      { purpose: 'delete shot and split section', fields: { sectionId, shotId } }
    );
  },

  deleteLastShotAndSection: async (sectionId: SectionId, shotId: ShotId) => {
    return executeCommand(
      'delete_last_shot_and_section',
      { sectionId, shotId },
      { purpose: 'remove single-shot section', fields: { sectionId, shotId } }
    );
  },

  deleteShotRangeSplit: async (
    sectionId: SectionId,
    startShotId: ShotId,
    endShotId: ShotId,
    nameA: string,
    nameB: string
  ) => {
    return executeCommand(
      'delete_shot_range_split',
      { sectionId, startShotId, endShotId, nameA, nameB },
      { purpose: 'delete shot range and split section', fields: { sectionId, startShotId, endShotId } }
    );
  },

  deleteShotRangeShift: async (sectionId: SectionId, startShotId: ShotId, endShotId: ShotId) => {
    return executeCommand(
      'delete_shot_range_shift',
      { sectionId, startShotId, endShotId },
      { purpose: 'delete shot range and close gap', fields: { sectionId, startShotId, endShotId } }
    );
  },

  setShotReference: async (sectionId: SectionId, shotId: ShotId, isReference: boolean) => {
    return executeCommand(
      'set_shot_reference',
      { sectionId, shotId, isReference },
      { purpose: 'toggle reference shot visibility', fields: { sectionId, shotId, isReference } }
    );
  },

  // Station operations
  mergeStations: async (stationA: StationRef, stationB: StationRef) => {
    return executeCommand(
      'merge_stations',
      { stationA, stationB },
      { purpose: 'connect survey sections', fields: { stationA, stationB } }
    );
  },

  unmergeStations: async (stationRef: StationRef) => {
    return executeCommand(
      'unmerge_stations',
      { stationRef },
      { purpose: 'disconnect merged stations', fields: { stationRef } }
    );
  },

  unmergeStationPair: async (stationA: StationRef, stationB: StationRef) => {
    return executeCommand(
      'unmerge_station_pair',
      { stationA, stationB },
      { purpose: 'disconnect selected merged station pair', fields: { stationA, stationB } }
    );
  },

  checkMergeConnectivity: async (stationA: StationRef, stationB: StationRef) => {
    return await invokeIpc<ConnectivityResult>(
      'check_merge_connectivity',
      { stationA, stationB },
      { purpose: 'check merge connectivity', fields: { stationA, stationB } }
    );
  },

  createLoopClosure: async (stationA: StationRef, stationB: StationRef, closureStrategy: ClosureStrategy) => {
    return executeCommand(
      'create_loop_closure',
      { stationA, stationB, closureStrategy },
      { purpose: 'add loop closure for accuracy', fields: { stationA, stationB, closureStrategy } }
    );
  },

  removeLoopClosure: async (closureId: bigint) => {
    return executeCommand(
      'remove_loop_closure',
      { closureId: Number(closureId) },
      { purpose: 'remove loop closure', fields: { closureId: closureId.toString() } }
    );
  },

  addMarker: async (sectionId, shotId, position, markerType, color, text, direction) => {
    return executeCommand(
      'add_marker',
      { sectionId, shotId, position, markerType, color, text, direction },
      { purpose: 'add marker to shot', fields: { sectionId, shotId } }
    );
  },

  updateMarker: async (markerId, updates) => {
    return executeCommand(
      'update_marker',
      { markerId: Number(markerId), updates },
      { purpose: 'update marker', fields: { markerId: markerId.toString() } }
    );
  },

  deleteMarker: async (markerId) => {
    return executeCommand(
      'delete_marker',
      { markerId: Number(markerId) },
      { purpose: 'delete marker', fields: { markerId: markerId.toString() } }
    );
  },

  loadAdjustedGeometry: async (): Promise<boolean> => {
    const { project, surveyData, stations, adjustedLoading, config } = get();
    // No-op (no project, or a fetch already in flight): not a failure.
    if (!project || adjustedLoading) return true;

    // No eligible closures → the adjusted view is identical to measured; reuse
    // the measured set directly and skip the backend round-trip.
    if (!hasEligibleClosures(project.loop_closures)) {
      set({ stationsAdjusted: stations, surveyDataAdjusted: surveyData });
      return true;
    }

    set({ adjustedLoading: true });
    try {
      const adjusted = await invokeIpc<Station[]>('get_adjusted_stations', undefined, {
        purpose: 'closure-adjusted geometry view',
      });
      // Discard if the project OR the closure-adjustment config changed while the
      // solve was in flight (a refreshProjectState / updateConfig nulled the cache
      // but the project ref is unchanged on a config-only change), otherwise we'd
      // cache geometry solved against stale sigmas. The map effect re-fetches once
      // `adjustedLoading` clears.
      if (get().project !== project || get().config !== config) {
        set({ adjustedLoading: false });
        return true;
      }
      const surveyDataAdjusted = surveyDataFromLegacy(project.sections, adjusted);
      set({ stationsAdjusted: adjusted, surveyDataAdjusted, adjustedLoading: false });
      return true;
    } catch (e) {
      console.warn('Failed to load adjusted geometry:', e);
      set({ adjustedLoading: false });
      return false;
    }
  },

  updateStationComment: async (stationRef: StationRef, comment: string | null) => {
    return executeCommand(
      'set_station_comment',
      { stationRef, comment },
      { purpose: 'update station comment', fields: { stationRef } }
    );
  },

  moveComment: async (source: CommentHolder, target: CommentHolder, swap: boolean) => {
    return executeCommand(
      'move_comment',
      { source, target, swap },
      { purpose: swap ? 'swap comments' : 'move comment', fields: { source, target } }
    );
  },

  setZeroDatum: async (stationRef: StationRef, coordinates: GpsCoordinates, options?: { fitToBounds?: boolean }) => {
    // Detect first zero datum before the IPC call changes state
    const isFirstDatum = (get().project?.zero_datums.length ?? 0) === 0;
    const result = await executeCommand(
      'set_zero_datum',
      { stationRef, coordinates },
      { purpose: 'anchor survey to GPS coordinates', fields: { stationRef } }
    );
    if (result.success && options?.fitToBounds !== false) {
      // Snap (no animation) for first zero datum — the camera has no
      // meaningful prior position so animating from it is disorienting.
      useUIStore.getState().triggerFitToBounds(isFirstDatum ? { snap: true } : undefined);
    }
    return result;
  },

  removeZeroDatum: async (stationRef: StationRef) => {
    return executeCommand(
      'remove_zero_datum',
      { stationRef },
      { purpose: 'remove GPS anchor', fields: { stationRef } }
    );
  },

  moveSection: async (sectionId: SectionId, offset: Coordinates) => {
    return executeCommand(
      'move_section',
      { sectionId, offset },
      { purpose: 'reposition section for alignment', fields: { sectionId } }
    );
  },

  moveSegment: async (sectionId: SectionId, offset: Coordinates) => {
    return executeCommand(
      'move_segment',
      { sectionId, offset },
      { purpose: 'move connected segment', fields: { sectionId } }
    );
  },

  moveStation: async (stationRef: StationRef, offset: Coordinates) => {
    return executeCommand(
      'move_station',
      { stationRef, offset },
      { purpose: 'adjust station position', fields: { stationRef } }
    );
  },

  getConnectedSectionIds: async (sectionId: SectionId) => {
    return await invokeIpc<number[]>(
      'get_rigid_connected_section_ids',
      { sectionId },
      { purpose: 'query connected section ids', fields: { sectionId } }
    );
  },

  getAnchoredSectionIds: async () => {
    return await invokeIpc<number[]>(
      'get_anchored_section_ids',
      undefined,
      { purpose: 'query anchored section ids' }
    );
  },

  // Undo/Redo
  undo: async () =>
    executeUndoRedo('undo', undefined, { purpose: 'undo last action' }, 'undo-failed', false),

  redo: async () =>
    executeUndoRedo('redo', undefined, { purpose: 'redo last undone action' }, 'redo-failed', false),

  undoTo: async (index: number) =>
    executeUndoRedo('undo_to', { index }, { purpose: 'undo to history index', fields: { index } }, 'undo-failed', true),

  redoTo: async (index: number) =>
    executeUndoRedo('redo_to', { index }, { purpose: 'redo to history index', fields: { index } }, 'redo-failed', true),

  getUndoStack: async () => {
    return await invokeIpc<UndoHistoryItem[]>('get_undo_stack', undefined, {
      purpose: 'fetch undo history',
    });
  },

  getRedoStack: async () => {
    return await invokeIpc<UndoHistoryItem[]>('get_redo_stack', undefined, {
      purpose: 'fetch redo history',
    });
  },

  refreshUndoState: async () => {
    try {
      const [canUndo, canRedo] = await Promise.all([
        invokeIpc<boolean>('can_undo', undefined, { purpose: 'check undo availability' }),
        invokeIpc<boolean>('can_redo', undefined, { purpose: 'check redo availability' }),
      ]);
      set({ canUndo, canRedo });
    } catch (e) {
      console.error('Failed to refresh undo state:', e);
    }
  },

  // Batch execution - groups multiple commands into a single undo/redo unit
  executeBatch: async (commands: BatchCommand[]) => {
    return executeCommand(
      'execute_batch',
      { commands },
      { purpose: 'apply batch edits', fields: { command_count: commands.length } }
    );
  },

  // Import/Export
  importDmp: async (path: string) => {
    return await invokeIpc<DmpFile>('import_dmp', { path }, {
      purpose: 'parse DMP for import preview',
      fields: { path },
    });
  },

  parseTml: async (path: string) => {
    return await invokeIpc<TmlFile>('parse_tml', { path }, {
      purpose: 'parse TML for import preview',
      fields: { path },
    });
  },

  executeTmlImport: async (path: string, fileHash?: string | null, includeExcluded?: boolean, commentsOnStations?: boolean) => {
    const s = peekProjectActionSource();
    if (s) setSource(s);
    try {
      return await executeCommand(
        'execute_tml_import',
        {
          path,
          fileHash: fileHash ?? null,
          includeExcluded: includeExcluded ?? false,
          commentsOnStations: commentsOnStations ?? true,
        },
        { purpose: 'import Ariane TML/TMLU', fields: { path } }
      );
    } finally {
      // Terminal flow: clear the pending source so a failed/abandoned import
      // can't leak the armed project-action source into the next command.
      clearProjectActionSource();
    }
  },

  parseCompass: async (path: string) => {
    return await invokeIpc<CompassFile>('parse_compass', { path }, {
      purpose: 'parse Compass for import preview',
      fields: { path },
    });
  },

  executeCompassImport: async (path: string, fileHash?: string | null, includeExcluded?: boolean, applyCorrections?: boolean, commentsOnStations?: boolean) => {
    const s = peekProjectActionSource();
    if (s) setSource(s);
    try {
      return await executeCommand(
        'execute_compass_import',
        {
          path,
          fileHash: fileHash ?? null,
          includeExcluded: includeExcluded ?? false,
          applyCorrections: applyCorrections ?? true,
          commentsOnStations: commentsOnStations ?? true,
        },
        { purpose: 'import Compass', fields: { path } }
      );
    } finally {
      // Terminal flow: clear the pending source so a failed/abandoned import
      // can't leak the armed project-action source into the next command.
      clearProjectActionSource();
    }
  },

  parseWalls: async (path: string) => {
    return await invokeIpc<WallsFile>('parse_walls', { path }, {
      purpose: 'parse Walls for import preview',
      fields: { path },
    });
  },

  executeWallsImport: async (path: string, fileHash?: string | null, commentsOnStations?: boolean, includeSplays?: boolean, selectedSurveys?: number[] | null) => {
    // Re-arm the transient `_source` from the durable pending bridge (it resets
    // ~50ms after being set), so `command.execute_walls_import` is attributed to
    // the real entry point (hamburger/keyboard/drag-drop/dialog) instead of
    // 'unknown' — matching executeCompassImport/Tml/Gpx.
    const s = peekProjectActionSource();
    if (s) setSource(s);
    try {
      return await executeCommand(
        'execute_walls_import',
        {
          path,
          fileHash: fileHash ?? null,
          commentsOnStations: commentsOnStations ?? true,
          includeSplays: includeSplays ?? false,
          // `null` ⇒ import every survey; an array picks specific survey indices
          // (into the preview's `surveys`, 1:1 with the parser order).
          selectedSurveys: selectedSurveys ?? null,
        },
        { purpose: 'import Walls', fields: { path } }
      );
    } finally {
      // Terminal flow: clear the pending source so a failed/abandoned import
      // can't leak the armed project-action source into the next command.
      clearProjectActionSource();
    }
  },

  // Collaboration mode
  updateProjectSettings: async (name: string, validation: Config['validation']) => {
    return executeCommand(
      'update_project_settings',
      { name, validation },
      { purpose: 'update per-project settings' }
    );
  },

  setCollabEnabled: async (enabled: boolean) => {
    return executeCommand(
      'set_collab_enabled',
      { enabled },
      { purpose: 'toggle collaboration mode' }
    );
  },

  setCollabEnabledForPath: async (path: string, enabled: boolean) => {
    return executeCommand(
      'set_collab_enabled_for_path',
      { path, enabled },
      { purpose: 'toggle collaboration mode for a recent project', fields: { path } }
    );
  },

  setProjectNameForPath: async (path: string, name: string) => {
    return executeCommand(
      'set_project_name_for_path',
      { path, name },
      { purpose: 'rename a recent project', fields: { path } }
    );
  },

  getCollabState: async () => {
    return await invokeIpc<CollabState | null>('get_collab_state', undefined, {
      purpose: 'read collaboration state',
    });
  },

  detectCollabChanges: async () => {
    return await invokeIpc<CollabChangeSet | null>('detect_collab_changes', undefined, {
      purpose: 'detect collaborator .tml changes',
    });
  },

  applyCollabChanges: async () => {
    return executeCommand('apply_collab_changes', {}, {
      purpose: 'apply collaborator changes',
    });
  },

  checkCollabSibling: async () => {
    return await invokeIpc<string | null>('collab_sibling_when_disabled', undefined, {
      purpose: 'warn about an unsynced Ariane sibling file',
    });
  },


  parseGpx: async (path: string) => {
    return await invokeIpc<GpxFile>('parse_gpx', { path }, {
      purpose: 'parse GPX for import preview',
      fields: { path },
    });
  },

  executeGpxImport: async (path: string, fileHash: string | null, importWaypoints: boolean, importTracks: boolean, importRoutes: boolean, sectionPlacements: GpxSectionPlacement[], depthInverted: boolean) => {
    const s = peekProjectActionSource();
    if (s) setSource(s);
    try {
      return await executeCommand(
        'execute_gpx_import',
        {
          path,
          fileHash,
          importWaypoints,
          importTracks,
          importRoutes,
          sectionPlacements,
          depthInverted,
        },
        { purpose: 'import GPX file', fields: { path } }
      );
    } finally {
      // Terminal flow: clear the pending source so a failed/abandoned import
      // can't leak the armed project-action source into the next command.
      clearProjectActionSource();
    }
  },

  calculateFileHash: async (path: string) => {
    return await invokeIpc<string>('calculate_file_hash', { path }, {
      purpose: 'detect duplicate imports',
      fields: { path },
    });
  },

  checkDuplicateImport: async (hash: string) => {
    return await invokeIpc<boolean>('check_duplicate_import', { hash }, {
      purpose: 'detect duplicate imports',
      fields: { hash },
    });
  },

  trackImportedFile: async (hash: string) => {
    await invokeIpc('track_imported_file', { hash }, {
      purpose: 'track imported file hash',
      fields: { hash },
    });
  },

  checkSectionHashes: async (hashes: string[]) => {
    return await invokeIpc<boolean[]>('check_section_hashes', { hashes }, {
      purpose: 'detect duplicate section imports',
      fields: { count: hashes.length },
    });
  },

  importSections: async (sections: ImportSectionConfig[], fileHash?: string | null, sectionHashes?: string[] | null) => {
    const s = peekProjectActionSource();
    if (s) setSource(s);
    try {
      return await executeCommand(
        'import_sections',
        {
          sections,
          fileHash: fileHash ?? null,
          sectionHashes: sectionHashes ?? null,
        },
        { purpose: 'apply imported sections', fields: { section_count: sections.length } }
      );
    } finally {
      // Terminal flow: the project is already open, so there's no
      // setView('single') to clear the pending source. Clear it here so it
      // can't leak into the next peeking command.
      clearProjectActionSource();
    }
  },

  suggestImportPlacement: async (shots: ShotData[]) => {
    return await invokeIpc<PlacementSuggestion[]>('suggest_import_placement', { shots }, {
      purpose: 'suggest placement from reference shot overlap',
    });
  },

  exportCsv: async (path: string) => {
    await invokeIpc('export_csv', { path }, {
      purpose: 'export project as CSV',
      fields: { path },
    });
  },

  exportJson: async (path: string) => {
    await invokeIpc('export_json', { path }, {
      purpose: 'export project as JSON',
      fields: { path },
    });
  },

  exportTml: async (path: string) => {
    await invokeIpc('export_tml', { path }, {
      purpose: 'export project as Ariane TML',
      fields: { path },
    });
  },

  exportImage: async (path: string, options: ExportImageOptions) => {
    await invokeIpc('export_image', { path, options }, {
      purpose: 'export map image',
      fields: { path, format: options.format },
    });
  },

  mergeProjects: async (sourcePaths: string[], outputPath: string, outputName: string, archiveOriginals: boolean) => {
    try {
      const result = await invokeIpc<CommandResult>(
        'merge_projects',
        {
          sourcePaths,
          outputPath,
          outputName,
          archiveOriginals,
        },
        {
          purpose: 'merge projects into single file',
          fields: { source_count: sourcePaths.length, outputPath },
        }
      );
      if (result.success) {
        // Refresh recent projects to reflect any changes (archived files removed, new file added)
        await get().loadRecentProjects();
      }
      return result;
    } catch (e) {
      console.error('Failed to merge projects:', e);
      return emptyCommandResult(false, String(e));
    }
  },
};
});
