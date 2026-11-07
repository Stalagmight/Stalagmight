/**
 * The DMP wizard's batch builder: which selected sections an import sends, in
 * what order, and placed how (split out of DmpNewImportDialog with #576 D4).
 * Pure - the wizard's early, late and per-segment imports all build their
 * payload here, from the run's connections, edits and placements.
 */
import type { CrossSectionSuggestion, DmpFile, ImportPlacement } from '../../../types/commands';
import type { ImportSectionConfig as BackendImportSectionConfig } from '../../../types/commands';
import type { ShotData } from '../../../types/project';
import type { DmpSelectionConfig } from '../DmpSelectionDialog';
import { DMP_AZIMUTH_REFERENCE, finalizePlacementForReverse } from '../../../utils/importSectionConfig';
import { convertToEditableShots, editableShotToShotData } from '../../../utils/shotConversion';
import { type VirtualConnection, buildAdjacency } from '../../../utils/segmentContext';
import { sectionColorPalette } from '../../../utils/colors';
import { debugLog } from '../../../utils/debugLog';
import {
  adjustStationForDeletedShots,
  findReverseImportConnection,
  resolveImportSectionDate,
  type SectionEditState,
} from '../../../utils/batchImportHelpers';
import { isEmittedShot } from '../../common/emittedShots';
import { assertSendableShots } from '../../../utils/importPayloadGuard';
import type { DirectPlacement } from './wizardTypes';

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
 * The wizard is its sole production caller; DmpNewImportDialog re-exports it
 * for the tests that import it from there.
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
  discarded?: ReadonlySet<number>,
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
  // What reachability may not pass through: the deferred groups, and the sections the
  // user DISCARDED in the wizard (#576 DMP-DISCARD, decided: "A discarded section is
  // never imported by a later placement in the same run"). A discard used to be only a
  // filter on the late import's re-queue, so once a later placement connected to a
  // discarded section - placing the section whose one link led to it - the rebuilt
  // batch reached it and imported it. A discarded section is never an anchor, never
  // reached, and so never emitted; it stays in `unreachable`, where the late import
  // drops it again rather than re-queueing it.
  const blocked = discarded && discarded.size > 0
    ? new Set<number>([...deferredImportSet, ...discarded])
    : deferredImportSet;

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
    if (c.targetType === 'existing' && !blocked.has(c.importSectionIndex)) {
      anchored.add(c.importSectionIndex);
    }
  }
  // Direct placements also anchor their sections
  for (const dp of directPlacements ?? []) {
    if (!blocked.has(dp.anchorSectionIndex)) {
      anchored.add(dp.anchorSectionIndex);
    }
  }

  // BFS from anchored sections
  let reachableOrder = bfsReachable(anchored, adjacency, blocked);
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
            if (!blocked.has(edge.neighbor) && !visited.has(edge.neighbor) && !reachableSet.has(edge.neighbor)) {
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
            !blocked.has(s.importSectionIndex)
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
        reachableOrder = bfsReachable(anchored, newAdjacency, blocked);
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

      // Use edited shots if available, otherwise fall back to DMP data
      let backendShots: ShotData[];
      if (edits) {
        const deletedCount = edits.shots.filter(s => s.isDeleted).length;
        backendShots = edits.shots
          // `!isBlankNewShot` as well: the completeness gate deliberately lets a
          // row added and left wholly blank through (it is an invitation, not an
          // obligation), which is only safe if every payload path drops it. The
          // zero-datum and segment paths do; this one did not, so a row typed in
          // and then cleared arrived as NaN length, depths and azimuths.
          .filter(isEmittedShot)
          .map(editableShotToShotData);
        if (deletedCount > 0) {
          debugLog(
            `[buildBatchImport] Section ${idx} "${section.name}": ${edits.shots.length} total shots, ${deletedCount} deleted, ${backendShots.length} emitted`,
          );
        }
        // Compare edited shots to original DMP data to detect interpolation
        if (import.meta.env.DEV) {
          const origShots = convertToEditableShots(section.shots, section.version);
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
        const editableShots = convertToEditableShots(section.shots, section.version);
        backendShots = editableShots.map(editableShotToShotData);
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
