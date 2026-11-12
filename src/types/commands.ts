// Command and IPC payload types generated from Rust.

import type { ShotData, StationRef, Coordinates, GpsCoordinates, ClosureStrategy, ShotUpdates, SectionId, ShotId } from './project';
import type { CreditEdit, CreditTargets } from './ipc.generated';

export type {
  CommandResult,
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
  CreditTargets,
  CreditEdit,
} from './ipc.generated';

// Batch command types for execute_batch IPC.
// These match the Rust Command enum variants.
export type BatchCommand =
  | { type: 'create_section'; name?: string; surveyor?: string; date?: string; color: string; direction?: 'in' | 'out'; shots: ShotData[] }
  // Withdrawing an abandoned import removes N sections in one user action, so it
  // is batched: one undo entry, all-or-nothing (#394). `untrack_import` rides the
  // same batch - the import-dedup hashes describe the sections being removed, so
  // one undo has to bring both back.
  | { type: 'delete_section'; section_id: SectionId }
  | { type: 'untrack_import'; file_hashes: string[]; section_hashes: string[] }
  | { type: 'create_loop_closure'; station_a: StationRef; station_b: StationRef; closure_strategy: ClosureStrategy }
  | { type: 'merge_stations'; station_a: StationRef; station_b: StationRef }
  | { type: 'move_section'; section_id: number; offset: Coordinates }
  | { type: 'move_segment'; section_id: number; offset: Coordinates }
  | { type: 'move_station'; station_ref: StationRef; offset: Coordinates }
  | { type: 'set_zero_datum'; station_ref: StationRef; coordinates: GpsCoordinates }
  // Reversing a multi-selection is one user action, so it goes through a batch:
  // all-or-nothing rollback and a single undo entry (#24).
  | { type: 'reverse_section'; section_id: SectionId; swap_left_right: boolean }
  // Per-shot field edit - lets the data-table grid commit a multi-cell edit
  // (paste/fill) as one undo unit. Nested payload uses snake_case (it bypasses
  // Tauri's top-level camelCase conversion). ShotUpdates fields are all optional.
  | { type: 'update_shot'; section_id: SectionId; shot_id: ShotId; updates: ShotUpdates }
  // Per-shot survey credit (surveyors / explorers / date), batchable so a grid
  // edit that also touches credit columns stays one undo unit.
  | { type: 'edit_shot_credit'; targets: CreditTargets; edit: CreditEdit };
