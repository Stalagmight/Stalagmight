// @vitest-environment jsdom
/**
 * Hook test (#218) - the shared pull+open flow for remote SpeleoDB projects.
 * Covers the outcome routing GlobalView + the browser dialog both depend on: a
 * normal open, access warnings, a load failure that rolls the link back, and
 * an open error that reports a re-login.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { renderHook } from '@testing-library/react';

const invokeIpcMock = vi.hoisted(() => vi.fn());
const confirmMock = vi.hoisted(() => vi.fn());
// The unsaved-edits question before a switch is in-app (`askExitChoice`); answer it
// through `confirmMock`: true = discard, false = cancel, 'save' = save on this
// computer first. The rest of the module (the drafts question) stays real.
vi.mock('../utils/exitConfirm', async (importOriginal) => ({
  ...await importOriginal<typeof import('../utils/exitConfirm')>(),
  askExitChoice: (options: unknown) => Promise.resolve(confirmMock(options))
    .then((answer: unknown) => (answer === 'save' ? 'save' : answer ? 'confirm' : 'cancel')),
}));
vi.mock('../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (_e: unknown, f?: string) => f ?? 'error',
}));
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ close: vi.fn(async () => {}), destroy: vi.fn(async () => {}) }),
}));

import { useOpenRemoteProject } from './useOpenRemoteProject';
import { useProjectStore } from '../store/projectStore';
import { useSpeleoStore, selectSpeleoReadOnly, type SpeleoLinkState } from '../store/speleoStore';
import { useUIStore } from '../store/uiStore';
import { guardQuit } from '../utils/quit';
import { resetSpeleoExitPromptForTests, useSpeleoExitPrompt } from '../utils/speleoExit';
import { useExitConfirm } from '../utils/exitConfirm';
import { registerWorkspaceDraft } from '../utils/workspaceDrafts';
import type { SpeleoProjectSummary } from '../types/ipc.generated';

// The genuine store action, captured before `beforeEach` swaps in the spy - so one
// case can exercise the real seed end to end (seed -> guardQuit).
const realSetLinkOnOpen = useSpeleoStore.getState().setLinkOnOpen;

const SUMMARY: SpeleoProjectSummary = {
  id: 'p1',
  name: 'Cave A',
  permission: 'READ_AND_WRITE',
  lock_holder: null,
  type_tag: 'stalx',
  latest_commit_sha: 'abc123',
};

/** A settled link for whatever project is OPEN, for the entry-guard cases below. */
const openLink = (o: Partial<SpeleoLinkState> = {}): SpeleoLinkState => ({
  permission: 'READ_AND_WRITE',
  editState: 'read-only',
  lockHolder: null,
  lockedByOther: false,
  hasUnpushedEdits: false,
  unwound: false,

  undoMode: 'full',
  remoteState: 'up_to_date',
  noAccess: false,
  serverError: null,
  ...o,
});

let loadProjectMock: ReturnType<typeof vi.fn>;
let setLinkOnOpenMock: ReturnType<typeof vi.fn>;

beforeEach(() => {
  invokeIpcMock.mockReset();
  confirmMock.mockReset().mockResolvedValue(true);
  loadProjectMock = vi.fn(async () => ({ success: true }));
  setLinkOnOpenMock = vi.fn();
  // `offline fallback` compares the refused path against the LIVE open project, so the
  // "nothing open" baseline has to be explicit.
  useProjectStore.setState({
    loadProject: loadProjectMock,
    project: null,
    isDirty: false,
    checkProjectIntegrity: vi.fn(async () => []),
  });
  useSpeleoStore.setState({
    setLinkOnOpen: setLinkOnOpenMock,
    link: null,
    // The open remote project's id, which the entry guard reads to tell "the target
    // IS what is on screen" from a swap. Reset explicitly - the store is shared.
    remoteProjectId: null,
    session: { loggedIn: true, userEmail: "me@example.com", endpoint: "https://example.com" },
  });
  useUIStore.setState({ showToast: vi.fn(), openDialog: vi.fn() });
  resetSpeleoExitPromptForTests();
});

const hook = (opts: Parameters<typeof useOpenRemoteProject>[0] = {}) =>
  renderHook(() => useOpenRemoteProject(opts)).result.current;

const openOfflineCopy = async () => {
  invokeIpcMock.mockResolvedValue({
    local_path: '/cache/p1.stalx',
    offline: true, has_unpushed_edits: true,
  });
  return hook().openRemote(SUMMARY);
};

