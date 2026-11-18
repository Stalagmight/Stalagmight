/**
 * Pure helpers for computing segment context geometry in the DMP import wizard.
 * These functions position DMP lines and existing project lines relative
 * to each other using BFS through virtual connections and segment geometry data.
 */
import type { DmpSection } from '../types/commands';
import { type ContextLineGeometry, computeStationsFromDmpShots } from '../components/Dialogs/LinePreview';
import { LINE_COLORS } from './colors';
export { LINE_COLORS } from './colors';

/** A virtual connection recorded by the user - no backend import yet. */
export type VirtualConnection = {
  importLineIndex: number;
  importStationIndex: number;
  targetType: 'existing' | 'importing';
  existingLineId?: number;
  existingStationIndex?: number;
  targetImportIndex?: number;
  targetImportStationIndex?: number;
};

export type ExistingAnchor = {
  existingLineId: number;
  existingStationIdx: number;
  x: number;
  y: number;
};

/** Stored geometry for an existing project line, positioned in a coordinate frame. */
export type ExistingLineGeoEntry = {
  shotLines: Array<Array<Array<number>>>;
  frameOffsetX: number;
  frameOffsetY: number;
  side: 'target' | 'import';
};

/** Build adjacency map from virtual connections (importing-type only). */
export function buildAdjacency(
  connections: VirtualConnection[],
): Map<number, Array<{ neighbor: number; connection: VirtualConnection }>> {
  const adj = new Map<number, Array<{ neighbor: number; connection: VirtualConnection }>>();
  const ensure = (k: number) => { if (!adj.has(k)) adj.set(k, []); };

  for (const c of connections) {
    ensure(c.importLineIndex);
    if (c.targetType === 'importing' && c.targetImportIndex != null) {
      ensure(c.targetImportIndex);
      adj.get(c.importLineIndex)!.push({ neighbor: c.targetImportIndex, connection: c });
      adj.get(c.targetImportIndex)!.push({ neighbor: c.importLineIndex, connection: c });
    }
    // 'existing' connections anchor this node to the project graph - no import neighbor
  }
  return adj;
}

/**
 * BFS from a DMP line through importing-type connections, collecting
 * neighboring DMP line geometry and existing-type anchor positions.
 */
