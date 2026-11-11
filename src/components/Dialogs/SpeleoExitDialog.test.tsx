import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const doneMock = vi.hoisted(() => vi.fn());
vi.mock('../../hooks/useSpeleoActions', () => ({ useSpeleoActions: () => ({ done: doneMock, busy: false }) }));

import SpeleoExitDialog from './SpeleoExitDialog';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore, type SpeleoLinkState } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { askSpeleoExit, resetSpeleoExitPromptForTests, useSpeleoExitPrompt } from '../../utils/speleoExit';

const ok = { success: true, warnings: [], error: null, section_ids: [] };
const link = (o: Partial<SpeleoLinkState> = {}): SpeleoLinkState => ({
  permission: 'READ_AND_WRITE', editState: 'editing', lockHolder: null, lockedByOther: false,
  hasUnpushedEdits: true, unwound: false, undoMode: 'full', remoteState: 'up_to_date',
  noAccess: false, serverError: null, ...o,
});

let saveProject: ReturnType<typeof vi.fn>;
let closeProject: ReturnType<typeof vi.fn>;
let showToast: ReturnType<typeof vi.fn>;

beforeEach(() => {
  resetSpeleoExitPromptForTests();
  doneMock.mockReset().mockImplementation(async () => {
    useSpeleoStore.setState({ link: link({ editState: 'read-only', hasUnpushedEdits: false }) });
    useProjectStore.setState({ isDirty: false });
    return { kind: 'released' };
  });
  saveProject = vi.fn(async () => ok);
  closeProject = vi.fn(async () => {
    useProjectStore.setState({ project: null });
    return ok;
  });
  showToast = vi.fn();
  useProjectStore.setState({
    project: { name: 'Synthetic cave', file_path: '/synthetic/cache/project.stalx' } as never,
    isDirty: false, saveProject, closeProject,
  } as never);
  useSpeleoStore.setState({ link: link(), banner: null });
  useUIStore.setState({ showToast: showToast as never, activeDialog: null, dialogData: {} });
});

/** Raise the prompt the way an exit does and render it. */
function raise(reason: 'close' | 'switch' | 'quit' = 'close') {
  const answer = askSpeleoExit(reason);
  render(<SpeleoExitDialog />);
  return answer;
}

/**
 * The app's dialog layout (`UnsavedChangesDialog`): every button in the one footer
 * row, in order, and none anywhere else in the body (the header's X aside).
 */
function expectButtonsOnlyInFooter(expected: Array<[testId: string, label: string]>) {
  const dialog = screen.getByTestId('speleodb-exit-dialog');
  const footer = screen.getByTestId('speleodb-exit-footer');
  expect(footer).toHaveClass('flex', 'flex-wrap', 'justify-end', 'gap-2', 'pt-4', 'border-t', 'border-border');
  const inFooter = within(footer).getAllByRole('button');
  expect(inFooter.map((b) => [b.getAttribute('data-testid'), b.textContent])).toEqual(expected);
  const elsewhere = within(dialog).getAllByRole('button')
    .filter((b) => !footer.contains(b) && b.getAttribute('data-testid') !== 'speleodb-exit-dialog-close');
  expect(elsewhere).toEqual([]);
  // The footer is the last thing in the body: nothing (no hint) sits under a button.
  expect(footer.nextElementSibling).toBeNull();
}

