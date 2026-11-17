import { describe, it, expect } from 'vitest';
import {
  buildAdjacency,
  computeDmpBfsContext,
  stationPositionFromShotLines,
  discoverBridgedDmpSections,
  computeUnreachableSegments,
  computeSegmentLayout,
  LINE_COLORS,
  type VirtualConnection,
  type ExistingLineGeoEntry,
} from '../segmentContext';
import type { DmpSection } from '../../types/commands';
import type { DmpShot } from '../../types/ipc/DmpShot';

// ─── Test data helpers ──────────────────────────────────────────────────────

/** Create a standard DMP shot with the given heading (degrees) and length. */
function makeShot(heading: number, length: number, overrides: Partial<DmpShot> = {}): DmpShot {
  return {
    shot_type: 'Standard',
    heading_in: heading,
    heading_out: heading,
    length,
    depth_in: 0,
    depth_out: 0,
    pitch_in: 0,
    pitch_out: 0,
    left: 0, right: 0, up: 0, down: 0,
    temperature: 0, hour: 0, minute: 0, second: 0, marker_index: 0,
    ...overrides,
  };
}

/** Create a DMP line with shots heading in a single direction. */
function makeDmpSection(name: string, shots: DmpShot[]): DmpSection {
  return { version: 1, name, direction: 'in', date: '', shots, content_hash: '' };
}

/** Create an importing-type virtual connection. */
function importingConn(
  importIdx: number, importStation: number,
  targetIdx: number, targetStation: number,
): VirtualConnection {
  return {
    importLineIndex: importIdx,
    importStationIndex: importStation,
    targetType: 'importing',
    targetImportIndex: targetIdx,
    targetImportStationIndex: targetStation,
  };
}

/** Create an existing-type virtual connection. */
function existingConn(
  importIdx: number, importStation: number,
  existingLineId: number, existingStation: number,
): VirtualConnection {
  return {
    importLineIndex: importIdx,
    importStationIndex: importStation,
    targetType: 'existing',
    existingLineId,
    existingStationIndex: existingStation,
  };
}

// ─── stationPositionFromShotLines ───────────────────────────────────────────

describe('stationPositionFromShotLines', () => {
  it('returns null for empty shot_lines', () => {
    expect(stationPositionFromShotLines([], 0)).toBeNull();
    expect(stationPositionFromShotLines([], 1)).toBeNull();
  });

  it('returns station 0 from shot_lines[0][0]', () => {
    const shotLines = [[[10, 20], [30, 40]]];
    expect(stationPositionFromShotLines(shotLines, 0)).toEqual({ x: 10, y: 20 });
  });

  it('returns station 1 from shot_lines[0][1]', () => {
    const shotLines = [[[0, 0], [10, 20]]];
    expect(stationPositionFromShotLines(shotLines, 1)).toEqual({ x: 10, y: 20 });
  });

  it('returns station N from shot_lines[N-1][1]', () => {
    const shotLines = [[[0, 0], [10, 20]], [[10, 20], [30, 40]]];
    expect(stationPositionFromShotLines(shotLines, 2)).toEqual({ x: 30, y: 40 });
  });

  // Regression: previously hardcoded (0,0) for station 0, which broke
  // positioning for non-queried lines in segment results.
  it('returns non-zero position for station 0 of non-queried line', () => {
    const shotLines = [[[5, 10], [15, 25]]];
    expect(stationPositionFromShotLines(shotLines, 0)).toEqual({ x: 5, y: 10 });
  });

  it('returns null for out-of-range station index', () => {
    const shotLines = [[[0, 0], [10, 20]]];
    expect(stationPositionFromShotLines(shotLines, 5)).toBeNull();
  });

  it('returns null for malformed shot_lines', () => {
    expect(stationPositionFromShotLines([[]], 0)).toBeNull();
    expect(stationPositionFromShotLines([[[1]]], 0)).toBeNull();
  });
});

// ─── buildAdjacency ─────────────────────────────────────────────────────────

