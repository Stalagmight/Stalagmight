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
 * the write lock). The `isMapReadOnly` term is backstopped for only one of its
 * two halves. Plan/Export are, since #576 C5: the frontend reports the
 * workspace mode (`set_workspace_mode`) and `guard_mutation` refuses survey
 * edits there with `WORKSPACE_READ_ONLY` - though only once the mode change
 * has reached the backend. The Closures Computed View is NOT: it is a Survey
 * sub-view, a local project there has `edit_enabled == true` server-side, so
 * read-only there rests entirely on exhaustive UI gating plus
 * the `check-readonly-gates.mjs` tripwire. A new destructive control in an
 * adjusted or Plan view must therefore be gated here at every entry point; in
 * the closures view there is no server backstop to catch a miss.
 *
 * [#576 B6-D1] A third term: a project saved by a NEWER Stalagmight
 * (`selectNewerFileReadOnly`) is read-only in every mode, settled from the load
 * result like the SpeleoDB one. Server-backed too - `guard_mutation` refuses
 * every edit of it and `save_project` every save.
 */
import { useUIStore, isMapReadOnly } from '../store/uiStore';
import { useSpeleoStore, selectSpeleoReadOnly } from '../store/speleoStore';
import { useProjectStore, selectNewerFileReadOnly } from '../store/projectStore';

/** React-hook form: subscribes to all three stores and recomposes on change. */
export function useEditingBlocked(): boolean {
  const mapReadOnly = useUIStore(isMapReadOnly);
  const speleoReadOnly = useSpeleoStore(selectSpeleoReadOnly);
  const newerFileReadOnly = useProjectStore(selectNewerFileReadOnly);
  return mapReadOnly || speleoReadOnly || newerFileReadOnly;
}

/**
 * Imperative twin for call sites outside React (raw keydown handlers, utility
 * functions) that need a one-shot read via `getState()` rather than a
 * subscription.
 */
export function isEditingBlockedNow(): boolean {
  return (
    isMapReadOnly(useUIStore.getState()) ||
    selectSpeleoReadOnly(useSpeleoStore.getState()) ||
    selectNewerFileReadOnly(useProjectStore.getState())
  );
}

/**
 * WHICH term of the gate blocks editing, so a disabled control can say what would
 * enable it (#576 D39(a)); null exactly when editing is not blocked. When several
 * apply, the one that cannot be lifted from the current screen comes first: a file
 * from a newer Stalagmight (no in-app remedy), then a remote project without the
 * write lock, then the Plan/Export workspace, then the Closures Computed View.
 */
export type EditBlockedReason = 'newerFile' | 'lock' | 'workspace' | 'closures';

function editBlockedReasonOf(
  ui: Parameters<typeof isMapReadOnly>[0],
  speleoReadOnly: boolean,
  newerFileReadOnly: boolean
): EditBlockedReason | null {
  if (newerFileReadOnly) return 'newerFile';
  if (speleoReadOnly) return 'lock';
  if (!isMapReadOnly(ui)) return null;
  // The map's own read-only term: the Plan/Export workspace, else the Closures Computed View.
  return ui.appMode === 'survey' ? 'closures' : 'workspace';
}

/** React-hook form of the reason; every store hook is called unconditionally (#275). */
export function useEditBlockedReason(): EditBlockedReason | null {
  const appMode = useUIStore((s) => s.appMode);
  const geometryMode = useUIStore((s) => s.geometryMode);
  const speleoReadOnly = useSpeleoStore(selectSpeleoReadOnly);
  const newerFileReadOnly = useProjectStore(selectNewerFileReadOnly);
  return editBlockedReasonOf({ appMode, geometryMode }, speleoReadOnly, newerFileReadOnly);
}

/** Imperative twin of {@link useEditBlockedReason}, for a re-check at commit time. */
export function editBlockedReasonNow(): EditBlockedReason | null {
  return editBlockedReasonOf(
    useUIStore.getState(),
    selectSpeleoReadOnly(useSpeleoStore.getState()),
    selectNewerFileReadOnly(useProjectStore.getState())
  );
}