export function computeDmpBfsContext(
  startIdx: number,
  excludeIndices: Set<number>,
  connections: VirtualConnection[],
  dmpSections: DmpSection[],
): {
  dmpContext: ContextLineGeometry[];
  existingAnchors: ExistingAnchor[];
  visitedIndices: Set<number>;
} {
  const adjacency = buildAdjacency(connections);
  const visited = new Set<number>();
  const queue: Array<{ idx: number; offsetX: number; offsetY: number }> = [];
  visited.add(startIdx);
  for (const ex of excludeIndices) visited.add(ex);
  queue.push({ idx: startIdx, offsetX: 0, offsetY: 0 });

  const dmpContext: ContextLineGeometry[] = [];
  const existingAnchors: ExistingAnchor[] = [];

  while (queue.length > 0) {
    const { idx, offsetX, offsetY } = queue.shift()!;

    // Collect existing-type anchors from this node
    const nodeStations = computeStationsFromDmpShots(
      (dmpSections[idx]?.shots ?? []).filter((s) => s.shot_type === 'Standard')
    );
    for (const conn of connections) {
      if (
        conn.targetType === 'existing' &&
        conn.importLineIndex === idx &&
        conn.existingLineId != null &&
        conn.existingStationIndex != null
      ) {
        const stn = nodeStations[conn.importStationIndex];
        if (stn) {
          existingAnchors.push({
            existingLineId: conn.existingLineId,
            existingStationIdx: conn.existingStationIndex,
            x: offsetX + stn.x,
            y: offsetY + stn.y,
          });
        }
      }
    }

    // BFS to importing-type neighbors
    for (const edge of adjacency.get(idx) ?? []) {
      if (visited.has(edge.neighbor)) continue;
      visited.add(edge.neighbor);

      const conn = edge.connection;
      let stationInA: number;
      let stationInB: number;
      let lineA: number;
      if (conn.importLineIndex === idx && conn.targetImportIndex === edge.neighbor) {
        lineA = idx;
        stationInA = conn.importStationIndex;
        stationInB = conn.targetImportStationIndex ?? 0;
      } else if (conn.targetImportIndex === idx && conn.importLineIndex === edge.neighbor) {
        lineA = conn.targetImportIndex;
        stationInA = conn.targetImportStationIndex ?? 0;
        stationInB = conn.importStationIndex;
      } else {
        continue;
      }

      const lineAData = dmpSections[lineA];
      const neighborData = dmpSections[edge.neighbor];
      if (!lineAData || !neighborData) continue;

      const stationsA = computeStationsFromDmpShots(
        lineAData.shots.filter((s) => s.shot_type === 'Standard')
      );
      const stationsB = computeStationsFromDmpShots(
        neighborData.shots.filter((s) => s.shot_type === 'Standard')
      );

      const posA = stationsA[stationInA];
      const posB = stationsB[stationInB];
      if (!posA || !posB) continue;

      const neighborOffsetX = offsetX + posA.x - posB.x;
      const neighborOffsetY = offsetY + posA.y - posB.y;

      const colorIdx = edge.neighbor % LINE_COLORS.length;
      const neighborColor = LINE_COLORS[colorIdx] ?? '#95a5a6';

      const shots: Array<{ from: [number, number]; to: [number, number] }> = [];
      for (let i = 0; i < stationsB.length - 1; i++) {
        const from = stationsB[i]!;
        const to = stationsB[i + 1]!;
        shots.push({
          from: [from.x + neighborOffsetX, from.y + neighborOffsetY],
          to: [to.x + neighborOffsetX, to.y + neighborOffsetY],
        });
      }

      if (shots.length > 0) {
        dmpContext.push({ color: neighborColor, shots });
      }

      queue.push({ idx: edge.neighbor, offsetX: neighborOffsetX, offsetY: neighborOffsetY });
    }
  }

  // Return visited indices minus excludes (those are shown by this BFS)
  const visitedIndices = new Set(visited);
  for (const ex of excludeIndices) visitedIndices.delete(ex);
  return { dmpContext, existingAnchors, visitedIndices };
}

/**
 * Extract a station's XY position from backend shot_lines geometry.
 * Works for both the queried line (station 0 at origin = shot_lines[0][0])
 * and other lines in a segment result (station 0 at their global position).
 */
export function stationPositionFromShotLines(
  shotLines: Array<Array<Array<number>>>,
  stationIdx: number,
): { x: number; y: number } | null {
  if (shotLines.length === 0) return null;
  if (stationIdx === 0) {
    const firstLine = shotLines[0];
    if (!firstLine || firstLine.length < 1 || !firstLine[0] || firstLine[0].length < 2) return null;
    return { x: firstLine[0][0]!, y: firstLine[0][1]! };
  }
  const line = shotLines[stationIdx - 1];
  if (!line || line.length < 2 || !line[1] || line[1].length < 2) return null;
  return { x: line[1][0]!, y: line[1][1]! };
}

/**
 * Discover DMP lines that are connected through shared existing project lines
 * ("bridges") but have no direct importing-type edge between them.
 *
 * For example: DMP lines A and B both connect to existing line X.
 * When previewing C→X, A and B should appear as context even though they
 * are not reachable from C via importing-type BFS.
 */
/**
 * Group unreachable DMP line indices into connected components
 * via importing-type virtual connections restricted to the unreachable set.
 * Returns array of segments (each sorted by index), ordered by first index.
 */
