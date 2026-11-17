import { describe, expect, it } from 'vitest';
import type { CrossLineSuggestion, DmpSection, DmpShot } from '../../types/commands';
import type { VirtualConnection } from '../segmentContext';
import {
  buildTemporalPairs,
  deriveDmpTiming,
  rerankSuggestionsByTemporalPlan,
} from '../dmpTemporalRerank';

function makeShot(length: number, hour: number, minute: number, second = 0): DmpShot {
  return {
    shot_type: 'Standard',
    heading_in: 90,
    heading_out: 90,
    length,
    depth_in: 0,
    depth_out: 0,
    pitch_in: 0,
    pitch_out: 0,
    left: 0,
    right: 0,
    up: 0,
    down: 0,
    temperature: 0,
    hour,
    minute,
    second,
    marker_index: 0,
  };
}

function makeLine(name: string, date: string, shots: DmpShot[]): DmpSection {
  return {
    version: 5,
    name,
    direction: 'in',
    date,
    shots,
    content_hash: name,
  };
}

function suggestion(overrides: Partial<CrossLineSuggestion>): CrossLineSuggestion {
  return {
    existingStationIndex: 0,
    importStationIndex: 0,
    score: 1,
    matchedShots: 2,
    direction: 'forward',
    isPartial: false,
    confidence: 'High',
    existingLineName: 'target',
    importLineName: 'import',
    importLineIndex: 0,
    targetType: 'importing',
    existingLineId: null,
    targetImportIndex: 1,
    ...overrides,
  } as CrossLineSuggestion;
}

function localParts(ms: number) {
  const d = new Date(ms);
  return {
    year: d.getFullYear(),
    month: d.getMonth() + 1,
    day: d.getDate(),
    hour: d.getHours(),
    minute: d.getMinutes(),
    second: d.getSeconds(),
  };
}

describe('deriveDmpTiming', () => {
  it('uses first and last Standard shot times', () => {
    const line = makeLine('A', '2026-06-11T10:00:00', [
      makeShot(10, 10, 1, 15),
      makeShot(10, 10, 5, 45),
    ]);

    const timing = deriveDmpTiming(line, 3);

    expect(timing?.lineIndex).toBe(3);
    expect(localParts(timing!.startedAtMs)).toMatchObject({ hour: 10, minute: 1, second: 15 });
    expect(localParts(timing!.endedAtMs)).toMatchObject({ hour: 10, minute: 5, second: 45 });
    expect(timing?.endStationIndex).toBe(2);
  });

  it('disables timing when any Standard shot has zero time', () => {
    const line = makeLine('A', '2026-06-11T10:00:00', [
      makeShot(10, 0, 0, 0),
      makeShot(10, 10, 5, 45),
    ]);

    expect(deriveDmpTiming(line, 0)).toBeNull();
  });

  it('rolls shot times forward across midnight', () => {
    const line = makeLine('A', '2026-06-11T23:50:00', [
      makeShot(10, 23, 59, 30),
      makeShot(10, 0, 2, 0),
    ]);

    const timing = deriveDmpTiming(line, 0);

    expect(localParts(timing!.endedAtMs)).toMatchObject({
      year: 2026,
      month: 6,
      day: 12,
      hour: 0,
      minute: 2,
      second: 0,
    });
  });
});

describe('buildTemporalPairs', () => {
  it('pairs only chronological neighbors inside the 8 hour cluster', () => {
    const lines = [
      makeLine('A', '2026-06-11T10:00:00', [makeShot(10, 10, 0), makeShot(10, 10, 5)]),
      makeLine('B', '2026-06-11T10:15:00', [makeShot(10, 10, 15), makeShot(10, 10, 20)]),
      makeLine('C', '2026-06-11T20:00:00', [makeShot(10, 20, 0), makeShot(10, 20, 5)]),
    ];

    expect(buildTemporalPairs(lines)).toEqual([
      { fromLineIndex: 0, toLineIndex: 1, minutes: 10 },
    ]);
  });
});

