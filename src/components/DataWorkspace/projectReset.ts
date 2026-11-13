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

/** A project's identity across edits and Save As: its creation stamp. */
const identityOf = (s: { project: { created_at: string } | null }) => s.project?.created_at ?? null;

export function installDataWorkspaceProjectReset(): void {
  if (installed) return;
  installed = true;
  useProjectStore.subscribe((s, prev) => {
    const next = identityOf(s);
    if (next !== identityOf(prev)) useDataWorkspaceStore.getState().resetForProject();
  });
}

installDataWorkspaceProjectReset();
