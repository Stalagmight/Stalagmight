import { finishSpeleoSettings } from '../../../utils/finishSpeleoSettings';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { AlertDialog } from '../AlertDialog';
import { Button } from '../../common/Button';
import { Toggle } from '../../common/Toggle';
import { SettingsSection } from './SettingsSection';
import { SettingRow } from './SettingRow';
import { SpeleoCachePurgeSection } from './SpeleoCachePurgeSection';
import { getPanelId, getTabId } from '../SettingsSidebar';
import type { SettingsCategoryProps } from './types';
import {
  isFieldChanged,
} from './types';
import { defaultConfig } from '../../../types/config';
import { cleanEndpointInput } from '../../../utils/speleoEndpoint';
import { useSpeleoStore, type PendingLogin } from '../../../store/speleoStore';
import { useUIStore } from '../../../store/uiStore';
import { humanizeSpeleoError } from '../../../utils/speleoError';
import { useSpeleoOperationBusy } from '../../../hooks/useSpeleoOperationBusy';

/** SpeleoDB account and connection settings. */
export function SpeleoDbSettings({
  config,
  onChange,
  changedFields,
  panelIdBase,
  saveBeforeConnect,
  onConnectionBusyChange,
}: SettingsCategoryProps) {
  const { t } = useTranslation();
  const session = useSpeleoStore((s) => s.session);
  const refreshSession = useSpeleoStore((s) => s.refreshSession);
  const pendingAuth = useSpeleoStore((s) => s.pendingAuth);
  const setPendingAuth = useSpeleoStore((s) => s.setPendingAuth);
  const applyPendingAuth = useSpeleoStore((s) => s.applyPendingAuth);
  const authError = useSpeleoStore((s) => s.authError);
  // Any lock, upload, download or account call in flight, not just the hand-off
  // verbs: the account and endpoint must not change under one.
  const busy = useSpeleoOperationBusy();

  const speleo = config.speleodb;
  const connectionRef = useRef(speleo);
  connectionRef.current = speleo;
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  const [loggingIn, setLoggingIn] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  useEffect(() => {
    onConnectionBusyChange?.(loggingIn);
    return () => onConnectionBusyChange?.(false);
  }, [loggingIn, onConnectionBusyChange]);
  useEffect(() => {
    void refreshSession();
  }, [refreshSession]);

  // --- staged account (A2) ---
  const login: PendingLogin =
    pendingAuth?.kind === 'login'
      ? pendingAuth
      : { kind: 'login', email: '', password: '', token: '', useToken: false };
  const updateLogin = (patch: Partial<PendingLogin>) => setPendingAuth({ ...login, ...patch });

  const showLoginForm = pendingAuth?.kind === 'login' || !session?.loggedIn;

  // The insecure-HTTP option exists only in a development build (docs/debug-gates.md);
  // there the box on screen is what Connect uses. A release build sends no choice,
  // and its backend ignores the stored one.
  const insecureHttpOption = import.meta.env.DEV;
  // Connected (or about to be), the connection is fixed: changing it is a
  // Disconnect and a fresh Connect.
  const connectionLocked = busy || loggingIn || session?.loggedIn === true;

  // The account-change questions first (the sign-out flow when an account is
  // replaced, unsent drafts the change would drop), then the staged settings are
  // saved, then only the request this form issued authenticates.
  const canTryLogin = login.useToken ? login.token.trim().length > 0 : login.email.trim().length > 0;
  const handleTryLogin = async () => {
    // Ensure the typed credentials are staged before applyPendingAuth reads them
    // (zustand `set` is synchronous, so the subsequent read sees this).
    if (pendingAuth?.kind !== 'login') setPendingAuth(login);
    const issuedDialog = useUIStore.getState().dialogData;
    const issuedAuth = useSpeleoStore.getState().pendingAuth;
    const issuedGeneration = useSpeleoStore.getState().sessionGeneration;
    const ownsConnect = () => {
      const dialog = useUIStore.getState();
      const account = useSpeleoStore.getState();
      return mounted.current && dialog.activeDialog === 'settings' && dialog.dialogData === issuedDialog &&
        account.pendingAuth === issuedAuth && account.sessionGeneration === issuedGeneration;
    };
    setLoggingIn(true);
    try {
      const result = await applyPendingAuth(speleo.endpoint, insecureHttpOption ? speleo.insecure_http : undefined, {
        beforeApply: async () => (!saveBeforeConnect || await saveBeforeConnect()) && ownsConnect(),
      });
      if (result.cancelled) return;
      // A slow login may finish after Settings was dismissed or replaced.
      // Its session still applies, but it must not close or edit the new dialog.
      const currentDialog = useUIStore.getState();
      if (!mounted.current || currentDialog.activeDialog !== 'settings' || currentDialog.dialogData !== issuedDialog) return;
      if (result.ok) {
        const endpoint = useSpeleoStore.getState().session?.endpoint;
        if (endpoint && connectionRef.current === speleo) onChange({ speleodb: { ...speleo, endpoint } });
        finishSpeleoSettings();
      } else {
        // Name the server this sign-in was sent to: the one on screen.
        setFailure(humanizeSpeleoError(result.error, t('speleodb.errors.loginFailed'), { endpoint: speleo.endpoint }));
      }
    } finally {
      setLoggingIn(false);
    }
  };

  // Endpoint preferences are saved on Connect or Close. Pasted input is
  // cleaned of the invisible characters around it (#675) so the field shows
  // exactly what will be saved; the backend cleans again before validating.
  const handleEndpointChange = (v: string) => {
    const cleaned = cleanEndpointInput(v);
    onChange({ speleodb: { ...speleo, endpoint: cleaned.length > 0 ? cleaned.toLowerCase() : null } });
  };


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

        {authError && (
          <div
            className="mb-3 p-3 bg-error-10 border border-error-20 rounded text-error text-sm"
            data-testid="speleodb-account-settings-error"
          >
            {authError}
          </div>
        )}

        {session?.loggedIn && !showLoginForm ? (
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
              disabled={busy || loggingIn}
              onClick={() => {
                // Never a direct logout: applyPendingAuth runs the sign-out flow
                // first (un-uploaded work, locks handed back). Cancelling it leaves
                // the session, and Settings, as they were.
                setLoggingIn(true);
                const logout = { kind: 'logout' } as const;
                setPendingAuth(logout);
                void applyPendingAuth(speleo.endpoint).then((result) => {
                  if (result.cancelled) {
                    if (useSpeleoStore.getState().pendingAuth === logout) setPendingAuth(null);
                  } else if (!result.ok) {
                    setFailure(humanizeSpeleoError(result.error));
                  }
                }).finally(() => setLoggingIn(false));
              }}
              data-testid="speleodb-settings-logout"
            >
              {t('settings.speleodb.account.logout')}
            </Button>
          </div>
        ) : (
          <div className="space-y-3" data-testid="speleodb-account-login-form">
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

      </SettingsSection>

      <SettingsSection
        title={t('settings.speleodb.sections.connection')}
        data-testid="section-speleodb-connection"
      >
        <SettingRow
          label={t('settings.speleodb.endpoint.label')}
          helpText={session?.loggedIn ? t('settings.speleodb.endpoint.disconnectFirst') : t('settings.speleodb.endpoint.help')}
          isChanged={isFieldChanged(changedFields, 'speleodb.endpoint')}
          onReset={() => onChange({ speleodb: { ...speleo, endpoint: defaultConfig.speleodb.endpoint } })}
          isDefault={connectionLocked || speleo.endpoint === defaultConfig.speleodb.endpoint}
          data-testid="setting-speleodb-endpoint"
        >
          <input
            type="text"
            inputMode="url"
            autoCapitalize="none"
            autoCorrect="off"
            spellCheck={false}
            disabled={connectionLocked}
            value={(session?.loggedIn ? session.endpoint : speleo.endpoint) ?? ''}
            onChange={(e) => handleEndpointChange(e.target.value)}
            className="w-full max-w-64 px-2 py-1 bg-background border border-border rounded text-sm"
            placeholder="https://www.speleodb.org/"
            aria-label={t('settings.speleodb.endpoint.aria')}
            data-testid="input-speleodb-endpoint"
          />
        </SettingRow>

        {insecureHttpOption && (
          <>
            <SettingRow
              label={t('settings.speleodb.insecureHttp.label')}
              helpText={t('settings.speleodb.insecureHttp.help')}
              isChanged={isFieldChanged(changedFields, 'speleodb.insecure_http')}
              onReset={() => onChange({ speleodb: { ...speleo, insecure_http: defaultConfig.speleodb.insecure_http } })}
              isDefault={connectionLocked || speleo.insecure_http === defaultConfig.speleodb.insecure_http}
              data-testid="setting-speleodb-insecure-http"
            >
              <Toggle
                disabled={connectionLocked}
                checked={speleo.insecure_http}
                onChange={(insecure_http) => onChange({ speleodb: { ...speleo, insecure_http } })}
                aria-label={t('settings.speleodb.insecureHttp.aria')}
                data-testid="toggle-speleodb-insecure-http"
              />
            </SettingRow>
            {speleo.insecure_http && (
              <div className="mb-2 p-3 bg-warning-20 text-warning-text rounded text-xs" data-testid="speleodb-insecure-http-warning">
                {t('settings.speleodb.insecureHttp.warning')}
              </div>
            )}
          </>
        )}
      </SettingsSection>

      <SpeleoCachePurgeSection />
      <AlertDialog open={failure !== null} title={t('settings.speleodb.account.connectionErrorTitle')} onClose={() => setFailure(null)} closeOnBackdropClick={false} data-testid="speleodb-connection-error" actions={<Button variant="primary" onClick={() => setFailure(null)} data-testid="speleodb-connection-error-close">{t('common.actions.close')}</Button>}>
        <p className="text-error">{failure}</p>
      </AlertDialog>
    </div>
  );
}