export function computeUnreachableSegments(
  unreachableIndices: number[],
  connections: VirtualConnection[],
): number[][] {
  if (unreachableIndices.length === 0) return [];

  const unreachableSet = new Set(unreachableIndices);

  // Build adjacency restricted to unreachable indices
  const adj = new Map<number, number[]>();
  for (const idx of unreachableIndices) adj.set(idx, []);

  for (const c of connections) {
    if (c.targetType !== 'importing' || c.targetImportIndex == null) continue;
    const a = c.importLineIndex;
    const b = c.targetImportIndex;
    if (!unreachableSet.has(a) || !unreachableSet.has(b)) continue;
    adj.get(a)!.push(b);
    adj.get(b)!.push(a);
  }

  // BFS to find connected components
  const visited = new Set<number>();
  const segments: number[][] = [];

  for (const seed of unreachableIndices) {
    if (visited.has(seed)) continue;
    visited.add(seed);
    const component: number[] = [seed];
    const queue = [seed];

    while (queue.length > 0) {
      const node = queue.shift()!;
      for (const neighbor of adj.get(node) ?? []) {
        if (visited.has(neighbor)) continue;
        visited.add(neighbor);
        component.push(neighbor);
        queue.push(neighbor);
      }
    }

    component.sort((a, b) => a - b);
    segments.push(component);
  }

  segments.sort((a, b) => a[0]! - b[0]!);
  return segments;
}

/**
 * Compute XY offsets for each line in a segment relative to the first
 * line's station 0. Uses BFS through importing-type connections within
 * the segment, aligning merge stations to compute offsets.
 */
export function computeSegmentLayout(
  segmentIndices: number[],
  connections: VirtualConnection[],
  dmpSections: DmpSection[],
): Array<{ lineIndex: number; offsetX: number; offsetY: number }> {
  if (segmentIndices.length === 0) return [];

  const segmentSet = new Set(segmentIndices);
  const root = segmentIndices[0]!;

  // Build adjacency restricted to this segment
  const adj = new Map<number, Array<{ neighbor: number; connection: VirtualConnection }>>();
  for (const idx of segmentIndices) adj.set(idx, []);

  for (const c of connections) {
    if (c.targetType !== 'importing' || c.targetImportIndex == null) continue;
    const a = c.importLineIndex;
    const b = c.targetImportIndex;
    if (!segmentSet.has(a) || !segmentSet.has(b)) continue;
    adj.get(a)!.push({ neighbor: b, connection: c });
    adj.get(b)!.push({ neighbor: a, connection: c });
  }

  const result: Array<{ lineIndex: number; offsetX: number; offsetY: number }> = [];
  const visited = new Set<number>();
  const queue: Array<{ idx: number; offsetX: number; offsetY: number }> = [];

  visited.add(root);
  queue.push({ idx: root, offsetX: 0, offsetY: 0 });

  while (queue.length > 0) {
    const { idx, offsetX, offsetY } = queue.shift()!;
    result.push({ lineIndex: idx, offsetX, offsetY });

    for (const edge of adj.get(idx) ?? []) {
      if (visited.has(edge.neighbor)) continue;
      visited.add(edge.neighbor);

      const conn = edge.connection;
      let stationInA: number;
      let stationInB: number;
      let lineA: number;

      if (conn.importLineIndex === idx && conn.targetImportIndex === edge.neighbor) {
        lineA = idx;
        stationInA = conn.importStationIndex;
        stationInB = conn.targetImportStationIndex ?? 0;
      } else if (conn.targetImportIndex === idx && conn.importLineIndex === edge.neighbor) {
        lineA = conn.targetImportIndex;
        stationInA = conn.targetImportStationIndex ?? 0;
        stationInB = conn.importStationIndex;
      } else {
        continue;
      }

      const lineAData = dmpSections[lineA];
      const neighborData = dmpSections[edge.neighbor];
      if (!lineAData || !neighborData) continue;

      const stationsA = computeStationsFromDmpShots(
        lineAData.shots.filter((s) => s.shot_type === 'Standard')
      );
      const stationsB = computeStationsFromDmpShots(
        neighborData.shots.filter((s) => s.shot_type === 'Standard')
      );

      const posA = stationsA[stationInA];
      const posB = stationsB[stationInB];
      if (!posA || !posB) continue;

      const neighborOffsetX = offsetX + posA.x - posB.x;
      const neighborOffsetY = offsetY + posA.y - posB.y;

      queue.push({ idx: edge.neighbor, offsetX: neighborOffsetX, offsetY: neighborOffsetY });
    }
  }

  // Include any unreachable lines within the segment (disconnected from root)
  for (const idx of segmentIndices) {
    if (!visited.has(idx)) {
      result.push({ lineIndex: idx, offsetX: 0, offsetY: 0 });
    }
  }

  return result;
}