describe('useOpenRemoteProject (#218)', () => {
  it('opens a project: seeds the link, loads, and reports opened', async () => {
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_open_project')
        return Promise.resolve({ local_path: '/cache/p1.stalx' });
      return Promise.resolve(null);
    });
    const outcome = await hook().openRemote(SUMMARY);
    expect(outcome).toEqual({ status: 'opened' });
    expect(setLinkOnOpenMock).toHaveBeenCalledWith({
      permission: 'READ_AND_WRITE',
      // The path the seed belongs to, so the `syncProjectLink` inside the load
      // that follows keeps THIS project's link instead of the previous one's.
      forPath: '/cache/p1.stalx',
      // A PULL has, by construction, nothing local the server has not seen.
      hasUnpushedEdits: false,
      // And WHICH project, so `remoteProjectId` names it from the same synchronous
      // `set()` rather than after a status refresh - which left every consumer keyed on
      // it reading the PREVIOUS project until that request finished.
      projectId: 'p1',
    });
    expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1.stalx', { accessConfirmed: true, request: expect.any(Number) });
  });

  it('names the opened project immediately, so A -> B does not leave A on the store', async () => {
    // Driven through the REAL seed, because the bug is invisible to a test that sets
    // `remoteProjectId` by hand: it was written only by the status refresh, so after
    // opening remote B from remote A the store named A until that request finished.
    // Everything keyed on it read A - the revision
    // history that lists revisions, and the revert guards that compare against the id
    // that history handed them, which is why both sides agreed on a stale A.
    useSpeleoStore.setState({
      setLinkOnOpen: realSetLinkOnOpen,
      remoteProjectId: 'p-previous',
      linkPath: null,
      link: null,
    });
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_open_project')
        return Promise.resolve({ local_path: '/cache/p1.stalx' });
      return Promise.resolve(null);
    });

    expect(await hook().openRemote(SUMMARY)).toEqual({ status: 'opened' });

    // No refresh has run - `loadProject` is mocked and `refreshLinkStatus` is not driven -
    // so this can only be the seed.
    expect(useSpeleoStore.getState().remoteProjectId).toBe('p1');
  });

  /**
   * [#270] A failed load must leave the user exactly where they were - the PREVIOUS
   * project is still on screen and still open backend-side.
   *
   * The rollback used `clearLink`, which sets `projectLinkKind: 'unknown'` - read-only
   * under the fail-closed gate - and left the repair to an unawaited
   * `refreshLinkStatus` that swallows its errors, in the very conditions that break a
   * load. So a LOCAL project the user was editing could go read-only for the rest of
   * the session because an unrelated remote open failed. Driven through the REAL seed
   * and rollback, since the assertion is about the state the user is left in.
   */
  it('restores the previous link when the pulled project fails to load', async () => {
    useSpeleoStore.setState({
      setLinkOnOpen: realSetLinkOnOpen,
      link: null,
      projectLinkKind: 'local',
      linkPath: null,
    });
    expect(selectSpeleoReadOnly(useSpeleoStore.getState())).toBe(false);

    loadProjectMock.mockResolvedValue({ success: false });
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_open_project')
        return Promise.resolve({ local_path: '/cache/p1.stalx' });
      return Promise.resolve(null);
    });
    const outcome = await hook().openRemote(SUMMARY);
    expect(outcome).toEqual({ status: 'load_failed', localPath: '/cache/p1.stalx' });

    // Back to 'local', NOT 'unknown' - and still editable, with no network involved.
    expect(useSpeleoStore.getState().projectLinkKind).toBe('local');
    expect(useSpeleoStore.getState().link).toBeNull();
    expect(selectSpeleoReadOnly(useSpeleoStore.getState())).toBe(false);
  });

  it('reports an open error and flags a re-login on a dead token', async () => {
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_open_project')
        return Promise.reject({ message: 'unauthorized', action: 'ReLogin', status: 401 });
      return Promise.resolve(null);
    });
    const outcome = await hook().openRemote(SUMMARY);
    expect(outcome.status).toBe('error');
    if (outcome.status === 'error') expect(outcome.reLogin).toBe(true);
  });

  it('raises the offline fallback prompt when the open is refused over unpushed edits', async () => {
    // GlobalView's shape: no onBeforeLoad, outcome routed straight to its toast/
    // busy handling. The refusal must NOT come back as `error` (that would toast a
    // dead-end message) - it raises the prompt and reports `unpushed_local`.
    const openDialogMock = vi.fn();
    useUIStore.setState({ openDialog: openDialogMock });
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_open_project')
        return Promise.reject({
          message: 'refused',
          action: 'UnpushedLocalEdits',
          status: null,
          unpushed_local: {
            project_id: 'p1',
            local_path: '/cache/p1.stalx',
            last_edit_at: '2026-07-20T10:30:00Z',
          },
        });
      return Promise.resolve(null);
    });
    const outcome = await hook().openRemote(SUMMARY);
    expect(outcome).toEqual({ status: 'unpushed_local' });
    expect(openDialogMock).toHaveBeenCalledWith('speleodb-unpushed-local', {
      summary: SUMMARY,
      unpushedLocal: {
        project_id: 'p1',
        local_path: '/cache/p1.stalx',
        last_edit_at: '2026-07-20T10:30:00Z',
      },
    });
    expect(loadProjectMock).not.toHaveBeenCalled();
  });

  it('does not swallow an unrelated failure into the prompt', async () => {
    const openDialogMock = vi.fn();
    useUIStore.setState({ openDialog: openDialogMock });
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_open_project')
        return Promise.reject({ message: 'boom', action: 'None', status: 500 });
      return Promise.resolve(null);
    });
    const outcome = await hook().openRemote(SUMMARY);
    expect(outcome.status).toBe('error');
    expect(openDialogMock).not.toHaveBeenCalled();
  });

  it('signed out: opens the cached copy read-only, without taking the lock, and says so', async () => {
    useSpeleoStore.setState({ session: { loggedIn: false, userEmail: null, endpoint: null } });
    invokeIpcMock.mockResolvedValue({ local_path: '/cache/p1.stalx', read_only: 'signed_out', has_unpushed_edits: true });
    const generation = useSpeleoStore.getState().sessionGeneration;
    expect(await hook().openRemote(SUMMARY)).toEqual({ status: 'opened' });
    expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1.stalx', {
      accessConfirmed: true, request: expect.any(Number), acquireLock: false,
    });
    // No server tip was confirmed, and the copy's un-uploaded edits are kept.
    expect(setLinkOnOpenMock).toHaveBeenCalledWith(expect.objectContaining({ hasUnpushedEdits: true, offline: true }));
    expect(useUIStore.getState().showToast).toHaveBeenCalledWith(
      'Opened a read-only copy. Connect to SpeleoDB to edit it.', 'speleodb-read-only-open', 'info');
    expect(useSpeleoStore.getState().sessionGeneration).toBe(generation);
  });

  it('expired session: ends the session here, still opens read-only, and warns', async () => {
    invokeIpcMock.mockResolvedValue({ local_path: '/cache/p1.stalx', read_only: 'session_expired' });
    const generation = useSpeleoStore.getState().sessionGeneration;
    // A status refresh landing during the load must not supersede the open.
    loadProjectMock.mockImplementation(async () => {
      useSpeleoStore.getState().setSession({ logged_in: false, user_email: null, endpoint: null });
      return { success: true };
    });
    expect(await hook().openRemote(SUMMARY)).toEqual({ status: 'opened' });
    expect(useSpeleoStore.getState().session).toEqual({ loggedIn: false, userEmail: null, endpoint: null });
    expect(useSpeleoStore.getState().sessionGeneration).toBe(generation + 1);
    expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1.stalx', expect.objectContaining({ acquireLock: false }));
    expect(useUIStore.getState().showToast).toHaveBeenCalledWith(
      expect.stringContaining('session has expired'), 'speleodb-read-only-open', 'warning');
  });

  it('offline fallback opens the cached path only after the remote open command', async () => {
    const outcome = await openOfflineCopy();
    expect(outcome).toEqual({ status: 'opened' });
    expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1.stalx', { accessConfirmed: true, request: expect.any(Number) });
    expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_open_project', expect.objectContaining({ projectId: SUMMARY.id }));
  });

  // Offline opening must preserve edits that never reached the server.
  it('offline fallback seeds the dirty bit TRUE - the backend reports that the cached copy is dirty', async () => {
    await openOfflineCopy();
    expect(setLinkOnOpenMock).toHaveBeenCalledWith({
      permission: 'READ_AND_WRITE',
      forPath: '/cache/p1.stalx',
      hasUnpushedEdits: true,
      offline: true,
      projectId: 'p1',
    });
  });

  it('offline fallback seeds NOTHING when the refusal names the already-open project', async () => {
    // "Open from SpeleoDB" is in the in-project menu, so re-opening the current
    // project is an ordinary mis-step. The live link is fully resolved (lock held,
    // permission, remote state); the optimistic seed would replace it with a
    // pessimistic guess, and `loadProject` then short-circuits on ALREADY_OPEN so
    // nothing re-resolves it synchronously - dropping the survey UI to read-only for
    // a project we hold the lock on. Skipping the seed is correct offline too.
    useProjectStore.setState({ project: { file_path: '/cache/p1.stalx' } as never });
    const outcome = await openOfflineCopy();
    expect(outcome).toEqual({ status: 'opened' });
    expect(setLinkOnOpenMock).not.toHaveBeenCalled();
    expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1.stalx', { accessConfirmed: true, request: expect.any(Number) });
  });

  it('does not touch the live link when the already-open offline fallback load fails', async () => {
    // There was no seed to roll back, and the link on screen is the open project's
    // own resolved state - downgrading it is the very thing the rollback undoes.
    const live = {
      permission: 'READ_AND_WRITE' as const,
      editState: 'editing' as const,
      lockHolder: null,
      lockedByOther: false,
      hasUnpushedEdits: true,
      unwound: false,

      undoMode: 'full' as const,
      remoteState: 'up_to_date' as const,
      noAccess: false,
      serverError: null,
    };
    useProjectStore.setState({ project: { file_path: '/cache/p1.stalx' } as never });
    useSpeleoStore.setState({ link: live, projectLinkKind: 'remote', linkPath: '/cache/p1.stalx', remoteProjectId: SUMMARY.id });
    loadProjectMock.mockResolvedValue({ success: false });
    const outcome = await openOfflineCopy();
    expect(outcome).toEqual({ status: 'load_failed', localPath: '/cache/p1.stalx' });
    expect(useSpeleoStore.getState().link).toEqual(live);
    expect(useSpeleoStore.getState().projectLinkKind).toBe('remote');
  });

  it('passes the refusal prompt a route back to the host dialog when given one', async () => {
    const openDialogMock = vi.fn();
    useUIStore.setState({ openDialog: openDialogMock });
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_open_project')
        return Promise.reject({
          message: 'refused',
          action: 'UnpushedLocalEdits',
          status: null,
          unpushed_local: {
            project_id: 'p1',
            local_path: '/cache/p1.stalx',
            last_edit_at: null,
          },
        });
      return Promise.resolve(null);
    });
    const withReturn = renderHook(() =>
      useOpenRemoteProject({ cancelReturnsTo: 'open-from-speleodb' }),
    ).result.current;
    await withReturn.openRemote(SUMMARY);
    expect(openDialogMock).toHaveBeenCalledWith(
      'speleodb-unpushed-local',
      expect.objectContaining({ cancelReturnsTo: 'open-from-speleodb' }),
    );
  });

  // Exercise the real store seed and exit guard with a dirty offline result.
  it('never claims an offline clean cache is up to date before the server responds', async () => {
    useSpeleoStore.setState({ setLinkOnOpen: realSetLinkOnOpen });
    invokeIpcMock.mockResolvedValue({
      local_path: '/cache/p1.stalx',
      offline: true, has_unpushed_edits: false,
    });
    expect(await hook().openRemote(SUMMARY)).toEqual({ status: 'opened' });
    expect(useSpeleoStore.getState().link?.remoteState).toBe('unknown');
    expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(false);
  });

  it('keeps saved offline work without requiring a quit prompt (real store seed)', async () => {
    useSpeleoStore.setState({ setLinkOnOpen: realSetLinkOnOpen });
    useUIStore.setState({ openDialog: vi.fn() as never, activeDialog: null, dialogData: {} });
    await openOfflineCopy();

    expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(true);
    // guardQuit needs a project open to guard. No question is asked: the quit only
    // hands back a lock first, if the backend holds one (none is, read-only).
    useProjectStore.setState({ project: { name: 'Cave A' } as never, isDirty: false });
    invokeIpcMock.mockResolvedValue({ success: true, warnings: [], error: null, section_ids: [] });
    guardQuit();
    await vi.waitFor(() => expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_settle_open_lock', { releaseLock: true }, expect.anything()));
    expect(useUIStore.getState().openDialog).not.toHaveBeenCalled();
  });

  /**
   * [#270] A superseded load is NOT a failure to roll back from.
   *
   * `loadProject` answers `success: false` for two different things, and only one
   * of them leaves the user where they were. When a newer open superseded this
   * one, the pre-attempt snapshot describes a project that is no longer on
   * screen - restoring it wrote `projectLinkKind: 'unknown'` (read-only under the
   * fail-closed gate) over the LOCAL project the user had just opened, with the
   * repair left to the unawaited, error-swallowing status refresh. Reachable:
   * GlobalView guards its remote rows with `remoteBusyRef`, but the recents-list
   * open and Publish-to-SpeleoDB have no such guard.
   */
  it('does NOT roll the link back when the load was superseded by a newer open', async () => {
    // What the newer open left behind: a local project, fully editable.
    const settled = () =>
      useSpeleoStore.setState({ link: null, projectLinkKind: 'local', linkPath: null });
    useSpeleoStore.setState({ setLinkOnOpen: realSetLinkOnOpen, projectLinkKind: 'unknown' });
    loadProjectMock.mockImplementation(async () => {
      settled();
      useProjectStore.setState({ project: { file_path: '/other/local.stalx' } as never });
      return { success: false, superseded: true };
    });
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_open_project')
        return Promise.resolve({ local_path: '/cache/p1.stalx' });
      return Promise.resolve(null);
    });

    const outcome = await hook().openRemote(SUMMARY);
    expect(outcome).toEqual({ status: 'load_failed', localPath: '/cache/p1.stalx' });
    // The project the user actually has open is untouched - and still editable.
    expect(useSpeleoStore.getState().projectLinkKind).toBe('local');
    expect(selectSpeleoReadOnly(useSpeleoStore.getState())).toBe(false);
  });

  it('openRemote seeds NOTHING when the pull names the already-open project', async () => {
    useProjectStore.setState({ project: { file_path: '/cache/p1.stalx' } as never });
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_open_project')
        return Promise.resolve({ local_path: '/cache/p1.stalx' });
      return Promise.resolve(null);
    });

    const outcome = await hook().openRemote(SUMMARY);
    expect(outcome).toEqual({ status: 'opened' });
    expect(setLinkOnOpenMock).not.toHaveBeenCalled();
    expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1.stalx', { accessConfirmed: true, request: expect.any(Number) });
  });

  /**
   * [#270] "Open from SpeleoDB" is a way OUT of the open project - `finishOpen` ends
   * in `loadProject`, which replaces it backend-side - and it was the only one that
   * ran no guard whatsoever: not the shared `needsSpeleoHandBackPrompt` the other
   * five exits share, and not even the disk-dirty check the two file-open exits had
   * before that. It went straight to the seed and the load, so picking a DIFFERENT
   * project in the browser discarded unsaved edits with no prompt and walked away
   * from un-pushed work with the server lock still held.
   */
  describe('discard confirmation before replacing a project', () => {
    beforeEach(() => {
      invokeIpcMock.mockResolvedValue({ local_path: '/cache/p1.stalx' });
      useProjectStore.setState({ project: { file_path: '/cache/p9.stalx' } as never, isDirty: false });
    });

    // Saved but un-uploaded work under our lock used to be walked away from here,
    // with the backend then releasing the lock over it. It now raises the exit
    // prompt BEFORE anything is downloaded; cancelling it leaves everything as it was.
    it.each(['openRemote', 'takeRemoteDiscardingLocal'] as const)('%s raises the exit prompt for un-uploaded work under our lock', async (action) => {
      useSpeleoStore.setState({ link: openLink({ editState: 'editing', hasUnpushedEdits: true }) });
      const pending = hook()[action](SUMMARY);
      await vi.waitFor(() => expect(useSpeleoExitPrompt.getState().request?.reason).toBe('switch'));
      expect(invokeIpcMock).not.toHaveBeenCalled();
      useSpeleoExitPrompt.getState().request?.settle(false);
      expect(await pending).toEqual({ status: 'cancelled' });
      expect(confirmMock).not.toHaveBeenCalled();
      expect(loadProjectMock).not.toHaveBeenCalled();
    });

    it.each(['openRemote', 'takeRemoteDiscardingLocal'] as const)('%s continues once the exit prompt has closed the project', async (action) => {
      useSpeleoStore.setState({ link: openLink({ editState: 'editing', hasUnpushedEdits: true }) });
      const pending = hook()[action](SUMMARY);
      await vi.waitFor(() => expect(useSpeleoExitPrompt.getState().request).not.toBeNull());
      // The prompt closes the project before it reports success.
      useProjectStore.setState({ project: null, isDirty: false });
      useSpeleoExitPrompt.getState().request?.settle(true);
      expect(await pending).toEqual({ status: 'opened' });
      expect(loadProjectMock).toHaveBeenCalled();
    });

    it('reopening the project already on screen is not leaving it: no prompt', async () => {
      useSpeleoStore.setState({ link: openLink({ editState: 'editing', hasUnpushedEdits: true }), remoteProjectId: 'p1' });
      expect(await hook().openRemote(SUMMARY)).toEqual({ status: 'opened' });
      expect(useSpeleoExitPrompt.getState().request).toBeNull();
    });

    it.each(['openRemote', 'takeRemoteDiscardingLocal'] as const)('%s does not prompt for a clean held lock', async (action) => {
      useSpeleoStore.setState({ link: openLink({ editState: 'editing', hasUnpushedEdits: false }) });
      expect(await hook()[action](SUMMARY)).toEqual({ status: 'opened' });
      expect(useSpeleoExitPrompt.getState().request).toBeNull();
      expect(confirmMock).not.toHaveBeenCalled();
      expect(loadProjectMock).toHaveBeenCalled();
    });

    it.each(['openRemote', 'takeRemoteDiscardingLocal'] as const)('%s cancellation preserves unsaved edits and the lock', async (action) => {
      useProjectStore.setState({ isDirty: true });
      confirmMock.mockResolvedValue(false);
      expect(await hook()[action](SUMMARY)).toEqual({ status: 'cancelled' });
      expect(confirmMock).toHaveBeenCalledOnce();
      expect(invokeIpcMock).not.toHaveBeenCalled();
      expect(loadProjectMock).not.toHaveBeenCalled();
    });

    it.each(['openRemote', 'takeRemoteDiscardingLocal'] as const)('%s proceeds after explicit discard without saving or uploading', async (action) => {
      useProjectStore.setState({ isDirty: true });
      expect(await hook()[action](SUMMARY)).toEqual({ status: 'opened' });
      expect(confirmMock).toHaveBeenCalledOnce();
      expect(invokeIpcMock.mock.calls.map(call => call[0])).not.toContain('save_project');
      expect(invokeIpcMock.mock.calls.map(call => call[0])).not.toContain('speleodb_push');
    });

    it.each(['openRemote', 'takeRemoteDiscardingLocal'] as const)('%s can save the edits on this computer instead of discarding them (frontend#6)', async (action) => {
      const saveProject = vi.fn(async () => {
        useProjectStore.setState({ isDirty: false });
        return { success: true };
      });
      useProjectStore.setState({ isDirty: true, saveProject: saveProject as never });
      confirmMock.mockResolvedValue('save');
      expect(await hook()[action](SUMMARY)).toEqual({ status: 'opened' });
      expect(confirmMock).toHaveBeenCalledWith(expect.objectContaining({ saveLabel: expect.any(String) }));
      expect(saveProject).toHaveBeenCalledOnce();
      expect(invokeIpcMock.mock.calls.map(call => call[0])).not.toContain('speleodb_push');
    });

    // frontend#10: unsent SpeleoDB input the switch drops is listed in that SAME
    // question - one question, not two - and is dropped by either answer that goes on.
    it.each([true, 'save'] as const)('lists unsent SpeleoDB input in the one unsaved-changes question (answer %s)', async (answer) => {
      const discard = vi.fn();
      const unregister = registerWorkspaceDraft({ label: () => 'the synthetic form', dirty: () => discard.mock.calls.length === 0, discard });
      useProjectStore.setState({
        isDirty: true,
        saveProject: vi.fn(async () => { useProjectStore.setState({ isDirty: false }); return { success: true }; }) as never,
      });
      confirmMock.mockResolvedValue(answer);
      expect(await hook().openRemote(SUMMARY)).toEqual({ status: 'opened' });
      unregister();
      expect(confirmMock).toHaveBeenCalledOnce();
      expect(confirmMock).toHaveBeenCalledWith(expect.objectContaining({ message: expect.stringContaining('the synthetic form') }));
      expect(useExitConfirm.getState().request).toBeNull();
      expect(discard).toHaveBeenCalledOnce();
    });

    it('asks about unsent SpeleoDB input alone when there are no unsaved edits; declining stops the open', async () => {
      const discard = vi.fn();
      const unregister = registerWorkspaceDraft({ label: () => 'the synthetic form', dirty: () => true, discard });
      const pending = hook().openRemote(SUMMARY);
      await vi.waitFor(() => expect(useExitConfirm.getState().request?.message).toContain('the synthetic form'));
      useExitConfirm.getState().request?.answer(false);
      expect(await pending).toEqual({ status: 'cancelled' });
      unregister();
      expect(discard).not.toHaveBeenCalled();
      expect(confirmMock).not.toHaveBeenCalled();
      expect(invokeIpcMock).not.toHaveBeenCalled();
    });

    it.each(['openRemote', 'takeRemoteDiscardingLocal'] as const)('%s stays put when saving the edits fails', async (action) => {
      useProjectStore.setState({ isDirty: true, saveProject: vi.fn(async () => ({ success: false, error: 'Synthetic disk failure' })) as never });
      useUIStore.setState({ openDialog: vi.fn() });
      confirmMock.mockResolvedValue('save');
      expect(await hook()[action](SUMMARY)).toEqual({ status: 'cancelled' });
      expect(loadProjectMock).not.toHaveBeenCalled();
    });

    it.each(['openRemote', 'takeRemoteDiscardingLocal'] as const)('%s protects edits made during download', async (action) => {
      const downloaded = deferredOpen<{ local_path: string }>();
      const entered = deferredOpen<void>();
      invokeIpcMock.mockImplementation(() => { entered.resolve(); return downloaded.promise; });
      const pending = hook()[action](SUMMARY);
      await entered.promise;
      useProjectStore.setState({ project: { file_path: '/cache/p9.stalx', name: 'Edited' } as never, isDirty: true });
      downloaded.resolve({ local_path: '/cache/p1.stalx' });
      expect((await pending).status).toBe('error');
      expect(loadProjectMock).not.toHaveBeenCalled();
    });

    it('does not treat edits made after a discard confirmation as acknowledged', async () => {
      useProjectStore.setState({ isDirty: true });
      const downloaded = deferredOpen<{ local_path: string }>();
      const entered = deferredOpen<void>();
      invokeIpcMock.mockImplementation(() => { entered.resolve(); return downloaded.promise; });
      const pending = hook().openRemote(SUMMARY);
      await entered.promise;
      useProjectStore.setState({ project: { file_path: '/cache/p9.stalx', name: 'New edit' } as never });
      downloaded.resolve({ local_path: '/cache/p1.stalx' });
      expect((await pending).status).toBe('error');
      expect(loadProjectMock).not.toHaveBeenCalled();
    });
  });

  /**
   * DO-NOT-REGRESS. Both "is this the project already on screen?" tests in the hook
   * (`finishOpen`'s `alreadyOpen`, `offline fallback`'s skip) compare paths as LITERAL
   * strings, and that is the point rather than an oversight: they predict whether
   * `loadProject` will short-circuit, and the backend decides that with the same
   * literal test (`do_load_project_full`: `project.file_path.as_ref() ==
   * Some(&path)`). An alias the backend cannot see through must therefore take the
   * NOT-already-open branch on this side too - the reload really does happen, so the
   * seed really is needed and the guard really does have something to protect.
   *
   * These cases fail the moment someone "fixes" either site with canonicalization or
   * a narrower normalization (collapsing `.`/`..`, case-folding): the first would
   * skip a seed the reload needs, the second would skip a guard that is standing in
   * front of unsaved in-memory survey. The webview has no filesystem to canonicalize
   * with in any case; alias tolerance would have to start in `do_load_project_full`
   * (`project_io::same_file`), with this side following.
   */
  describe('path spellings the backend would not match either', () => {
    it('openRemote still seeds when the open project is spelled differently', async () => {
      // Same file, dot-segment spelling. `do_load_project_full` compares literally,
      // so it reloads - and a reload with no seed resolves through `syncProjectLink`
      // to read-only / nothing-unpushed, dropping the permission on the floor.
      useProjectStore.setState({ project: { file_path: '/cache/./p1.stalx' } as never });
      invokeIpcMock.mockImplementation((cmd: string) =>
        cmd === 'speleodb_open_project'
          ? Promise.resolve({ local_path: '/cache/p1.stalx' })
          : Promise.resolve(null),
      );

      expect(await hook().openRemote(SUMMARY)).toEqual({ status: 'opened' });
      expect(setLinkOnOpenMock).toHaveBeenCalledWith({
        permission: 'READ_AND_WRITE',
        forPath: '/cache/p1.stalx',
        hasUnpushedEdits: false,
        projectId: 'p1',
      });
    });

    it('offline fallback still guards when the open project is spelled differently', async () => {
      // Case alias - the same file on Windows/macOS, and NOT what the backend's
      // literal check matches. So this ends in a full reload that drops the unsaved
      // in-memory survey, which is precisely what the disk-dirty refusal is for.
      useProjectStore.setState({
        project: { file_path: '/cache/P1.stalx' } as never,
        isDirty: true,
      });

      confirmMock.mockResolvedValue(false);
      const outcome = await openOfflineCopy();

      expect(outcome.status).toBe('cancelled');
      if (outcome.status === 'error') expect(outcome.message).toMatch(/sav/i);
      expect(loadProjectMock).not.toHaveBeenCalled();
    });
  });

});

