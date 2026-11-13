/**
 * Recently entered people (surveyors, explorers), remembered across projects
 * so a name typed once is one keystroke away next time. Stored in config.toml
 * `[people] recent` through the partial `remember_people` command, which owns
 * the merge (newest first, one entry per identity, at most 200), so it never
 * reaches a survey's `.stalx` or SpeleoDB.
 *
 * `rememberPeople` publishes the merged list at once (the next field shows the
 * name immediately), coalesces the people of one gesture (a pasted list) into
 * one write, and replaces the optimistic list with what the backend stored. A
 * failed write keeps the optimistic list for this session and only logs: the
 * list is a convenience, never worth a notification.
 */
import { useProjectStore } from './projectStore';
import { invokeIpc } from '../utils/ipc';
import { debugLog } from '../utils/debugLog';
import { dedupePeople, isValidPerson, normalizeName, personKey, type PersonLike } from '../utils/people';
import type { Person } from '../types/ipc.generated';

/** `MAX_RECENT_PEOPLE` in `config/people.rs`. */
export const MAX_RECENT_PEOPLE = 200;

const EMPTY: readonly Person[] = Object.freeze([]) as readonly Person[];

/** The remembered people, newest first (hook). */
export function useRecentPeople(): readonly Person[] {
  return useProjectStore((s) => s.config?.people?.recent) ?? EMPTY;
}

/** As the backend stores a person: display-normalized name, trimmed email or none. */
export function toPerson(person: PersonLike): Person {
  const email = (person.email ?? '').trim();
  return email === '' ? { name: normalizeName(person.name) } : { name: normalizeName(person.name), email };
}

/** `remember_people`'s merge, for the optimistic publish. */
export function mergeRecent(current: readonly Person[], people: readonly Person[]): Person[] {
  const fresh = dedupePeople(people);
  const keys = new Set(fresh.map(personKey));
  return [...fresh, ...current.filter((p) => !keys.has(personKey(p)))].slice(0, MAX_RECENT_PEOPLE);
}

let pending: Person[] = [];
let flushQueued = false;
let writeSeq = 0;

function publish(recent: Person[]): void {
  const config = useProjectStore.getState().config;
  if (!config) return;
  useProjectStore.setState({ config: { ...config, people: { ...config.people, recent } } });
}

async function flush(): Promise<void> {
  flushQueued = false;
  const people = pending;
  pending = [];
  if (people.length === 0) return;
  const seq = ++writeSeq;
  try {
    const stored = await invokeIpc<Person[]>('remember_people', { people }, {
      purpose: 'remember recently entered people',
      level: 'debug',
    });
    // A newer write owns the list now; its own reply will publish it.
    if (seq === writeSeq && Array.isArray(stored)) publish(stored);
  } catch (e) {
    debugLog('[recentPeople] remember_people failed:', e);
  }
}

/**
 * Remember `people` as the most recently used, `people[0]` first (the backend's
 * order). A later call is newer and goes in front of an earlier one; several
 * calls in one task (a pasted list adds one pill per name) become one write.
 */
export function rememberPeople(people: readonly PersonLike[]): void {
  const valid = people.filter(isValidPerson).map(toPerson);
  if (valid.length === 0) return;
  pending = mergeRecent(pending, valid);
  const config = useProjectStore.getState().config;
  if (config) publish(mergeRecent(config.people?.recent ?? [], valid));
  if (flushQueued) return;
  flushQueued = true;
  queueMicrotask(() => void flush());
}
