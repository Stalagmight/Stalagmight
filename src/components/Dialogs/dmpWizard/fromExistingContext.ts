/**
 * The DMP wizard's "From Existing" context (#576 D15): the existing project
 * segment the user is attaching a DMP segment to, drawn dimmed behind the
 * segment preview, aligned so the chosen existing station sits on the chosen
 * import anchor station.
 *
 * A pure function of the project's sections and merges and the wizard's
 * choices, moved out of DmpNewImportDialog.tsx verbatim except for the walk:
 * every existing section is now walked the way the backend walks it - the
 * shared `walkStations`, with its own north reference, else the project default
 * (`azimuthReferenceFor`). It used to walk them with no reference, so a section
 * recorded with a declination drew rotated by it, and two merged sections with
 * different references were drawn apart at their merge.
 */
import type { AzimuthReference } from '../../../types/ipc/AzimuthReference';
import type { Section, StationMerge } from '../../../types/project';
import { azimuthReferenceFor } from '../../../utils/stationWalk';
import {
  computeStationsFromDmpShots,
  computeStationsFromShots,
  type ContextSectionGeometry,
  type SegmentSectionEntry,
} from '../SectionPreview';

/** Station positions of an existing project section, station 0 at the origin,
 *  walked with the north reference the backend walks it with. */
export function existingSectionStations(
  section: Pick<Section, 'shots' | 'azimuth_reference'>,
  projectDefault: AzimuthReference | null | undefined,
): Array<{ x: number; y: number; z: number }> {
  return computeStationsFromShots(section.shots, azimuthReferenceFor(section.azimuth_reference, projectDefault));
}

export interface FromExistingContextInput {
  fromExistingSectionId: number;
  fromExistingStationIdx: number;
  projectSections: Section[];
  projectMerges: StationMerge[];
  projectDefault: AzimuthReference | null | undefined;
  anchorSectionIndex: number;
  /** The anchor station in the emitted (backend) numbering. */
  emittedAnchorStation: number;
  segmentPreviewSections: SegmentSectionEntry[];
}

/** The existing segment around `fromExistingSectionId`, in the segment preview's frame. */
export function buildFromExistingContext({
  fromExistingSectionId,
  fromExistingStationIdx,
  projectSections,
  projectMerges,
  projectDefault,
  anchorSectionIndex,
  emittedAnchorStation,
  segmentPreviewSections,
}: FromExistingContextInput): ContextSectionGeometry[] | null {
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

    const stnPositions = existingSectionStations(section, projectDefault);
    entries.push({ sectionId, stations: stnPositions, offsetX, offsetY });

    for (const edge of mergeAdj.get(sectionId) ?? []) {
      if (visited.has(edge.neighborId)) continue;
      visited.add(edge.neighborId);

      const neighborSection = projectSections.find((s) => s.id === edge.neighborId);
      if (!neighborSection) continue;

      const neighborStns = existingSectionStations(neighborSection, projectDefault);
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

  const anchorEntry = segmentPreviewSections.find((e) => e.sectionIndex === anchorSectionIndex);
  if (!anchorEntry) return null;
  const anchorStns = computeStationsFromDmpShots(
    anchorEntry.section.shots.filter((s) => s.shot_type === 'Standard'),
  );
  // `anchorStns` comes from the effective (emitted) section - so must the index.
  const anchorStn = anchorStns[emittedAnchorStation];
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
}
