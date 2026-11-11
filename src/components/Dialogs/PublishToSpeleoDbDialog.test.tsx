import { setPublishContinuation } from '../../utils/publishContinuation';
import { confirmWorkspaceDrafts, hasWorkspaceDrafts } from '../../utils/workspaceDrafts';
import { useExitConfirm } from '../../utils/exitConfirm';
import { autoAcquireSpeleoLock } from '../../utils/autoAcquireSpeleoLock';
vi.mock('../../utils/autoAcquireSpeleoLock', () => ({ autoAcquireSpeleoLock: vi.fn(async () => {}) }));
/**
 * Component test (#207) - the Create-on-SpeleoDB Publish dialog.
 *
 * Covers the submit gating (name + country + valid coords), the `speleodb_publish`
 * payload, and the load-guard: a failed reopen must NOT claim success or close.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, render, screen, waitFor, fireEvent } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({ invokeIpc: invokeIpcMock, getErrorMessage: (_e: unknown, f: string) => f }));

// jsdom does not implement scrollIntoView; the Select component calls it on open.
Element.prototype.scrollIntoView = vi.fn();

/** Pick `value` from a portal-based `Select` (src/components/common/Select.tsx):
 *  it isn't a native `<select>`, so this clicks the trigger, then the option
 *  rendered at `data-testid="${testId}-option-${value}"`. */
async function pickOption(testId: string, value: string) {
  await userEvent.click(screen.getByTestId(testId));
  await userEvent.click(screen.getByTestId(`${testId}-option-${encodeURIComponent(value)}`));
}

import PublishToSpeleoDbDialog from './PublishToSpeleoDbDialog';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';

let loadProjectMock: ReturnType<typeof vi.fn>;
let closeDialogMock: ReturnType<typeof vi.fn>;
let showToastMock: ReturnType<typeof vi.fn>;

beforeEach(() => {
  invokeIpcMock.mockReset();
  vi.mocked(autoAcquireSpeleoLock).mockClear();
  setPublishContinuation(null);
  loadProjectMock = vi.fn(async () => ({ success: true }));
  closeDialogMock = vi.fn();
  showToastMock = vi.fn();
  useProjectStore.setState({
    project: { name: 'My Cave', zero_datums: [], file_path: '/local.stalx' } as never,
    isDirty: true,
    loadProject: loadProjectMock,
  });
  useSpeleoStore.setState({
    link: null, projectLinkKind: 'local', remoteProjectId: null, busy: false,
    session: { loggedIn: true, userEmail: 'user@example.com', endpoint: 'https://x' },
    refreshLinkStatus: vi.fn(async () => {}),
    clearSession: vi.fn(),
  });
  useUIStore.setState({ activeDialog: 'publish-to-speleodb', dialogData: {}, closeDialog: closeDialogMock, showToast: showToastMock });
});

