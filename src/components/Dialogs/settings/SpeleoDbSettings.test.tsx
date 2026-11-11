import { act, fireEvent, render, screen } from '@testing-library/react';
import { useState } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
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

function startLogin(config = defaultConfig) {
  render(
    <SpeleoDbSettings
      config={config}
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

describe('the insecure-HTTP option (development builds only)', () => {
  afterEach(() => { vi.unstubAllEnvs(); });
  const insecureConfig = { ...defaultConfig, speleodb: { ...defaultConfig.speleodb, endpoint: 'http://lab.example/', insecure_http: true } };

  it('a development build shows it and connects over the on-screen connection, including its value', async () => {
    vi.stubEnv('DEV', true);
    startLogin(insecureConfig);
    expect(screen.getByTestId('toggle-speleodb-insecure-http')).toHaveAttribute('aria-checked', 'true');
    expect(screen.getByTestId('speleodb-insecure-http-warning')).toBeVisible();
    await act(async () => finishLogin());
    expect(useSpeleoStore.getState().applyPendingAuth).toHaveBeenCalledWith('http://lab.example/', true, expect.anything());
  });

  it('a development build stages a change of the box like any other setting', () => {
    vi.stubEnv('DEV', true);
    render(<SpeleoDbSettings config={defaultConfig} onChange={onChange} changedFields={new Set()} onValidationError={vi.fn()} panelIdBase="dev-toggle" />);
    fireEvent.click(screen.getByTestId('toggle-speleodb-insecure-http'));
    expect(onChange).toHaveBeenLastCalledWith({ speleodb: { ...defaultConfig.speleodb, insecure_http: true } });
    // The automatic exceptions are named from their one description.
    const panel = screen.getByTestId('settings-panel-speleodb');
    expect(panel).toHaveTextContent('HTTPS is required except for localhost, 127.0.0.1 and 192.168.x.x addresses.');
    expect(panel).toHaveTextContent('Development builds only.');
    expect(panel).not.toHaveTextContent('$t(');
  });

  it('a development build locks it with the server while connected', () => {
    vi.stubEnv('DEV', true);
    useSpeleoStore.setState({ session: { loggedIn: true, endpoint: 'https://example.com', userEmail: 'test@example.com' }, pendingAuth: null });
    render(<SpeleoDbSettings config={insecureConfig} onChange={onChange} changedFields={new Set()} onValidationError={vi.fn()} panelIdBase="dev-locked" />);
    expect(screen.getByTestId('toggle-speleodb-insecure-http')).toHaveAttribute('aria-disabled', 'true');
    expect(screen.getByTestId('setting-speleodb-insecure-http-reset')).toBeDisabled();
  });

  it('a release build has no control for it and sends no choice, whatever the stored value', async () => {
    vi.stubEnv('DEV', false);
    startLogin(insecureConfig);
    expect(screen.queryByTestId('setting-speleodb-insecure-http')).not.toBeInTheDocument();
    expect(screen.queryByTestId('toggle-speleodb-insecure-http')).not.toBeInTheDocument();
    expect(screen.queryByTestId('speleodb-insecure-http-warning')).not.toBeInTheDocument();
    await act(async () => finishLogin());
    expect(useSpeleoStore.getState().applyPendingAuth).toHaveBeenCalledWith('http://lab.example/', undefined, expect.anything());
  });
});

it.each(['error', 'settings'])('does not dismiss a replacement %s dialog after a slow login', async (activeDialog) => {
  startLogin();
  act(() => useUIStore.setState({ activeDialog, dialogData: { replacement: true } }));
  await act(async () => finishLogin());
  expect(closeDialog).not.toHaveBeenCalled();
  expect(onChange).not.toHaveBeenCalled();
});

it('keeps typed and pasted server endpoints lowercase with keyboard corrections disabled', () => {
  function SettingsHarness() {
    const [config, setConfig] = useState(defaultConfig);
    return (
      <SpeleoDbSettings
        config={config}
        onChange={(patch) => {
          onChange(patch);
          setConfig((current) => ({ ...current, ...patch }));
        }}
        changedFields={new Set()}
        onValidationError={vi.fn()}
        panelIdBase="settings-test"
      />
    );
  }
  render(<SettingsHarness />);
  const endpoint = screen.getByTestId('input-speleodb-endpoint');
  expect(screen.queryByTestId('speleodb-credential-storage')).not.toBeInTheDocument();
  expect(endpoint).toHaveAttribute('autocapitalize', 'none');
  expect(endpoint).toHaveAttribute('autocorrect', 'off');
  expect(endpoint).toHaveAttribute('spellcheck', 'false');
  expect(endpoint).toHaveAttribute('inputmode', 'url');

  fireEvent.change(endpoint, { target: { value: 'Https://' } });
  expect(endpoint).toHaveValue('https://');
  // #675: a paste with whitespace / a zero-width space around it lands clean.
  fireEvent.change(endpoint, { target: { value: ' https://www.speleodb.org\u200B\n' } });
  expect(endpoint).toHaveValue('https://www.speleodb.org');
  expect(onChange).toHaveBeenLastCalledWith({
    speleodb: { ...defaultConfig.speleodb, endpoint: 'https://www.speleodb.org' },
  });
  fireEvent.change(endpoint, { target: { value: '\u200B \uFEFF' } });
  expect(endpoint).toHaveValue('');
  expect(onChange).toHaveBeenLastCalledWith({
    speleodb: { ...defaultConfig.speleodb, endpoint: null },
  });
  fireEvent.change(endpoint, { target: { value: 'HTTPS://WWW.SPELEODB.ORG/API/' } });
  expect(endpoint).toHaveValue('https://www.speleodb.org/api/');
  expect(onChange).toHaveBeenLastCalledWith({
    speleodb: { ...defaultConfig.speleodb, endpoint: 'https://www.speleodb.org/api/' },
  });

  // The standard reset icon every settings row uses, not a text button.
  const reset = screen.getByTestId('setting-speleodb-endpoint-reset');
  expect(reset).toHaveTextContent(/^$/);
  expect(reset).toHaveAttribute('title', 'Reset to default');
  expect(reset.querySelector('svg')).not.toBeNull();
  fireEvent.click(reset);
  expect(endpoint).toHaveValue(defaultConfig.speleodb.endpoint ?? '');
  expect(reset).toBeDisabled();
  fireEvent.change(endpoint, { target: { value: 'Https://' } });
  fireEvent.change(endpoint, { target: { value: '' } });
  expect(endpoint).toHaveValue('');
  expect(onChange).toHaveBeenLastCalledWith({
    speleodb: { ...defaultConfig.speleodb, endpoint: null },
  });
});

it('disconnects immediately and keeps Settings open with the endpoint disabled while connected', async () => {
  const apply = vi.fn(async () => ({ ok: true }));
  useSpeleoStore.setState({ session: { loggedIn: true, endpoint: 'https://example.com', userEmail: 'test@example.com' }, pendingAuth: null, applyPendingAuth: apply });
  render(<SpeleoDbSettings config={defaultConfig} onChange={onChange} changedFields={new Set()} onValidationError={vi.fn()} panelIdBase="immediate" />);
  expect(screen.getByTestId('input-speleodb-endpoint')).toBeDisabled();
  expect(screen.getByTestId('setting-speleodb-endpoint-reset')).toBeDisabled();
  expect(screen.getByTestId('setting-speleodb-endpoint-reset')).toHaveAttribute('title', 'Reset to default');
  await act(async () => fireEvent.click(screen.getByTestId('speleodb-settings-logout')));
  expect(apply).toHaveBeenCalledOnce();
  expect(useSpeleoStore.getState().pendingAuth).toEqual({ kind: 'logout' });
  expect(closeDialog).not.toHaveBeenCalled();
});

it('shows connection rejection in a modal and preserves Settings', async () => {
  useSpeleoStore.setState({ applyPendingAuth: vi.fn(async () => ({ ok: false, error: { message: 'HTTPS is required.', action: 'Fatal' } })) });
  startLogin();
  await act(async () => {});
  expect(screen.getByTestId('speleodb-connection-error')).toBeVisible();
  expect(closeDialog).not.toHaveBeenCalled();
});

it.each(['dialog', 'draft', 'session'] as const)('does not authenticate after a pending preference save loses its %s owner', async (owner) => {
  let finishSave!: (saved: boolean) => void;
  const saveBeforeConnect = vi.fn(() => new Promise<boolean>((resolve) => { finishSave = resolve; }));
  // The store runs `beforeApply` (the save) after its questions and authenticates
  // only when it answers true.
  let authenticated = false;
  const apply = vi.fn(async (_endpoint: string | null, _insecure?: boolean, opts?: { beforeApply?: () => Promise<boolean> }) => {
    if (opts?.beforeApply && !await opts.beforeApply()) return { ok: false, cancelled: true };
    authenticated = true;
    return { ok: true };
  });
  useSpeleoStore.setState({ applyPendingAuth: apply });
  render(<SpeleoDbSettings config={defaultConfig} onChange={onChange} changedFields={new Set()} onValidationError={vi.fn()} panelIdBase="save-owner" saveBeforeConnect={saveBeforeConnect} />);
  fireEvent.click(screen.getByTestId('speleodb-settings-try-login'));
  await act(async () => {
    if (owner === 'dialog') useUIStore.setState({ activeDialog: 'settings', dialogData: { replacement: true } });
    if (owner === 'draft') useSpeleoStore.getState().setPendingAuth({ kind: 'login', email: 'replacement@example.com', password: 'synthetic', token: '', useToken: false });
    if (owner === 'session') useSpeleoStore.setState((state) => ({ sessionGeneration: state.sessionGeneration + 1 }));
    finishSave(true);
  });
  expect(saveBeforeConnect).toHaveBeenCalledOnce();
  expect(authenticated).toBe(false);
  expect(closeDialog).not.toHaveBeenCalled();
});

it('saves the staged settings only from inside the account change, after its questions', async () => {
  const order: string[] = [];
  const saveBeforeConnect = vi.fn(async () => { order.push('save'); return true; });
  const apply = vi.fn(async (_endpoint: string | null, _insecure?: boolean, opts?: { beforeApply?: () => Promise<boolean> }) => {
    order.push('questions');
    if (opts?.beforeApply && !await opts.beforeApply()) return { ok: false, cancelled: true };
    order.push('login');
    return { ok: true };
  });
  useSpeleoStore.setState({ applyPendingAuth: apply });
  render(<SpeleoDbSettings config={defaultConfig} onChange={onChange} changedFields={new Set()} onValidationError={vi.fn()} panelIdBase="order" saveBeforeConnect={saveBeforeConnect} />);
  await act(async () => fireEvent.click(screen.getByTestId('speleodb-settings-try-login')));
  expect(order).toEqual(['questions', 'save', 'login']);
  expect(closeDialog).toHaveBeenCalledOnce();
});

it('a cancelled sign-out flow leaves the user connected, Settings open and no logout staged', async () => {
  const apply = vi.fn(async () => ({ ok: false, cancelled: true }));
  useSpeleoStore.setState({ session: { loggedIn: true, endpoint: 'https://example.com', userEmail: 'test@example.com' }, pendingAuth: null, applyPendingAuth: apply });
  render(<SpeleoDbSettings config={defaultConfig} onChange={onChange} changedFields={new Set()} onValidationError={vi.fn()} panelIdBase="cancelled" />);
  await act(async () => fireEvent.click(screen.getByTestId('speleodb-settings-logout')));
  expect(apply).toHaveBeenCalledOnce();
  expect(useSpeleoStore.getState().pendingAuth).toBeNull();
  expect(screen.getByTestId('speleodb-account-signed-in')).toBeVisible();
  expect(screen.getByTestId('speleodb-settings-logout')).toBeEnabled();
  expect(screen.queryByTestId('speleodb-connection-error')).not.toBeInTheDocument();
  expect(closeDialog).not.toHaveBeenCalled();
});

it('releases the parent busy state when the connection panel unmounts', async () => {
  const onConnectionBusyChange = vi.fn();
  const view = render(<SpeleoDbSettings config={defaultConfig} onChange={onChange} changedFields={new Set()} onValidationError={vi.fn()} panelIdBase="busy-owner" onConnectionBusyChange={onConnectionBusyChange} />);
  fireEvent.click(screen.getByTestId('speleodb-settings-try-login'));
  expect(onConnectionBusyChange).toHaveBeenLastCalledWith(true);
  view.unmount();
  expect(onConnectionBusyChange).toHaveBeenLastCalledWith(false);
  await act(async () => finishLogin());
  expect(closeDialog).not.toHaveBeenCalled();
});
