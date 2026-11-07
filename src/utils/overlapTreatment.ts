/**
 * Overlap treatment: the single source of truth for what an accepted overlap
 * does to the shots on both sides of it.
 *
 * Extracted from DmpNewImportDialog, which is past the file-size ratchet's
 * budget and cannot host a pure pass this size. Nothing here touches React -
 * it reads the wizard's per-section edit map, mutates it in place, and returns
 * the updates owed to sections that already exist in the project.
 */
import type { CrossSectionSuggestion, DmpFile } from '../types/commands';
import type { OverlapMode } from '../types/config';
import type { EditableShot } from '../components/common/ShotTableEditor';
import { isEmittedShot } from '../components/common/emittedShots';
import { convertToEditableShots } from './shotConversion';
import { useProjectStore } from '../store/projectStore';
import { changedShotFields } from './shotUpdatesDiff';
import type { SectionEditState, ConfirmedOverlapInfo } from './batchImportHelpers';
import {
  computeActionableIds,
  computeOverlapIds,
  computeTargetShotIndex,
  needsDirectionReversal,
  reverseTargetShot,
  applyTargetData,
  interpolateShots,
  type TargetShotLike,
} from './overlapHelpers';

/**
 * [#576 NF-W7F-2] A reading of an EXISTING section as the backend stored it,
 * from the store's copy.
 *
 * The binary envelope carries every shot reading as an f32, so a length stored
 * as 1.2 reaches the store as 1.2000000476837158 - and copying that into a NEW
 * import row ("existing data", or an interpolation with it) created the section
 * with the rounding noise. The shortest decimal that is the same f32 is the
 * stored value for any reading of up to six significant digits, since an f32
 * keeps every such decimal apart. A value that is not an f32 at all did not
 * come through the envelope, and is returned as it is.
 */
function storedReading<T extends number | null>(value: T): T {
  if (value == null || !Number.isFinite(value)) return value;
  const f32 = Math.fround(value);
  if (f32 !== value) return value;
  // Nine significant digits always name an f32 exactly, so the loop returns.
  for (let digits = 1; digits <= 9; digits++) {
    const decimal = Number(value.toPrecision(digits));
    if (Math.fround(decimal) === f32) return decimal as T;
  }
  return value;
}

/** An existing section's shot with its readings as the backend stored them. */
function storedReadings(shot: TargetShotLike): TargetShotLike {
  return {
    azimuth_in: storedReading(shot.azimuth_in),
    azimuth_out: storedReading(shot.azimuth_out),
    length: storedReading(shot.length),
    depth_in: storedReading(shot.depth_in),
    depth_out: storedReading(shot.depth_out),
    pitch_in: storedReading(shot.pitch_in),
    pitch_out: storedReading(shot.pitch_out),
    left: storedReading(shot.left),
    right: storedReading(shot.right),
    up: storedReading(shot.up),
    down: storedReading(shot.down),
  };
}

/**
 * Rescue a section that would lose every shot to deletion (overlaps at both
 * ends, with this section the "import" side of both) by converting those
 * deletions to a virtual kind. The section stays on the map as reference
 * copies while the adjacent targets keep the overlap with treated values, so
 * nothing is duplicated un-marked and nothing silently disappears.
 *
 * Counted over the rows that will be EMITTED rather than merely the undeleted
 * ones: a row added and left wholly blank is alive on screen and absent from
 * the payload, so counting it here let a fully-deleted section walk past the
 * rescue, emit nothing, and vanish without a word.
 *
 * Mutates `sectionEdits` in place; returns the indices it rescued.
 */
