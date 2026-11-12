import { describe, it, expect } from 'vitest';
import {
  adjustStationForDeletedShots,
  findReverseImportConnection,
  findOverlapForStation,
  redirectDeletedTargets,
  type SectionEditState,
  type ConfirmedOverlapInfo,
} from '../batchImportHelpers';
import type { VirtualConnection } from '../segmentContext';
import type { CrossSectionSuggestion } from '../../types/commands';

// ─── Test data helpers ──────────────────────────────────────────────────────

/** Minimal editable shot for testing. */
function makeShot(id: number, overrides: Partial<{
  isDeleted: boolean; isNew: boolean; is_reference: boolean;
}> = {}) {
  return {
    id,
    azimuth_in: 90, azimuth_out: 91, length: 4.0,
    depth_in: 8, depth_out: 10, pitch_in: -3, pitch_out: -4,
    left: 1.5, right: 2.5, up: 1.0, down: 0.8,
    comment: null, kind: 'Normal' as const,
    isNew: false, isDeleted: false,
    ...overrides,
  };
}

/** Create a minimal SectionEditState. */
function makeEdits(shots: ReturnType<typeof makeShot>[]): SectionEditState {
  return { name: 'S', date: '', color: '', direction: 'in', quality: 1.0, reverse: false, shots };
}

/** Create an importing-type VirtualConnection. */
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

/** Create a minimal CrossSectionSuggestion for the pool. */
function makeSuggestion(overrides: Partial<CrossSectionSuggestion>): CrossSectionSuggestion {
  return {
    importSectionIndex: 0,
    importStationIndex: 0,
    existingStationIndex: 0,
    matchedShots: 3,
    score: 100,
    direction: 'forward',
    isPartial: false,
    confidence: 'High',
    existingSectionName: 'target',
    importSectionName: 'import',
    targetType: 'importing',
    existingSectionId: null,
    targetImportIndex: null,
    ...overrides,
  } as CrossSectionSuggestion;
}

// ─── adjustStationForDeletedShots ────────────────────────────────────────────

describe('adjustStationForDeletedShots', () => {
  it('returns original station when no shots are deleted', () => {
    const shots = [makeShot(0), makeShot(1), makeShot(2), makeShot(3)];
    expect(adjustStationForDeletedShots(shots, 2)).toBe(2);
  });

  it('adjusts for deleted shots before the station', () => {
    const shots = [
      makeShot(0, { isDeleted: true }),
      makeShot(1),
      makeShot(2),
      makeShot(3),
    ];
    expect(adjustStationForDeletedShots(shots, 3)).toBe(2);
  });

  it('adjusts for deleted shots at the end (overlap region)', () => {
    const shots = [
      makeShot(0), makeShot(1), makeShot(2),
      makeShot(3, { isDeleted: true }),
      makeShot(4, { isDeleted: true }),
    ];
    // Station 5 (past all shots): 3 non-deleted before it
    expect(adjustStationForDeletedShots(shots, 5)).toBe(3);
  });
});

// ─── findReverseImportConnection ─────────────────────────────────────────────

describe('findReverseImportConnection', () => {
  it('returns undefined when no connection targets the section', () => {
    const conns = [importingConn(0, 5, 1, 0)]; // 0→1
    const emitted = new Map([[0, 0]]); // section 0 emitted
    expect(findReverseImportConnection(2, conns, emitted)).toBeUndefined();
  });

  it('returns undefined when the import side is not yet emitted', () => {
    const conns = [importingConn(0, 5, 1, 0)]; // 0→1
    const emitted = new Map<number, number>(); // nothing emitted
    expect(findReverseImportConnection(1, conns, emitted)).toBeUndefined();
  });

  it('returns a reversed connection when section is target and source is emitted', () => {
    // Connection A(0)→B(1): A at station 5, B at station 0
    const conns = [importingConn(0, 5, 1, 0)];
    const emitted = new Map([[0, 0]]); // A is emitted at position 0

    const result = findReverseImportConnection(1, conns, emitted);
    expect(result).toBeDefined();
    expect(result!.importSectionIndex).toBe(1);    // B becomes the import side
    expect(result!.importStationIndex).toBe(0);     // B's station (was target)
    expect(result!.targetType).toBe('importing');
    expect(result!.targetImportIndex).toBe(0);      // A becomes the target
    expect(result!.targetImportStationIndex).toBe(5); // A's station (was import)
  });

  it('handles chain A→B→C→D where A is emitted', () => {
    const conns = [
      importingConn(0, 10, 1, 0), // A→B
      importingConn(1, 5, 2, 0),  // B→C
      importingConn(2, 5, 3, 0),  // C→D
    ];

    // After A is emitted, B can use reverse of A→B
    const emittedAfterA = new Map([[0, 0]]);
    const reverseB = findReverseImportConnection(1, conns, emittedAfterA);
    expect(reverseB).toBeDefined();
    expect(reverseB!.targetImportIndex).toBe(0); // B→A

    // After A and B emitted, C can use reverse of B→C
    const emittedAfterAB = new Map([[0, 0], [1, 1]]);
    const reverseC = findReverseImportConnection(2, conns, emittedAfterAB);
    expect(reverseC).toBeDefined();
    expect(reverseC!.targetImportIndex).toBe(1); // C→B

    // After A, B, C emitted, D can use reverse of C→D
    const emittedAfterABC = new Map([[0, 0], [1, 1], [2, 2]]);
    const reverseD = findReverseImportConnection(3, conns, emittedAfterABC);
    expect(reverseD).toBeDefined();
    expect(reverseD!.targetImportIndex).toBe(2); // D→C
  });

  it('skips existing-type connections', () => {
    const conns: VirtualConnection[] = [{
      importSectionIndex: 0,
      importStationIndex: 5,
      targetType: 'existing',
      existingSectionId: 99,
      existingStationIndex: 3,
    }];
    const emitted = new Map([[0, 0]]);
    // Section 99 is not targetImportIndex, so no reverse
    expect(findReverseImportConnection(1, conns, emitted)).toBeUndefined();
  });
});

