/**
 * [#576 RVF-c-C1] The `expectedProject` a request made NOW carries: the project open
 * in the store at this moment, in the form `projectIssue.expectedProjectOf` gives it.
 * Every IPC that writes the open project - beyond the edits and the save, which
 * `projectStore` names itself - sends it: the dive-plan save and delete, and the
 * SpeleoDB verbs that act on the open project or on its server side. The backend
 * refuses one made for another project than the open one with PROJECT_CHANGED
 * (`core/state/expected_project.rs`).
 *
 * Call it when the user makes the request, before any await a project switch could
 * span (the queued-edits wait, a dialog), and send what it returned.
 */
import { expectedProjectOf, issuedAgainstOf } from './projectIssue';
import { useProjectStore } from './projectStore';
import type { ProjectIdentity } from '../types/ipc/ProjectIdentity';

/** The identity of the project open now, or null when none is. */
export function expectedOpenProject(): ProjectIdentity | null {
  return expectedProjectOf(issuedAgainstOf(useProjectStore.getState().project));
}
