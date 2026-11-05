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

/** The current project session; compare two reads to tell whether a switch happened. */
export function projectSessionEpoch(): number {
  return epoch;
}

/** Called where the open project is replaced (load / create commit, close, delete). */
export function noteProjectSessionChanged(): void {
  epoch += 1;
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