export function rescueSectionsWithNoEmittedShots(
  sectionEdits: Map<number, SectionEditState>,
): number[] {
  const rescued: number[] = [];
  for (const [sectionIdx, edits] of sectionEdits.entries()) {
    const emittedCount = edits.shots.filter(isEmittedShot).length;
    const deletedCount = edits.shots.filter(s => s.isDeleted).length;
    if (emittedCount > 0 || deletedCount === 0) continue;
    const newShots = edits.shots.map(s =>
      s.isDeleted ? { ...s, isDeleted: false, kind: 'Reference' as const } : s,
    );
    sectionEdits.set(sectionIdx, { ...edits, shots: newShots });
    rescued.push(sectionIdx);
    if (import.meta.env.DEV) {
      console.warn(
        `[applyAllOverlaps] Section ${sectionIdx} would have 0 emitted shots - converted ${deletedCount} deletions to reference`,
      );
    }
  }
  return rescued;
}

/** Check if a suggestion matches a confirmed overlap info by field comparison. */
function matchesConfirmed(s: CrossSectionSuggestion, info: ConfirmedOverlapInfo): boolean {
  return s.importStationIndex === info.importStationIndex
    && s.matchedShots === info.matchedShots
    && s.direction === info.direction
    && s.existingStationIndex === info.existingStationIndex
    && s.targetType === info.targetType
    && s.existingSectionId === info.existingSectionId
    && s.targetImportIndex === info.targetImportIndex;
}

/**
 * Apply overlap treatments for ALL known overlaps in a single pass.
 *
 * This is the single source of truth for overlap mutation. It runs once before
 * `buildBatchImport`. For confirmed suggestions, it uses the user-selected
 * merge station to determine actionable IDs. For unconfirmed suggestions,
 * it treats ALL overlap IDs.
 *
 * Mutates `sectionEdits` in-place.
 */
export interface ExistingShotUpdateLocal {
  importSectionIndex: number;
  section_id: number;
  shot_id: number;
  updates: Record<string, number | null>;
}

export function backendExistingShotUpdatesForSections(
  updates: ExistingShotUpdateLocal[],
  sectionIndices: Set<number>,
): Array<Omit<ExistingShotUpdateLocal, 'importSectionIndex'>> {
  return updates
    .filter((update) => sectionIndices.has(update.importSectionIndex))
    .map(({ importSectionIndex: _importSectionIndex, ...update }) => update);
}

export function removeExistingShotUpdatesForSections(
  updates: ExistingShotUpdateLocal[],
  sectionIndices: Set<number>,
): ExistingShotUpdateLocal[] {
  return updates.filter((update) => !sectionIndices.has(update.importSectionIndex));
}

/**
 * Resolve the array position of a target-import overlap row by its ORIGINAL
 * identity, not by raw array position.
 *
 * `computeTargetShotIndex` answers in the target section's ORIGINAL DMP
 * ordering, and `convertToEditableShots` stamps each original shot's `id` with
 * exactly that index - so the value it returns IS the id of the row to treat.
 * Rows the user added in the wizard carry their own temp ids and shift every
 * position after them, so `shots[dmpIndex]` lands on an inserted row whenever
 * one sits before the overlap, treating the wrong row and leaving the real
 * overlap row untouched. Matching on the non-new row whose id equals the DMP
 * index instead follows the shot through inserts AND deletions (the same
 * identity-key rule the edit dialog's frozen-index resolution relies on). On an
 * unedited section (id === position, nothing new) it returns the same index the
 * old positional lookup did. Returns -1 when no such row exists.
 */
function resolveTargetRowIndex(shots: readonly EditableShot[], dmpIndex: number): number {
  if (dmpIndex < 0) return -1;
  return shots.findIndex((shot) => shot.isNew !== true && shot.id === dmpIndex);
}

