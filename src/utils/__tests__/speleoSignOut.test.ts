/**
 * The sign-out survey: what signing out would strand, sorted into the three groups
 * the sign-out dialog acts on. Synthetic projects only.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (e: unknown, f?: string) => (e as { message?: string })?.message ?? f ?? 'error',
}));

import { guardSpeleoSignOut, releaseSignOutLocks, signOutHasWork, surveySignOut, useSpeleoSignOut } from '../speleoSignOut';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore, type SpeleoLinkState } from '../../store/speleoStore';
import type { SpeleoLocalProjectState, SpeleoProjectSummary } from '../../types/ipc.generated';

const ME = 'diver@example.com';
const summary = (id: string, lockHolder: string | null = null): SpeleoProjectSummary => ({
  id, name: `Cave ${id}`, permission: 'READ_AND_WRITE', lock_holder: lockHolder, type_tag: 'stalx', latest_commit_sha: null,
});
const localState = (o: Partial<SpeleoLocalProjectState> = {}): SpeleoLocalProjectState => ({
  has_local_copy: true, has_unpushed_edits: false, last_edit_at: null, is_open: false, state_unknown: false, ...o,
});
const link = (o: Partial<SpeleoLinkState> = {}): SpeleoLinkState => ({
  permission: 'READ_AND_WRITE', editState: 'editing', lockHolder: ME, lockedByOther: false,
  hasUnpushedEdits: false, unwound: false, undoMode: 'full', remoteState: 'up_to_date',
  noAccess: false, serverError: null, ...o,
});

function serve(listed: SpeleoProjectSummary[], local: Record<string, SpeleoLocalProjectState>) {
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'speleodb_list_projects') return listed;
    if (command === 'speleodb_local_project_states') return local;
    return null;
  });
}

beforeEach(() => {
  invokeIpcMock.mockReset();
  useSpeleoSignOut.setState({ request: null });
  useProjectStore.setState({ project: null, isDirty: false });
  useSpeleoStore.setState({
    session: { loggedIn: true, userEmail: ME, endpoint: 'https://sdb.example' },
    link: null,
    remoteProjectId: null,
  });
});

describe('surveySignOut', () => {
  it('nothing held and nothing un-uploaded: nothing to do', async () => {
    serve([summary('a'), summary('b', 'someone@else.org')], { a: localState(), b: localState() });
    const survey = await surveySignOut();
    expect(signOutHasWork(survey)).toBe(false);
  });

  it('the open project with un-uploaded work under our lock needs an answer', async () => {
    useProjectStore.setState({ project: { file_path: '/c/a/project.stalx' } as never, isDirty: false });
    useSpeleoStore.setState({ link: link({ hasUnpushedEdits: true }), remoteProjectId: 'a' });
    serve([summary('a', ME)], { a: localState({ has_unpushed_edits: true, is_open: true }) });
    const survey = await surveySignOut();
    expect(survey.openProject?.summary.id).toBe('a');
    expect(survey.releasable).toEqual([]);
    expect(survey.unuploaded).toEqual([]);
  });

  it('locks with nothing un-uploaded - open or not - are simply handed back', async () => {
    useProjectStore.setState({ project: { file_path: '/c/a/project.stalx' } as never });
    useSpeleoStore.setState({ link: link({ hasUnpushedEdits: false }), remoteProjectId: 'a' });
    serve([summary('a', ME), summary('b', 'DIVER@example.com'), summary('c')],
      { a: localState({ is_open: true }), b: localState(), c: localState() });
    const survey = await surveySignOut();
    expect(survey.openProject).toBeNull();
    expect(survey.releasable.map((p) => p.summary.id)).toEqual(['a', 'b']);
    expect(survey.unuploaded).toEqual([]);
  });

  it('other cached projects with un-uploaded work are listed, locked or not; unreadable ones too', async () => {
    serve([summary('a', ME), summary('b'), summary('c', ME)], {
      a: localState({ has_unpushed_edits: true, last_edit_at: '2026-11-09T10:00:00Z' }),
      b: localState({ has_unpushed_edits: true }),
      c: localState({ has_local_copy: false, state_unknown: true }),
    });
    const survey = await surveySignOut();
    expect(survey.unuploaded.map((p) => [p.summary.id, p.holdsLock])).toEqual([['a', true], ['b', false], ['c', true]]);
    expect(survey.unuploaded[0]?.lastEditAt).toBe('2026-11-09T10:00:00Z');
    expect(survey.releasable).toEqual([]);
  });

  it('a stranded open project (un-uploaded work, no lock) is an un-uploaded row, not the open-project answer', async () => {
    useProjectStore.setState({ project: { file_path: '/c/a/project.stalx' } as never });
    useSpeleoStore.setState({ link: link({ editState: 'read-only', hasUnpushedEdits: true }), remoteProjectId: 'a' });
    serve([summary('a')], { a: localState({ has_unpushed_edits: true, is_open: true }) });
    const survey = await surveySignOut();
    expect(survey.openProject).toBeNull();
    expect(survey.unuploaded.map((p) => p.summary.id)).toEqual(['a']);
  });

  it('a server that cannot be asked is an error, never "no locks"', async () => {
    invokeIpcMock.mockRejectedValue(new Error('Synthetic: unreachable'));
    await expect(surveySignOut()).rejects.toThrow('unreachable');
  });
});

describe('guardSpeleoSignOut', () => {
  it('signs out directly when there is nothing to do', async () => {
    serve([summary('a')], { a: localState() });
    expect(await guardSpeleoSignOut('signOut')).toBe(true);
    expect(useSpeleoSignOut.getState().request).toBeNull();
  });

  it('asks nothing when no one is signed in', async () => {
    useSpeleoStore.setState({ session: null });
    expect(await guardSpeleoSignOut('signOut')).toBe(true);
    expect(invokeIpcMock).not.toHaveBeenCalled();
  });

  it('raises the dialog with the survey when something is held, and waits for it', async () => {
    serve([summary('a', ME)], { a: localState() });
    const answer = guardSpeleoSignOut('switch');
    await vi.waitFor(() => expect(useSpeleoSignOut.getState().request).not.toBeNull());
    const request = useSpeleoSignOut.getState().request;
    expect(request?.kind).toBe('switch');
    expect(request?.survey?.releasable.map((p) => p.summary.id)).toEqual(['a']);
    request?.settle(false);
    expect(await answer).toBe(false);
  });

  it('raises the dialog in its "could not check" state when the server cannot be asked', async () => {
    invokeIpcMock.mockRejectedValue(new Error('Synthetic: unreachable'));
    const answer = guardSpeleoSignOut('signOut');
    await vi.waitFor(() => expect(useSpeleoSignOut.getState().request).not.toBeNull());
    expect(useSpeleoSignOut.getState().request?.survey).toBeNull();
    useSpeleoSignOut.getState().request?.settle(true);
    expect(await answer).toBe(true);
  });
});

describe('releaseSignOutLocks', () => {
  it('releases each lock explicitly and reports the ones that failed', async () => {
    invokeIpcMock.mockImplementation(async (command: string, args: { projectId: string }) => {
      if (command === 'speleodb_release_lock' && args.projectId === 'b') throw new Error('Synthetic: offline');
      return null;
    });
    const projects = [summary('a', ME), summary('b', ME)].map((s) => ({ summary: s, holdsLock: true, lastEditAt: null }));
    const failed = await releaseSignOutLocks(projects);
    expect(failed.map((p) => p.summary.id)).toEqual(['b']);
    expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_release_lock', { projectId: 'a' }, expect.anything());
    expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_release_lock', { projectId: 'b' }, expect.anything());
  });

  it('a release that never answers counts as failed once the bound runs out', async () => {
    vi.useFakeTimers();
    try {
      invokeIpcMock.mockImplementation(() => new Promise(() => {}));
      const pending = releaseSignOutLocks([{ summary: summary('a', ME), holdsLock: true, lastEditAt: null }]);
      await vi.advanceTimersByTimeAsync(25_000);
      expect((await pending).map((p) => p.summary.id)).toEqual(['a']);
    } finally {
      vi.useRealTimers();
    }
  });
});
