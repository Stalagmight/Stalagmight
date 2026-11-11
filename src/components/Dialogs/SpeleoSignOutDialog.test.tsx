import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const doneMock = vi.hoisted(() => vi.fn());
const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../hooks/useSpeleoActions', () => ({ useSpeleoActions: () => ({ done: doneMock, busy: false }) }));
vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (e: unknown, f?: string) => (e as { message?: string })?.message ?? f ?? 'error',
}));

import SpeleoSignOutDialog from './SpeleoSignOutDialog';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore, type SpeleoLinkState } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { useSpeleoSignOut, type SignOutProject, type SignOutSurvey } from '../../utils/speleoSignOut';
import type { SpeleoProjectSummary } from '../../types/ipc.generated';

const ME = 'diver@example.com';
const ok = { success: true, warnings: [], error: null, section_ids: [] };
const summary = (id: string): SpeleoProjectSummary => ({
  id, name: `Cave ${id}`, permission: 'READ_AND_WRITE', lock_holder: ME, type_tag: 'stalx', latest_commit_sha: null,
});
const entry = (id: string, holdsLock = true): SignOutProject => ({ summary: summary(id), holdsLock, lastEditAt: null });
const link = (o: Partial<SpeleoLinkState> = {}): SpeleoLinkState => ({
  permission: 'READ_AND_WRITE', editState: 'editing', lockHolder: ME, lockedByOther: false,
  hasUnpushedEdits: true, unwound: false, undoMode: 'full', remoteState: 'up_to_date',
  noAccess: false, serverError: null, ...o,
});

let closeProject: ReturnType<typeof vi.fn>;
let released: string[];

/** Open the dialog the way `guardSpeleoSignOut` does; resolves with its answer. */
function show(survey: SignOutSurvey | null, kind: 'signOut' | 'switch' = 'signOut'): Promise<boolean> {
  const answer = new Promise<boolean>((resolve) => {
    useSpeleoSignOut.setState({ request: { id: Math.random(), kind, survey, settle: (go) => {
      useSpeleoSignOut.setState({ request: null });
      resolve(go);
    } } });
  });
  render(<SpeleoSignOutDialog />);
  return answer;
}

beforeEach(() => {
  released = [];
  doneMock.mockReset().mockImplementation(async () => {
    useSpeleoStore.setState({ link: link({ editState: 'read-only', hasUnpushedEdits: false }) });
    return { kind: 'released' };
  });
  invokeIpcMock.mockReset().mockImplementation(async (command: string, args: { projectId: string }) => {
    if (command === 'speleodb_release_lock') released.push(args.projectId);
    if (command === 'speleodb_discard_cached_copy') return ok;
    if (command === 'speleodb_list_projects') return [];
    if (command === 'speleodb_local_project_states') return {};
    return null;
  });
  closeProject = vi.fn(async () => { useProjectStore.setState({ project: null }); return ok; });
  useProjectStore.setState({
    project: { name: 'Cave open', file_path: '/c/open/project.stalx' } as never,
    isDirty: false, closeProject, loadRecentProjects: vi.fn(async () => {}),
  } as never);
  useSpeleoStore.setState({
    session: { loggedIn: true, userEmail: ME, endpoint: 'https://sdb.example' },
    link: link(), remoteProjectId: 'open', banner: null,
  });
  useSpeleoSignOut.setState({ request: null });
});

/**
 * The app's dialog layout (`UnsavedChangesDialog`): every button in the one footer
 * row, in order, and none anywhere else in the body (the header's X aside).
 */
function expectButtonsOnlyInFooter(expected: Array<[testId: string, label: string]>) {
  const dialog = screen.getByTestId('speleodb-signout-dialog');
  const footer = screen.getByTestId('speleodb-signout-footer');
  expect(footer).toHaveClass('flex', 'flex-wrap', 'justify-end', 'gap-2', 'pt-4', 'border-t', 'border-border');
  const inFooter = within(footer).getAllByRole('button');
  expect(inFooter.map((b) => [b.getAttribute('data-testid'), b.textContent])).toEqual(expected);
  const elsewhere = within(dialog).getAllByRole('button')
    .filter((b) => !footer.contains(b) && b.getAttribute('data-testid') !== 'speleodb-signout-dialog-close');
  expect(elsewhere).toEqual([]);
  expect(footer.nextElementSibling).toBeNull();
}