describe('access warnings follow acquisition instead of blocking opening', () => {
  it.each([
    { permission: 'READ_ONLY' as const, lock_holder: null },
    { permission: 'READ_AND_WRITE' as const, lock_holder: 'other@example.com' },
    { permission: 'READ_ONLY' as const, lock_holder: 'other@example.com' },
  ])('opens $permission / $lock_holder without a pre-open warning', async (access) => {
    invokeIpcMock.mockResolvedValue({ local_path: '/cache/p1.stalx' });
    expect(await hook().openRemote({ ...SUMMARY, ...access })).toEqual({ status: 'opened' });
    expect(confirmMock).not.toHaveBeenCalled();
  });
});

function deferredOpen<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

it.each(['openRemote', 'takeRemoteDiscardingLocal'] as const)(
  '%s cannot replay a slow remote download over a newer project selection', async (action) => {
    const download = deferredOpen<{ local_path: string }>();
    const entered = deferredOpen<void>();
    invokeIpcMock.mockImplementation(() => { entered.resolve(); return download.promise; });
    const beforeLoad = vi.fn();
    const pending = hook({ onBeforeLoad: beforeLoad })[action](SUMMARY);
    await entered.promise;
    const { runConfirmedProjectSwitch } = await import('../utils/projectTransition');
    await runConfirmedProjectSwitch('/newer.stalx', () => null, true, async () => ({ success: true }));
    download.resolve({ local_path: '/cache/p1.stalx' });
    expect(await pending).toEqual({ status: 'cancelled' });
    expect(setLinkOnOpenMock).not.toHaveBeenCalled();
    expect(loadProjectMock).not.toHaveBeenCalled();
    expect(beforeLoad).not.toHaveBeenCalled();
  },
);