describe('buildAdjacency', () => {
  it('creates bidirectional edges for importing connections', () => {
    const conns = [importingConn(0, 1, 1, 0)];
    const adj = buildAdjacency(conns);

    expect(adj.get(0)?.map((e) => e.neighbor)).toEqual([1]);
    expect(adj.get(1)?.map((e) => e.neighbor)).toEqual([0]);
  });

  it('ignores existing connections (no adjacency edges)', () => {
    const conns = [existingConn(0, 1, 42, 3)];
    const adj = buildAdjacency(conns);

    // Line 0 is registered but has no neighbors
    expect(adj.get(0)).toEqual([]);
    expect(adj.has(42)).toBe(false);
  });

  it('handles mixed connection types', () => {
    const conns = [
      importingConn(0, 1, 1, 0),
      existingConn(0, 1, 42, 3),
    ];
    const adj = buildAdjacency(conns);

    expect(adj.get(0)?.map((e) => e.neighbor)).toEqual([1]);
    expect(adj.get(1)?.map((e) => e.neighbor)).toEqual([0]);
  });
});

// ─── computeDmpBfsContext ───────────────────────────────────────────────────

describe('computeDmpBfsContext', () => {
  // Line 0: 1 shot heading north (0°), length 10 → station 1 at (0, 10)
  // Line 1: 1 shot heading east (90°), length 5 → station 1 at (5, 0)
  // Line 2: 1 shot heading south (180°), length 8 → station 1 at (0, -8)
  const lines: DmpSection[] = [
    makeDmpSection('A', [makeShot(0, 10)]),
    makeDmpSection('B', [makeShot(90, 5)]),
    makeDmpSection('C', [makeShot(180, 8)]),
  ];

  it('returns empty context for a line with no connections', () => {
    const result = computeDmpBfsContext(0, new Set(), [], lines);
    expect(result.dmpContext).toEqual([]);
    expect(result.existingAnchors).toEqual([]);
    expect(result.visitedIndices).toEqual(new Set([0]));
  });

  it('finds a direct importing neighbor with correct positioning', () => {
    // Line 0 station 1 → Line 1 station 0
    const conns = [importingConn(0, 1, 1, 0)];
    const result = computeDmpBfsContext(0, new Set(), conns, lines);

    expect(result.dmpContext).toHaveLength(1);
    expect(result.visitedIndices).toEqual(new Set([0, 1]));

    // Line 0 station 1 is at (0, 10). Line 1 station 0 is (0, 0).
    // So line 1's offset = (0, 10).
    // Line 1's shot goes from (0+0, 10+0) to (0+5, 10+0).
    const ctx = result.dmpContext[0]!;
    expect(ctx.shots).toHaveLength(1);
    expect(ctx.shots[0]!.from[0]).toBeCloseTo(0, 5);
    expect(ctx.shots[0]!.from[1]).toBeCloseTo(10, 5);
    expect(ctx.shots[0]!.to[0]).toBeCloseTo(5, 5);
    expect(ctx.shots[0]!.to[1]).toBeCloseTo(10, 5);
  });

  it('chains BFS through multiple importing connections', () => {
    // Line 0 → Line 1 → Line 2
    const conns = [
      importingConn(0, 1, 1, 0),
      importingConn(1, 1, 2, 0),
    ];
    const result = computeDmpBfsContext(0, new Set(), conns, lines);

    expect(result.dmpContext).toHaveLength(2);
    expect(result.visitedIndices).toEqual(new Set([0, 1, 2]));
  });

  it('respects exclude indices', () => {
    const conns = [
      importingConn(0, 1, 1, 0),
      importingConn(1, 1, 2, 0),
    ];
    // Exclude line 1 - BFS from 0 should not visit 1 or reach 2
    const result = computeDmpBfsContext(0, new Set([1]), conns, lines);

    expect(result.dmpContext).toEqual([]);
    expect(result.visitedIndices).toEqual(new Set([0]));
  });

  it('collects existing anchors from the start node', () => {
    // Line 0 has an existing connection to project line 42 at station 1
    const conns = [existingConn(0, 1, 42, 3)];
    const result = computeDmpBfsContext(0, new Set(), conns, lines);

    expect(result.dmpContext).toEqual([]);
    expect(result.existingAnchors).toHaveLength(1);

    const anchor = result.existingAnchors[0]!;
    expect(anchor.existingLineId).toBe(42);
    expect(anchor.existingStationIdx).toBe(3);
    // Station 1 of line 0 (heading 0°, length 10) is at (0, 10)
    expect(anchor.x).toBeCloseTo(0, 5);
    expect(anchor.y).toBeCloseTo(10, 5);
  });

  it('collects existing anchors from BFS-visited neighbors', () => {
    // Line 0 → Line 1 (importing), Line 1 → existing 99 (existing)
    const conns = [
      importingConn(0, 1, 1, 0),
      existingConn(1, 1, 99, 5),
    ];
    const result = computeDmpBfsContext(0, new Set(), conns, lines);

    expect(result.dmpContext).toHaveLength(1);
    expect(result.existingAnchors).toHaveLength(1);

    const anchor = result.existingAnchors[0]!;
    expect(anchor.existingLineId).toBe(99);
    expect(anchor.existingStationIdx).toBe(5);
    // Line 1 is offset by (0, 10) (from line 0's station 1).
    // Line 1's station 1 at (5, 0) in its own frame → (5, 10) in BFS frame.
    expect(anchor.x).toBeCloseTo(5, 5);
    expect(anchor.y).toBeCloseTo(10, 5);
  });

  it('exclude indices are not in visitedIndices', () => {
    const conns = [importingConn(0, 1, 1, 0)];
    const result = computeDmpBfsContext(0, new Set([2]), conns, lines);

    expect(result.visitedIndices.has(2)).toBe(false);
    expect(result.visitedIndices.has(0)).toBe(true);
    expect(result.visitedIndices.has(1)).toBe(true);
  });
});