// ─── findOverlapForStation ───────────────────────────────────────────────────

describe('findOverlapForStation', () => {
  it('returns null when no overlap exists', () => {
    expect(findOverlapForStation(0, 5, new Map(), [])).toBeNull();
  });

  it('finds front overlap from confirmed overlaps', () => {
    const confirmed = new Map<number, ConfirmedOverlapInfo>([[0, {
      importStationIndex: 0,
      matchedShots: 3,
      direction: 'forward',
      existingStationIndex: 5,
      targetType: 'existing',
      existingSectionId: 99,
      targetImportIndex: null,
      mergeImportStation: 2,
    }]]);
    // Station 1 is in deleted zone (< mergeStation 2)
    expect(findOverlapForStation(0, 1, confirmed, [])).toBeDefined();
    // Station 3 is NOT in deleted zone (>= mergeStation 2)
    expect(findOverlapForStation(0, 3, confirmed, [])).toBeNull();
  });

  it('finds back overlap from suggestion pool', () => {
    const pool = [makeSuggestion({
      importSectionIndex: 0,
      importStationIndex: 10, // back-end overlap
      matchedShots: 3,
      targetType: 'importing',
      targetImportIndex: 1,
    })];
    // mergeStation = 10 - 3 = 7; station 8 > 7 → in deleted zone
    expect(findOverlapForStation(0, 8, new Map(), pool)).toBeDefined();
    // station 6 <= 7 → NOT in deleted zone
    expect(findOverlapForStation(0, 6, new Map(), pool)).toBeNull();
  });
});

// ─── redirectDeletedTargets ──────────────────────────────────────────────────

