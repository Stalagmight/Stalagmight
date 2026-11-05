import { create } from 'zustand';
import i18n from '../i18n';
import { invokeIpc, type ForwardedInvoke, type IpcResult, type MutationCommand, type QueuedCallArgs } from '../utils/ipc';
import { trackCommand, setSource, peekProjectActionSource, clearProjectActionSource } from '../utils/telemetry';
import { debugLog } from '../utils/debugLog';
import { extractFilename } from '../utils/path';
import { isPermissionError, saveErrorKind, type SaveErrorKind } from '../utils/saveErrors';
import { isEditGateCode, isReverifyDivergedCode, READ_ONLY_EDIT_NOTICE } from '../utils/editErrors';
import { commandErrorOf, getErrorMessage } from '../utils/commandError';
import { hasEligibleClosures } from '../utils/closures';
import { applyReverseAftermath, composeReverseTransforms, flushPlanAnchors, remapPlanPins } from '../utils/reverseAftermath';
import {
  captureDistanceOrigin,
  dropDormantDistanceOrigin,
  remapDistanceOrigin,
  type OriginCapture,
} from '../utils/distanceOriginAnchor';
import { useUIStore } from './uiStore';
import { autosaveFailureMessage, saveFailureOf, surfaceSaveFailure, surfaceSaveWarnings } from '../utils/saveFeedback';
import { useGlobalViewStore } from './globalViewStore';
import { speleoState } from './speleoStoreHandle';
import { emptyCommandResult, refusedLocally, settleCommand, type CommandOutcome } from './commandResult';
import { useExportStore } from './exportStore';
import { refuseMutationDuringCarry, refuseStaleCarryCommit, takeMoveCarryCommit, type MoveCarryCommit } from './moveCarry';
import { runSerialized } from './mutationQueue';
import {
  noteProjectDirtied,
  noteProjectSessionChanged,
  projectDirtyMarks,
  projectSessionEpoch,
  registerOpenProjectIdentity,
} from './projectSession';
import type {
  Project,
  ProjectMetadata,
  Station,
  ProjectStats,
  SectionData,
  SectionUpdates,
  ShotData,
  ShotUpdates,
  ShotKind,
  VirtualShotMode,
  SectionId,
  ShotId,
  StationRef,
  GpsCoordinates,
  ClosureStrategy,
  Coordinates,
  FrozenShotRef,
} from '../types/project';
import type { ExistingShotUpdate } from '../types/ipc/ExistingShotUpdate';
import type { ReversedSection } from '../types/ipc/ReversedSection';
import type { BatchOutcome, DeleteProjectOutcome, EditOutcome, HistoryOutcome, ImportOutcome, ProjectFileOutcome, ReverseOutcome, SaveOutcome, SectionCreated, ShotCreated, ValidationWarning } from '../types/ipc.generated';
import type { Config } from '../types/config';
import type { AzimuthReference } from '../types/ipc/AzimuthReference';
import type { CommandResult, CommentHolder, ConnectivityResult, DmpFile, TmlFile, TmlRootPlacement, CompassFile, WallsFile, GpxFile, GpxSectionPlacement, ProjectWarning, BatchCommand, UndoHistoryItem, ImportSectionConfig, PlacementSuggestion } from '../types/commands';

/** Additive import-receipt provenance (#391) - see `import_sections` (Rust) for the fields' meaning. */
export interface ImportReceiptProvenance {
  sourceLabel?: string;
  sourceFileName?: string;
  sectionsFound?: number;
  /** The file's own parse warnings (`DmpFile.warnings`, #576 B13); they lead the receipt. */
  sourceWarnings?: string[];
}
import type { SurveyData } from '../types/survey-data';
import { emptySurveyData, surveyDataFromLegacy, surveyDataWithAdjustedStations } from '../types/survey-data';
import { createProjectConfigActions, readRecentPathsInOrder, withRecentPaths, type ProjectConfigActions } from './projectConfigActions';
import { createProjectHistoryBannerActions, HISTORY_BANNER_CLEARED, type ProjectHistoryBannerActions, type ProjectHistoryBannerState } from './projectHistoryBannerActions';
import { createProjectLoadRepairsActions, LOAD_REPAIRS_CLEARED, loadRepairsStateFor, type ProjectLoadRepairsActions, type ProjectLoadRepairsState } from './projectLoadRepairsBanner';
import { createProjectIntegrityActions, type ProjectIntegrityActions } from './projectIntegrityActions';
import { createProjectExportActions, type ProjectExportActions } from './projectExportActions';
import { runProjectSwitch } from '../utils/projectTransition';
import { TOAST_KEYS } from '../utils/toastKeys';

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
  loadingId: number;
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
  loadingId: 0,
  clientOpId: 0,
  isLoading: false,
  overallProgress: 0,
};

// Separate client operation ID for race detection (independent of backend loading ID)
let clientOpIdCounter = 0;
let projectTreeGenerationCounter = 0;

function nextProjectTreeGeneration(): number {
  projectTreeGenerationCounter += 1;
  return projectTreeGenerationCounter;
}

// [#576 RV5-8] `refreshProjectState` requests, numbered in issue order, and the
// newest one whose answer the store has applied. The commands run on the backend's
// pool, so two refreshes (one inside the mutation queue, one from a caller outside
// it) can answer in either order; an answer older than one already applied is
// dropped rather than written over the newer state.
let refreshRequestCounter = 0;
let refreshAppliedRequest = 0;
// [#576 FZ-FZ-INT-10] The same pair for `refreshUndoState`.
let undoStateRequestCounter = 0;
let undoStateAppliedRequest = 0;

interface CreateProjectOptions {
  savePath?: string;
  saveDirectory?: string;
  deferInitialSave?: boolean;
}

/**
 * What a `loadProject` call resolved to.
 *
 * `success: false` used to carry two very different things under one boolean: a
 * genuine failure (the file is gone, corrupt, unreadable - the previous project
 * is still on screen and the user has been shown an error), and "SUPERSEDED" -
 * another load started while this one was in flight, so this call's result was
 * deliberately dropped and the project on screen is now someone else's.
 *
 * Callers that undo something on failure need to tell those apart: a rollback is
 * correct for the first and actively harmful for the second, where it writes
 * pre-attempt state over the project the user actually opened (#270). So the
 * superseded case says so. It is optional because "not superseded" is the only
 * meaning `success: false` ever had for the callers that just check the boolean,
 * and every existing call site (and test double) keeps compiling unchanged.
 */
export interface LoadProjectOutcome {
  success: boolean;
  /**
   * This call lost its slot to a NEWER load (the `clientOpId` moved under it).
   * Nothing about the failure describes the project now on screen, so a caller
   * must not roll anything back on it. Never set together with `success: true`.
   */
  superseded?: boolean;
}

export interface ProjectStore extends ProjectConfigActions, ProjectHistoryBannerActions, ProjectHistoryBannerState, ProjectLoadRepairsActions, ProjectLoadRepairsState, ProjectIntegrityActions, ProjectExportActions {
  // State
  project: Project | null;
  /**
   * Monotonic identity of the measured project tree currently mirrored in this store.
   * Incremented on every successful load, create, refresh, and close so async caches can
   * distinguish same-path revision swaps and in-memory edits whose project metadata did
   * not change.
   */
  projectTreeGeneration: number;
  /**
   * Whether the open project is backed by an on-disk database. A new project
   * created with a deferred initial save reserves `project.file_path` but leaves
   * the backend database unopened until the first save, so DB-backed actions
   * (e.g. persisting an ignored warning) can't run yet - gate them on this, not
   * on `file_path` alone (issue #57 / Codex).
   */
  databaseOpen: boolean;
  /**
   * [#576 B6-D1] The open file's schema version when a NEWER Stalagmight saved it,
   * else null. The backend opened such a project read-only (every edit and save
   * refused, the file never written); this is its verdict for THIS load, set in the
   * same `set()` as the project so the UI can never present it as editable. Read
   * through `selectNewerFileReadOnly` - the canonical gate (`useEditingBlocked`)
   * composes it.
   */
  newerSchemaVersion: number | null;
  /**
   * [#576 B35(b) review] The open file's SpeleoDB link row could not be read. It
   * opened as a local project, but the backend refuses every save over this file
   * (the rebuilt file would lose the row), so App.tsx shows a banner from the first
   * frame instead of leaving the first failed save to explain it. Set from the load
   * result; cleared on close, create, and a Save As that moves the session away.
   */
  speleoLinkUnreadable: boolean;
  /** [#576 B15] Stored project settings the load could not read; while any are
   *  listed the backend refuses every save (and autosave does not try). */
  unreadableProjectSettings: string[];
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
  /**
   * A failing background autosave (docs/notifications.md: banner-class - an
   * ongoing "your latest changes are not on disk" condition, not a modal
   * moment). Set by `saveProject` on an autosave failure, cleared by the next
   * successful save, project load/close, or an explicit dismiss.
   */
  autosaveFailure: string | null;
  /**
   * What KIND of failure `autosaveFailure` describes (#517). The message is
   * already user-ready, so this exists only for routing: the banner yields to
   * the integrity banner for an ordinary failure, and must NOT yield for one
   * Review & Repair cannot fix. Kept beside the message and cleared with it.
   */
  autosaveFailureKind: SaveErrorKind | null;
  clearAutosaveFailure: () => void;
  canUndo: boolean;
  canRedo: boolean;
  isDirty: boolean;
  /** Guard to prevent concurrent close operations (rapid Back clicks) */
  isClosing: boolean;