// ─── discoverBridgedDmpSections ─────────────────────────────────────────────

describe('discoverBridgedDmpSections', () => {
  // Line 0: heading north, length 10 → station 1 at (0, 10)
  // Line 1: heading east, length 5 → station 1 at (5, 0)
  // Line 2: heading south, length 8 → station 1 at (0, -8)
  // Line 3: heading east, length 3 → station 1 at (3, 0)
  const lines: DmpSection[] = [
    makeDmpSection('A', [makeShot(0, 10)]),
    makeDmpSection('B', [makeShot(90, 5)]),
    makeDmpSection('C', [makeShot(180, 8)]),
    makeDmpSection('D', [makeShot(90, 3)]),
  ];

  // Existing line X: station 0 at origin, station 1 at (10, 0)
  // shot_lines: [[[0, 0], [10, 0]]]
  const existingXGeo: ExistingLineGeoEntry = {
    shotLines: [[[0, 0], [10, 0]]],
    frameOffsetX: 0,
    frameOffsetY: 0,
    side: 'target',
  };

  it('returns empty when no connections match', () => {
    const result = discoverBridgedDmpSections([], new Set([0]), new Map(), lines);

    expect(result.importSide).toEqual([]);
    expect(result.targetSide).toEqual([]);
  });

  it('returns empty when all connected lines are already shown', () => {
    const conns = [existingConn(1, 0, 42, 0)];
    const geoMap = new Map<number, ExistingLineGeoEntry>([[42, existingXGeo]]);

    // Line 1 already shown
    const result = discoverBridgedDmpSections(conns, new Set([0, 1]), geoMap, lines);
    expect(result.targetSide).toEqual([]);
  });

  // Regression: DMP lines A and B both connect to existing X.
  // When previewing C→X, A and B should appear as context.
  it('discovers DMP lines connected through shared existing line', () => {
    const conns = [
      existingConn(0, 0, 42, 0),  // Line A → existing X at station 0
      existingConn(1, 0, 42, 1),  // Line B → existing X at station 1
    ];
    const geoMap = new Map<number, ExistingLineGeoEntry>([[42, existingXGeo]]);

    // Current import line (2) is shown, A and B are not
    const result = discoverBridgedDmpSections(conns, new Set([2]), geoMap, lines);

    // Both A and B should be discovered on the target side (existingXGeo.side = 'target')
    expect(result.targetSide).toHaveLength(2);
    expect(result.discoveredIndices).toContain(0);
    expect(result.discoveredIndices).toContain(1);
  });

  it('positions bridged line correctly relative to existing merge station', () => {
    // Line A (index 0) connects to existing X at:
    //   A's station 0 → existing X's station 0 (which is at (0, 0) in X's frame)
    const conns = [existingConn(0, 0, 42, 0)];
    const geoMap = new Map<number, ExistingLineGeoEntry>([[42, existingXGeo]]);

    const result = discoverBridgedDmpSections(conns, new Set([2]), geoMap, lines);

    expect(result.targetSide).toHaveLength(1);
    const ctx = result.targetSide[0]!;

    // Line A: heading 0°, length 10. Station 0 at origin, station 1 at (0, 10).
    // Existing X station 0 is at (0, 0). frameOffset is (0, 0).
    // A's station 0 is at (0, 0) in its own frame.
    // Offset = (0 + 0 - 0, 0 + 0 - 0) = (0, 0).
    // So A's shot goes from (0, 0) to (0, 10).
    expect(ctx.shots).toHaveLength(1);
    expect(ctx.shots[0]!.from[0]).toBeCloseTo(0, 5);
    expect(ctx.shots[0]!.from[1]).toBeCloseTo(0, 5);
    expect(ctx.shots[0]!.to[0]).toBeCloseTo(0, 5);
    expect(ctx.shots[0]!.to[1]).toBeCloseTo(10, 5);
  });

  it('positions bridged line with non-zero merge station', () => {
    // Line B (index 1) connects to existing X at:
    //   B's station 0 → existing X's station 1 (at (10, 0) in X's frame)
    const conns = [existingConn(1, 0, 42, 1)];
    const geoMap = new Map<number, ExistingLineGeoEntry>([[42, existingXGeo]]);

    const result = discoverBridgedDmpSections(conns, new Set([2]), geoMap, lines);

    expect(result.targetSide).toHaveLength(1);
    const ctx = result.targetSide[0]!;

    // Line B: heading 90°, length 5. Station 0 at (0,0), station 1 at (5, 0).
    // Existing X station 1 is at (10, 0). frameOffset is (0, 0).
    // B's station 0 is at (0, 0).
    // Offset = (0 + 10 - 0, 0 + 0 - 0) = (10, 0).
    // B's shot goes from (0+10, 0+0) to (5+10, 0+0) = (10, 0) to (15, 0).
    expect(ctx.shots).toHaveLength(1);
    expect(ctx.shots[0]!.from[0]).toBeCloseTo(10, 5);
    expect(ctx.shots[0]!.from[1]).toBeCloseTo(0, 5);
    expect(ctx.shots[0]!.to[0]).toBeCloseTo(15, 5);
    expect(ctx.shots[0]!.to[1]).toBeCloseTo(0, 5);
  });

  it('applies frameOffset from the existing geo entry', () => {
    const geoWithOffset: ExistingLineGeoEntry = {
      shotLines: [[[0, 0], [10, 0]]],
      frameOffsetX: 100,
      frameOffsetY: 200,
      side: 'import',
    };
    const conns = [existingConn(0, 0, 42, 0)];
    const geoMap = new Map<number, ExistingLineGeoEntry>([[42, geoWithOffset]]);

    const result = discoverBridgedDmpSections(conns, new Set([2]), geoMap, lines);

    expect(result.importSide).toHaveLength(1);
    const ctx = result.importSide[0]!;

    // frameOffset (100, 200), existing station 0 at (0, 0), A's station 0 at (0, 0)
    // Offset = (100 + 0 - 0, 200 + 0 - 0) = (100, 200)
    expect(ctx.shots[0]!.from[0]).toBeCloseTo(100, 5);
    expect(ctx.shots[0]!.from[1]).toBeCloseTo(200, 5);
    expect(ctx.shots[0]!.to[0]).toBeCloseTo(100, 5);
    expect(ctx.shots[0]!.to[1]).toBeCloseTo(210, 5);
  });

  it('assigns to correct side based on geo entry', () => {
    const targetGeo: ExistingLineGeoEntry = { ...existingXGeo, side: 'target' };
    const importGeo: ExistingLineGeoEntry = {
      shotLines: [[[0, 0], [5, 0]]],
      frameOffsetX: 0, frameOffsetY: 0, side: 'import',
    };

    const conns = [
      existingConn(0, 0, 42, 0),  // connects to target-side existing
      existingConn(1, 0, 99, 0),  // connects to import-side existing
    ];
    const geoMap = new Map<number, ExistingLineGeoEntry>([
      [42, targetGeo],
      [99, importGeo],
    ]);

    const result = discoverBridgedDmpSections(conns, new Set([2]), geoMap, lines);

    expect(result.targetSide.length).toBeGreaterThan(0);
    expect(result.importSide.length).toBeGreaterThan(0);
  });

  it('follows importing-type connections from bridged lines', () => {
    // Line A → existing X (bridge), Line A → Line D (importing)
    const conns = [
      existingConn(0, 0, 42, 0),
      importingConn(0, 1, 3, 0),
    ];
    const geoMap = new Map<number, ExistingLineGeoEntry>([[42, existingXGeo]]);

    const result = discoverBridgedDmpSections(conns, new Set([2]), geoMap, lines);

    // A is discovered via bridge, D is discovered via BFS from A
    expect(result.discoveredIndices).toContain(0);
    expect(result.discoveredIndices).toContain(3);
    // Both A and D should have context geometry
    expect(result.targetSide.length).toBe(2);
  });

  it('does not duplicate lines already shown', () => {
    const conns = [existingConn(0, 0, 42, 0)];
    const geoMap = new Map<number, ExistingLineGeoEntry>([[42, existingXGeo]]);

    // Line 0 already shown
    const result = discoverBridgedDmpSections(conns, new Set([0, 2]), geoMap, lines);

    expect(result.targetSide).toEqual([]);
    expect(result.importSide).toEqual([]);
  });

  it('skips connections to unknown existing lines', () => {
    // Line A connects to existing line 999, which is not in the geo map
    const conns = [existingConn(0, 0, 999, 0)];
    const geoMap = new Map<number, ExistingLineGeoEntry>([[42, existingXGeo]]);

    const result = discoverBridgedDmpSections(conns, new Set([2]), geoMap, lines);
    expect(result.targetSide).toEqual([]);
  });

  it('uses correct color from LINE_COLORS', () => {
    const conns = [existingConn(1, 0, 42, 0)];
    const geoMap = new Map<number, ExistingLineGeoEntry>([[42, existingXGeo]]);

    const result = discoverBridgedDmpSections(conns, new Set([2]), geoMap, lines);
    expect(result.targetSide).toHaveLength(1);
    expect(result.targetSide[0]!.color).toBe(LINE_COLORS[1 % LINE_COLORS.length]);
  });
});