describe('PublishToSpeleoDbDialog', () => {
  it('searches countries by name and code, supports keyboard selection, and resets on reopen', async () => {
    const user = userEvent.setup();
    render(<PublishToSpeleoDbDialog />);
    const trigger = screen.getByTestId('publish-to-speleodb-dialog-country');
    await user.click(trigger);
    const search = screen.getByTestId('publish-to-speleodb-dialog-country-search');
    expect(search).toHaveFocus();
    await user.type(search, 'united states of america');
    expect(screen.getAllByRole('option')).toHaveLength(1);
    await user.keyboard('{ArrowDown}{Enter}');
    expect(trigger).toHaveTextContent('United States');
    expect(trigger).toHaveFocus();
    await user.click(trigger);
    const reopened = screen.getByTestId('publish-to-speleodb-dialog-country-search');
    expect(reopened).toHaveValue('');
    await user.type(reopened, 'zzzzzz');
    expect(screen.queryAllByRole('option')).toHaveLength(0);
    expect(screen.getByRole('status')).toHaveTextContent('No countries');
    await user.clear(reopened);
    await user.type(reopened, 'mx');
    expect(screen.getAllByRole('option')).toHaveLength(1);
    await user.click(screen.getByRole('option', { name: 'Mexico' }));
    expect(trigger).toHaveTextContent('Mexico');
    await user.click(trigger);
    await user.keyboard('{Escape}');
    expect(screen.queryByTestId('publish-to-speleodb-dialog-country-search')).toBeNull();
    expect(trigger).toHaveFocus();
  });

  it('gates Publish on a chosen country and publishes with the right params', async () => {
    invokeIpcMock.mockImplementation(async (cmd) => {
      if (cmd === 'speleodb_link_status') return { is_remote: true, project_id: 'p1', has_unpushed_edits: false, edit_enabled: false, permission: 'ADMIN', remote_state: 'up_to_date' };
      return { project_id: 'p1', endpoint: 'https://x', local_path: '/cache/p1/project.stalx' };
    });
    render(<PublishToSpeleoDbDialog />);
    expect(screen.queryByTestId('publish-to-speleodb-dialog-visibility')).toBeNull();
    expect(screen.queryByTestId('publish-to-speleodb-dialog-message')).toBeNull();

    // Name prefilled from the project; country still unchosen -> Publish disabled.
    expect(screen.getByTestId('publish-to-speleodb-dialog-name')).toHaveValue('My Cave');
    expect(screen.getByTestId('publish-to-speleodb-dialog-name')).toBeDisabled();
    expect(screen.getByTestId('publish-to-speleodb-dialog-submit')).toBeDisabled();

    await pickOption('publish-to-speleodb-dialog-country', 'US');
    const submit = screen.getByTestId('publish-to-speleodb-dialog-submit');
    // Country chosen but description still empty -> stays disabled (server requires it).
    expect(submit).toBeDisabled();
    expect(screen.getByTestId('publish-to-speleodb-dialog-description-error')).toBeInTheDocument();

    fireEvent.change(screen.getByTestId('publish-to-speleodb-dialog-description'), { target: { value: 'Main line survey' } });
    expect(screen.queryByTestId('publish-to-speleodb-dialog-description-error')).toBeNull();
    await waitFor(() => expect(submit).toBeEnabled());
    await userEvent.click(submit);

    await waitFor(() => {
      const call = invokeIpcMock.mock.calls.find((c) => c[0] === 'speleodb_publish');
      expect(call).toBeTruthy();
      expect(call![1]).toEqual({
        params: {
          name: 'My Cave',
          description: 'Main line survey',
          message: 'Project Creation',
          country: 'US',
          visibility: 'PRIVATE',
          latitude: null,
          longitude: null,
        },
      });
    });
    // Reloaded the now-remote project and closed on success.
    await waitFor(() => expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1/project.stalx', expect.objectContaining({ accessConfirmed: true, acquireLock: false })));
    await waitFor(() => expect(closeDialogMock).toHaveBeenCalled());
    expect(useProjectStore.getState().project?.file_path).toBe('/cache/p1/project.stalx');
    expect(autoAcquireSpeleoLock).toHaveBeenCalledWith('/cache/p1/project.stalx', expect.any(Function));
  });

  it('keeps the newly linked identity and offers remote recovery when a fresh reopen fails', async () => {
    invokeIpcMock.mockImplementation(async (cmd) => {
      if (cmd === 'speleodb_link_status') return { is_remote: true, project_id: 'p1', has_unpushed_edits: false, edit_enabled: false, permission: 'ADMIN', remote_state: 'up_to_date' };
      return { project_id: 'p1', endpoint: 'https://x', local_path: '/cache/p1/project.stalx' };
    });
    loadProjectMock.mockResolvedValue({ success: false });
    render(<PublishToSpeleoDbDialog />);

    await pickOption('publish-to-speleodb-dialog-country', 'US');
    fireEvent.change(screen.getByTestId('publish-to-speleodb-dialog-description'), { target: { value: 'desc' } });
    await userEvent.click(screen.getByTestId('publish-to-speleodb-dialog-submit'));

    await waitFor(() => expect(loadProjectMock).toHaveBeenCalled());
    expect(closeDialogMock).toHaveBeenCalled();
    expect(useSpeleoStore.getState().projectLinkKind).toBe('remote');
    expect(showToastMock).toHaveBeenCalledWith(expect.stringContaining('fresh server copy'), TOAST_KEYS.SPELEODB_PUBLISH, 'error');
  });

  it('warns when the first upload left the undo history out to fit the size limit (sdk#3)', async () => {
    invokeIpcMock.mockImplementation(async (cmd) => {
      if (cmd === 'speleodb_link_status') return { is_remote: true, project_id: 'p1', has_unpushed_edits: false, edit_enabled: false, permission: 'ADMIN', remote_state: 'up_to_date' };
      if (cmd === 'speleodb_open_project') return { local_path: '/cache/p1/project.stalx' };
      return { project_id: 'p1', endpoint: 'https://x', local_path: '/cache/p1/project.stalx', undo_history_omitted: true };
    });
    render(<PublishToSpeleoDbDialog />);

    await pickOption('publish-to-speleodb-dialog-country', 'US');
    fireEvent.change(screen.getByTestId('publish-to-speleodb-dialog-description'), { target: { value: 'desc' } });
    await userEvent.click(screen.getByTestId('publish-to-speleodb-dialog-submit'));

    await waitFor(() => expect(closeDialogMock).toHaveBeenCalled());
    expect(showToastMock).toHaveBeenCalledTimes(1);
    expect(showToastMock).toHaveBeenCalledWith(expect.stringContaining('without undo history'), TOAST_KEYS.SPELEODB_PUBLISH, 'warning');
  });

  it('adds the undo-history note to the reopen failure instead of a second toast (sdk#3)', async () => {
    invokeIpcMock.mockImplementation(async (cmd) => {
      if (cmd === 'speleodb_link_status') return { is_remote: true, project_id: 'p1', has_unpushed_edits: false, edit_enabled: false, permission: 'ADMIN', remote_state: 'up_to_date' };
      return { project_id: 'p1', endpoint: 'https://x', local_path: '/cache/p1/project.stalx', undo_history_omitted: true };
    });
    loadProjectMock.mockResolvedValue({ success: false });
    render(<PublishToSpeleoDbDialog />);

    await pickOption('publish-to-speleodb-dialog-country', 'US');
    fireEvent.change(screen.getByTestId('publish-to-speleodb-dialog-description'), { target: { value: 'desc' } });
    await userEvent.click(screen.getByTestId('publish-to-speleodb-dialog-submit'));

    await waitFor(() => expect(showToastMock).toHaveBeenCalledTimes(1));
    const [message, key, kind] = showToastMock.mock.calls[0] ?? [];
    expect(key).toBe(TOAST_KEYS.SPELEODB_PUBLISH);
    expect(kind).toBe('error');
    expect(message).toContain('fresh server copy');
    expect(message).toContain('without undo history');
  });

  it('blocks submit on an out-of-range coordinate pair', async () => {
    render(<PublishToSpeleoDbDialog />);
    await pickOption('publish-to-speleodb-dialog-country', 'US');
    await userEvent.type(screen.getByTestId('publish-to-speleodb-dialog-latitude'), '999');
    await userEvent.type(screen.getByTestId('publish-to-speleodb-dialog-longitude'), '0');
    expect(screen.getByTestId('publish-to-speleodb-dialog-submit')).toBeDisabled();
    expect(screen.getByTestId('publish-to-speleodb-dialog-coords-error')).toBeInTheDocument();
  });

  it('prefills country and coordinates from the first georeferenced datum (#217)', async () => {
    // A Yucatan cenote datum should offline-resolve the required country to MX.
    useProjectStore.setState({
      project: {
        name: 'Cenote',
        zero_datums: [{ gps: { latitude: 20.21, longitude: -87.46 } }],
      } as never,
      loadProject: loadProjectMock,
    });
    render(<PublishToSpeleoDbDialog />);
    expect(screen.getByTestId('publish-to-speleodb-dialog-country')).toHaveTextContent('Mexico');
    expect(screen.getByTestId('publish-to-speleodb-dialog-latitude')).toHaveValue(20.21);
    expect(screen.getByTestId('publish-to-speleodb-dialog-longitude')).toHaveValue(-87.46);
    // Country + coords + name are prefilled, but a description is still required, so
    // Publish stays disabled until one is entered.
    expect(screen.getByTestId('publish-to-speleodb-dialog-submit')).toBeDisabled();
    fireEvent.change(screen.getByTestId('publish-to-speleodb-dialog-description'), { target: { value: 'Cenote line' } });
    await waitFor(() =>
      expect(screen.getByTestId('publish-to-speleodb-dialog-submit')).toBeEnabled(),
    );
  });

  it('leaves country empty when the datum resolves to no country (open ocean)', () => {
    useProjectStore.setState({
      project: {
        name: 'Blue Hole',
        zero_datums: [{ gps: { latitude: 30.0, longitude: -40.0 } }],
      } as never,
      loadProject: loadProjectMock,
    });
    render(<PublishToSpeleoDbDialog />);
    expect(screen.getByTestId('publish-to-speleodb-dialog-country')).toHaveTextContent('Select a country');
  });
});


it('conversion creation form uses readable field labels and Settings-only sign-in', async () => {
  useSpeleoStore.setState({ session: null });
  render(<PublishToSpeleoDbDialog />);
  expect(screen.getByTestId('publish-to-speleodb-dialog-name')).toHaveAccessibleName('Name');
  expect(screen.getByTestId('publish-to-speleodb-dialog-description')).toHaveAccessibleName('Description');
  await pickOption('publish-to-speleodb-dialog-country', 'US');
  fireEvent.change(screen.getByTestId('publish-to-speleodb-dialog-description'), { target: { value: 'Draft description' } });
  expect(screen.getByTestId('publish-to-speleodb-dialog-submit')).toBeDisabled();
  await userEvent.click(screen.getByTestId('publish-to-speleodb-dialog-sign-in'));
  expect(useUIStore.getState().activeDialog).toBe('settings');
  expect(useUIStore.getState().dialogData.initialCategory).toBe('speleodb');
  expect(invokeIpcMock).not.toHaveBeenCalled();
});


it('publishing blocks the app with a centered progress overlay that expires after exactly 120 seconds', async () => {
  let finish!: (value: unknown) => void;
  invokeIpcMock.mockImplementation((cmd) => {
    if (cmd === 'speleodb_publish') return new Promise((resolve) => { finish = resolve; });
    return Promise.resolve({ is_remote: true, project_id: 'p1', has_unpushed_edits: true });
  });
  render(<PublishToSpeleoDbDialog />);
  await pickOption('publish-to-speleodb-dialog-country', 'US');
  fireEvent.change(screen.getByTestId('publish-to-speleodb-dialog-description'), { target: { value: 'Synthetic shared survey' } });
  vi.useFakeTimers();
  try {
    fireEvent.click(screen.getByTestId('publish-to-speleodb-dialog-submit'));
    const overlay = screen.getByTestId('speleodb-publish-progress');
    expect(overlay).toHaveAttribute('aria-busy', 'true');
    expect(overlay).toHaveFocus();
    expect(screen.getByTestId('publish-to-speleodb-dialog').closest('[inert]')).not.toBeNull();
    fireEvent.keyDown(overlay, { key: 'Escape' });
    expect(closeDialogMock).not.toHaveBeenCalled();
    act(() => vi.advanceTimersByTime(119_999));
    expect(screen.getByTestId('speleodb-publish-progress')).toBeInTheDocument();
    act(() => vi.advanceTimersByTime(1));
    expect(screen.queryByTestId('speleodb-publish-progress')).toBeNull();
    expect(screen.getByTestId('speleodb-publish-timeout')).toHaveTextContent('120 seconds');
    expect(screen.getByTestId('publish-to-speleodb-dialog-cancel')).toBeEnabled();
    expect(screen.getByTestId('publish-to-speleodb-dialog-submit')).toBeDisabled();
    await act(async () => finish({ project_id: 'p1', endpoint: 'https://x', local_path: '/cache/p1/project.stalx' }));
  } finally { vi.useRealTimers(); }
});

/** frontend#10 - what is typed in the project form is guarded on every way out. */
describe('PublishToSpeleoDbDialog as unsent SpeleoDB input', () => {
  it('is a draft once something is typed, and Discard closes the form', async () => {
    render(<PublishToSpeleoDbDialog />);
    expect(hasWorkspaceDrafts()).toBe(false);
    await userEvent.type(screen.getByTestId('publish-to-speleodb-dialog-description'), 'Synthetic system');
    expect(hasWorkspaceDrafts()).toBe(true);
    const pending = confirmWorkspaceDrafts();
    await waitFor(() => expect(useExitConfirm.getState().request?.message).toContain('the SpeleoDB project form'));
    act(() => useExitConfirm.getState().request?.answer(true));
    expect(await pending).toBe(true);
    expect(closeDialogMock).toHaveBeenCalledOnce();
  });

  it('stops being a draft once unmounted', async () => {
    const { unmount } = render(<PublishToSpeleoDbDialog />);
    await userEvent.type(screen.getByTestId('publish-to-speleodb-dialog-description'), 'Synthetic system');
    unmount();
    expect(hasWorkspaceDrafts()).toBe(false);
  });
});
