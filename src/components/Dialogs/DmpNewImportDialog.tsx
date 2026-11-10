import { usePreviewSize } from './usePreviewSize';
import { useState, useEffect, useMemo, useCallback, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import { invokeIpc } from '../../utils/ipc';
import { Button } from '../common/Button';
import { CheckCircleIcon } from '../common/icons';
import type { DmpFile, CrossSectionSuggestion, ImportSectionForSuggestion, CommandResult, SegmentGeometry } from '../../types/commands';
import type { DmpSelectionConfig } from './DmpSelectionDialog';
import type { DistanceField, Project, Station, ShotData } from '../../types/project';
import type { ImportSectionConfig as BackendImportSectionConfig, ImportPlacement } from '../../types/commands';
import type { ZeroDatumConfigResult } from './ZeroDatumConfigDialog';
import type { DmpSection } from '../../types/commands';
import type { OverlapMode } from '../../types/config';
import { WizardMapPanel } from './WizardMapPanel';
// The preview-square cap comes straight from layoutConstants rather than
// through WizardMapPanel's re-export: two component tests mock that module, and
// a mocked module cannot supply a plain number the render arithmetic needs.
import { WIZARD_MAP_PANEL_MAX_WIDTH as MAP_PANEL_MAX_WIDTH } from '../SingleSystemView/layoutConstants';
import { WizardScrollBody } from './WizardScrollBody';
import SectionPreview, { type MergePreviewConfig, type DmpMergePreviewConfig, type ContextSectionGeometry, computeStationsFromDmpShots, computeStationsFromShots } from './SectionPreview';
import { Select } from '../common/Select';
import { DialogWrapper } from './DialogWrapper';
import { AlertDialog } from './AlertDialog';
import { SectionMetadataForm } from '../common/SectionMetadataForm';
import { SuggestionSectionSettings } from './SuggestionSectionSettings';
import { SegmentPlacementModes, type SegmentPlacementMode } from './SegmentPlacementModes';
import {
  buildZeroDatumSectionConfig,
  DMP_AZIMUTH_REFERENCE,
  finalizePlacementForReverse,
  stationNamesForEmittedShots,
} from '../../utils/importSectionConfig';
import { ShotTableEditor, type EditableShot } from '../common/ShotTableEditor';
import { useProjectStore, projectSwitched } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { convertToEditableShots } from '../../hooks/useImportState';
import { editableShotToShotData, editableShotsToDmpSection } from '../../utils/shotConversion';
import {
  computeMergeOptions,
  defaultMergeOffset,
  computeOverlapIds,
  computeActionableIds,
  computeTargetShotIndex,
  needsDirectionReversal,
  reverseTargetShot,
  applyTargetData,
  interpolateShots,
  type TargetShotLike,
} from '../../utils/overlapHelpers';
import {
  type VirtualConnection,
  type ExistingSectionGeoEntry,
  buildAdjacency,
  computeDmpBfsContext,
  stationPositionFromShotLines,
  discoverBridgedDmpSections,
  computeUnreachableSegments,
  computeSegmentLayout,
} from '../../utils/segmentContext';
import { sectionColorPalette } from '../../utils/colors';
import { debugLog } from '../../utils/debugLog';
import { useUIStore, type PlacementResult } from '../../store/uiStore';
import { setSource, trackDialogComplete } from '../../utils/telemetry';
import {
  adjustStationForDeletedShots,
  buildPlacementSectionData,
  findReverseImportConnection,
  redirectDeletedTargets,
  resolveImportSectionDate,
  type SectionEditState,
  type ConfirmedOverlapInfo,
} from '../../utils/batchImportHelpers';
import {
  collectExistingAnchorsForTemporalRerank,
  distanceMapFromField,
  rerankSuggestionsByTemporalPlan,
} from '../../utils/dmpTemporalRerank';
import { extractFilename } from '../../utils/path';
import {
  fromEmittedStationIndex,
  isEmittedShot,
  toEmittedStationIndex,
  visibleBefore,
  visibleShotCount,
} from '../common/emittedShots';
import {
  applyAllOverlapTreatments,
  backendExistingShotUpdatesForSections,
  removeExistingShotUpdatesForSections,
  type ExistingShotUpdateLocal,
} from '../../utils/overlapTreatment';
import {
  NonFiniteShotError,
  assertSendableShots,
  findIncompleteSections,
} from '../../utils/importPayloadGuard';
import { reanchorAfterRunDelete, reanchorStation } from '../../utils/stationReanchor';
import { blankEditableShot } from '../common/editableShot';
import { aggregateImportReceipts } from '../../utils/importReceipt';
import type { ImportReceipt } from '../../types/ipc/ImportReceipt';
import { TOAST_KEYS } from '../../utils/toastKeys';

interface DmpNewImportDialogProps {
  dmpFile: DmpFile;
  selectionConfig: DmpSelectionConfig;
  project: Project | null;
  stations: Station[];
  zeroDatumConfig?: ZeroDatumConfigResult | undefined;
  onCancel: () => void;
  onBack: () => void;
  onComplete: (newSectionIds?: number[], receipt?: ImportReceipt) => void;
  /** Saved state to restore after returning from manual map placement. */
  savedManualPlacingState?: SavedManualPlacingState | undefined;
  /** If true, immediately start map placement when the dialog opens in manual placing phase. */
  autoStartManualPlacement?: boolean;
}

/** Mirror both frontend stores after a direct `import_sections` mutation. */
export function markSuccessfulDmpImportDirty(
  markProjectDirty = useProjectStore.getState().markDirty,
  markForwardEdit = useSpeleoStore.getState().markForwardEdit,
): void {
  markProjectDirty();
  markForwardEdit();
}

// SectionEditState and ConfirmedOverlapInfo imported from batchImportHelpers

/** A refusal from the payload guard is already written for the user; anything
 *  else reaching a catch here is a raw failure with no better wording. */
function importErrorMessage(e: unknown): string {
  return e instanceof NonFiniteShotError ? e.message : String(e);
}

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

type SavedSkippedSuggestionEntry = CrossSectionSuggestion | SkippedSuggestionPlacement;

function normalizeSkippedSuggestionEntry(entry: SavedSkippedSuggestionEntry): SkippedSuggestionPlacement {
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

/** Saved state for restoring the dialog after manual map placement. */
export interface SavedManualPlacingState {
  connections: VirtualConnection[];
  skippedIndices: number[];
  segmentQueue: number[][];
  segmentAnchor: { sectionIndex: number; stationIndex: number };
  directPlacements: DirectPlacement[];
  skippedSuggestions: Array<[number, SavedSkippedSuggestionEntry]>;
  segmentSkipCounts: Array<[string, number]>;
  sectionEdits: Array<[number, SectionEditState]>;
  confirmedOverlaps?: Array<[number, ConfirmedOverlapInfo]>;
  existingShotUpdates?: ExistingShotUpdateLocal[];
  earlyImported?: Array<[number, number]>;
  /** DMP indices of a segment just placed that needs immediate import on dialog restore. */
  pendingSegmentImport?: number[];
  /**
   * Every `import_receipt` this wizard run has banked so far (#487).
   *
   * The run's receipts live in a component ref, and manual map placement closes
   * this dialog and reopens a fresh instance - so without carrying them here the
   * reopened instance completes the import with an empty list, which aggregates
   * to `undefined` and silently suppresses the receipt the other import paths
   * all show. Optional because a state saved by an older build (or a test) has
   * none; absent reads as "nothing banked yet".
   */
  importedReceipts?: ImportReceipt[];
  /**
   * Every project `section_id` this wizard run has created so far (#487
   * residue 2).
   *
   * Same carry, same reason, as `importedReceipts` above - and the same
   * consequence when it is missing: the reopened instance completes with an
   * empty list, App.tsx's `if (newSectionIds?.length)` is false, and
   * `suggest_orphan_placements` is never called. So the manual-placement route
   * silently skipped orphan detection that the direct route always ran.
   *
   * It is also what `withdrawImportedSections` needs to be able to undo a run
   * that crossed the handoff. Optional for the same reason: an older saved
   * state (or a test) has none, and absent reads as "nothing imported yet".
   */
  importedSectionIds?: number[];
  /**
   * The `created_at` of the project those ids were created in.
   *
   * Nothing clears `dialogData` when the open project changes, so this struct
   * can outlive the project it describes - and section ids are small
   * per-project integers, so every one of them would RESOLVE against whatever
   * is open instead. Carried so the withdrawal can refuse rather than delete
   * same-numbered sections in the wrong survey. Optional: a state saved by an
   * older build has none, and absent means "unknown", which never refuses.
   */
  projectKey?: string | null;
  /** DMP indices of segments the user chose to discard ("Do not place"). */
  discardedIndices?: number[];
}

// ─── Effective section helpers ────────────────────────────────────────────────

/**
 * Build a DmpSection reflecting edits (deleted shots removed, interpolated
 * values applied). Falls back to the raw section when no edits exist.
 */
function buildEffectiveDmpSection(raw: DmpSection, edits: SectionEditState | undefined): DmpSection {
  if (!edits) return raw;
  return {
    ...editableShotsToDmpSection(raw, edits.shots),
    name: edits.name,
    direction: edits.direction,
  };
}

/**
 * Adjust VirtualConnection station indices to account for deleted shots in
 * edited sections (so layout offsets align with post-treatment geometry).
 */
function adjustConnectionsForEdits(
  connections: VirtualConnection[],
  sectionEdits: Map<number, SectionEditState>,
): VirtualConnection[] {
  return connections.map(conn => {
    const importEdits = sectionEdits.get(conn.importSectionIndex);
    const adjustedImport = importEdits
      ? adjustStationForDeletedShots(importEdits.shots, conn.importStationIndex)
      : conn.importStationIndex;

    const adjustedTarget =
      conn.targetType === 'importing' && conn.targetImportIndex != null && conn.targetImportStationIndex != null
        ? (sectionEdits.get(conn.targetImportIndex)
            ? adjustStationForDeletedShots(sectionEdits.get(conn.targetImportIndex)!.shots, conn.targetImportStationIndex)
            : conn.targetImportStationIndex)
        : conn.targetImportStationIndex;

    return {
      ...conn,
      importStationIndex: adjustedImport,
      ...(adjustedTarget != null ? { targetImportStationIndex: adjustedTarget } : {}),
    };
  });
}

// ─── Graph helpers ───────────────────────────────────────────────────────────

/** BFS from a set of seed nodes, returning reachable nodes in traversal order. */
function bfsReachable(
  seeds: Set<number>,
  adjacency: Map<number, Array<{ neighbor: number; connection: VirtualConnection }>>,
  blocked: Set<number> = new Set(),
): number[] {
  const visited = new Set<number>();
  const queue: number[] = [];
  const order: number[] = [];

  for (const seed of seeds) {
    if (blocked.has(seed)) continue;
    if (!visited.has(seed)) {
      visited.add(seed);
      queue.push(seed);
    }
  }

  while (queue.length > 0) {
    const node = queue.shift()!;
    order.push(node);
    for (const edge of adjacency.get(node) ?? []) {
      if (blocked.has(edge.neighbor)) continue;
      if (!visited.has(edge.neighbor)) {
        visited.add(edge.neighbor);
        queue.push(edge.neighbor);
      }
    }
  }
  return order;
}

function collectImportConnectedComponents(
  seeds: Set<number>,
  adjacency: Map<number, Array<{ neighbor: number; connection: VirtualConnection }>>,
): Set<number> {
  const result = new Set<number>();
  const queue = Array.from(seeds);

  for (const seed of seeds) {
    result.add(seed);
  }

  while (queue.length > 0) {
    const node = queue.shift()!;
    for (const edge of adjacency.get(node) ?? []) {
      if (result.has(edge.neighbor)) continue;
      result.add(edge.neighbor);
      queue.push(edge.neighbor);
    }
  }

  return result;
}


/**
 * Build batch import configs from virtual connections + BFS order.
 *
 * Throws {@link NonFiniteShotError} rather than serializing a row that holds no
 * measurement, and reports every reachable section that produced no shots at
 * all in `emptySections` - both are sections that used to leave here silently
 * and fail (or vanish) somewhere the user could not read.
 *
 * Exported for tests only - the dialog is its sole production caller.
 */
export function buildBatchImport(
  connections: VirtualConnection[],
  zeroDatumIndex: number | null,
  dmpFile: DmpFile,
  selectionConfig: DmpSelectionConfig,
  sectionEdits?: Map<number, SectionEditState>,
  fullPool?: CrossSectionSuggestion[],
  directPlacements?: DirectPlacement[],
  alreadyImported?: Map<number, number>,
  deferImportConnectedTo?: Set<number>,
  resolvedTheme = 'dark',
): {
  configs: BackendImportSectionConfig[];
  unreachable: number[];
  emittedDmpIndices: number[];
  emptySections: number[];
} {
  const palette = sectionColorPalette(resolvedTheme);
  // Reachable, selected, and emitted nothing. A set because the emit loop makes
  // several passes over the same order.
  const emptySections = new Set<number>();
  // Build adjacency + connections including any fallback anchors for disconnected components
  const allConnections = [...connections];
  const adjacency = buildAdjacency(allConnections);
  const deferredImportSet = deferImportConnectedTo && deferImportConnectedTo.size > 0
    ? collectImportConnectedComponents(deferImportConnectedTo, adjacency)
    : new Set<number>();

  // Direct placement map: section index → ImportPlacement (for manual_placing phase)
  const directPlacementMap = new Map<number, ImportPlacement>();
  if (directPlacements) {
    for (const dp of directPlacements) {
      directPlacementMap.set(dp.anchorSectionIndex, dp.placement);
    }
  }

  // Sections connected to existing project sections (anchors)
  const anchored = new Set<number>();
  if (zeroDatumIndex != null) anchored.add(zeroDatumIndex);
  for (const c of allConnections) {
    if (c.targetType === 'existing' && !deferredImportSet.has(c.importSectionIndex)) {
      anchored.add(c.importSectionIndex);
    }
  }
  // Direct placements also anchor their sections
  for (const dp of directPlacements ?? []) {
    if (!deferredImportSet.has(dp.anchorSectionIndex)) {
      anchored.add(dp.anchorSectionIndex);
    }
  }

  // BFS from anchored sections
  let reachableOrder = bfsReachable(anchored, adjacency, deferredImportSet);
  let reachableSet = new Set(reachableOrder);

  // Find disconnected components with confirmed connections but no anchor.
  // For each, look for an existing-target suggestion in the full pool to create a fallback anchor.
  if (fullPool && fullPool.length > 0) {
    const connectedSections = new Set(allConnections.map((c) => c.importSectionIndex));
    const unreachableConnected = Array.from(connectedSections).filter((idx) => !reachableSet.has(idx));

    if (unreachableConnected.length > 0) {
      // Group unreachable connected sections into components via BFS on adjacency
      const visited = new Set<number>();
      for (const seed of unreachableConnected) {
        if (visited.has(seed)) continue;
        // BFS to find this component
        const component: number[] = [];
        const queue = [seed];
        visited.add(seed);
        while (queue.length > 0) {
          const node = queue.shift()!;
          component.push(node);
          for (const edge of adjacency.get(node) ?? []) {
            if (!deferredImportSet.has(edge.neighbor) && !visited.has(edge.neighbor) && !reachableSet.has(edge.neighbor)) {
              visited.add(edge.neighbor);
              queue.push(edge.neighbor);
            }
          }
        }

        // Find best existing-target suggestion for any section in this component
        const componentSet = new Set(component);
        let bestExisting: CrossSectionSuggestion | null = null;
        for (const s of fullPool) {
          if (
            s.targetType === 'existing' &&
            s.existingSectionId != null &&
            componentSet.has(s.importSectionIndex) &&
            !deferredImportSet.has(s.importSectionIndex)
          ) {
            if (!bestExisting || s.score > bestExisting.score) {
              bestExisting = s;
            }
          }
        }

        if (bestExisting) {
          // Create a fallback connection to anchor this component
          const fallbackConn: VirtualConnection = {
            importSectionIndex: bestExisting.importSectionIndex,
            importStationIndex: bestExisting.importStationIndex,
            targetType: 'existing',
            existingSectionId: bestExisting.existingSectionId!,
            existingStationIndex: bestExisting.existingStationIndex,
          };
          allConnections.push(fallbackConn);
          anchored.add(bestExisting.importSectionIndex);
        }
      }

      // Re-run BFS with new anchors
      if (anchored.size > reachableSet.size) {
        // Rebuild adjacency with new connections
        const newAdjacency = buildAdjacency(allConnections);
        reachableOrder = bfsReachable(anchored, newAdjacency, deferredImportSet);
        reachableSet = new Set(reachableOrder);
      }
    }
  }

  // Build connection map - fallback 'existing' connections override original 'importing' ones
  const connectionMap = new Map<number, VirtualConnection>();
  for (const c of allConnections) {
    const existing = connectionMap.get(c.importSectionIndex);
    if (!existing || c.targetType === 'existing') {
      connectionMap.set(c.importSectionIndex, c);
    }
  }

  // Track which sections are already emitted (by section_index) and their position in configs[]
  const emittedPosition = new Map<number, number>();

  const configs: BackendImportSectionConfig[] = [];
  const sortedSelected = Array.from(selectionConfig.selectedIndices).sort((a, b) => a - b);

  // Multi-pass: BFS order may not match dependency order for fallback-anchored
  // components, so retry sections whose from_importing target wasn't emitted yet.
  let lastConfigCount = -1;
  while (configs.length > lastConfigCount) {
    lastConfigCount = configs.length;
    for (const idx of reachableOrder) {
      if (emittedPosition.has(idx)) continue;
      // Skip zero datum - already imported
      if (idx === zeroDatumIndex) continue;
      // Skip sections already imported in a previous batch (early import)
      if (alreadyImported?.has(idx)) continue;
      // Skipped sections and their import-to-import connected components are
      // deferred from early import so manual placement can move the whole group.
      if (deferredImportSet.has(idx)) continue;

      const section = dmpFile.sections[idx];
      if (!section) continue;

      const edits = sectionEdits?.get(idx);

      // Use edited shots if available, otherwise fall back to DMP data. The
      // emitted rows are kept as well as converted: their mNemo station numbers
      // become the section's station names.
      let emittedShots: EditableShot[];
      let backendShots: ShotData[];
      if (edits) {
        const deletedCount = edits.shots.filter(s => s.isDeleted).length;
        emittedShots = edits.shots
          // `!isBlankNewShot` as well: the completeness gate deliberately lets a
          // row added and left wholly blank through (it is an invitation, not an
          // obligation), which is only safe if every payload path drops it. The
          // zero-datum and segment paths do; this one did not, so a row typed in
          // and then cleared arrived as NaN length, depths and azimuths.
          .filter(isEmittedShot);
        backendShots = emittedShots.map(editableShotToShotData);
        if (deletedCount > 0) {
          debugLog(
            `[buildBatchImport] Section ${idx} "${section.name}": ${edits.shots.length} total shots, ${deletedCount} deleted, ${backendShots.length} emitted`,
          );
        }
        // Compare edited shots to original DMP data to detect interpolation
        if (import.meta.env.DEV) {
          const origShots = convertToEditableShots(section.shots);
          let modifiedCount = 0;
          const aliveEdited = edits.shots.filter(s => !s.isDeleted);
          for (const edited of aliveEdited) {
            if (edited.isNew) continue;
            const orig = origShots.find(o => o.id === edited.id);
            if (orig && (
              Math.abs(orig.azimuth_in - edited.azimuth_in) > 0.01 ||
              Math.abs(orig.length - edited.length) > 0.001
            )) {
              modifiedCount++;
              if (modifiedCount <= 3) {
                debugLog(
                  `[buildBatchImport]   MODIFIED shot id=${edited.id}:`,
                  `az_in ${orig.azimuth_in.toFixed(1)}→${edited.azimuth_in.toFixed(1)}`,
                  `len ${orig.length.toFixed(3)}→${edited.length.toFixed(3)}`,
                  `kind=${edited.kind}`,
                );
              }
            }
          }
          if (modifiedCount > 0) {
            debugLog(`[buildBatchImport] Section ${idx} "${section.name}": ${modifiedCount} shots have INTERPOLATED values`);
          }
        }
      } else {
        emittedShots = convertToEditableShots(section.shots);
        backendShots = emittedShots.map(editableShotToShotData);
        debugLog(`[buildBatchImport] Section ${idx} "${section.name}": no edits, using ${backendShots.length} DMP shots`);
      }
      if (backendShots.length === 0) {
        // Reported, not just logged: the `unreachable` list names what was never
        // placed, so a section that WAS placed and then emitted nothing fell
        // through both and disappeared between the wizard and the map. Anything
        // anchored to it stalls the emit loop and disappears with it.
        emptySections.add(idx);
        debugLog(`[buildBatchImport] Section ${idx} "${section.name}": SKIPPED - 0 shots after filtering deleted`);
        continue;
      }
      // The gates upstream ask the user to finish a half-filled row; this asks
      // whether the payload can be sent at all, which has to hold for every
      // route into this builder. A NaN here serializes to null and Rust rejects
      // the whole batch naming nothing.
      assertSendableShots(backendShots, idx, edits?.name ?? section.name);

      // Check for direct placement first (from manual_placing phase)
      const directPlacement = directPlacementMap.get(idx);
      let conn = connectionMap.get(idx);

      // Fallback: section is only the TARGET of an import-to-import connection
      if (!directPlacement && !conn) {
        conn = findReverseImportConnection(idx, allConnections, emittedPosition);
        // Also check if the reverse target was already imported in a previous batch
        if (!conn && alreadyImported) {
          for (const c of allConnections) {
            if (c.targetType !== 'importing') continue;
            if (c.targetImportIndex !== idx) continue;
            if (!alreadyImported.has(c.importSectionIndex)) continue;
            conn = {
              importSectionIndex: idx,
              importStationIndex: c.targetImportStationIndex ?? 0,
              targetType: 'importing',
              targetImportIndex: c.importSectionIndex,
              targetImportStationIndex: c.importStationIndex,
            };
            break;
          }
        }
        if (!conn) {
          debugLog(`[buildBatchImport] Section ${idx} "${section.name}": SKIPPED - no connection or direct placement`);
          continue;
        }
        debugLog(`[buildBatchImport] Section ${idx} "${section.name}": using reverse import connection → ${conn.targetImportIndex}`);
      }

      let placement: ImportPlacement;
      if (directPlacement) {
        placement = directPlacement;
      } else {
        // Derive placement from virtual connection
        // Adjust import station index for deleted shots (overlap removal shifts stations)
        const adjustedImportStation = edits
          ? adjustStationForDeletedShots(edits.shots, conn!.importStationIndex)
          : conn!.importStationIndex;

        if (conn!.targetType === 'existing' && conn!.existingSectionId != null && conn!.existingStationIndex != null) {
          placement = {
            type: 'from_existing',
            import_station: adjustedImportStation,
            existing_station: {
              section_id: conn!.existingSectionId,
              station_index: conn!.existingStationIndex,
            },
          };
        } else if (conn!.targetType === 'importing' && conn!.targetImportIndex != null && conn!.targetImportStationIndex != null) {
          // Check if the target was already imported in a previous batch
          const alreadyImportedSectionId = alreadyImported?.get(conn!.targetImportIndex);
          if (alreadyImportedSectionId != null) {
            // Convert from_importing → from_existing using the real section_id
            const targetEdits = sectionEdits?.get(conn!.targetImportIndex);
            const adjTargetStn = targetEdits
              ? adjustStationForDeletedShots(targetEdits.shots, conn!.targetImportStationIndex!)
              : conn!.targetImportStationIndex!;
            placement = {
              type: 'from_existing',
              import_station: adjustedImportStation,
              existing_station: {
                section_id: alreadyImportedSectionId,
                station_index: adjTargetStn,
              },
            };
          } else {
          // The target must already be emitted in configs[]
          let targetPos = emittedPosition.get(conn!.targetImportIndex);
          if (targetPos == null) {
            // Primary target not yet emitted - try reversed connection to an already-emitted section
            const reverseConn = findReverseImportConnection(idx, allConnections, emittedPosition);
            if (reverseConn) {
              conn = reverseConn;
              const adjustedImportStation2 = edits
                ? adjustStationForDeletedShots(edits.shots, conn.importStationIndex)
                : conn.importStationIndex;
              // Check if the reverse target was already imported
              const reverseAlreadyId = alreadyImported?.get(conn.targetImportIndex!);
              if (reverseAlreadyId != null) {
                const targetEdits2 = sectionEdits?.get(conn.targetImportIndex!);
                const adjTargetStn2 = targetEdits2
                  ? adjustStationForDeletedShots(targetEdits2.shots, conn.targetImportStationIndex!)
                  : conn.targetImportStationIndex!;
                placement = {
                  type: 'from_existing',
                  import_station: adjustedImportStation2,
                  existing_station: {
                    section_id: reverseAlreadyId,
                    station_index: adjTargetStn2,
                  },
                };
              } else {
              targetPos = emittedPosition.get(conn.targetImportIndex!);
              if (targetPos == null) {
                // Shouldn't happen - findReverseImportConnection checks emittedPosition
                continue;
              }
              const targetEdits2 = sectionEdits?.get(conn.targetImportIndex!);
              const adjustedTargetStation2 = targetEdits2
                ? adjustStationForDeletedShots(targetEdits2.shots, conn.targetImportStationIndex!)
                : conn.targetImportStationIndex!;
              placement = {
                type: 'from_importing',
                import_station: adjustedImportStation2,
                target_section_index: targetPos,
                target_station_index: adjustedTargetStation2,
              };
              }
            } else {
              // No reverse available - will retry in next pass
              debugLog(
                `[buildBatchImport] Section ${idx} "${section.name}": DEFERRED - target import ${conn!.targetImportIndex} not yet emitted`,
              );
              continue;
            }
          } else {
            // Target is emitted - original code path
            const targetEdits = sectionEdits?.get(conn!.targetImportIndex);
            const adjustedTargetStation = targetEdits
              ? adjustStationForDeletedShots(targetEdits.shots, conn!.targetImportStationIndex)
              : conn!.targetImportStationIndex;
            placement = {
              type: 'from_importing',
              import_station: adjustedImportStation,
              target_section_index: targetPos,
              target_station_index: adjustedTargetStation,
            };
          }
          }
        } else {
          continue;
        }
      }

      // Use edits for metadata if available, otherwise fall back to DMP data + selectionConfig
      const sectionName = edits?.name ?? section.name;
      const surveyorValue = edits?.surveyor ?? selectionConfig.defaultSurveyor ?? null;
      // `resolveImportSectionDate`, not the raw `??` chain plus a trailing
      // `|| null`: a cleared date has to leave as null on EVERY emitter, and
      // the rule is stated once (#478 - see the helper).
      const dateStr = resolveImportSectionDate(edits?.date, selectionConfig.defaultDate, section.date);
      const selectedPos = sortedSelected.indexOf(idx);
      const colorValue = (edits?.color || null) ?? palette[(selectedPos >= 0 ? selectedPos : 0) % palette.length] ?? '#4ecdc4';
      const directionValue = edits?.direction ?? section.direction;
      const qualityValue = edits?.quality ?? 1.0;
      const reverseValue = edits?.reverse ?? false;

      // #24 / q14: the backend reverses before creating the section, so every
      // placement index must address the post-reverse order - on BOTH sides of a
      // connection. A directPlacement was not derived from `conn`, so it passes
      // no connection at all (see finalizePlacementForReverse).
      const finalPlacement = finalizePlacementForReverse(placement, reverseValue, backendShots.length, directPlacement ? null : conn, sectionEdits);

      const config: BackendImportSectionConfig = {
        section_index: configs.length,
        name: sectionName,
        surveyor: surveyorValue || null,
        date: dateStr,
        color: colorValue,
        direction: directionValue,
        quality: qualityValue,
        shots: backendShots,
        placement: finalPlacement,
        station_comments: [],
        station_names: stationNamesForEmittedShots(emittedShots),
        reverse: reverseValue,
        // The mNemo measures magnetic; nothing in the file declares a
        // declination (#438).
        azimuth_reference: DMP_AZIMUTH_REFERENCE,
      };

      emittedPosition.set(idx, configs.length);
      configs.push(config);
    }
  }

  // Unreachable = connected sections not in reachableSet + sections with no connection at all
  const allSelected = Array.from(selectionConfig.selectedIndices);
  const unreachable = allSelected.filter(
    (idx) => idx !== zeroDatumIndex && (deferredImportSet.has(idx) || !reachableSet.has(idx)),
  );

  if (import.meta.env.DEV) {
    // Summarize connection types
    let importingCount = 0;
    let existingCount = 0;
    let directCount = 0;
    for (const c of allConnections) {
      if (c.targetType === 'importing') importingCount++;
      else existingCount++;
    }
    for (const _dp of directPlacements ?? []) directCount++;
    const editedSections = sectionEdits ? Array.from(sectionEdits.keys()).length : 0;
    debugLog(
      `[buildBatchImport] SUMMARY: ${configs.length} configs emitted, ${unreachable.length} unreachable,`,
      `connections: ${existingCount} existing + ${importingCount} importing + ${directCount} direct,`,
      `sectionEdits has ${editedSections} entries`,
    );
    // Flag reachable sections that were never emitted (stuck in DEFERRED loop or skipped)
    for (const idx of reachableOrder) {
      if (idx === zeroDatumIndex) continue;
      if (!emittedPosition.has(idx)) {
        const s = dmpFile.sections[idx];
        const ed = sectionEdits?.get(idx);
        const alive = ed ? ed.shots.filter(sh => !sh.isDeleted).length : s?.shots.length ?? 0;
        const hasConn = connectionMap.has(idx);
        const hasDirect = directPlacementMap.has(idx);
        debugLog(
          `[buildBatchImport] ⚠ Section ${idx} "${s?.name}" REACHABLE but NOT EMITTED:`,
          `alive=${alive} hasConn=${hasConn} hasDirect=${hasDirect}`,
          hasConn ? `conn=${JSON.stringify(connectionMap.get(idx))}` : '',
        );
      }
    }
  }

  // A reachable section that never got emitted is stuck in the DEFERRED loop
  // or was silently skipped - a genuinely unexpected condition worth production
  // signal (debugLog.ts: "keep console.warn for genuinely unexpected runtime
  // conditions"), unlike the trace-only diagnostics above. Index/count only,
  // never a section NAME or `JSON.stringify`'d connection - both are
  // file-controlled content that must not reach a production console log.
  //
  // #535: `!emittedPosition.has(idx)` alone is a superset of "gap" - it is
  // ALSO true of every section that was legitimately handled elsewhere:
  // already emitted in an earlier batch (`alreadyImported`, skipped before
  // `emittedPosition.set` at the top of the loop), or reachable but empty
  // (`emptySections`, already surfaced via `reportEmptySections`). Manual
  // placement calls `buildBatchImport` fresh on every step with the FULL
  // accumulated graph, so `alreadyImported` alone grows by one on every
  // successful step - counting it here made the warning fire on every healthy
  // incremental placement, climbing with each step instead of naming a gap.
  //
  // `!deferredImportSet.has(idx)` is NOT a third spam source and is dead in
  // practice: both `bfsReachable` calls above (:387, :450) pass
  // `deferredImportSet` as `blocked`, and `bfsReachable` never visits or
  // returns a blocked index (:280, :291) - so no `idx` in `reachableOrder`
  // can ever be in `deferredImportSet`. Kept only as defensive belt-and-
  // braces against a future caller passing a differently-scoped set.
  const unemittedReachableCount = reachableOrder.filter(
    (idx) =>
      idx !== zeroDatumIndex &&
      !emittedPosition.has(idx) &&
      !alreadyImported?.has(idx) &&
      !deferredImportSet.has(idx) &&
      !emptySections.has(idx),
  ).length;
  if (unemittedReachableCount > 0) {
    console.warn(
      `[buildBatchImport] ${unemittedReachableCount} reachable section(s) were not emitted`,
    );
  }

  return {
    configs,
    unreachable,
    emittedDmpIndices: Array.from(emittedPosition.keys()),
    emptySections: Array.from(emptySections),
  };
}


/**
 * Connection Wizard - pre-calculates all placements at startup, presents
 * suggestions sorted by score, records virtual connections on confirm,
 * then batch-places everything when suggestions are exhausted.
 */
export default function DmpNewImportDialog({
  dmpFile,
  selectionConfig,
  project: _project,
  stations: _stations,
  zeroDatumConfig,
  onCancel,
  onBack,
  onComplete,
  savedManualPlacingState,
  autoStartManualPlacement = false,
}: DmpNewImportDialogProps) {
  const { t } = useTranslation();
  const importSecondaryButtonClass = 'ring-1 ring-inset ring-border hover:bg-background-50';

  // Responsive preview - fills the available column so it never overflows the
  // fullscreen dialog on short windows (#93 review) and never leaves the column
  // half empty on wide ones (#567). One per flow.
  // The map column's width, dragged by the divider between it and the settings.
  // One value for both side-by-side blocks (suggestion and manual placing), so
  // the wizard does not change shape as it steps between them - and store-owned
  // since #404, so it also survives the dialog closing and the session ending
  // (WizardMapPanel persists it on divider release).
  const mapPanelWidth = useUIStore((s) => s.wizardMapPanelWidth);
  const setMapPanelWidth = useUIStore((s) => s.setWizardMapPanelWidth);
  const suggestionBodyRef = useRef<HTMLDivElement | null>(null);
  const manualBodyRef = useRef<HTMLDivElement | null>(null);
  const [suggestionPreviewRef, suggestionPreviewSize] = usePreviewSize(MAP_PANEL_MAX_WIDTH);
  const [manualPreviewRef, manualPreviewSize] = usePreviewSize(MAP_PANEL_MAX_WIDTH);

  // Store selectors
  const importSections = useProjectStore((s) => s.importSections);
  const executeBatch = useProjectStore((s) => s.executeBatch);
  const refreshProjectState = useProjectStore((s) => s.refreshProjectState);
  const config = useProjectStore((s) => s.config);
  const updateConfig = useProjectStore((s) => s.updateConfig);
  const resolvedTheme = useUIStore((s) => s.resolvedTheme);
  // #357 item 9: seed new sections from the palette legible on this theme's
  // MAP CANVAS, not its chrome - a section keeps whatever color it was
  // created with, so this only ever affects colors handed to NEW sections.
  const sectionPalette = useMemo(() => sectionColorPalette(resolvedTheme), [resolvedTheme]);

  // Shorthand for restoring state
  const saved = savedManualPlacingState;

  // Core state - virtual connections instead of imported indices
  const [connections, setConnections] = useState<VirtualConnection[]>(saved?.connections ?? []);
  const [skippedIndices, setSkippedIndices] = useState<Set<number>>(
    saved ? new Set(saved.skippedIndices) : new Set()
  );
  const [currentSuggestion, setCurrentSuggestion] = useState<CrossSectionSuggestion | null>(null);
  const [alternatives, setAlternatives] = useState<CrossSectionSuggestion[]>([]);
  const [showAlternatives, setShowAlternatives] = useState(true);
  const [isCalculating, setIsCalculating] = useState(false);
  const [isImporting, setIsImporting] = useState(false);
  const [phase, setPhase] = useState<'init' | 'suggesting' | 'manual_placing' | 'placing' | 'done'>(
    saved ? 'manual_placing' : 'init'
  );
  const [autoStartManualPlacementPending, setAutoStartManualPlacementPending] = useState(
    autoStartManualPlacement
  );
  const [showCancelConfirm, setShowCancelConfirm] = useState(false);
  const [showBackConfirm, setShowBackConfirm] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // F8: registers this dialog's footer height with the toast stack for as long
  // as the footer is mounted, so a lingering toast (e.g. the manual-placement
  // hint, which fires just before this dialog closes and can still be on
  // screen when it reopens) is raised above the footer instead of covering
  // the Cancel/Back row.
  const footerRef = useRef<HTMLDivElement | null>(null);
  const registerToastBottomOffset = useUIStore((s) => s.registerToastBottomOffset);
  const unregisterToastBottomOffset = useUIStore((s) => s.unregisterToastBottomOffset);
  useEffect(() => {
    const el = footerRef.current;
    if (!el || typeof ResizeObserver === 'undefined') return undefined;
    const owner = 'dmp-import-new-footer';
    const report = () => registerToastBottomOffset(owner, el.getBoundingClientRect().height);
    const ro = new ResizeObserver(report);
    ro.observe(el);
    report();
    return () => {
      ro.disconnect();
      unregisterToastBottomOffset(owner);
    };
  }, [registerToastBottomOffset, unregisterToastBottomOffset]);
  const [placingRetry, setPlacingRetry] = useState(0);

  // Merge station selection
  const [selectedMergeOffset, setSelectedMergeOffset] = useState(0);

  // Overlap handling mode (persisted to config)
  const [overlapMode, setOverlapMode] = useState<OverlapMode>(
    config?.import?.overlap_mode ?? 'import_data'
  );

  // Section editing state
  const [editState, setEditState] = useState<SectionEditState | null>(null);
  /** The live edit state, for actions that must not read a captured copy - see
   *  refuseIncompleteShots. */
  const editStateRef = useRef<SectionEditState | null>(null);
  editStateRef.current = editState;
  /** Rows a mutator produced earlier in the CURRENT event, before React has
   *  rendered them. Cleared on every render - by then `editStateRef` holds them
   *  - so a second row change in one gesture still counts from the first's
   *  result rather than from the array on screen. */
  const pendingShotsRef = useRef<EditableShot[] | null>(null);
  pendingShotsRef.current = null;
  // Tracks which import section index the current editState was loaded for.
  // Used to prevent the sync effect from writing stale editState to the wrong section
  // when currentSuggestion changes before editState catches up.
  const [editStateForSection, setEditStateForSection] = useState<number | null>(null);
  /** Which section `editStateRef` currently holds, as a ref: the gate below has
   *  to pair the two, and a captured pairing is the stale-closure bug again. */
  const editStateForSectionRef = useRef<number | null>(null);
  editStateForSectionRef.current = editStateForSection;
  const sectionEditsRef = useRef<Map<number, SectionEditState>>(
    saved ? new Map(saved.sectionEdits) : new Map()
  );
  const confirmedOverlapsRef = useRef<Map<number, ConfirmedOverlapInfo>>(
    saved ? new Map(saved.confirmedOverlaps ?? []) : new Map()
  );
  const existingShotUpdatesRef = useRef<ExistingShotUpdateLocal[]>(
    saved?.existingShotUpdates ?? []
  );
  const nextTempIdRef = useRef(1);

  // Unified segment context (import-side DMP BFS + existing anchors, target-side geometry)
  const [segmentContext, setSegmentContext] = useState<{
    importSide: ContextSectionGeometry[];
    targetSide: ContextSectionGeometry[];
  } | null>(null);

  // Cache all cross-section suggestions from startup calculation
  const suggestionPoolRef = useRef<CrossSectionSuggestion[]>([]);
  // Keep full pool (unfiltered) so buildBatchImport can find existing-target anchors
  const fullPoolRef = useRef<CrossSectionSuggestion[]>([]);
  const temporalDistanceCacheRef = useRef<Map<string, number>>(new Map());
  const temporalDistanceFetchedOriginsRef = useRef<Set<string>>(new Set());

  // ─── Manual placing phase state ────────────────────────────────────────────
  const [segmentQueue, setSegmentQueue] = useState<number[][]>(saved?.segmentQueue ?? []);
  const currentSegment = segmentQueue[0] ?? null;

  const [segmentPlacementMode, setSegmentPlacementMode] = useState<SegmentPlacementMode>('manual');
  const [segmentAnchor, setSegmentAnchor] = useState<{
    sectionIndex: number; stationIndex: number;
  } | null>(saved?.segmentAnchor ?? null);

  // Preview anchor tracks which station the SectionPreview highlights.
  // Follows dropdown hover; reverts to segmentAnchor on dropdown close.
  const [previewAnchor, setPreviewAnchor] = useState<{
    sectionIndex: number; stationIndex: number;
  } | null>(saved?.segmentAnchor ?? null);
  const segmentAnchorRef = useRef(segmentAnchor);

  // Commit a new anchor: updates committed value, preview, and ref together.
  const commitSegmentAnchor = useCallback((anchor: { sectionIndex: number; stationIndex: number } | null) => {
    setSegmentAnchor(anchor);
    setPreviewAnchor(anchor);
    segmentAnchorRef.current = anchor;
  }, []);

  // When a section dropdown hovers an option, preview it on the map;
  // when dropdown closes without selection, revert to the committed anchor.
  // Uses ref (not state) to avoid stale-closure race conditions.
  const handleAnchorSectionHover = useCallback((value: string | null) => {
    if (value !== null) {
      setPreviewAnchor({ sectionIndex: Number(value), stationIndex: 0 });
    } else {
      setPreviewAnchor(segmentAnchorRef.current);
    }
  }, []);

  // Same for station dropdown hover - reads current section from ref.
  const handleAnchorStationHover = useCallback((value: string | null) => {
    if (value !== null) {
      const current = segmentAnchorRef.current;
      if (current) {
        setPreviewAnchor({ sectionIndex: current.sectionIndex, stationIndex: Number(value) });
      }
    } else {
      setPreviewAnchor(segmentAnchorRef.current);
    }
  }, []);

  // ─── The anchor's two station spaces ──────────────────────────────────────
  //
  // The pick is STORED in visible row space: that is what the station dropdowns
  // list, what the map preview's own click handler talks in once translated, and
  // the space the shot-table mutators re-anchor it in. Two surfaces speak
  // emitted space instead - the payloads, which carry only the rows the import
  // will create, and the previews, which draw those same rows - so each converts
  // at its edge rather than storing a second number that can drift.

  /** The anchor station as the BACKEND counts it. One derivation for every
   *  payload site: the two that shipped the raw pick merged a station
   *  downstream of the one clicked, and a pick past the emitted end made
   *  `merge_stations` refuse the whole batch. */
  const emittedAnchorStation = useCallback(
    (anchor: { sectionIndex: number; stationIndex: number }): number => {
      const edits = sectionEditsRef.current.get(anchor.sectionIndex);
      return edits ? toEmittedStationIndex(edits.shots, anchor.stationIndex) : anchor.stationIndex;
    },
    []
  );

  /** The other direction, for a station the PREVIEW drew: it renders the emitted
   *  rows, so a dot below a blank row is numbered one lower than the row the
   *  user is looking at in the grid. */
  const visibleAnchorStation = useCallback(
    (sectionIndex: number, emittedStationIndex: number): number => {
      const edits = sectionEditsRef.current.get(sectionIndex);
      return edits ? fromEmittedStationIndex(edits.shots, emittedStationIndex) : emittedStationIndex;
    },
    []
  );

  // Per-mode state for manual_placing
  const [fromExistingSectionId, setFromExistingSectionId] = useState<number | null>(null);
  const [fromExistingStationIdx, setFromExistingStationIdx] = useState<number>(0);

  // Track original suggestions for sections skipped during suggesting phase
  const skippedSuggestionsRef = useRef<Map<number, SkippedSuggestionPlacement>>(
    new Map((saved?.skippedSuggestions ?? []).map(([idx, entry]) => [idx, normalizeSkippedSuggestionEntry(entry)]))
  );

  // Accumulated direct placements for manual_placing phase
  const directPlacementsRef = useRef<DirectPlacement[]>(saved?.directPlacements ?? []);

  // DMP index → project section_id for sections imported during early import
  const earlyImportedRef = useRef<Map<number, number>>(
    saved?.earlyImported ? new Map(saved.earlyImported) : new Map()
  );

  // DMP indices of segments the user chose to discard ("Do not place")
  const discardedIndicesRef = useRef<Set<number>>(
    new Set(saved?.discardedIndices ?? [])
  );

  // Skip counter per segment (prevent infinite loops)
  const segmentSkipCountRef = useRef<Map<string, number>>(
    saved ? new Map(saved.segmentSkipCounts) : new Map()
  );

  // Guard against React StrictMode double-firing the mount effect
  // When restoring from saved state, skip the init effect entirely
  const importStartedRef = useRef(!!saved);
  // Prevent duplicate "advance suggestion" actions (double-click skip/confirm)
  // from racing the suggesting→placing transition.
  const suggestionActionInFlightRef = useRef(false);
  // Prevent re-entering early-import transition before state updates land.
  const transitionToPlacingInFlightRef = useRef(false);

  // Accumulate all section IDs created during import for post-import orphan
  // detection - seeded from the saved state for the same reason the receipts
  // below are, and with a sharper failure when it is not: an empty list here
  // reaches App.tsx as "this import created nothing", which skips
  // `suggest_orphan_placements` altogether.
  const importedSectionIdsRef = useRef<number[]>([...(saved?.importedSectionIds ?? [])]);
  // WHICH project those ids belong to, carried with them. `created_at` is the
  // identity `executeCommand`'s own staleness guard uses: immutable for the life
  // of a project, unlike `file_path`. Read once (the ref keeps its first value).
  const importProjectKeyRef = useRef<string | null>(
    saved?.projectKey ?? useProjectStore.getState().project?.created_at ?? null
  );
  // Accumulate the import_receipt from each import_sections call (early/segment/
  // late/pending) into one #391 summary for the whole wizard run, keyed by
  // constant file-level provenance (source label/name/total sections) spread
  // into every call below.
  //
  // Seeded from the saved state (#487): manual map placement CLOSES this dialog
  // (`handleStartSegmentPlacement`) and reopens a fresh instance afterwards, so
  // every receipt banked before the handoff dies with the old instance's refs
  // unless it rides `SavedManualPlacingState`. That is the whole run's receipts,
  // not a subset - the reopened instance completes the wizard, and an empty list
  // there aggregates to `undefined`, which App.tsx reads as "nothing to show".
  const importedReceiptsRef = useRef<ImportReceipt[]>([...(saved?.importedReceipts ?? [])]);
  const dmpFilePath = (useUIStore.getState().dialogData as Record<string, unknown>).filePath as string | undefined;
  const dmpProvenance = {
    sourceLabel: 'mNemo DMP',
    sourceFileName: extractFilename(dmpFilePath ?? ''),
    sectionsFound: dmpFile.sections.length,
  };

  // Refs to avoid stale closures
  const connectionsRef = useRef(connections);
  connectionsRef.current = connections;
  const skippedIndicesRef = useRef(skippedIndices);
  skippedIndicesRef.current = skippedIndices;

  const chooseSegmentAnchorSection = useCallback((segment: number[]): number => {
    return segment.find((idx) => skippedIndicesRef.current.has(idx)) ?? segment[0]!;
  }, []);

  const selectedIndices = useMemo(
    () => Array.from(selectionConfig.selectedIndices).sort((a, b) => a - b),
    [selectionConfig.selectedIndices]
  );

  // Section currently being edited in the right-side editor panel.
  // In suggesting mode this follows the current suggestion section.
  // In manual placing mode this follows the selected segment anchor section.
  const activeEditableSectionIndex = useMemo(() => {
    if (currentSuggestion) return currentSuggestion.importSectionIndex;
    if (phase === 'manual_placing' && segmentAnchor) return segmentAnchor.sectionIndex;
    return null;
  }, [currentSuggestion, phase, segmentAnchor]);

  const totalSelected = selectedIndices.length;

  // Set of section indices that already have a connection
  const connectedIndices = useMemo(() => {
    return new Set(connections.map((c) => c.importSectionIndex));
  }, [connections]);

  // Compute import shot count for current suggestion
  const importShotCount = useMemo(() => {
    if (!currentSuggestion) return 0;
    const section = dmpFile.sections[currentSuggestion.importSectionIndex];
    if (!section) return 0;
    return section.shots.filter((s) => s.shot_type === 'Standard').length;
  }, [currentSuggestion, dmpFile.sections]);

  // Compute merge options for the current suggestion
  const mergeOptions = useMemo(() => {
    if (!currentSuggestion) return [];
    return computeMergeOptions(currentSuggestion, importShotCount);
  }, [currentSuggestion, importShotCount]);

  // Selected merge pair
  const selectedMergePair = useMemo(() => {
    if (mergeOptions.length === 0) return null;
    return mergeOptions.find((o) => o.offset === selectedMergeOffset) ?? mergeOptions[mergeOptions.length - 1]!;
  }, [mergeOptions, selectedMergeOffset]);

  // Reset the merge offset to the end of the repeat when the suggestion changes.
  // The same rule `computeDefaultMergeOffset` applies up front (#685).
  useEffect(() => {
    if (currentSuggestion && mergeOptions.length > 0) {
      setSelectedMergeOffset(defaultMergeOffset(currentSuggestion, importShotCount));
    }
  }, [currentSuggestion, mergeOptions, importShotCount]);

  // Check if the current suggestion targets an existing section that is doubly-anchored
  // (modified overlap shots sit between two positional constraints). When true, only
  // 'existing_data' and 'keep_both' are valid - interpolation / import_data would break
  // a closure or zero-datum constraint.
  //
  // A merge/closure is only an anchor if the partner can independently reach a zero datum
  // without going through this section.  A merge to a free-floating section is NOT a
  // positional constraint - the partner can follow via cascading offset adjustments.
  const isTargetDoublyAnchored = useMemo(() => {
    if (!currentSuggestion) return false;
    if (currentSuggestion.targetType !== 'existing' || currentSuggestion.existingSectionId == null) return false;
    if (currentSuggestion.matchedShots === 0) return false;

    const project = useProjectStore.getState().project;
    if (!project) return false;

    const sid = currentSuggestion.existingSectionId;

    // BFS: can `start` reach a zero datum via merges + aligned closures
    // without traversing through `excluded`?
    const hasDatumPathExcluding = (start: number, excluded: number): boolean => {
      const visited = new Set<number>();
      const queue = [start];
      while (queue.length > 0) {
        const current = queue.pop()!;
        if (current === excluded) continue;
        if (visited.has(current)) continue;
        visited.add(current);
        if (project.zero_datums.some((zd) => zd.station_ref.section_id === current)) {
          return true;
        }
        for (const m of project.station_merges) {
          if (m.station_a.section_id === current && !visited.has(m.station_b.section_id)) {
            queue.push(m.station_b.section_id);
          }
          if (m.station_b.section_id === current && !visited.has(m.station_a.section_id)) {
            queue.push(m.station_a.section_id);
          }
        }
        for (const c of project.loop_closures) {
          if (!c.is_aligned) continue;
          if (c.station_a.section_id === current && !visited.has(c.station_b.section_id)) {
            queue.push(c.station_b.section_id);
          }
          if (c.station_b.section_id === current && !visited.has(c.station_a.section_id)) {
            queue.push(c.station_a.section_id);
          }
        }
      }
      return false;
    };

    // Compute the range of target shot indices that would be modified
    let minShot: number;
    let maxShot: number;
    if (currentSuggestion.direction === 'forward') {
      minShot = currentSuggestion.existingStationIndex;
      maxShot = currentSuggestion.existingStationIndex + currentSuggestion.matchedShots - 1;
    } else {
      minShot = currentSuggestion.existingStationIndex - currentSuggestion.matchedShots;
      maxShot = currentSuggestion.existingStationIndex - 1;
    }

    // Collect anchor station indices on the target section.
    // Zero datums are always anchors.  Merges/closures are only anchors
    // if the partner is independently datum-backed.
    const anchors: number[] = [];

    for (const zd of project.zero_datums) {
      if (zd.station_ref.section_id === sid) anchors.push(zd.station_ref.station_index);
    }
    for (const m of project.station_merges) {
      if (m.station_a.section_id === sid && hasDatumPathExcluding(m.station_b.section_id, sid)) {
        anchors.push(m.station_a.station_index);
      }
      if (m.station_b.section_id === sid && hasDatumPathExcluding(m.station_a.section_id, sid)) {
        anchors.push(m.station_b.station_index);
      }
    }
    for (const c of project.loop_closures) {
      if (!c.is_aligned) continue;
      if (c.station_a.section_id === sid && hasDatumPathExcluding(c.station_b.section_id, sid)) {
        anchors.push(c.station_a.station_index);
      }
      if (c.station_b.section_id === sid && hasDatumPathExcluding(c.station_a.section_id, sid)) {
        anchors.push(c.station_b.station_index);
      }
    }

    const hasFront = anchors.some((idx) => idx <= minShot);
    const hasBack = anchors.some((idx) => idx > maxShot);
    return hasFront && hasBack;
  }, [currentSuggestion]);

  // Unified segment context - BFS through DMP connections + fetch existing segment geometry
  useEffect(() => {
    if (!currentSuggestion) {
      setSegmentContext(null);
      return;
    }

    let cancelled = false;

    const compute = async () => {
      const importIdx = currentSuggestion.importSectionIndex;
      const importSide: ContextSectionGeometry[] = [];
      const targetSide: ContextSectionGeometry[] = [];
      const coveredExistingSectionIds = new Set<number>();

      // Store raw segment geometry so we can position DMP sections relative to existing ones.
      // Maps existing section ID → { shotLines, frameOffsetX/Y, side }
      const existingSectionGeo = new Map<number, ExistingSectionGeoEntry>();

      // Import-side BFS through importing-type connections
      const importExclude = new Set<number>();
      if (currentSuggestion.targetType === 'importing' && currentSuggestion.targetImportIndex != null) {
        importExclude.add(currentSuggestion.targetImportIndex);
      }
      const importBfs = computeDmpBfsContext(importIdx, importExclude, connections, dmpFile.sections);
      importSide.push(...importBfs.dmpContext);

      // Target-side handling
      let targetBfsVisited: Set<number> | undefined;
      if (currentSuggestion.targetType === 'existing' && currentSuggestion.existingSectionId != null) {
        // Fetch segment geometry for the existing target section
        try {
          const result = await invokeIpc<SegmentGeometry>('get_segment_geometry', { sectionId: currentSuggestion.existingSectionId }, { purpose: 'fetch segment geometry for target section' });
          if (cancelled) return;

          for (const s of result.sections) {
            coveredExistingSectionIds.add(s.section_id);
            existingSectionGeo.set(s.section_id, {
              shotLines: s.shot_lines,
              frameOffsetX: 0,
              frameOffsetY: 0,
              side: 'target',
            });
          }

          const ctx: ContextSectionGeometry[] = result.sections
            .filter((s) => s.section_id !== currentSuggestion.existingSectionId)
            .map((s) => ({
              color: s.color,
              shots: s.shot_lines
                .filter((line) => line.length >= 2 && line[0]!.length >= 2 && line[1]!.length >= 2)
                .map((line) => ({
                  from: [line[0]![0]!, line[0]![1]!] as [number, number],
                  to: [line[1]![0]!, line[1]![1]!] as [number, number],
                })),
            }))
            .filter((s) => s.shots.length > 0);

          targetSide.push(...ctx);
        } catch {
          // Non-fatal
        }
      } else if (currentSuggestion.targetType === 'importing' && currentSuggestion.targetImportIndex != null) {
        // Target-side DMP BFS
        const targetExclude = new Set<number>([importIdx]);
        const targetBfs = computeDmpBfsContext(currentSuggestion.targetImportIndex, targetExclude, connections, dmpFile.sections);
        targetSide.push(...targetBfs.dmpContext);
        targetBfsVisited = targetBfs.visitedIndices;

        // Process target-side existing anchors
        for (const anchor of targetBfs.existingAnchors) {
          if (coveredExistingSectionIds.has(anchor.existingSectionId)) continue;
          coveredExistingSectionIds.add(anchor.existingSectionId);

          try {
            const result = await invokeIpc<SegmentGeometry>('get_segment_geometry', { sectionId: anchor.existingSectionId }, { purpose: 'fetch segment geometry for anchor section' });
            if (cancelled) return;

            const anchorGeo = result.sections.find((s) => s.section_id === anchor.existingSectionId);
            if (!anchorGeo) continue;

            const mergePos = stationPositionFromShotLines(anchorGeo.shot_lines, anchor.existingStationIdx);
            if (!mergePos) continue;

            const dx = anchor.x - mergePos.x;
            const dy = anchor.y - mergePos.y;

            for (const s of result.sections) {
              if (coveredExistingSectionIds.has(s.section_id)) continue;
              coveredExistingSectionIds.add(s.section_id);
              existingSectionGeo.set(s.section_id, {
                shotLines: s.shot_lines,
                frameOffsetX: dx,
                frameOffsetY: dy,
                side: 'target',
              });

              const shots = s.shot_lines
                .filter((line) => line.length >= 2 && line[0]!.length >= 2 && line[1]!.length >= 2)
                .map((line) => ({
                  from: [line[0]![0]! + dx, line[0]![1]! + dy] as [number, number],
                  to: [line[1]![0]! + dx, line[1]![1]! + dy] as [number, number],
                }));

              if (shots.length > 0) targetSide.push({ color: s.color, shots });
            }
          } catch {
            // Non-fatal
          }
        }
      }

      // Process import-side existing anchors
      for (const anchor of importBfs.existingAnchors) {
        if (coveredExistingSectionIds.has(anchor.existingSectionId)) continue;
        coveredExistingSectionIds.add(anchor.existingSectionId);

        try {
          const result = await invokeIpc<SegmentGeometry>('get_segment_geometry', { sectionId: anchor.existingSectionId }, { purpose: 'fetch segment geometry for import-side anchor' });
          if (cancelled) return;

          const anchorGeo = result.sections.find((s) => s.section_id === anchor.existingSectionId);
          if (!anchorGeo) continue;

          const mergePos = stationPositionFromShotLines(anchorGeo.shot_lines, anchor.existingStationIdx);
          if (!mergePos) continue;

          const dx = anchor.x - mergePos.x;
          const dy = anchor.y - mergePos.y;

          for (const s of result.sections) {
            if (coveredExistingSectionIds.has(s.section_id)) continue;
            coveredExistingSectionIds.add(s.section_id);
            existingSectionGeo.set(s.section_id, {
              shotLines: s.shot_lines,
              frameOffsetX: dx,
              frameOffsetY: dy,
              side: 'import',
            });

            const shots = s.shot_lines
              .filter((line) => line.length >= 2 && line[0]!.length >= 2 && line[1]!.length >= 2)
              .map((line) => ({
                from: [line[0]![0]! + dx, line[0]![1]! + dy] as [number, number],
                to: [line[1]![0]! + dx, line[1]![1]! + dy] as [number, number],
              }));

            if (shots.length > 0) importSide.push({ color: s.color, shots });
          }
        } catch {
          // Non-fatal
        }
      }

      // Bridge discovery: find DMP sections with confirmed existing connections
      // to sections we have geometry for, but not yet shown by BFS.
      // This handles the case where DMP sections A and B both connect to existing X
      // but have no importing-type edge between them.
      const shownDmpIndices = new Set(importBfs.visitedIndices);
      if (targetBfsVisited) {
        for (const idx of targetBfsVisited) shownDmpIndices.add(idx);
      }

      // Bridge discovery: find DMP sections connected through shared existing sections
      const bridged = discoverBridgedDmpSections(connections, shownDmpIndices, existingSectionGeo, dmpFile.sections);
      importSide.push(...bridged.importSide);
      targetSide.push(...bridged.targetSide);

      if (cancelled) return;
      setSegmentContext(
        importSide.length > 0 || targetSide.length > 0
          ? { importSide, targetSide }
          : null
      );
    };

    compute();
    return () => { cancelled = true; };
  }, [currentSuggestion, connections, dmpFile.sections]);

  // Initialize / restore section edit state when active editable section changes.
  useEffect(() => {
    if (activeEditableSectionIndex == null) {
      setEditStateForSection(null);
      setEditState(null);
      return;
    }

    const idx = activeEditableSectionIndex;

    // Restore from ref if previously edited
    const existing = sectionEditsRef.current.get(idx);
    if (existing) {
      // Re-wrapped in the DEV guard (unlike the trivial-argument sites nearby):
      // `debugLog` itself no-ops in production, but JS evaluates a function's
      // arguments before the call, so the `.filter(...).length` scan below ran
      // on every restore in production regardless - the guard is what actually
      // skips the work, not the no-op.
      if (import.meta.env.DEV) {
        debugLog(
          `[init-effect] RESTORE section=${idx} from sectionEditsRef`,
          `shots.length=${existing.shots.length}`,
          `deleted=${existing.shots.filter(s => s.isDeleted).length}`,
        );
      }
      // The counter is per section, and only the initialize branch below used to
      // seed it: coming back to a section after editing a SHORTER one handed out
      // temp ids this section had already used, so two rows shared a key and the
      // grid painted one row's marks on the other.
      nextTempIdRef.current =
        existing.shots.reduce((max, s) => Math.max(max, s.tempId ?? 0), existing.shots.length) + 1;
      setEditStateForSection(idx);
      setEditState(existing);
      return;
    }

    // Initialize from DMP data + selectionConfig defaults
    const section = dmpFile.sections[idx];
    if (!section) {
      setEditStateForSection(null);
      setEditState(null);
      return;
    }

    const shots = convertToEditableShots(section.shots);
    nextTempIdRef.current = shots.length + 1;
    debugLog(
      `[init-effect] INITIALIZE section=${idx} "${section.name}"`,
      `dmpShots=${section.shots.length} standardShots=${shots.length}`,
    );

    // Color based on position in selection order (matches zero datum dialog)
    const selectedPos = selectedIndices.indexOf(idx);
    const newState: SectionEditState = {
      name: section.name,
      surveyor: selectionConfig.defaultSurveyor || '',
      date: selectionConfig.defaultDate ?? section.date?.split(/[T ]/)[0] ?? '',
      color: sectionPalette[(selectedPos >= 0 ? selectedPos : 0) % sectionPalette.length] ?? '#4ecdc4',
      direction: section.direction === 'out' ? 'out' : 'in',
      quality: 1.0,
      reverse: false,
      shots,
    };

    sectionEditsRef.current.set(idx, newState);
    setEditStateForSection(idx);
    setEditState(newState);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeEditableSectionIndex]);

  // Keep sectionEditsRef in sync with editState changes (user edits to shots/metadata).
  // Guard: only write when editState actually belongs to the active edit section.
  // Without this guard, when the active section changes, this effect can briefly fire
  // with stale editState from the previous section before setEditState takes effect.
  useEffect(() => {
    if (!editState || activeEditableSectionIndex == null) return;
    if (editStateForSection !== activeEditableSectionIndex) {
      debugLog(
        `[sync-effect] SKIPPED stale write: editStateFor=${editStateForSection} !== active=${activeEditableSectionIndex}`,
      );
      return;
    }
    debugLog(
      `[sync-effect] WRITING sectionEditsRef[${activeEditableSectionIndex}]`,
      `editState.shots.length=${editState.shots.length}`,
    );
    sectionEditsRef.current.set(activeEditableSectionIndex, editState);
  }, [editState, activeEditableSectionIndex, editStateForSection]);

  // Compute frozen shot indices - matched overlap shots that must not be deleted
  const frozenShotIndices = useMemo(() => {
    if (!currentSuggestion || !editState) return new Set<number>();

    const { importStationIndex, matchedShots } = currentSuggestion;
    const frozenIds = computeOverlapIds(importStationIndex, matchedShots);

    // Map original IDs to current indices in editState.shots
    const indices = new Set<number>();
    editState.shots.forEach((shot, idx) => {
      if (!shot.isNew && frozenIds.has(shot.id)) indices.add(idx);
    });
    return indices;
  }, [currentSuggestion, editState]);

  // Compute effective shots: applies overlap mode transformations as a derived view.
  // Raw editState is never mutated by overlap settings - effectiveShots is computed.
  const effectiveShots = useMemo(() => {
    if (!editState || !currentSuggestion || currentSuggestion.matchedShots === 0) {
      return editState?.shots ?? [];
    }

    const { importStationIndex, matchedShots, existingMatchedShots, direction } = currentSuggestion;
    const mergeImportStation = selectedMergePair?.importStation ?? importStationIndex;

    const allOverlapIds = computeOverlapIds(importStationIndex, matchedShots);
    const actionableIds = computeActionableIds(importStationIndex, matchedShots, mergeImportStation);

    // Get target shots for data replacement
    let targetShots: TargetShotLike[] | null = null;
    if (overlapMode !== 'import_data' && overlapMode !== 'keep_both') {
      const { targetType, existingSectionId, targetImportIndex, existingStationIndex } = currentSuggestion;

      if (targetType === 'existing' && existingSectionId != null) {
        const currentProject = useProjectStore.getState().project;
        const targetSection = currentProject?.sections.find((s) => s.id === existingSectionId);
        if (targetSection) targetShots = targetSection.shots;
      } else if (targetType === 'importing' && targetImportIndex != null) {
        const targetEdits = sectionEditsRef.current.get(targetImportIndex);
        if (targetEdits) {
          // Station space - see the note on the other target-shot build.
          targetShots = targetEdits.shots.filter(isEmittedShot);
        } else {
          const targetDmpSection = dmpFile.sections[targetImportIndex];
          if (targetDmpSection) {
            targetShots = convertToEditableShots(targetDmpSection.shots);
          }
        }
      }

      // Build a mapping: import overlap shot ID → target shot.
      // The mapping is anchored by existingStationIndex (the suggestion's base),
      // NOT the merge-adjusted station - the merge picker only controls which
      // shots are actionable, not the overlap-to-target correspondence.
      if (targetShots) {
        const shouldReverse = needsDirectionReversal(importStationIndex, direction);
        const overlapParams = { importStationIndex, matchedShots, existingMatchedShots, existingStationIndex, direction };
        const targetMap = new Map<number, TargetShotLike>();
        for (const overlapId of allOverlapIds) {
          const targetIdx = computeTargetShotIndex(overlapId, overlapParams);
          if (targetIdx >= 0 && targetIdx < targetShots.length) {
            const raw = targetShots[targetIdx]!;
            targetMap.set(overlapId, shouldReverse ? reverseTargetShot(raw) : raw);
          }
        }
        // Replace targetShots reference with the map for efficient lookup
        return editState.shots.map((shot) => {
          if (shot.isNew || shot.isDeleted) return shot;
          const isOverlap = !shot.isNew && allOverlapIds.has(shot.id);
          if (!isOverlap) return shot;

          const isActionable = actionableIds.has(shot.id);
          if (isActionable) {
            const target = targetMap.get(shot.id);
            if (target) {
              if (overlapMode === 'existing_data') {
                return applyTargetData(shot, target);
              } else if (overlapMode === 'interpolate') {
                return interpolateShots(shot, target);
              }
            }
            return shot;
          }
          // Past merge station: leave unchanged to keep geometry continuous in preview.
          // Deletion happens at import time in handleConfirm.
          return shot;
        });
      }
    }

    // For import_data / keep_both / no target data available:
    return editState.shots.map((shot) => {
      if (shot.isNew || shot.isDeleted) return shot;
      const isOverlap = !shot.isNew && allOverlapIds.has(shot.id);
      if (!isOverlap) return shot;

      const isActionable = actionableIds.has(shot.id);
      if (overlapMode === 'keep_both') {
        return { ...shot, kind: 'Reference' as const };
      }
      if (!isActionable) {
        // Past merge station: leave unchanged to keep geometry continuous in preview.
        return shot;
      }
      // import_data: keep import values as-is
      return shot;
    });
  }, [editState, currentSuggestion, selectedMergePair, overlapMode, dmpFile.sections]);

  /**
   * Rows the keep-both view DERIVES as Reference.
   *
   * Their glyph is computed, not read back from `editState`, so the kind cell on
   * these rows never showed what a click on it wrote: the display stayed
   * Reference while the write destroyed the Jump the user had set underneath,
   * and the import forces Reference on them again regardless. Nothing to edit,
   * so nothing pretends to be editable.
   */
  const derivedKindIndices = useMemo(() => {
    if (
      overlapMode !== 'keep_both' ||
      !editState ||
      !currentSuggestion ||
      currentSuggestion.matchedShots === 0
    ) {
      return new Set<number>();
    }
    const overlapIds = computeOverlapIds(
      currentSuggestion.importStationIndex,
      currentSuggestion.matchedShots,
    );
    const indices = new Set<number>();
    editState.shots.forEach((shot, idx) => {
      if (!shot.isNew && !shot.isDeleted && overlapIds.has(shot.id)) indices.add(idx);
    });
    return indices;
  }, [overlapMode, editState, currentSuggestion]);

  // Section edit callbacks
  const handleMetadataChange = useCallback((field: keyof SectionEditState, value: string | number | boolean) => {
    setEditState((prev) => prev ? { ...prev, [field]: value } : prev);
  }, []);

  const handleShotChange = useCallback(
    (index: number, field: keyof EditableShot, value: number | string | boolean | null) => {
      if (field === 'kind' && derivedKindIndices.has(index)) return;
      setEditState((prev) => {
        if (!prev) return prev;
        const newShots = [...prev.shots];
        const shot = newShots[index];
        if (shot) {
          newShots[index] = { ...shot, [field]: value };
        }
        return { ...prev, shots: newShots };
      });
    },
    [derivedKindIndices]
  );

  /** The rows as they stand NOW - including a change made earlier in this same
   *  event, which React has not rendered yet. */
  const currentShots = useCallback(
    (): EditableShot[] => pendingShotsRef.current ?? editStateRef.current?.shots ?? [],
    []
  );

  /**
   * The one path a row-count change takes.
   *
   * `arrayIndex` is where the change lands in the FULL array and `delta` how
   * many VISIBLE rows it adds (positive) or removes (negative) there. The
   * segment anchor is a station index in that same visible space, so it is
   * renumbered here, in the same step as the rows: a mutator that wrote only the
   * rows left the pick addressing a station its own dropdown no longer offered.
   *
   * The transition is read from the rows as they stand now and committed OUTSIDE
   * the state updater, which stays pure - StrictMode invokes updaters twice on
   * purpose, and a commit in there re-anchored the pick twice for one edit.
   */
  const applyShotEdit = useCallback(
    (arrayIndex: number, delta: number, mutate: (shots: EditableShot[]) => EditableShot[]) => {
      const before = currentShots();
      pendingShotsRef.current = mutate(before);
      setEditState((prev) => (prev ? { ...prev, shots: mutate(prev.shots) } : prev));

      const anchor = segmentAnchorRef.current;
      if (delta === 0 || !anchor || anchor.sectionIndex !== activeEditableSectionIndex) return;
      const at = visibleBefore(before, arrayIndex);
      const survivorsAfter = visibleShotCount(before) + delta;
      const stationIndex =
        delta < 0
          ? reanchorAfterRunDelete(anchor.stationIndex, at, -delta, survivorsAfter)
          : reanchorStation(anchor.stationIndex, at, delta, survivorsAfter);
      if (stationIndex !== anchor.stationIndex) {
        commitSegmentAnchor({ sectionIndex: anchor.sectionIndex, stationIndex });
      }
    },
    [activeEditableSectionIndex, commitSegmentAnchor, currentShots]
  );

  /** Blank rows to insert, each with its own temp id. */
  const blankRows = useCallback(
    (count: number): EditableShot[] =>
      Array.from({ length: count }, () => {
        const tempId = nextTempIdRef.current++;
        return { ...blankEditableShot(-tempId), tempId };
      }),
    []
  );

  /** Append `count` blank shots in ONE update, each with its own temp id. */
  const handleAddShots = useCallback((count: number) => {
    const rows = blankRows(Math.max(1, count));
    // Past every row: an append cannot renumber a station that already exists.
    applyShotEdit(Number.POSITIVE_INFINITY, rows.length, (shots) => [...shots, ...rows]);
  }, [applyShotEdit, blankRows]);

  const handleAddShot = useCallback(() => handleAddShots(1), [handleAddShots]);

  /** Insert `count` blank shots at `index`, in ONE update with an id per row. */
  const handleInsertShots = useCallback(
    (index: number, count: number, position: 'before' | 'after') => {
      const at = position === 'after' ? index + 1 : index;
      const rows = blankRows(Math.max(1, count));
      applyShotEdit(at, rows.length, (shots) => {
        const next = [...shots];
        next.splice(at, 0, ...rows);
        return next;
      });
    },
    [applyShotEdit, blankRows]
  );

  /** Say so when a row cannot go: the wizard freezes the matched overlap rows,
   *  and refusing them in silence left them on screen while the grid cleared the
   *  selection as though they had been deleted. Same wording as the edit-section
   *  grid, which refuses for the same reason. */
  const refuseFrozenDelete = useCallback((count: number) => {
    useUIStore
      .getState()
      .showToast(t('editSection.frozenNotDeleted', { count }), TOAST_KEYS.DMP_WIZARD_FROZEN_DELETE, 'warning');
  }, [t]);

  const handleDeleteShot = useCallback((index: number) => {
    const shot = currentShots()[index];
    if (!shot || shot.isDeleted === true) return;
    if (frozenShotIndices.has(index)) {
      refuseFrozenDelete(1);
      return;
    }
    applyShotEdit(index, -1, (shots) => {
      const next = [...shots];
      const row = next[index];
      if (row) next[index] = { ...row, isDeleted: true };
      return next;
    });
  }, [applyShotEdit, currentShots, frozenShotIndices, refuseFrozenDelete]);

  /** Delete a selection as ONE decision. Asked one row at a time, the frozen
   *  rows were dropped without a word and the grid collapsed the selection as
   *  though the whole range had gone; the anchor was also re-anchored once per
   *  row, each time against a count that no longer held. */
  const handleDeleteShots = useCallback((indices: number[]): boolean => {
    const shots = currentShots();
    const requested = indices.filter((i) => shots[i] !== undefined && shots[i]!.isDeleted !== true);
    if (requested.length === 0) return false;
    const targets = requested.filter((i) => !frozenShotIndices.has(i));
    if (targets.length < requested.length) refuseFrozenDelete(requested.length - targets.length);
    if (targets.length === 0) return false;

    const doomed = new Set(targets);
    applyShotEdit(Math.min(...targets), -targets.length, (rows) =>
      rows.map((row, i) => (doomed.has(i) ? { ...row, isDeleted: true } : row)),
    );
    return true;
  }, [applyShotEdit, currentShots, frozenShotIndices, refuseFrozenDelete]);

  // Persist overlap mode to config on change
  const handleOverlapModeChange = useCallback((value: OverlapMode) => {
    setOverlapMode(value);
    if (config) {
      updateConfig({ ...config, import: { ...config.import, overlap_mode: value } });
    }
  }, [config, updateConfig]);

  // Auto-switch to 'existing_data' when a doubly-anchored target disables the current mode
  useEffect(() => {
    if (isTargetDoublyAnchored && (overlapMode === 'interpolate' || overlapMode === 'import_data')) {
      handleOverlapModeChange('existing_data');
    }
  }, [isTargetDoublyAnchored, overlapMode, handleOverlapModeChange]);

  // Pick the next suggestion from the cached pool, filtering out connected/skipped sections.
  // The end of the repeat, never the matcher's interior pick (#685): set before
  // the suggestion renders, so its first frame - and a confirm that lands in
  // it - already uses the station the reset effect above would choose.
  const computeDefaultMergeOffset = useCallback((suggestion: CrossSectionSuggestion): number => {
    const section = dmpFile.sections[suggestion.importSectionIndex];
    const importShotCount = section
      ? section.shots.filter((s) => s.shot_type === 'Standard').length
      : 0;
    return defaultMergeOffset(suggestion, importShotCount);
  }, [dmpFile.sections]);

  const ensureTemporalDistances = useCallback(async (pool: CrossSectionSuggestion[]) => {
    const anchors = collectExistingAnchorsForTemporalRerank(pool, connectionsRef.current, dmpFile.sections);
    const missing = anchors.filter((ref) => {
      const key = `${ref.section_id}:${ref.station_index}`;
      return !temporalDistanceFetchedOriginsRef.current.has(key);
    });

    await Promise.all(missing.map(async (origin) => {
      const key = `${origin.section_id}:${origin.station_index}`;
      try {
        const field = await invokeIpc<DistanceField>(
          'get_station_distances_from',
          { from: origin },
          { purpose: 'temporal DMP placement rerank', fields: { origin: key } },
        );
        const distances = distanceMapFromField(origin, field);
        for (const [distanceKey, distance] of distances) {
          temporalDistanceCacheRef.current.set(distanceKey, distance);
        }
        temporalDistanceFetchedOriginsRef.current.add(key);
      } catch (e) {
        debugLog('[dmp-temporal-rerank] distance fetch failed', origin, e);
      }
    }));
  }, []);

  const pickFromPool = useCallback(async () => {
    const connected = new Set(connectionsRef.current.map((c) => c.importSectionIndex));
    const skipped = skippedIndicesRef.current;

    // Build reverse map: for each import-to-import connection, record who targets whom.
    // Used to detect cycles: if A→B exists and we're considering B→A, skip it.
    const importingTargets = new Map<number, Set<number>>();
    for (const c of connectionsRef.current) {
      if (c.targetType === 'importing' && c.targetImportIndex != null) {
        let s = importingTargets.get(c.targetImportIndex);
        if (!s) { s = new Set(); importingTargets.set(c.targetImportIndex, s); }
        s.add(c.importSectionIndex);
      }
    }

    // Filter pool to only remaining sections, excluding cycle-forming connections
    const pool = suggestionPoolRef.current.filter((s) => {
      if (connected.has(s.importSectionIndex) || skipped.has(s.importSectionIndex)) return false;
      // Skip import-to-import suggestions that would create a direct cycle:
      // if the target already has an importing connection pointing back to this section
      if (s.targetType === 'importing' && s.targetImportIndex != null) {
        const sourcesTargetingMe = importingTargets.get(s.importSectionIndex);
        if (sourcesTargetingMe?.has(s.targetImportIndex)) return false;
      }
      return true;
    });
    suggestionPoolRef.current = pool;

    if (pool.length === 0) return false;

    await ensureTemporalDistances(pool);
    const reranked = rerankSuggestionsByTemporalPlan(
      pool,
      dmpFile.sections,
      connectionsRef.current,
      temporalDistanceCacheRef.current,
    );
    suggestionPoolRef.current = reranked.suggestions;
    if (reranked.rejectedCount > 0) {
      debugLog(`[dmp-temporal-rerank] rejected ${reranked.rejectedCount} impossible candidate(s)`);
    }
    if (reranked.suggestions.length === 0) return false;

    const top = reranked.suggestions[0]!;
    // Set default merge option immediately to avoid a one-render race where
    // confirm can read the previous suggestion's offset.
    setSelectedMergeOffset(computeDefaultMergeOffset(top));
    setCurrentSuggestion(top);
    setAlternatives(
      reranked.suggestions
        .filter((s) => s !== top && s.importSectionIndex === top.importSectionIndex)
        .slice(0, 4)
    );
    return true;
  }, [computeDefaultMergeOffset, dmpFile.sections, ensureTemporalDistances]);

  /** A section that reached the emit loop and produced nothing would otherwise
   *  disappear between the wizard and the map: it is not in the `unreachable`
   *  report either, which lists only what was never placed. */
  const reportEmptySections = useCallback((indices: number[]) => {
    if (indices.length === 0) return;
    const names = indices.map(
      (i) => sectionEditsRef.current.get(i)?.name ?? dmpFile.sections[i]?.name ?? `Section ${i}`,
    );
    useUIStore.getState().showToast(
      t('dmpWizard.toasts.emptySections', { names: names.join(', ') }),
      TOAST_KEYS.DMP_WIZARD_EMPTY_SECTIONS,
      'warning',
    );
  }, [dmpFile.sections, t]);

  // Transition from suggesting phase: import reachable sections immediately (early import),
  // then enter manual_placing for unreachable segments, or complete if none.
  const transitionToPlacingOrManual = useCallback(async () => {
    if (transitionToPlacingInFlightRef.current) return;
    transitionToPlacingInFlightRef.current = true;
    setIsImporting(true);
    setError(null);

    try {
      let unreachable: number[] = [];
      try {
        // 1. Apply overlap treatments (single source of truth)
        let existingShotUpdates: ExistingShotUpdateLocal[] = [];
        if (fullPoolRef.current.length > 0) {
          existingShotUpdates = applyAllOverlapTreatments(
            fullPoolRef.current,
            confirmedOverlapsRef.current,
            sectionEditsRef.current,
            dmpFile,
            overlapMode,
          );
          existingShotUpdatesRef.current = existingShotUpdates;
        }

        // 2. Redirect connections targeting deleted overlap zones
        redirectDeletedTargets(
          connectionsRef.current,
          sectionEditsRef.current,
          confirmedOverlapsRef.current,
          fullPoolRef.current,
        );

        // 3. Build batch import configs for reachable sections
        const zeroDatumIdx = zeroDatumConfig?.sectionIndex ?? null;
        let emittedDmpIndices: number[];
        let configs: BackendImportSectionConfig[];
        let emptySections: number[];
        ({ configs, unreachable, emittedDmpIndices, emptySections } = buildBatchImport(
          connectionsRef.current,
          zeroDatumIdx,
          dmpFile,
          selectionConfig,
          sectionEditsRef.current,
          fullPoolRef.current,
          directPlacementsRef.current,
          undefined,
          skippedIndicesRef.current,
          resolvedTheme,
        ));
        reportEmptySections(emptySections);

        // 4. Import reachable sections immediately
        if (configs.length > 0) {
          const emittedSet = new Set(emittedDmpIndices);
          const earlyExistingShotUpdates = backendExistingShotUpdatesForSections(
            existingShotUpdatesRef.current,
            emittedSet,
          );
          const result = await invokeIpc<CommandResult>('import_sections', {
            sections: configs,
            fileHash: selectionConfig.fileHash ?? null,
            sectionHashes: selectionConfig.sectionHashes ?? null,
            existingShotUpdates: earlyExistingShotUpdates.length > 0 ? earlyExistingShotUpdates : null,
            virtualShotMode: 'AllReferences',
            ...dmpProvenance,
          }, { purpose: 'import reachable sections (early import)' });

          if (!result.success) {
            setError(result.error ?? 'Import failed');
            return;
          }

          if (result.section_ids.length > 0) {
            importedSectionIdsRef.current.push(...result.section_ids);
          }
          if (result.import_receipt) importedReceiptsRef.current.push(result.import_receipt);

          await refreshProjectState();
          // Imports bypass executeCommand, so mark dirty ourselves - the sections
          // then register as unsaved (autosave + the project-close guard);
          // window-close is guarded separately by isImportInProgress (App.tsx) (#110).
          markSuccessfulDmpImportDirty();

          // Build DMP index -> section_id mapping for late import.
          // Use backend response ordering directly; it matches submitted configs.
          for (let i = 0; i < emittedDmpIndices.length; i++) {
            const sectionId = result.section_ids[i];
            if (sectionId != null) {
              earlyImportedRef.current.set(emittedDmpIndices[i]!, sectionId);
            }
          }
          existingShotUpdatesRef.current = removeExistingShotUpdatesForSections(
            existingShotUpdatesRef.current,
            emittedSet,
          );
        }
      } catch (e) {
        console.error('[earlyImport] Error during early import:', e);
        setError(importErrorMessage(e));
        return;
      } finally {
        setIsImporting(false);
      }

      // 5. Check for unreachable sections
      if (unreachable.length > 0) {
        const segments = computeUnreachableSegments(unreachable, connectionsRef.current);
        if (segments.length > 0) {
          setSegmentQueue(segments);
          segmentSkipCountRef.current = new Map();

          // Initialize anchor to first section's station 0
          const firstSegment = segments[0]!;
          commitSegmentAnchor({ sectionIndex: chooseSegmentAnchorSection(firstSegment), stationIndex: 0 });
          setSegmentPlacementMode('manual');
          setAutoStartManualPlacementPending(true);
          // The shot table follows the suggestion first and nothing ever cleared
          // it, so entering manual placing with an exhausted pool left the grid
          // editing the last SUGGESTED section while the anchor named another.
          setCurrentSuggestion(null);

          setPhase('manual_placing');
          return;
        }
      }

      // No unreachable - all done
      trackDialogComplete('dmp-import-new');
      onComplete(importedSectionIdsRef.current, aggregateImportReceipts(importedReceiptsRef.current));
    } finally {
      transitionToPlacingInFlightRef.current = false;
      suggestionActionInFlightRef.current = false;
    }
  }, [zeroDatumConfig, dmpFile, selectionConfig, overlapMode, onComplete, refreshProjectState, reportEmptySections, resolvedTheme]);

  // On mount: import zero datum section and fire suggest_all_import_placements
  useEffect(() => {
    if (importStartedRef.current) return;
    importStartedRef.current = true;

    const init = async () => {
      // 1. Import zero datum section if provided (this is the anchor)
      if (zeroDatumConfig) {
        setIsImporting(true);
        const { sectionIndex } = zeroDatumConfig;
        const config: BackendImportSectionConfig = buildZeroDatumSectionConfig(zeroDatumConfig);

        // 'AllReferences', not Auto (#27): the DMP parser only ever emits
        // Normal shots, so every virtual shot reaching this import is one the
        // user marked Reference here - the "Keep both (reference)" overlap
        // treatment. Auto would re-judge them by the chain rule and promote the
        // bounded ones to Jump, putting the duplicated geometry back into gas
        // and penetration, which is the double-count keep-both exists to avoid.
        // AllReferences leaves Normal shots untouched and preserves the choice.
        // try/finally, unlike the other four import sites which already had it:
        // a throw here used to leave `isImporting` stuck true forever (this
        // `init()` is called with no catch). That was merely a stuck spinner
        // until the exits started refusing to open mid-import - now it would
        // TRAP the user in a wizard with Cancel and Back both disabled.
        let result: CommandResult;
        try {
          result = await importSections(
            [config], selectionConfig.fileHash, selectionConfig.sectionHashes, 'AllReferences', dmpProvenance,
          );
        } catch (e) {
          setError(importErrorMessage(e));
          return;
        } finally {
          setIsImporting(false);
        }

        if (!result.success) {
          setError(result.error ?? 'Failed to import zero datum section');
          return;
        }

        if (result.section_ids.length > 0) {
          importedSectionIdsRef.current.push(...result.section_ids);
        }
        if (result.import_receipt) importedReceiptsRef.current.push(result.import_receipt);

        // Snap-fit to the newly placed zero datum section
        useUIStore.getState().triggerFitToBounds({ snap: true });

        // Check if only 1 section was selected - we're done
        if (selectedIndices.length === 1 && selectedIndices[0] === sectionIndex) {
          setPhase('done');
          return;
        }
      } else {
        // No zero datum - check if we can suggest
        const existingProject = useProjectStore.getState().project;
        const hasExistingSections = (existingProject?.sections.length ?? 0) > 0;

        if (!hasExistingSections && selectedIndices.length <= 1) {
          setPhase('done');
          return;
        }
      }

      // 2. Fire suggest_all_import_placements with all remaining sections
      setIsCalculating(true);
      const zeroDatumIdx = zeroDatumConfig?.sectionIndex ?? null;

      const remainingSections: ImportSectionForSuggestion[] = [];
      for (const idx of selectedIndices) {
        if (idx === zeroDatumIdx) continue;
        const section = dmpFile.sections[idx];
        if (!section) continue;

        const editableShots = convertToEditableShots(section.shots);
        const shotData: ShotData[] = editableShots.map(editableShotToShotData);
        if (shotData.length === 0) continue;

        remainingSections.push({
          index: idx,
          name: section.name,
          shots: shotData,
          // Scored in the frame these readings are actually in (#438). Without
          // it the backend falls back to the project default, and a DMP section
          // would be matched against an existing Compass section across a
          // declination - a real overlap scoring as a mismatch.
          // camelCase here, snake_case on `ImportSectionConfig`: the two IPC
          // structs differ in their serde rename, and the generated types are
          // the authority.
          azimuthReference: DMP_AZIMUTH_REFERENCE,
        });
      }

      if (remainingSections.length === 0) {
        setIsCalculating(false);
        setPhase('done');
        return;
      }

      try {
        const suggestions = await invokeIpc<CrossSectionSuggestion[]>(
          'suggest_all_import_placements',
          { importSections: remainingSections },
          { purpose: 'suggest import placements for remaining sections' },
        );

        suggestionPoolRef.current = suggestions;
        fullPoolRef.current = suggestions;
      } catch {
        // Suggestion failure is non-fatal
        suggestionPoolRef.current = [];
      }

      setIsCalculating(false);

      if (!(await pickFromPool())) {
        // No suggestions at all - check for unreachable sections
        transitionToPlacingOrManual();
        return;
      }

      setPhase('suggesting');
    };

    init();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // On restore: import a just-placed segment immediately so it appears on the map
  const pendingSegmentImportedRef = useRef(false);
  useEffect(() => {
    if (!saved?.pendingSegmentImport || saved.pendingSegmentImport.length === 0) return;
    if (pendingSegmentImportedRef.current) return;
    pendingSegmentImportedRef.current = true;

    const importPendingSegment = async () => {
      setIsImporting(true);
      try {
        const zeroDatumIdx = zeroDatumConfig?.sectionIndex ?? null;
        const { configs, emittedDmpIndices, emptySections } = buildBatchImport(
          connectionsRef.current,
          zeroDatumIdx,
          dmpFile,
          selectionConfig,
          sectionEditsRef.current,
          fullPoolRef.current,
          directPlacementsRef.current,
          earlyImportedRef.current,
          undefined,
          resolvedTheme,
        );
        reportEmptySections(emptySections);

        // Filter to only the sections in the pending segment, remapping from_importing indices
        const pendingSet = new Set(saved.pendingSegmentImport);
        const segmentConfigs: BackendImportSectionConfig[] = [];
        const segmentDmpIndices: number[] = [];
        // Map original config position → new position in segmentConfigs
        const posRemap = new Map<number, number>();
        for (let i = 0; i < configs.length; i++) {
          const dmpIdx = emittedDmpIndices[i];
          if (dmpIdx != null && pendingSet.has(dmpIdx)) {
            posRemap.set(i, segmentConfigs.length);
            segmentConfigs.push({ ...configs[i]!, section_index: segmentConfigs.length });
            segmentDmpIndices.push(dmpIdx);
          }
        }
        // Remap from_importing target_section_index to new positions
        for (const cfg of segmentConfigs) {
          if (cfg.placement.type === 'from_importing') {
            const oldTarget = (cfg.placement as { target_section_index: number }).target_section_index;
            const newTarget = posRemap.get(oldTarget);
            if (newTarget != null) {
              (cfg.placement as { target_section_index: number }).target_section_index = newTarget;
            }
          }
        }

        if (segmentConfigs.length > 0) {
          const pendingExistingShotUpdates = backendExistingShotUpdatesForSections(
            existingShotUpdatesRef.current,
            pendingSet,
          );
          const result = await invokeIpc<CommandResult>('import_sections', {
            sections: segmentConfigs,
            fileHash: selectionConfig.fileHash ?? null,
            sectionHashes: selectionConfig.sectionHashes ?? null,
            existingShotUpdates: pendingExistingShotUpdates.length > 0 ? pendingExistingShotUpdates : null,
            virtualShotMode: 'AllReferences',
            ...dmpProvenance,
          }, { purpose: 'import segment sections' });

          if (result.success) {
            if (result.section_ids.length > 0) {
              importedSectionIdsRef.current.push(...result.section_ids);
            }
            if (result.import_receipt) importedReceiptsRef.current.push(result.import_receipt);

            await refreshProjectState();
            markSuccessfulDmpImportDirty();

            for (let i = 0; i < segmentDmpIndices.length; i++) {
              const sectionId = result.section_ids[i];
              if (sectionId != null) {
                earlyImportedRef.current.set(segmentDmpIndices[i]!, sectionId);
              }
            }
            existingShotUpdatesRef.current = removeExistingShotUpdatesForSections(
              existingShotUpdatesRef.current,
              pendingSet,
            );
          }
        }
      } catch (e) {
        // Non-fatal - segment will be retried in late import
        console.error('[pendingSegmentImport] Error:', e);
      } finally {
        setIsImporting(false);
      }

      // If this was the last segment, finish the import - don't wait for the
      // placing phase which would race with isImporting state.
      if (segmentQueue.length === 0) {
        trackDialogComplete('dmp-import-new');
        onComplete(importedSectionIdsRef.current, aggregateImportReceipts(importedReceiptsRef.current));
      }
    };

    importPendingSegment();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // When restored from manual placement with an empty segment queue, transition to placing.
  // Skip if a pending segment import is in progress - it will call onComplete directly.
  useEffect(() => {
    if (phase === 'manual_placing' && segmentQueue.length === 0 && !saved?.pendingSegmentImport?.length) {
      setPhase('placing');
    }
  }, [phase, segmentQueue.length, saved?.pendingSegmentImport?.length]);

  // Handle 'placing' phase - late import of remaining sections after manual_placing.
  // Overlap treatment and connection redirect already happened during early import.
  useEffect(() => {
    if (phase !== 'placing') return;
    if (isImporting) return;

    const doLateImport = async () => {
      setIsImporting(true);
      setError(null);

      let unreachable: number[] = [];
      try {
        const zeroDatumIdx = zeroDatumConfig?.sectionIndex ?? null;
        let configs: BackendImportSectionConfig[];
        let emittedDmpIndices: number[];
        let emptySections: number[];
        ({ configs, unreachable, emittedDmpIndices, emptySections } = buildBatchImport(
          connectionsRef.current,
          zeroDatumIdx,
          dmpFile,
          selectionConfig,
          sectionEditsRef.current,
          fullPoolRef.current,
          directPlacementsRef.current,
          earlyImportedRef.current,
          undefined,
          resolvedTheme,
        ));
        reportEmptySections(emptySections);

        if (configs.length > 0) {
          const emittedSet = new Set(emittedDmpIndices);
          const lateExistingShotUpdates = backendExistingShotUpdatesForSections(
            existingShotUpdatesRef.current,
            emittedSet,
          );
          const result = await invokeIpc<CommandResult>('import_sections', {
            sections: configs,
            fileHash: selectionConfig.fileHash ?? null,
            sectionHashes: selectionConfig.sectionHashes ?? null,
            existingShotUpdates: lateExistingShotUpdates.length > 0 ? lateExistingShotUpdates : null,
            virtualShotMode: 'AllReferences',
            ...dmpProvenance,
          }, { purpose: 'import remaining sections (late import)' });

          if (!result.success) {
            setError(result.error ?? 'Late import failed');
            return;
          }

          if (result.section_ids.length > 0) {
            importedSectionIdsRef.current.push(...result.section_ids);
          }
          if (result.import_receipt) importedReceiptsRef.current.push(result.import_receipt);

          await refreshProjectState();
          markSuccessfulDmpImportDirty();
          existingShotUpdatesRef.current = removeExistingShotUpdatesForSections(
            existingShotUpdatesRef.current,
            emittedSet,
          );
        }
      } catch (e) {
        console.error('[lateImport] Error:', e);
        setError(importErrorMessage(e));
        return;
      } finally {
        setIsImporting(false);
      }

      // Filter out discarded segments - they should not be placed later.
      const remaining = unreachable.filter(idx => !discardedIndicesRef.current.has(idx));
      if (remaining.length > 0) {
        const segments = computeUnreachableSegments(remaining, connectionsRef.current);
        if (segments.length > 0) {
          setSegmentQueue(segments);
          segmentSkipCountRef.current = new Map();

          const firstSegment = segments[0]!;
          commitSegmentAnchor({ sectionIndex: chooseSegmentAnchorSection(firstSegment), stationIndex: 0 });
          const hasSuggestion = firstSegment.some(idx => skippedSuggestionsRef.current.has(idx));
          setSegmentPlacementMode(hasSuggestion ? 'suggestion' : 'manual');
          setFromExistingSectionId(null);
          setFromExistingStationIdx(0);
          setAutoStartManualPlacementPending(true);
          setError(null);
          setCurrentSuggestion(null);
          setPhase('manual_placing');
          return;
        }
      }

      trackDialogComplete('dmp-import-new');
      onComplete(importedSectionIdsRef.current, aggregateImportReceipts(importedReceiptsRef.current));
    };

    doLateImport();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [phase, placingRetry]);

  // Handle 'done' phase (only reached when no sections to place)
  useEffect(() => {
    if (phase !== 'done') return;
    if (isImporting || isCalculating) return;

    const remaining = selectedIndices.filter(
      (idx) => !connectedIndices.has(idx) && idx !== zeroDatumConfig?.sectionIndex
    );

    if (remaining.length > 0) {
      const segments = computeUnreachableSegments(remaining, connectionsRef.current);
      if (segments.length > 0) {
        setSegmentQueue(segments);
        segmentSkipCountRef.current = new Map();

        const firstSegment = segments[0]!;
        commitSegmentAnchor({ sectionIndex: chooseSegmentAnchorSection(firstSegment), stationIndex: 0 });
        const hasSuggestion = firstSegment.some(idx => skippedSuggestionsRef.current.has(idx));
        setSegmentPlacementMode(hasSuggestion ? 'suggestion' : 'manual');
        setFromExistingSectionId(null);
        setFromExistingStationIdx(0);
        setAutoStartManualPlacementPending(true);
        setError(null);
        setCurrentSuggestion(null);
        setPhase('manual_placing');
        return;
      }
    }

    trackDialogComplete('dmp-import-new');
    // With the receipt, like the other three completion sites (#487). This one
    // is reached after the zero-datum import has already banked one - a
    // single-selected-section file, or a selection with nothing left to
    // suggest - so dropping it silenced the receipt for those imports too.
    onComplete(importedSectionIdsRef.current, aggregateImportReceipts(importedReceiptsRef.current));
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [phase, isImporting, isCalculating]);

  // Build preview section for import side from original shots (no mode modifications).
  // Both import and target sections display their original geometry so the user can
  // visually compare the overlap regardless of the selected overlap mode.
  const importPreviewSection = useMemo<DmpSection | null>(() => {
    if (!currentSuggestion) return null;
    const section = dmpFile.sections[currentSuggestion.importSectionIndex];
    if (!section) return null;
    if (!editState) return section;

    return {
      ...editableShotsToDmpSection(section, editState.shots),
      name: editState.name,
      direction: editState.direction,
    };
  }, [currentSuggestion, dmpFile.sections, editState]);

  // Build merge preview config for SectionPreview (existing target)
  const mergePreviewConfig = useMemo((): MergePreviewConfig | null => {
    if (!currentSuggestion || currentSuggestion.targetType !== 'existing') return null;
    if (currentSuggestion.existingSectionId == null) return null;

    const currentProject = useProjectStore.getState().project;
    if (!currentProject) return null;

    const targetSection = currentProject.sections.find(
      (s) => s.id === currentSuggestion.existingSectionId
    );
    if (!targetSection) return null;

    return {
      targetSection,
      importStationIndex: selectedMergePair?.importStation ?? currentSuggestion.importStationIndex,
      targetStationIndex: selectedMergePair?.existingStation ?? currentSuggestion.existingStationIndex,
    };
  }, [currentSuggestion, selectedMergePair]);

  // Build DMP merge preview config for import-to-import targets
  const dmpMergePreviewConfig = useMemo((): DmpMergePreviewConfig | null => {
    if (!currentSuggestion || currentSuggestion.targetType !== 'importing') return null;
    if (currentSuggestion.targetImportIndex == null) return null;

    const targetSection = dmpFile.sections[currentSuggestion.targetImportIndex];
    if (!targetSection) return null;

    const targetColorIdx = (currentSuggestion.targetImportIndex ?? 0) % sectionPalette.length;

    return {
      targetDmpSection: targetSection,
      targetColor: sectionPalette[targetColorIdx] ?? '#95a5a6',
      importStationIndex: selectedMergePair?.importStation ?? currentSuggestion.importStationIndex,
      targetStationIndex: selectedMergePair?.existingStation ?? currentSuggestion.existingStationIndex,
    };
  }, [currentSuggestion, selectedMergePair, dmpFile.sections, sectionPalette]);

  // Merge target-side + import-side context for SectionPreview.
  // Import-side context is relative to the import section's station 0 and needs the same
  // merge offset that SectionPreview applies to align the import section with the target.
  const mergedContextSections = useMemo((): ContextSectionGeometry[] | null => {
    const parts: ContextSectionGeometry[] = [];
    if (segmentContext?.targetSide) parts.push(...segmentContext.targetSide);
    if (segmentContext?.importSide && segmentContext.importSide.length > 0) {
      const activePreview = mergePreviewConfig ?? dmpMergePreviewConfig;
      let dx = 0;
      let dy = 0;
      if (activePreview && importPreviewSection) {
        // Compute the same importOffset that SectionPreview uses internally
        const importStations = computeStationsFromDmpShots(
          importPreviewSection.shots.filter((s) => s.shot_type === 'Standard')
        );
        let targetStations: Array<{ x: number; y: number }> = [];
        if (mergePreviewConfig) {
          targetStations = computeStationsFromShots(
            mergePreviewConfig.targetSection.shots
          );
        } else if (dmpMergePreviewConfig) {
          targetStations = computeStationsFromDmpShots(
            dmpMergePreviewConfig.targetDmpSection.shots.filter((s) => s.shot_type === 'Standard')
          );
        }
        const importStn = importStations[activePreview.importStationIndex];
        const targetStn = targetStations[activePreview.targetStationIndex];
        if (importStn && targetStn) {
          dx = targetStn.x - importStn.x;
          dy = targetStn.y - importStn.y;
        }
      }
      parts.push(...segmentContext.importSide.map((ctx) => ({
        color: ctx.color,
        shots: ctx.shots.map((shot) => ({
          from: [shot.from[0] + dx, shot.from[1] + dy] as [number, number],
          to: [shot.to[0] + dx, shot.to[1] + dy] as [number, number],
        })),
      })));
    }
    return parts.length > 0 ? parts : null;
  }, [segmentContext, mergePreviewConfig, dmpMergePreviewConfig, importPreviewSection]);

  // Import color for the current section (from editState, which tracks user changes)
  const importColor = useMemo(() => {
    if (editState?.color) return editState.color;
    const sectionIdx = currentSuggestion?.importSectionIndex ?? -1;
    const pos = selectedIndices.indexOf(sectionIdx);
    return sectionPalette[(pos >= 0 ? pos : 0) % sectionPalette.length] ?? '#4ecdc4';
  }, [editState?.color, currentSuggestion?.importSectionIndex, selectedIndices, sectionPalette]);

  // Target section name for display
  const targetSectionName = useMemo(() => {
    if (!currentSuggestion) return '';
    return currentSuggestion.existingSectionName;
  }, [currentSuggestion]);

  // Confirm handler - records a virtual connection + overlap info (instant, no backend call).
  // Overlap treatment is deferred to applyAllOverlapTreatments which runs before batch import.
  /**
   * Refuse while ANY section that will be sent holds a half-filled row.
   *
   * Every selected section's edits are serialized, not just the one on screen,
   * so a gate on the active section alone let a cleared cell in section B walk
   * past it (switching the active section asks nothing) and die at Rust's
   * deserializer with an error that named nothing. The section is named here for
   * the same reason.
   *
   * Read FRESH from the refs: the wizard's actions suppress exhaustive-deps, so
   * a captured count is stale exactly when it matters - clear a cell, click
   * Confirm without touching anything else, and the old callback still remembers
   * zero.
   */
  const refuseIncompleteShots = useCallback(() => {
    const live = new Map(sectionEditsRef.current);
    // The active section's newest rows reach `sectionEditsRef` an effect later.
    const active = editStateForSectionRef.current;
    if (active != null && editStateRef.current) live.set(active, editStateRef.current);
    const blocking = findIncompleteSections(selectedIndices, live)[0];
    if (!blocking) return false;
    useUIStore.getState().showToast(
      t('dmpWizard.toasts.sectionIncomplete', { name: blocking.name, count: blocking.count }),
      TOAST_KEYS.DMP_WIZARD_INCOMPLETE,
      'warning',
    );
    return true;
  }, [selectedIndices, t]);

  const handleConfirm = useCallback(async () => {
    if (!currentSuggestion) return;
    if (refuseIncompleteShots()) return;
    if (suggestionActionInFlightRef.current || transitionToPlacingInFlightRef.current) return;
    suggestionActionInFlightRef.current = true;
    try {
      debugLog(
        `[handleConfirm] import=${currentSuggestion.importSectionIndex} "${currentSuggestion.importSectionName}"`,
        `→ target=${currentSuggestion.targetType === 'importing' ? currentSuggestion.targetImportIndex : currentSuggestion.existingSectionId}`,
        `"${currentSuggestion.existingSectionName}"`,
        `type=${currentSuggestion.targetType}`,
        `impStn=${currentSuggestion.importStationIndex} exStn=${currentSuggestion.existingStationIndex}`,
        `matched=${currentSuggestion.matchedShots} dir=${currentSuggestion.direction}`,
      );
      setError(null);

      // Read fresh edit state from the ref (synchronously up-to-date) instead of
      // the editState React state which may be one render behind when the user
      // confirms quickly after a suggestion change.
      const importIdx = currentSuggestion.importSectionIndex;
      const freshEditState = sectionEditsRef.current.get(importIdx);
      if (!freshEditState) return; // shouldn't happen - init-effect always populates

      // Save raw user edits unchanged - NO overlap treatment here.
      // applyAllOverlapTreatments will handle all overlap mutation before batch import.
      sectionEditsRef.current.set(importIdx, freshEditState);

      // Record confirmed overlap info so applyAllOverlapTreatments knows the merge station
      const mergeImportStation = selectedMergePair?.importStation ?? currentSuggestion.importStationIndex;
      confirmedOverlapsRef.current.set(importIdx, {
        importStationIndex: currentSuggestion.importStationIndex,
        matchedShots: currentSuggestion.matchedShots,
        existingMatchedShots: currentSuggestion.existingMatchedShots,
        direction: currentSuggestion.direction,
        existingStationIndex: currentSuggestion.existingStationIndex,
        targetType: currentSuggestion.targetType,
        existingSectionId: currentSuggestion.existingSectionId ?? null,
        targetImportIndex: currentSuggestion.targetImportIndex ?? null,
        mergeImportStation,
      });

      // Use selected merge pair for station indices (falls back to suggestion's original)
      const mergeExistingStation = selectedMergePair?.existingStation ?? currentSuggestion.existingStationIndex;

      const newConn: VirtualConnection = {
        importSectionIndex: currentSuggestion.importSectionIndex,
        importStationIndex: mergeImportStation,
        targetType: currentSuggestion.targetType,
        ...(currentSuggestion.existingSectionId != null && { existingSectionId: currentSuggestion.existingSectionId }),
        ...(currentSuggestion.targetType === 'existing' && { existingStationIndex: mergeExistingStation }),
        ...(currentSuggestion.targetImportIndex != null && { targetImportIndex: currentSuggestion.targetImportIndex }),
        ...(currentSuggestion.targetType === 'importing' && { targetImportStationIndex: mergeExistingStation }),
      };

      setConnections((prev) => [...prev, newConn]);
      // Update ref eagerly
      connectionsRef.current = [...connectionsRef.current, newConn];

      // Check if all sections are now connected or skipped
      const newConnected = new Set(connectionsRef.current.map((c) => c.importSectionIndex));
      const zeroDatumIdx = zeroDatumConfig?.sectionIndex ?? null;
      const remaining = selectedIndices.filter(
        (idx) => idx !== zeroDatumIdx && !newConnected.has(idx) && !skippedIndicesRef.current.has(idx)
      );

      if (remaining.length === 0) {
        // All handled - transition to placing (or manual_placing if unreachable)
        transitionToPlacingOrManual();
        return;
      }

      // Pick next from pool
      if (!(await pickFromPool())) {
        // Pool exhausted - check for unreachable sections
        transitionToPlacingOrManual();
        return;
      }
    } finally {
      if (!transitionToPlacingInFlightRef.current) {
        suggestionActionInFlightRef.current = false;
      }
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentSuggestion, selectedMergePair, zeroDatumConfig, selectedIndices, pickFromPool, transitionToPlacingOrManual]);

  // Skip handler (suggesting phase)
  const handleSkip = useCallback(async () => {
    if (refuseIncompleteShots()) return;
    if (!currentSuggestion) return;
    if (suggestionActionInFlightRef.current || transitionToPlacingInFlightRef.current) return;
    suggestionActionInFlightRef.current = true;
    try {
      const skippedIdx = currentSuggestion.importSectionIndex;
      const mergeImportStation = selectedMergePair?.importStation ?? currentSuggestion.importStationIndex;
      const mergeExistingStation = selectedMergePair?.existingStation ?? currentSuggestion.existingStationIndex;

      // Record the suggestion so it can be re-shown in manual_placing
      skippedSuggestionsRef.current.set(skippedIdx, {
        suggestion: currentSuggestion,
        mergeImportStation,
        mergeExistingStation,
      });

      setSkippedIndices((prev) => {
        const next = new Set(prev);
        next.add(skippedIdx);
        return next;
      });

      // Update ref eagerly
      const newSkipped = new Set(skippedIndicesRef.current);
      newSkipped.add(skippedIdx);
      skippedIndicesRef.current = newSkipped;

      const zeroDatumIdx = zeroDatumConfig?.sectionIndex ?? null;
      const newConnected = new Set(connectionsRef.current.map((c) => c.importSectionIndex));
      const remaining = selectedIndices.filter(
        (idx) => idx !== zeroDatumIdx && !newConnected.has(idx) && !newSkipped.has(idx)
      );

      if (remaining.length === 0) {
        transitionToPlacingOrManual();
        return;
      }

      if (!(await pickFromPool())) {
        transitionToPlacingOrManual();
        return;
      }
    } finally {
      if (!transitionToPlacingInFlightRef.current) {
        suggestionActionInFlightRef.current = false;
      }
    }
  }, [currentSuggestion, selectedMergePair, zeroDatumConfig, selectedIndices, pickFromPool, transitionToPlacingOrManual]);

  // Alternative click handler
  const handleAlternativeClick = useCallback((alt: CrossSectionSuggestion) => {
    setSelectedMergeOffset(computeDefaultMergeOffset(alt));
    setCurrentSuggestion(alt);
    setAlternatives((prev) => {
      const newAlts = prev.filter((a) => a !== alt);
      if (currentSuggestion) {
        newAlts.push(currentSuggestion);
        newAlts.sort((a, b) => b.score - a.score);
      }
      return newAlts.slice(0, 4);
    });
  }, [currentSuggestion, computeDefaultMergeOffset]);

  // ─── Manual placing phase handlers ────────────────────────────────────────

  // Access UI store for map placement (use selectors to avoid full-store subscription)
  const startPlacementMode = useUIStore((s) => s.startPlacementMode);
  const closeDialog = useUIStore((s) => s.closeDialog);
  const openDialog = useUIStore((s) => s.openDialog);
  const showToast = useUIStore((s) => s.showToast);
  const setDmpImportWizardPhase = useUIStore((s) => s.setDmpImportWizardPhase);
  const onboardingWizardPhase = useMemo(() => {
    if (phase === 'suggesting' && !isCalculating && currentSuggestion) {
      return 'suggesting' as const;
    }
    if (phase === 'manual_placing' && currentSegment && !autoStartManualPlacementPending) {
      return 'manual_placing' as const;
    }
    return null;
  }, [phase, isCalculating, currentSuggestion, currentSegment, autoStartManualPlacementPending]);

  useEffect(() => {
    setDmpImportWizardPhase(onboardingWizardPhase);
  }, [onboardingWizardPhase, setDmpImportWizardPhase]);

  useEffect(
    () => () => {
      setDmpImportWizardPhase(null);
    },
    [setDmpImportWizardPhase]
  );

  // Project sections and merges for "From Existing" dropdown + preview
  const projectSections = useProjectStore((s) => s.project?.sections ?? []);
  const projectMerges = useProjectStore((s) => s.project?.station_merges ?? []);

  // Get target section stations for "From Existing" mode
  const fromExistingTargetSection = useMemo(() => {
    if (fromExistingSectionId == null) return null;
    return projectSections.find(s => s.id === fromExistingSectionId) ?? null;
  }, [fromExistingSectionId, projectSections]);

  const fromExistingStationCount = useMemo(() => {
    if (!fromExistingTargetSection) return 0;
    return fromExistingTargetSection.shots.length + 1;
  }, [fromExistingTargetSection]);

  // Compute segment preview for manual_placing.
  // Uses effective (post-treatment) sections so the preview reflects deleted
  // shots and interpolated values - matching what will be imported.
  const segmentPreviewSections = useMemo(() => {
    if (!currentSegment) return null;

    // Build effective DmpSections with edits applied
    const effectiveSections = dmpFile.sections.map((sec, idx) =>
      buildEffectiveDmpSection(sec, sectionEditsRef.current.get(idx)),
    );
    // Adjust connection station indices to match post-deletion geometry
    const effectiveConns = adjustConnectionsForEdits(connections, sectionEditsRef.current);
    const layout = computeSegmentLayout(currentSegment, effectiveConns, effectiveSections);

    return layout.map((entry) => {
      const section = effectiveSections[entry.sectionIndex];
      if (!section) return null;
      const edits = sectionEditsRef.current.get(entry.sectionIndex);
      const selectedPos = selectedIndices.indexOf(entry.sectionIndex);
      const sectionColor = (edits?.color || null) ?? sectionPalette[(selectedPos >= 0 ? selectedPos : entry.sectionIndex) % sectionPalette.length] ?? '#95a5a6';
      return {
        section,
        color: sectionColor,
        sectionIndex: entry.sectionIndex,
        offsetX: entry.offsetX,
        offsetY: entry.offsetY,
      };
    }).filter((x): x is NonNullable<typeof x> => x !== null);
  }, [currentSegment, connections, dmpFile.sections, sectionPalette]);

  // Compute target segment context for "From Existing" preview (shows the target section's
  // connected segment as dimmed lines behind the import segment preview)
  const fromExistingContextSections = useMemo((): ContextSectionGeometry[] | null => {
    if (segmentPlacementMode !== 'fromExisting' || fromExistingSectionId == null) return null;
    if (!segmentAnchor || !segmentPreviewSections || segmentPreviewSections.length === 0) return null;

    // Build adjacency from project station_merges
    const mergeAdj = new Map<number, Array<{ neighborId: number; thisIdx: number; neighborIdx: number }>>();
    for (const m of projectMerges) {
      const a = m.station_a;
      const b = m.station_b;
      if (!mergeAdj.has(a.section_id)) mergeAdj.set(a.section_id, []);
      if (!mergeAdj.has(b.section_id)) mergeAdj.set(b.section_id, []);
      mergeAdj.get(a.section_id)!.push({ neighborId: b.section_id, thisIdx: a.station_index, neighborIdx: b.station_index });
      mergeAdj.get(b.section_id)!.push({ neighborId: a.section_id, thisIdx: b.station_index, neighborIdx: a.station_index });
    }

    // BFS from target section through merges to discover connected segment
    const visited = new Set<number>();
    const queue: Array<{ sectionId: number; offsetX: number; offsetY: number }> = [];
    visited.add(fromExistingSectionId);
    queue.push({ sectionId: fromExistingSectionId, offsetX: 0, offsetY: 0 });

    const entries: Array<{ sectionId: number; stations: Array<{ x: number; y: number }>; offsetX: number; offsetY: number }> = [];

    while (queue.length > 0) {
      const { sectionId, offsetX, offsetY } = queue.shift()!;
      const section = projectSections.find((s) => s.id === sectionId);
      if (!section) continue;

      const stnPositions = computeStationsFromShots(section.shots);
      entries.push({ sectionId, stations: stnPositions, offsetX, offsetY });

      for (const edge of mergeAdj.get(sectionId) ?? []) {
        if (visited.has(edge.neighborId)) continue;
        visited.add(edge.neighborId);

        const neighborSection = projectSections.find((s) => s.id === edge.neighborId);
        if (!neighborSection) continue;

        const neighborStns = computeStationsFromShots(neighborSection.shots);
        const thisPos = stnPositions[edge.thisIdx];
        const neighborPos = neighborStns[edge.neighborIdx];
        if (!thisPos || !neighborPos) continue;

        queue.push({
          sectionId: edge.neighborId,
          offsetX: offsetX + thisPos.x - neighborPos.x,
          offsetY: offsetY + thisPos.y - neighborPos.y,
        });
      }
    }

    // Align target segment to import segment: target station → import anchor station
    const targetEntry = entries.find((e) => e.sectionId === fromExistingSectionId);
    if (!targetEntry) return null;
    const targetStn = targetEntry.stations[fromExistingStationIdx];
    if (!targetStn) return null;

    const anchorEntry = segmentPreviewSections.find((e) => e.sectionIndex === segmentAnchor.sectionIndex);
    if (!anchorEntry) return null;
    const anchorStns = computeStationsFromDmpShots(
      anchorEntry.section.shots.filter((s) => s.shot_type === 'Standard'),
    );
    // `anchorStns` comes from the effective (emitted) section - so must the index.
    const anchorStn = anchorStns[emittedAnchorStation(segmentAnchor)];
    if (!anchorStn) return null;

    const gx = (anchorEntry.offsetX + anchorStn.x) - (targetEntry.offsetX + targetStn.x);
    const gy = (anchorEntry.offsetY + anchorStn.y) - (targetEntry.offsetY + targetStn.y);

    // Build ContextSectionGeometry for each section in the target segment
    const result: ContextSectionGeometry[] = [];
    for (const entry of entries) {
      const shots: Array<{ from: [number, number]; to: [number, number] }> = [];
      for (let i = 0; i < entry.stations.length - 1; i++) {
        const from = entry.stations[i]!;
        const to = entry.stations[i + 1]!;
        shots.push({
          from: [from.x + entry.offsetX + gx, from.y + entry.offsetY + gy],
          to: [to.x + entry.offsetX + gx, to.y + entry.offsetY + gy],
        });
      }
      if (shots.length > 0) {
        result.push({
          color: entry.sectionId === fromExistingSectionId ? '#95a5a6' : '#6b7280',
          shots,
        });
      }
    }

    return result.length > 0 ? result : null;
  }, [segmentPlacementMode, fromExistingSectionId, fromExistingStationIdx, projectSections, projectMerges, segmentAnchor, segmentPreviewSections, emittedAnchorStation]);

  // Current segment's skipped suggestion (if any section in the segment had one)
  const segmentSkippedSuggestion = useMemo(() => {
    if (!currentSegment) return null;
    for (const idx of currentSegment) {
      const suggestion = skippedSuggestionsRef.current.get(idx);
      if (suggestion) return suggestion;
    }
    return null;
  }, [currentSegment]);

  // Advance to next segment or transition to placing
  const advanceSegmentQueue = useCallback(() => {
    setSegmentQueue((prev) => {
      const next = prev.slice(1);
      if (next.length === 0) {
        // Queue empty - transition to placing
        setPhase('placing');
        return [];
      }
      // Initialize anchor for next segment
      const firstSegment = next[0]!;
      commitSegmentAnchor({ sectionIndex: chooseSegmentAnchorSection(firstSegment), stationIndex: 0 });
      const hasSuggestion = firstSegment.some(idx => skippedSuggestionsRef.current.has(idx));
      setSegmentPlacementMode(hasSuggestion ? 'suggestion' : 'manual');
      setFromExistingSectionId(null);
      setFromExistingStationIdx(0);
      return next;
    });
  }, [chooseSegmentAnchorSection, commitSegmentAnchor]);

  // Place segment handler
  const handleSegmentPlace = useCallback(() => {
    if (refuseIncompleteShots()) return;
    if (!currentSegment || !segmentAnchor) return;
    const anchorStation = emittedAnchorStation(segmentAnchor);

    let placement: ImportPlacement;
    if (segmentPlacementMode === 'suggestion' && segmentSkippedSuggestion) {
      // Use the skipped suggestion as a from_existing connection
      const suggestion = segmentSkippedSuggestion.suggestion;
      if (suggestion.targetType === 'existing' && suggestion.existingSectionId != null) {
        // Add as a virtual connection instead
        const newConn: VirtualConnection = {
          importSectionIndex: suggestion.importSectionIndex,
          importStationIndex: segmentSkippedSuggestion.mergeImportStation,
          targetType: 'existing',
          existingSectionId: suggestion.existingSectionId,
          existingStationIndex: segmentSkippedSuggestion.mergeExistingStation,
        };
        setConnections((prev) => [...prev, newConn]);
        connectionsRef.current = [...connectionsRef.current, newConn];
        advanceSegmentQueue();
        return;
      } else if (suggestion.targetType === 'importing' && suggestion.targetImportIndex != null) {
        const newConn: VirtualConnection = {
          importSectionIndex: suggestion.importSectionIndex,
          importStationIndex: segmentSkippedSuggestion.mergeImportStation,
          targetType: 'importing',
          targetImportIndex: suggestion.targetImportIndex,
          targetImportStationIndex: segmentSkippedSuggestion.mergeExistingStation,
        };
        setConnections((prev) => [...prev, newConn]);
        connectionsRef.current = [...connectionsRef.current, newConn];
        advanceSegmentQueue();
        return;
      }
      return;
    } else if (segmentPlacementMode === 'fromExisting') {
      if (fromExistingSectionId == null) return;
      placement = {
        type: 'from_existing',
        import_station: anchorStation,
        existing_station: {
          section_id: fromExistingSectionId,
          station_index: fromExistingStationIdx,
        },
      };
    } else if (segmentPlacementMode === 'discard') {
      // Discard: skip this segment entirely without creating a placement
      for (const idx of currentSegment) {
        discardedIndicesRef.current.add(idx);
      }
      advanceSegmentQueue();
      return;
    } else if (segmentPlacementMode === 'manual') {
      // Manual mode is handled by handleStartSegmentPlacement
      return;
    } else {
      return;
    }

    const dp: DirectPlacement = {
      anchorSectionIndex: segmentAnchor.sectionIndex,
      anchorStationIndex: anchorStation,
      placement,
    };
    directPlacementsRef.current = [...directPlacementsRef.current, dp];
    advanceSegmentQueue();
  }, [currentSegment, segmentAnchor, segmentPlacementMode, segmentSkippedSuggestion, fromExistingSectionId, fromExistingStationIdx, advanceSegmentQueue, emittedAnchorStation, refuseIncompleteShots]);

  // Skip segment handler - appends to end of queue
  const handleSegmentSkip = useCallback(() => {
    if (!currentSegment) return;
    const key = currentSegment.join(',');
    const count = (segmentSkipCountRef.current.get(key) ?? 0) + 1;
    segmentSkipCountRef.current.set(key, count);

    setSegmentQueue((prev) => {
      if (prev.length <= 1) {
        // Only one segment left - can't skip, it stays
        return prev;
      }
      const [first, ...rest] = prev;
      const next = [...rest, first!];
      // Initialize anchor for next segment
      const nextSeg = next[0]!;
      commitSegmentAnchor({ sectionIndex: chooseSegmentAnchorSection(nextSeg), stationIndex: 0 });
      const hasSuggestion = nextSeg.some(idx => skippedSuggestionsRef.current.has(idx));
      setSegmentPlacementMode(hasSuggestion ? 'suggestion' : 'origin');
      setFromExistingSectionId(null);
      setFromExistingStationIdx(0);
      return next;
    });
  }, [currentSegment, chooseSegmentAnchorSection, commitSegmentAnchor]);

  // Station click handler for segment preview. The index arrives in the
  // preview's own (emitted) numbering - see the note on the two spaces.
  const handleSegmentStationClick = useCallback((stationIndex: number, sectionIndex?: number) => {
    const idx = sectionIndex ?? (currentSegment && currentSegment.length > 0 ? currentSegment[0]! : null);
    if (idx == null) return;
    commitSegmentAnchor({ sectionIndex: idx, stationIndex: visibleAnchorStation(idx, stationIndex) });
  }, [currentSegment, commitSegmentAnchor, visibleAnchorStation]);

  const buildSegmentPlacementPayload = useCallback(
    (segment: number[], anchor: { sectionIndex: number; stationIndex: number }) => {
      const anchorDmpSection = dmpFile.sections[anchor.sectionIndex];
      if (!anchorDmpSection) {
        return null;
      }

      const edits = sectionEditsRef.current.get(anchor.sectionIndex);
      const shots: ShotData[] = edits
        ? edits.shots.filter(isEmittedShot).map(editableShotToShotData)
        : convertToEditableShots(anchorDmpSection.shots).map(editableShotToShotData);
      const emittedStationIndex = emittedAnchorStation(anchor);

      const sectionData = buildPlacementSectionData({
        dmpSection: anchorDmpSection,
        edits,
        defaultSurveyor: selectionConfig.defaultSurveyor,
        defaultDate: selectionConfig.defaultDate,
        palette: sectionPalette,
        paletteIndex: selectedIndices.indexOf(anchor.sectionIndex),
        shots,
      });

      // Build additional sections for segment shadow using effective (post-treatment)
      // sections so the overlay reflects deleted shots and interpolated values.
      const effectiveSections = dmpFile.sections.map((sec, idx) =>
        buildEffectiveDmpSection(sec, sectionEditsRef.current.get(idx)),
      );
      const effectiveConns = adjustConnectionsForEdits(connectionsRef.current, sectionEditsRef.current);
      const layout = computeSegmentLayout(segment, effectiveConns, effectiveSections);
      // Offsets from computeSegmentLayout are relative to station 0 of the root
      // section (segmentIndices[0]). Re-base them relative to the anchor section's
      // station 0 so PlacementOverlay positions them correctly.
      const anchorLayoutEntry = layout.find((entry) => entry.sectionIndex === anchor.sectionIndex);
      const anchorLayoutOffsetX = anchorLayoutEntry?.offsetX ?? 0;
      const anchorLayoutOffsetY = anchorLayoutEntry?.offsetY ?? 0;
      const additionalSections = layout
        .filter((entry) => entry.sectionIndex !== anchor.sectionIndex)
        .map((entry) => {
          const sec = effectiveSections[entry.sectionIndex];
          if (!sec) return null;
          const secEdits = sectionEditsRef.current.get(entry.sectionIndex);
          const secSelectedPos = selectedIndices.indexOf(entry.sectionIndex);
          const secColor =
            (secEdits?.color || null) ??
            sectionPalette[(secSelectedPos >= 0 ? secSelectedPos : entry.sectionIndex) % sectionPalette.length] ??
            '#95a5a6';
          const secShots = secEdits
            ? secEdits.shots.filter(isEmittedShot).map(editableShotToShotData)
            : convertToEditableShots(sec.shots).map(editableShotToShotData);
          return {
            sectionData: {
              name: secEdits?.name ?? sec.name,
              surveyor: null,
              date: null,
              color: secColor,
              direction: secEdits?.direction ?? sec.direction,
              quality: secEdits?.quality ?? 1.0,
              shots: secShots,
            },
            color: secColor,
            offsetX: entry.offsetX - anchorLayoutOffsetX,
            offsetY: entry.offsetY - anchorLayoutOffsetY,
          };
        })
        .filter((value): value is NonNullable<typeof value> => value !== null);

      return { sectionData, additionalSections, emittedStationIndex };
    },
    [dmpFile.sections, emittedAnchorStation, selectionConfig.defaultDate, selectionConfig.defaultSurveyor, selectedIndices, sectionPalette],
  );

  const importPlacedSegmentNow = useCallback(
    async (segmentIndices: number[], directPlacements: DirectPlacement[]) => {
      const zeroDatumIdx = zeroDatumConfig?.sectionIndex ?? null;
      const { configs, emittedDmpIndices, emptySections } = buildBatchImport(
        connectionsRef.current,
        zeroDatumIdx,
        dmpFile,
        selectionConfig,
        sectionEditsRef.current,
        fullPoolRef.current,
        directPlacements,
        earlyImportedRef.current,
        undefined,
        resolvedTheme,
      );
      reportEmptySections(emptySections);

      const pendingSet = new Set(segmentIndices);
      const segmentConfigs: BackendImportSectionConfig[] = [];
      const segmentDmpIndices: number[] = [];
      // Map original config position -> new position in segmentConfigs
      const posRemap = new Map<number, number>();
      for (let i = 0; i < configs.length; i++) {
        const dmpIdx = emittedDmpIndices[i];
        if (dmpIdx != null && pendingSet.has(dmpIdx)) {
          posRemap.set(i, segmentConfigs.length);
          segmentConfigs.push({ ...configs[i]!, section_index: segmentConfigs.length });
          segmentDmpIndices.push(dmpIdx);
        }
      }

      for (const cfg of segmentConfigs) {
        if (cfg.placement.type === 'from_importing') {
          const placement = cfg.placement as {
            type: 'from_importing';
            import_station: number;
            target_section_index: number;
            target_station_index: number;
          };
          const oldTarget = placement.target_section_index;
          const newTarget = posRemap.get(oldTarget);
          if (newTarget != null) {
            placement.target_section_index = newTarget;
            continue;
          }
          const targetDmpIdx = emittedDmpIndices[oldTarget];
          const existingTargetId = targetDmpIdx != null
            ? earlyImportedRef.current.get(targetDmpIdx)
            : undefined;
          if (existingTargetId != null) {
            cfg.placement = {
              type: 'from_existing',
              import_station: placement.import_station,
              existing_station: {
                section_id: existingTargetId,
                station_index: placement.target_station_index,
              },
            };
            continue;
          }
          setError('Segment import failed: unresolved relative placement target');
          return false;
        }
      }

      if (segmentConfigs.length === 0) {
        return true;
      }

      const pendingSetForUpdates = new Set(segmentDmpIndices);
      const segmentExistingShotUpdates = backendExistingShotUpdatesForSections(
        existingShotUpdatesRef.current,
        pendingSetForUpdates,
      );
      const result = await invokeIpc<CommandResult>('import_sections', {
        sections: segmentConfigs,
        fileHash: selectionConfig.fileHash ?? null,
        sectionHashes: selectionConfig.sectionHashes ?? null,
        existingShotUpdates: segmentExistingShotUpdates.length > 0 ? segmentExistingShotUpdates : null,
        virtualShotMode: 'AllReferences',
        ...dmpProvenance,
      }, { purpose: 'import placed segment sections' });

      if (!result.success) {
        setError(result.error ?? 'Segment import failed');
        return false;
      }

      if (result.section_ids.length > 0) {
        importedSectionIdsRef.current.push(...result.section_ids);
      }
      // Banked BEFORE the refresh below deliberately: if that throws, the catch
      // in the placement callback reopens with the same queue and the retry
      // banks a second receipt - which is the honest count, because the backend
      // did import twice. Moving this after the await would under-report.
      if (result.import_receipt) importedReceiptsRef.current.push(result.import_receipt);

      await refreshProjectState();
      markSuccessfulDmpImportDirty();

      for (let i = 0; i < segmentDmpIndices.length; i++) {
        const sectionId = result.section_ids[i];
        if (sectionId != null) {
          earlyImportedRef.current.set(segmentDmpIndices[i]!, sectionId);
        }
      }
      existingShotUpdatesRef.current = removeExistingShotUpdatesForSections(
        existingShotUpdatesRef.current,
        pendingSetForUpdates,
      );

      return true;
    },
    [dmpFile, refreshProjectState, reportEmptySections, selectionConfig, zeroDatumConfig?.sectionIndex, resolvedTheme],
  );

  /**
   * Hand this segment off to map placement.
   *
   * Carries NO payload guard, deliberately (#476). `refuseIncompleteShots`
   * exists to protect an IPC payload, and this hands off nothing: the payload
   * leaves later, from `importPlacedSegmentNow`, once the user has dropped the
   * section. Gating the handoff on the completeness of the WHOLE selection is
   * what turned one zero-length row in the file into a dead end - the auto-start
   * effect below runs this while the dialog is hidden, so a refusal here was
   * invisible AND unrecoverable. The guard stays where a payload is actually at
   * stake: `handlePlaceOnMapClick` (the user-initiated button), the segment
   * submit, and `handleConfirm`/`handleSkip`.
   *
   * It also no longer clears `autoStartManualPlacementPending`. That window is
   * owned by the effect that opened it - see there.
   */
  const handleStartSegmentPlacement = useCallback(() => {
    // The payload is still guarded: importPlacedSegmentNow -> buildBatchImport -> assertSendableShots.
    if (!currentSegment || !segmentAnchor) return;

    // Reopen data (same as what App.tsx passes)
    const reopenData = {
      dmpFile,
      filePath: (useUIStore.getState().dialogData as Record<string, unknown>).filePath,
      selectionConfig,
      zeroDatumConfig,
    };

    const buildSavedState = (
      queue: number[][],
      anchor: { sectionIndex: number; stationIndex: number },
      directPlacements: DirectPlacement[],
    ): SavedManualPlacingState => ({
      connections: [...connectionsRef.current],
      skippedIndices: Array.from(skippedIndicesRef.current),
      segmentQueue: queue,
      segmentAnchor: anchor,
      directPlacements,
      skippedSuggestions: Array.from(skippedSuggestionsRef.current.entries()),
      segmentSkipCounts: Array.from(segmentSkipCountRef.current.entries()),
      sectionEdits: Array.from(sectionEditsRef.current.entries()),
      confirmedOverlaps: Array.from(confirmedOverlapsRef.current.entries()),
      existingShotUpdates: [...existingShotUpdatesRef.current],
      earlyImported: Array.from(earlyImportedRef.current.entries()),
      discardedIndices: Array.from(discardedIndicesRef.current),
      // Read live, not captured: the segment import that precedes the reopen
      // (`importPlacedSegmentNow`) banks its receipt into this ref first, and
      // this builder runs inside the placement callback, after it.
      importedReceipts: [...importedReceiptsRef.current],
      // Same liveness requirement, same call: that import also banks the
      // section ids the reopened instance must complete with.
      importedSectionIds: [...importedSectionIdsRef.current],
      // Which project they mean. Ids alone are ambiguous across a switch.
      projectKey: importProjectKeyRef.current,
    });

    const openPlacementDialog = (
      queue: number[][],
      anchor: { sectionIndex: number; stationIndex: number },
      directPlacements: DirectPlacement[],
    ) => {
      setSource('dialog');
      openDialog('dmp-import-new', {
        ...reopenData,
        autoStartManualPlacement: false,
        savedManualPlacingState: buildSavedState(queue, anchor, directPlacements),
      });
    };

    const startSegmentPlacement = (
      queue: number[][],
      anchor: { sectionIndex: number; stationIndex: number },
      directPlacements: DirectPlacement[],
    ) => {
      const segment = queue[0];
      if (!segment) {
        openPlacementDialog([], anchor, directPlacements);
        return;
      }

      const payload = buildSegmentPlacementPayload(segment, anchor);
      if (!payload) {
        openPlacementDialog(queue, anchor, directPlacements);
        return;
      }

      startPlacementMode(
        payload.sectionData,
        payload.emittedStationIndex,
        (result: PlacementResult) => {
          const handlePlacementComplete = async () => {
            // The SAME station the overlay was aligned to. Storing the visible
            // index here while placing against the emitted one meant the preview
            // and the import disagreed, and the segment landed a station too far
            // downstream whenever a blank row preceded the anchor.
            const alignStation = payload.emittedStationIndex;
            const placement: ImportPlacement = result.snappedToStation
              ? {
                  type: 'manual_snap',
                  align_station_index: alignStation,
                  snapped_to_station: result.snappedToStation,
                }
              : {
                  type: 'manual',
                  align_station_index: alignStation,
                  offset: result.position,
                };

            const dp: DirectPlacement = {
              anchorSectionIndex: anchor.sectionIndex,
              anchorStationIndex: alignStation,
              placement,
            };

            const nextQueue = queue.slice(1);
            const nextDirectPlacements = [...directPlacements, dp];
            directPlacementsRef.current = nextDirectPlacements;

            let segmentImported = false;
            setIsImporting(true);
            try {
              segmentImported = await importPlacedSegmentNow(segment, nextDirectPlacements);
            } catch (e) {
              console.error('[manualPlacement] Immediate segment import failed:', e);
              setError(importErrorMessage(e));
            } finally {
              setIsImporting(false);
            }
            if (!segmentImported) {
              openPlacementDialog(queue, anchor, directPlacements);
              return;
            }

            if (nextQueue.length > 0) {
              const nextAnchor = {
                sectionIndex: chooseSegmentAnchorSection(nextQueue[0]!),
                stationIndex: 0,
              };
              startSegmentPlacement(nextQueue, nextAnchor, nextDirectPlacements);
              return;
            }

            openPlacementDialog([], anchor, nextDirectPlacements);
          };

          void handlePlacementComplete();
        },
        () => {
          openPlacementDialog(queue, anchor, directPlacements);
        },
        payload.additionalSections.length > 0 ? payload.additionalSections : undefined,
      );

      showToast(
        t('dmpWizard.toasts.placementHint'),
        TOAST_KEYS.DMP_MANUAL_PLACEMENT_HINT,
        'info'
      );
    };

    startSegmentPlacement(segmentQueue, segmentAnchor, [...directPlacementsRef.current]);

    // Close dialog so user can interact with the map
    trackDialogComplete('dmp-import-new');
    closeDialog();
  }, [
    currentSegment,
    segmentAnchor,
    segmentQueue,
    buildSegmentPlacementPayload,
    dmpFile,
    selectionConfig,
    zeroDatumConfig,
    importPlacedSegmentNow,
    chooseSegmentAnchorSection,
    startPlacementMode,
    closeDialog,
    openDialog,
    showToast,
  ]);

  /**
   * "Place on Map", clicked by the user. This is where the payload guard
   * belongs: the click is a deliberate step toward an import, the dialog is
   * VISIBLE, and the refusal toast names a section the user can then go and fix
   * (#476 moved it off the auto-start path, where none of that was true).
   */
  const handlePlaceOnMapClick = useCallback(() => {
    if (refuseIncompleteShots()) return;
    handleStartSegmentPlacement();
  }, [refuseIncompleteShots, handleStartSegmentPlacement]);

  /**
   * The auto-start handoff, and the sole owner of `autoStartManualPlacementPending`.
   *
   * That flag hides the whole dialog (`hidden` -> `display:none` on
   * DialogWrapper's container), so the window it opens MUST be closed by
   * whatever happens next. Its honest meaning is "the handoff has not been
   * ATTEMPTED yet" - a state that any single attempt leaves, whatever the
   * attempt decides.
   *
   * It used to be cleared inside `handleStartSegmentPlacement`, after two early
   * returns, so a callee that declined left the dialog mounted, `display:none`,
   * with `activeDialog` still set - an invisible modal that made the whole app
   * `inert` with no mouse affordance and no retry, because nothing in this
   * effect's dependencies changes on a refusal (#476). Keeping ownership here
   * makes the window unstrandable by any future early return, which is the
   * actual invariant; clearing above each individual `return` would be correct
   * today and re-breakable tomorrow.
   *
   * `isImporting` is the ONLY guard that is transient - it flips false and
   * re-runs this effect. Every other reason not to attempt is terminal, so it
   * ends the hidden window instead of waiting forever.
   */
  useEffect(() => {
    if (!autoStartManualPlacementPending) {
      return;
    }
    if (isImporting) {
      return;
    }
    if (phase !== 'manual_placing' || !currentSegment || !segmentAnchor) {
      setAutoStartManualPlacementPending(false);
      return;
    }

    const timeoutId = window.setTimeout(() => {
      // Attempted, so the window is over either way. In the success case the
      // handoff closes the dialog in this same commit, so nothing flashes; in
      // any other case the wizard becomes VISIBLE on the manual-placing screen,
      // which is where the user can act.
      try {
        handleStartSegmentPlacement();
      } finally {
        setAutoStartManualPlacementPending(false);
      }
    }, 0);

    return () => {
      window.clearTimeout(timeoutId);
    };
  }, [
    autoStartManualPlacementPending,
    phase,
    currentSegment,
    segmentAnchor,
    isImporting,
    handleStartSegmentPlacement,
  ]);

  // ─── Cancel / Back ────────────────────────────────────────────────────────
  //
  // This wizard imports EAGERLY - the zero-datum anchor lands in the project on
  // mount, and each reachable segment as it is placed, because the map has to
  // render what it is placing. So by the time either exit is reached the project
  // may already hold sections the user never agreed to keep, and neither exit
  // used to say so: the gate asked only about `connections` (client-side records
  // that have imported nothing), so cancelling right after the zero-datum import
  // closed silently and left the section behind (#394 F2).
  //
  // Both exits therefore offer to WITHDRAW what was imported, and the withdrawal
  // is one batched `delete_section` - one undo entry for one user action, not N.

  /** A withdrawal already running. A REF, not just the state mirror below: two
   *  clicks in the same tick both read the pre-`setState` value, and the second
   *  must not send a second batch for ids the first one is already deleting. */
  const withdrawInFlightRef = useRef(false);
  const [withdrawInFlight, setWithdrawInFlight] = useState(false);

  /** The removal landed in memory but is not yet on disk. Survives a failed
   *  save so a second Remove click RETRIES THE SAVE instead of finding an empty
   *  id list, returning true, and closing over the stale file it just refused to
   *  close over. */
  const withdrawalNeedsSaveRef = useRef(false);

  /** Remove every section this wizard run imported, as ONE undoable step, and
   *  leave the result on disk.
   *  Returns false (leaving the accumulators untouched) if it refused, the
   *  backend did, or the follow-up save did, so the caller can keep the wizard
   *  open on a visible error instead of closing over a project it failed to
   *  clean up. */
  const withdrawImportedSections = useCallback(async (): Promise<boolean> => {
    if (withdrawInFlightRef.current) return false;
    // DEDUPLICATED: this accumulator is seeded from the saved state AND appended
    // to by live imports, so one id can appear twice across the map-placement
    // handoff. `execute_batch` is all-or-nothing and the second delete of an id
    // fails with "not found", which rolls the WHOLE batch back - the withdrawal
    // would remove nothing at all and report an error for it.
    const sectionIds = [...new Set(importedSectionIdsRef.current)];
    if (sectionIds.length === 0 && !withdrawalNeedsSaveRef.current) return true;
    // Section ids are small per-project integers, so a withdrawal aimed at a
    // project that is no longer open would delete same-numbered sections in
    // whatever replaced it - and every id would RESOLVE, so nothing downstream
    // fails safe. Nothing clears `dialogData` on a project switch
    // (`resetToolTransientsForProjectSwitch` leaves the dialog slot alone), so
    // a saved state really can outlive its project. This is the refusal.
    const openProjectKey = useProjectStore.getState().project?.created_at ?? null;
    if (projectSwitched(importProjectKeyRef.current, openProjectKey)) {
      setError(t('dialogs.dmpNewImport.removeImportedWrongProject'));
      return false;
    }
    withdrawInFlightRef.current = true;
    setWithdrawInFlight(true);
    setIsImporting(true);
    try {
      if (sectionIds.length > 0) {
        const fileHashes = selectionConfig.fileHash ? [selectionConfig.fileHash] : [];
        const sectionHashes = selectionConfig.sectionHashes ?? [];
        const result = await executeBatch([
          ...sectionIds.map((sectionId) => ({ type: 'delete_section', section_id: sectionId }) as const),
          // In the SAME batch, and that is the point of batching it: deleting a
          // section does not touch the import-dedup hashes `import_sections`
          // recorded (they are project-level bookkeeping, not references), so
          // without this the project still claims this file was imported while
          // none of it remains - and `DmpSelectionDialog` DESELECTS every section
          // it believes is a duplicate, so the obvious retry silently offers
          // nothing to import. One batch, so one undo restores the sections and
          // their tracking together.
          //
          // Only the hashes THIS run sent to `import_sections`: `sectionHashes`
          // holds the selected sections alone (duplicates were auto-deselected
          // before the wizard opened), so an earlier session's tracking is out of
          // reach. The file hash is coarser - one per file - so a file imported
          // in an earlier session too loses its file-level duplicate flag. That
          // is the intended direction: after a withdrawal the user may well want
          // to re-import it, and the per-section flags still stand.
          ...(fileHashes.length > 0 || sectionHashes.length > 0
            ? [{
                type: 'untrack_import',
                file_hashes: fileHashes,
                section_hashes: sectionHashes,
              } as const]
            : []),
        ]);
        if (!result.success) {
          setError(result.error ?? t('dialogs.dmpNewImport.removeImportedFailed'));
          return false;
        }

        // DIFFED, not cleared: an import that resolved while the batch was in
        // flight has pushed an id the batch never named, and that section is
        // still in the project. Wiping the list would drop it from tracking
        // while it survives on disk - the very shape F2 exists to prevent. (The
        // handlers refuse to start a withdrawal mid-import, so this is the
        // second line of defence, not the first.)
        const removed = new Set(sectionIds);
        importedSectionIdsRef.current = importedSectionIdsRef.current.filter((id) => !removed.has(id));
        for (const dmpIndex of [...earlyImportedRef.current.keys()]) {
          const sectionId = earlyImportedRef.current.get(dmpIndex);
          if (sectionId != null && removed.has(sectionId)) earlyImportedRef.current.delete(dmpIndex);
        }
        // Receipts are per-`import_sections` CALL and carry no section id, so
        // they cannot be diffed - only safe to drop once nothing survives.
        if (importedSectionIdsRef.current.length === 0) importedReceiptsRef.current = [];

        // #394 F14. The eager anchor import already RESOLVED this project's
        // deferred first save (`finalize_deferred_import`), so the file on disk
        // still holds the sections just withdrawn. It has to be written through.
        //
        // Gated as `saveAfterImport` gates its own save, for its reason:
        // importing INTO AN EXISTING project the user may have unrelated pending
        // edits, and committing those behind their back is not this action's to
        // do. There the withdrawal leaves the project dirty and autosave / the
        // close guard carry it, as for any other edit.
        withdrawalNeedsSaveRef.current = useUIStore.getState().importContext === 'new-project';
      }

      // The save is NOT best-effort: closing here is what makes the removal
      // final from the user's side, and a `.stalx` still holding the sections
      // they chose to remove is the exact F2 shape one layer out. So a failed
      // write keeps the wizard open with the reason on screen, and
      // `withdrawalNeedsSaveRef` makes the next Remove click retry the SAVE -
      // without it the retry would find an empty id list, return true, and close
      // over the very file the first attempt refused to close over.
      if (withdrawalNeedsSaveRef.current) {
        const saved = await useProjectStore.getState().saveProject();
        if (!saved.success) {
          setError(saved.error ?? t('dialogs.dmpNewImport.removeImportedSaveFailed'));
          return false;
        }
        withdrawalNeedsSaveRef.current = false;
      }
      return true;
    } catch (e) {
      setError(importErrorMessage(e));
      return false;
    } finally {
      withdrawInFlightRef.current = false;
      setWithdrawInFlight(false);
      setIsImporting(false);
    }
  }, [executeBatch, selectionConfig.fileHash, selectionConfig.sectionHashes, t]);

  /** How many sections the pending confirmation is talking about. Snapshotted
   *  into state when the confirmation opens: the live count lives in a ref, and
   *  a ref does not re-render the copy that quotes it. */
  const [pendingWithdrawCount, setPendingWithdrawCount] = useState(0);

  /**
   * Neither exit may be OPENED while an import is in flight.
   *
   * An import banks its section ids only when it resolves, so a withdrawal
   * decided now would snapshot a list that is about to grow, and the late
   * arrival would survive in the project with nothing tracking it - the exact
   * shape F2 exists to prevent. The footer buttons are disabled for the same
   * reason; this also covers the routes that bypass them (the dialog's own
   * close button and Escape, both of which land on `handleCancel`).
   */
  const exitBlocked = isImporting;

  const handleCancel = useCallback(() => {
    if (isImporting) return;
    const importedCount = importedSectionIdsRef.current.length;
    if (connections.length > 0 || importedCount > 0) {
      setPendingWithdrawCount(importedCount);
      setShowCancelConfirm(true);
    } else {
      onCancel();
    }
  }, [connections.length, onCancel, isImporting]);

  /** Close the wizard, first removing what it imported. */
  const handleCancelWithRemoval = useCallback(async () => {
    if (!(await withdrawImportedSections())) return;
    setShowCancelConfirm(false);
    onCancel();
  }, [withdrawImportedSections, onCancel]);

  const handleBack = useCallback(() => {
    if (isImporting) return;
    const importedCount = importedSectionIdsRef.current.length;
    if (connections.length > 0 || importedCount > 0) {
      setPendingWithdrawCount(importedCount);
      setShowBackConfirm(true);
    } else {
      onBack();
    }
  }, [connections.length, onBack, isImporting]);

  /**
   * Back re-enters the flow at a step BEFORE anything was imported (zero-datum
   * config, or the section selection), and the forward pass re-imports from
   * scratch - the anchor on mount, then the whole suggest/early-import pipeline.
   * So Back has to withdraw first, or the second pass duplicates every section
   * the first one created (#394 F1). Carrying "already imported, don't redo it"
   * across instead cannot work: the mount effect that imports the anchor is the
   * same one that fires `suggest_all_import_placements`, so suppressing it
   * strands the reopened wizard in `init` with nothing to do - and it would also
   * pin the anchor to the GPS/name/shot edits of the FIRST pass, which is
   * exactly what the user pressed Back to change.
   */
  const handleBackWithRemoval = useCallback(async () => {
    if (!(await withdrawImportedSections())) return;
    setShowBackConfirm(false);
    onBack();
  }, [withdrawImportedSections, onBack]);

  // Confidence helpers
  const confidenceColors: Record<string, string> = {
    high: 'bg-success-10 text-success border-success-30',
    medium: 'bg-warning-10 text-warning border-warning-30',
    low: 'bg-background-50 text-text-secondary border-border',
  };

  const directionLabel = (direction: string) =>
    direction === 'forward' ? '\u2192' : '\u2190';

  // Progress - during manual_placing, compute from queue (which has ALL unplaced sections).
  // Otherwise, count connected sections + zero datum.
  const sectionsInQueue = useMemo(() => segmentQueue.flat().length, [segmentQueue]);
  const placedCount = phase === 'manual_placing' || phase === 'placing' || phase === 'done'
    ? totalSelected - sectionsInQueue
    : connectedIndices.size + (zeroDatumConfig ? 1 : 0);
  const skippedCount = skippedIndices.size;

  // Whether we're in the suggestion view with a current suggestion (side-by-side layout)
  const showSideBySide = phase === 'suggesting' && !isCalculating && currentSuggestion != null;

  // Whether we're in the manual_placing phase
  const showManualPlacing = phase === 'manual_placing' && currentSegment != null;

  // Target type label for import-to-import connections
  const isImportTarget = currentSuggestion?.targetType === 'importing';

  // Section names in current segment (for display)
  const segmentSectionNames = useMemo(() => {
    if (!currentSegment) return [];
    return currentSegment.map(idx => dmpFile.sections[idx]?.name ?? `Section ${idx}`);
  }, [currentSegment, dmpFile.sections]);

  // Whether current placement mode is valid for submission
  const isSegmentPlacementValid = useMemo(() => {
    if (!segmentAnchor) return false;
    if (segmentPlacementMode === 'suggestion') return segmentSkippedSuggestion != null;
    if (segmentPlacementMode === 'fromExisting') return fromExistingSectionId != null;
    if (segmentPlacementMode === 'manual') return false; // must use "Place on Map" button
    if (segmentPlacementMode === 'discard') return true;
    return false;
  }, [segmentAnchor, segmentPlacementMode, segmentSkippedSuggestion, fromExistingSectionId]);

  return (
    <DialogWrapper
      title={t('dialogs.dmpNewImport.title')}
      onClose={handleCancel}
      size="fullscreen"
      overflow="hidden"
      bodyClassName="flex-1 flex flex-col p-0 min-h-0"
      hidden={autoStartManualPlacementPending}
      data-testid="dmp-new-import-dialog"
    >
      <div className="flex-1 flex flex-col min-h-0">
        {/* Progress header */}
        <div className="px-4 py-2 border-b border-border text-sm text-text-secondary">
          <span data-testid="wizard-progress">
            {t('dialogs.dmpNewImport.progress', { placed: placedCount, total: totalSelected })}
          </span>
          {skippedCount > 0 && (
            <span className="ml-2" data-testid="wizard-skipped">
              · {t('dialogs.dmpNewImport.skipped', { count: skippedCount })}
            </span>
          )}
        </div>

        {/* Body: side-by-side when showing suggestion, centered otherwise */}
        {showSideBySide ? (
          <WizardScrollBody
            bodyRef={suggestionBodyRef}
            table={
              editState ? (
                <ShotTableEditor
                  shots={effectiveShots}
                  onShotChange={handleShotChange}
                  onAddShot={handleAddShot}
                  onAddShots={handleAddShots}
                  onInsertShots={handleInsertShots}
                  onDeleteShot={handleDeleteShot}
                  onDeleteShots={handleDeleteShots}
                  frozenShotIndices={frozenShotIndices}
                />
              ) : null
            }
          >
            {(bandHeight) => (
            <div className="flex" style={{ minHeight: bandHeight }}>
            <WizardMapPanel
              width={mapPanelWidth}
              onWidthChange={setMapPanelWidth}
              bodyRef={suggestionBodyRef} panelRef={suggestionPreviewRef} height={bandHeight}
            >
              <SectionPreview
                section={importPreviewSection}
                color={importColor}
                size={suggestionPreviewSize.width}
                height={suggestionPreviewSize.height}
                mergePreview={mergePreviewConfig}
                dmpMergePreview={dmpMergePreviewConfig}
                focusStationIndex={selectedMergePair?.importStation ?? currentSuggestion!.importStationIndex}
                contextSections={mergedContextSections}
              />
            </WizardMapPanel>

            {/* Right column: connection details */}
            <div className="flex-1 min-w-0 p-5 space-y-4" data-testid="wizard-suggestion">
              {/* Suggested connection - prominent card */}
              <div className="rounded-lg border border-accent/30 bg-accent/5 p-4 space-y-3">
                <div>
                  <h3 className="text-sm font-semibold text-text-primary">
                    {t('dialogs.dmpNewImport.connectionHeading')}
                  </h3>
                  <p className="text-xs text-text-secondary mt-0.5">
                    {t('dialogs.dmpNewImport.connectionDescription')}
                  </p>
                </div>

                {/* Connection description */}
                <div className="rounded border border-border bg-background p-3" data-testid="wizard-connection-description">
                  <div className="text-sm">
                    <span className="font-medium">&ldquo;{currentSuggestion!.importSectionName}&rdquo;</span>
                    {' '}{t('dialogs.dmpNewImport.station', { index: selectedMergePair?.importStation ?? currentSuggestion!.importStationIndex })}
                  </div>
                  <div className="text-text-secondary text-sm my-1">
                    {t('dialogs.dmpNewImport.connectsTo')}
                  </div>
                  <div className="text-sm">
                    <span className="font-medium">&ldquo;{targetSectionName}&rdquo;</span>
                    {' '}{t('dialogs.dmpNewImport.station', { index: selectedMergePair?.existingStation ?? currentSuggestion!.existingStationIndex })}
                    {isImportTarget && (
                      <span className="ml-1 text-xs text-text-secondary">(importing)</span>
                    )}
                  </div>
                </div>

                {/* Confidence badge */}
                <div className="flex items-center gap-2" data-testid="wizard-confidence">
                  <span className={`px-2 py-1 rounded border text-xs font-medium ${confidenceColors[currentSuggestion!.confidence] ?? ''}`}>
                    {t(`dialogs.dmpNewImport.confidence.${currentSuggestion!.confidence}`)}
                  </span>
                  <span className="text-xs text-text-secondary">
                    · {t('dialogs.dmpNewImport.matchedShots', { count: currentSuggestion!.matchedShots })} {directionLabel(currentSuggestion!.direction)}
                    {currentSuggestion!.isPartial && <span className="ml-0.5 opacity-60">~</span>}
                  </span>
                </div>

                {/* Merge station control */}
                <div className="flex items-center gap-2" data-testid="wizard-merge-station-control">
                  <label className="text-xs text-text-secondary whitespace-nowrap">
                    {t('dialogs.dmpNewImport.mergeStation')}
                  </label>
                  {mergeOptions.length > 1 ? (
                    <Select
                      value={String(selectedMergeOffset)}
                      onChange={(v) => setSelectedMergeOffset(Number(v))}
                      options={mergeOptions.map((opt) => ({
                        value: String(opt.offset),
                        label: `stn ${opt.importStation} → ${targetSectionName} stn ${opt.existingStation}`,
                      }))}
                      size="sm"
                      data-testid="merge-station-select"
                    />
                  ) : (
                    <span className="text-xs text-text-secondary">
                      stn {selectedMergePair?.importStation ?? currentSuggestion!.importStationIndex}
                      {' → '}
                      {targetSectionName} stn{' '}
                      {selectedMergePair?.existingStation ?? currentSuggestion!.existingStationIndex}
                    </span>
                  )}
                </div>

                {/* Overlap control */}
                <div className="flex items-center gap-2" data-testid="wizard-overlap-control">
                  <label className="text-xs text-text-secondary whitespace-nowrap">
                    {t('dialogs.dmpNewImport.overlapMode')}
                  </label>
                  {currentSuggestion && currentSuggestion.matchedShots > 0 ? (
                    <Select
                      value={overlapMode}
                      onChange={(v) => handleOverlapModeChange(v as OverlapMode)}
                      options={[
                        { value: 'interpolate', label: t('dialogs.dmpNewImport.overlapMode_interpolate'), disabled: isTargetDoublyAnchored },
                        { value: 'import_data', label: t('dialogs.dmpNewImport.overlapMode_importData'), disabled: isTargetDoublyAnchored },
                        { value: 'existing_data', label: t('dialogs.dmpNewImport.overlapMode_existingData') },
                        { value: 'keep_both', label: t('dialogs.dmpNewImport.overlapMode_keepBoth') },
                      ]}
                      size="sm"
                      data-testid="overlap-mode-select"
                    />
                  ) : (
                    <span className="text-xs text-text-secondary">
                      {t('dialogs.dmpNewImport.noOverlapShots')}
                    </span>
                  )}
                </div>

                {/* Prompt */}
                <div className="text-sm font-medium">
                  {t('dialogs.dmpNewImport.doesThisLookRight')}
                </div>
              </div>

              {/* Error */}
              {error && (
                <div className="text-sm text-error" data-testid="wizard-error">
                  {error}
                </div>
              )}

              {/* Alternatives */}
              {alternatives.length > 0 && (
                <div>
                  <button
                    type="button"
                    onClick={() => setShowAlternatives(!showAlternatives)}
                    className="text-xs text-accent hover:text-accent-hover flex items-center gap-1"
                    data-testid="toggle-alternatives"
                  >
                    <span className={`transition-transform ${showAlternatives ? 'rotate-90' : ''}`}>&#9654;</span>
                    {showAlternatives
                      ? t('dialogs.dmpNewImport.hideAlternatives')
                      : t('dialogs.dmpNewImport.showAlternatives', { count: alternatives.length })}
                  </button>

                  {showAlternatives && (
                    <div className="mt-2 space-y-1" data-testid="alternatives-list">
                      {alternatives.map((alt, idx) => (
                        <button
                          key={`${alt.importSectionIndex}-${alt.existingSectionId ?? alt.targetImportIndex}-${alt.existingStationIndex}-${alt.importStationIndex}-${alt.direction}`}
                          type="button"
                          className={`w-full p-2 rounded border text-left text-xs flex items-center gap-2 hover:bg-accent-5 transition-colors ${confidenceColors[alt.confidence] ?? ''}`}
                          onClick={() => handleAlternativeClick(alt)}
                          data-testid={`alternative-${idx}`}
                        >
                          <span className={`px-1 py-0.5 rounded text-[10px] font-medium ${confidenceColors[alt.confidence] ?? ''}`}>
                            {t(`dialogs.dmpNewImport.confidence.${alt.confidence}`).split(' ')[0]}
                          </span>
                          <span className="flex-1 truncate">
                            <span className="text-text-secondary">stn {alt.importStationIndex}</span>
                            <span className="text-text-secondary"> → </span>
                            <span className="font-medium">{alt.existingSectionName}</span>
                            <span className="text-text-secondary"> stn {alt.existingStationIndex}</span>
                            {alt.targetType === 'importing' && (
                              <span className="text-text-secondary text-[10px] ml-0.5">(imp)</span>
                            )}
                          </span>
                          <span className="text-text-secondary">
                            {alt.matchedShots} {directionLabel(alt.direction)}
                          </span>
                        </button>
                      ))}
                    </div>
                  )}
                </div>
              )}

              {/* Section Settings */}
              {/* #61 embedding slot: section-name + SectionMetadataForm + ShotTableEditor.
                  Consumed as-is here; the reusable focus-mode section-edit surface
                  (fields-right / table-bottom) drops in here later. Do not refactor.
                  #567 item 2: wrapped in a collapsed-by-default disclosure so it
                  stops competing with the connection card/confidence badge above -
                  the wrapped content itself is unchanged. */}
              {editState && (
                <div className="border-t border-border pt-3">
                  <SuggestionSectionSettings editState={editState} onMetadataChange={handleMetadataChange} />
                </div>
              )}
            </div>
            </div>
            )}
          </WizardScrollBody>
        ) : showManualPlacing ? (
          <WizardScrollBody
            bodyRef={manualBodyRef}
            table={
              editState ? (
                <ShotTableEditor
                  shots={effectiveShots}
                  onShotChange={handleShotChange}
                  onAddShot={handleAddShot}
                  onAddShots={handleAddShots}
                  onInsertShots={handleInsertShots}
                  onDeleteShot={handleDeleteShot}
                  onDeleteShots={handleDeleteShots}
                  frozenShotIndices={frozenShotIndices}
                />
              ) : null
            }
          >
            {(bandHeight) => (
            <div className="flex" style={{ minHeight: bandHeight }}>
            <WizardMapPanel
              width={mapPanelWidth}
              onWidthChange={setMapPanelWidth}
              bodyRef={manualBodyRef} panelRef={manualPreviewRef} height={bandHeight}
            >
              <SectionPreview
                segmentSections={segmentPreviewSections}
                contextSections={fromExistingContextSections}
                size={manualPreviewSize.width}
                height={manualPreviewSize.height}
                onStationClick={handleSegmentStationClick}
                selectedStationIndex={previewAnchor ? emittedAnchorStation(previewAnchor) : null}
                selectedSectionIndex={previewAnchor?.sectionIndex ?? null}
              />
            </WizardMapPanel>

            {/* Right column: Placement configuration */}
            <div className="flex-1 min-w-0 p-5 space-y-4" data-testid="wizard-manual-placing">
              {/* Segment header */}
              <div>
                <div className="text-sm font-medium">
                  {t('dialogs.dmpNewImport.manualPlacing.segmentHeader', {
                    current: segmentQueue.length > 0 ? 1 : 0,
                    total: segmentQueue.length,
                  })}
                </div>
                <div className="text-xs text-text-secondary mt-0.5">
                  {t('dialogs.dmpNewImport.manualPlacing.sections')}: {segmentSectionNames.join(', ')}
                </div>
              </div>

              {/* Placement mode radio group */}
              <SegmentPlacementModes
                segmentSkippedSuggestion={segmentSkippedSuggestion}
                placementMode={segmentPlacementMode}
                onPlacementModeChange={setSegmentPlacementMode}
                currentSegment={currentSegment}
                segmentAnchor={segmentAnchor}
                onCommitSegmentAnchor={commitSegmentAnchor}
                sectionEdits={sectionEditsRef.current}
                dmpSections={dmpFile.sections}
                onAnchorSectionHover={handleAnchorSectionHover}
                onAnchorStationHover={handleAnchorStationHover}
                projectSections={projectSections}
                fromExistingSectionId={fromExistingSectionId}
                onFromExistingSectionChange={setFromExistingSectionId}
                fromExistingStationIdx={fromExistingStationIdx}
                onFromExistingStationChange={setFromExistingStationIdx}
                fromExistingStationCount={fromExistingStationCount}
                onPlaceOnMap={handlePlaceOnMapClick}
              />

              {/* Section settings (same editing surface as section connection / zero datum flows) */}
              {/* #61 embedding slot: section-edit surface goes here later. Do not refactor. */}
              {editState && (
                <>
                  <div className="space-y-2" data-testid="wizard-manual-section-settings">
                    <div className="border-t border-border" />
                    <div className="text-sm font-medium">
                      {t('dialogs.dmpNewImport.sectionSettings')}
                    </div>
                    <div>
                      <label className="block text-sm text-text-secondary mb-1">
                        {t('common.labels.name')}
                      </label>
                      <input
                        type="text"
                        value={editState.name}
                        onChange={(e) => handleMetadataChange('name', e.target.value)}
                        className="w-full px-3 py-2 bg-background border border-border rounded"
                        placeholder={t('sectionMetadata.placeholders.name')}
                        data-testid="section-name-input"
                      />
                    </div>
                  </div>
                  <div className="space-y-4" data-testid="wizard-manual-editable-rest">
                    <SectionMetadataForm
                      name={editState.name}
                      surveyor={editState.surveyor}
                      date={editState.date}
                      color={editState.color}
                      direction={editState.direction}
                      quality={editState.quality}
                      onNameChange={(v) => handleMetadataChange('name', v)}
                      onSurveyorChange={(v) => handleMetadataChange('surveyor', v)}
                      onDateChange={(v) => handleMetadataChange('date', v)}
                      onColorChange={(v) => handleMetadataChange('color', v)}
                      onDirectionChange={(v) => handleMetadataChange('direction', v)}
                      onQualityChange={(v) => handleMetadataChange('quality', v)}
                      reverse={editState.reverse}
                      onReverseChange={(v) => handleMetadataChange('reverse', v)}
                      showName={false}
                      showDirection
                      showReverse
                    />
                  </div>
                </>
              )}

              {/* Error */}
              {error && (
                <div className="text-sm text-error" data-testid="wizard-error">
                  {error}
                </div>
              )}
            </div>
            </div>
            )}
          </WizardScrollBody>
        ) : (
          <div className="flex-1 flex flex-col items-center justify-center p-6 min-h-0 overflow-auto">
            {/* Init/Importing zero datum */}
            {phase === 'init' && isImporting && (
              <div className="text-center" data-testid="wizard-importing-zero-datum">
                <div className="text-lg mb-2">{t('dialogs.dmpNewImport.importingZeroDatum')}</div>
                <div className="animate-pulse text-text-secondary">...</div>
              </div>
            )}

            {/* Calculating suggestions */}
            {(phase === 'init' || phase === 'suggesting') && isCalculating && (
              <div className="text-center" data-testid="wizard-calculating">
                <div className="text-lg mb-2">{t('dialogs.dmpNewImport.calculating')}</div>
                <div className="animate-pulse text-text-secondary">...</div>
              </div>
            )}

            {/* Placing - batch import in progress or failed */}
            {phase === 'placing' && (
              <div className="text-center" data-testid="wizard-placing">
                {error && !isImporting ? (
                  <>
                    <div className="text-lg mb-2 text-error">{error}</div>
                  </>
                ) : (
                  <>
                    <div className="text-lg mb-2">{t('dialogs.dmpNewImport.placing', { count: connectedIndices.size })}</div>
                    <div className="animate-pulse text-text-secondary">...</div>
                  </>
                )}
              </div>
            )}

            {/* Early import in progress (between suggesting and manual_placing/done) */}
            {phase === 'suggesting' && isImporting && (
              <div className="text-center" data-testid="wizard-early-importing">
                <div className="text-lg mb-2">{t('dialogs.dmpNewImport.placing', { count: connectedIndices.size })}</div>
                <div className="animate-pulse text-text-secondary">...</div>
              </div>
            )}

            {/* No suggestions - show status while transitioning to manual placement */}
            {phase === 'suggesting' && !isCalculating && !currentSuggestion && !isImporting && (
              <div className="text-center" data-testid="wizard-no-suggestions">
                <div className="text-lg mb-2">{t('dialogs.dmpNewImport.noSuggestions')}</div>
              </div>
            )}

            {/* Done - all placed */}
            {phase === 'done' && placedCount === totalSelected && (
              <div className="text-center" data-testid="wizard-all-placed">
                <CheckCircleIcon size={32} className="text-success mx-auto mb-2" />
                <div className="text-lg">{t('dialogs.dmpNewImport.allPlaced')}</div>
              </div>
            )}
          </div>
        )}

        {/* Footer */}
        <div
          ref={footerRef}
          data-testid="dmp-import-dialog-footer"
          className="px-4 py-3 border-t border-border flex justify-between items-center"
        >
          <div className="flex gap-2">
            <Button
              onClick={handleCancel}
              disabled={exitBlocked}
              className={importSecondaryButtonClass}
              data-testid="cancel-button"
            >
              {t('common.actions.cancel')}
            </Button>
            <Button
              onClick={handleBack}
              disabled={exitBlocked}
              className={importSecondaryButtonClass}
              data-testid="back-button"
            >
              {t('common.actions.back')}
            </Button>
          </div>
          <div className="flex gap-2" data-testid="wizard-suggestion-actions">
            {phase === 'suggesting' && !isCalculating && currentSuggestion && (
              <>
                <Button
                  onClick={handleSkip}
                  disabled={isImporting}
                  className={importSecondaryButtonClass}
                  data-testid="skip-button"
                >
                  {t('dialogs.dmpNewImport.skip')}
                </Button>
                <Button
                  variant="primary"
                  onClick={handleConfirm}
                  disabled={isImporting}
                  data-testid="confirm-button"
                >
                  {t('dialogs.dmpNewImport.confirm')}
                </Button>
              </>
            )}
            {phase === 'manual_placing' && currentSegment && (
              <>
                {segmentQueue.length > 1 && (
                  <Button
                    onClick={handleSegmentSkip}
                    className={importSecondaryButtonClass}
                    data-testid="segment-skip-button"
                  >
                    {t('dialogs.dmpNewImport.manualPlacing.skip')}
                  </Button>
                )}
                <Button
                  variant="primary"
                  onClick={handleSegmentPlace}
                  disabled={!isSegmentPlacementValid}
                  data-testid="segment-place-button"
                >
                  {segmentPlacementMode === 'discard'
                    ? t('dialogs.dmpNewImport.manualPlacing.next')
                    : t('dialogs.dmpNewImport.manualPlacing.placeSegment')}
                </Button>
              </>
            )}
            {phase === 'placing' && error && !isImporting && (
              <Button
                variant="primary"
                onClick={() => setPlacingRetry((n) => n + 1)}
                data-testid="retry-button"
              >
                {t('common.actions.retry')}
              </Button>
            )}
          </div>
        </div>
      </div>

      {/* Cancel confirmation. Three actions once anything has been imported -
          the exit is no longer just "close a dialog", it decides what happens to
          sections that are already in the project. `confirm-cancel-button` keeps
          its id and its meaning (close, change nothing on disk); the removal is
          the new, additional action. */}
      <AlertDialog
        open={showCancelConfirm}
        title={t('dialogs.dmpNewImport.cancelWizard')}
        // A delete is on the wire; every route out of this confirmation is shut
        // until it lands, including Escape and the backdrop. "Keep and close"
        // above all: it would close the wizard over a removal that still
        // completes, leaving the user looking at a project they chose to keep
        // sections in while those sections disappear behind them.
        onClose={() => { if (!withdrawInFlight) setShowCancelConfirm(false); }}
        data-testid="cancel-confirm-dialog"
        actions={
          <>
            <Button
              onClick={() => setShowCancelConfirm(false)}
              disabled={withdrawInFlight}
              className={importSecondaryButtonClass}
              data-testid="cancel-confirm-continue"
            >
              {pendingWithdrawCount > 0
                ? t('dialogs.dmpNewImport.cancelContinueEditing')
                : t('common.actions.continue')}
            </Button>
            <Button
              onClick={onCancel}
              disabled={withdrawInFlight}
              className={importSecondaryButtonClass}
              data-testid="confirm-cancel-button"
            >
              {pendingWithdrawCount > 0
                ? t('dialogs.dmpNewImport.cancelKeepAndClose')
                : t('common.actions.cancel')}
            </Button>
            {pendingWithdrawCount > 0 && (
              <Button
                variant="danger"
                disabled={withdrawInFlight}
                onClick={() => { void handleCancelWithRemoval(); }}
                data-testid="cancel-remove-button"
              >
                {t('dialogs.dmpNewImport.cancelRemoveAndClose')}
              </Button>
            )}
          </>
        }
      >
        <p data-testid="cancel-confirm-message">
          {pendingWithdrawCount > 0
            ? t('dialogs.dmpNewImport.cancelWizardMessage', { count: pendingWithdrawCount })
            : t('dialogs.dmpNewImport.cancelWizardNoImports')}
        </p>
      </AlertDialog>

      {/* Back confirmation. No "keep" here: the step Back returns to re-imports
          from scratch on the way forward, so keeping would duplicate. */}
      <AlertDialog
        open={showBackConfirm}
        title={t('dialogs.dmpNewImport.backWizard')}
        onClose={() => { if (!withdrawInFlight) setShowBackConfirm(false); }}
        data-testid="back-confirm-dialog"
        actions={
          <>
            <Button
              onClick={() => setShowBackConfirm(false)}
              disabled={withdrawInFlight}
              className={importSecondaryButtonClass}
              data-testid="back-confirm-continue"
            >
              {t('common.actions.continue')}
            </Button>
            <Button
              variant="danger"
              disabled={withdrawInFlight}
              onClick={() => { void handleBackWithRemoval(); }}
              data-testid="confirm-back-button"
            >
              {pendingWithdrawCount > 0
                ? t('dialogs.dmpNewImport.backRemoveAndGo')
                : t('common.actions.back')}
            </Button>
          </>
        }
      >
        <p data-testid="back-confirm-message">
          {pendingWithdrawCount > 0
            ? t('dialogs.dmpNewImport.backWizardMessage', { count: pendingWithdrawCount })
            : t('dialogs.dmpNewImport.backWizardNoImports')}
        </p>
      </AlertDialog>
    </DialogWrapper>
  );
}