// ─── computeUnreachableSegments ──────────────────────────────────────────────

describe('computeUnreachableSegments', () => {
  it('returns empty result for empty input', () => {
    expect(computeUnreachableSegments([], [])).toEqual([]);
  });

  it('returns single isolated line as a segment of 1', () => {
    const result = computeUnreachableSegments([3], []);
    expect(result).toEqual([[3]]);
  });

  it('groups two connected lines into one segment', () => {
    const conns = [importingConn(1, 0, 2, 0)];
    const result = computeUnreachableSegments([1, 2], conns);
    expect(result).toEqual([[1, 2]]);
  });

  it('splits three lines into two segments when one is disconnected', () => {
    // 1-2 connected, 3 isolated
    const conns = [importingConn(1, 0, 2, 0)];
    const result = computeUnreachableSegments([1, 2, 3], conns);
    expect(result).toEqual([[1, 2], [3]]);
  });

  it('ignores connections where one endpoint is not in unreachable set', () => {
    // Connection from 1→0, but 0 is not unreachable
    const conns = [importingConn(1, 0, 0, 0)];
    const result = computeUnreachableSegments([1, 2], conns);
    // Each is its own segment since the connection to 0 is excluded
    expect(result).toEqual([[1], [2]]);
  });

  it('ignores existing-type connections', () => {
    const conns = [existingConn(1, 0, 42, 0)];
    const result = computeUnreachableSegments([1, 2], conns);
    expect(result).toEqual([[1], [2]]);
  });

  it('chains through multiple importing connections', () => {
    const conns = [
      importingConn(1, 0, 2, 0),
      importingConn(2, 1, 3, 0),
    ];
    const result = computeUnreachableSegments([1, 2, 3], conns);
    expect(result).toEqual([[1, 2, 3]]);
  });

  it('sorts segments by first index', () => {
    const conns = [importingConn(5, 0, 6, 0)];
    const result = computeUnreachableSegments([5, 6, 2], conns);
    // Segment [2] comes first, then [5, 6]
    expect(result).toEqual([[2], [5, 6]]);
  });
});

