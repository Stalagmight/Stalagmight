/**
 * Navigation reveals (`revealRowKeys`) last "until the filters change or the
 * user dismisses them" (store.ts). The store's filter setters don't know about
 * reveals, so this one subscription enforces it for every writer (the toolbar,
 * the chips, "Open in Data"), mounted or not. A single `set` that changes the
 * filters AND the reveals keeps the new reveals.
 */
import { useDataWorkspaceStore } from '../store';
import type { FilterState } from '../types';

let installed = false;

/** Same filters by content (a setter called with what is already set keeps reveals). */
export function sameFilters(a: FilterState, b: FilterState): boolean {
  if (a === b) return true;
  if (a.rowTypes !== b.rowTypes) return false;
  const sa = a.sectionIds;
  const sb = b.sectionIds;
  if (sa !== sb && (sa === null || sb === null || sa.length !== sb.length || sa.some((id, i) => id !== sb[i]))) {
    return false;
  }
  const qa = a.quick;
  const qb = b.quick;
  if ((Object.keys(qa) as (keyof typeof qa)[]).some((k) => qa[k] !== qb[k])) return false;
  return (
    a.conditions.length === b.conditions.length &&
    a.conditions.every((c, i) => {
      const d = b.conditions[i]!;
      return c.id === d.id && c.colKey === d.colKey && c.op === d.op && c.value === d.value;
    })
  );
}

export function installRevealPolicy(): void {
  if (installed) return;
  installed = true;
  useDataWorkspaceStore.subscribe((state, prev) => {
    if (
      state.revealRowKeys.length > 0 &&
      state.revealRowKeys === prev.revealRowKeys &&
      !sameFilters(state.filters, prev.filters)
    ) {
      useDataWorkspaceStore.setState({ revealRowKeys: [] });
    }
  });
}
