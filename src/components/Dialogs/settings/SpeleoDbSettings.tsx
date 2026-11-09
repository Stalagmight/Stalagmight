import { finishSpeleoSettings } from '../../../utils/finishSpeleoSettings';
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
} from './types';
import { defaultConfig } from '../../../types/config';
import { useSpeleoStore, type PendingLogin } from '../../../store/speleoStore';
import { useUIStore } from '../../../store/uiStore';
import { humanizeSpeleoError } from '../../../utils/speleoError';

/** SpeleoDB account and connection settings. */
export function SpeleoDbSettings({
  config,
  onChange,
  changedFields,
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
  const busy = useSpeleoStore((s) => s.busy);

  const speleo = config.speleodb;
  const connectionRef = useRef(speleo);
  connectionRef.current = speleo;
  const [loggingIn, setLoggingIn] = useState(false);
  useEffect(() => {
    void refreshSession();
  }, [refreshSession]);

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
  // Save; on success it closes Settings while the project listing refreshes,
  // on failure it surfaces the error. The credentials required (email or token) -
  // an empty form just reports the backend's "missing credentials".
  const canTryLogin = login.useToken ? login.token.trim().length > 0 : login.email.trim().length > 0;
  const handleTryLogin = async () => {
    // Ensure the typed credentials are staged before applyPendingAuth reads them
    // (zustand `set` is synchronous, so the subsequent read sees this).
    if (pendingAuth?.kind !== 'login') setPendingAuth(login);
    const issuedDialog = useUIStore.getState().dialogData;
    setLoggingIn(true);
    try {
      const result = await applyPendingAuth(speleo.endpoint);
      if (result.cancelled) return;
      // A slow login may finish after Settings was dismissed or replaced.
      // Its session still applies, but it must not close or edit the new dialog.
      const currentDialog = useUIStore.getState();
      if (currentDialog.activeDialog !== 'settings' || currentDialog.dialogData !== issuedDialog) return;
      if (result.ok) {
        const endpoint = useSpeleoStore.getState().session?.endpoint;
        if (endpoint && connectionRef.current === speleo) onChange({ speleodb: { ...speleo, endpoint } });
        finishSpeleoSettings();
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
              variant="danger"
              disabled={busy}
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
                  aria-label={t('settings.speleodb.account.email')}
                  disabled={loggingIn || busy}
                  value={login.email}
                  onChange={(e) => updateLogin({ email: e.target.value })}
                  autoCapitalize="off"
                  autoCorrect="off"
                  spellCheck={false}
                  className="w-full px-2 py-1 bg-background border border-border rounded text-sm"
                  data-testid="speleodb-settings-email"
                />
              </div>
            )}
            <label className="flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                disabled={loggingIn || busy}
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
                  disabled={loggingIn || busy}
                  aria-label={t('settings.speleodb.account.token')}
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
                  disabled={loggingIn || busy}
                  aria-label={t('settings.speleodb.account.password')}
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
                disabled={loggingIn || !canTryLogin || busy}
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
            disabled={busy}
            value={speleo.endpoint ?? ''}
            onChange={(e) => handleEndpointChange(e.target.value)}
            className="w-full max-w-64 px-2 py-1 bg-background border border-border rounded text-sm"
            inputMode="url"
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
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
            disabled={busy}
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

      </SettingsSection>
    </div>
  );
}