  // Actions (loadConfig / updateConfig / updateGasConfig / setConfig: see ProjectConfigActions)

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
  loadProject: (path: string) => Promise<LoadProjectOutcome>;
  /** Create a new project (legacy, still supported) */
  createProject: (name: string, projectLocation?: string, options?: CreateProjectOptions) => Promise<CommandResult>;
  /**
   * Save the project. `isAutosave` marks a QUIET, BACKGROUND save - one the user
   * did not ask for and is not waiting on - and suppresses two bits of UI churn
   * for it: the save-permission dialog (which would otherwise reopen on every
   * retry while folder access is denied) and the post-save integrity re-check
   * (skipped unless the project entered the save already flagged, since the save
   * path auto-heals some issues).
   *
   * It also crosses IPC, where it does exactly one thing: name the trigger in
   * the backend's log line (#429). The save itself is byte-for-byte identical
   * either way - the flag never reaches the pipeline.
   *
   * Read it as "background save", NOT literally "the auto-save timer fired":
   * `useAutoSave`'s debounced write is one caller, and `flushToDiskAfterPush` in
   * `useSpeleoActions` is the other - a single, undebounced flush after a
   * confirmed SpeleoDB push. Both are quiet writes the user never requested, so
   * both pass `true`; anything the user explicitly asked for (Ctrl+S, Save As,
   * save-after-import) must not.
   */
  saveProject: (path?: string, isAutosave?: boolean) => Promise<CommandOutcome<SaveOutcome>>;
  /**
   * Persist the project after a DMP import completes (#92). Imports bypass the
   * dirty/save machinery, so the change would otherwise be lost. For a NEW
   * project (`importContext === 'new-project'`) the initial save is deferred -
   * only a real save writes the `.stalx` and registers it in Recent Projects -
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
  closeProject: (opts?: { releaseLock?: boolean }) => Promise<CommandOutcome<ProjectFileOutcome>>;
  refreshProjectState: () => Promise<void>;
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
  /** True when the open project has unresolvable cross-references - the exact
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
  // checkProjectIntegrity / repairProjectIntegrity, and refreshWarnings / resolveWarning /
  // unresolveWarning: see ProjectIntegrityActions
  /** Dismiss the integrity warning without repairing. */
  dismissIntegrityIssues: () => void;
  loadRecentProjects: () => Promise<void>;
  removeFromRecentProjects: (path: string) => Promise<CommandOutcome<ProjectFileOutcome>>;
  deleteProject: (path: string) => Promise<CommandOutcome<DeleteProjectOutcome>>;
  mergeProjects: (sourcePaths: string[], outputPath: string, outputName: string, archiveOriginals: boolean) => Promise<CommandOutcome<ProjectFileOutcome>>;

  createSection: (data: SectionData) => Promise<CommandOutcome<SectionCreated>>;
  createSectionAndMerge: (
    data: SectionData,
    importStationIndex: number,
    targetStation: StationRef,
    closureStrategy: ClosureStrategy
  ) => Promise<CommandOutcome<SectionCreated>>;
  updateSection: (sectionId: SectionId, updates: SectionUpdates) => Promise<CommandOutcome<EditOutcome>>;
  deleteSection: (sectionId: SectionId) => Promise<CommandOutcome<EditOutcome>>;
  /**
   * Reverse a section end-to-end so the far end becomes station 0 (#24).
   * Geometry is preserved - every station keeps its world coordinate and only
   * the numbering direction changes. `swapLeftRight` also mirrors the left/right
   * passage dimensions; it is opt-in because whether LRUD was recorded facing
   * along travel is a survey convention we cannot infer, so the user is asked.
   */
  reverseSection: (sectionId: SectionId, swapLeftRight: boolean) => Promise<CommandOutcome<ReverseOutcome>>;

  createShot: (sectionId: SectionId, shot: ShotData, afterShotId?: ShotId, beforeShotId?: ShotId) => Promise<CommandOutcome<ShotCreated>>;
  updateShot: (sectionId: SectionId, shotId: ShotId, updates: ShotUpdatesPatch) => Promise<CommandOutcome<EditOutcome>>;
  deleteShot: (sectionId: SectionId, shotId: ShotId) => Promise<CommandOutcome<EditOutcome>>;
  splitShotAtDistance: (sectionId: SectionId, shotId: ShotId, distance: number) => Promise<CommandOutcome<ShotCreated>>;
  deleteShotAndSplit: (sectionId: SectionId, shotId: ShotId, nameA: string, nameB: string) => Promise<CommandOutcome<SectionCreated>>;
  deleteLastShotAndSection: (sectionId: SectionId, shotId: ShotId) => Promise<CommandOutcome<EditOutcome>>;
  deleteShotRangeSplit: (sectionId: SectionId, startShotId: ShotId, endShotId: ShotId, nameA: string, nameB: string) => Promise<CommandOutcome<SectionCreated>>;
  deleteShotRangeShift: (sectionId: SectionId, startShotId: ShotId, endShotId: ShotId) => Promise<CommandOutcome<EditOutcome>>;
  setShotKind: (sectionId: SectionId, shotId: ShotId, kind: ShotKind) => Promise<CommandOutcome<EditOutcome>>;

  mergeStations: (stationA: StationRef, stationB: StationRef) => Promise<CommandOutcome<EditOutcome>>;
  unmergeStations: (stationRef: StationRef) => Promise<CommandOutcome<EditOutcome>>;
  unmergeStationPair: (stationA: StationRef, stationB: StationRef) => Promise<CommandOutcome<EditOutcome>>;
  checkMergeConnectivity: (stationA: StationRef, stationB: StationRef) => Promise<ConnectivityResult>;
  createLoopClosure: (stationA: StationRef, stationB: StationRef, closureStrategy: ClosureStrategy) => Promise<CommandOutcome<EditOutcome>>;
  removeLoopClosure: (closureId: number) => Promise<CommandOutcome<EditOutcome>>;
  updateStationComment: (stationRef: StationRef, comment: string | null) => Promise<CommandOutcome<EditOutcome>>;
  moveComment: (source: CommentHolder, target: CommentHolder, swap: boolean) => Promise<CommandOutcome<EditOutcome>>;
  setZeroDatum: (stationRef: StationRef, coordinates: GpsCoordinates, options?: { fitToBounds?: boolean }) => Promise<CommandOutcome<EditOutcome>>;
  removeZeroDatum: (stationRef: StationRef) => Promise<CommandOutcome<EditOutcome>>;
  moveSection: (sectionId: SectionId, offset: Coordinates) => Promise<CommandOutcome<EditOutcome>>;
  moveSegment: (sectionId: SectionId, offset: Coordinates) => Promise<CommandOutcome<EditOutcome>>;
  moveStation: (stationRef: StationRef, newPosition: Coordinates) => Promise<CommandOutcome<EditOutcome>>;
  getConnectedSectionIds: (sectionId: SectionId) => Promise<number[]>;
  getAnchoredSectionIds: () => Promise<number[]>;

  undo: () => Promise<CommandOutcome<HistoryOutcome>>;
  redo: () => Promise<CommandOutcome<HistoryOutcome>>;
  undoTo: (index: number) => Promise<CommandOutcome<HistoryOutcome>>;
  redoTo: (index: number) => Promise<CommandOutcome<HistoryOutcome>>;
  getUndoStack: () => Promise<UndoHistoryItem[]>;
  getRedoStack: () => Promise<UndoHistoryItem[]>;
  refreshUndoState: () => Promise<void>;

  // Batch execution - groups multiple commands into a single undo/redo unit
  executeBatch: (commands: BatchCommand[]) => Promise<CommandOutcome<BatchOutcome>>;

  importDmp: (path: string) => Promise<DmpFile>;
  parseTml: (path: string) => Promise<TmlFile>;
  executeTmlImport: (path: string, fileHash?: string | null, includeExcluded?: boolean, commentsOnStations?: boolean, virtualShotMode?: VirtualShotMode, selectedSections?: number[] | null, rootPlacement?: TmlRootPlacement | null) => Promise<CommandOutcome<ImportOutcome>>;
  parseCompass: (path: string) => Promise<CompassFile>;
  executeCompassImport: (path: string, fileHash?: string | null, includeExcluded?: boolean, applyCorrections?: boolean, commentsOnStations?: boolean) => Promise<CommandOutcome<ImportOutcome>>;
  parseWalls: (path: string) => Promise<WallsFile>;
  executeWallsImport: (path: string, fileHash?: string | null, commentsOnStations?: boolean, includeSplays?: boolean, selectedSurveys?: number[] | null) => Promise<CommandOutcome<ImportOutcome>>;
  // Per-project settings (#57): name + validation thresholds, stored in the .stalx, and
  // the default north reference (#576 NF-W3-2; `null` = nothing recorded). All three go
  // on every call: the backend sets each to what it is given.
  updateProjectSettings: (
    name: string,
    validation: Config['validation'],
    defaultAzimuthReference: AzimuthReference | null
  ) => Promise<CommandOutcome<EditOutcome>>;
  /** Set (or, given `null`, clear) the entrance elevation in meters (#465). */
  setSurfaceElevation: (elevation: number | null) => Promise<CommandOutcome<EditOutcome>>;
  /** Rename a recent project's display name by path (project need not be open). */
  setProjectNameForPath: (path: string, name: string) => Promise<CommandOutcome<EditOutcome>>;
  parseGpx: (path: string) => Promise<GpxFile>;
  executeGpxImport: (path: string, fileHash: string | null, importWaypoints: boolean, importTracks: boolean, importRoutes: boolean, sectionPlacements: GpxSectionPlacement[], depthInverted: boolean) => Promise<CommandOutcome<ImportOutcome>>;
  calculateFileHash: (path: string) => Promise<string>;
  checkDuplicateImport: (hash: string) => Promise<boolean>;
  checkSectionHashes: (hashes: string[]) => Promise<boolean[]>;
  /**
   * `existingShotUpdates` is the overlap interpolation the DMP wizard computes
   * client-side (`applyAllOverlapTreatments`). It is LAST rather than in the
   * command's own third position deliberately: appending leaves every existing
   * call's argument positions untouched. Optional and defaulted to `null`, which
   * the backend reads as "no updates". `readOnlyNoticeInline`: the caller shows a
   * read-only refusal inline itself (the DMP wizard, #576 RV2-FE-D3), so the edit path
   * does not toast the same notice on top.
   */
  importSections: (
    sections: ImportSectionConfig[],
    fileHash?: string | null,
    sectionHashes?: string[] | null,
    virtualShotMode?: VirtualShotMode,
    provenance?: ImportReceiptProvenance,
    existingShotUpdates?: ExistingShotUpdate[] | null,
    readOnlyNoticeInline?: boolean,
  ) => Promise<CommandOutcome<ImportOutcome>>;
  suggestImportPlacement: (
    shots: ShotData[],
    azimuthReference?: AzimuthReference | null,
  ) => Promise<PlacementSuggestion[]>;
}

