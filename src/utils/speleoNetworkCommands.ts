/**
 * Which IPC commands talk to the SpeleoDB server, and how.
 *
 * - `read`: a GET that changes nothing on the server or in the session (project
 *   list, link status, revisions, metadata, the access preflight).
 * - `exclusive`: takes, releases, uploads, downloads or replaces the signed-in
 *   account. Two of these must not be started side by side, and the SpeleoDB
 *   endpoint or account must not change while one runs, so the UI disables the
 *   conflicting controls while any of them is in flight.
 *
 * `exclusive` is also exactly the set of commands the backend's exit waits for
 * (`commands/exit_wait.rs`): the quit gate (`utils/quitAfterWork`) counts these
 * as work in flight, and a command that held the backend's exit without being
 * counted here let a quit destroy the window while the process lived on
 * windowless until it finished. A Rust test there pins the two sets equal, both
 * ways.
 *
 * Commands missing from this table are purely local (`speleodb_session`,
 * `speleodb_local_project_states`, `speleodb_remove_link_at`), never hold the
 * exit, and are not counted. `invokeIpc` consults this table for every call, so
 * the busy indicator, the UI gating and the quit gate cover every caller without
 * each one opting in.
 */
export type SpeleoNetworkKind = 'read' | 'exclusive';

const KINDS: Readonly<Record<string, SpeleoNetworkKind>> = {
  speleodb_list_projects: 'read',
  speleodb_link_status: 'read',
  speleodb_get_revisions: 'read',
  speleodb_unwind_target: 'read',
  speleodb_rewind_target: 'read',
  speleodb_get_project_metadata: 'read',
  speleodb_project_collaborators: 'read',
  speleodb_project_access_at: 'read',
  speleodb_login: 'exclusive',
  speleodb_logout: 'exclusive',
  speleodb_open_project: 'exclusive',
  speleodb_open_project_discarding_local: 'exclusive',
  speleodb_start_editing: 'exclusive',
  speleodb_done: 'exclusive',
  speleodb_push: 'exclusive',
  speleodb_update: 'exclusive',
  speleodb_take_remote: 'exclusive',
  speleodb_discard_changes: 'exclusive',
  speleodb_restore_revision: 'exclusive',
  speleodb_release_lock: 'exclusive',
  speleodb_release_lock_acquired_here: 'exclusive',
  speleodb_release_current_lock: 'exclusive',
  // A project switch / quit hands the open project's lock back before leaving it.
  speleodb_settle_open_lock: 'exclusive',
  speleodb_unlink: 'exclusive',
  speleodb_publish: 'exclusive',
  // Removing a cached copy (one, or the purge) hands back this account's lock on
  // the project afterwards, when the server says the account holds it.
  speleodb_remove_cached_copy: 'exclusive',
  speleodb_purge_cached_copies: 'exclusive',
  // Not SpeleoDB calls of their own, but the backend's exit waits for both, so the
  // quit gate counts them: closing a project hands its lock back first (or keeps
  // it, or discards the copy, per the exit prompt's answer), and sign-out's
  // Discard removes a cached copy before the account goes.
  close_project: 'exclusive',
  speleodb_discard_cached_copy: 'exclusive',
};

/** The network kind of one IPC call, or `null` when it never reaches SpeleoDB. */
export function speleoNetworkKind(
  command: string,
  args?: Record<string, unknown>,
): SpeleoNetworkKind | null {
  // The access preflight with `localOnly` reads only the local file.
  if (command === 'speleodb_project_access_at' && args?.localOnly === true) return null;
  return Object.prototype.hasOwnProperty.call(KINDS, command) ? (KINDS[command] ?? null) : null;
}