describe('rerankSuggestionsByTemporalPlan', () => {
  const lines = [
    makeLine('A', '2026-06-11T10:00:00', [makeShot(100, 10, 0), makeShot(100, 10, 5)]),
    makeLine('B', '2026-06-11T10:10:00', [makeShot(10, 10, 10), makeShot(10, 10, 15)]),
    makeLine('C', '2026-06-11T10:20:00', [makeShot(10, 10, 20), makeShot(10, 10, 25)]),
  ];

  it('rejects a candidate that requires impossible travel speed', () => {
    const bad = suggestion({
      importLineIndex: 1,
      targetImportIndex: 0,
      importStationIndex: 0,
      existingStationIndex: 0,
      matchedShots: 1,
      isPartial: true,
      score: 2,
    });

    const result = rerankSuggestionsByTemporalPlan([bad], lines, []);

    expect(result.suggestions).toEqual([]);
    expect(result.rejectedCount).toBeGreaterThan(0);
  });

  it('keeps and prefers a lower geometry score when it makes the temporal plan feasible', () => {
    const bad = suggestion({
      importLineIndex: 1,
      targetImportIndex: 0,
      importStationIndex: 0,
      existingStationIndex: 0,
      matchedShots: 1,
      isPartial: true,
      score: 2,
    });
    const good = suggestion({
      importLineIndex: 1,
      targetImportIndex: 0,
      importStationIndex: 0,
      existingStationIndex: 2,
      matchedShots: 1,
      isPartial: true,
      score: 1,
    });

    const result = rerankSuggestionsByTemporalPlan([bad, good], lines, []);

    expect(result.suggestions[0]).toBe(good);
    expect(result.suggestions).not.toContain(bad);
  });

  it('reranks around fixed user choices', () => {
    const fixed: VirtualConnection[] = [{
      importLineIndex: 1,
      importStationIndex: 0,
      targetType: 'importing',
      targetImportIndex: 0,
      targetImportStationIndex: 2,
    }];
    const goodNext = suggestion({
      importLineIndex: 2,
      targetImportIndex: 1,
      importStationIndex: 0,
      existingStationIndex: 2,
      matchedShots: 1,
      isPartial: true,
      score: 1,
    });
    const badNext = suggestion({
      importLineIndex: 2,
      targetImportIndex: 0,
      importStationIndex: 0,
      existingStationIndex: 0,
      matchedShots: 1,
      isPartial: true,
      score: 2,
    });

    const result = rerankSuggestionsByTemporalPlan([badNext, goodNext], lines, fixed);

    expect(result.suggestions[0]).toBe(goodNext);
    expect(result.suggestions).not.toContain(badNext);
  });

  it('evaluates candidates at the default merge station, not the raw backend anchor', () => {
    const localLines = [
      makeLine('A', '2026-06-11T10:00:00', [makeShot(100, 10, 0), makeShot(100, 10, 5)]),
      makeLine('B', '2026-06-11T10:10:00', [makeShot(100, 10, 10), makeShot(100, 10, 15)]),
    ];
    const candidate = suggestion({
      importLineIndex: 1,
      targetImportIndex: 0,
      importStationIndex: 0,
      existingStationIndex: 2,
      matchedShots: 2,
      direction: 'reverse',
      score: 2,
    });

    const result = rerankSuggestionsByTemporalPlan([candidate], localLines, []);

    expect(result.suggestions).toEqual([]);
    expect(result.rejectedCount).toBeGreaterThan(0);
  });

  it('honors the preferred one-to-one merge offset', () => {
    const localLines = [
      makeLine('A', '2026-06-11T10:00:00', [makeShot(10, 10, 0), makeShot(10, 10, 5)]),
      makeLine('B', '2026-06-11T10:15:00', [makeShot(100, 10, 15), makeShot(100, 10, 20)]),
    ];
    const candidate = suggestion({
      importLineIndex: 0,
      targetImportIndex: 1,
      importStationIndex: 0,
      existingStationIndex: 0,
      matchedShots: 1,
      preferredMergeOffset: 0,
      score: 2,
    });

    const result = rerankSuggestionsByTemporalPlan([candidate], localLines, []);

    expect(result.suggestions).toEqual([candidate]);
    expect(result.rejectedCount).toBe(0);
  });

  it('keeps a feasible High match ahead of a Low import-chain link the plan would open with (#101)', () => {
    // Mirrors the AC3 regression: C has a strong existing-line match (High) and
    // a weak import-to-import link to its predecessor B (Low). The chain link
    // "explains" the B→C temporal adjacency, so the planner scores it higher on
    // checkedPairs and would surface it first - but a feasible High must win.
    const localLines = [
      makeLine('B', '2026-06-11T10:00:00', [makeShot(10, 10, 0), makeShot(10, 10, 5)]),
      makeLine('C', '2026-06-11T10:10:00', [makeShot(10, 10, 10), makeShot(10, 10, 15)]),
    ];
    const highExisting = suggestion({
      importLineIndex: 1,
      targetType: 'existing',
      existingLineId: 100,
      targetImportIndex: null,
      existingStationIndex: 0,
      importStationIndex: 0,
      matchedShots: 1,
      score: 1.2,
      confidence: 'high',
      existingLineName: 'Mainline 2',
    });
    const lowChain = suggestion({
      importLineIndex: 1,
      targetType: 'importing',
      targetImportIndex: 0,
      existingLineId: null,
      existingStationIndex: 0,
      importStationIndex: 0,
      matchedShots: 1,
      score: 0.6,
      confidence: 'low',
      existingLineName: 'B',
    });

    const result = rerankSuggestionsByTemporalPlan([highExisting, lowChain], localLines, []);

    // Both feasible → both survive as options...
    expect(result.suggestions).toContain(highExisting);
    expect(result.suggestions).toContain(lowChain);
    // ...but the High existing-line match must be the primary, not the Low link.
    expect(result.suggestions[0]).toBe(highExisting);
  });

  it('orders across import lines by the temporal plan, not by confidence tier (#101 / review)', () => {
    // The confidence guard must be scoped to a line's own alternatives, not a
    // global sort - otherwise an unrelated high-confidence line is pulled ahead
    // of a link the plan chose to satisfy a temporal pair. Confidence is
    // intentionally decoupled from score here to isolate the cross-line
    // property: P's candidate is the plan's lead (higher score) but labelled
    // 'low'; Q's is 'high' but weaker. A line-scoped fix keeps P first; a
    // global confidence sort would wrongly surface Q's 'high'.
    const localLines = [
      makeLine('P', '2026-06-11T10:00:00', [makeShot(10, 10, 0), makeShot(10, 10, 5)]),
      makeLine('Q', '2026-06-11T10:10:00', [makeShot(10, 10, 10), makeShot(10, 10, 15)]),
    ];
    const pLeadLow = suggestion({
      importLineIndex: 0,
      targetType: 'existing',
      existingLineId: 200,
      targetImportIndex: null,
      existingStationIndex: 0,
      importStationIndex: 0,
      matchedShots: 1,
      score: 1.5,
      confidence: 'low',
      existingLineName: 'P-target',
    });
    const qHighWeak = suggestion({
      importLineIndex: 1,
      targetType: 'existing',
      existingLineId: 201,
      targetImportIndex: null,
      existingStationIndex: 0,
      importStationIndex: 0,
      matchedShots: 1,
      score: 0.5,
      confidence: 'high',
      existingLineName: 'Q-target',
    });

    const result = rerankSuggestionsByTemporalPlan([pLeadLow, qHighWeak], localLines, []);

    // Cross-line order follows the plan (P leads), not confidence tier.
    expect(result.suggestions[0]).toBe(pLeadLow);
  });
});