describe('redirectDeletedTargets', () => {
  it('does not redirect when target station is not in overlap zone', () => {
    const conn = importingConn(0, 5, 1, 3); // A→B at B's station 3
    const conns = [conn];
    // B has no overlap suggestions
    redirectDeletedTargets(conns, new Map(), new Map(), []);

    expect(conn.targetType).toBe('importing');
    expect(conn.targetImportIndex).toBe(1);
    expect(conn.targetImportStationIndex).toBe(3);
  });

  it('redirects when target station is in a truly deleted zone', () => {
    // Connection A(0) → B(1) at B's station 1
    const conn = importingConn(0, 5, 1, 1);
    const conns = [conn];

    // B's overlap: front, 3 matched shots → mergeStation=3
    // B is importSectionIndex, so B's overlap shots are on the import (deleted) side
    const pool = [makeSuggestion({
      importSectionIndex: 1,
      importStationIndex: 0,
      matchedShots: 3,
      direction: 'forward',
      existingStationIndex: 5,
      targetType: 'existing',
      existingSectionId: 99,
      targetImportIndex: null,
    })];

    // B's sectionEdits: shots 0,1,2 are deleted (actual deletion)
    const bEdits = makeEdits([
      makeShot(0, { isDeleted: true }),
      makeShot(1, { isDeleted: true }),
      makeShot(2, { isDeleted: true }),
      makeShot(3), makeShot(4), makeShot(5),
    ]);
    const edits = new Map<number, SectionEditState>([[1, bEdits]]);

    redirectDeletedTargets(conns, edits, new Map(), pool);

    // Should redirect to existing section 99
    expect(conn.targetType).toBe('existing');
    expect(conn.existingSectionId).toBe(99);
  });

  it('does NOT redirect when import-to-import overlap shots are interpolated, not deleted', () => {
    // Connection A(0) → B(1) at B's station 1
    const conn = importingConn(0, 5, 1, 1);
    const conns = [conn];

    // Pool has B→A suggestion (B is importSectionIndex) - but B's shots are
    // actually INTERPOLATED (not deleted) because A→B was processed first
    const pool = [makeSuggestion({
      importSectionIndex: 1,     // B is the "import" side of this suggestion
      importStationIndex: 0,     // front overlap
      matchedShots: 3,
      direction: 'forward',
      existingStationIndex: 5,
      targetType: 'importing',   // import-to-import overlap
      targetImportIndex: 0,      // target is A
    })];

    // B's sectionEdits: overlap shots 0,1,2 are ALIVE (interpolated, not deleted)
    const bEdits = makeEdits([
      makeShot(0), // interpolated but alive
      makeShot(1), // interpolated but alive
      makeShot(2), // interpolated but alive
      makeShot(3), makeShot(4), makeShot(5),
    ]);
    const edits = new Map<number, SectionEditState>([[1, bEdits]]);

    redirectDeletedTargets(conns, edits, new Map(), pool);

    // Should NOT redirect - B's overlap shots are interpolated, not deleted
    expect(conn.targetType).toBe('importing');
    expect(conn.targetImportIndex).toBe(1);
    expect(conn.targetImportStationIndex).toBe(1);
  });

  it('regression: bidirectional pool does not cause false redirect for interpolated target', () => {
    // Full scenario: A(0)→B(1) connection, pool has BOTH A→B and B→A suggestions.
    // A→B is processed first by applyAllOverlapTreatments:
    //   - A's overlap (front 3) shots are DELETED
    //   - B's overlap (front 3) shots are INTERPOLATED
    // redirectDeletedTargets should not redirect A→B because B's shots are not deleted.

    const conn = importingConn(0, 5, 1, 1); // A→B at B's station 1
    const conns = [conn];

    // Pool has both directions
    const pool = [
      makeSuggestion({
        importSectionIndex: 0,   // A→B: A is import side
        importStationIndex: 8,   // back overlap on A
        matchedShots: 3,
        targetType: 'importing',
        targetImportIndex: 1,
      }),
      makeSuggestion({
        importSectionIndex: 1,   // B→A: B is import side
        importStationIndex: 0,   // front overlap on B
        matchedShots: 3,
        targetType: 'importing',
        targetImportIndex: 0,
      }),
    ];

    // A's edits: overlap shots 8,9,10 are DELETED (A was the import side)
    const aEdits = makeEdits([
      makeShot(0), makeShot(1), makeShot(2), makeShot(3),
      makeShot(4), makeShot(5), makeShot(6), makeShot(7),
      makeShot(8, { isDeleted: true }),
      makeShot(9, { isDeleted: true }),
      makeShot(10, { isDeleted: true }),
    ]);
    // B's edits: overlap shots 0,1,2 are ALIVE (interpolated, B was the target side)
    const bEdits = makeEdits([
      makeShot(0), makeShot(1), makeShot(2), // interpolated but alive
      makeShot(3), makeShot(4), makeShot(5),
    ]);
    const edits = new Map<number, SectionEditState>([[0, aEdits], [1, bEdits]]);

    redirectDeletedTargets(conns, edits, new Map(), pool);

    // Connection A→B should be PRESERVED (not redirected)
    expect(conn.targetType).toBe('importing');
    expect(conn.targetImportIndex).toBe(1);
    expect(conn.targetImportStationIndex).toBe(1);
  });

  it('ignores reverse unconfirmed overlap when opposite direction is confirmed', () => {
    // Confirmed A→B exists. While redirecting a connection targeting B, the pool
    // may still contain reverse B→A suggestions. Those reverse candidates must
    // not be used for redirect, otherwise confirmed pairs can get detached.
    const conn = importingConn(2, 4, 1, 1); // C→B at B station 1
    const conns = [conn];

    const pool = [
      makeSuggestion({
        importSectionIndex: 1, // B→A (reverse of confirmed A→B)
        importStationIndex: 0,
        matchedShots: 3,
        direction: 'forward',
        targetType: 'importing',
        targetImportIndex: 0,
      }),
    ];

    // B looks like it has deleted overlap shots, which would normally trigger redirect.
    const bEdits = makeEdits([
      makeShot(0, { isDeleted: true }),
      makeShot(1, { isDeleted: true }),
      makeShot(2, { isDeleted: true }),
      makeShot(3), makeShot(4),
    ]);
    const edits = new Map<number, SectionEditState>([[1, bEdits]]);

    const confirmed = new Map<number, ConfirmedOverlapInfo>([
      [0, {
        importStationIndex: 0,
        matchedShots: 3,
        direction: 'forward',
        existingStationIndex: 0,
        targetType: 'importing',
        existingSectionId: null,
        targetImportIndex: 1,
        mergeImportStation: 3,
      }],
    ]);

    redirectDeletedTargets(conns, edits, confirmed, pool);

    // Keep original target B - no redirect through reverse unconfirmed candidate.
    expect(conn.targetType).toBe('importing');
    expect(conn.targetImportIndex).toBe(1);
    expect(conn.targetImportStationIndex).toBe(1);
  });
});