it('suppresses a stale download error instead of replacing a newer dialog', async () => {
  const download = deferredOpen<never>();
  const entered = deferredOpen<void>();
  invokeIpcMock.mockImplementation(() => { entered.resolve(); return download.promise; });
  const pending = hook().openRemote(SUMMARY);
  await entered.promise;
  const { reserveProjectRequest } = await import('../utils/projectTransition');
  reserveProjectRequest();
  download.reject(new Error('stale download failure'));
  expect(await pending).toEqual({ status: 'cancelled' });
  expect(useUIStore.getState().openDialog).not.toHaveBeenCalled();
});

it('does not download after a stale discard confirmation is accepted', async () => {
  useProjectStore.setState({ project: { file_path: '/cache/old.stalx' } as never, isDirty: true });
  const warning = deferredOpen<boolean>();
  confirmMock.mockReturnValue(warning.promise);
  const pending = hook().openRemote({ ...SUMMARY, permission: 'READ_ONLY' });
  const { reserveProjectRequest } = await import('../utils/projectTransition');
  reserveProjectRequest();
  warning.resolve(true);
  expect(await pending).toEqual({ status: 'cancelled' });
  expect(invokeIpcMock).not.toHaveBeenCalled();
});

it('does not continue a remote download across login/logout', async () => {
  const download = deferredOpen<{ local_path: string }>();
  const entered = deferredOpen<void>();
  invokeIpcMock.mockImplementation(() => { entered.resolve(); return download.promise; });
  const pending = hook().openRemote(SUMMARY);
  await entered.promise;
  useSpeleoStore.setState((state) => ({ sessionGeneration: state.sessionGeneration + 1 }));
  download.resolve({ local_path: '/cache/p1.stalx' });
  expect(await pending).toEqual({ status: 'cancelled' });
  expect(loadProjectMock).not.toHaveBeenCalled();
});

