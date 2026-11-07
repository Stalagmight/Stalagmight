/**
 * [#576 NF-FIX5-1] Which project session the frontend is in: a counter that moves
 * every time the open project is REPLACED - a load or a create commits its project,
 * a close or a delete of the open file clears it.
 *
 * An answer to a request issued in one session describes that session's project.
 * Applying it after the counter moved writes the OLD project over the new one, so
 * the async paths that outlive a switch capture the counter when they issue and
 * drop their answer when it moved (`refreshProjectState`). Unlike
 * `projectTreeGeneration`, a refresh does not move it: refreshes of one session are
 * ordered against each other by their own sequence guard.
 *
 * Its own module, importing nothing but a type, so the stores and utils can all
 * read it without an import cycle.
 */

import type { DisplaySettingTarget } from '../types/ipc.generated';

let epoch = 0;

/**
 * [#576 RVF-c-A2-2] The project each ended session ended on: its identity when the
 * session moved, null with none open. A Save As moves a session's project to another
 * file without a new session, so the file a session was OPENED from is not always the
 * one its project was last open from (`followSession`, `projectIssue.ts`).
 * [#576 W44-NF2] Kept for the most recent {@link ENDED_SESSIONS_KEPT} sessions only.
 */
const endedOn = new Map<number, DisplaySettingTarget | null>();

/**
 * [#576 W44-NF2] How many ended sessions `endedOn` keeps; the oldest goes first (the
 * map's insertion order is the session order). Its one reader is `followSession`, for
 * the DMP wizard's run, which rides `dialogData` across the project switches the user
 * makes while it is open: every session change is a load, create, close or delete the
 * user asked for, so a run would have to outlive 256 of them for its own session to
 * be gone. An entry is one identity (a `created_at` and a path: a few hundred bytes
 * for a path of ordinary length), so 256 stay well under a megabyte. An evicted
 * session reads as one that recorded nothing: `projectSessionEndedOn` answers
 * undefined and `followSession` gives the identity back unchanged (the rule before
 * #576 RVF-c-A2-2).
 */
export const ENDED_SESSIONS_KEPT = 256;

/** The current project session; compare two reads to tell whether a switch happened. */
export function projectSessionEpoch(): number {
  return epoch;
}

/**
 * Called where the open project is replaced (load / create commit, close, delete),
 * before the store drops the project it replaces.
 */
export function noteProjectSessionChanged(): void {
  endedOn.set(epoch, openProjectIdentity());
  for (const oldest of endedOn.keys()) {
    if (endedOn.size <= ENDED_SESSIONS_KEPT) break;
    endedOn.delete(oldest);
  }
  epoch += 1;
}

/**
 * The project `session` ended on (see `endedOn`); undefined while it is the current
 * one, and once it is older than the last {@link ENDED_SESSIONS_KEPT} ended sessions.
 */
export function projectSessionEndedOn(session: number): DisplaySettingTarget | null | undefined {
  return endedOn.get(session);
}

/**
 * [#576 FZ-FZ-INT-8] How many changes the store has acknowledged (marked the project
 * dirty for) so far. A save reads it when it is issued and clears `isDirty` only if
 * it has not moved since: a change that landed while the save was in flight may not
 * be in the file the save wrote, and clearing the flag over it hid it from the
 * autosave and the close prompt.
 */
let dirtyMarks = 0;

/** Called wherever the store sets `isDirty: true`. */
export function noteProjectDirtied(): void {
  dirtyMarks += 1;
}

export function projectDirtyMarks(): number {
  return dirtyMarks;
}

/**
 * [#576 RV6b-2] The open project's identity as the backend knows it, or null with
 * none open: what a per-project write names so the backend can refuse it once
 * another project is open. Its `created_at` (the key `projectSwitched` uses) and -
 * [#576 NF-W8FE-3] - the file it is open from, because two copies of one project
 * file share `created_at`. Late-bound like `speleoStoreHandle`: projectStore
 * registers its getter when it evaluates; before that (a unit test that never
 * loads it) there is no identity to give.
 */
let identity: (() => DisplaySettingTarget | null) | null = null;

export function registerOpenProjectIdentity(get: () => DisplaySettingTarget | null): void {
  identity = get;
}

export function openProjectIdentity(): DisplaySettingTarget | null {
  return identity ? identity() : null;
}
