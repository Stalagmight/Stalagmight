/**
 * Component test - the SpeleoDB settings panel.
 *
 * Two things live here:
 *  1. (#213) the "Allow editing without the write lock" opt-in toggle: it stages
 *     `allow_force_edit` through onChange (only that field) and surfaces its
 *     warning while enabled.
 *  2. the credential-storage block: it must report where the API key ACTUALLY is
 *     in every keychain state, and must do so WITHOUT probing - the panel renders
 *     from the session the backend already handed it and issues no keychain call
 *     of its own, because on Linux the probe is what raises the desktop unlock
 *     prompt this whole feature exists to stop.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../../utils/ipc');

// Real copy, so the four states are asserted as the user reads them.
import '../../../i18n';
import { SpeleoDbSettings } from './SpeleoDbSettings';
import { defaultConfig } from '../../../types/config';
import type { Config } from '../../../types/config';
import { useSpeleoStore, type SpeleoSession } from '../../../store/speleoStore';
import { invokeIpcMock } from '../../../test/factories/ipc';

let onChange: ReturnType<typeof vi.fn>;

function renderPanel(overrides: Partial<Config['speleodb']> = {}) {
  const config: Config = {
    ...defaultConfig,
    speleodb: { ...defaultConfig.speleodb, ...overrides },
  };
  render(
    <SpeleoDbSettings
      config={config}
      onChange={onChange}
      changedFields={new Set()}
      onValidationError={vi.fn()}
      panelIdBase="settings"
    />,
  );
  return config;
}

beforeEach(() => {
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue({ logged_in: false });
  onChange = vi.fn();
  // A signed-in-less session so the account panel stays inert.
  useSpeleoStore.setState({ session: null, refreshSession: vi.fn(async () => {}) });
});

describe('SpeleoDbSettings allow-force-edit toggle (#213)', () => {
  it('stages allow_force_edit=true through onChange when toggled on', async () => {
    renderPanel({ allow_force_edit: false });
    await userEvent.click(screen.getByTestId('toggle-speleodb-allow-force-edit'));
    expect(onChange).toHaveBeenCalledWith({
      speleodb: expect.objectContaining({ allow_force_edit: true }),
    });
  });

  it('shows the warning only while enabled', () => {
    renderPanel({ allow_force_edit: true });
    expect(screen.getByTestId('speleodb-allow-force-edit-warning')).toBeInTheDocument();
  });

  it('hides the warning while disabled', () => {
    renderPanel({ allow_force_edit: false });
    expect(screen.queryByTestId('speleodb-allow-force-edit-warning')).not.toBeInTheDocument();
  });
});

describe('SpeleoDbSettings credential storage', () => {
  const signedIn = (o: Partial<SpeleoSession>): SpeleoSession => ({
    loggedIn: true,
    userEmail: 'dan@example.com',
    endpoint: 'https://www.speleodb.org',
    keychainBacked: false,
    storage: 'memory',
    keychain: 'not_attempted',
    ...o,
  });

  const seed = (session: SpeleoSession | null) =>
    useSpeleoStore.setState({
      session,
      // Frozen so the panel's mount effect can't overwrite the seeded session.
      refreshSession: vi.fn(async () => {}),
      retryKeychain: vi.fn(async () => true),
    });

  it('reports a keychain-backed key as durable, with no retry offered', () => {
    seed(signedIn({ keychainBacked: true, storage: 'keychain', keychain: 'available' }));
    renderPanel();

    expect(screen.getByTestId('speleodb-credential-stored-in')).toHaveAttribute(
      'data-storage',
      'keychain',
    );
    expect(screen.getByTestId('speleodb-credential-keyring')).toHaveAttribute(
      'data-keychain',
      'available',
    );
    expect(screen.getByTestId('speleodb-credential-stored-in').textContent).toContain(
      'OS keychain',
    );
    // Nothing to retry: pressing it would be a no-op that costs an unlock prompt.
    expect(
      screen.queryByTestId('speleodb-credential-retry-keychain'),
    ).not.toBeInTheDocument();
  });

  it('DO NOT REGRESS: hides the retry when the keychain is switched OFF in settings', () => {
    // The switch is a standing policy; the retry verb revokes a DECLINE and cannot
    // bypass it. It used to be reported as `declined`, so the button appeared and
    // pressing it did nothing - and the frontend could not tell, because the retry
    // resolves true for any non-throwing response.
    seed(signedIn({ storage: 'plaintext', keychain: 'disabled' }));
    renderPanel();

    expect(
      screen.queryByTestId('speleodb-credential-retry-keychain'),
    ).not.toBeInTheDocument();
  });

  it('DO NOT REGRESS: offers the retry after the keychain is switched back on', () => {
    // Re-enabling leaves the key where the disable put it, and resets the state to
    // `not_attempted` - which the old state allowlist hid, so there was no honest way
    // to move the key back into the keyring short of signing out and in again.
    seed(signedIn({ storage: 'plaintext', keychain: 'not_attempted' }));
    renderPanel();

    expect(screen.getByTestId('speleodb-credential-retry-keychain')).toBeInTheDocument();
  });

  it.each(['declined', 'unavailable'] as const)(
    'still offers the retry for a %s keychain holding the key elsewhere',
    (keychain) => {
      seed(signedIn({ storage: 'plaintext', keychain }));
      renderPanel();
      expect(screen.getByTestId('speleodb-credential-retry-keychain')).toBeInTheDocument();
    },
  );

  it('names the plaintext file - not "memory only" - when the user opted out', () => {
    // The bug this replaced: the old single warning said "kept in memory only (no
    // keychain)" for the plaintext tier too, telling someone whose key is in a file
    // on disk that it never leaves RAM.
    seed(signedIn({ storage: 'plaintext', keychain: 'declined' }));
    renderPanel();

    const where = screen.getByTestId('speleodb-credential-stored-in');
    expect(where).toHaveAttribute('data-storage', 'plaintext');
    expect(where.textContent).toContain('plaintext file');
    // Reworded: the old line said "you chose otherwise when signing in", which was
    // false for a choice made here in Settings.
    expect(screen.getByTestId('speleodb-credential-keyring').textContent).toContain(
      'you chose to keep the key elsewhere',
    );
    expect(screen.getByTestId('speleodb-credential-retry-keychain')).toBeInTheDocument();
  });

  it('says a session-only key will not survive a restart', () => {
    seed(signedIn({ storage: 'memory', keychain: 'unavailable' }));
    renderPanel();

    expect(screen.getByTestId('speleodb-credential-stored-in').textContent).toContain(
      'signed out when the app closes',
    );
    // A broken keyring is retryable too - the user may have unlocked it since.
    expect(screen.getByTestId('speleodb-credential-retry-keychain')).toBeInTheDocument();
  });

  it('says the keyring has not been checked when signed out, and places no key', () => {
    seed(null);
    renderPanel();

    // "not checked yet" is an honest answer we are willing to show: finding out
    // would mean touching the keyring, which is the prompt we are avoiding.
    const keyring = screen.getByTestId('speleodb-credential-keyring');
    expect(keyring).toHaveAttribute('data-keychain', 'not_attempted');
    expect(keyring.textContent).toContain('not checked yet');
    // Signed out there is no key, so no location is claimed.
    expect(screen.queryByTestId('speleodb-credential-stored-in')).not.toBeInTheDocument();
    expect(
      screen.queryByTestId('speleodb-credential-retry-keychain'),
    ).not.toBeInTheDocument();
  });

  it('never probes the keychain just to draw the block', () => {
    seed(signedIn({ storage: 'plaintext', keychain: 'declined' }));
    renderPanel();

    // Rendering issues no credential IPC of any kind. `refreshSession` is stubbed
    // above, so anything reaching invokeIpc here would be the panel going looking.
    expect(invokeIpcMock).not.toHaveBeenCalled();
  });

  it('Try again asks the backend to re-store the held token in the keychain', async () => {
    const retryKeychain = vi.fn(async () => true);
    seed(signedIn({ storage: 'plaintext', keychain: 'declined' }));
    useSpeleoStore.setState({ retryKeychain });
    renderPanel();

    await userEvent.click(screen.getByTestId('speleodb-credential-retry-keychain'));
    expect(retryKeychain).toHaveBeenCalledTimes(1);
  });
});
