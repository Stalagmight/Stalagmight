/**
 * Applying one orphan-placement suggestion (#681): join the orphan at the merge
 * station, then resolve the shots it repeats, all through the undoable store
 * actions. Those actions do not check the read-only mode themselves, so the
 * caller must (the dialog does, with isEditingBlockedNow). Kept out of
 * OrphanPlacementDialog so the dialog holds the UI and this holds what Accept
 * and Accept All do to the project.
 */

import type { CommandResult, OrphanPlacementSuggestion } from '../types/commands';
import type { Project } from '../types/ipc/Project';
import type { ProjectStore } from '../store/projectStore';
import { computeMergeOptions, type MergeOption, type OverlapParams } from './overlapHelpers';
import { repeatedShots, resolvedReading, type OverlapChoice } from './overlapChoices';

/** The overlap geometry of a suggestion, as the overlap helpers take it. */
export function overlapParamsOf(s: OrphanPlacementSuggestion): OverlapParams {
  return {
    importStationIndex: s.orphanStationIndex,
    matchedShots: s.matchedShots,
    existingMatchedShots: s.existingMatchedShots,
    existingStationIndex: s.targetStationIndex,
    direction: s.direction,
  };
}

/** The merge stations a suggestion offers; the last is the end of the repeat. */
export function mergeOptionsOf(s: OrphanPlacementSuggestion, orphanShotCount: number): MergeOption[] {
  return computeMergeOptions(
    {
      importStationIndex: s.orphanStationIndex,
      existingStationIndex: s.targetStationIndex,
      matchedShots: s.matchedShots,
      existingMatchedShots: s.existingMatchedShots,
      preferredMergeOffset: s.preferredMergeOffset,
      direction: s.direction,
      isPartial: s.isPartial,
    },
    orphanShotCount,
  );
}

/**
 * Whether the existing survey the repeat overlays is tied down at both ends: a
 * placement zero datum (a GPS fix pins nothing in raw geometry, #741, as in the
 * backend's import check), or a merge / aligned closure leading to one without passing back
 * through the target, at or before the repeat AND after it. Rewriting its
 * readings would then bend survey that is fixed at both ends, so the dialog
 * allows Keep existing only.
 */
export function isRepeatDoublyAnchored(suggestion: OrphanPlacementSuggestion, project: Project): boolean {
  if (suggestion.matchedShots === 0) return false;

  const sid = suggestion.targetLineId;

  // BFS: can `start` reach a placement zero datum via merges + aligned
  // closures without traversing through `excluded`?
  const hasDatumPathExcluding = (start: number, excluded: number): boolean => {
    const visited = new Set<number>();
    const queue = [start];
    while (queue.length > 0) {
      const current = queue.pop()!;
      if (current === excluded) continue;
      if (visited.has(current)) continue;
      visited.add(current);
      if (project.zero_datums.some((zd) => zd.role === 'Placement' && zd.station_ref.line_id === current)) {
        return true;
      }
      for (const m of project.station_merges) {
        if (m.station_a.line_id === current && !visited.has(m.station_b.line_id)) {
          queue.push(m.station_b.line_id);
        }
        if (m.station_b.line_id === current && !visited.has(m.station_a.line_id)) {
          queue.push(m.station_a.line_id);
        }
      }
      for (const c of project.loop_closures) {
        if (!c.is_aligned) continue;
        if (c.station_a.line_id === current && !visited.has(c.station_b.line_id)) {
          queue.push(c.station_b.line_id);
        }
        if (c.station_b.line_id === current && !visited.has(c.station_a.line_id)) {
          queue.push(c.station_a.line_id);
        }
      }
    }
    return false;
  };

  // Compute the range of target shot indices that would be modified
  let minShot: number;
  let maxShot: number;
  if (suggestion.direction === 'forward') {
    minShot = suggestion.targetStationIndex;
    maxShot = suggestion.targetStationIndex + suggestion.matchedShots - 1;
  } else {
    minShot = suggestion.targetStationIndex - suggestion.matchedShots;
    maxShot = suggestion.targetStationIndex - 1;
  }

  // Collect anchor station indices on the target line
  const anchors: number[] = [];
  for (const zd of project.zero_datums) {
    if (zd.role === 'Placement' && zd.station_ref.line_id === sid) anchors.push(zd.station_ref.station_index);
  }
  for (const m of project.station_merges) {
    if (m.station_a.line_id === sid && hasDatumPathExcluding(m.station_b.line_id, sid)) {
      anchors.push(m.station_a.station_index);
    }
    if (m.station_b.line_id === sid && hasDatumPathExcluding(m.station_a.line_id, sid)) {
      anchors.push(m.station_b.station_index);
    }
  }
  for (const c of project.loop_closures) {
    if (!c.is_aligned) continue;
    if (c.station_a.line_id === sid && hasDatumPathExcluding(c.station_b.line_id, sid)) {
      anchors.push(c.station_a.station_index);
    }
    if (c.station_b.line_id === sid && hasDatumPathExcluding(c.station_a.line_id, sid)) {
      anchors.push(c.station_b.station_index);
    }
  }

  const hasFront = anchors.some((idx) => idx <= minShot);
  const hasBack = anchors.some((idx) => idx > maxShot);
  return hasFront && hasBack;
}

/** The store actions a placement runs, and the project as it stands now. */
export interface PlacementActions {
  mergeStations: ProjectStore['mergeStations'];
  setShotKind: ProjectStore['setShotKind'];
  updateShot: ProjectStore['updateShot'];
  project: () => Project | null;
}

/**
 * Join `suggestion` at `mergePair` and resolve its repeat: the repeated shots up
 * to the merge station, the ones the dialog's table lists. Each new copy becomes
 * a Reference shot - kept, never deleted. For Use new and Average the EXISTING
 * shot takes the resolved reading, so its stations, connections, closures and
 * comments stay where they are.
 *
 * Returns the merge's result. A refused merge writes nothing else: no shot is
 * marked or rewritten, and the caller must not count the orphan as placed.
 */
export async function placeSuggestion(
  suggestion: OrphanPlacementSuggestion,
  mergePair: MergeOption | null,
  choices: ReadonlyMap<number, OverlapChoice>,
  actions: PlacementActions,
): Promise<CommandResult> {
  const mergeImport = mergePair?.importStation ?? suggestion.orphanStationIndex;
  const mergeExisting = mergePair?.existingStation ?? suggestion.targetStationIndex;
  const merged = await actions.mergeStations(
    { line_id: suggestion.orphanLineId, station_index: mergeImport },
    { line_id: suggestion.targetLineId, station_index: mergeExisting },
  );
  if (!merged.success || suggestion.matchedShots === 0) return merged;

  // Fresh lines: the merge above refreshed the project.
  const proj = actions.project();
  const oLine = proj?.lines.find((s) => s.id === suggestion.orphanLineId);
  const tLine = proj?.lines.find((s) => s.id === suggestion.targetLineId);
  if (!oLine || !tLine) return merged;

  const params = overlapParamsOf(suggestion);
  for (const repeat of repeatedShots(params, mergeImport, oLine.shots, tLine.shots)) {
    await actions.setShotKind(suggestion.orphanLineId, repeat.orphanShot.id, 'Reference');
    const choice = choices.get(repeat.orphanShotIndex) ?? 'keep_existing';
    const reading = resolvedReading(choice, repeat.targetShot, repeat.incoming);
    if (reading) {
      await actions.updateShot(suggestion.targetLineId, repeat.targetShot.id, reading);
    }
  }
  return merged;
}
