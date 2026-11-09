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
import { useSpeleoStore, type PendingLogin } from '../../../store/speleoStore';
import { humanizeSpeleoError } from '../../../utils/speleoError';

/** SpeleoDB account and connection settings. */
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

  const speleo = config.speleodb;
  const [loggingIn, setLoggingIn] = useState(false);

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
      if (result.ok) {
        const endpoint = useSpeleoStore.getState().session?.endpoint;
        if (endpoint) onChange({ speleodb: { ...speleo, endpoint } });
      } else {
        setAuthError(humanizeSpeleoError(result.error, t('speleodb.errors.loginFailed')));
      }
    } finally {
      setLoggingIn(false);
    }
  };

  // --- connection (staged config) ---
  // Editing is staged. Saving a different server signs out the current session;
  // "Log in now" instead authenticates against the staged server immediately.
  const handleEndpointChange = (v: string) =>
    onChange({ speleodb: { ...speleo, endpoint: v.length > 0 ? v : null } });
  const handleInsecureHttpToggle = createToggleHandler(onChange, (v) => ({
    speleodb: { ...speleo, insecure_http: v },
  }));
  const handleAllowForceEditToggle = createToggleHandler(onChange, (v) => ({
    speleodb: { ...speleo, allow_force_edit: v },
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

        <p className="mt-4 text-xs text-text-secondary" data-testid="speleodb-credential-storage">
          {t('settings.speleodb.credentials.fileStorage')}
        </p>
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

      </SettingsSection>
    </div>
  );
}