/**
 * Tracks in-flight `loadProject(path)` calls so concurrent requests for the
 * same path share one execution. Each entry is removed when the underlying
 * promise settles. See loadProject for rationale.
 */
const inFlightLoads = new Map<string, Promise<{ success: boolean }>>();

/**
 * Whether the project changed identity between issuing a command and its reply (#24).
 *
 * `created_at` alone: it is immutable for the life of a project, whereas `file_path` changes on
 * a first save or a Save As - keying on that made the guard reject replies that were perfectly
 * valid and silently leave the frontend's positional refs on the wrong physical station.
 *
 * A missing value on either side is NOT a switch. Before the first refresh the store may hold
 * no project at all, and a closed project resolves to no sections, which the transform builder
 * already reduces to a no-op. Only two DIFFERENT identities mean the reply belongs to a project
 * that is no longer open - and because section ids are small per-project integers, those entries
 * would otherwise match sections in whatever is now open.
 */
export function projectSwitched(before: string | null, after: string | null): boolean {
  return before !== null && after !== null && before !== after;
}

/**
 * [#576 B6-D1] Is the open project read-only because a NEWER Stalagmight saved it?
 * One of the terms `useEditingBlocked` composes; gate edits through that hook, not
 * on this directly. Backstopped server-side: `AppState::guard_mutation` refuses
 * every edit of such a project and `save_project` every save.
 */
export const selectNewerFileReadOnly = (s: Pick<ProjectStore, 'newerSchemaVersion'>): boolean =>
  s.newerSchemaVersion !== null;

/** What a mutation was issued against: see `lostProject` in the store. */
type IssuedAgainst = { key: string | null; file: string | null; session: number };

/** What every seam returns for a request whose project changed out from under it. */
const PROJECT_CHANGED = 'The project changed before this action could run.';

/** The four history verbs `executeUndoRedo` runs. */
type HistoryCommand = 'undo' | 'redo' | 'undo_to' | 'redo_to';

/** What a forward edit renumbered (`reverse_section`, `execute_batch`); absent on the rest. */
function reversedSectionsOf(outcome: object): readonly ReversedSection[] | undefined {
  return (outcome as { reversed_sections?: ReversedSection[] }).reversed_sections;
}

/** The commands that funnel through the backend's `finalize_deferred_import`. */
const IMPORT_COMMANDS = new Set([
  'import_sections',
  'execute_tml_import',
  'execute_compass_import',
  'execute_walls_import',
  'execute_gpx_import',
]);

/**
 * Learn, from a successful import, that this project's DEFERRED first save is
 * resolved - the file exists now.
 *
 * `finalize_deferred_import` (`commands/import.rs`) writes the `.stalx` on the
 * first import into a project created with `deferInitialSave`, for EVERY import
 * command. Only the DMP route ever told the store, because only it goes on to
 * call `saveAfterImport`; the TML/Compass/Walls/GPX dialogs finish through
 * `completeImportToSingleView`, which touches no save state at all. That left
 * `databaseOpen` false over a file that exists, and it is exactly the flag the
 * DB-backed surfaces read (`WarningResolution`'s resolve actions,
 * `DivePlanBar`'s plan persistence), so they stayed disabled for the rest of the
 * session.
 *
 * Two warnings mean the file is NOT usable, and both leave the flag alone:
 *  - `SYSTEM` - the deferred save's own failure ("Imported, but could not save
 *    the new project to disk"), which is how that path reports itself.
 *  - `DATABASE_NOT_REOPENED` - written, but the handle could not be reopened;
 *    `saveProject` already refuses to open the gate on it, for the same reason.
 *
 * Any other `SYSTEM` warning costs a false negative only - the flag stays as it
 * was, which is the direction that cannot enable a surface over a missing file.
 */
function noteImportResolvedDeferredSave(
  command: string,
  result: { warnings?: ValidationWarning[] },
  set: (partial: Partial<ProjectStore>) => void,
): void {
  if (!IMPORT_COMMANDS.has(command)) return;
  const unusable = (result.warnings ?? []).some(
    (w) => w.code === 'SYSTEM' || w.code === 'DATABASE_NOT_REOPENED',
  );
  if (!unusable) set({ databaseOpen: true });
}

/**
 * The frontend half of "no project is open" - every piece of project-derived state
 * that must not outlive the project it describes, plus the two sibling stores that
 * hold project-scoped state of their own.
 *
 * Shared by `closeProject` (the ordinary teardown, once the backend confirms the
 * close) and `deleteProject` (when the `.stalx` just handed to the OS delete was
 * the open one). The delete path can't reuse `closeProject` itself: that issues the
 * `close_project` IPC, and by then the backend has ALREADY torn its own session
 * down as part of the delete, so the round trip is pointless - and its `isClosing`
 * guard can answer `success: false`, which would leave the frontend still rendering
 * a project whose file is gone. Sharing the state reset instead keeps the two paths
 * from drifting.
 */
function clearOpenProjectState(set: (partial: Partial<ProjectStore>) => void): void {
  noteProjectSessionChanged();
  set({
    project: null,
    projectTreeGeneration: nextProjectTreeGeneration(),
    databaseOpen: false,
    newerSchemaVersion: null,
    speleoLinkUnreadable: false,
    unreadableProjectSettings: [],
    stations: [],
    surveyData: null,
    stationsAdjusted: null,
    surveyDataAdjusted: null,
    stats: null,
    frozenShots: [],
    isDirty: false,
    // [#576 FZ-FZ-INT-5] No project, no history to undo; the next create reads none either.
    canUndo: false,
    canRedo: false,
    autosaveFailure: null,
    autosaveFailureKind: null,
    integrityIssues: [],
    integrityDismissed: false,
    integritySaveBlocked: false,
    integrityDialogOpen: false,
    ...HISTORY_BANNER_CLEARED,
    ...LOAD_REPAIRS_CLEARED,
  });
  // Losing a project bypasses the setView() cleanup funnel, so drop the gas
  // sub-mode (#38) here too - a stale overlay/panel must not outlive the project.
  useUIStore.getState().exitGasPlan();
  // Same bypass, same fix, for the Export-mode option set: `applyDisplaySettings`
  // resets it on load/create, but a CLOSE never reaches that funnel, so without
  // this the next project opened from a closed state would still inherit the
  // previous one's format/DPI/threshold. `exitGasPlan()` above only leaves the
  // sub-mode; it does not clear either store's values.
  useExportStore.getState().reset();
  // #204: drop the SpeleoDB action-shell edit-state - no project is open.
  speleoState()?.clearLink();
  // The distance origin a delete displaced belonged to the project now gone.
  dropDormantDistanceOrigin();
}

