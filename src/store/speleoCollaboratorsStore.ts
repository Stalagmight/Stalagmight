/**
 * The SpeleoDB collaborators of the OPEN, LINKED project, as people the credit
 * fields suggest (matched on name and email). Fetched in the background once
 * per (server, account, project) when a linked project is open and the user is
 * signed in; never for a local project, never while signed out.
 *
 * Nothing waits on it: a field renders its other suggestions at once and gains
 * the collaborators when they arrive. A failed fetch is silent (one debug log)
 * and is tried again after {@link RETRY_AFTER_MS} while the project stays
 * open - the suggestions are a convenience, and a dead server already has its
 * own surfaces, so never a tight retry loop. Signing out forgets everything
 * ({@link CollaboratorsState.reset}), and a reply to a request made before
 * that is dropped instead of filling the cache again. The backend keeps its
 * own short-lived cache; this one only saves the round trip per session.
 */
import { useEffect } from 'react';
import { create } from 'zustand';
import { useSpeleoStore } from './speleoStore';
import { invokeIpc } from '../utils/ipc';
import { debugLog } from '../utils/debugLog';
import { dedupePeople } from '../utils/people';
import type { Person, SpeleoCollaborator } from '../types/ipc.generated';

type Entry =
  | { status: 'loading' }
  | { status: 'ready'; people: Person[] }
  /** `at`: when it failed (ms since the epoch), for the back-off. */
  | { status: 'failed'; at: number };

/** How long a failed fetch waits before it is tried again. */
export const RETRY_AFTER_MS = 60_000;

interface CollaboratorsState {
  /** Keyed by {@link collaboratorsKey}. */
  entries: Record<string, Entry>;
  /** Bumped by every {@link reset}: a reply to an older request is dropped. */
  generation: number;
  /**
   * Fetch the collaborators for `key` unless already fetched or in flight, or
   * failed less than {@link RETRY_AFTER_MS} ago.
   */
  ensure: (key: string, projectId: string) => void;
  /** Forget everything, and every reply still on its way (sign-out, tests). */
  reset: () => void;
}

const EMPTY: readonly Person[] = Object.freeze([]) as readonly Person[];

/** One cache slot per server, account and project. */
export function collaboratorsKey(endpoint: string | null, email: string | null, projectId: string): string {
  return `${endpoint ?? ''}\u0000${(email ?? '').toLowerCase()}\u0000${projectId}`;
}

/**
 * A collaborator as a person. The server may not know a name; the email's
 * local part (before the `@`) then stands in for it, so the pill still says
 * who it is. Never the whole address: a person's NAME is what reaches the
 * legacy `sections.surveyor`, the Ariane export and the JSON export, which
 * carry no emails. The email itself stays on the person.
 */
export function collaboratorPerson(c: SpeleoCollaborator): Person {
  const name = (c.name ?? '').trim();
  const email = c.email.trim();
  const at = email.lastIndexOf('@');
  const local = (at > 0 ? email.slice(0, at) : email).trim();
  return { name: name !== '' ? name : local !== '' ? local : email, email };
}

export const useSpeleoCollaboratorsStore = create<CollaboratorsState>((set, get) => ({
  entries: {},
  generation: 0,
  ensure: (key, projectId) => {
    const entry = get().entries[key];
    if (entry && (entry.status !== 'failed' || Date.now() - entry.at < RETRY_AFTER_MS)) return;
    const generation = get().generation;
    // A reply to a request made before the last reset (a sign-out) is dropped.
    const current = () => get().generation === generation;
    set((s) => ({ entries: { ...s.entries, [key]: { status: 'loading' } } }));
    invokeIpc<SpeleoCollaborator[]>('speleodb_project_collaborators', { projectId }, {
      purpose: 'suggest the linked project collaborators as surveyors',
      level: 'debug',
    })
      .then((list) => {
        if (!current()) return;
        const people = dedupePeople((Array.isArray(list) ? list : []).filter((c) => !!c?.email).map(collaboratorPerson));
        set((s) => ({ entries: { ...s.entries, [key]: { status: 'ready', people } } }));
      })
      .catch((e: unknown) => {
        debugLog('[collaborators] speleodb_project_collaborators failed:', e);
        if (!current()) return;
        set((s) => ({ entries: { ...s.entries, [key]: { status: 'failed', at: Date.now() } } }));
      });
  },
  reset: () => set((s) => ({ entries: {}, generation: s.generation + 1 })),
}));

/** The cache key of the open linked project, or null when there is none to ask about. */
function useLinkedKey(): { key: string; projectId: string } | null {
  const projectId = useSpeleoStore((s) => (s.projectLinkKind === 'remote' ? s.remoteProjectId : null));
  const loggedIn = useSpeleoStore((s) => s.session?.loggedIn === true);
  const endpoint = useSpeleoStore((s) => s.session?.endpoint ?? null);
  const email = useSpeleoStore((s) => s.session?.userEmail ?? null);
  if (!projectId || !loggedIn) return null;
  return { key: collaboratorsKey(endpoint, email, projectId), projectId };
}

/**
 * Mount once for the open project: starts the background fetch whenever a
 * linked project is open with a signed-in account, tries a failed one again
 * after {@link RETRY_AFTER_MS}, and forgets every collaborator on sign-out.
 */
export function useCollaboratorsSync(): void {
  const linked = useLinkedKey();
  const ensure = useSpeleoCollaboratorsStore((s) => s.ensure);
  const reset = useSpeleoCollaboratorsStore((s) => s.reset);
  const loggedIn = useSpeleoStore((s) => s.session?.loggedIn === true);
  const key = linked?.key ?? null;
  const projectId = linked?.projectId ?? null;
  const failedAt = useSpeleoCollaboratorsStore((s) => {
    const entry = key !== null ? s.entries[key] : undefined;
    return entry?.status === 'failed' ? entry.at : null;
  });
  useEffect(() => {
    if (!loggedIn) reset();
  }, [loggedIn, reset]);
  useEffect(() => {
    if (key === null || projectId === null) return undefined;
    if (failedAt === null) {
      ensure(key, projectId);
      return undefined;
    }
    const timer = setTimeout(() => ensure(key, projectId), Math.max(0, failedAt + RETRY_AFTER_MS - Date.now()));
    return () => clearTimeout(timer);
  }, [key, projectId, failedAt, ensure]);
}

/** The open linked project's collaborators; empty until fetched, or for a local project (hook). */
export function useLinkedCollaborators(): readonly Person[] {
  const linked = useLinkedKey();
  return useSpeleoCollaboratorsStore((s) => {
    const entry = linked ? s.entries[linked.key] : undefined;
    return entry?.status === 'ready' ? entry.people : EMPTY;
  });
}
