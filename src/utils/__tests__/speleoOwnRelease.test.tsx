/**
 * A release this session starts itself never reads as "Editing stopped"
 * (pre-PR review of the X3 round, must-fix 1).
 *
 * The status refresh raises that notice when a reply ends the edit session with
 * no lock for this account: another session of the account handed it back. A
 * release THIS session made looks the same to the reply (`edit_enabled: false`,
 * `lock_holder: null`), so each release path either marks the session read-only
 * once its release lands (no edit session left to lose) or runs under the
 * own-release marker (a reply that overlaps it is no loss). One case per path,
 * each with the reply the review saw: no holder, editing off.
 *
 * Synthetic projects only.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, renderHook } from '@testing-library/react';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (e: unknown, f?: string) => (e as { message?: string })?.message ?? f ?? 'error',
}));

import { useSpeleoActions } from '../../hooks/useSpeleoActions';
import { useRevertRevision } from '../../hooks/useRevertRevision';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import type { SpeleoLinkStatus } from '../../types/ipc.generated';
import { useSpeleoReadOnlyNotice } from '../autoAcquireSpeleoLock';
import { releaseOpenProjectLock } from '../releaseOpenProjectLock';
import { settleOpenLockBeforeLeaving } from '../speleoReleaseFailure';
import { releaseSignOutLocks } from '../speleoSignOut';

const ME = 'diver@example.com';
const PATH = '/synthetic/cache/p1/project.stalx';

const status = (o: Partial<SpeleoLinkStatus> = {}): SpeleoLinkStatus => ({
  is_remote: true, edit_enabled: true, has_unpushed_edits: false, unwound: false,
  permission: 'READ_AND_WRITE', lock_holder: ME, remote_state: 'up_to_date',
  access_denied: false, server_error: null, undo_mode: 'full', project_id: 'p1', ...o,
});
/** What a status says once the lock is back on SpeleoDB: no holder, editing off. */
const RELEASED = status({ edit_enabled: false, lock_holder: null });

type Routes = Record<string, (args?: Record<string, unknown>) => unknown>;
let routes: Routes;
/** The first status refresh waits; whoever calls `answerStatus` replies to it. */
let answerStatus: ((s: SpeleoLinkStatus) => void) | null;
let statusCalls: number;

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

beforeEach(() => {
  routes = {};
  answerStatus = null;
  statusCalls = 0;
  invokeIpcMock.mockReset().mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
    if (cmd in routes) return routes[cmd]!(args);
    if (cmd === 'speleodb_link_status') {
      statusCalls += 1;
      return RELEASED;
    }
    return null;
  });
  useProjectStore.setState({
    project: { name: 'Synthetic cave', file_path: PATH, zero_datums: [{}] } as never,
    isDirty: false, isClosing: false, projectTreeGeneration: 0,
    refreshProjectState: vi.fn(async () => {}),
    checkHistoryDiscarded: vi.fn(async () => {}),
    checkFileStatus: vi.fn(async () => {}),
    saveProject: vi.fn(async () => ({ success: true }) as never),
    loadRecentProjects: vi.fn(async () => {}),
  } as never);
  useSpeleoStore.setState({
    session: { loggedIn: true, userEmail: ME, endpoint: 'https://synthetic.example' },
    sessionGeneration: 0, busy: false, divergence: null, banner: null, heldLocks: null,
    remoteProjectId: 'p1', remoteLocks: {},
  });
  useSpeleoStore.getState().syncProjectLink(true, PATH);
  useSpeleoStore.setState({ remoteProjectId: 'p1' });
  // An edit session under this account's lock.
  useSpeleoStore.getState().seedFromStatus(status());
  useSpeleoReadOnlyNotice.setState({ notice: null });
  useUIStore.setState({ showToast: vi.fn(), openDialog: vi.fn() as never });
});

/**
 * A status refresh issued while editing, whose reply (`RELEASED`) arrives while
 * `cmd` - the release - is in flight: `cmd`'s route answers it and lets it land.
 */
function overlapping(cmd: string, reply: unknown): Promise<void> {
  routes.speleodb_link_status = () => {
    statusCalls += 1;
    if (statusCalls > 1) return RELEASED;
    return new Promise<SpeleoLinkStatus>((resolve) => { answerStatus = resolve; });
  };
  routes[cmd] = async () => {
    answerStatus?.(RELEASED);
    await flush();
    return reply;
  };
  return useSpeleoStore.getState().refreshLinkStatus();
}

const notice = () => useSpeleoReadOnlyNotice.getState().notice;

