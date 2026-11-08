/**
 * [#579] The update banner's two outbound calls that no other tier covers.
 *
 * Release notes went through `window.open`, which is dead in the Tauri webview
 * (no browser chrome to open a tab in, and the CSP would not admit the
 * navigation): the button looked wired and did nothing. Restart now arms the
 * backend first and can be refused, which must be visible rather than silent.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const openUrlMock = vi.hoisted(() => vi.fn(async () => {}));
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: openUrlMock }));

const invokeIpcMock = vi.hoisted(() => vi.fn(async () => null));
vi.mock('../../utils/ipc', () => ({ invokeIpc: invokeIpcMock }));

const closeMock = vi.hoisted(() => vi.fn(async () => {}));
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ close: closeMock, destroy: vi.fn(async () => {}) }),
}));

import { UpdateBanner } from '../UpdateBanner';
import { useUpdateStore } from '../../store/updateStore';
import { useUIStore } from '../../store/uiStore';
import { resetUpdateRestartForTests } from '../../utils/quit';

const INFO = { version: '9.9.9', notesUrl: 'https://example.test/notes', installsOnQuit: true };

function seed(status: 'available' | 'ready'): void {
  const store = useUpdateStore.getState();
  store.setAvailable(store.beginCheck(), INFO);
  if (status === 'ready') {
    useUpdateStore.getState().startDownloading(INFO.version);
    useUpdateStore.getState().setReady(INFO.version);
  }
}

const showToast = vi.fn();

beforeEach(() => {
  openUrlMock.mockClear();
  openUrlMock.mockResolvedValue(undefined);
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue(null);
  closeMock.mockReset();
  closeMock.mockResolvedValue(undefined);
  showToast.mockReset();
  resetUpdateRestartForTests();
  useUpdateStore.getState().reset();
  useUIStore.setState({ showToast: showToast as never });
});

describe('Release notes', () => {
  it('hands the URL to the OS browser through the opener plugin', async () => {
    seed('available');
    render(<UpdateBanner />);

    await userEvent.click(screen.getByTestId('update-banner-notes'));

    expect(openUrlMock).toHaveBeenCalledWith(INFO.notesUrl);
  });

  it('logs rather than throwing when the opener rejects', async () => {
    const err = vi.spyOn(console, 'error').mockImplementation(() => {});
    openUrlMock.mockRejectedValueOnce(new Error('no handler'));
    seed('available');
    render(<UpdateBanner />);

    await userEvent.click(screen.getByTestId('update-banner-notes'));

    expect(err).toHaveBeenCalled();
    err.mockRestore();
  });
});

describe('Restart', () => {
  it('arms the backend before asking for a close', async () => {
    seed('ready');
    render(<UpdateBanner />);

    await userEvent.click(screen.getByTestId('update-banner-restart'));

    expect(invokeIpcMock).toHaveBeenCalledWith(
      'arm_app_update_restart',
      undefined,
      expect.anything(),
    );
    expect(closeMock).toHaveBeenCalledTimes(1);
  });

  /**
   * The arm can be refused - nothing downloaded, or a newer check superseded
   * this one. Without this the button would appear to do nothing at all.
   */
  it('toasts and re-offers Download when the backend refuses the arm', async () => {
    const err = vi.spyOn(console, 'error').mockImplementation(() => {});
    invokeIpcMock.mockRejectedValueOnce(new Error('nothing downloaded'));
    seed('ready');
    render(<UpdateBanner />);

    await userEvent.click(screen.getByTestId('update-banner-restart'));

    expect(closeMock).not.toHaveBeenCalled();
    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast.mock.calls[0]?.[1]).toBe('app-update-install-failed');
    expect(useUpdateStore.getState().status).toBe('available');
    expect(screen.getByTestId('update-banner-download')).toBeInTheDocument();
    err.mockRestore();
  });
});

describe('Download', () => {
  it('pins the completion to the version it started for', async () => {
    seed('available');
    render(<UpdateBanner />);

    await userEvent.click(screen.getByTestId('update-banner-download'));

    expect(invokeIpcMock).toHaveBeenCalledWith(
      'download_app_update',
      undefined,
      expect.anything(),
    );
    expect(useUpdateStore.getState().status).toBe('ready');
    expect(useUpdateStore.getState().downloadVersion).toBe(INFO.version);
  });
});

describe('Download (manual) failure', () => {
  it('toasts, because the user asked for this download', async () => {
    invokeIpcMock.mockRejectedValueOnce(new Error('offline'));
    seed('available');
    render(<UpdateBanner />);

    await userEvent.click(screen.getByTestId('update-banner-download'));

    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast.mock.calls[0]?.[1]).toBe('app-update-download-failed');
    expect(useUpdateStore.getState().status).toBe('available');
  });
});

/** [#631] Background download and install on quit. */
describe('the background download and install on quit', () => {
  it('stays hidden while a download a check started is running', () => {
    seed('available');
    useUpdateStore.getState().startDownloading(INFO.version, true);
    render(<UpdateBanner />);

    expect(screen.queryByTestId('update-banner')).not.toBeInTheDocument();
  });

  it('shows the progress of a download the user started', () => {
    seed('available');
    useUpdateStore.getState().startDownloading(INFO.version, false);
    render(<UpdateBanner />);

    expect(screen.getByTestId('update-banner-progress')).toBeInTheDocument();
  });

  it('says a ready update installs on quit, and offers Restart now and Dismiss', () => {
    seed('ready');
    render(<UpdateBanner />);

    expect(screen.getByTestId('update-banner')).toHaveTextContent(
      'Stalagmight 9.9.9 will be installed when you quit',
    );
    expect(screen.getByTestId('update-banner-restart')).toHaveTextContent('Restart now');
    expect(screen.getByTestId('update-banner-dismiss')).toBeInTheDocument();
    expect(screen.getByTestId('update-banner-notes')).toBeInTheDocument();
  });

  it('promises nothing about quitting where the platform does not install on quit', () => {
    const store = useUpdateStore.getState();
    store.setAvailable(store.beginCheck(), { ...INFO, installsOnQuit: false });
    useUpdateStore.getState().startDownloading(INFO.version);
    useUpdateStore.getState().setReady(INFO.version);
    render(<UpdateBanner />);

    expect(screen.getByTestId('update-banner')).not.toHaveTextContent('when you quit');
    expect(screen.getByTestId('update-banner-restart')).toHaveTextContent('Restart to update');
  });
});
