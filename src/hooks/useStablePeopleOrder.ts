import { useMemo, useRef } from 'react';
import { personKey, type PersonLike } from '../utils/people';

/**
 * `entries` in the order this view has been showing them: people already on
 * screen keep their place, and people who appear go at the END - next to the
 * input where they were just typed - instead of wherever the aggregate's
 * first-appearance order puts them (a person added to every shot otherwise
 * jumps in front of anyone who first appears on a later shot). A person who
 * goes away is forgotten, so adding them again appends them too.
 *
 * Pure, so the hook is a thin memo over it.
 */
export function stablePeopleOrder<T extends { person: PersonLike }>(
  entries: readonly T[],
  previous: readonly string[],
): { ordered: T[]; keys: string[] } {
  const byKey = new Map(entries.map((entry) => [personKey(entry.person), entry]));
  const keys = previous.filter((key) => byKey.has(key));
  const known = new Set(keys);
  for (const key of byKey.keys()) {
    if (!known.has(key)) {
      known.add(key);
      keys.push(key);
    }
  }
  return { ordered: keys.map((key) => byKey.get(key)!), keys };
}

/**
 * {@link stablePeopleOrder} remembered for as long as `scope` stays the same
 * (an open dialog, one Detail panel selection); a new scope starts from the
 * entries' own order.
 */
export function useStablePeopleOrder<T extends { person: PersonLike }>(entries: readonly T[], scope: unknown): T[] {
  const memory = useRef<{ scope: unknown; keys: string[] }>({ scope, keys: [] });
  return useMemo(() => {
    const previous = memory.current.scope === scope ? memory.current.keys : [];
    const { ordered, keys } = stablePeopleOrder(entries, previous);
    memory.current = { scope, keys };
    return ordered;
  }, [entries, scope]);
}
