// Command and IPC payload types generated from Rust.

import type { Command, ValidationWarning } from './ipc.generated';

export type {
  IntegrityRepairResult,
  IntegrityCheckResult,
  ValidationWarning,
  WarningSeverity,
  ProjectWarning,
  DmpFile,
  DmpSection,
  DmpShot,
  DmpDirection,
  DmpShotType,
  TmlFile,
  TmlSection,
  TmlShot,
  TmlShotType,
  TmlUnit,
  TmlGeoCoding,
  TmlClosure,
  TmlRootPlacement,
  CompassFile,
  CompassSurveySummary,
  InvalidShotCounts,
  WallsFile,
  WallsSurveySummary,
  ImportSectionConfig,
  ImportPlacement,
  SectionRelationships,
  StationPair,
  ClosurePair,
  ClosureUpdate,
  DeleteShotImpact,
  DeleteShotRangeImpact,
  RangeDeleteKind,
  ConnectivityResult,
  ClosurePreview,
  CommentHolder,
  UndoHistoryItem,
  PreviewData,
  FieldChange,
  ExportImageOptions,
  ImageFormat,
  ColorMode,
  GpxFile,
  GpxWaypoint,
  GpxTrack,
  GpxTrackSegment,
  GpxTrackPoint,
  GpxRoute,
  GpxRoutePoint,
  GpxSectionPlacement,
  MatchDirection,
  SuggestionConfidence,
  PlacementSuggestion,
  TargetType,
  CrossSectionSuggestion,
  ImportSectionForSuggestion,
  OrphanPlacementSuggestion,
  SegmentGeometry,
  SegmentSectionGeometry,
} from './ipc.generated';

/**
 * The outcome of an action the frontend reports itself, with no per-command backend
 * outcome behind it: the settings writes (`projectConfigActions.ts`) and project
 * creation (`emptyCommandResult`). It used to be the backend's one result type for
 * every command; since #576 A12 each command answers with its own outcome and the
 * store mutations with `CommandOutcome<T>` (`src/store/commandResult.ts`).
 */
export interface CommandResult {
  success: boolean;
  warnings: ValidationWarning[];
  error: string | null;
  section_ids: number[];
}

// Batch command types for execute_batch IPC: the generated `Command` (#576 C3),
// narrowed to the variants the frontend batches. Field names and types come from
// the Rust enum, so a nested key spelled in camelCase (the top-level Tauri
// convention, which a nested payload does NOT get) fails to compile.
export type BatchCommand = Extract<
  Command,
  {
    type:
      | 'create_section'
      // Withdrawing an abandoned import removes N sections in one user action, so it
      // is batched: one undo entry, all-or-nothing (#394). `untrack_import` rides the
      // same batch - the import-dedup hashes describe the sections being removed, so
      // one undo has to bring both back.
      | 'delete_section'
      | 'untrack_import'
      | 'create_loop_closure'
      | 'merge_stations'
      | 'move_section'
      | 'move_segment'
      | 'move_station'
      | 'set_zero_datum'
      // Reversing a multi-selection is one user action, so it goes through a batch:
      // all-or-nothing rollback and a single undo entry (#24).
      | 'reverse_section'
      // Per-shot field edit - lets the data-table grid commit a multi-cell edit
      // (paste/fill) as one undo unit. ShotUpdates fields are all optional.
      | 'update_shot';
  }
>;
