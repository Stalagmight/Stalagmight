/**
 * Component test (#579) - the About dialog's manual "Check for updates"
 * button: three inline result states (up to date / available / failed),
 * with mocked `invokeIpc`, no toast.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, act } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

import '../../i18n';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
  getErrorMessage: (e: unknown) => (e instanceof Error ? e.message : String(e)),
}));

import SettingsDialog from './SettingsDialog';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { defaultConfig } from '../../types/config';

beforeEach(() => {
  invokeIpcMock.mockReset();
  useProjectStore.setState({ config: defaultConfig, updateConfig: vi.fn() });
  useUIStore.setState({
    closeDialog: vi.fn(),
    showToast: vi.fn(),
    dialogData: {},
  });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
});

async function openAbout() {
  render(<SettingsDialog />);
  await userEvent.click(await screen.findByTestId('settings-about-button'));
  await screen.findByTestId('about-dialog');
}

describe('About "Check for updates" (#579)', () => {
  it('shows "up to date" when the check finds nothing', async () => {
    invokeIpcMock.mockResolvedValue({
      status: { kind: 'up_to_date' },
      current_version: '0.5.0',
    });
    await openAbout();
    await userEvent.click(screen.getByTestId('about-check-updates'));
    const result = await screen.findByTestId('about-check-updates-result');
    expect(result).toHaveTextContent(/up to date/i);
  });

  it('shows the available version when one is on offer', async () => {
    invokeIpcMock.mockResolvedValue({
      status: { kind: 'available', version: '9.9.9', pub_date: null, notes_url: 'https://x' },
      current_version: '0.5.0',
    });
    await openAbout();
    await userEvent.click(screen.getByTestId('about-check-updates'));
    const result = await screen.findByTestId('about-check-updates-result');
    expect(result).toHaveTextContent('9.9.9');
  });

  it('shows a failure state when the check rejects, with no toast', async () => {
    invokeIpcMock.mockRejectedValue(new Error('network down'));
    const showToast = vi.fn();
    useUIStore.setState({ showToast });
    await openAbout();
    await userEvent.click(screen.getByTestId('about-check-updates'));
    const result = await screen.findByTestId('about-check-updates-result');
    expect(result).toHaveTextContent(/could not check/i);
    expect(showToast).not.toHaveBeenCalled();
  });
});

/**
 * [review] The track LINE and the check BEHAVIOUR must agree. The line used to
 * read the staged `localConfig`, while `check_for_app_update` reads the config
 * the backend has on disk - so staging Weekly without saving showed "Weekly"
 * next to a button that queried Stable.
 */
describe('the About update-track line', () => {
  beforeEach(() => {
    invokeIpcMock.mockResolvedValue(undefined);
  });

  it('shows the PERSISTED track, not a staged one', async () => {
    useProjectStore.setState({
      config: { ...defaultConfig, general: { ...defaultConfig.general, update_track: 'stable' } },
      updateConfig: vi.fn(),
    });
    await openAbout();

    expect(screen.getByTestId('about-update-track')).toHaveTextContent(/stable/i);
    expect(screen.queryByTestId('about-update-track-pending')).not.toBeInTheDocument();
  });

  it('flags a staged change instead of displaying it as fact', async () => {
    // `localConfig` is seeded once on mount and then only by the user's edits,
    // so changing the STORE config afterwards is the staged/persisted split
    // without having to drive the portal-based Select: staged = stable (what
    // the dialog opened with), persisted = weekly (what the backend would
    // query).
    useProjectStore.setState({
      config: { ...defaultConfig, general: { ...defaultConfig.general, update_track: 'stable' } },
      updateConfig: vi.fn(),
    });
    render(<SettingsDialog />);
    await screen.findByTestId('settings-about-button');

    act(() => {
      useProjectStore.setState({
        config: {
          ...defaultConfig,
          general: { ...defaultConfig.general, update_track: 'weekly' },
        },
      });
    });
    await userEvent.click(screen.getByTestId('settings-about-button'));
    await screen.findByTestId('about-dialog');

    // The line follows the PERSISTED track - what the check will actually ask.
    expect(screen.getByTestId('about-update-track')).toHaveTextContent(/weekly/i);
    // ...and the disagreement with the staged value is called out rather than
    // silently resolved either way.
    expect(screen.getByTestId('about-update-track-pending')).toBeInTheDocument();
  });
});