export function applyAllOverlapTreatments(
  fullPool: CrossSectionSuggestion[],
  confirmedOverlaps: Map<number, ConfirmedOverlapInfo>,
  sectionEdits: Map<number, SectionEditState>,
  dmpFile: DmpFile,
  overlapMode: OverlapMode,
): ExistingShotUpdateLocal[] {
  const existingShotUpdates: ExistingShotUpdateLocal[] = [];
  if (import.meta.env.DEV) {
    console.warn(
      `[applyAllOverlaps] START: pool=${fullPool.length} suggestions, confirmed=${confirmedOverlaps.size}, mode=${overlapMode}`,
      `edits keys=[${[...sectionEdits.keys()].join(',')}]`,
    );
  }
  // Track shots already treated by a previous suggestion to prevent double-treatment.
  // Key: "sectionIndex:shotId".  This replaced the old is_reference guard which was
  // removed so that the shot kind only has user-facing semantics (keep_both mode).
  const treatedShots = new Set<string>();

  for (const suggestion of fullPool) {
    const {
      importSectionIndex,
      importStationIndex,
      matchedShots,
      existingMatchedShots,
      existingStationIndex,
      direction,
      targetType,
      existingSectionId,
      targetImportIndex,
    } = suggestion;

    const allOverlapIds = computeOverlapIds(importStationIndex, matchedShots);
    if (allOverlapIds.size === 0) continue;

    const confirmed = confirmedOverlaps.get(importSectionIndex);
    const isConfirmed = !!(confirmed && matchesConfirmed(suggestion, confirmed));

    // Once a section has a confirmed suggestion, ignore all alternatives for that
    // same import section. This ensures overlap treatment follows exactly the path
    // the user accepted in the wizard.
    if (confirmed && !isConfirmed) {
      if (import.meta.env.DEV) {
        console.warn(
          `[applyAllOverlaps] skipping alternative for confirmed section ${importSectionIndex}`,
        );
      }
      continue;
    }

    // If the opposite direction was confirmed (A->B), ignore unconfirmed reverse
    // suggestions (B->A). Otherwise reverse candidates can pre-treat shots and
    // prevent the confirmed direction from applying its intended deletion boundary.
    if (!isConfirmed && targetType === 'importing' && targetImportIndex != null) {
      const reverseConfirmed = confirmedOverlaps.get(targetImportIndex);
      if (
        reverseConfirmed &&
        reverseConfirmed.targetType === 'importing' &&
        reverseConfirmed.targetImportIndex === importSectionIndex
      ) {
        if (import.meta.env.DEV) {
          console.warn(
            `[applyAllOverlaps] skipping reverse unconfirmed candidate ${importSectionIndex}->${targetImportIndex}`,
          );
        }
        continue;
      }
    }

    // Determine actionable IDs: use merge station from confirmed info, or full overlap range
    let actionableIds: Set<number>;
    if (isConfirmed) {
      actionableIds = computeActionableIds(importStationIndex, matchedShots, confirmed!.mergeImportStation);
    } else {
      actionableIds = allOverlapIds;
    }

    if (import.meta.env.DEV) {
      console.warn(
        `[applyAllOverlaps] processing suggestion: import=${importSectionIndex} impStn=${importStationIndex}`,
        `matched=${matchedShots} dir=${direction} target=${targetType === 'importing' ? targetImportIndex : existingSectionId}`,
        `targetType=${targetType} mode=${overlapMode}`,
        `confirmed=${!!confirmed && matchesConfirmed(suggestion, confirmed)}`,
        `actionableIds=[${[...actionableIds].join(',')}]`,
      );
    }

    // --- Import side: treat untreated overlap shots on the import section ---
    let importEdits = sectionEdits.get(importSectionIndex);
    if (!importEdits) {
      const section = dmpFile.sections[importSectionIndex];
      if (section) {
        importEdits = {
          name: section.name,
          surveyor: '',
          date: section.date?.split(/[T ]/)[0] ?? '',
          color: '',
          direction: section.direction === 'out' ? 'out' : 'in',
          quality: 1.0,
          reverse: false,
          shots: convertToEditableShots(section.shots, section.version),
        };
        sectionEdits.set(importSectionIndex, importEdits);
      }
    }
    // Save raw import shots BEFORE import-side treatment modifies them.
    // Target-side treatment needs original values to produce symmetric interpolation.
    const rawImportShots = importEdits?.shots;

    if (importEdits) {
      const hasUntreated = importEdits.shots.some(
        shot => !shot.isNew && !shot.isDeleted && actionableIds.has(shot.id)
          && !treatedShots.has(`${importSectionIndex}:${shot.id}`)
      );

      if (hasUntreated) {
        if (overlapMode === 'keep_both') {
          // keep_both: mark import overlap shots as reference (no deletion, no value change)
          const newShots = importEdits.shots.map(shot => {
            if (shot.isNew || shot.isDeleted) return shot;
            if (!actionableIds.has(shot.id)) return shot;
            if (treatedShots.has(`${importSectionIndex}:${shot.id}`)) return shot;
            treatedShots.add(`${importSectionIndex}:${shot.id}`);
            return { ...shot, kind: 'Reference' as const };
          });
          if (import.meta.env.DEV) {
            const marked = newShots.filter(s => s.kind !== 'Normal').length - importEdits.shots.filter(s => s.kind !== 'Normal').length;
            console.warn(`[applyAllOverlaps]   import ${importSectionIndex}: keep_both marked ${marked} shots as reference`);
          }
          sectionEdits.set(importSectionIndex, { ...importEdits, shots: newShots });
        } else {
          // Build target map for interpolate/existing_data modes
          let targetMap: Map<number, TargetShotLike> | null = null;
          let targetMapDiag = '';
          if (overlapMode !== 'import_data') {
            let targetShotArray: TargetShotLike[] | null = null;
            if (targetType === 'existing' && existingSectionId != null) {
              const currentProject = useProjectStore.getState().project;
              const targetSection = currentProject?.sections.find(s => s.id === existingSectionId);
              if (targetSection) {
                // The values the backend stored, not the store's f32 copy: these
                // are written into the new import rows (#576 NF-W7F-2).
                targetShotArray = targetSection.shots.map(storedReadings);
                targetMapDiag = `existing section ${existingSectionId} (${targetSection.shots.length} shots)`;
              } else {
                targetMapDiag = `MISS: existing section ${existingSectionId} not found in project`;
              }
            } else if (targetType === 'importing' && targetImportIndex != null) {
              const targetEdits = sectionEdits.get(targetImportIndex);
              if (targetEdits) {
                // Station space: the target section reaches the backend without
                // its blank rows, so an index into this array must be counted
                // over the same set. (Audit of the emitted-vs-visible split.)
                targetShotArray = targetEdits.shots.filter(isEmittedShot);
                targetMapDiag = `importing section ${targetImportIndex} from edits (${targetEdits.shots.length} total, ${targetShotArray.length} alive)`;
              } else {
                const targetDmpSection = dmpFile.sections[targetImportIndex];
                if (targetDmpSection) {
                  targetShotArray = convertToEditableShots(targetDmpSection.shots, targetDmpSection.version);
                  targetMapDiag = `importing section ${targetImportIndex} from DMP (${targetShotArray.length} shots)`;
                } else {
                  targetMapDiag = `MISS: importing section ${targetImportIndex} not in edits or DMP`;
                }
              }
            } else {
              targetMapDiag = `MISS: targetType=${targetType} existingSectionId=${existingSectionId} targetImportIndex=${targetImportIndex}`;
            }

            if (targetShotArray) {
              const shouldReverse = needsDirectionReversal(importStationIndex, direction);
              const overlapParams = { importStationIndex, matchedShots, existingMatchedShots, existingStationIndex, direction };
              targetMap = new Map<number, TargetShotLike>();
              for (const overlapId of actionableIds) {
                const targetIdx = computeTargetShotIndex(overlapId, overlapParams);
                if (targetIdx >= 0 && targetIdx < targetShotArray.length) {
                  const raw = targetShotArray[targetIdx]!;
                  targetMap.set(overlapId, shouldReverse ? reverseTargetShot(raw) : raw);
                } else if (import.meta.env.DEV) {
                  console.warn(
                    `[applyAllOverlaps]   targetIdx OUT OF RANGE: overlapId=${overlapId} → targetIdx=${targetIdx} (array len=${targetShotArray.length})`,
                  );
                }
              }
            }
          } else {
            targetMapDiag = 'SKIPPED (import_data mode)';
          }

          if (import.meta.env.DEV) {
            console.warn(
              `[applyAllOverlaps]   import ${importSectionIndex}: targetMap=${targetMap ? targetMap.size : 'null'}`,
              `source: ${targetMapDiag}`,
            );
          }

          let diagInterpolated = 0, diagRefOnly = 0, diagAlreadyTreated = 0, diagDeleted = 0;
          const newShots = importEdits.shots.map(shot => {
            if (shot.isNew || shot.isDeleted) return shot;
            if (!actionableIds.has(shot.id)) return shot;
            const treatKey = `${importSectionIndex}:${shot.id}`;
            if (treatedShots.has(treatKey)) { diagAlreadyTreated++; return shot; }

            // Import-to-import: always delete - target section keeps the overlap (treated below).
            // Confirmed import-to-existing: delete - target section gets interpolated values.
            if (targetType === 'importing' || isConfirmed) {
              diagDeleted++;
              treatedShots.add(treatKey);
              return { ...shot, isDeleted: true };
            }

            const target = targetMap?.get(shot.id);
            treatedShots.add(treatKey);
            if (overlapMode === 'interpolate' && target) {
              diagInterpolated++;
              return interpolateShots(shot, target);
            } else if (overlapMode === 'existing_data' && target) {
              diagInterpolated++;
              return applyTargetData(shot, target);
            } else {
              diagRefOnly++;
              return shot;
            }
          });

          if (import.meta.env.DEV) {
            console.warn(
              `[applyAllOverlaps]   import ${importSectionIndex} RESULT:`,
              `interpolated=${diagInterpolated} deleted=${diagDeleted} refOnly=${diagRefOnly} alreadyTreated=${diagAlreadyTreated}`,
            );
            if (diagRefOnly > 0 && overlapMode !== 'import_data') {
              console.warn(
                `[applyAllOverlaps]   ⚠ ${diagRefOnly} shots WITHOUT target data (kept original values)`,
                `targetMap=${targetMap ? `size=${targetMap.size} keys=[${[...targetMap.keys()].join(',')}]` : 'null'}`,
                `actionableIds=[${[...actionableIds].join(',')}]`,
              );
            }
          }

          sectionEdits.set(importSectionIndex, { ...importEdits, shots: newShots });
          if (import.meta.env.DEV) {
            const alive = newShots.filter(s => !s.isDeleted).length;
            const deleted = newShots.filter(s => s.isDeleted).length;
            const refs = newShots.filter(s => s.kind !== 'Normal' && !s.isDeleted).length;
            console.warn(
              `[applyAllOverlaps]   import ${importSectionIndex} AFTER: ${newShots.length} total, ${alive} alive, ${deleted} deleted, ${refs} reference`,
            );
          }
        }
      }
    }

    // --- Target side (import-to-existing): collect shot updates for existing project sections ---
    // Confirmed import-to-existing: compute interpolated values and collect as ExistingShotUpdate.
    // These are sent to the backend as part of the atomic import transaction.
    if (overlapMode !== 'keep_both' && isConfirmed && targetType === 'existing' && existingSectionId != null) {
      const currentProject = useProjectStore.getState().project;
      const targetSection = currentProject?.sections.find(s => s.id === existingSectionId);
      if (targetSection) {
        const overlapParams = { importStationIndex, matchedShots, existingMatchedShots, existingStationIndex, direction };
        const shouldReverse = needsDirectionReversal(importStationIndex, direction);
        const importShotsForInterp = rawImportShots ?? [];

        for (const overlapId of actionableIds) {
          const targetShotIdx = computeTargetShotIndex(overlapId, overlapParams);
          if (targetShotIdx < 0 || targetShotIdx >= targetSection.shots.length) continue;

          const existingShot = targetSection.shots[targetShotIdx];
          if (!existingShot) continue;
          const treatKey = `existing:${existingSectionId}:${existingShot.id}`;
          if (treatedShots.has(treatKey)) continue;

          const importShot = importShotsForInterp.find(
            s => !s.isNew && !s.isDeleted && s.id === overlapId
          );
          if (!importShot) continue;

          const importAsTarget: TargetShotLike = shouldReverse
            ? reverseTargetShot(importShot)
            : importShot;

          let updates: Record<string, number | null> | null = null;
          if (overlapMode === 'interpolate') {
            const interp = interpolateShots({ ...existingShot, id: existingShot.id, comment: existingShot.comment ?? null, kind: existingShot.kind, isNew: false, isDeleted: false }, importAsTarget);
            updates = {
              azimuth_in: interp.azimuth_in, azimuth_out: interp.azimuth_out,
              length: interp.length, depth_in: interp.depth_in, depth_out: interp.depth_out,
              pitch_in: interp.pitch_in, pitch_out: interp.pitch_out,
              left: interp.left, right: interp.right, up: interp.up, down: interp.down,
            };
          } else if (overlapMode === 'import_data') {
            updates = {
              azimuth_in: importAsTarget.azimuth_in, azimuth_out: importAsTarget.azimuth_out,
              length: importAsTarget.length, depth_in: importAsTarget.depth_in, depth_out: importAsTarget.depth_out,
              pitch_in: importAsTarget.pitch_in, pitch_out: importAsTarget.pitch_out,
              left: importAsTarget.left, right: importAsTarget.right, up: importAsTarget.up, down: importAsTarget.down,
            };
          }
          // existing_data: target keeps its own values - no update needed

          // [#576 F32-EDIT] Only the readings that change: the project's shots hold
          // their values rounded through an f32 (the binary envelope), so an
          // unchanged one re-sent from here would write that rounding over the
          // backend's exact value.
          if (updates) {
            updates = changedShotFields(existingShot, updates) as Record<string, number | null>;
            if (Object.keys(updates).length === 0) {
              treatedShots.add(treatKey);
              updates = null;
            }
          }

          if (updates) {
            if (import.meta.env.DEV) {
              console.warn(
                `[applyAllOverlaps]   i2e update: section=${existingSectionId} shotId=${existingShot.id} shotIdx=${targetShotIdx}`,
                `existing len=${existingShot.length.toFixed(3)} → update len=${(updates.length as number)?.toFixed(3)}`,
                `existing az=${existingShot.azimuth_in.toFixed(1)} → update az=${(updates.azimuth_in as number)?.toFixed(1)}`,
              );
            }
            existingShotUpdates.push({
              importSectionIndex,
              section_id: existingSectionId,
              shot_id: existingShot.id,
              updates,
            });
            treatedShots.add(treatKey);
          }
        }

        if (import.meta.env.DEV) {
          console.warn(
            `[applyAllOverlaps]   existing target ${existingSectionId}: ${existingShotUpdates.length} shot updates collected`,
          );
        }
      }
    }

    // --- Target side (import-to-import): treat untreated overlap shots on the target section ---
    // keep_both: no target modification - both sections retain their original values.
    if (overlapMode !== 'keep_both' && targetType === 'importing' && targetImportIndex != null) {
      let targetEdits = sectionEdits.get(targetImportIndex);
      if (!targetEdits) {
        const section = dmpFile.sections[targetImportIndex];
        if (section) {
          targetEdits = {
            name: section.name,
            surveyor: '',
            date: section.date?.split(/[T ]/)[0] ?? '',
            color: '',
            direction: section.direction === 'out' ? 'out' : 'in',
            quality: 1.0,
            reverse: false,
            shots: convertToEditableShots(section.shots, section.version),
          };
          sectionEdits.set(targetImportIndex, targetEdits);
        }
      }
      if (targetEdits) {
        const overlapParams = { importStationIndex, matchedShots, existingMatchedShots, existingStationIndex, direction };
        const shouldReverse = needsDirectionReversal(importStationIndex, direction);

        let hasUntreatedTarget = false;
        for (const overlapId of actionableIds) {
          const targetShotIdx = resolveTargetRowIndex(
            targetEdits.shots,
            computeTargetShotIndex(overlapId, overlapParams),
          );
          if (targetShotIdx >= 0) {
            const shot = targetEdits.shots[targetShotIdx];
            if (shot && !shot.isDeleted && !treatedShots.has(`${targetImportIndex}:${shot.id}`)) {
              hasUntreatedTarget = true;
              break;
            }
          }
        }

        if (hasUntreatedTarget) {
          // Use raw (pre-interpolation) import shots so both sides get symmetric results.
          // If we read from sectionEdits here, import-side treatment has already averaged
          // the values, and averaging again biases toward the target: avg(T, avg(I,T)) = (3T+I)/4.
          const importShotsForInterp = rawImportShots ?? [];
          const targetShots = targetEdits.shots.map(s => ({ ...s }));
          let treated = 0;
          let skippedOutOfRange = 0, skippedAlreadyTreated = 0, skippedNoImport = 0;

          for (const overlapId of actionableIds) {
            const targetShotIdx = resolveTargetRowIndex(
              targetShots,
              computeTargetShotIndex(overlapId, overlapParams),
            );
            if (targetShotIdx < 0) { skippedOutOfRange++; continue; }

            const targetShot = targetShots[targetShotIdx];
            if (!targetShot || targetShot.isDeleted) { skippedAlreadyTreated++; continue; }
            const targetTreatKey = `${targetImportIndex}:${targetShot.id}`;
            if (treatedShots.has(targetTreatKey)) { skippedAlreadyTreated++; continue; }

            const importShot = importShotsForInterp.find(
              s => !s.isNew && !s.isDeleted && s.id === overlapId
            );
            if (!importShot) { skippedNoImport++; continue; }

            const importAsTarget: TargetShotLike = shouldReverse
              ? reverseTargetShot(importShot)
              : importShot;

            if (overlapMode === 'interpolate') {
              const interpResult = interpolateShots(targetShot, importAsTarget);
              if (import.meta.env.DEV) {
                console.warn(
                  `[applyAllOverlaps]   i2i target shot ${targetShotIdx} (id=${targetShot.id}):`,
                  `len ${targetShot.length.toFixed(3)}→${interpResult.length.toFixed(3)}`,
                  `az_in ${targetShot.azimuth_in.toFixed(1)}→${interpResult.azimuth_in.toFixed(1)}`,
                  `(import: len=${importAsTarget.length.toFixed(3)} az=${importAsTarget.azimuth_in.toFixed(1)})`,
                );
              }
              targetShots[targetShotIdx] = interpResult;
              treatedShots.add(targetTreatKey);
              treated++;
            } else if (overlapMode === 'existing_data') {
              // existing_data: target keeps its own values - nothing to change
              treatedShots.add(targetTreatKey);
            } else {
              targetShots[targetShotIdx] = applyTargetData(targetShot, importAsTarget);
              treatedShots.add(targetTreatKey);
              treated++;
            }
          }

          if (import.meta.env.DEV) {
            console.warn(
              `[applyAllOverlaps]   target ${targetImportIndex} RESULT: treated=${treated}`,
              `skippedOutOfRange=${skippedOutOfRange} skippedAlreadyTreated=${skippedAlreadyTreated} skippedNoImport=${skippedNoImport}`,
            );
          }

          if (treated > 0) {
            sectionEdits.set(targetImportIndex, { ...targetEdits, shots: targetShots });
          }
        }
      }
    }
  }

  rescueSectionsWithNoEmittedShots(sectionEdits);

  return existingShotUpdates;
}