export const useProjectStore = create<ProjectStore>((set, get) => {
  /** What a request was ISSUED against (#24): the project's key, the file it was open from and the session. */
  const issuedAgainst = (): IssuedAgainst => ({
    key: get().project?.created_at ?? null,
    file: get().project?.file_path ?? null,
    session: projectSessionEpoch(),
  });
  /**
   * True when the project a request was ISSUED against is no longer the open one (#24).
   *
   * [#576 FZ-FZ-INT-2] Two copies of one project file share `created_at`, so an edit
   * issued in one copy while a switch to the other was under way ran in the other.
   * Once the session moved (another open committed) the file must match too. A Save
   * As moves the file within the session, and a reload of the same file opens the
   * project the edit was made in: neither counts - the display-setting chain's rule
   * (`utils/settingWriteChain.ts`).
   */
  const lostProject = (issued: IssuedAgainst) => {
    const open = get().project;
    if (projectSwitched(issued.key, open?.created_at ?? null)) return true;
    return (
      issued.key !== null &&
      open !== null &&
      issued.session !== projectSessionEpoch() &&
      issued.file !== (open.file_path ?? null)
    );
  };
  /** Mark the project dirty, counting the change for a save in flight ([#576 FZ-FZ-INT-8], see saveProject). */
  const markDirty = () => {
    noteProjectDirtied();
    set({ isDirty: true });
  };
  /** Shared helper: invoke IPC command, refresh state on success, mark dirty. */
  const executeCommand = async <K extends MutationCommand>(
    command: K,
    typedArgs: QueuedCallArgs<K>,
    meta: { purpose: string; fields?: Record<string, unknown>; readOnlyNoticeInline?: boolean }
  ): Promise<CommandOutcome<IpcResult<K>>> => {
    // Checked against the command's manifest entry at the call site (#576 C3); forwarded as is.
    const args = typedArgs as Record<string, unknown> | undefined;
    // Captured when the user ISSUES the command, not when the queue gets round to it. The two
    // differ once a mutation can wait behind another: project load/create/close do not go
    // through the queue, so B can open while this entry is still waiting, and reading the
    // identity inside the callback would capture B and run A's section ids against B's
    // sections - which, since section ids are small per-project integers, would mostly RESOLVE.
    const issued = issuedAgainst();
    // GHOST-2 (#576), also at ISSUE time: a move carry's own commit claims its marker here;
    // any other survey mutation issued while a carry is in flight cancels the carry and is
    // refused before it can queue (src/store/moveCarry.ts). Renaming a project by path is a
    // survey mutation only for the OPEN project (`set_project_name_for_path`'s own gate).
    const carryCommit = takeMoveCarryCommit();
    const mutatesOpenSurvey = command !== 'set_project_name_for_path' || args?.path === get().project?.file_path;
    const refused = carryCommit || !mutatesOpenSurvey ? null : refuseMutationDuringCarry();
    if (refused) return refused;
    return runSerialized(() => executeCommandInner(command, args, meta, issued, carryCommit));
  };

  const executeCommandInner = async <K extends MutationCommand>(
    command: K,
    args: Record<string, unknown> | undefined,
    meta: { purpose: string; fields?: Record<string, unknown>; readOnlyNoticeInline?: boolean },
    issued: IssuedAgainst,
    carryCommit: MoveCarryCommit | null
  ): Promise<CommandOutcome<IpcResult<K>>> => {
    const requestProjectKey = issued.key;
    // Refuse before the IPC leaves: a command issued against a project that is no longer open
    // must not mutate whatever replaced it.
    if (lostProject(issued)) return refusedLocally(PROJECT_CHANGED);
    try {
      // #310: anchor the gas-plan pins against the survey as it stands BEFORE this edit - a pin
      // placed a moment ago may still be sitting on the debounce. No-op for a plan with no pins.
      // The distance-gradient origin is anchored alongside (#576 D39(g)); no-op at the entrance.
      const [, origin] = await Promise.all([flushPlanAnchors(), captureDistanceOrigin()]);
      // Re-checked AFTER the flush: it is a real IPC round trip, and project load/create/close do
      // not go through the mutation queue, so a project can open while it is in flight. The guard
      // above ran before that window existed, and `args` were captured against the OUTGOING
      // project - whose section ids, being small per-project integers, would mostly RESOLVE in
      // whatever is now open.
      if (lostProject(issued)) return refusedLocally(PROJECT_CHANGED);
      // GHOST-2 R3: a carry's commit that queued behind another mutation, or outlived the
      // survey it was computed against, is refused here - the last moment before IPC.
      const staleCarryCommit = refuseStaleCarryCommit(carryCommit, get().projectTreeGeneration);
      if (staleCarryCommit) return staleCarryCommit;
      // The backend's refusal arrives as the rejection's CommandError and becomes the refused
      // outcome; any other rejection (a transport failure) is the `catch` below. The SpeleoDB
      // link bits on either answer were applied by `invokeIpc` (#576 D22).
      const result = await settleCommand((invokeIpc as ForwardedInvoke)(command, args, meta));
      if (result.success) {
        // Re-resolve those pins BEFORE the refetch: the reach-field effect keys on modified_at,
        // so a stale pin sent here would paint a route through the wrong station for a frame.
        if (!lostProject(issued)) await Promise.all([remapPlanPins(), remapDistanceOrigin(origin, requestProjectKey)]);
        await get().refreshProjectState();
        // A reverse's aftermath (clearing the selection, the merge staging and the comment
        // pickup it renumbered) runs HERE rather than in the caller, so a reverse and an undo
        // can never interleave: both routes apply it in the same place, in completion order,
        // immediately after the refresh they belong to. (The distance-gradient origin is re-resolved
        // from its anchor above, #576 D39(g); it is mirrored here only when that anchor could not be
        // taken.)
        if (!lostProject(issued)) {
          applyReverseAftermath(composeReverseTransforms(reversedSectionsOf(result), get().project?.sections ?? []), origin);
        }
        markDirty();
        noteImportResolvedDeferredSave(command, result, set);
        // A command can RESOLVE (or surface) an integrity issue - e.g. removing a
        // drifted loop closure clears its banner entry - so re-check, mirroring
        // executeUndoRedo. refreshProjectState doesn't re-check, and the next save
        // may be an autosave (which skips its own post-save re-check to avoid churn),
        // so without this the banner/dialog can keep showing an already-fixed issue
        // until a manual save or reload.
        await get().checkProjectIntegrity();
      } else {
        // executeCommand used to be SILENT on a refused command (no refresh,
        // no feedback). That left optimistic UI desynced from backend truth - the
        // #191 read-only edit gate's "shots vanish on drag" bug: a committed drag
        // preview stayed rendered because the success-only refresh never fired.
        // ALWAYS re-sync the view to backend truth, discarding the phantom edit.
        await get().refreshProjectState();
        // Surface a read-only rejection (EDIT_DISABLED) as a single friendly notice
        // across ALL edit paths (drag, dialogs, shortcuts) - a blocked drag was
        // otherwise a silent no-op. Other failures keep their existing handling
        // (dialogs show an inline error), so validation errors aren't double-toasted.
        // Full edit-state UX (suppress this, offer "Start editing") is owned by #204.
        // #202: a re-verify divergence (lost lock / moved tip) routes to the
        // divergence prompt instead of the plain read-only notice - the dialog is
        // the feedback, and the backend already dropped its edit gate.
        if (isReverifyDivergedCode(result.code)) {
          speleoState()?.notifyReverifyDivergence();
        } else if (isEditGateCode(result.code)) {
          // ...unless the caller shows it inline itself (`importSections`).
          if (!meta.readOnlyNoticeInline) {
            useUIStore.getState().showToast(READ_ONLY_EDIT_NOTICE(result.code), TOAST_KEYS.EDIT_DISABLED, 'warning');
          }
        } else {
          // Catch-all (docs/notifications.md): a failure no specific handler
          // claims must not die silently. Dialogs own their inline errors and
          // are open when they issue commands, so the catch-all yields to them.
          // The context menu is the exception: it issues commands but renders
          // no errors (its actions await the command before closing), so it
          // must not suppress.
          const dialog = useUIStore.getState().activeDialog;
          if (dialog === null || dialog === 'context-menu') {
            useUIStore
              .getState()
              .showToast(result.error ?? i18n.t('editGate.commandFailed'), TOAST_KEYS.COMMAND_FAILED, 'error');
          }
        }
      }
      return result;
    } catch (e) {
      // A genuine IPC/transport rejection (rare - distinct from the backend's coded
      // refusal) would otherwise skip the re-sync above and strand an
      // optimistic edit (a committed drag preview) hidden forever - the same
      // "vanishing shots" failure. Re-sync the view first (the re-render drains
      // the scheduled drag-preview restore), THEN re-throw so every caller's
      // existing error handling (dialog catch blocks, etc.) is unchanged.
      await get().refreshProjectState();
      throw e;
    }
  };

  /** Shared helper for undo/redo operations: invoke, toast on failure, always refresh. */
  const executeUndoRedo = async <K extends HistoryCommand>(
    command: K,
    typedArgs: QueuedCallArgs<K>,
    meta: { purpose: string; fields?: Record<string, unknown> },
    toastId: string,
  ): Promise<CommandOutcome<HistoryOutcome>> => {
    // Captured at ISSUE time - see `executeCommand`, which also explains the GHOST-2 check.
    const issued = issuedAgainst();
    const refused = refuseMutationDuringCarry();
    if (refused) return refused;
    const args = typedArgs as Record<string, unknown> | undefined;
    return runSerialized(() => executeUndoRedoInner(command, args, meta, toastId, issued));
  };

  const executeUndoRedoInner = async (
    command: HistoryCommand,
    args: Record<string, unknown> | undefined,
    meta: { purpose: string; fields?: Record<string, unknown> },
    toastId: string,
    issued: IssuedAgainst,
  ): Promise<CommandOutcome<HistoryOutcome>> => {
    const requestProjectKey = issued.key;
    if (lostProject(issued)) return refusedLocally(PROJECT_CHANGED);
    let result: CommandOutcome<HistoryOutcome>;
    // #24: undoing (or redoing) a reverse renumbers station i back to N - i, so
    // every positional StationRef the frontend holds - the gas-plan pins, the
    // distance-gradient origin - keeps its index and starts naming the OPPOSITE
    // physical station, silently and persistably. The backend reports what it
    // renumbered on the outcome (`reversed_steps`: undo/redo of a reverse re-reverses
    // the same sections), one entry per reversing STEP with the shot count in force at
    // it, so compose them and re-mirror once the refreshed state lands. Empty on
    // everything else, which is the common case and costs nothing.
    let reversedSections: readonly ReversedSection[] | undefined;
    let origin: OriginCapture | null = null; // #576 D39(g), see executeCommandInner
    try {
      // #310, see executeCommandInner. An undo/redo can also restore the station a delete took
      // the origin from: its dormant anchor is re-resolved too (#576 D39(g)-UNDO).
      [, origin] = await Promise.all([flushPlanAnchors(), captureDistanceOrigin(requestProjectKey)]);
      // Re-checked after the flush for the same reason executeCommandInner does: the flush is a
      // real round trip, and a project can open across it while `args` still name the outgoing
      // one. Returning from inside the `try` still runs the `finally`, which is correct rather
      // than merely tolerable: its remap and its aftermath carry the same guard and skip
      // themselves, and the refresh + integrity re-check simply re-read whatever is now open -
      // exactly what already happens today when the switch is caught one guard later.
      if (lostProject(issued)) return refusedLocally(PROJECT_CHANGED);
      // An undo/redo mutates the remote project too: its SpeleoDB link bits ride on the
      // answer, success or failure, and `invokeIpc` applied them (#576 D22).
      result = await settleCommand((invokeIpc as ForwardedInvoke)(command, args, meta));
      if (result.success) {
        reversedSections = result.reversed_steps;
        markDirty();
      } else {
        // `HISTORY_STEP_FAILED`: a step RAN and failed - "failed at step N of M" of a
        // jump (`history_step`), or a single undo/redo past the empty-stack check (1 of
        // 1, #576 NF-W2S-2). The failing step can mutate before it reports, so EVERY
        // such failure changed the project, step 1 included (#576 A18(a)). Every other
        // refusal (nothing to undo, the edit gate, the re-verify divergence, the
        // refusal after an internal error) ran no step.
        const failedStep = result.code === 'HISTORY_STEP_FAILED' ? (result.failure?.history_step?.failed_step ?? 1) : null;
        // `INTERNAL_ERROR` on a single undo/redo: its body crashed, possibly with the step
        // half-run, so it counts as having changed the project (#576 C4: before the coded
        // envelope it was an untagged failure, which dirtied). A jump's crash reports no
        // failed step and keeps the jump's rule: only `HISTORY_STEP_FAILED` dirties it.
        const singleStepCrashed = result.code === 'INTERNAL_ERROR' && (command === 'undo' || command === 'redo');
        if (failedStep !== null || singleStepCrashed) markDirty();
        if (failedStep !== null && failedStep > 1) {
          // A partial multi-step failure still renumbered: the steps BEFORE the
          // failing one already flipped their sections and cannot be rolled back,
          // which is why the backend attaches the entries it applied to the
          // failure too. Dropping them here would leave the plan pins and the
          // distance origin mirrored onto the opposite physical stations with no
          // error the user could connect to it.
          reversedSections = result.failure?.reversed_sections;
        }
        // A read-only rejection (an edit-gate code) gets the same friendly notice +
        // dedup key as executeCommand - mashing Ctrl+Z on a read-only remote project
        // shows one tidy toast. A #202 re-verify divergence routes to the divergence
        // prompt instead.
        if (isReverifyDivergedCode(result.code)) {
          speleoState()?.notifyReverifyDivergence();
        } else if (isEditGateCode(result.code)) {
          useUIStore.getState().showToast(READ_ONLY_EDIT_NOTICE(result.code), TOAST_KEYS.EDIT_DISABLED, 'warning');
        } else {
          useUIStore.getState().showToast(result.error ?? i18n.t('editGate.commandFailed'), toastId, 'error');
        }
      }
    } catch (e) {
      result = refusedLocally(String(e));
      useUIStore.getState().showToast(result.error, toastId, 'error');
    } finally {
      // #310: re-resolve the plan pins before the refetch (see executeCommandInner). In the
      // `finally` because a PARTIAL multi-step undo renumbered too, and its pins need it just
      // as much as a clean one's do. The distance-gradient origin likewise (#576 D39(g)).
      if (!lostProject(issued)) await Promise.all([remapPlanPins(), remapDistanceOrigin(origin, requestProjectKey)]);
      await get().refreshProjectState();
      // #24: same project-switch guard as executeCommand. Section ids are small per-project
      // integers, so entries from the OUTGOING project would otherwise match sections in
      // whatever is now open and mirror that project's origin instead.
      // The per-step counts come off the wire; the REFRESHED project supplies only
      // the range to clamp into, which is the numbering the origin now has to fit.
      // A multi-step undo_to can walk past a CreateShot, so the post-range count
      // is NOT the divisor the earlier steps mirrored against - composing the
      // entries is what keeps those steps landing on the right station.
      if (!lostProject(issued)) {
        applyReverseAftermath(composeReverseTransforms(reversedSections, get().project?.sections ?? []), origin);
      }
      // Undo/redo can change which cross-references resolve - e.g. undoing a
      // Repair restores the dropped dangling refs, redoing a delete removes a
      // station an offset anchored on - so re-check integrity, otherwise the
      // banner and save-block go stale (showing clean while the project isn't).
      await get().checkProjectIntegrity();
    }
    return result;
  };

  return {
  // Initial state
  project: null,
  projectTreeGeneration: 0,
  databaseOpen: false,
  newerSchemaVersion: null,
  speleoLinkUnreadable: false,
  unreadableProjectSettings: [],
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
  ...HISTORY_BANNER_CLEARED,
  ...LOAD_REPAIRS_CLEARED,
  config: null,
  loading: { ...initialLoadingState },
  autosaveFailure: null,
  autosaveFailureKind: null,
  canUndo: false,
  canRedo: false,
  isDirty: false,
  isClosing: false,

  // Config (loadConfig / updateConfig / updateGasConfig / setConfig live in projectConfigActions.ts)
  ...createProjectConfigActions(set, get),
  ...createProjectHistoryBannerActions(set, get),
  ...createProjectLoadRepairsActions(set),
  // Warnings and integrity (refreshWarnings / resolveWarning / unresolveWarning /
  // checkProjectIntegrity / repairProjectIntegrity live in projectIntegrityActions.ts)
  ...createProjectIntegrityActions(set, get),
  // exportCsv / exportJson / exportTml / exportImage live in projectExportActions.ts
  ...createProjectExportActions(),

  // Loading state actions
  startLoading: (projectName: string, projectPath: string) => {
    clientOpIdCounter += 1;
    const newClientOpId = clientOpIdCounter;
    set({
      loading: {
        ...initialLoadingState,
        loadingId: 0, // Will be set from first progress event or result
        clientOpId: newClientOpId,
        isLoading: true,
        projectName,
        projectPath,
        phase: 'opening_database',
        lastEventTime: Date.now(),
      },
      autosaveFailure: null, // A new project's autosave state starts clean
      autosaveFailureKind: null,
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

    const promise: Promise<LoadProjectOutcome> = runProjectSwitch(async () => {
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

        // Check clientOpId hasn't changed (cancelled/new load started). Reported
        // as SUPERSEDED, not as a plain failure: the caller's rollback (if it has
        // one) would land on a project this call never opened.
        if (get().loading.clientOpId !== clientOpId) {
          clearProjectActionSource();
          return { success: false, superseded: true };
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
        // [#576 NF-FIX5-1] A new session: an answer issued for the previous project is stale.
        noteProjectSessionChanged();
        set({
          project: result.project,
          projectTreeGeneration: nextProjectTreeGeneration(),
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
          // [#576 B6-D1] The backend's own read-only verdict for THIS load, in the
          // same set() as the project: a newer file is read-only from the first frame.
          newerSchemaVersion: result.newerSchemaVersion,
          speleoLinkUnreadable: result.speleoLinkUnreadable,
          unreadableProjectSettings: result.unreadableProjectSettings,
          // [#576 B35(h)] What this load reset or dropped: one notice after the open.
          ...loadRepairsStateFor(result.loadRepairs),
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
          ...HISTORY_BANNER_CLEARED,
        });
        const tSetDone = performance.now();
        recordPerfTiming('store set(project+stations)', tSetDone - tSetStart);

        // #270: resolve the SpeleoDB edit gate BEFORE `setView('single')` paints
        // the project. `refreshLinkStatus` below is fired unawaited, so until it
        // answers the shell used to have no idea what this project was - and the
        // read-only selector read "no idea" as editable, showing a locked remote
        // project with every edit affordance on. `result.isRemote` is the backend's
        // own `EXISTS(speleo_link)` verdict for THIS load - the very value it set
        // its own edit gate from - so the two gates cannot disagree, and it costs
        // no round trip: a remote project is read-only from the first frame and a
        // local one is editable from the first frame (no read-only flash).
        // `path` identifies WHICH project this link is for, so a link left over
        // from the previously-open remote project is replaced rather than reused.
        speleoState()?.syncProjectLink(result.isRemote, path);

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
        // An undo in this project must not bring back the previous one's distance origin
        // (#576 RV4-FE-FEA2) - nor this project's own, from before it was reopened.
        dropDormantDistanceOrigin();
        const tViewStart = performance.now();
        useUIStore.getState().setView('single');
        const tViewDone = performance.now();
        recordPerfTiming('setView(single)', tViewDone - tViewStart);

        // Update recent projects list
        await get().loadRecentProjects();

        get().resetLoading();
        await get().refreshUndoState();
        recordPerfTiming('loadProject total', performance.now() - tStart);

        // #204: refine the SpeleoDB action-shell edit-state (permission / lock
        // holder / remote state) from the backend. Best-effort and non-blocking -
        // it is only a REFINEMENT now: `syncProjectLink` above already settled the
        // gate itself, so a slow or failing status call can no longer leave a
        // remote project looking editable. Deliberately not awaited: for a remote
        // project this command also makes best-effort `get_project` /
        // `get_revisions` HTTP calls, and no open should wait on the network.
        void speleoState()?.refreshLinkStatus();
        // Also refresh who we are, so the action button can tell our own held lock
        // from someone else's (otherwise a reopen-from-recents has no session and
        // every held lock reads as "locked by other").
        void speleoState()?.refreshSession();

        // #27: shots whose stored `kind` this build could not read. The backend
        // already failed them closed to Reference (so nothing routes through a
        // leg it could not parse) and hands them over exactly once, but that is
        // a substitution the user never agreed to - ask what they should be.
        // Staleness-guarded: a user who switched projects before this resolved
        // must not get a dialog about a file that is no longer open. The list
        // has already been consumed from the backend, so losing this dialog
        // loses the prompt for the session, leaving substitutions unreviewed.
        try {
          const found = await invokeIpc(
            'take_unrecognised_shot_kinds',
            // Scoped to THIS load: a cancelled load runs on to its next
            // checkpoint, so without the id an abandoned project's shot ids
            // could be handed to the one actually open.
            { loadingId: result.loadingId },
            { purpose: 'resolve unreadable shot kinds' }
          );
          const st = get();
          if (st.project !== null && st.loading.clientOpId === clientOpId && found.length > 0) {
            useUIStore.getState().openDialog('unrecognised-shot-kind', { shots: found });
          }
        } catch {
          /* best-effort: the shots are already safely Reference either way */
        }

        return { success: true };
      } catch (e) {
        // The load's refusal is its CommandError (`code`, `details`, `speleodb_managed`);
        // anything else (a decode failure, a transport error) has no code.
        const error = commandErrorOf(e);
        const code = error?.code ?? null;
        const message = getErrorMessage(e);

        // Handle ALREADY_OPEN specially - not an error, just switch view
        if (code === 'ALREADY_OPEN') {
          // The same staleness guard both neighbouring branches carry (the
          // success path above, the error path below). Without it this branch
          // acted for a load that had already lost its slot: it flipped the view
          // and called `resetLoading()`, which tears down the loading modal of the
          // NEWER load still running, and then reported success - a boolean
          // `keepLocalCopy` / `finishOpen` now act on.
          if (get().loading.clientOpId !== clientOpId) {
            clearProjectActionSource();
            return { success: false, superseded: true };
          }
          // #270 (hardening): resolve the edit gate SYNCHRONOUSLY when nothing has
          // resolved it, mirroring the success path's `syncProjectLink` before the
          // view paints. `refreshLinkStatus` below refines it, but it is unawaited
          // and swallows its errors, so with `unknown` left standing a failed
          // status call renders the project permanently read-only under the
          // fail-closed gate AND leaves no status button to act from (it arms its
          // recovery poll on `link !== null`). Seeding remote is the pessimistic
          // direction - read-only, with a way out on screen - and the status reply
          // resolves it either way (a non-remote status seeds `local`).
          if (speleoState()?.projectLinkKind === 'unknown') {
            speleoState()?.syncProjectLink(true, path);
          }
          useUIStore.getState().setView('single');
          get().resetLoading();
          await get().refreshUndoState();
          // #270: then re-resolve from the backend, which knows what the short
          // circuit cannot - the lock holder, the permission, the remote state,
          // and whether a link seeded on the way in still matches the server. That
          // mattered because this branch RENDERS a project while returning a
          // success the callers act on: the refused-open prompt's "Keep the local
          // copy" used to seed a pessimistic read-only link over a project we hold
          // the lock on, and this branch preserved it verbatim - a read-only survey
          // UI and a disarmed quit guard that healed only on the next status poll
          // (Survey mode only, 30 s to 24 h).
          //
          // Unawaited and best-effort on purpose: `speleodb_link_status` does
          // blocking HTTP for a remote project, and no caller of `loadProject` may
          // be stalled behind the network. So it is a REFINEMENT, never the fix -
          // the seeding side (`keepLocalCopy`, and now every route through
          // `finishOpen`) is correct on its own, offline included, and the
          // fail-closed fallback above covers the case nothing else resolved.
          void speleoState()?.refreshLinkStatus();
          return { success: true };
        }

        // Handle a permission denial - folder-access guidance, not a corruption/error.
        // Checked before CORRUPT so a denial doesn't masquerade as corruption.
        if (code === 'FS_PERMISSION_DENIED') {
          get().resetLoading();
          useUIStore.getState().openDialog('save-permission', {
            detail: message,
            context: 'open',
          });
          clearProjectActionSource();
          return { success: false };
        }

        // Handle CORRUPT - open recovery dialog instead of generic error
        if (code === 'CORRUPT' && error?.details) {
          get().resetLoading();
          useUIStore.getState().openDialog('corruption-recovery', {
            corruptPath: error.details,
            errorMessage: message,
          });
          clearProjectActionSource();
          return { success: false };
        }

        // Canonical-storage refusal: this .stalx is a non-canonical copy of a
        // SpeleoDB project. Backend-enforced (recents clicks bypass the
        // routeOpenedFile guards); the prompt offers the two real ways forward.
        if (code === 'SPELEODB_MANAGED' && error?.speleodb_managed) {
          // A SUPERSEDED load must not speak. Phase 0 refuses before the loader
          // reaches its first cancellation checkpoint, so a load the user has
          // already navigated away from really can arrive here - and resetting
          // the loading state would discard the CURRENT load's progress, while
          // the prompt would be about a file nobody is opening any more. The
          // newer load owns both the loading state and the action source.
          if (get().loading.clientOpId !== clientOpId) return { success: false };
          get().resetLoading();
          useUIStore.getState().openDialog('speleodb-managed-file', {
            managed: error.speleodb_managed,
          });
          clearProjectActionSource();
          return { success: false };
        }

        // Only set error if this is still the current load operation
        if (get().loading.clientOpId === clientOpId) {
          get().setLoadingError(message, error?.details);
        }
        clearProjectActionSource();
        return { success: false };
      }
    });

    inFlightLoads.set(path, promise);
    void promise.finally(() => {
      if (inFlightLoads.get(path) === promise) inFlightLoads.delete(path);
    });
    return promise;
  },

  // Project operations - uses consolidated create_project_full command
  createProject: (name: string, projectLocation?: string, options?: CreateProjectOptions) =>
    runProjectSwitch(async () => {
    // [#576 RV15-2] A create that names its file or folder is judged by the
    // write-path policy before anything else (the backend does the same, RV14-1):
    // one it refuses must not stop the open or load in progress, nor show the
    // loading state, on its way to the error. The create checks again.
    if (options?.savePath !== undefined || options?.saveDirectory !== undefined) {
      try {
        await invokeIpc(
          'check_create_target',
          { name, saveDirectory: options.saveDirectory, savePath: options.savePath },
          { purpose: 'check the new project file is somewhere the app may write', fields: { name } }
        );
      } catch (e) {
        clearProjectActionSource();
        return emptyCommandResult(false, getErrorMessage(e));
      }
    }

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
      const result = await invokeIpc(
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
        // Superseded/cancelled - drop the entry-point source so it can't
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
      noteProjectSessionChanged(); // [#576 NF-FIX5-1] see loadProject
      set({
        project: result.project,
        projectTreeGeneration: nextProjectTreeGeneration(),
        // A deferred new project has no on-disk database until the first save.
        databaseOpen: !deferred,
        // This build created it, so it is this build's to edit (#576 B6-D1).
        newerSchemaVersion: null,
        speleoLinkUnreadable: false,
        unreadableProjectSettings: [],
        stations: [],
        stationsAdjusted: null,
        surveyDataAdjusted: null,
        surveyData: emptySurveyData(),
        stats: null,
        frozenShots: [],
        warnings: [],
        isDirty: false,
        // [#576 FZ-FZ-INT-5] A new project has no history: the previous project's
        // Undo/Redo availability must not carry over (nothing on this path re-reads it).
        canUndo: false,
        canRedo: false,
        // Reset integrity state on project switch (see loadProject).
        integrityIssues: [],
        integrityDismissed: false,
        integritySaveBlocked: false,
        integrityDialogOpen: false,
        ...HISTORY_BANNER_CLEARED,
        ...LOAD_REPAIRS_CLEARED,
      });

      // #270: a freshly created project is local by construction - it has no
      // `speleo_link` row until a later Publish - and saying so before the view
      // renders is what keeps the pessimistic default from leaving it read-only:
      // nothing on this path ever calls `refreshLinkStatus`.
      speleoState()?.syncProjectLink(false);

      // Apply display settings from backend (defaults for new project)
      useUIStore.getState().applyDisplaySettings(result.displaySettings);
      // Reset the Details panel to the project-level view on open (#54).
      useUIStore.getState().clearSelection();
      // Stale tool state must not carry into the new project either (#275 B1).
      useUIStore.getState().resetToolTransientsForProjectSwitch();
      dropDormantDistanceOrigin();
      useUIStore.getState().setView('single');
      await get().loadRecentProjects();
      get().resetLoading();

      return emptyCommandResult(true, null);
    } catch (e) {
      // Create failed - drop the entry-point source (no auto-reset) so it can't
      // mis-attribute a later, unrelated project command. Mirrors loadProject.
      clearProjectActionSource();
      const message = getErrorMessage(e);
      if (get().loading.clientOpId === clientOpId) {
        get().setLoadingError(message, commandErrorOf(e)?.details);
      }
      return emptyCommandResult(false, message);
    }
    }),

  saveProject: async (path?: string, isAutosave?: boolean) => {
    // The Rust save path heals duplicate shot UUIDs / orphan section offsets /
    // non-finite geometry in memory before persisting, which mutates the backend
    // project - so afterward the frontend's mirror can be stale. Those heals only
    // ever fire on a project the integrity check already flags, so capture that here
    // as a cheap signal to refresh below, avoiding a get_project_state_binary on every
    // clean save / autosave.
    const hadIntegrityIssues = get().integrityIssues.length > 0;
    // [#576 FZ-FZ-INT-8] The changes acknowledged so far: one acknowledged after this
    // point may have run after the save wrote the file, so it keeps the project dirty.
    const dirtyMarksAtIssue = projectDirtyMarks();
    // A refused save is the rejection's CommandError, settled into the refused outcome
    // (its `code` routes it below); a transport failure still rejects.
    const result = await settleCommand(
      invokeIpc(
        'save_project',
        { path, isAutosave: isAutosave === true },
        { purpose: 'persist project changes', fields: { path, isAutosave } }
      )
    );
    if (result.success) {
      // A detached copy was written; the OPEN project was not saved. Do not
      // re-point file_path and do not clear isDirty (see SaveOutcome).
      if (result.unlinked_copy_path) {
        useUIStore
          .getState()
          .showToast(
            i18n.t('speleodb.managedFile.copySaved', { path: result.unlinked_copy_path }),
            TOAST_KEYS.SPELEODB_UNLINKED_COPY,
            'warning'
          );
        // [#576 A3-D2] What the copy went without (e.g. its display settings).
        surfaceSaveWarnings(result.warnings);
        // The heals above ran on the LIVE project before the pipeline decided to
        // write a copy, so the frontend's mirror is stale here too - and the
        // healed state reached only the copy, never the managed file, which
        // leaves the session dirty even if it entered the save clean.
        if (hadIntegrityIssues) {
          await get().refreshProjectState();
          markDirty();
        }
        return result;
      }
      // Update file_path if the backend returned it (first save / save-as)
      if (result.saved_path) {
        const project = get().project;
        // Only mint a new project object when the path ACTUALLY changed (first
        // save / Save As). A successful save always echoes `saved_path` (Rust
        // `SaveOutcome::saved_path`), so an autosave to the same
        // path would otherwise re-spread the project on every debounced write.
        // That changes the project's identity, and MapCanvas's republish
        // useLayoutEffect keys on it - a no-op save would trigger a full survey
        // rebuild + republish, briefly blanking all geometry (#mem).
        if (project && project.file_path !== result.saved_path) {
          // The session moved to that file, written without the unreadable link
          // (#576 B35(b)), so saving over the open file works again.
          set({ project: { ...project, file_path: result.saved_path }, speleoLinkUnreadable: false });
          // [#576 FZ-FZ-INT-9] A refresh read before the save moved the file still
          // names the old one, and landing after this it put the old path back. Drop
          // every refresh issued so far and read the state once more, after the move.
          refreshAppliedRequest = refreshRequestCounter;
          await get().refreshProjectState();
        }
      }
      // Lightweight: just refresh undo/redo availability
      await get().refreshUndoState();
      // The first successful save opens the on-disk database for a deferred project
      // - UNLESS the backend saved the file but couldn't reopen the DB (it clears
      // the handle and flags DATABASE_NOT_REOPENED). In that case keep
      // databaseOpen=false so DB-backed actions stay disabled (issue #57 / Codex).
      const dbNotReopened = result.warnings?.some((w) => w.code === 'DATABASE_NOT_REOPENED') ?? false;
      set({
        isDirty: projectDirtyMarks() === dirtyMarksAtIssue ? false : get().isDirty,
        databaseOpen: !dbNotReopened,
        autosaveFailure: null,
        autosaveFailureKind: null,
      });
      // [#576 A3-D2] Saved, but not everything: the dropped undo history, the
      // display settings, a file that could not be reopened - told, not silent.
      surfaceSaveWarnings(result.warnings);
      // If the save healed the project in memory (re-minted duplicate UUIDs,
      // dropped orphan section offsets, sanitized non-finite geometry), the
      // frontend's shots/stations are now stale vs the saved file - refresh so the
      // rendered state matches. Gated on hadIntegrityIssues so a clean save/autosave
      // skips the extra get_project_state_binary fetch and re-render.
      if (hadIntegrityIssues) {
        await get().refreshProjectState();
      }
      // Don't let geometric corruption persist silently: re-check after save so
      // drifted merges surface in the integrity banner this session, not only on
      // the next load. Skip on a clean autosave to avoid churn - BUT still
      // re-check when the project entered the save flagged (hadIntegrityIssues),
      // because the save path auto-heals some issues (duplicate UUIDs / orphan
      // offsets) on autosave too; without this the banner would stay stale after
      // an autosave silently cleared them.
      if (!isAutosave || hadIntegrityIssues) {
        void get().checkProjectIntegrity();
      }
    } else {
      const permission = isPermissionError(result.code);
      // An autosave failure is an ongoing condition, not a modal moment: feed
      // the banner (docs/notifications.md). `autosaveFailureMessage` rewords the
      // coded destination failures - the banner renders this string.
      // Gated on the project still being dirty for the same reason useAutoSave
      // gates its red status: a manual Ctrl+S that landed while this autosave was
      // in flight already put the changes on disk, and a banner saying the latest
      // changes are not on disk on a clean project is a claim the user cannot
      // clear by saving (#429).
      if (isAutosave && get().isDirty) {
        set({
          autosaveFailure: autosaveFailureMessage(result),
          autosaveFailureKind: saveErrorKind(result.code),
        });
      }
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
      // `integritySaveBlocked` is up to date by the time saveProject resolves -
      // the auto-save hook reads it to decide whether to pause, and
      // surfaceSaveFailure reads it to route a blocked save to Review & Repair.
      // (On a permission error this is a harmless no-op: the project is
      // structurally fine, so integritySaveBlocked stays false.)
      await get().checkProjectIntegrity();
    }
    return result;
  },

  clearAutosaveFailure: () => set({ autosaveFailure: null, autosaveFailureKind: null }),

  saveAfterImport: async (): Promise<boolean> => {
    // Import into an existing project: just mark dirty so we don't silently
    // commit the user's unrelated pending edits. The existing project already
    // has a file + Recent Projects entry; the normal unsaved-changes / autosave
    // flow persists the import, and the close-guard prevents silent loss.
    // (#204/fix 8, #303: the SpeleoDB link bits of the import itself rode back on the
    // import command's answer, #576 D22.)
    if (useUIStore.getState().importContext !== 'new-project') {
      markDirty();
      return true;
    }
    // New project: the initial save is deferred, so the import is invisible until
    // a real save writes the .stalx and adds it to Recent Projects. Do that now.
    // (The backend import_sections command also resolves this as a durable
    // safety net; this gives the immediate, complete save with correct dirty state.)
    let result: CommandOutcome<SaveOutcome>;
    try {
      result = await get().saveProject();
    } catch (e) {
      // A reject (e.g. IPC transport failure) means saveProject never ran its
      // integrity recheck, so `integritySaveBlocked` may be stale -
      // skipIntegrityRouting keeps the router from wrongly opening Review &
      // Repair. The project stays dirty (close-guarded).
      console.error('[import] save after import failed:', e);
      markDirty();
      surfaceSaveFailure(saveFailureOf(e), useUIStore.getState().openDialog, get(), {
        skipIntegrityRouting: true,
      });
      return false;
    }
    if (!result.success) {
      // The import isn't on disk: reflect the unsaved state (not a phantom
      // "saved") and surface the actionable error. Signal the caller NOT to close
      // the dialog, so it doesn't dismiss the error/permission guidance.
      markDirty();
      surfaceSaveFailure(
        { code: result.code, error: result.error || 'Failed to save the imported project.' },
        useUIStore.getState().openDialog,
        get(),
      );
      return false;
    }
    return true;
  },

  closeProject: (opts) => {
    // Guard against concurrent close operations (rapid Back clicks or double Discard)
    if (get().isClosing) {
      debugLog('[projectStore] closeProject ignored - already closing');
      return Promise.resolve(refusedLocally('Already closing'));
    }

    set({ isClosing: true });
    return runProjectSwitch(async () => {
      try {
        const result = await settleCommand(
          invokeIpc(
            'close_project',
            // Default: release a held SpeleoDB lock so the close doesn't strand it
            // (fix 1). The keep-lock close path passes { releaseLock: false }.
            { releaseLock: opts?.releaseLock ?? true },
            { purpose: 'close project and release resources' },
          ),
        );
        if (result.success) {
          clearOpenProjectState(set);
        }
        return result;
      } finally {
        set({ isClosing: false });
      }
    });
  },

  isShotFrozen: (sectionId: number, shotIndex: number) => {
    return get().frozenShots.some(
      (f) => f.section_id === sectionId && f.shot_index === shotIndex
    );
  },

  refreshProjectState: async () => {
    try {
      debugLog('[projectStore] refreshProjectState: invoking get_project_state_binary...');
      // [#576 A9-D2] The load's binary envelope, decoded by the load's decoder: the
      // SoA `surveyData` comes straight from the typed arrays and the sections carry
      // lazy `shots`, exactly as after a load. Null when no project is open.
      refreshRequestCounter += 1;
      const request = refreshRequestCounter;
      const session = projectSessionEpoch();
      const { fetchProjectStateBinary } = await import('../utils/loadProjectBinary');
      const state = await fetchProjectStateBinary();
      debugLog('[projectStore] refreshProjectState: got state, sections:', state?.project.sections.length);
      // [#576 RV5-8] A newer refresh already landed: this answer is older than
      // what the store shows, so applying it would roll the project back.
      // A newer "no project open" answer counts too: it supersedes an older state.
      // [#576 NF-FIX5-1] So does a project switch: a load, create or close since
      // this refresh was issued replaced the project it describes, and none of them
      // runs a refresh of its own for the guard above to order against.
      const superseded = request < refreshAppliedRequest || session !== projectSessionEpoch();
      if (!superseded) refreshAppliedRequest = request;
      if (state && !superseded) {
        set({
          project: state.project,
          projectTreeGeneration: nextProjectTreeGeneration(),
          stations: state.stations,
          surveyData: state.surveyData,
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

  loadRecentProjects: async () => {
    try {
      // [#576 HFI-12] With the list, the recent PATHS. A recent that no longer loads
      // (moved, deleted) leaves the list, and the Global View shows it as missing
      // only if the config's recents name it - but the store's copy was read before
      // every project opened or created since, so such a project just vanished until
      // a config reload. Both land in one `set`: no render sees one without the other.
      const recentPaths = readRecentPathsInOrder();
      // [perf] Hot path on every navigation to GlobalView. Surfaced in the
      // memory probe panel to confirm the SQL-aggregate fast path stays fast.
      const t0 = performance.now();
      const recent = await invokeIpc('list_recent_projects', undefined, {
        purpose: 'load recent projects list',
      });
      const ms = performance.now() - t0;
      const { recordPerfTiming } = await import('../utils/memoryProbe');
      recordPerfTiming('list_recent_projects IPC', ms);
      const paths = await recentPaths;
      set((s) => ({ recentProjects: recent, ...withRecentPaths(s.config, paths) }));
    } catch (e) {
      console.error('Failed to load recent projects:', e);
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
      const result = await settleCommand(
        invokeIpc('remove_from_recent_projects', { path }, { purpose: 'update recent projects list', fields: { path } })
      );
      if (result.success) {
        // Refresh both the loaded list and the config after removal. The config
        // refresh matters for "missing" recents (paths in config.general
        // .recent_projects that didn't load): those rows are derived from config,
        // so without this the removed path lingers and the row re-renders.
        await Promise.all([get().loadRecentProjects(), get().reloadConfigInOrder()]);
      }
      return result;
    } catch (e) {
      console.error('Failed to remove from recent projects:', e);
      return refusedLocally(String(e));
    }
  },

  // Permanently delete a project's files from disk (destructive - distinct from
  // removeFromRecentProjects, which only forgets the entry). The backend hands the
  // file to the OS delete; on success we refresh both the loaded list and the
  // config so the project disappears from the available AND "missing" recent rows.
  deleteProject: (path: string) => runProjectSwitch(async () => {
    try {
      const result = await settleCommand(
        invokeIpc('delete_project', { path }, { purpose: 'permanently delete project', fields: { path } })
      );
      // Deleting the file backing the OPEN project closes it - the backend tears
      // its own session down (project, SQLite handle, undo stack) - so the
      // frontend has to follow. Without that the UI kept rendering the deleted
      // project as open, and the next autosave tick, which does not care that the
      // file is gone, failed the save with "No project open" and surfaced that as
      // a save error.
      //
      // The backend REPORTS that teardown as `session_closed` on the delete's own
      // answer (the outcome, or the payload of its refusal), so this is a plain read
      // of a value we already have: no second IPC call, nothing predicted from the
      // deleted path, and nothing that can throw
      // between the delete and the state reset. (Asking instead - calling
      // `get_project_state` and testing it for null - answered the same yes/no
      // question by making the backend clone the whole project and recompute
      // coordinates, stats, penetration and frozen shots, which on a 75k-shot
      // project is seconds of work to learn one bit.)
      //
      // `result.success` is deliberately NOT a precondition. The backend closes
      // BEFORE handing the `.stalx` to the OS trash, and it has to: SQLite holds
      // the file open without `FILE_SHARE_DELETE`, so on Windows the trash fails
      // with "Access is denied" while that handle lives, and a close-afterwards
      // would never be reached. A trash that is then denied for some other reason
      // (no write access to the parent directory) therefore returns FAILURE with
      // the session already gone, and a success-only guard left the frontend
      // rendering a phantom project with no backend behind it. The recents refresh
      // below stays success-only for the opposite reason: on that branch the file
      // survived, so its entry has to stay so the project is re-openable.
      if (result.success ? result.session_closed : result.failure?.session_closed) clearOpenProjectState(set);
      if (result.success) {
        await Promise.all([get().loadRecentProjects(), get().reloadConfigInOrder()]);
      }
      return result;
    } catch (e) {
      console.error('Failed to delete project:', e);
      return refusedLocally(String(e));
    }
  }),

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

  reverseSection: async (sectionId: SectionId, swapLeftRight: boolean) => {
    return executeCommand(
      'reverse_section',
      { sectionId, swapLeftRight },
      {
        purpose: 'reverse section numbering end-to-end',
        fields: { sectionId, swapLeftRight },
      }
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

  setShotKind: async (sectionId: SectionId, shotId: ShotId, kind: ShotKind) => {
    return executeCommand(
      'set_shot_kind',
      { sectionId, shotId, kind },
      { purpose: 'set shot kind (permanent/jump/reference)', fields: { sectionId, shotId, kind } }
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
    return await invokeIpc(
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

  removeLoopClosure: async (closureId: number) => {
    return executeCommand(
      'remove_loop_closure',
      { closureId },
      { purpose: 'remove loop closure', fields: { closureId } }
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
      const adjusted = await invokeIpc('get_adjusted_stations', undefined, {
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
      // Reuse the measured shot columns (identical under closure adjustment) and
      // rebuild only the station columns, so entering/restoring the adjusted view
      // never force-materializes the lazy `section.shots` getters (#mem). Fall
      // back to the full legacy walk only if the measured SoA is somehow absent.
      const surveyDataAdjusted = surveyData
        ? surveyDataWithAdjustedStations(surveyData, adjusted)
        : surveyDataFromLegacy(project.sections, adjusted);
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
      // Snap (no animation) for first zero datum - the camera has no
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
    return await invokeIpc(
      'get_rigid_connected_section_ids',
      { sectionId },
      { purpose: 'query connected section ids', fields: { sectionId } }
    );
  },

  getAnchoredSectionIds: async () => {
    return await invokeIpc(
      'get_anchored_section_ids',
      undefined,
      { purpose: 'query anchored section ids' }
    );
  },

  // Undo/Redo
  undo: async () =>
    executeUndoRedo('undo', undefined, { purpose: 'undo last action' }, TOAST_KEYS.UNDO_FAILED),

  redo: async () =>
    executeUndoRedo('redo', undefined, { purpose: 'redo last undone action' }, TOAST_KEYS.REDO_FAILED),

  undoTo: async (index: number) =>
    executeUndoRedo('undo_to', { index }, { purpose: 'undo to history index', fields: { index } }, TOAST_KEYS.UNDO_FAILED),

  redoTo: async (index: number) =>
    executeUndoRedo('redo_to', { index }, { purpose: 'redo to history index', fields: { index } }, TOAST_KEYS.REDO_FAILED),

  getUndoStack: async () => {
    return await invokeIpc('get_undo_stack', undefined, {
      purpose: 'fetch undo history',
    });
  },

  getRedoStack: async () => {
    return await invokeIpc('get_redo_stack', undefined, {
      purpose: 'fetch redo history',
    });
  },

  refreshUndoState: async () => {
    // [#576 FZ-FZ-INT-10] In issue order and for the session it was issued in, like the
    // project refresh (RV5-8, NF-FIX5-1): a read issued for the previous project (an
    // autosave's, a refresh's tail) answering after a switch showed its Undo/Redo
    // availability in the new one, and an older answer could overwrite a newer one.
    undoStateRequestCounter += 1;
    const request = undoStateRequestCounter;
    const session = projectSessionEpoch();
    try {
      const [canUndo, canRedo] = await Promise.all([
        invokeIpc('can_undo', undefined, { purpose: 'check undo availability' }),
        invokeIpc('can_redo', undefined, { purpose: 'check redo availability' }),
      ]);
      if (request < undoStateAppliedRequest || session !== projectSessionEpoch()) return;
      undoStateAppliedRequest = request;
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
    return await invokeIpc('import_dmp', { path }, {
      purpose: 'parse DMP for import preview',
      fields: { path },
    });
  },

  parseTml: async (path: string) => {
    return await invokeIpc('parse_tml', { path }, {
      purpose: 'parse TML for import preview',
      fields: { path },
    });
  },

  executeTmlImport: async (path: string, fileHash?: string | null, includeExcluded?: boolean, commentsOnStations?: boolean, virtualShotMode?: VirtualShotMode, selectedSections?: number[] | null, rootPlacement?: TmlRootPlacement | null) => {
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
          // #27: null lets the backend default to Auto (the chain rule).
          virtualShotMode: virtualShotMode ?? null,
          // #425: null on either means "no filter" / "decide the root placement
          // the way you always did", so an older caller behaves unchanged.
          selectedSections: selectedSections ?? null,
          rootPlacement: rootPlacement ?? null,
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
    return await invokeIpc('parse_compass', { path }, {
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
    return await invokeIpc('parse_walls', { path }, {
      purpose: 'parse Walls for import preview',
      fields: { path },
    });
  },

  executeWallsImport: async (path: string, fileHash?: string | null, commentsOnStations?: boolean, includeSplays?: boolean, selectedSurveys?: number[] | null) => {
    // Re-arm the transient `_source` from the durable pending bridge (it resets
    // ~50ms after being set), so `command.execute_walls_import` is attributed to
    // the real entry point (hamburger/keyboard/drag-drop/dialog) instead of
    // 'unknown' - matching executeCompassImport/Tml/Gpx.
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

  updateProjectSettings: async (
    name: string,
    validation: Config['validation'],
    defaultAzimuthReference: AzimuthReference | null
  ) => {
    return executeCommand(
      'update_project_settings',
      { name, validation, defaultAzimuthReference },
      { purpose: 'update per-project settings' }
    );
  },

  setSurfaceElevation: async (elevation: number | null) => {
    return executeCommand(
      'set_surface_elevation',
      { elevation },
      { purpose: 'set entrance elevation' }
    );
  },

  setProjectNameForPath: async (path: string, name: string) => {
    return executeCommand(
      'set_project_name_for_path',
      { path, name },
      { purpose: 'rename a recent project', fields: { path } }
    );
  },

  parseGpx: async (path: string) => {
    return await invokeIpc('parse_gpx', { path }, {
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
    return await invokeIpc('calculate_file_hash', { path }, {
      purpose: 'detect duplicate imports',
      fields: { path },
    });
  },

  checkDuplicateImport: async (hash: string) => {
    return await invokeIpc('check_duplicate_import', { hash }, {
      purpose: 'detect duplicate imports',
      fields: { hash },
    });
  },

  checkSectionHashes: async (hashes: string[]) => {
    return await invokeIpc('check_section_hashes', { hashes }, {
      purpose: 'detect duplicate section imports',
      fields: { count: hashes.length },
    });
  },

  importSections: async (
    sections: ImportSectionConfig[],
    fileHash?: string | null,
    sectionHashes?: string[] | null,
    virtualShotMode?: VirtualShotMode,
    provenance?: ImportReceiptProvenance,
    existingShotUpdates?: ExistingShotUpdate[] | null,
    readOnlyNoticeInline?: boolean,
  ) => {
    const s = peekProjectActionSource();
    if (s) setSource(s);
    try {
      return await executeCommand(
        'import_sections',
        {
          sections,
          fileHash: fileHash ?? null,
          sectionHashes: sectionHashes ?? null,
          // The command has taken these since the overlap work; this wrapper
          // could not express them, so the only route to the overlap path was
          // the DMP wizard's direct `invokeIpc` (all four of its imports now
          // come through here, #576 D6/D13). Optional and null-defaulted, so no
          // existing caller changes behaviour.
          existingShotUpdates: existingShotUpdates ?? null,
          virtualShotMode: virtualShotMode ?? null,
          sourceLabel: provenance?.sourceLabel ?? null,
          sourceFileName: provenance?.sourceFileName ?? null,
          sectionsFound: provenance?.sectionsFound ?? null,
          sourceWarnings: provenance?.sourceWarnings ?? null,
        },
        { purpose: 'apply imported sections', fields: { section_count: sections.length }, readOnlyNoticeInline: readOnlyNoticeInline === true }
      );
    } finally {
      // Terminal flow: the project is already open, so there's no
      // setView('single') to clear the pending source. Clear it here so it
      // can't leak into the next peeking command.
      clearProjectActionSource();
    }
  },

  suggestImportPlacement: async (
    shots: ShotData[],
    // What `shots` are measured against (#438). Pass the same value the
    // `ImportSectionConfig` will carry: the matcher compares these bearings
    // against existing sections' and scores them in one frame, so omitting it
    // falls back to the project default and mis-scores a real overlap by the
    // difference of the two corrections.
    azimuthReference?: AzimuthReference | null,
  ) => {
    return await invokeIpc(
      'suggest_import_placement',
      { shots, azimuthReference: azimuthReference ?? null },
      { purpose: 'suggest placement from reference shot overlap' },
    );
  },

  mergeProjects: async (sourcePaths: string[], outputPath: string, outputName: string, archiveOriginals: boolean) => {
    try {
      const result = await settleCommand(
        invokeIpc(
          'merge_projects',
          { sourcePaths, outputPath, outputName, archiveOriginals },
          { purpose: 'merge projects into single file', fields: { source_count: sourcePaths.length, outputPath } }
        )
      );
      if (result.success) {
        // Refresh recent projects to reflect any changes (archived files removed, new file added)
        await get().loadRecentProjects();
      }
      return result;
    } catch (e) {
      console.error('Failed to merge projects:', e);
      return refusedLocally(String(e));
    }
  },
};
});

// [#576 RV6b-2, NF-W8FE-3] The identity a per-project setting write names
// (store/projectSession.ts): the project's created_at and the file it is open from.
registerOpenProjectIdentity(() => {
  const project = useProjectStore.getState().project;
  return project ? { createdAt: project.created_at, filePath: project.file_path ?? null } : null;
});
