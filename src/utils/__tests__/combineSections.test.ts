import { describe, it, expect } from 'vitest';
import { combinePreview, defaultCombinedName } from '../combineSections';
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
