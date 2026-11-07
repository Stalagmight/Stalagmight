/**
 * The DMP wizard's records of a run in progress, as they cross the
 * manual-placement round trip (split out of DmpNewImportDialog with #576 D4):
 * a placement made in the manual_placing phase, a suggestion the user skipped,
 * and the state a reopened instance restores from.
 */
import type { CrossSectionSuggestion, ImportPlacement } from '../../../types/commands';
import type { VirtualConnection } from '../../../utils/segmentContext';
import type { SectionEditState, ConfirmedOverlapInfo } from '../../../utils/batchImportHelpers';
import type { DmpRunCarry } from './useDmpImportRun';

// ─── Direct placement (manual_placing phase) ─────────────────────────────────

/** A placement decision made during the manual_placing phase for a segment. */
export type DirectPlacement = {
  anchorSectionIndex: number;   // DMP file section index (anchor within segment)
  anchorStationIndex: number;   // station within that section, in EMITTED numbering
  placement: ImportPlacement;   // origin, zero_datum, manual, manual_snap, or from_existing
};

export type SkippedSuggestionPlacement = {
  suggestion: CrossSectionSuggestion;
  mergeImportStation: number;
  mergeExistingStation: number;
};

export type SavedSkippedSuggestionEntry = CrossSectionSuggestion | SkippedSuggestionPlacement;

export function normalizeSkippedSuggestionEntry(entry: SavedSkippedSuggestionEntry): SkippedSuggestionPlacement {
  if (
    typeof entry === 'object' &&
    entry !== null &&
    'suggestion' in entry &&
    'mergeImportStation' in entry &&
    'mergeExistingStation' in entry
  ) {
    return entry as SkippedSuggestionPlacement;
  }

  const suggestion = entry as CrossSectionSuggestion;
  return {
    suggestion,
    mergeImportStation: suggestion.importStationIndex,
    mergeExistingStation: suggestion.existingStationIndex,
  };
}

/**
 * Saved state for restoring the dialog after manual map placement. The run's
 * accumulators (receipts, imported section ids, the project they belong to, ...)
 * are `DmpRunCarry`'s fields - see there for why each must ride along.
 */
export interface SavedManualPlacingState extends DmpRunCarry {
  connections: VirtualConnection[];
  skippedIndices: number[];
  segmentQueue: number[][];
  segmentAnchor: { sectionIndex: number; stationIndex: number };
  directPlacements: DirectPlacement[];
  skippedSuggestions: Array<[number, SavedSkippedSuggestionEntry]>;
  segmentSkipCounts: Array<[string, number]>;
  sectionEdits: Array<[number, SectionEditState]>;
  confirmedOverlaps?: Array<[number, ConfirmedOverlapInfo]>;
  /** DMP indices of segments the user chose to discard ("Do not place"). */
  discardedIndices?: number[];
  /**
   * The segment just dropped on the map (#576 D13): `segmentQueue[0]` and the
   * placement the drop made for it, NOT yet in `directPlacements` and not yet
   * imported. The drop only records it and reopens the wizard, which imports it
   * itself - so a failure lands on that segment's manual-placing screen instead
   * of in an instance that has already closed. Optional: absent means nothing
   * is waiting.
   */
  placedSegment?: { segment: number[]; placement: DirectPlacement };
}
