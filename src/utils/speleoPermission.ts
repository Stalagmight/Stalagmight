import type { Permission } from '../types/ipc.generated';

/**
 * Capability ordering for SpeleoDB permission tiers, mirroring the Rust
 * `Permission` `Ord` derive (`src-tauri/src/speleodb/models.rs`): download ≥
 * READ_ONLY, upload/lock ≥ READ_AND_WRITE, delete/admin = ADMIN. The wire value
 * is the label string, so this map - not lexical comparison - is the source of
 * truth for thresholds.
 *
 * `UNKNOWN` (#576 B16) is a tier the server added after this build shipped: it ranks
 * below every known tier, so every capability check denies it (the fail-safe way).
 */
const PERMISSION_ORDER: Record<Permission, number> = {
  UNKNOWN: -1,
  WEB_VIEWER: 0,
  READ_ONLY: 1,
  READ_AND_WRITE: 2,
  ADMIN: 3,
};

/**
 * Levels a USER collaborator can be granted, in ascending order. Teams may never
 * hold ADMIN (the server rejects it), so the team picker omits it. Shared by the
 * Project Settings admin roster and the Publish dialog's collaborator picker so
 * both offer exactly the same tiers.
 */
export const USER_LEVELS: readonly Permission[] = [
  'WEB_VIEWER',
  'READ_ONLY',
  'READ_AND_WRITE',
  'ADMIN',
];
export const TEAM_LEVELS: readonly Permission[] = ['WEB_VIEWER', 'READ_ONLY', 'READ_AND_WRITE'];

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
