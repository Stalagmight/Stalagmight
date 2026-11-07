import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Toggle } from '../../common/Toggle';
import { Button } from '../../common/Button';
import { SettingsSection } from './SettingsSection';
import { SettingRow } from './SettingRow';
import { getPanelId, getTabId } from '../SettingsSidebar';
import type { SettingsCategoryProps } from './types';
import {
  isFieldChanged,
  createToggleHandler,
  createNumericHandler,
  parseIntRadix10,
} from './types';
import { defaultConfig, MIN_POLL_INTERVAL_SECONDS } from '../../../types/config';
import type { KeychainReport, TokenFallback } from '../../../types/ipc.generated';
import { useSpeleoStore, type PendingLogin } from '../../../store/speleoStore';
import { humanizeSpeleoError } from '../../../utils/speleoError';

/**
 * The backend's `KeychainReport` values are snake_case over IPC; the i18n leaves
 * are camelCase. Mapped explicitly rather than transformed, so a value added on
 * the Rust side fails the type check here instead of silently rendering a raw key.
 */
const KEYCHAIN_STATE_KEYS: Record<KeychainReport, string> = {
  not_attempted: 'notAttempted',
  available: 'available',
  unavailable: 'unavailable',
  declined: 'declined',
  disabled: 'disabled',
};

/**
 * SpeleoDB settings category (#205, reworked per feedback A1-A4).
 *
 * - **Account** (shown first): sign-in/out is **staged** - applied only when the
 *   dialog's Save is pressed (`speleoStore.pendingAuth` + `applyPendingAuth`),
 *   matching the rest of Settings - with a "Log in now" escape hatch to verify
 *   credentials immediately. Shows which server the live session is signed in to,
 *   and where the API key is actually being kept.
 * - **Connection** (staged config): endpoint, insecure HTTP, force-edit, and the
 *   remote-state poll interval (floor 30s). The **endpoint** is only the default
 *   for FUTURE logins - the live session is pinned to the server it authenticated
 *   against - so changing it does NOT log the user out.
 *
 * There is deliberately no "Token storage" control here any more. Its two options
 * read as a choice of where the key is kept, but the backend only ever consulted
 * `token_fallback` as a fallback *preference* - it takes effect solely when the
 * keychain fails - so it told a user whose key was safely in the keychain that it
 * lived in a plaintext file. The real choice is now made at the one moment it
 * means anything: the login where the keychain actually failed
 * (`SpeleoStorageChoiceDialog`, which writes this same field). What is left here
 * is the REPORT of where the key ended up, plus a way to reconsider.
 */
