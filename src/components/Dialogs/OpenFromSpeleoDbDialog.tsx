import { LoadingSpinner } from '../common/LoadingSpinner';
import { useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useOpenRemoteProject, type OpenRemoteOutcome } from '../../hooks/useOpenRemoteProject';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { useSpeleoOperationBusy } from '../../hooks/useSpeleoOperationBusy';
import { invokeIpc } from '../../utils/ipc';
import { speleoErrorAction, humanizeSpeleoError } from '../../utils/speleoError';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import type {
  SpeleoProjectSummary,
  SpeleoSessionStatus,
} from '../../types/ipc.generated';

/**
 * Production "Open from SpeleoDB" browser (#203). Lists the remote projects
 * visible to the logged-in account with permission / lock / type badges, filters
 * by name, and pulls + opens one read-only (the backend edit gate keeps it
 * read-only until Start editing - #204). Replaces the dev spike's list half;
 * login now lives in Settings (#205), so this dialog only browses + opens.
 */
export default function OpenFromSpeleoDbDialog() {
  const { t } = useTranslation();
  const closeDialog = useUIStore((s) => s.closeDialog);
  const openDialog = useUIStore((s) => s.openDialog);
  const session = useSpeleoStore((s) => s.session);
  const setSession = useSpeleoStore((s) => s.setSession);
  const clearSession = useSpeleoStore((s) => s.clearSession);
  // Shared pull+open flow (#218): closeDialog runs right before loadProject so its
  // LoadingState progress is visible.
  // `cancelReturnsTo`: the refused-open prompt the hook may raise REPLACES this
  // browser in the single `activeDialog` slot, so its Cancel must hand the user back
  // here - otherwise cancelling leaves nothing open and nothing opened (#270).
  const sessionGeneration = useSpeleoStore((s) => s.sessionGeneration);
  const { openRemote } = useOpenRemoteProject({
    onBeforeLoad: closeDialog,
    cancelReturnsTo: 'open-from-speleodb',
  });

  // Start in 'checking' so the session probe settles before the first paint
  // (avoids a logged-out flash for an already-authenticated session).
  const [phase, setPhase] = useState<'checking' | 'logged-out' | 'list'>('checking');
  const [projects, setProjects] = useState<SpeleoProjectSummary[]>([]);
  const [filter, setFilter] = useState('');
  const [busy, setBusy] = useState(false);
  // The open in flight, if any. While it runs the browser cannot be dismissed
  // (Escape, the X, a backdrop click or Close): its outcome, including a 401 that
  // ends the session, belongs on this surface. Cleared by the open that set it,
  // so a session change resetting `busy` does not unblock it early.
  const [openingRequest, setOpeningRequest] = useState<number | null>(null);
  const opening = openingRequest !== null;
  // A lock, upload or account call already running (an upload, an auto-acquire):
  // opening another project now would start a download and a lock under it.
  const operationBusy = useSpeleoOperationBusy();
  const [error, setError] = useState<string | null>(null);
  // #270: the last project-list fetch failed, so an empty list means "we could not
  // load them", NOT "this account has no projects". Keeps the empty state honest.
  const [listFailed, setListFailed] = useState(false);
  const request = useRef(0);
  // An error raised together with `clearSession()`. The session change re-runs the
  // mount effect, which resets the surface; this carries the message across it so
  // the reason for the sign-out stays on screen.
  const carriedError = useRef<string | null>(null);

  const refreshProjects = async (isCancelled: () => boolean = () => false) => {
    const list = await invokeIpc<SpeleoProjectSummary[]>('speleodb_list_projects');
    if (!isCancelled() && useSpeleoStore.getState().sessionGeneration === sessionGeneration) {
      setProjects(list);
      setListFailed(false);
    }
  };

  useEffect(() => {
    let cancelled = false;
    request.current += 1;
    setBusy(false);
    setError(carriedError.current);
    carriedError.current = null;
    setProjects([]);
    setListFailed(false);
    setPhase('checking');
    const stale = () => cancelled || useSpeleoStore.getState().sessionGeneration !== sessionGeneration;
    void (async () => {
      // Step 1 - the AUTH check. Only THIS deciding we're not signed in may send the
      // user to the login prompt.
      let status: SpeleoSessionStatus;
      try {
        status = await invokeIpc<SpeleoSessionStatus>('speleodb_session');
        if (stale()) return;
        setSession(status);
      } catch (e) {
        if (stale()) return;
        // A dead token surfaces here - drop the session and prompt a re-login.
        const message = humanizeSpeleoError(e, t('speleodb.errors.listFailed'));
        setError(message);
        if (speleoErrorAction(e) === 'ReLogin') {
          carriedError.current = message;
          clearSession();
        }
        setPhase('logged-out');
        return;
      }
      if (!status.logged_in) {
        setPhase('logged-out');
        return;
      }

      // Step 2 - the project LIST fetch, kept separate (#270). A flaky list call on a
      // perfectly valid session used to bounce the user to the login screen; now it
      // leaves them in the authenticated list phase with the inline error + Refresh.
      // Only a genuinely dead token (`ReLogin`) surfacing here escalates to logged-out.
      try {
        await refreshProjects(stale);
        if (!stale()) setPhase('list');
      } catch (e) {
        if (stale()) return;
        setListFailed(true);
        const message = humanizeSpeleoError(e, t('speleodb.errors.listFailed'));
        setError(message);
        if (speleoErrorAction(e) === 'ReLogin') {
          carriedError.current = message;
          clearSession();
          setPhase('logged-out');
        } else {
          setPhase('list');
        }
      }
    })();
    return () => {
      cancelled = true;
      request.current += 1;
    };
  }, [setSession, clearSession, t, sessionGeneration]);

  const filtered = useMemo(() => {
    const q = filter.trim().toLowerCase();
    if (!q) return projects;
    return projects.filter((p) => p.name.toLowerCase().includes(q));
  }, [projects, filter]);

  const handleRefresh = async () => {
    const issued = ++request.current;
    const current = () => issued === request.current && useSpeleoStore.getState().sessionGeneration === sessionGeneration;
    setBusy(true);
    setError(null);
    try {
      await refreshProjects(() => !current());
    } catch (e) {
      if (!current()) return;
      setListFailed(true);
      const message = humanizeSpeleoError(e, t('speleodb.errors.listFailed'));
      setError(message);
      if (speleoErrorAction(e) === 'ReLogin') {
        carriedError.current = message;
        clearSession();
        setPhase('logged-out');
      }
    } finally {
      if (current()) setBusy(false);
    }
  };

  // Route the shared open outcome (#218 `useOpenRemoteProject`) to this dialog's
  // inline surfaces: error -> inline banner (+ re-login drop); the
  // opened / load_failed cases are fully handled by the hook (it already closed the
  // dialog before loading), so there is nothing left to do here.
  const surfaceOpenError = (message: string, reLogin: boolean) => {
    setError(message);
    if (reLogin) {
      carriedError.current = message;
      clearSession();
      setPhase('logged-out');
    }
    setBusy(false);
  };

  // The browser can still lose its slot mid-open (a dialog raised by the exit
  // prompt replaces it). The error must not vanish with it: report it the way
  // Home's remote rows do, and drop a dead session that is still the current one.
  const reportDetachedOpenError = (outcome: Extract<OpenRemoteOutcome, { status: 'error' }>) => {
    if (outcome.reLogin && useSpeleoStore.getState().sessionGeneration === sessionGeneration) {
      useSpeleoStore.getState().clearSession();
    }
    useUIStore.getState().showToast(outcome.message, TOAST_KEYS.SPELEODB_OPEN_REMOTE_ERROR, 'error');
  };

  const handleOpen = async (project: SpeleoProjectSummary) => {
    const issued = ++request.current;
    const current = () => issued === request.current && useSpeleoStore.getState().sessionGeneration === sessionGeneration;
    setBusy(true);
    setOpeningRequest(issued);
    setError(null);
    let outcome: OpenRemoteOutcome;
    try {
      outcome = await openRemote(project);
    } finally {
      setOpeningRequest((open) => (open === issued ? null : open));
      // A refused-open prompt returns to this browser on cancellation.
      if (current()) setBusy(false);
    }
    if (outcome.status !== 'error') return;
    if (current()) surfaceOpenError(outcome.message, outcome.reLogin);
    else reportDetachedOpenError(outcome);
  };

  const goToLogin = () => openDialog('settings', { initialCategory: 'speleodb' });

  const content = (
      <div className="space-y-4">
        {error && (
          <div
            className="p-3 bg-error-10 border border-error-20 rounded text-error text-sm"
            data-testid="open-from-speleodb-error"
          >
            {error}
          </div>
        )}

        {phase === 'checking' ? (
          <div
            data-testid="open-from-speleodb-checking"
            className="flex flex-col items-center justify-center gap-2 py-10"
          >
            <div className="w-6 h-6 border-2 border-accent border-t-transparent rounded-full animate-spin" />
            <span className="text-xs text-text-secondary">{t('speleodb.browser.checking')}</span>
          </div>
        ) : phase === 'logged-out' ? (
          <div className="space-y-3 py-6 text-center" data-testid="open-from-speleodb-logged-out">
            <p className="text-sm text-text-secondary">{t('speleodb.browser.loginPrompt')}</p>
            <Button variant="primary" onClick={goToLogin} data-testid="open-from-speleodb-go-login">
              {t('speleodb.browser.goToSettings')}
            </Button>
          </div>
        ) : (
          <>
            <input
              type="text"
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              className="w-full px-2 py-1 bg-background border border-border rounded text-sm"
              placeholder={t('speleodb.browser.filterPlaceholder')}
              aria-label={t('speleodb.browser.filterPlaceholder')}
              data-testid="open-from-speleodb-filter"
            />

            {filtered.length === 0 ? (
              <p className="text-sm text-text-secondary" data-testid="open-from-speleodb-empty">
                {projects.length > 0
                  ? t('speleodb.browser.noMatches')
                  : listFailed
                    ? t('speleodb.browser.listUnavailable')
                    : t('speleodb.browser.noProjects')}
              </p>
            ) : (
              <ul
                className="divide-y divide-border border border-border rounded max-h-80 overflow-auto"
                data-testid="open-from-speleodb-list"
              >
                {filtered.map((p) => (
                  <li key={p.id} className="flex items-center justify-between gap-3 p-3">
                    <div className="min-w-0">
                      <div className="truncate text-sm text-text-primary">{p.name}</div>
                      <div className="mt-1 flex flex-wrap items-center gap-1.5">
                        <span
                          className={`px-1.5 py-0.5 rounded text-xs ${
                            p.permission === 'READ_ONLY' ? 'bg-warning-20 text-warning' : p.permission ? 'bg-accent-10 text-accent' : 'bg-error-10 text-error'
                          }`}
                        >
                          {p.permission === 'READ_ONLY' ? t('speleodb.browser.readOnly') : p.permission ?? t('speleodb.browser.noAccess')}
                        </span>
                        <span className="px-1.5 py-0.5 rounded text-xs bg-panel border border-border text-text-secondary">
                          {p.type_tag}
                        </span>
                        {p.lock_holder && (
                          <span className="px-1.5 py-0.5 rounded text-xs bg-warning-20 text-warning">
                            {t('speleodb.browser.lockedBy', { email: p.lock_holder })}
                          </span>
                        )}
                      </div>
                    </div>
                    <Button
                      variant="primary"
                      onClick={() => void handleOpen(p)}
                      disabled={busy || operationBusy}
                      data-testid={`open-from-speleodb-open-${p.id}`}
                    >
                      {t('speleodb.browser.open')}
                    </Button>
                  </li>
                ))}
              </ul>
            )}

            <div className="flex items-center justify-between gap-3 pt-2">
              <span className="text-xs text-text-secondary truncate">
                {session?.userEmail
                  ? t('speleodb.browser.signedInAs', { email: session.userEmail })
                  : ''}
              </span>
              <div className="flex gap-2">
                <Button onClick={() => void handleRefresh()} disabled={busy} aria-busy={busy} icon={busy ? <LoadingSpinner testId="open-from-speleodb-refresh-spinner" /> : undefined} data-testid="open-from-speleodb-refresh">
                  {t('speleodb.browser.refresh')}
                </Button>
                <Button onClick={closeDialog} disabled={busy || opening} data-testid="open-from-speleodb-close">
                  {t('common.actions.close')}
                </Button>
              </div>
            </div>
          </>
        )}
      </div>
  );
  return (
    <DialogWrapper title={t('speleodb.browser.title')} onClose={opening ? () => {} : closeDialog} size="lg" data-testid="open-from-speleodb-dialog">
      {content}
    </DialogWrapper>
  );
}