// ─── computeSegmentLayout ───────────────────────────────────────────────────

describe('computeSegmentLayout', () => {
  // Line 0: 1 shot heading north (0°), length 10 → station 1 at (0, 10)
  // Line 1: 1 shot heading east (90°), length 5 → station 1 at (5, 0)
  // Line 2: 1 shot heading south (180°), length 8 → station 1 at (0, -8)
  const lines: DmpSection[] = [
    makeDmpSection('A', [makeShot(0, 10)]),
    makeDmpSection('B', [makeShot(90, 5)]),
    makeDmpSection('C', [makeShot(180, 8)]),
  ];

  it('returns single entry at origin for segment of 1', () => {
    const result = computeSegmentLayout([0], [], lines);
    expect(result).toEqual([{ lineIndex: 0, offsetX: 0, offsetY: 0 }]);
  });

  it('computes correct offset for two connected lines', () => {
    // Line 0 station 1 → Line 1 station 0
    // Line 0 station 1 is at (0, 10). Line 1 station 0 is at (0, 0).
    // Offset for line 1 = (0 + 0 - 0, 0 + 10 - 0) = (0, 10)
    const conns = [importingConn(0, 1, 1, 0)];
    const result = computeSegmentLayout([0, 1], conns, lines);

    expect(result).toHaveLength(2);
    expect(result[0]).toEqual({ lineIndex: 0, offsetX: 0, offsetY: 0 });
    expect(result[1]!.lineIndex).toBe(1);
    expect(result[1]!.offsetX).toBeCloseTo(0, 5);
    expect(result[1]!.offsetY).toBeCloseTo(10, 5);
  });

  it('chains through multiple connections', () => {
    // 0 → 1 → 2
    const conns = [
      importingConn(0, 1, 1, 0),
      importingConn(1, 1, 2, 0),
    ];
    const result = computeSegmentLayout([0, 1, 2], conns, lines);

    expect(result).toHaveLength(3);
    // Line 1 offset = (0, 10) (from line 0's station 1)
    expect(result[1]!.offsetX).toBeCloseTo(0, 5);
    expect(result[1]!.offsetY).toBeCloseTo(10, 5);
    // Line 2 offset: line 1's station 1 at (5, 0) + line 1's offset (0, 10) = (5, 10)
    // Line 2's station 0 at (0, 0), so offset = (5 - 0, 10 - 0) = (5, 10)
    expect(result[2]!.offsetX).toBeCloseTo(5, 5);
    expect(result[2]!.offsetY).toBeCloseTo(10, 5);
  });

  it('returns empty for empty input', () => {
    expect(computeSegmentLayout([], [], lines)).toEqual([]);
  });

  it('includes disconnected lines at origin', () => {
    // Line 0 and 1 are in the segment but have no connection between them
    const result = computeSegmentLayout([0, 1], [], lines);
    expect(result).toHaveLength(2);
    // Root (0) at origin, line 1 also at origin since no connection to offset it
    expect(result[0]).toEqual({ lineIndex: 0, offsetX: 0, offsetY: 0 });
    expect(result[1]).toEqual({ lineIndex: 1, offsetX: 0, offsetY: 0 });
  });
});