/**
 * "Open my local copy" - the refused-open prompt's non-destructive answer. It opens
 * the copy already on this computer as it is: nothing is downloaded and nothing is
 * replaced, and the dirty bit rides into the seed so the shell knows the work is
 * there. The load's own Start Editing then re-acquires and runs the divergence check.
 */
describe('openLocalCopy', () => {
  const LOCAL = { project_id: 'p1', local_path: '/cache/p1/project.stalx', last_edit_at: null };

  it('loads the local copy without downloading, seeded as carrying un-uploaded edits', async () => {
    expect(await hook().openLocalCopy(SUMMARY, LOCAL)).toEqual({ status: 'opened' });
    expect(invokeIpcMock.mock.calls.map((c) => c[0])).not.toContain('speleodb_open_project');
    expect(invokeIpcMock.mock.calls.map((c) => c[0])).not.toContain('speleodb_open_project_discarding_local');
    expect(setLinkOnOpenMock).toHaveBeenCalledWith(expect.objectContaining({
      forPath: LOCAL.local_path, hasUnpushedEdits: true, projectId: 'p1',
    }));
    expect(loadProjectMock).toHaveBeenCalledWith(LOCAL.local_path, { accessConfirmed: true, request: expect.any(Number) });
  });

  it('still asks before leaving un-uploaded work under our lock in ANOTHER project', async () => {
    useProjectStore.setState({ project: { file_path: '/cache/p9.stalx' } as never });
    useSpeleoStore.setState({ link: openLink({ editState: 'editing', hasUnpushedEdits: true }), remoteProjectId: 'p9' });
    const pending = hook().openLocalCopy(SUMMARY, LOCAL);
    await vi.waitFor(() => expect(useSpeleoExitPrompt.getState().request).not.toBeNull());
    useSpeleoExitPrompt.getState().request?.settle(false);
    expect(await pending).toEqual({ status: 'cancelled' });
    expect(loadProjectMock).not.toHaveBeenCalled();
  });
});

