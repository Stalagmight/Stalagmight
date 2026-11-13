/**
 * Clear the Data workspace's project-specific state (filters - which name
 * section ids -, search, sort, collapse, the active row, focus requests and
 * the Back-to-Data chip) whenever a different project is opened or the
 * project is closed. The persisted view (columns, tint, map pane) is kept.
 *
 * Installed once, on first import, by the workspace modules that are mounted
 * in the single-project view; idempotent.
 */
import { useProjectStore } from '../../store/projectStore';
import { useDataWorkspaceStore } from './store';

let installed = false;

type IdentitySource = {
  project: { created_at: string } | null;
  loading: { clientOpId: number };
};

/**
 * The open project INSTANCE: the load that produced it (`loading.clientOpId`,
 * bumped by every open, create and import) plus its creation stamp. Not
 * `created_at` alone: a Save As copy keeps it, so opening the copy (or the
 * original again) would inherit the other file's filters, active row and
 * Back-to-Data chip. Not the file path: a Save As or a first save re-points
 * it while the same data stays open, and the user's view should survive that.
 */
const identityOf = (s: IdentitySource): string | null =>
  s.project ? `${s.loading.clientOpId}:${s.project.created_at}` : null;

export function installDataWorkspaceProjectReset(): void {
  if (installed) return;
  installed = true;
  let current = identityOf(useProjectStore.getState());
  useProjectStore.subscribe((s, prev) => {
    // Re-keyed only when the project object changes: a load that starts (the
    // op id moves) and is then cancelled leaves the open project, and its
    // state, alone.
    if (s.project === prev.project) return;
    const next = identityOf(s);
    if (next === current) return;
    current = next;
    useDataWorkspaceStore.getState().resetForProject();
  });
}

installDataWorkspaceProjectReset();
