import { describe, it, expect } from 'vitest';
import {
  buildAdjacency,
  computeDmpBfsContext,
  stationPositionFromShotLines,
  discoverBridgedDmpSections,
  computeUnreachableSegments,
  computeSegmentLayout,
  SECTION_COLORS,
  type VirtualConnection,
  type ExistingSectionGeoEntry,
} from '../segmentContext';
import type { DmpSection } from '../../types/commands';
import type { DmpShot } from '../../types/ipc/DmpShot';
import { makeDmpSection as dmpSectionOf, makeDmpShot as dmpShotOf } from '../../test/factories/dmp';

// ─── Test data helpers ──────────────────────────────────────────────────────

/** Create a standard DMP shot with the given heading (degrees) and length. */
function makeShot(heading: number, length: number, overrides: Partial<DmpShot> = {}): DmpShot {
  return dmpShotOf({ heading_in: heading, heading_out: heading, length, ...overrides });
}

/** Create a DMP section with shots heading in a single direction. */
function makeDmpSection(name: string, shots: DmpShot[]): DmpSection {
  return dmpSectionOf({ name, date: '', shots, content_hash: '' });
}

/** Create an importing-type virtual connection. */
function importingConn(
  importIdx: number, importStation: number,
  targetIdx: number, targetStation: number,
): VirtualConnection {
  return {
    importSectionIndex: importIdx,
    importStationIndex: importStation,
    targetType: 'importing',
    targetImportIndex: targetIdx,
    targetImportStationIndex: targetStation,
  };
}