// Must-fix 1: one account never takes over another account's un-uploaded work.
describe("another account's un-uploaded work", () => {
  const THEIRS = {
    project_id: 'p1', local_path: '/cache/p1/project.stalx', last_edit_at: null, other_owner: 'someone@example.com',
  };

  it('"Open it read-only" loads the copy without taking the lock, and names whose changes they are', async () => {
    expect(await hook().openLocalCopy(SUMMARY, THEIRS)).toEqual({ status: 'opened' });
    expect(invokeIpcMock.mock.calls.map((c) => c[0])).not.toContain('speleodb_open_project');
    expect(loadProjectMock).toHaveBeenCalledWith(THEIRS.local_path, {
      accessConfirmed: true, request: expect.any(Number), acquireLock: false,
    });
    expect(setLinkOnOpenMock).toHaveBeenCalledWith(expect.objectContaining({
      forPath: THEIRS.local_path, hasUnpushedEdits: true, offline: true, projectId: 'p1',
    }));
    expect(useUIStore.getState().showToast).toHaveBeenCalledWith(
      expect.stringContaining('only someone@example.com can edit and upload them'), 'speleodb-read-only-open', 'info');
  });

  it('an offline open the backend marked as another account\'s opens read-only and says whose it is', async () => {
    invokeIpcMock.mockResolvedValue({
      local_path: '/cache/p1/project.stalx', offline: true, has_unpushed_edits: true,
      read_only: 'other_account', other_owner: 'someone@example.com',
    });
    expect(await hook().openRemote(SUMMARY)).toEqual({ status: 'opened' });
    expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1/project.stalx', expect.objectContaining({ acquireLock: false }));
    expect(useUIStore.getState().showToast).toHaveBeenCalledWith(
      expect.stringContaining('changes someone@example.com made'), 'speleodb-read-only-open', 'info');
  });
});
