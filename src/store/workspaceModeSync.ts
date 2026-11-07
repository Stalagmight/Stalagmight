/**
 * [#576 C5] Mirror the workspace mode (`uiStore.appMode`) into the backend.
 *
 * Plan and Export are read-only workspaces. The UI hides and disables the editing
 * controls there (docs/app-modes.md), but the backend is the backstop: while the
 * mode it was last told is Plan or Export, its mutation chokepoint refuses every
 * survey edit with `EDIT_DISABLED: WORKSPACE_READ_ONLY` (see `editErrors.ts`), so a
 * gap in the UI gating cannot reach the project. Only the project name, the
 * validation thresholds and the rename are allowed there, on purpose; the default
 * north reference that saves with them is a survey edit (#576 NF-W3-2-PLAN).
 *
 * Every `appMode` change is sent, whoever made it (`setAppMode`, the load reset in
 * `applyDisplaySettings`, `exitGasPlan` on close). The backend also resets to
 * Survey on every project open, create and close, matching the frontend's own
 * reset, so the two agree across a project switch without a round trip.
 *
 * Sends go through the store's mutation queue (`mutationQueue.ts`), behind every
 * survey edit issued before the mode changed (#576 RV2-FE-G1): an edit made in Survey
 * that is still waiting there (behind the previous edit's refresh, say) when the user
 * clicks Plan reaches the backend first and lands, instead of arriving after the mode
 * and being refused. The same queue keeps the sends in the order the mode changed. A
 * failed send is logged and the next mode change sends again (a stale Plan would
 * refuse edits in Survey with the workspace notice, a stale Survey is the old UI-only
 * gating - neither corrupts the project).
 */
import { invokeIpc } from '../utils/ipc';
import { runSerialized } from './mutationQueue';
import { useUIStore, type AppMode } from './uiStore';

/** Send one mode, after every edit and send already queued. Never rejects. */
export function sendWorkspaceMode(mode: AppMode): Promise<void> {
  return runSerialized(async () => {
    await invokeIpc('set_workspace_mode', { mode }, {
      purpose: 'mirror the workspace mode for the read-only gate',
      fields: { mode },
    }).catch((e: unknown) => {
      console.warn('[workspaceModeSync] could not report the workspace mode to the backend', {
        mode,
        reason: String(e),
      });
    });
  });
}

/**
 * Start mirroring: send the current mode once (a webview reload keeps the backend
 * but starts the frontend in Survey), then every change. Returns the unsubscribe.
 */
export function startWorkspaceModeSync(): () => void {
  void sendWorkspaceMode(useUIStore.getState().appMode);
  return useUIStore.subscribe((state, prev) => {
    if (state.appMode !== prev.appMode) void sendWorkspaceMode(state.appMode);
  });
}