describe('SpeleoExitDialog', () => {
  it('offers Upload, Keep the lock, Discard and Cancel, in plain words for the exit at hand', () => {
    void raise('quit');
    expect(screen.getByTestId('speleodb-exit-dialog')).toBeVisible();
    expect(screen.getByTestId('speleodb-exit-body')).toHaveTextContent('Synthetic cave');
    expect(screen.getByTestId('speleodb-exit-body')).toHaveTextContent('Stalagmight closes');
    expect(screen.getByTestId('speleodb-exit-upload')).toHaveAttribute('data-primary-action', '');
    expect(screen.getByTestId('speleodb-exit-keep')).toBeEnabled();
    expect(screen.getByTestId('speleodb-exit-discard')).toBeEnabled();
    expect(screen.getByTestId('speleodb-exit-cancel')).toBeEnabled();
  });

  it('lays the choices out like every other dialog: one footer row, Cancel first, Upload last', () => {
    void raise();
    expectButtonsOnlyInFooter([
      ['speleodb-exit-cancel', 'Cancel'],
      ['speleodb-exit-discard', 'Discard'],
      ['speleodb-exit-keep', 'Keep the lock'],
      ['speleodb-exit-upload', 'Upload'],
    ]);
    expect(screen.getByTestId('speleodb-exit-discard')).toHaveClass('bg-error');
    expect(screen.getByTestId('speleodb-exit-upload')).toHaveClass('bg-accent');
    expect(screen.getByTestId('speleodb-exit-keep')).not.toHaveClass('bg-accent');
    // What each answer does is said once, in the body, by the buttons' own names.
    const choices = screen.getByTestId('speleodb-exit-choices');
    expect(choices).toHaveTextContent('Upload sends them to SpeleoDB and hands the project back');
    expect(choices).toHaveTextContent('Keep the lock leaves them on this computer');
    expect(choices).toHaveTextContent('Discard throws them away');
  });

  it('the upload and discard steps keep their buttons in the same footer row', async () => {
    void raise();
    await userEvent.click(screen.getByTestId('speleodb-exit-upload'));
    expectButtonsOnlyInFooter([
      ['speleodb-exit-back', 'Back'],
      ['speleodb-exit-upload-confirm', 'Upload and continue'],
    ]);
    await userEvent.click(screen.getByTestId('speleodb-exit-back'));
    await userEvent.click(screen.getByTestId('speleodb-exit-discard'));
    expectButtonsOnlyInFooter([
      ['speleodb-exit-back', 'Back'],
      ['speleodb-exit-discard-confirm', 'Yes, discard my changes'],
    ]);
    expect(screen.getByTestId('speleodb-exit-discard-confirm')).toHaveClass('bg-error');
  });

  it('while an answer is carried out, the whole row is disabled and the pressed one says so', async () => {
    let finish!: (value: unknown) => void;
    closeProject.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
    void raise();
    await userEvent.click(screen.getByTestId('speleodb-exit-keep'));
    await waitFor(() => expect(screen.getByTestId('speleodb-exit-keep')).toHaveTextContent('Working'));
    for (const id of ['speleodb-exit-cancel', 'speleodb-exit-discard', 'speleodb-exit-keep', 'speleodb-exit-upload']) {
      expect(screen.getByTestId(id)).toBeDisabled();
    }
    await act(async () => { useProjectStore.setState({ project: null }); finish(ok); });
  });

  it('Upload runs the normal upload with the description, then closes and lets the exit go on', async () => {
    const answer = raise();
    await userEvent.click(screen.getByTestId('speleodb-exit-upload'));
    const confirm = screen.getByTestId('speleodb-exit-upload-confirm');
    expect(confirm).toBeDisabled();
    await userEvent.type(screen.getByTestId('speleodb-exit-message'), 'Surveyed the north passage');
    await userEvent.click(confirm);
    expect(await answer).toBe(true);
    expect(doneMock).toHaveBeenCalledWith('Surveyed the north passage', expect.objectContaining({ isCurrent: expect.any(Function) }));
    expect(closeProject).toHaveBeenCalledWith({});
    expect(useSpeleoExitPrompt.getState().request).toBeNull();
  });

  it('a failed upload stays in the project with the failure on screen, and does not exit', async () => {
    doneMock.mockImplementation(async () => {
      useSpeleoStore.getState().raisePushFailedBanner({ message: 'Synthetic network failure', origin: 'done', pushMessage: 'm' });
      return null;
    });
    let settled = false;
    void raise().then(() => { settled = true; });
    await userEvent.click(screen.getByTestId('speleodb-exit-upload'));
    await userEvent.type(screen.getByTestId('speleodb-exit-message'), 'm');
    await userEvent.click(screen.getByTestId('speleodb-exit-upload-confirm'));
    expect(await screen.findByTestId('speleodb-push-failure-inline')).toHaveTextContent('Synthetic network failure');
    expect(closeProject).not.toHaveBeenCalled();
    expect(settled).toBe(false);
    expect(screen.getByTestId('speleodb-exit-upload-confirm')).toBeEnabled();
  });

  it('a divergence hands over to its own dialog and cancels the exit', async () => {
    doneMock.mockResolvedValue({ kind: 'divergence_needed' });
    const answer = raise();
    await userEvent.click(screen.getByTestId('speleodb-exit-upload'));
    await userEvent.type(screen.getByTestId('speleodb-exit-message'), 'm');
    await userEvent.click(screen.getByTestId('speleodb-exit-upload-confirm'));
    expect(await answer).toBe(false);
    expect(closeProject).not.toHaveBeenCalled();
  });

  it('an older-version copy says so before the upload, and the upload is the informed choice', async () => {
    useSpeleoStore.setState({ link: link({ unwound: true }) });
    void raise();
    await userEvent.click(screen.getByTestId('speleodb-exit-upload'));
    expect(screen.getByTestId('speleodb-exit-older-version')).toBeVisible();
    await userEvent.type(screen.getByTestId('speleodb-exit-message'), 'm');
    await userEvent.click(screen.getByTestId('speleodb-exit-upload-confirm'));
    await waitFor(() => expect(doneMock).toHaveBeenCalledWith('m', expect.objectContaining({ acceptOlderVersion: true })));
  });

  it('Keep the lock saves unsaved edits first, then closes WITHOUT releasing', async () => {
    useProjectStore.setState({ isDirty: true });
    const answer = raise();
    await userEvent.click(screen.getByTestId('speleodb-exit-keep'));
    expect(await answer).toBe(true);
    expect(saveProject).toHaveBeenCalledTimes(1);
    expect(closeProject).toHaveBeenCalledWith({ releaseLock: false });
    expect(saveProject.mock.invocationCallOrder[0]).toBeLessThan(closeProject.mock.invocationCallOrder[0]!);
    expect(doneMock).not.toHaveBeenCalled();
  });

  it('Keep the lock with nothing unsaved goes straight to the keeping close', async () => {
    const answer = raise();
    await userEvent.click(screen.getByTestId('speleodb-exit-keep'));
    expect(await answer).toBe(true);
    expect(saveProject).not.toHaveBeenCalled();
    expect(closeProject).toHaveBeenCalledWith({ releaseLock: false });
  });

  it('a failed save keeps the project open and says why', async () => {
    useProjectStore.setState({ isDirty: true });
    saveProject.mockResolvedValue({ ...ok, success: false, error: 'Synthetic disk full' });
    let settled = false;
    void raise().then(() => { settled = true; });
    await userEvent.click(screen.getByTestId('speleodb-exit-keep'));
    expect(await screen.findByTestId('speleodb-exit-error')).toHaveTextContent('Synthetic disk full');
    expect(closeProject).not.toHaveBeenCalled();
    expect(settled).toBe(false);
  });

  it('Discard needs a second "lost forever" confirmation, then discards the local copy', async () => {
    const answer = raise();
    await userEvent.click(screen.getByTestId('speleodb-exit-discard'));
    expect(closeProject).not.toHaveBeenCalled();
    expect(screen.getByTestId('speleodb-exit-discard-body')).toHaveTextContent('Your changes will be lost forever.');
    await userEvent.click(screen.getByTestId('speleodb-exit-back'));
    expect(screen.getByTestId('speleodb-exit-discard')).toBeVisible();
    await userEvent.click(screen.getByTestId('speleodb-exit-discard'));
    await userEvent.click(screen.getByTestId('speleodb-exit-discard-confirm'));
    expect(await answer).toBe(true);
    expect(closeProject).toHaveBeenCalledWith({ discardLocal: true });
  });

  it('a discard the backend could only half do still exits, and says what was kept', async () => {
    closeProject.mockImplementation(async () => {
      useProjectStore.setState({ project: null });
      return { ...ok, warnings: [{ code: 'speleodb_discard_failed', message: 'Synthetic: kept', severity: 'warning' }] };
    });
    const answer = raise();
    await userEvent.click(screen.getByTestId('speleodb-exit-discard'));
    await userEvent.click(screen.getByTestId('speleodb-exit-discard-confirm'));
    expect(await answer).toBe(true);
    expect(showToast).toHaveBeenCalledWith('Synthetic: kept', expect.any(String), 'warning');
  });

  it('Cancel aborts the exit and changes nothing', async () => {
    const answer = raise();
    await userEvent.click(screen.getByTestId('speleodb-exit-cancel'));
    expect(await answer).toBe(false);
    expect(closeProject).not.toHaveBeenCalled();
    expect(saveProject).not.toHaveBeenCalled();
    expect(doneMock).not.toHaveBeenCalled();
  });

  it('Escape cancels too', async () => {
    const answer = raise();
    await userEvent.keyboard('{Escape}');
    expect(await answer).toBe(false);
  });

  it('cannot be dismissed while an answer is being carried out', async () => {
    let finish!: (value: unknown) => void;
    closeProject.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
    let settled: boolean | null = null;
    void raise().then((value) => { settled = value; });
    await userEvent.click(screen.getByTestId('speleodb-exit-keep'));
    await waitFor(() => expect(screen.getByTestId('speleodb-exit-cancel')).toBeDisabled());
    await userEvent.keyboard('{Escape}');
    expect(settled).toBeNull();
    // A second exit request meanwhile is refused rather than replacing this one.
    expect(await askSpeleoExit('quit')).toBe(false);
    await act(async () => { useProjectStore.setState({ project: null }); finish(ok); });
    await waitFor(() => expect(settled).toBe(true));
  });

  it('an upload walked away from still has its result applied, and a late failure is toasted (frontend#2)', async () => {
    let finish!: (value: unknown) => void;
    let opts!: { isCurrent: () => boolean };
    doneMock.mockImplementation((_message: string, o: { isCurrent: () => boolean }) => {
      opts = o;
      return new Promise((resolve) => { finish = resolve; });
    });
    let settled: boolean | null = null;
    void raise('quit').then((value) => { settled = value; });
    await userEvent.click(screen.getByTestId('speleodb-exit-upload'));
    await userEvent.type(screen.getByTestId('speleodb-exit-message'), 'm');
    vi.useFakeTimers();
    try {
      await act(async () => screen.getByTestId('speleodb-exit-upload-confirm').click());
      act(() => vi.advanceTimersByTime(120_000));
      expect(screen.getByTestId('speleodb-exit-upload-slow')).toBeVisible();
      // Walk away, which only a slow upload allows (Escape / X on the prompt).
      await act(async () => { useSpeleoExitPrompt.getState().request?.settle(false); });
      expect(settled).toBe(false);
      // The prompt is gone, but the upload is still applied for this project.
      expect(opts.isCurrent()).toBe(true);
      await act(async () => finish(null));
      expect(showToast).toHaveBeenCalledWith(expect.stringContaining('did not reach SpeleoDB'), 'speleodb-late-upload', 'error');
      expect(closeProject).not.toHaveBeenCalled();
    } finally { vi.useRealTimers(); }
  });

  it('refuses to act for a project that is no longer the one it was raised for', async () => {
    const answer = raise();
    useProjectStore.setState({ project: { name: 'Other', file_path: '/synthetic/other.stalx' } as never });
    await userEvent.click(screen.getByTestId('speleodb-exit-keep'));
    expect(await answer).toBe(false);
    expect(closeProject).not.toHaveBeenCalled();
  });
});