describe('a release this session starts is never "Editing stopped"', () => {
  it('a real loss still says so (the control)', async () => {
    await useSpeleoStore.getState().refreshLinkStatus();
    expect(notice()?.kind).toBe('lockLost');
  });

  // Delta review of X4, should-fix 1: the marker was project-blind, so releasing
  // a stale lock on ANOTHER project (the held-lock prompt, the sign-out loop)
  // while a refresh of the open project was in flight swallowed that project's
  // genuine "Editing stopped" - and once the reply turned it read-only, for good.
  it('a release of another project does not swallow the open project\'s real loss', async () => {
    // p1 is open and editing; another session of the account hands its lock back.
    // The held-lock prompt releases a stale lock on p2 while p1's refresh is out.
    const refresh = overlapping('speleodb_release_lock', null);
    await act(async () => {
      await useSpeleoStore.getState().releaseLock('p2');
      await refresh;
    });
    expect(useSpeleoStore.getState().link?.editState).toBe('read-only');
    expect(notice()?.kind).toBe('lockLost');
  });

  // The review's trace: SpeleoDiscardChangesDialog -> discardChanges(false); the
  // backend hands the lock back and returns null, and the tree replacement's
  // status re-read sees no holder.
  it('Discard and hand back the lock', async () => {
    routes.speleodb_discard_changes = () => null;
    const { result } = renderHook(() => useSpeleoActions());
    let outcome: string | undefined;
    await act(async () => {
      outcome = await result.current.discardChanges(false);
    });
    expect(outcome).toBe('released');
    expect(statusCalls).toBeGreaterThan(0);
    expect(useSpeleoStore.getState().link?.editState).toBe('read-only');
    expect(notice()).toBeNull();
  });

  it('Discard, with a status refresh overlapping it', async () => {
    const refresh = overlapping('speleodb_discard_changes', null);
    const { result } = renderHook(() => useSpeleoActions());
    await act(async () => {
      await result.current.discardChanges(false);
      await refresh;
    });
    expect(notice()).toBeNull();
  });

  it('Done (upload and release), with a status refresh overlapping it', async () => {
    const refresh = overlapping('speleodb_done', { kind: 'released' });
    const { result } = renderHook(() => useSpeleoActions());
    await act(async () => {
      await result.current.done('Synthetic message');
      await refresh;
    });
    expect(useSpeleoStore.getState().link?.editState).toBe('read-only');
    expect(notice()).toBeNull();
  });

  it('the held-lock prompt\'s release of the open project\'s lock', async () => {
    routes.speleodb_release_lock = () => null;
    await act(async () => {
      await useSpeleoStore.getState().releaseLock('p1');
    });
    expect(statusCalls).toBeGreaterThan(0);
    expect(notice()).toBeNull();
  });

  it('Switch to Read Only Mode, with a status refresh overlapping it', async () => {
    routes.speleodb_project_access_at = () => ({ release_context: { project_id: 'p1' } });
    const refresh = overlapping('speleodb_release_current_lock', null);
    await act(async () => {
      await releaseOpenProjectLock();
      await refresh;
    });
    expect(notice()).toBeNull();
  });

  it('the revert, whose upload releases the lock, with a status refresh overlapping it', async () => {
    // The lock stays this account's through the acquire and the restore; the
    // upload hands it back. A refresh issued as the upload starts answers during it.
    let released = false;
    let refresh: Promise<void> | undefined;
    routes.speleodb_start_editing = () => ({ kind: 'editing' });
    routes.speleodb_restore_revision = () => ({ local_path: PATH });
    routes.speleodb_link_status = () => (released ? RELEASED : status());
    routes.speleodb_done = async () => {
      released = true;
      answerStatus?.(RELEASED);
      await flush();
      return { kind: 'released' };
    };
    const onStep = (step: string) => {
      if (step !== 'pushing') return;
      routes.speleodb_link_status = () => {
        routes.speleodb_link_status = () => (released ? RELEASED : status());
        return new Promise<SpeleoLinkStatus>((resolve) => { answerStatus = resolve; });
      };
      refresh = useSpeleoStore.getState().refreshLinkStatus();
    };
    const { result } = renderHook(() => useRevertRevision());
    await act(async () => {
      await result.current({
        projectId: 'p1', projectName: 'Synthetic cave', hexsha: 'abcdef0123',
        shortSha: 'abcdef01', message: 'Synthetic revert', onStep,
      });
      await refresh;
    });
    expect(refresh).toBeDefined();
    expect(useSpeleoStore.getState().link?.editState).toBe('read-only');
    expect(notice()).toBeNull();
  });

  it('the revert\'s unwind of the lock it took, with a status refresh overlapping it', async () => {
    routes.speleodb_start_editing = () => {
      // Another project of the same file name came up during the acquire.
      useSpeleoStore.setState({ remoteProjectId: 'p2' });
      return { kind: 'editing' };
    };
    const refresh = overlapping('speleodb_release_lock_acquired_here', null);
    const { result } = renderHook(() => useRevertRevision());
    await act(async () => {
      await result.current({
        projectId: 'p1', projectName: 'Synthetic cave', hexsha: 'abcdef0123',
        shortSha: 'abcdef01', message: 'Synthetic revert',
      });
      await refresh;
    });
    expect(notice()).toBeNull();
  });

  it('the sign-out\'s hand-back of the open project\'s lock, then a status refresh', async () => {
    routes.speleodb_release_lock = () => null;
    const project = {
      summary: { id: 'p1', name: 'Synthetic cave', permission: 'READ_AND_WRITE', lock_holder: ME, type_tag: 'stalx', latest_commit_sha: null },
      holdsLock: true, claimedHere: true, lastEditAt: null,
    } as const;
    await act(async () => {
      expect(await releaseSignOutLocks([project])).toEqual([]);
      await useSpeleoStore.getState().refreshLinkStatus();
    });
    expect(notice()).toBeNull();
  });

  it('a project switch settling the lock, with a status refresh overlapping it', async () => {
    const refresh = overlapping('speleodb_settle_open_lock', {
      success: true, warnings: [], error: null, line_ids: [], lock_release: 'released',
    });
    await act(async () => {
      expect(await settleOpenLockBeforeLeaving('switch', true)).toBe(true);
      await refresh;
    });
    expect(notice()).toBeNull();
  });

  it('Close Project, with a status refresh overlapping it', async () => {
    const refresh = overlapping('close_project', {
      success: true, warnings: [], error: null, line_ids: [], lock_release: 'released',
    });
    await act(async () => {
      await useProjectStore.getState().closeProject();
      await refresh;
    });
    expect(notice()).toBeNull();
  });
});
