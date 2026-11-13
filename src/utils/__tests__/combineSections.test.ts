import { describe, it, expect } from 'vitest';
import { combinePreview, combinePreviewAll, defaultCombinedName, formatRotation } from '../combineSections';
import type { CombineCandidate } from '../../types/ipc/CombineCandidate';

describe('defaultCombinedName', () => {
  it('gives the two halves of a split their original name back, whichever is kept', () => {
    expect(defaultCombinedName('Main-A', 'Main-B')).toBe('Main');
    expect(defaultCombinedName('Main-B', 'Main-A')).toBe('Main');
  });

  it('keeps the kept section name otherwise', () => {
    expect(defaultCombinedName('Main', 'Side')).toBe('Main');
    expect(defaultCombinedName('Main-A', 'Other-B')).toBe('Main-A');
    expect(defaultCombinedName('Main-A', 'Main-A')).toBe('Main-A');
    expect(defaultCombinedName('-A', '-B')).toBe('-A');
  });
});

describe('combinePreview', () => {
  it('highlights both sections and rings both join stations', () => {
    const candidate = {
      section_a: 1,
      section_b: 4,
      station_a: { section_id: 1, station_index: 3 },
      station_b: { section_id: 4, station_index: 0 },
    } as CombineCandidate;
    expect(combinePreview(candidate)).toEqual({
      sections: [1, 4],
      stations: [
        { section_id: 1, station_index: 3 },
        { section_id: 4, station_index: 0 },
      ],
    });
  });
});

describe('combinePreviewAll', () => {
  it('also rings the stations of every tie the combine turns into a loop closure', () => {
    const plan = (kept: number, a: [number, number], b: [number, number]) => ({
      kept,
      torn_ties: [
        {
          station_a: { section_id: a[0], station_index: a[1] },
          station_b: { section_id: b[0], station_index: b[1] },
          kind: 'merge',
        },
      ],
    });
    const candidate = {
      section_a: 1,
      section_b: 4,
      station_a: { section_id: 1, station_index: 3 },
      station_b: { section_id: 4, station_index: 0 },
      plan_keeping_a: plan(1, [4, 2], [7, 1]),
      plan_keeping_b: plan(4, [1, 0], [7, 0]),
    } as unknown as CombineCandidate;
    expect(combinePreviewAll([candidate]).stations).toEqual([
      { section_id: 1, station_index: 3 },
      { section_id: 4, station_index: 0 },
      { section_id: 4, station_index: 2 },
      { section_id: 7, station_index: 1 },
      { section_id: 1, station_index: 0 },
      { section_id: 7, station_index: 0 },
    ]);
  });
});

describe('formatRotation', () => {
  it('signs the angle and keeps one decimal', () => {
    expect(formatRotation(5)).toBe('+5.0°');
    expect(formatRotation(-0.34)).toBe('-0.3°');
    expect(formatRotation(0)).toBe('0.0°');
  });
});