export function SpeleoDbSettings({
  config,
  onChange,
  changedFields,
  onValidationError,
  panelIdBase,
}: SettingsCategoryProps) {
  const { t } = useTranslation();
  const session = useSpeleoStore((s) => s.session);
  const refreshSession = useSpeleoStore((s) => s.refreshSession);
  const pendingAuth = useSpeleoStore((s) => s.pendingAuth);
  const setPendingAuth = useSpeleoStore((s) => s.setPendingAuth);
  const clearPendingAuth = useSpeleoStore((s) => s.clearPendingAuth);
  const applyPendingAuth = useSpeleoStore((s) => s.applyPendingAuth);
  const setAuthError = useSpeleoStore((s) => s.setAuthError);
  const authError = useSpeleoStore((s) => s.authError);
  const retryKeychain = useSpeleoStore((s) => s.retryKeychain);
  const setTokenStorage = useSpeleoStore((s) => s.setTokenStorage);

  const speleo = config.speleodb;
  const [loggingIn, setLoggingIn] = useState(false);

  // Reflect the backend session (a rehydrated keychain token = already signed in).
  // Reuses the store's shared `refreshSession` (the same probe the project browser
  // uses) rather than re-implementing the speleodb_session query inline.
  useEffect(() => {
    void refreshSession();
  }, [refreshSession]);

  // Poll interval floor is a hard validation error (blocks Save; backend also
  // clamps a hand-edited file on load).
  const pollHasErrorRef = useRef(false);
  useEffect(() => {
    const hasError = speleo.poll_interval_seconds < MIN_POLL_INTERVAL_SECONDS;
    if (hasError !== pollHasErrorRef.current) {
      pollHasErrorRef.current = hasError;
      onValidationError('speleodb', hasError);
    }
  }, [speleo.poll_interval_seconds, onValidationError]);
  // Clear a lingering validation error when the panel unmounts (switching category
  // only renders the active one) - otherwise a strayed error would block Save with
  // no visible cause.
  useEffect(
    () => () => {
      if (pollHasErrorRef.current) onValidationError('speleodb', false);
    },
    [onValidationError],
  );

  // --- staged account (A2) ---
  const login: PendingLogin =
    pendingAuth?.kind === 'login'
      ? pendingAuth
      : { kind: 'login', email: '', password: '', token: '', useToken: false };
  const updateLogin = (patch: Partial<PendingLogin>) => setPendingAuth({ ...login, ...patch });

  const stagedLogout = pendingAuth?.kind === 'logout';
  const showLoginForm = pendingAuth?.kind === 'login' || (!stagedLogout && !session?.loggedIn);

  // Log in NOW (don't wait for Save) so the user can verify their credentials
  // against the staged endpoint. Applies the same login `applyPendingAuth` runs on
  // Save; on success it clears the staged login + flips the panel to "signed in",
  // on failure it surfaces the error. The credentials required (email or token) -
  // an empty form just reports the backend's "missing credentials".
  const canTryLogin = login.useToken ? login.token.trim().length > 0 : login.email.trim().length > 0;
  const handleTryLogin = async () => {
    // Ensure the typed credentials are staged before applyPendingAuth reads them
    // (zustand `set` is synchronous, so the subsequent read sees this).
    if (pendingAuth?.kind !== 'login') setPendingAuth(login);
    setLoggingIn(true);
    try {
      const result = await applyPendingAuth(speleo.endpoint);
      // [#576 SIGNOUT-PROMPT-D1] A Cancel at the sign-out prompt is no failure.
      if (!result.ok && !result.cancelled) {
        setAuthError(humanizeSpeleoError(result.error, t('speleodb.errors.loginFailed')));
      }
    } finally {
      setLoggingIn(false);
    }
  };

  // --- credential storage (item 1) ---
  // Report-only, and NEVER a probe: on Linux, asking the keyring anything is what
  // raises the desktop unlock prompt, so this panel renders whatever earlier access
  // already taught the backend - including "not checked yet", which is an honest
  // answer rather than a gap to fill in.
  //
  // Signed out there is no key to place, so only the keyring line applies.
  const storedIn = session?.loggedIn
    ? t(`settings.speleodb.credentials.where.${session.storage ?? 'memory'}`)
    : null;
  const keychainState = KEYCHAIN_STATE_KEYS[session?.keychain ?? 'not_attempted'];
  // Offered only where it can change something. That is a statement about WHERE THE
  // TOKEN IS, not about which state was reported: anything held outside the keychain
  // can be moved into it, and anything already in it cannot - pressing there would be
  // a no-op costing an unlock prompt.
  //
  // Keying on the state list instead got both directions wrong. `disabled` used to
  // report as `declined`, so the button appeared for a policy the retry cannot
  // bypass; and re-enabling the keychain leaves `not_attempted` with the token still
  // in a plaintext file, which the list hid - so there was no honest way to migrate
  // it short of signing out.
  //
  // Read from the SESSION, never from `speleo.keychain_disabled`: that is the staged
  // config and the toggle only takes effect on Save, so an unsaved flip would render
  // a control that disagrees with the backend.
  const storageTier = session?.storage ?? 'memory';
  const canRetryKeychain =
    (session?.loggedIn ?? false) && storageTier !== 'keychain' && session?.keychain !== 'disabled';
  const [retryingKeychain, setRetryingKeychain] = useState(false);
  // Credential-storage failures get their OWN slot rather than the store's shared
  // `authError`. That field belongs to the login form: writing a storage message
  // into it told a user "your credentials failed" when what actually failed was
  // moving an already-valid key into the keyring, and clearing it on success wiped
  // a genuine sign-in error they had not read yet.
  const [storageError, setStorageError] = useState<string | null>(null);
  const handleRetryKeychain = async () => {
    setRetryingKeychain(true);
    try {
      // The refreshed session is the whole answer - a keychain that is still broken
      // reports `unavailable` again, so the panel just re-renders the truth.
      const ok = await retryKeychain();
      setStorageError(ok ? null : t('settings.speleodb.credentials.retryFailed'));
    } finally {
      setRetryingKeychain(false);
    }
  };

  // The route back out of a storage choice, without signing out and in again. The
  // login-time prompt is raised only when the keychain fails DURING that login, so
  // without this a user who once answered "keep it in a file" had no way to undo
  // it: the preference persists, and every later fallback silently re-writes the
  // cleartext file. Offered only while a token is actually held in a non-keychain
  // tier, which is the only state either direction can change.
  const [movingToken, setMovingToken] = useState(false);
  const canChooseStorage = (session?.loggedIn ?? false) && storageTier !== 'keychain';
  const handleSetStorage = async (mode: Exclude<TokenFallback, 'unknown'>) => {
    setMovingToken(true);
    try {
      const ok = await setTokenStorage(mode);
      setStorageError(ok ? null : t('settings.speleodb.credentials.moveFailed'));
    } finally {
      setMovingToken(false);
    }
  };

  // --- connection (staged config) ---
  // The endpoint is just the default server for FUTURE logins; the live session is
  // pinned to the server it authenticated against (shown in the Account panel). So
  // changing it here does NOT log the current session out - no side effects, purely
  // staged config applied on Save.
  const handleEndpointChange = (v: string) =>
    onChange({ speleodb: { ...speleo, endpoint: v.length > 0 ? v : null } });
  const handleInsecureHttpToggle = createToggleHandler(onChange, (v) => ({
    speleodb: { ...speleo, insecure_http: v },
  }));
  const handleAllowForceEditToggle = createToggleHandler(onChange, (v) => ({
    speleodb: { ...speleo, allow_force_edit: v },
  }));
  const handleDisableKeychainToggle = createToggleHandler(onChange, (v) => ({
    speleodb: { ...speleo, keychain_disabled: v },
  }));
  const handlePollIntervalChange = createNumericHandler(
    onChange,
    (n) => ({ speleodb: { ...speleo, poll_interval_seconds: n } }),
    parseIntRadix10,
  );

  return (
    <div
      role="tabpanel"
      id={getPanelId(panelIdBase, 'speleodb')}
      aria-labelledby={getTabId(panelIdBase, 'speleodb')}
      data-testid="settings-panel-speleodb"
    >
      <h2 className="text-lg font-medium mb-4">{t('settings.speleodb.title')}</h2>

      <SettingsSection
        title={t('settings.speleodb.sections.account')}
        data-testid="section-speleodb-account"
      >
        <p className="mb-3 text-xs text-text-secondary">
          {t('settings.speleodb.account.appliesOnSave')}
        </p>

        {authError && (
          <div
            className="mb-3 p-3 bg-error-10 border border-error-20 rounded text-error text-sm"
            data-testid="speleodb-account-settings-error"
          >
            {authError}
          </div>
        )}

        {stagedLogout ? (
          <div className="space-y-2" data-testid="speleodb-account-staged-logout">
            <p className="text-sm text-text-primary">
              {t('settings.speleodb.account.willSignOut')}
            </p>
            <Button onClick={clearPendingAuth} data-testid="speleodb-settings-undo-logout">
              {t('settings.speleodb.account.undo')}
            </Button>
          </div>
        ) : session?.loggedIn && !showLoginForm ? (
          <div className="space-y-2" data-testid="speleodb-account-signed-in">
            <p className="text-sm text-text-primary break-all">
              {session.endpoint
                ? t('settings.speleodb.account.signedInAsAt', {
                    email: session.userEmail ?? '',
                    endpoint: session.endpoint,
                  })
                : t('settings.speleodb.account.signedInAs', { email: session.userEmail ?? '' })}
            </p>
            <Button
              onClick={() => setPendingAuth({ kind: 'logout' })}
              data-testid="speleodb-settings-logout"
            >
              {t('settings.speleodb.account.logout')}
            </Button>
          </div>
        ) : (
          <div className="space-y-3" data-testid="speleodb-account-login-form">
            <p className="text-xs text-text-secondary">
              {t('settings.speleodb.account.willSignIn')}
            </p>
            {!login.useToken && (
              <div>
                <label className="block text-sm text-text-secondary mb-1">
                  {t('settings.speleodb.account.email')}
                </label>
                <input
                  type="email"
                  value={login.email}
                  onChange={(e) => updateLogin({ email: e.target.value })}
                  className="w-full px-2 py-1 bg-background border border-border rounded text-sm"
                  data-testid="speleodb-settings-email"
                />
              </div>
            )}
            <label className="flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                checked={login.useToken}
                onChange={(e) => updateLogin({ useToken: e.target.checked })}
                className="rounded"
                data-testid="speleodb-settings-use-token"
              />
              {t('settings.speleodb.account.useToken')}
            </label>
            {login.useToken ? (
              <div>
                <label className="block text-sm text-text-secondary mb-1">
                  {t('settings.speleodb.account.token')}
                </label>
                <input
                  type="password"
                  value={login.token}
                  onChange={(e) => updateLogin({ token: e.target.value })}
                  className="w-full px-2 py-1 bg-background border border-border rounded text-sm"
                  data-testid="speleodb-settings-token"
                />
              </div>
            ) : (
              <div>
                <label className="block text-sm text-text-secondary mb-1">
                  {t('settings.speleodb.account.password')}
                </label>
                <input
                  type="password"
                  value={login.password}
                  onChange={(e) => updateLogin({ password: e.target.value })}
                  className="w-full px-2 py-1 bg-background border border-border rounded text-sm"
                  data-testid="speleodb-settings-password"
                />
              </div>
            )}
            {session?.loggedIn && (
              <p className="text-xs text-text-secondary">
                {t('settings.speleodb.account.replacesSession', { email: session.userEmail ?? '' })}
              </p>
            )}
            <div>
              <Button
                variant="primary"
                onClick={() => void handleTryLogin()}
                disabled={loggingIn || !canTryLogin}
                data-testid="speleodb-settings-try-login"
              >
                {loggingIn
                  ? t('settings.speleodb.account.tryingLogin')
                  : t('settings.speleodb.account.tryLogin')}
              </Button>
            </div>
          </div>
        )}

        <div
          className="mt-4 pt-3 border-t border-border space-y-1"
          data-testid="speleodb-credential-storage"
        >
          <p className="text-sm font-medium text-text-primary">
            {t('settings.speleodb.credentials.title')}
          </p>
          {storedIn && (
            <p
              className={`text-xs ${session?.storage === 'keychain' ? 'text-text-secondary' : 'text-warning'}`}
              data-testid="speleodb-credential-stored-in"
              // The raw tier / report alongside the prose, so a test can pin WHICH
              // of the states is being shown without matching on translated copy.
              data-storage={session?.storage ?? 'memory'}
            >
              {t('settings.speleodb.credentials.storedIn', { where: storedIn })}
            </p>
          )}
          <p
            className="text-xs text-text-secondary"
            data-testid="speleodb-credential-keyring"
            data-keychain={session?.keychain ?? 'not_attempted'}
          >
            {t('settings.speleodb.credentials.keyring', {
              state: t(`settings.speleodb.credentials.keychainState.${keychainState}`),
            })}
          </p>
          <p className="text-xs text-text-secondary">
            {t('settings.speleodb.credentials.keyOnly')}
          </p>
          {/* Cleartext on disk is worth saying plainly, every time the panel is
              open - not only in the one-shot prompt at login that put it there. */}
          {storageTier === 'plaintext' && (
            <p
              className="mt-2 p-2 bg-warning-10 border border-warning-20 rounded text-xs text-warning"
              data-testid="speleodb-plaintext-warning"
            >
              {t('settings.speleodb.credentials.plaintextWarning')}
            </p>
          )}
          {storageError && (
            <p
              className="mt-2 p-2 bg-error-10 border border-error-20 rounded text-xs text-error"
              data-testid="speleodb-credential-storage-error"
            >
              {storageError}
            </p>
          )}
          <div className="flex flex-wrap gap-2 pt-1">
            {canRetryKeychain && (
              <Button
                onClick={() => void handleRetryKeychain()}
                disabled={retryingKeychain || movingToken}
                data-testid="speleodb-credential-retry-keychain"
              >
                {retryingKeychain
                  ? t('settings.speleodb.credentials.retrying')
                  : t('settings.speleodb.credentials.retry')}
              </Button>
            )}
            {canChooseStorage && (
              <Button
                onClick={() =>
                  void handleSetStorage(storageTier === 'plaintext' ? 'session_only' : 'plaintext')
                }
                disabled={retryingKeychain || movingToken}
                data-testid="speleodb-credential-set-storage"
              >
                {storageTier === 'plaintext'
                  ? t('settings.speleodb.credentials.useSessionOnly')
                  : t('settings.speleodb.credentials.usePlaintext')}
              </Button>
            )}
          </div>
        </div>
      </SettingsSection>

      <SettingsSection
        title={t('settings.speleodb.sections.connection')}
        data-testid="section-speleodb-connection"
      >
        <SettingRow
          label={t('settings.speleodb.endpoint.label')}
          helpText={t('settings.speleodb.endpoint.help')}
          isChanged={isFieldChanged(changedFields, 'speleodb.endpoint')}
          onReset={() => onChange({ speleodb: { ...speleo, endpoint: defaultConfig.speleodb.endpoint } })}
          isDefault={speleo.endpoint === defaultConfig.speleodb.endpoint}
          data-testid="setting-speleodb-endpoint"
        >
          <input
            type="text"
            value={speleo.endpoint ?? ''}
            onChange={(e) => handleEndpointChange(e.target.value)}
            className="w-64 px-2 py-1 bg-background border border-border rounded text-sm"
            placeholder="https://www.speleodb.org/"
            aria-label={t('settings.speleodb.endpoint.aria')}
            data-testid="input-speleodb-endpoint"
          />
        </SettingRow>

        <SettingRow
          label={t('settings.speleodb.insecureHttp.label')}
          helpText={t('settings.speleodb.insecureHttp.help')}
          isChanged={isFieldChanged(changedFields, 'speleodb.insecure_http')}
          onReset={() =>
            onChange({ speleodb: { ...speleo, insecure_http: defaultConfig.speleodb.insecure_http } })
          }
          isDefault={speleo.insecure_http === defaultConfig.speleodb.insecure_http}
          data-testid="setting-speleodb-insecure-http"
        >
          <Toggle
            checked={speleo.insecure_http}
            onChange={handleInsecureHttpToggle}
            aria-label={t('settings.speleodb.insecureHttp.aria')}
            data-testid="toggle-speleodb-insecure-http"
          />
        </SettingRow>
        {speleo.insecure_http && (
          <div
            className="mb-2 p-3 bg-warning-20 text-warning rounded text-xs"
            data-testid="speleodb-insecure-http-warning"
          >
            {t('settings.speleodb.insecureHttp.warning')}
          </div>
        )}

        <SettingRow
          label={t('settings.speleodb.allowForceEdit.label')}
          helpText={t('settings.speleodb.allowForceEdit.help')}
          isChanged={isFieldChanged(changedFields, 'speleodb.allow_force_edit')}
          onReset={() =>
            onChange({ speleodb: { ...speleo, allow_force_edit: defaultConfig.speleodb.allow_force_edit } })
          }
          isDefault={speleo.allow_force_edit === defaultConfig.speleodb.allow_force_edit}
          data-testid="setting-speleodb-allow-force-edit"
        >
          <Toggle
            checked={speleo.allow_force_edit}
            onChange={handleAllowForceEditToggle}
            aria-label={t('settings.speleodb.allowForceEdit.aria')}
            data-testid="toggle-speleodb-allow-force-edit"
          />
        </SettingRow>
        {speleo.allow_force_edit && (
          <div
            className="mb-2 p-3 bg-warning-20 text-warning rounded text-xs"
            data-testid="speleodb-allow-force-edit-warning"
          >
            {t('settings.speleodb.allowForceEdit.warning')}
          </div>
        )}

        <SettingRow
          label={t('settings.speleodb.pollInterval.label')}
          helpText={t('settings.speleodb.pollInterval.help', { min: MIN_POLL_INTERVAL_SECONDS })}
          isChanged={isFieldChanged(changedFields, 'speleodb.poll_interval_seconds')}
          onReset={() =>
            onChange({
              speleodb: { ...speleo, poll_interval_seconds: defaultConfig.speleodb.poll_interval_seconds },
            })
          }
          isDefault={speleo.poll_interval_seconds === defaultConfig.speleodb.poll_interval_seconds}
          data-testid="setting-speleodb-poll-interval"
        >
          <input
            type="number"
            inputMode="numeric"
            min={MIN_POLL_INTERVAL_SECONDS}
            value={speleo.poll_interval_seconds}
            onChange={(e) => handlePollIntervalChange(e.target.value)}
            className="w-20 px-2 py-1 bg-background border border-border rounded text-sm text-right"
            aria-label={t('settings.speleodb.pollInterval.aria')}
            data-testid="input-speleodb-poll-interval"
          />
        </SettingRow>
        {speleo.poll_interval_seconds < MIN_POLL_INTERVAL_SECONDS && (
          <div className="mb-2 text-xs text-error" data-testid="speleodb-poll-interval-error">
            {t('settings.speleodb.pollInterval.tooLow', { min: MIN_POLL_INTERVAL_SECONDS })}
          </div>
        )}

        {/* Last, and deliberately a standing instruction rather than an answer to
            one login's failure: on a headless or permanently-locked box there is no
            working keyring to offer, and being asked about it on every sign-in is
            the same nag in a different costume. While this is on nothing goes near
            the keyring on any path - not a store, not a startup read, not a status
            probe - and the API key lives wherever the storage choice above says. */}
        <SettingRow
          label={t('settings.speleodb.disableKeychain.label')}
          helpText={t('settings.speleodb.disableKeychain.help')}
          isChanged={isFieldChanged(changedFields, 'speleodb.keychain_disabled')}
          onReset={() =>
            onChange({
              speleodb: { ...speleo, keychain_disabled: defaultConfig.speleodb.keychain_disabled },
            })
          }
          isDefault={speleo.keychain_disabled === defaultConfig.speleodb.keychain_disabled}
          data-testid="setting-speleodb-disable-keychain"
        >
          <Toggle
            checked={speleo.keychain_disabled}
            onChange={handleDisableKeychainToggle}
            aria-label={t('settings.speleodb.disableKeychain.aria')}
            data-testid="toggle-speleodb-disable-keychain"
          />
        </SettingRow>
        {speleo.keychain_disabled && (
          <div
            className="mb-2 p-3 bg-warning-20 text-warning rounded text-xs"
            data-testid="speleodb-disable-keychain-warning"
          >
            {t('settings.speleodb.disableKeychain.warning')}
          </div>
        )}
      </SettingsSection>
    </div>
  );
}
