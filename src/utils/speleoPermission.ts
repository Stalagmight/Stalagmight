import type { Permission } from '../types/ipc.generated';

/**
 * Capability ordering for SpeleoDB permission tiers, mirroring the Rust
 * `Permission` `Ord` derive (`src-tauri/src/speleodb/models.rs`): download ≥
 * READ_ONLY, upload/lock ≥ READ_AND_WRITE, administration = ADMIN. The wire value
 * is the label string, so this map - not lexical comparison - is the source of
 * truth for thresholds.
 */
const PERMISSION_ORDER: Record<Permission, number> = {
  WEB_VIEWER: 0,
  READ_ONLY: 1,
  READ_AND_WRITE: 2,
  ADMIN: 3,
};

/**
 * True when `permission` meets or exceeds `min`. A `null`/`undefined` permission
 * (no access, or not yet known) never meets a threshold.
 */
export function permissionAtLeast(
  permission: Permission | null | undefined,
  min: Permission,
): boolean {
  if (!permission) return false;
  return PERMISSION_ORDER[permission] >= PERMISSION_ORDER[min];
}

/**
 * Whether "Resume editing" (take the write lock again) can apply to the open
 * project: signed in, with project access, and at least read-and-write
 * permission. Whether the lock is actually free is the server's answer - the
 * start-editing flow reports a contended lock on its own. Shared by the SpeleoDB
 * workspace and the read-only map pill so the two never disagree.
 */
export function canResumeSpeleoEditing(
  link: { noAccess: boolean; permission: Permission | null } | null | undefined,
  session: { loggedIn: boolean } | null | undefined,
): boolean {
  return !!link && session?.loggedIn === true && !link.noAccess &&
    permissionAtLeast(link.permission, 'READ_AND_WRITE');
}
