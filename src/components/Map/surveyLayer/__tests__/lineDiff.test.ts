/**
 * Line-set diff tests. Moved here with `diffRemovedLineIds` when it was
 * extracted from the deleted stock `labelRestore.ts`.
 */
import { describe, it, expect } from 'vitest';
import { diffRemovedLineIds } from '../lineDiff';

describe('diffRemovedLineIds', () => {
  it('returns line ids in prev but not in next, in prev order', () => {
    const prev = [{ id: 1 }, { id: 5 }, { id: 8 }, { id: 12 }];
    const next = [{ id: 1 }, { id: 8 }];
    expect(diffRemovedLineIds(prev, next)).toEqual([5, 12]);
  });

  it('returns [] when nothing was removed (edit / add only)', () => {
    const prev = [{ id: 1 }, { id: 2 }];
    const next = [{ id: 1 }, { id: 2 }, { id: 3 }];
    expect(diffRemovedLineIds(prev, next)).toEqual([]);
  });

  it('returns every id when the project emptied', () => {
    expect(diffRemovedLineIds([{ id: 4 }, { id: 9 }], [])).toEqual([4, 9]);
  });

  it('short-circuits an empty prev (app start / project open)', () => {
    expect(diffRemovedLineIds([], [{ id: 1 }])).toEqual([]);
  });
});
