import type { SpeleoCachedProject } from '../types/ipc.generated';

/**
 * Whether the home screen lists this cached copy for the current session: signed
 * out, every copy; signed in, the account's own copies and the older ones with no
 * recorded account from the same SpeleoDB instance. The backend classifies.
 */
export function isListedCachedCopy(copy: SpeleoCachedProject): boolean {
  return copy.ownership !== 'other_account';
}

/** The copy may hold work SpeleoDB does not have (or could not be read). */
export function cachedCopyAtRisk(copy: SpeleoCachedProject): boolean {
  return copy.has_unpushed_edits || copy.state_unknown;
}
