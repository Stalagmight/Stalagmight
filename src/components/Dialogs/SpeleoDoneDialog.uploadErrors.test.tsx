/**
 * Component test - the SpeleoDB upload dialog stops at survey errors.
 *
 * An upload sends the survey to every collaborator as it is, so when it carries
 * errors (not warnings) the dialog says so in red, and the upload button waits
 * until the user either ticks "Upload anyway, with the errors" or picks
 * "Resolve now". Resolve now uploads nothing, keeps the typed message for the
 * next upload, and opens the Warnings tab.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, render, screen, waitFor, fireEvent } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (_e: unknown, f?: string) => f ?? 'the server said no',
}));

import SpeleoDoneDialog from './SpeleoDoneDialog';
import { countUnresolvedErrors } from '../../utils/validation';
import { getSetAsideUploadMessage, getUploadContinuation } from '../../utils/uploadContinuation';
import { finishSpeleoSettings } from '../../utils/finishSpeleoSettings';
import { useSpeleoStore, type SpeleoLinkState } from '../../store/speleoStore';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import type { ProjectWarning } from '../../types/commands';

const PATH = '/synthetic/errors.stalx';

const LINK: SpeleoLinkState = {
  permission: 'READ_AND_WRITE',
  editState: 'editing',
  lockHolder: null,
  lockedByOther: false,
  hasUnpushedEdits: true,
  unwound: false,
  undoMode: 'full',
  remoteState: 'up_to_date',
  noAccess: false,
  serverError: null,
};

function issue(severity: 'error' | 'warning', shotId: number, isResolved = false): ProjectWarning {
  return {
    sectionId: 1, shotId, shotIndex: shotId, code: severity === 'error' ? 'NEGATIVE_LENGTH' : 'LARGE_LENGTH',
    message: 'synthetic', severity, isResolved,
  };
}

const confirm = () => screen.getByTestId('speleodb-done-confirm');
let lifetime = 0;
const uploadCalls = () => invokeIpcMock.mock.calls.filter(([c]) => c === 'speleodb_push' || c === 'speleodb_done');

beforeEach(() => {
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue({ kind: 'pushed' });
  // A fresh project lifetime per test: it clears the module-level pending and
  // set-aside upload messages an earlier test left behind.
  lifetime += 1;
  useProjectStore.setState({
    project: { file_path: PATH, created_at: `t${lifetime}`, zero_datums: [{}] } as never,
    isDirty: false,
    warnings: [issue('error', 1), issue('error', 2), issue('warning', 3)],
    saveProject: vi.fn(async () => ({ success: true }) as never),
  });
  useSpeleoStore.setState({ link: { ...LINK }, banner: null, busy: false, session: { loggedIn: true, userEmail: 'me@x', endpoint: 'https://synthetic.example' } });
  useUIStore.setState({
    activeDialog: 'speleodb-done',
    dialogData: { mode: 'push' },
    showToast: vi.fn(),
    appMode: 'survey',
    detailPanelTab: 'details',
    detailPanelOpen: true,
    detailPanelMode: 'pin',
  });
});

describe('countUnresolvedErrors', () => {
  it('counts unresolved errors only', () => {
    expect(countUnresolvedErrors([issue('error', 1), issue('warning', 2), issue('error', 3, true)])).toBe(1);
    expect(countUnresolvedErrors([])).toBe(0);
  });
});

describe('SpeleoDoneDialog - survey errors before an upload', () => {
  it('names the number of errors, ignoring warnings', () => {
    render(<SpeleoDoneDialog />);
    expect(screen.getByTestId('speleodb-upload-errors')).toBeVisible();
    expect(screen.getByTestId('speleodb-upload-errors-count')).toHaveTextContent('This survey has 2 errors.');
  });

  it('uses the singular for one error', () => {
    useProjectStore.setState({ warnings: [issue('error', 1)] });
    render(<SpeleoDoneDialog />);
    expect(screen.getByTestId('speleodb-upload-errors-count')).toHaveTextContent('This survey has 1 error.');
  });

  it('holds the upload until the user accepts uploading the errors', async () => {
    render(<SpeleoDoneDialog />);
    fireEvent.change(screen.getByTestId('speleodb-done-message'), { target: { value: 'Synthetic update' } });
    expect(confirm()).toBeDisabled();
    expect(confirm()).toHaveAttribute('title', expect.stringContaining('Upload anyway'));
    fireEvent.click(confirm());
    expect(uploadCalls()).toHaveLength(0);

    await userEvent.click(screen.getByTestId('speleodb-upload-errors-accept'));
    expect(confirm()).toBeEnabled();
    expect(confirm()).not.toHaveAttribute('title');
    // Unticking holds it again.
    await userEvent.click(screen.getByTestId('speleodb-upload-errors-accept'));
    expect(confirm()).toBeDisabled();

    await userEvent.click(screen.getByTestId('speleodb-upload-errors-accept'));
    await userEvent.click(confirm());
    await waitFor(() => expect(uploadCalls()).toEqual([['speleodb_push', expect.objectContaining({ message: 'Synthetic update' })]]));
  });

  it('says nothing and holds nothing when the survey has only warnings', async () => {
    useProjectStore.setState({ warnings: [issue('warning', 3)] });
    render(<SpeleoDoneDialog />);
    expect(screen.queryByTestId('speleodb-upload-errors')).toBeNull();
    fireEvent.change(screen.getByTestId('speleodb-done-message'), { target: { value: 'Synthetic update' } });
    expect(confirm()).toBeEnabled();
  });

  it('Resolve now uploads nothing, keeps the message, and opens the Warnings tab', async () => {
    useUIStore.setState({ dialogData: { mode: 'push', closeAfterSave: true }, appMode: 'plan' });
    render(<SpeleoDoneDialog />);
    fireEvent.change(screen.getByTestId('speleodb-done-message'), { target: { value: 'Fixed the sump' } });
    await userEvent.click(screen.getByTestId('speleodb-upload-errors-resolve'));

    expect(useUIStore.getState().activeDialog).toBeNull();
    expect(uploadCalls()).toHaveLength(0);
    expect(useProjectStore.getState().saveProject).not.toHaveBeenCalled();
    // The message is set aside for the next upload dialog, but no upload is left
    // pending: the close the Save and Close asked for is dropped too, because the
    // project stays open for the fix.
    expect(getUploadContinuation(PATH)).toBeNull();
    expect(getSetAsideUploadMessage(PATH)).toBe('Fixed the sump');
    expect(useUIStore.getState().detailPanelTab).toBe('warnings');
    expect(useUIStore.getState().detailPanelOpen).toBe(true);
    expect(useUIStore.getState().appMode).toBe('survey');
  });

  it('closing Settings afterwards does not bring the upload dialog back', async () => {
    render(<SpeleoDoneDialog />);
    fireEvent.change(screen.getByTestId('speleodb-done-message'), { target: { value: 'Fixed the sump' } });
    await userEvent.click(screen.getByTestId('speleodb-upload-errors-resolve'));

    // SettingsDialog closes through finishSpeleoSettings, which returns to a
    // pending upload; Resolve now must not leave one behind.
    useUIStore.setState({ activeDialog: 'settings', dialogData: {} });
    finishSpeleoSettings();
    expect(useUIStore.getState().activeDialog).toBeNull();
    expect(getSetAsideUploadMessage(PATH)).toBe('Fixed the sump');
  });

  it.each([
    ['another error appears', [issue('error', 1), issue('error', 2), issue('error', 4)], 3],
    ['one error is swapped for another, the count unchanged', [issue('error', 1), issue('error', 4)], 2],
  ])('a tick accepts the errors it was given for: asks again when %s', async (_label, next, count) => {
    render(<SpeleoDoneDialog />);
    fireEvent.change(screen.getByTestId('speleodb-done-message'), { target: { value: 'Synthetic update' } });
    await userEvent.click(screen.getByTestId('speleodb-upload-errors-accept'));
    expect(confirm()).toBeEnabled();

    act(() => useProjectStore.setState({ warnings: next }));
    expect(screen.getByTestId('speleodb-upload-errors-count')).toHaveTextContent(`This survey has ${count} errors.`);
    expect(screen.getByTestId('speleodb-upload-errors-accept')).not.toBeChecked();
    expect(confirm()).toBeDisabled();
  });

  it('the next upload dialog starts from the message Resolve now kept', async () => {
    const { unmount } = render(<SpeleoDoneDialog />);
    fireEvent.change(screen.getByTestId('speleodb-done-message'), { target: { value: 'Fixed the sump' } });
    await userEvent.click(screen.getByTestId('speleodb-upload-errors-resolve'));
    unmount();

    useProjectStore.setState({ warnings: [] });
    useUIStore.setState({ activeDialog: 'speleodb-done', dialogData: { mode: 'push' } });
    render(<SpeleoDoneDialog />);
    expect(screen.getByTestId('speleodb-done-message')).toHaveValue('Fixed the sump');
    expect(screen.queryByTestId('speleodb-upload-errors')).toBeNull();
    expect(confirm()).toBeEnabled();
    // The open dialog owns the message again, as a normal pending upload.
    expect(getUploadContinuation(PATH)?.message).toBe('Fixed the sump');
    expect(getSetAsideUploadMessage(PATH)).toBeNull();
  });

  it('Done editing with changes to upload asks the same question', () => {
    useUIStore.setState({ dialogData: {} });
    render(<SpeleoDoneDialog />);
    expect(screen.getByText('Finish editing')).toBeVisible();
    expect(screen.getByTestId('speleodb-upload-errors')).toBeVisible();
    expect(confirm()).toBeDisabled();
  });

  it('Done editing that only releases the lock uploads nothing, so it does not ask', () => {
    useUIStore.setState({ dialogData: {} });
    useSpeleoStore.setState({ link: { ...LINK, hasUnpushedEdits: false } });
    render(<SpeleoDoneDialog />);
    expect(screen.queryByTestId('speleodb-upload-errors')).toBeNull();
    expect(confirm()).toBeEnabled();
  });

  it('Save and Close with nothing to upload only releases the lock, so it does not ask', () => {
    useUIStore.setState({ dialogData: { mode: 'push', closeAfterSave: true } });
    useSpeleoStore.setState({ link: { ...LINK, hasUnpushedEdits: false } });
    render(<SpeleoDoneDialog />);
    expect(screen.queryByTestId('speleodb-upload-errors')).toBeNull();
    fireEvent.change(screen.getByTestId('speleodb-done-message'), { target: { value: 'Synthetic update' } });
    expect(confirm()).toBeEnabled();
  });

  it('stays out of the way while the upload itself is blocked', () => {
    useSpeleoStore.setState({ link: { ...LINK, editState: 'read-only' } });
    render(<SpeleoDoneDialog />);
    expect(screen.getByTestId('speleodb-upload-blocked')).toBeVisible();
    expect(screen.queryByTestId('speleodb-upload-errors')).toBeNull();
  });
});
