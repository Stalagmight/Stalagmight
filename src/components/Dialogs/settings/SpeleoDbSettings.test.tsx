import { act, fireEvent, render, screen } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { SpeleoDbSettings } from './SpeleoDbSettings';
import { useSpeleoStore } from '../../../store/speleoStore';
import { useUIStore } from '../../../store/uiStore';
import { defaultConfig } from '../../../types/config';

const closeDialog = vi.fn();
const onChange = vi.fn();
let finishLogin: () => void;

beforeEach(() => {
  closeDialog.mockReset();
  onChange.mockReset();
  useUIStore.setState({ activeDialog: 'settings', dialogData: {}, closeDialog });
  useSpeleoStore.setState({
    session: null,
    authError: null,
    refreshSession: vi.fn(async () => {}),
    pendingAuth: { kind: 'login', email: 'synthetic@example.com', password: 'synthetic', token: '', useToken: false },
    applyPendingAuth: vi.fn(() => new Promise<{ ok: true }>((resolve) => {
      finishLogin = () => resolve({ ok: true });
    })),
  });
});

function startLogin() {
  render(
    <SpeleoDbSettings
      config={defaultConfig}
      onChange={onChange}
      changedFields={new Set()}
      onValidationError={vi.fn()}
      panelIdBase="settings-test"
    />,
  );
  fireEvent.click(screen.getByTestId('speleodb-settings-try-login'));
}

it('closes the Settings instance that initiated a successful login', async () => {
  startLogin();
  await act(async () => finishLogin());
  expect(closeDialog).toHaveBeenCalledOnce();
});

it.each(['error', 'settings'])('does not dismiss a replacement %s dialog after a slow login', async (activeDialog) => {
  startLogin();
  act(() => useUIStore.setState({ activeDialog, dialogData: { replacement: true } }));
  await act(async () => finishLogin());
  expect(closeDialog).not.toHaveBeenCalled();
  expect(onChange).not.toHaveBeenCalled();
});

function renderSettings(config = defaultConfig) {
  render(
    <SpeleoDbSettings
      config={config}
      onChange={onChange}
      changedFields={new Set()}
      onValidationError={vi.fn()}
      panelIdBase="settings-test"
    />,
  );
}

// #664: an empty server box must not look like it holds a server, and typing an
// address or email must not be "corrected" (macOS capitalized the first letter).
it('shows an empty server box as unset, not as a server address', () => {
  renderSettings({ ...defaultConfig, speleodb: { ...defaultConfig.speleodb, endpoint: null } });
  const input = screen.getByTestId('input-speleodb-endpoint') as HTMLInputElement;
  expect(input.value).toBe('');
  expect(input.placeholder).toBe('');
});

it.each(['input-speleodb-endpoint', 'speleodb-settings-email'])('turns off text correction on %s', (testId) => {
  useSpeleoStore.setState({ pendingAuth: null });
  renderSettings();
  const input = screen.getByTestId(testId);
  expect(input.getAttribute('autocapitalize')).toBe('off');
  expect(input.getAttribute('autocorrect')).toBe('off');
  expect(input.getAttribute('spellcheck')).toBe('false');
});