export function discoverBridgedDmpSections(
  connections: VirtualConnection[],
  initialShownIndices: Set<number>,
  existingLineGeo: Map<number, ExistingLineGeoEntry>,
  dmpSections: DmpSection[],
): {
  importSide: ContextLineGeometry[];
  targetSide: ContextLineGeometry[];
  discoveredIndices: Set<number>;
} {
  const importSide: ContextLineGeometry[] = [];
  const targetSide: ContextLineGeometry[] = [];
  const shownIndices = new Set(initialShownIndices);

  for (const conn of connections) {
    if (conn.targetType !== 'existing') continue;
    if (shownIndices.has(conn.importLineIndex)) continue;
    if (conn.existingLineId == null || conn.existingStationIndex == null) continue;

    const geo = existingLineGeo.get(conn.existingLineId);
    if (!geo) continue;

    // Found a DMP line connected to an existing line we have geometry for
    const existingMergePos = stationPositionFromShotLines(geo.shotLines, conn.existingStationIndex);
    if (!existingMergePos) continue;

    const dmpSection = dmpSections[conn.importLineIndex];
    if (!dmpSection) continue;

    const dmpStations = computeStationsFromDmpShots(
      dmpSection.shots.filter((s) => s.shot_type === 'Standard')
    );
    const dmpMergeStation = dmpStations[conn.importStationIndex];
    if (!dmpMergeStation) continue;

    const dmpOffsetX = geo.frameOffsetX + existingMergePos.x - dmpMergeStation.x;
    const dmpOffsetY = geo.frameOffsetY + existingMergePos.y - dmpMergeStation.y;

    // Build shot lines for this bridged DMP line
    const colorIdx = conn.importLineIndex % LINE_COLORS.length;
    const color = LINE_COLORS[colorIdx] ?? '#95a5a6';
    const dmpShots: Array<{ from: [number, number]; to: [number, number] }> = [];
    for (let i = 0; i < dmpStations.length - 1; i++) {
      const from = dmpStations[i]!;
      const to = dmpStations[i + 1]!;
      dmpShots.push({
        from: [from.x + dmpOffsetX, from.y + dmpOffsetY],
        to: [to.x + dmpOffsetX, to.y + dmpOffsetY],
      });
    }

    if (dmpShots.length > 0) {
      if (geo.side === 'target') targetSide.push({ color, shots: dmpShots });
      else importSide.push({ color, shots: dmpShots });
    }

    shownIndices.add(conn.importLineIndex);

    // BFS from this bridged DMP line for its importing-type neighbors
    const bridgeBfs = computeDmpBfsContext(conn.importLineIndex, shownIndices, connections, dmpSections);
    for (const idx of bridgeBfs.visitedIndices) shownIndices.add(idx);

    // Offset BFS context into the correct frame
    for (const ctx of bridgeBfs.dmpContext) {
      const offsetCtx: ContextLineGeometry = {
        color: ctx.color,
        shots: ctx.shots.map((shot) => ({
          from: [shot.from[0] + dmpOffsetX, shot.from[1] + dmpOffsetY] as [number, number],
          to: [shot.to[0] + dmpOffsetX, shot.to[1] + dmpOffsetY] as [number, number],
        })),
      };
      if (geo.side === 'target') targetSide.push(offsetCtx);
      else importSide.push(offsetCtx);
    }
  }

  return { importSide, targetSide, discoveredIndices: shownIndices };
}