/** Create an existing-type virtual connection. */
function existingConn(
  importIdx: number, importStation: number,
  existingSectionId: number, existingStation: number,
): VirtualConnection {
  return {
    importSectionIndex: importIdx,
    importStationIndex: importStation,
    targetType: 'existing',
    existingSectionId,
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
  // positioning for non-queried sections in segment results.
  it('returns non-zero position for station 0 of non-queried section', () => {
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

    // Section 0 is registered but has no neighbors
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
  // Section 0: 1 shot heading north (0°), length 10 → station 1 at (0, 10)
  // Section 1: 1 shot heading east (90°), length 5 → station 1 at (5, 0)
  // Section 2: 1 shot heading south (180°), length 8 → station 1 at (0, -8)
  const sections: DmpSection[] = [
    makeDmpSection('A', [makeShot(0, 10)]),
    makeDmpSection('B', [makeShot(90, 5)]),
    makeDmpSection('C', [makeShot(180, 8)]),
  ];

  it('returns empty context for a section with no connections', () => {
    const result = computeDmpBfsContext(0, new Set(), [], sections);
    expect(result.dmpContext).toEqual([]);
    expect(result.existingAnchors).toEqual([]);
    expect(result.visitedIndices).toEqual(new Set([0]));
  });

  it('finds a direct importing neighbor with correct positioning', () => {
    // Section 0 station 1 → Section 1 station 0
    const conns = [importingConn(0, 1, 1, 0)];
    const result = computeDmpBfsContext(0, new Set(), conns, sections);

    expect(result.dmpContext).toHaveLength(1);
    expect(result.visitedIndices).toEqual(new Set([0, 1]));

    // Section 0 station 1 is at (0, 10). Section 1 station 0 is (0, 0).
    // So section 1's offset = (0, 10).
    // Section 1's shot goes from (0+0, 10+0) to (0+5, 10+0).
    const ctx = result.dmpContext[0]!;
    expect(ctx.shots).toHaveLength(1);
    expect(ctx.shots[0]!.from[0]).toBeCloseTo(0, 5);
    expect(ctx.shots[0]!.from[1]).toBeCloseTo(10, 5);
    expect(ctx.shots[0]!.to[0]).toBeCloseTo(5, 5);
    expect(ctx.shots[0]!.to[1]).toBeCloseTo(10, 5);
  });

  it('chains BFS through multiple importing connections', () => {
    // Section 0 → Section 1 → Section 2
    const conns = [
      importingConn(0, 1, 1, 0),
      importingConn(1, 1, 2, 0),
    ];
    const result = computeDmpBfsContext(0, new Set(), conns, sections);

    expect(result.dmpContext).toHaveLength(2);
    expect(result.visitedIndices).toEqual(new Set([0, 1, 2]));
  });

  it('respects exclude indices', () => {
    const conns = [
      importingConn(0, 1, 1, 0),
      importingConn(1, 1, 2, 0),
    ];
    // Exclude section 1 - BFS from 0 should not visit 1 or reach 2
    const result = computeDmpBfsContext(0, new Set([1]), conns, sections);

    expect(result.dmpContext).toEqual([]);
    expect(result.visitedIndices).toEqual(new Set([0]));
  });

  it('collects existing anchors from the start node', () => {
    // Section 0 has an existing connection to project section 42 at station 1
    const conns = [existingConn(0, 1, 42, 3)];
    const result = computeDmpBfsContext(0, new Set(), conns, sections);

    expect(result.dmpContext).toEqual([]);
    expect(result.existingAnchors).toHaveLength(1);

    const anchor = result.existingAnchors[0]!;
    expect(anchor.existingSectionId).toBe(42);
    expect(anchor.existingStationIdx).toBe(3);
    // Station 1 of section 0 (heading 0°, length 10) is at (0, 10)
    expect(anchor.x).toBeCloseTo(0, 5);
    expect(anchor.y).toBeCloseTo(10, 5);
  });

  it('collects existing anchors from BFS-visited neighbors', () => {
    // Section 0 → Section 1 (importing), Section 1 → existing 99 (existing)
    const conns = [
      importingConn(0, 1, 1, 0),
      existingConn(1, 1, 99, 5),
    ];
    const result = computeDmpBfsContext(0, new Set(), conns, sections);

    expect(result.dmpContext).toHaveLength(1);
    expect(result.existingAnchors).toHaveLength(1);

    const anchor = result.existingAnchors[0]!;
    expect(anchor.existingSectionId).toBe(99);
    expect(anchor.existingStationIdx).toBe(5);
    // Section 1 is offset by (0, 10) (from section 0's station 1).
    // Section 1's station 1 at (5, 0) in its own frame → (5, 10) in BFS frame.
    expect(anchor.x).toBeCloseTo(5, 5);
    expect(anchor.y).toBeCloseTo(10, 5);
  });

  it('exclude indices are not in visitedIndices', () => {
    const conns = [importingConn(0, 1, 1, 0)];
    const result = computeDmpBfsContext(0, new Set([2]), conns, sections);

    expect(result.visitedIndices.has(2)).toBe(false);
    expect(result.visitedIndices.has(0)).toBe(true);
    expect(result.visitedIndices.has(1)).toBe(true);
  });
});

// ─── discoverBridgedDmpSections ─────────────────────────────────────────────

describe('discoverBridgedDmpSections', () => {
  // Section 0: heading north, length 10 → station 1 at (0, 10)
  // Section 1: heading east, length 5 → station 1 at (5, 0)
  // Section 2: heading south, length 8 → station 1 at (0, -8)
  // Section 3: heading east, length 3 → station 1 at (3, 0)
  const sections: DmpSection[] = [
    makeDmpSection('A', [makeShot(0, 10)]),
    makeDmpSection('B', [makeShot(90, 5)]),
    makeDmpSection('C', [makeShot(180, 8)]),
    makeDmpSection('D', [makeShot(90, 3)]),
  ];

  // Existing section X: station 0 at origin, station 1 at (10, 0)
  // shot_lines: [[[0, 0], [10, 0]]]
  const existingXGeo: ExistingSectionGeoEntry = {
    shotLines: [[[0, 0], [10, 0]]],
    frameOffsetX: 0,
    frameOffsetY: 0,
    side: 'target',
  };

  it('returns empty when no connections match', () => {
    const result = discoverBridgedDmpSections([], new Set([0]), new Map(), sections);

    expect(result.importSide).toEqual([]);
    expect(result.targetSide).toEqual([]);
  });

  it('returns empty when all connected sections are already shown', () => {
    const conns = [existingConn(1, 0, 42, 0)];
    const geoMap = new Map<number, ExistingSectionGeoEntry>([[42, existingXGeo]]);

    // Section 1 already shown
    const result = discoverBridgedDmpSections(conns, new Set([0, 1]), geoMap, sections);
    expect(result.targetSide).toEqual([]);
  });

  // Regression: DMP sections A and B both connect to existing X.
  // When previewing C→X, A and B should appear as context.
  it('discovers DMP sections connected through shared existing section', () => {
    const conns = [
      existingConn(0, 0, 42, 0),  // Section A → existing X at station 0
      existingConn(1, 0, 42, 1),  // Section B → existing X at station 1
    ];
    const geoMap = new Map<number, ExistingSectionGeoEntry>([[42, existingXGeo]]);

    // Current import section (2) is shown, A and B are not
    const result = discoverBridgedDmpSections(conns, new Set([2]), geoMap, sections);

    // Both A and B should be discovered on the target side (existingXGeo.side = 'target')
    expect(result.targetSide).toHaveLength(2);
    expect(result.discoveredIndices).toContain(0);
    expect(result.discoveredIndices).toContain(1);
  });

  it('positions bridged section correctly relative to existing merge station', () => {
    // Section A (index 0) connects to existing X at:
    //   A's station 0 → existing X's station 0 (which is at (0, 0) in X's frame)
    const conns = [existingConn(0, 0, 42, 0)];
    const geoMap = new Map<number, ExistingSectionGeoEntry>([[42, existingXGeo]]);

    const result = discoverBridgedDmpSections(conns, new Set([2]), geoMap, sections);

    expect(result.targetSide).toHaveLength(1);
    const ctx = result.targetSide[0]!;

    // Section A: heading 0°, length 10. Station 0 at origin, station 1 at (0, 10).
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

  it('positions bridged section with non-zero merge station', () => {
    // Section B (index 1) connects to existing X at:
    //   B's station 0 → existing X's station 1 (at (10, 0) in X's frame)
    const conns = [existingConn(1, 0, 42, 1)];
    const geoMap = new Map<number, ExistingSectionGeoEntry>([[42, existingXGeo]]);

    const result = discoverBridgedDmpSections(conns, new Set([2]), geoMap, sections);

    expect(result.targetSide).toHaveLength(1);
    const ctx = result.targetSide[0]!;

    // Section B: heading 90°, length 5. Station 0 at (0,0), station 1 at (5, 0).
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
    const geoWithOffset: ExistingSectionGeoEntry = {
      shotLines: [[[0, 0], [10, 0]]],
      frameOffsetX: 100,
      frameOffsetY: 200,
      side: 'import',
    };
    const conns = [existingConn(0, 0, 42, 0)];
    const geoMap = new Map<number, ExistingSectionGeoEntry>([[42, geoWithOffset]]);

    const result = discoverBridgedDmpSections(conns, new Set([2]), geoMap, sections);

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
    const targetGeo: ExistingSectionGeoEntry = { ...existingXGeo, side: 'target' };
    const importGeo: ExistingSectionGeoEntry = {
      shotLines: [[[0, 0], [5, 0]]],
      frameOffsetX: 0, frameOffsetY: 0, side: 'import',
    };

    const conns = [
      existingConn(0, 0, 42, 0),  // connects to target-side existing
      existingConn(1, 0, 99, 0),  // connects to import-side existing
    ];
    const geoMap = new Map<number, ExistingSectionGeoEntry>([
      [42, targetGeo],
      [99, importGeo],
    ]);

    const result = discoverBridgedDmpSections(conns, new Set([2]), geoMap, sections);

    expect(result.targetSide.length).toBeGreaterThan(0);
    expect(result.importSide.length).toBeGreaterThan(0);
  });

  it('follows importing-type connections from bridged sections', () => {
    // Section A → existing X (bridge), Section A → Section D (importing)
    const conns = [
      existingConn(0, 0, 42, 0),
      importingConn(0, 1, 3, 0),
    ];
    const geoMap = new Map<number, ExistingSectionGeoEntry>([[42, existingXGeo]]);

    const result = discoverBridgedDmpSections(conns, new Set([2]), geoMap, sections);

    // A is discovered via bridge, D is discovered via BFS from A
    expect(result.discoveredIndices).toContain(0);
    expect(result.discoveredIndices).toContain(3);
    // Both A and D should have context geometry
    expect(result.targetSide.length).toBe(2);
  });

  it('does not duplicate sections already shown', () => {
    const conns = [existingConn(0, 0, 42, 0)];
    const geoMap = new Map<number, ExistingSectionGeoEntry>([[42, existingXGeo]]);

    // Section 0 already shown
    const result = discoverBridgedDmpSections(conns, new Set([0, 2]), geoMap, sections);

    expect(result.targetSide).toEqual([]);
    expect(result.importSide).toEqual([]);
  });

  it('skips connections to unknown existing sections', () => {
    // Section A connects to existing section 999, which is not in the geo map
    const conns = [existingConn(0, 0, 999, 0)];
    const geoMap = new Map<number, ExistingSectionGeoEntry>([[42, existingXGeo]]);

    const result = discoverBridgedDmpSections(conns, new Set([2]), geoMap, sections);
    expect(result.targetSide).toEqual([]);
  });

  it('uses correct color from SECTION_COLORS', () => {
    const conns = [existingConn(1, 0, 42, 0)];
    const geoMap = new Map<number, ExistingSectionGeoEntry>([[42, existingXGeo]]);

    const result = discoverBridgedDmpSections(conns, new Set([2]), geoMap, sections);
    expect(result.targetSide).toHaveLength(1);
    expect(result.targetSide[0]!.color).toBe(SECTION_COLORS[1 % SECTION_COLORS.length]);
  });
});

// ─── computeUnreachableSegments ──────────────────────────────────────────────

describe('computeUnreachableSegments', () => {
  it('returns empty result for empty input', () => {
    expect(computeUnreachableSegments([], [])).toEqual([]);
  });

  it('returns single isolated section as a segment of 1', () => {
    const result = computeUnreachableSegments([3], []);
    expect(result).toEqual([[3]]);
  });

  it('groups two connected sections into one segment', () => {
    const conns = [importingConn(1, 0, 2, 0)];
    const result = computeUnreachableSegments([1, 2], conns);
    expect(result).toEqual([[1, 2]]);
  });

  it('splits three sections into two segments when one is disconnected', () => {
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
  // Section 0: 1 shot heading north (0°), length 10 → station 1 at (0, 10)
  // Section 1: 1 shot heading east (90°), length 5 → station 1 at (5, 0)
  // Section 2: 1 shot heading south (180°), length 8 → station 1 at (0, -8)
  const sections: DmpSection[] = [
    makeDmpSection('A', [makeShot(0, 10)]),
    makeDmpSection('B', [makeShot(90, 5)]),
    makeDmpSection('C', [makeShot(180, 8)]),
  ];

  it('returns single entry at origin for segment of 1', () => {
    const result = computeSegmentLayout([0], [], sections);
    expect(result).toEqual([{ sectionIndex: 0, offsetX: 0, offsetY: 0 }]);
  });

  it('computes correct offset for two connected sections', () => {
    // Section 0 station 1 → Section 1 station 0
    // Section 0 station 1 is at (0, 10). Section 1 station 0 is at (0, 0).
    // Offset for section 1 = (0 + 0 - 0, 0 + 10 - 0) = (0, 10)
    const conns = [importingConn(0, 1, 1, 0)];
    const result = computeSegmentLayout([0, 1], conns, sections);

    expect(result).toHaveLength(2);
    expect(result[0]).toEqual({ sectionIndex: 0, offsetX: 0, offsetY: 0 });
    expect(result[1]!.sectionIndex).toBe(1);
    expect(result[1]!.offsetX).toBeCloseTo(0, 5);
    expect(result[1]!.offsetY).toBeCloseTo(10, 5);
  });

  it('chains through multiple connections', () => {
    // 0 → 1 → 2
    const conns = [
      importingConn(0, 1, 1, 0),
      importingConn(1, 1, 2, 0),
    ];
    const result = computeSegmentLayout([0, 1, 2], conns, sections);

    expect(result).toHaveLength(3);
    // Section 1 offset = (0, 10) (from section 0's station 1)
    expect(result[1]!.offsetX).toBeCloseTo(0, 5);
    expect(result[1]!.offsetY).toBeCloseTo(10, 5);
    // Section 2 offset: section 1's station 1 at (5, 0) + section 1's offset (0, 10) = (5, 10)
    // Section 2's station 0 at (0, 0), so offset = (5 - 0, 10 - 0) = (5, 10)
    expect(result[2]!.offsetX).toBeCloseTo(5, 5);
    expect(result[2]!.offsetY).toBeCloseTo(10, 5);
  });

  it('returns empty for empty input', () => {
    expect(computeSegmentLayout([], [], sections)).toEqual([]);
  });

  it('includes disconnected sections at origin', () => {
    // Section 0 and 1 are in the segment but have no connection between them
    const result = computeSegmentLayout([0, 1], [], sections);
    expect(result).toHaveLength(2);
    // Root (0) at origin, section 1 also at origin since no connection to offset it
    expect(result[0]).toEqual({ sectionIndex: 0, offsetX: 0, offsetY: 0 });
    expect(result[1]).toEqual({ sectionIndex: 1, offsetX: 0, offsetY: 0 });
  });
});
