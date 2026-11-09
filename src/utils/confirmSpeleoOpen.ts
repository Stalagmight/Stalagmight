import { confirm } from '@tauri-apps/plugin-dialog';
import i18n from '../i18n';
import { invokeIpc } from './ipc';
import { useSpeleoStore } from '../store/speleoStore';
import { emailsMatch } from './email';
import { endpointsMatch } from './speleoEndpoint';
import type { SpeleoFileAccess, SpeleoProjectSummary } from '../types/ipc.generated';

/** Warn before opening a project with restricted access. Cancel leaves it unopened. */
export async function confirmSpeleoOpen(summary: SpeleoProjectSummary, endpoint?: string, unavailable = false): Promise<boolean> {
  const session = useSpeleoStore.getState().session;
  const ownLock = session?.loggedIn && endpointsMatch(endpoint ?? session.endpoint, session.endpoint) && emailsMatch(summary.lock_holder, session.userEmail);
  const warnings = [
    unavailable ? 'SpeleoDB access could not be verified. This file will open conservatively until access can be verified.' : null,
    !unavailable && summary.permission === 'READ_ONLY' ? i18n.t('speleodb.browser.readOnlyWarning') : null,
    summary.lock_holder && !ownLock ? i18n.t('speleodb.browser.lockedWarning', { email: summary.lock_holder }) : null,
  ].filter(Boolean);
  if (!warnings.length) return true;
  return confirm(warnings.join('\n\n'), {
    title: summary.name,
    kind: 'warning',
    okLabel: i18n.t('speleodb.browser.open'),
    cancelLabel: i18n.t('common.actions.cancel'),
  });
}

/** Probe disk metadata even before the recents/session caches have loaded. */
export async function confirmSpeleoFileOpen(path: string, current: () => boolean = () => true, localOnly = false): Promise<boolean> {
  const access = await invokeIpc<SpeleoFileAccess | null>('speleodb_project_access_at', localOnly ? { path, localOnly: true } : { path });
  if (!current()) return false;
  return !access || confirmSpeleoOpen(access.project, access.endpoint, access.verification === 'unavailable');
}
