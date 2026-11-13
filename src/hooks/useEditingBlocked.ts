/**
 * Canonical "is editing blocked right now" predicate: the survey map is
 * read-only whenever `isMapReadOnly` is true (closure-adjusted view, or
 * Plan/Export mode) OR a remote SpeleoDB project is open without the write
 * lock (`selectSpeleoReadOnly`).
 *
 * A prior generation of call sites wrote this composition inline as
 * `useUIStore(isMapReadOnly) || useSpeleoStore(selectSpeleoReadOnly)`. That
 * expression is a Rules-of-Hooks violation: `||` short-circuits, so the
 * second zustand hook is skipped entirely whenever the first term is
 * already `true` on a given render. If that term later flips back to
 * `false` on a rerender, React sees fewer hooks called than the previous
 * render and throws ("Rendered fewer hooks than expected"). Every call
 * site must call BOTH store hooks unconditionally, on separate lines, and
 * only OR the resulting plain booleans - so this hook is the single place
 * that does it correctly (#275).
 *
 * The SpeleoDB term fails CLOSED (#270): a project whose remoteness has not been
 * established yet (`projectLinkKind === 'unknown'`) counts as read-only, so there
 * is no window where a remote project without the write lock presents as
 * editable. Only a confirmed-local project or a held write lock opens the gate.
 *
 * Backstop asymmetry: the SpeleoDB term (`selectSpeleoReadOnly`) has a
 * fail-safe server guard (`AppState::guard_mutation` rejects mutations without
 * the write lock). The `isMapReadOnly` term (closure-adjusted view, Plan/Export
 * mode) does NOT: a local project in those views has `edit_enabled == true`
 * server-side, so read-only there rests entirely on exhaustive UI gating plus
 * the `check-readonly-gates.mjs` tripwire. A new destructive control in an
 * adjusted/Plan view must therefore be gated here at every entry point; there
 * is no server backstop to catch a miss.
 */
import { useUIStore, isMapReadOnly } from '../store/uiStore';
import { useSpeleoStore, selectSpeleoReadOnly } from '../store/speleoStore';

/** React-hook form: subscribes to both stores and recomposes on change. */
export function useEditingBlocked(): boolean {
  const mapReadOnly = useUIStore(isMapReadOnly);
  const speleoReadOnly = useSpeleoStore(selectSpeleoReadOnly);
  return mapReadOnly || speleoReadOnly;
}

/**
 * Imperative twin for call sites outside React (raw keydown handlers, utility
 * functions) that need a one-shot read via `getState()` rather than a
 * subscription.
 */
export function isEditingBlockedNow(): boolean {
  return isMapReadOnly(useUIStore.getState()) || selectSpeleoReadOnly(useSpeleoStore.getState());
}

/**
 * Which of the two terms blocks editing, for copy that says why: `'project'`
 * for a remote SpeleoDB project open without the write lock, `'view'` for the
 * closure-adjusted view or Plan/Export, `null` when editing is open. The
 * project wins when both hold, since leaving the view would not unblock it.
 */
export type EditingBlockedReason = 'project' | 'view' | null;

function blockedReason(mapReadOnly: boolean, speleoReadOnly: boolean): EditingBlockedReason {
  if (speleoReadOnly) return 'project';
  return mapReadOnly ? 'view' : null;
}

/** React-hook form of the reason; calls both store hooks unconditionally. */
export function useEditingBlockedReason(): EditingBlockedReason {
  const mapReadOnly = useUIStore(isMapReadOnly);
  const speleoReadOnly = useSpeleoStore(selectSpeleoReadOnly);
  return blockedReason(mapReadOnly, speleoReadOnly);
}

/** Imperative twin of `useEditingBlockedReason`, read via `getState()`. */
export function editingBlockedReasonNow(): EditingBlockedReason {
  return blockedReason(isMapReadOnly(useUIStore.getState()), selectSpeleoReadOnly(useSpeleoStore.getState()));
}
