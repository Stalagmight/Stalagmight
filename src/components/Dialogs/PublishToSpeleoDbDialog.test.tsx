/**
 * Component test (#207) - the Create-on-SpeleoDB Publish dialog.
 *
 * Covers the submit gating (name + country + valid coords), the `speleodb_publish`
 * payload, and the load-guard: a failed reopen must NOT claim success or close.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
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
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';

let loadProjectMock: ReturnType<typeof vi.fn>;
let closeDialogMock: ReturnType<typeof vi.fn>;
let showToastMock: ReturnType<typeof vi.fn>;

beforeEach(() => {
  invokeIpcMock.mockReset();
  loadProjectMock = vi.fn(async () => ({ success: true }));
  closeDialogMock = vi.fn();
  showToastMock = vi.fn();
  useProjectStore.setState({
    project: { name: 'My Cave', zero_datums: [] } as never,
    loadProject: loadProjectMock,
  });
  useSpeleoStore.setState({
    refreshLinkStatus: vi.fn(async () => {}),
    clearSession: vi.fn(),
  });
  useUIStore.setState({ closeDialog: closeDialogMock, showToast: showToastMock });
});

describe('PublishToSpeleoDbDialog', () => {
  it('gates Publish on a chosen country and publishes with the right params', async () => {
    invokeIpcMock.mockResolvedValue({
      project_id: 'p1',
      endpoint: 'https://x',
      local_path: '/cache/p1/project.stalx',
    });
    render(<PublishToSpeleoDbDialog />);
    fireEvent.change(screen.getByTestId('publish-to-speleodb-dialog-message'), { target: { value: 'First survey' } });

    // Name prefilled from the project; country still unchosen -> Publish disabled.
    expect(screen.getByTestId('publish-to-speleodb-dialog-name')).toHaveValue('My Cave');
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
          message: 'First survey',
          country: 'US',
          visibility: 'PRIVATE',
          latitude: null,
          longitude: null,
        },
      });
    });
    // Reloaded the now-remote project and closed on success.
    await waitFor(() => expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1/project.stalx'));
    await waitFor(() => expect(closeDialogMock).toHaveBeenCalled());
  });

  it('does NOT close or claim success when the reopen fails', async () => {
    invokeIpcMock.mockResolvedValue({
      project_id: 'p1',
      endpoint: 'https://x',
      local_path: '/cache/p1/project.stalx',
    });
    loadProjectMock.mockResolvedValue({ success: false });
    render(<PublishToSpeleoDbDialog />);
    fireEvent.change(screen.getByTestId('publish-to-speleodb-dialog-message'), { target: { value: 'First survey' } });

    await pickOption('publish-to-speleodb-dialog-country', 'US');
    fireEvent.change(screen.getByTestId('publish-to-speleodb-dialog-description'), { target: { value: 'desc' } });
    await userEvent.click(screen.getByTestId('publish-to-speleodb-dialog-submit'));

    await waitFor(() => expect(loadProjectMock).toHaveBeenCalled());
    expect(closeDialogMock).not.toHaveBeenCalled();
  });

  it('blocks submit on an out-of-range coordinate pair', async () => {
    render(<PublishToSpeleoDbDialog />);
    fireEvent.change(screen.getByTestId('publish-to-speleodb-dialog-message'), { target: { value: 'First survey' } });
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
    fireEvent.change(screen.getByTestId('publish-to-speleodb-dialog-message'), { target: { value: 'First survey' } });
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
    fireEvent.change(screen.getByTestId('publish-to-speleodb-dialog-message'), { target: { value: 'First survey' } });
    expect(screen.getByTestId('publish-to-speleodb-dialog-country')).toHaveTextContent('Select a country');
  });
});
