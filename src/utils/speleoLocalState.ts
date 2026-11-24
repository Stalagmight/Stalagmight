import { invokeIpc } from './ipc';
import type { SpeleoLocalProjectState } from '../types/ipc.generated';

/**
 * "This work exists only on this computer" - the local half of the SpeleoDB
 * picture (#270).
 *
 * `has_unpushed_edits` is NOT `projectStore.isDirty`: it means "edits that are not
 * on the SpeleoDB server", it survives a save and a restart, and it is true for a
 * project nobody currently has open. The server-derived scans (the held-lock scan,
 * the GlobalView remote list) cannot see it, so the frontend joins it in from
 * `speleodb_local_project_states`.
 */
export type SpeleoLocalStateMap = Record<string, SpeleoLocalProjectState>;

/**
 * The accent colour for "there are changes here that SpeleoDB does not have".
 *
 * This is the SAME token the toolbar's Import icon uses (`--sh-comment`, the
 * syntactic-highlighting comment/amber role - see Toolbar.tsx and index.css), so the
 * app has one yellow for "attention, but nothing is broken" rather than a second
 * hand-picked shade. Referenced as a CSS var (not a Tailwind class) because the
 * SpeleoDB mark is a raster PNG tinted via a mask, which needs a real colour value.
 */
export const UNPUSHED_ACCENT = 'var(--sh-comment)';

/**
 * Per-project local state for `projectIds`, keyed by project id. Purely local (no
 * token, no network): the backend walks the pull cache read-only and degrades a
 * bad file to a "nothing here" row rather than failing the call.
 *
 * Throws only if the IPC itself fails; callers must treat that as "unknown", never
 * as "no local edits".
 */
export async function fetchSpeleoLocalStates(projectIds: string[]): Promise<SpeleoLocalStateMap> {
  if (projectIds.length === 0) return {};
  const reply = await invokeIpc<SpeleoLocalStateMap | null>(
    'speleodb_local_project_states',
    { projectIds },
    { purpose: 'find which projects have changes only on this computer' },
  );
  // A mocked/older backend can answer `null`; treat it as "nothing known".
  return reply ?? {};
}

/** Whether `states` positively reports un-pushed local edits for `projectId`. */
export function hasUnpushedEdits(
  states: SpeleoLocalStateMap,
  projectId: string | null | undefined,
): boolean {
  if (!projectId) return false;
  return states[projectId]?.has_unpushed_edits === true;
}

/**
 * Whether the backend found a local copy for `projectId` but could NOT read its
 * state - `SpeleoLocalProjectState.state_unknown` (#270).
 *
 * The listing fails OPEN per row so one bad file costs only its own row, which
 * means an unreadable copy arrives with `has_unpushed_edits: false` - byte-identical
 * to a genuinely clean one. `state_unknown` is the discriminant, and callers that
 * gate an IRREVERSIBLE action on "clean" must treat it as unknown, not as clean:
 * the likely cause is a hot journal left by a kill/power-loss (a read-only
 * connection cannot roll one back), i.e. exactly the abandoned session whose work
 * is most at risk. The guard on the Rust side already fails CLOSED on the same
 * input; this keeps the UI from disagreeing with it.
 */
export function localStateUnknown(
  states: SpeleoLocalStateMap,
  projectId: string | null | undefined,
): boolean {
  if (!projectId) return false;
  return states[projectId]?.state_unknown === true;
}

/**
 * Whether this computer can positively NOT have taken the write lock the account
 * holds on `projectId` (demo item 1).
 *
 * SpeleoDB's lock belongs to the ACCOUNT, so the same account signed in on another
 * computer holds it too, and the server cannot say which session took it. This
 * computer can only rule itself out: the row was read (not `state_unknown`), there
 * is no local copy, the project is not open, this run holds no claim on the lock
 * (`lock_claimed_here`), and no recent file is linked to the project
 * (`linkedHere`, a published copy kept outside the cache). A clean copy here is
 * NOT enough: a session on this computer that crashed over it leaves exactly that,
 * and claims are not persisted across a restart.
 *
 * A positive report only: no row, or a failed read, is "cannot tell" (false).
 */
export function lockNotTakenHere(
  states: SpeleoLocalStateMap,
  projectId: string | null | undefined,
  linkedHere: boolean,
): boolean {
  if (!projectId || linkedHere) return false;
  const state = states[projectId];
  if (!state) return false;
  return (
    !state.has_local_copy &&
    !state.state_unknown &&
    !state.is_open &&
    state.lock_claimed_here !== true
  );
}

/**
 * Where this account's write lock on `projectId` was taken, as far as this
 * computer can tell (sdbdemo2 issue 3): `here` when this run holds a claim on it
 * (`lock_claimed_here`), `elsewhere` when this computer can positively not have
 * taken it ({@link lockNotTakenHere}), `unknown` otherwise (a copy here, a
 * failed or missing read, a recent file linked to the project). Only `here`
 * may say "You are editing": the Home card used to say it for every lock of
 * the account, right behind the held-lock prompt's "not by this computer".
 */
export type SpeleoLockOrigin = 'here' | 'elsewhere' | 'unknown';

export function speleoLockOrigin(
  states: SpeleoLocalStateMap,
  projectId: string | null | undefined,
  linkedHere: boolean,
): SpeleoLockOrigin {
  if (projectId && states[projectId]?.lock_claimed_here === true) return 'here';
  return lockNotTakenHere(states, projectId, linkedHere) ? 'elsewhere' : 'unknown';
}
