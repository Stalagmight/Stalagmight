/**
 * Which project a request was ISSUED against, and whether that project is still the
 * open one (#24, [#576 FZ-FZ-INT-2]): the identity `projectStore` captures for every
 * edit and save, shared with the callers that keep a run of requests aimed at one
 * project (the DMP wizard, [#576 RVF-a-1]) so they decide by the same rule.
 *
 * Pure functions of the open project (what the store holds) and the session counter
 * (`projectSession.ts`); importing no store, so the store and its callers can both
 * use it without an import cycle.
 */
import { projectSessionEpoch } from './projectSession';
import type { ProjectIdentity } from '../types/ipc/ProjectIdentity';

/** What a request was ISSUED against: the project's key, the file it was open from and the session. */
export type IssuedAgainst = { key: string | null; file: string | null; session: number };

/** The part of the open project the identity reads (`ProjectStore['project']`). */
type OpenProject = { created_at: string; file_path?: string | null } | null | undefined;

/**
 * Whether the project changed identity between issuing a command and its reply (#24).
 *
 * `created_at` alone: it is immutable for the life of a project, whereas `file_path` changes on
 * a first save or a Save As - keying on that made the guard reject replies that were perfectly
 * valid and silently leave the frontend's positional refs on the wrong physical station.
 *
 * A missing value on either side is NOT a switch. Before the first refresh the store may hold
 * no project at all, and a closed project resolves to no sections, which the transform builder
 * already reduces to a no-op. Only two DIFFERENT identities mean the reply belongs to a project
 * that is no longer open - and because section ids are small per-project integers, those entries
 * would otherwise match sections in whatever is now open.
 */
export function projectSwitched(before: string | null, after: string | null): boolean {
  return before !== null && after !== null && before !== after;
}

/** The identity of `open` (the store's open project) now. */
export function issuedAgainstOf(open: OpenProject): IssuedAgainst {
  return { key: open?.created_at ?? null, file: open?.file_path ?? null, session: projectSessionEpoch() };
}

/**
 * True when the project a request was ISSUED against is no longer `open` (#24).
 *
 * [#576 FZ-FZ-INT-2] Two copies of one project file share `created_at`, so an edit
 * issued in one copy while a switch to the other was under way ran in the other.
 * Once the session moved (another open committed) the file must match too. A Save
 * As moves the file within the session, and a reload of the same file opens the
 * project the edit was made in: neither counts - the display-setting chain's rule
 * (`utils/settingWriteChain.ts`).
 */
export function lostProjectOf(issued: IssuedAgainst, open: OpenProject): boolean {
  if (projectSwitched(issued.key, open?.created_at ?? null)) return true;
  return (
    issued.key !== null &&
    open != null &&
    issued.session !== projectSessionEpoch() &&
    issued.file !== (open.file_path ?? null)
  );
}

/**
 * [#576 FZ-FZ-INT-4, FZ-FZ-INT-7] The `expectedProject` a save or an edit IPC carries:
 * the project it was issued against, which the backend checks under its command lock
 * (`core/state/expected_project.rs`) and refuses with PROJECT_CHANGED when another is
 * open. Null - "made with no project open" - when none was.
 */
export function expectedProjectOf(issued: IssuedAgainst): ProjectIdentity | null {
  return issued.key === null ? null : { createdAt: issued.key, filePath: issued.file };
}
