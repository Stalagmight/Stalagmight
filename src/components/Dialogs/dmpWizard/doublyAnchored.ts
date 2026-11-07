/**
 * Whether a suggestion's target is DOUBLY anchored (split out of
 * DmpNewImportDialog with #576 D4): the existing section's overlap shots sit
 * between two positional constraints, so rewriting them (interpolate, import
 * data) would break one of them.
 */
import type { CrossSectionSuggestion } from '../../../types/commands';
import type { Project } from '../../../types/project';

// Check if the current suggestion targets an existing section that is doubly-anchored
// (modified overlap shots sit between two positional constraints). When true, only
// 'existing_data' and 'keep_both' are valid - interpolation / import_data would break
// a closure or zero-datum constraint.
//
// A merge/closure is only an anchor if the partner can independently reach a zero datum
// without going through this section.  A merge to a free-floating section is NOT a
// positional constraint - the partner can follow via cascading offset adjustments.
export function targetDoublyAnchored(
  currentSuggestion: CrossSectionSuggestion | null,
  project: Project | null,
): boolean {
  if (!currentSuggestion) return false;
  if (currentSuggestion.targetType !== 'existing' || currentSuggestion.existingSectionId == null) return false;
  if (currentSuggestion.matchedShots === 0) return false;

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
}
