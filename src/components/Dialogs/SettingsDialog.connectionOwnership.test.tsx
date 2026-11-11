import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import '../../i18n';
import SettingsDialog from './SettingsDialog';
import { defaultConfig, type Config } from '../../types/config';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { emptyCommandResult } from '../../store/projectStore';

const initialProject = useProjectStore.getState();
const initialSpeleo = useSpeleoStore.getState();
const initialUi = useUIStore.getState();
const closeDialog = vi.fn();
const endpoint = (value: string): Config => ({
  ...defaultConfig, speleodb: { ...defaultConfig.speleodb, endpoint: value },
});

beforeEach(() => {
  closeDialog.mockReset();
  useProjectStore.setState({ config: endpoint('https://original.example'), updateConfig: vi.fn(async () => emptyCommandResult(true)) });
  useSpeleoStore.setState({
    session: null, pendingAuth: { kind: 'login', email: 'synthetic@example.com', password: 'synthetic', token: '', useToken: false },
    refreshSession: vi.fn(async () => {}), busy: false, authError: null,
  });
  useUIStore.setState({ activeDialog: 'settings', dialogData: { initialCategory: 'speleodb' }, closeDialog });
});
afterEach(() => {
  useProjectStore.setState(initialProject, true);
  useSpeleoStore.setState(initialSpeleo, true);
  useUIStore.setState(initialUi, true);
});

it('preserves a newer endpoint edit when Close persistence publishes an older endpoint', async () => {
  let finish!: () => void;
  const updateConfig = vi.fn((config: Config) => new Promise<ReturnType<typeof emptyCommandResult>>((resolve) => {
    finish = () => {
      useProjectStore.setState({ config });
      resolve(emptyCommandResult(true));
    };
  }));
  useProjectStore.setState({ updateConfig });
  render(<SettingsDialog />);
  const field = screen.getByTestId('input-speleodb-endpoint');
  fireEvent.change(field, { target: { value: 'https://submitted.example' } });
  fireEvent.click(screen.getByTestId('settings-close-button'));
  fireEvent.change(field, { target: { value: 'https://newer.example' } });
  await act(async () => finish());
  expect(closeDialog).not.toHaveBeenCalled();
  expect(field).toHaveValue('https://newer.example');
  fireEvent.click(screen.getByTestId('settings-close-button'));
  expect(updateConfig.mock.calls[1]?.[0].speleodb.endpoint).toBe('https://newer.example');
  await act(async () => finish());
  expect(closeDialog).toHaveBeenCalledOnce();
});

it('keeps category navigation and resets blocked until a failed connection finishes', async () => {
  let finish!: () => void;
  useSpeleoStore.setState({ applyPendingAuth: vi.fn(() => new Promise<{ ok: boolean; error?: unknown }>((resolve) => {
    finish = () => resolve({ ok: false, error: new Error('Synthetic refusal') });
  })) });
  render(<SettingsDialog />);
  fireEvent.click(screen.getByTestId('speleodb-settings-try-login'));
  await act(async () => {});
  fireEvent.click(screen.getByTestId('settings-tab-general'));
  expect(screen.getByTestId('settings-tab-speleodb')).toHaveAttribute('aria-selected', 'true');
  expect(screen.getByTestId('reset-category-defaults')).toBeDisabled();
  await act(async () => finish());
  fireEvent.click(screen.getByTestId('speleodb-connection-error-close'));
  expect(screen.getByTestId('settings-close-button')).toBeEnabled();
  fireEvent.click(screen.getByTestId('settings-tab-general'));
  expect(screen.getByTestId('settings-tab-general')).toHaveAttribute('aria-selected', 'true');
});

it('preserves preference drafts while adopting logout recents and a new connected endpoint', async () => {
  const initial = { ...endpoint('https://original.example'), general: { ...defaultConfig.general, recent_projects: ['/synthetic/local.stalx', '/synthetic/remote.stalx'] } };
  const updateConfig = vi.fn(async (_config: Config) => emptyCommandResult(true));
  useProjectStore.setState({ config: initial, updateConfig });
  render(<SettingsDialog />);
  fireEvent.click(screen.getByTestId('settings-tab-general'));
  fireEvent.click(screen.getByTestId('toggle-auto-save'));
  await act(async () => {
    useProjectStore.setState({ config: { ...endpoint('https://current.example'), general: { ...defaultConfig.general, recent_projects: ['/synthetic/local.stalx'] } } });
    useSpeleoStore.setState({ session: { loggedIn: true, endpoint: 'https://current.example', userEmail: 'synthetic@example.com' }, pendingAuth: null });
  });
  fireEvent.click(screen.getByTestId('settings-tab-speleodb'));
  fireEvent.click(screen.getByTestId('settings-close-button'));
  await act(async () => {});
  expect(updateConfig).toHaveBeenCalledOnce();
  const saved = updateConfig.mock.calls[0]?.[0];
  expect(saved?.general.recent_projects).toEqual(['/synthetic/local.stalx']);
  expect(saved?.general.auto_save).toBe(!defaultConfig.general.auto_save);
  expect(saved?.speleodb.endpoint).toBe('https://current.example');
  expect(closeDialog).toHaveBeenCalledOnce();
});

it.each([true, false])('does not affect replacement Settings when an earlier Save completes with success=%s', async (success) => {
  let finish!: () => void;
  useProjectStore.setState({ updateConfig: vi.fn(() => new Promise<ReturnType<typeof emptyCommandResult>>((resolve) => {
    finish = () => resolve(emptyCommandResult(success, 'Synthetic persistence failure'));
  })) });
  useUIStore.setState({ dialogData: { initialCategory: 'general' } });
  render(<SettingsDialog />);
  fireEvent.click(screen.getByTestId('toggle-auto-save'));
  fireEvent.click(screen.getByTestId('settings-save-button'));
  act(() => useUIStore.setState({ dialogData: { replacement: true } }));
  await act(async () => finish());
  expect(closeDialog).not.toHaveBeenCalled();
  expect(screen.queryByTestId('settings-save-error')).not.toBeInTheDocument();
});
