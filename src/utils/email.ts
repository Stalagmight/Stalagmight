/**
 * Case-insensitive email equality, the single source of truth for "is this SpeleoDB
 * lock holder me?" Both the recents badge (`resolveSpeleoLock` → `lockedByMe`) and
 * the held-lock scan (`scanLocksOnOpen` → `heldLocks`) derive ownership off the same
 * `list_projects` response; routing both through this helper keeps them from drifting
 * (e.g. one adding trimming the other doesn't). SpeleoDB may normalise case, so the
 * comparison mirrors the backend's case-insensitive `lock_held_by_me`.
 */
export function emailsMatch(
  a: string | null | undefined,
  b: string | null | undefined,
): boolean {
  return !!a?.trim() && !!b?.trim() && a.trim().toLowerCase() === b.trim().toLowerCase();
}
