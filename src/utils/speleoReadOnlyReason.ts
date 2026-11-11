import type { Permission } from '../types/ipc.generated';
import { permissionAtLeast } from './speleoPermission';

/**
 * Why an open SpeleoDB project cannot be edited at all, as the user should read it,
 * or `null` when editing is possible in principle (someone else may still hold the
 * lock - that has its own words).
 *
 * - `signed_out`: there is no SpeleoDB session, so the project is a read-only copy
 *   until the user logs in.
 * - `view_only`: signed in, but the account's permission on this project is below
 *   read-and-write.
 *
 * One rule for the map's read-only pill and the SpeleoDB workspace header, so the
 * two never disagree.
 */
export type SpeleoReadOnlyReason = 'signed_out' | 'view_only' | null;

export function speleoReadOnlyReason(
  link: { permission: Permission | null } | null | undefined,
  session: { loggedIn: boolean } | null | undefined,
): SpeleoReadOnlyReason {
  if (session?.loggedIn !== true) return 'signed_out';
  if (link?.permission && !permissionAtLeast(link.permission, 'READ_AND_WRITE')) return 'view_only';
  return null;
}

/** The i18n key of the prose for `reason`. */
export function speleoReadOnlyReasonKey(reason: Exclude<SpeleoReadOnlyReason, null>): string {
  return reason === 'signed_out'
    ? 'speleodb.cachedCopies.readOnlySignedOut'
    : 'speleodb.cachedCopies.viewOnly';
}
