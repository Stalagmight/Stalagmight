/**
 * Section-level pill fields keep the order they are showing: a person just
 * added goes at the end (beside the input), not wherever the shots' aggregate
 * sorts them.
 */
import { describe, expect, it } from 'vitest';
import { renderHook } from '@testing-library/react';
import { stablePeopleOrder, useStablePeopleOrder } from '../useStablePeopleOrder';

const e = (name: string) => ({ person: { name } });
const names = (list: Array<{ person: { name: string } }>) => list.map((x) => x.person.name);

describe('stablePeopleOrder', () => {
  it('starts from the entries own order', () => {
    expect(names(stablePeopleOrder([e('Ann'), e('Bo')], []).ordered)).toEqual(['Ann', 'Bo']);
  });

  it('keeps known people in place and appends new ones', () => {
    // The aggregate now lists Cy second (first appearance), but Cy is new.
    const { ordered } = stablePeopleOrder([e('Ann'), e('Cy'), e('Bo')], stablePeopleOrder([e('Ann'), e('Bo')], []).keys);
    expect(names(ordered)).toEqual(['Ann', 'Bo', 'Cy']);
  });

  it('forgets a person who went away, so adding them again appends them', () => {
    const shown = stablePeopleOrder([e('Ann'), e('Bo'), e('Cy')], []).keys;
    const removed = stablePeopleOrder([e('Bo'), e('Cy')], shown).keys;
    expect(names(stablePeopleOrder([e('Ann'), e('Bo'), e('Cy')], removed).ordered)).toEqual(['Bo', 'Cy', 'Ann']);
  });

  it('matches people by identity, not spelling', () => {
    const shown = stablePeopleOrder([e('Ann'), e('Bo')], []).keys;
    expect(names(stablePeopleOrder([e('bo'), e('ANN')], shown).ordered)).toEqual(['ANN', 'bo']);
  });
});

describe('useStablePeopleOrder', () => {
  it('remembers the order for one scope and starts over for another', () => {
    const { result, rerender } = renderHook(({ list, scope }) => useStablePeopleOrder(list, scope), {
      initialProps: { list: [e('Ann'), e('Bo')], scope: 'a' as string },
    });
    rerender({ list: [e('Ann'), e('Cy'), e('Bo')], scope: 'a' });
    expect(names(result.current)).toEqual(['Ann', 'Bo', 'Cy']);
    rerender({ list: [e('Ann'), e('Cy'), e('Bo')], scope: 'b' });
    expect(names(result.current)).toEqual(['Ann', 'Cy', 'Bo']);
  });
});
