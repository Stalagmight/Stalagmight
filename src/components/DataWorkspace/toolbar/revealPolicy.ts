/**
 * Navigation reveals (`revealRowKeys`) last "until the filters change or the
 * user dismisses them" (store.ts). The store's filter setters don't know about
 * reveals, so this one subscription enforces it for every writer (the toolbar,
 * the chips, "Open in Data"), mounted or not. A single `set` that changes the
 * filters AND the reveals keeps the new reveals.
 */
import { useDataWorkspaceStore } from '../store';

let installed = false;

export function installRevealPolicy(): void {
  if (installed) return;
  installed = true;
  useDataWorkspaceStore.subscribe((state, prev) => {
    if (state.filters !== prev.filters && state.revealRowKeys.length > 0 && state.revealRowKeys === prev.revealRowKeys) {
      useDataWorkspaceStore.setState({ revealRowKeys: [] });
    }
  });
}