describe('SpeleoSignOutDialog', () => {
  it('nothing to answer: the locks to hand back are listed, with Cancel and Disconnect', async () => {
    const answer = show({ openProject: null, releasable: [entry('a'), entry('b')], unuploaded: [] });
    expect(screen.getByTestId('speleodb-signout-releasable')).toHaveTextContent('Cave a');
    expectButtonsOnlyInFooter([['speleodb-signout-cancel', 'Cancel'], ['speleodb-signout-go', 'Disconnect']]);
    expect(screen.getByTestId('speleodb-signout-go')).toHaveAttribute('data-primary-action', '');
    await userEvent.click(screen.getByTestId('speleodb-signout-go'));
    expect(await answer).toBe(true);
    expect(released).toEqual(['a', 'b']);
  });

  it('open project with changes: Cancel, Discard, Keep on this computer, Upload and disconnect, all in the footer', () => {
    void show({ openProject: entry('open'), releasable: [entry('a')], unuploaded: [entry('x')] });
    expect(screen.getByTestId('speleodb-signout-open')).toHaveTextContent('"Cave open", the project you have open, has changes');
    expectButtonsOnlyInFooter([
      ['speleodb-signout-cancel', 'Cancel'],
      ['speleodb-signout-open-discard', 'Discard'],
      ['speleodb-signout-open-keep', 'Keep on this computer'],
      ['speleodb-signout-open-upload', 'Upload and disconnect'],
    ]);
    expect(screen.getByTestId('speleodb-signout-open-discard')).toHaveClass('bg-error');
    expect(screen.getByTestId('speleodb-signout-open-upload')).toHaveAttribute('data-primary-action', '');
    expect(screen.queryByTestId('speleodb-signout-go')).toBeNull();
  });

  it('an account switch says so on its buttons', () => {
    void show({ openProject: entry('open'), releasable: [], unuploaded: [] }, 'switch');
    expect(screen.getByText('Before you switch accounts')).toBeInTheDocument();
    expect(screen.getByTestId('speleodb-signout-open-upload')).toHaveTextContent('Upload and switch');
  });

  it('without the open project\'s lock there is no Upload; Keep and Discard remain', () => {
    useSpeleoStore.setState({ link: link({ editState: 'read-only' }) });
    void show({ openProject: entry('open'), releasable: [], unuploaded: [] });
    expectButtonsOnlyInFooter([
      ['speleodb-signout-cancel', 'Cancel'],
      ['speleodb-signout-open-discard', 'Discard'],
      ['speleodb-signout-open-keep', 'Keep on this computer'],
    ]);
  });

  it('there is no "keep the lock" answer anywhere in a sign-out', () => {
    void show({ openProject: entry('open'), releasable: [], unuploaded: [entry('x')] });
    expect(screen.queryByText(/keep the lock/i)).toBeNull();
  });

  it('other projects with changes are a plain list of names: kept on this computer, no buttons of their own', async () => {
    const answer = show({ openProject: null, releasable: [entry('a')], unuploaded: [entry('x'), entry('y', false)] });
    const list = screen.getByTestId('speleodb-signout-unuploaded');
    expect(list).toHaveTextContent('The changes stay on this computer, and you can open and upload them after you connect again');
    expect(screen.getByTestId('speleodb-signout-row-x')).toHaveTextContent(/^Cave x$/);
    expect(screen.getByTestId('speleodb-signout-row-y')).toHaveTextContent(/^Cave y$/);
    expect(within(list).queryAllByRole('button')).toEqual([]);
    // The lock held on one of them is handed back with the clean ones, and listed so.
    const handBack = screen.getByTestId('speleodb-signout-releasable');
    expect(handBack).toHaveTextContent('Cave a');
    expect(handBack).toHaveTextContent('Cave x');
    expect(handBack).not.toHaveTextContent('Cave y');
    expectButtonsOnlyInFooter([['speleodb-signout-cancel', 'Cancel'], ['speleodb-signout-go', 'Disconnect']]);
    await userEvent.click(screen.getByTestId('speleodb-signout-go'));
    expect(await answer).toBe(true);
    expect(released).toEqual(['a', 'x']);
    // Never discarded from here, never opened.
    expect(invokeIpcMock).not.toHaveBeenCalledWith('speleodb_discard_cached_copy', expect.anything(), expect.anything());
    expect(closeProject).not.toHaveBeenCalled();
  });

  it('Upload and disconnect: the description step, then the normal upload, the other locks and the sign-out', async () => {
    const answer = show({ openProject: entry('open'), releasable: [entry('a')], unuploaded: [] });
    await userEvent.click(screen.getByTestId('speleodb-signout-open-upload'));
    expectButtonsOnlyInFooter([
      ['speleodb-signout-open-back', 'Back'],
      ['speleodb-signout-open-upload-confirm', 'Upload and disconnect'],
    ]);
    const confirm = screen.getByTestId('speleodb-signout-open-upload-confirm');
    expect(confirm).toBeDisabled();
    await userEvent.type(screen.getByTestId('speleodb-signout-message'), 'Surveyed the sump');
    await userEvent.click(confirm);
    expect(await answer).toBe(true);
    expect(doneMock).toHaveBeenCalledWith('Surveyed the sump', expect.anything());
    // The upload already handed the open project back; only the others are released.
    expect(released).toEqual(['a']);
  });

  it('Back from the upload step returns to the choices', async () => {
    void show({ openProject: entry('open'), releasable: [], unuploaded: [] });
    await userEvent.click(screen.getByTestId('speleodb-signout-open-upload'));
    await userEvent.click(screen.getByTestId('speleodb-signout-open-back'));
    expect(screen.getByTestId('speleodb-signout-open-keep')).toBeVisible();
    expect(doneMock).not.toHaveBeenCalled();
  });

  it('a failed upload stays on the upload step with the failure on screen, and does not sign out', async () => {
    doneMock.mockImplementation(async () => {
      useSpeleoStore.getState().raisePushFailedBanner({ message: 'Synthetic offline', origin: 'done', pushMessage: 'm' });
      return null;
    });
    let settled: boolean | null = null;
    void show({ openProject: entry('open'), releasable: [entry('a')], unuploaded: [] }).then((v) => { settled = v; });
    await userEvent.click(screen.getByTestId('speleodb-signout-open-upload'));
    await userEvent.type(screen.getByTestId('speleodb-signout-message'), 'm');
    await userEvent.click(screen.getByTestId('speleodb-signout-open-upload-confirm'));
    expect(await screen.findByTestId('speleodb-push-failure-inline')).toHaveTextContent('Synthetic offline');
    expect(screen.getByTestId('speleodb-signout-open-upload-confirm')).toBeEnabled();
    expect(settled).toBeNull();
    expect(released).toEqual([]);
  });

  it('a divergence cancels the sign-out so its own dialog can take over', async () => {
    doneMock.mockResolvedValue({ kind: 'divergence_needed' });
    const answer = show({ openProject: entry('open'), releasable: [entry('a')], unuploaded: [] });
    await userEvent.click(screen.getByTestId('speleodb-signout-open-upload'));
    await userEvent.type(screen.getByTestId('speleodb-signout-message'), 'm');
    await userEvent.click(screen.getByTestId('speleodb-signout-open-upload-confirm'));
    expect(await answer).toBe(false);
    expect(released).toEqual([]);
  });

  it('Discard: a "lost forever" step listing what is lost, then the discarding close and the sign-out', async () => {
    useProjectStore.setState({ isDirty: true });
    const answer = show({ openProject: entry('open'), releasable: [entry('a')], unuploaded: [entry('x')] });
    await userEvent.click(screen.getByTestId('speleodb-signout-open-discard'));
    expect(closeProject).not.toHaveBeenCalled();
    expect(screen.getByText('Are you sure?')).toBeInTheDocument();
    expect(screen.getByTestId('speleodb-signout-open-discard-body')).toHaveTextContent('Your changes to "Cave open" will be lost forever');
    const lost = screen.getByTestId('speleodb-signout-discard-lost');
    expect(lost).toHaveTextContent('the changes you have not saved yet');
    expect(lost).toHaveTextContent('the changes saved on this computer that are not on SpeleoDB');
    // It applies to the open project only.
    expect(screen.getByTestId('speleodb-signout-discard')).toHaveTextContent('Only this project is affected');
    expectButtonsOnlyInFooter([
      ['speleodb-signout-open-discard-back', 'Back'],
      ['speleodb-signout-open-discard-confirm', 'Discard and disconnect'],
    ]);
    expect(screen.getByTestId('speleodb-signout-open-discard-confirm')).toHaveClass('bg-error');
    await userEvent.click(screen.getByTestId('speleodb-signout-open-discard-confirm'));
    expect(await answer).toBe(true);
    expect(closeProject).toHaveBeenCalledWith({ discardLocal: true });
    expect(released).toEqual(['a', 'x']);
    expect(invokeIpcMock).not.toHaveBeenCalledWith('speleodb_discard_cached_copy', expect.anything(), expect.anything());
  });

  it.each([
    ['speleodb_discard_failed', 'Synthetic: the copy is still there, and the project stays reserved for you on SpeleoDB.'],
    ['delete_project_lock_release_failed', 'Synthetic: the SpeleoDB lock could not be handed back.'],
  ])('a discarding close that leaves the lock held (%s) says so, then signs out', async (code, message) => {
    const showToast = vi.fn();
    useUIStore.setState({ showToast: showToast as never });
    closeProject.mockImplementation(async () => {
      useProjectStore.setState({ project: null });
      return { ...ok, warnings: [{ code, message, severity: 'warning' }] };
    });
    const answer = show({ openProject: entry('open'), releasable: [entry('a')], unuploaded: [] });
    await userEvent.click(screen.getByTestId('speleodb-signout-open-discard'));
    await userEvent.click(screen.getByTestId('speleodb-signout-open-discard-confirm'));
    expect(await answer).toBe(true);
    // The same surface and words as the exit prompt: the backend's warning, as a toast.
    expect(showToast).toHaveBeenCalledOnce();
    expect(showToast).toHaveBeenCalledWith(message, 'speleodb-lock', 'warning');
    expect(closeProject).toHaveBeenCalledWith({ discardLocal: true });
    // The sign-out carries on: the other locks are handed back as before.
    expect(released).toEqual(['a']);
  });

  it('a clean discarding close shows no warning', async () => {
    const showToast = vi.fn();
    useUIStore.setState({ showToast: showToast as never });
    const answer = show({ openProject: entry('open'), releasable: [], unuploaded: [] });
    await userEvent.click(screen.getByTestId('speleodb-signout-open-discard'));
    await userEvent.click(screen.getByTestId('speleodb-signout-open-discard-confirm'));
    expect(await answer).toBe(true);
    expect(showToast).not.toHaveBeenCalled();
  });

  it('Back from the discard step discards nothing', async () => {
    void show({ openProject: entry('open'), releasable: [], unuploaded: [] });
    await userEvent.click(screen.getByTestId('speleodb-signout-open-discard'));
    await userEvent.click(screen.getByTestId('speleodb-signout-open-discard-back'));
    expect(screen.getByTestId('speleodb-signout-open-discard')).toBeVisible();
    expect(closeProject).not.toHaveBeenCalled();
  });

  it('a discard that fails stays on the confirmation with the reason, and does not sign out', async () => {
    closeProject.mockResolvedValue({ ...ok, success: false, error: 'Synthetic close failure' });
    let settled: boolean | null = null;
    void show({ openProject: entry('open'), releasable: [], unuploaded: [] }).then((v) => { settled = v; });
    await userEvent.click(screen.getByTestId('speleodb-signout-open-discard'));
    await userEvent.click(screen.getByTestId('speleodb-signout-open-discard-confirm'));
    expect(await screen.findByTestId('speleodb-signout-error')).toHaveTextContent('Synthetic close failure');
    expect(screen.getByTestId('speleodb-signout-open-discard-confirm')).toBeEnabled();
    expect(settled).toBeNull();
  });

  // Decision 10: when the upload is not possible, the edits are never discard-only.
  it('Keep on this computer saves the open project locally, then hands every lock back and signs out', async () => {
    const saveProject = vi.fn(async () => { useProjectStore.setState({ isDirty: false }); return ok; });
    useProjectStore.setState({ isDirty: true, saveProject } as never);
    const answer = show({ openProject: entry('open'), releasable: [entry('a')], unuploaded: [entry('x'), entry('y', false)] });
    await userEvent.click(screen.getByTestId('speleodb-signout-open-keep'));
    expect(await answer).toBe(true);
    expect(saveProject).toHaveBeenCalledOnce();
    // Kept, not closed, discarded or uploaded.
    expect(closeProject).not.toHaveBeenCalled();
    expect(doneMock).not.toHaveBeenCalled();
    expect(invokeIpcMock).not.toHaveBeenCalledWith('speleodb_discard_cached_copy', expect.anything(), expect.anything());
    expect(released).toEqual(['a', 'x', 'open']);
  });

  it('Keep stays put when the local save fails, and says why', async () => {
    const saveProject = vi.fn(async () => ({ success: false, error: 'Synthetic disk failure' }));
    useProjectStore.setState({ isDirty: true, saveProject } as never);
    let settled: boolean | null = null;
    void show({ openProject: entry('open'), releasable: [entry('a')], unuploaded: [] }).then((v) => { settled = v; });
    await userEvent.click(screen.getByTestId('speleodb-signout-open-keep'));
    expect(await screen.findByTestId('speleodb-signout-error')).toHaveTextContent('could not be saved');
    expect(screen.getByTestId('speleodb-signout-open-keep')).toBeEnabled();
    expect(settled).toBeNull();
    expect(released).toEqual([]);
  });

  it('while an answer is carried out the whole row is disabled, and the pressed button says what it is doing', async () => {
    let finish!: () => void;
    invokeIpcMock.mockImplementation((command: string, args: { projectId: string }) => {
      if (command !== 'speleodb_release_lock') return Promise.resolve(null);
      return new Promise((resolve) => { finish = () => { released.push(args.projectId); resolve(null); }; });
    });
    const answer = show({ openProject: entry('open'), releasable: [], unuploaded: [] });
    await userEvent.click(screen.getByTestId('speleodb-signout-open-keep'));
    expect(await screen.findByTestId('speleodb-signout-releasing')).toBeVisible();
    expect(screen.getByTestId('speleodb-signout-open-keep')).toHaveTextContent('Handing back');
    for (const id of ['speleodb-signout-cancel', 'speleodb-signout-open-discard', 'speleodb-signout-open-keep', 'speleodb-signout-open-upload']) {
      expect(screen.getByTestId(id)).toBeDisabled();
    }
    await userEvent.keyboard('{Escape}');
    finish();
    expect(await answer).toBe(true);
  });

  it('a kept project whose hand-back fails gets Try again / Disconnect anyway in the same footer row', async () => {
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'speleodb_release_lock') throw new Error('Synthetic offline');
      return null;
    });
    const answer = show({ openProject: entry('open'), releasable: [], unuploaded: [] });
    await userEvent.click(screen.getByTestId('speleodb-signout-open-keep'));
    expect(await screen.findByTestId('speleodb-signout-release-failed')).toHaveTextContent('Cave open');
    expectButtonsOnlyInFooter([
      ['speleodb-signout-cancel', 'Cancel'],
      ['speleodb-signout-anyway', 'Disconnect anyway'],
      ['speleodb-signout-retry', 'Try again'],
    ]);
    await userEvent.click(screen.getByTestId('speleodb-signout-anyway'));
    expect(await answer).toBe(true);
  });

  it('a release that fails is reported; Try again retries only what failed', async () => {
    let failB = true;
    invokeIpcMock.mockImplementation(async (command: string, args: { projectId: string }) => {
      if (command === 'speleodb_release_lock') {
        if (args.projectId === 'b' && failB) throw new Error('Synthetic offline');
        released.push(args.projectId);
      }
      return null;
    });
    const answer = show({ openProject: null, releasable: [entry('a'), entry('b')], unuploaded: [] });
    await userEvent.click(screen.getByTestId('speleodb-signout-go'));
    const failed = await screen.findByTestId('speleodb-signout-release-failed');
    expect(failed).toHaveTextContent('Cave b');
    expect(failed).not.toHaveTextContent('Cave a');
    expect(failed).toHaveTextContent('until you connect again');
    failB = false;
    await userEvent.click(screen.getByTestId('speleodb-signout-retry'));
    expect(await answer).toBe(true);
    expect(released).toEqual(['a', 'b']);
  });

  it('"Disconnect anyway" after a failed release goes ahead', async () => {
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'speleodb_release_lock') throw new Error('Synthetic offline');
      return null;
    });
    const answer = show({ openProject: null, releasable: [entry('a')], unuploaded: [] });
    await userEvent.click(screen.getByTestId('speleodb-signout-go'));
    await screen.findByTestId('speleodb-signout-release-failed');
    await userEvent.click(screen.getByTestId('speleodb-signout-anyway'));
    expect(await answer).toBe(true);
  });

  it('when SpeleoDB could not be checked: Try again re-checks, and signs out if nothing is held', async () => {
    const answer = show(null);
    expect(screen.getByTestId('speleodb-signout-check-failed')).toBeVisible();
    expectButtonsOnlyInFooter([
      ['speleodb-signout-cancel', 'Cancel'],
      ['speleodb-signout-anyway', 'Disconnect anyway'],
      ['speleodb-signout-retry', 'Try again'],
    ]);
    await userEvent.click(screen.getByTestId('speleodb-signout-retry'));
    expect(await answer).toBe(true);
    expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_list_projects', undefined, expect.anything());
  });

  it('Cancel aborts the sign-out and releases nothing', async () => {
    const answer = show({ openProject: null, releasable: [entry('a')], unuploaded: [] }, 'switch');
    expect(screen.getByTestId('speleodb-signout-go')).toHaveTextContent('Switch account');
    await userEvent.click(screen.getByTestId('speleodb-signout-cancel'));
    expect(await answer).toBe(false);
    expect(released).toEqual([]);
  });

  it('Escape cancels too', async () => {
    const answer = show({ openProject: entry('open'), releasable: [entry('a')], unuploaded: [] });
    await userEvent.keyboard('{Escape}');
    expect(await answer).toBe(false);
    expect(released).toEqual([]);
  });
});
