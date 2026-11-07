import { useCallback, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { SettingsSection } from './SettingsSection';
import { Button } from '../../common/Button';
import { useUIStore } from '../../../store/uiStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import { useProjectStore } from '../../../store/projectStore';
import { expectedOpenProject } from '../../../store/expectedOpenProject';
import { invokeIpc } from '../../../utils/ipc';
import { humanizeSpeleoError } from '../../../utils/speleoError';
import { toastProjectChanged } from '../../../utils/projectChangedNotice';
import { permissionAtLeast } from '../../../utils/speleoPermission';
import { holdsWriteLock } from '../../../utils/quit';
import { convertToUnmanagedStalx } from '../../../utils/projectActions';
import { TOAST_KEYS } from '../../../utils/toastKeys';

const inputClass =
  'w-full px-2 py-1 bg-background border border-border rounded text-sm disabled:opacity-60';

/**
 * [#215] The Admin tab of the SpeleoDB project dialog: in-app lock release and the
 * danger-zone project delete.
 *
 * The tab itself is NOT gated on ADMIN, only the delete section is: releasing your
 * own stuck lock is the one in-app recovery a write-tier user has, so hiding the tab
 * from non-admins would strand them.
 */
export function SpeleoAdminTab() {
  const { t } = useTranslation();
  const showToast = useUIStore((s) => s.showToast);
  const closeDialog = useUIStore((s) => s.closeDialog);
  const link = useSpeleoStore((s) => s.link);
  const refreshLinkStatus = useSpeleoStore((s) => s.refreshLinkStatus);
  const syncProjectLink = useSpeleoStore((s) => s.syncProjectLink);
  const projectName = useProjectStore((s) => s.project?.name ?? '');

  const canManage = permissionAtLeast(link?.permission, 'ADMIN');

  const [releasing, setReleasing] = useState(false);
  const [confirmName, setConfirmName] = useState('');
  const [deleting, setDeleting] = useState(false);

  const handleRelease = useCallback(async () => {
    setReleasing(true);
    try {
      // [#576 RVF-c-C1] Names the project: refused once another is open.
      await invokeIpc('speleodb_release_current_lock', { expectedProject: expectedOpenProject() });
      await refreshLinkStatus();
      showToast(t('speleodb.lock.released'), TOAST_KEYS.SPELEODB_LOCK, 'success');
    } catch (e) {
      // [#576 Q-W44-7-D1] Refused because the project changed: the short line instead.
      if (!toastProjectChanged(e)) {
        showToast(humanizeSpeleoError(e, t('speleodb.lock.releaseFailed')), TOAST_KEYS.SPELEODB_LOCK, 'error');
      }
    } finally {
      setReleasing(false);
    }
  }, [refreshLinkStatus, showToast, t]);

  const handleDelete = useCallback(async () => {
    setDeleting(true);
    try {
      await invokeIpc('speleodb_delete_project', { expectedProject: expectedOpenProject() });
      // The server soft-deleted it and the backend already unlinked locally, so
      // the still-open project is now a plain LOCAL one - say that, rather than
      // just dropping the link.
      //
      // `clearLink()` would leave `projectLinkKind: 'unknown'`, and
      // `selectSpeleoReadOnly` reads unresolved-as-read-only (#270). With the
      // project still open and nothing left to re-resolve it - the status poll
      // arms on `link !== null`, which delete just falsified - that stranded the
      // now-local project permanently read-only until close/reopen.
      syncProjectLink(false);
      showToast(t('speleodb.delete.deleted'), TOAST_KEYS.SPELEODB_DELETE, 'success');
      closeDialog();
    } catch (e) {
      if (!toastProjectChanged(e)) {
        showToast(humanizeSpeleoError(e, t('speleodb.delete.deleteFailed')), TOAST_KEYS.SPELEODB_DELETE, 'error');
      }
      setDeleting(false);
      return;
    }
    // [#576 SDB-DEL-D1] The unlinked copy is still at its SpeleoDB cache path. Ask
    // where to keep it and move it out, exactly as Unlink does (the same Save As in
    // the default projects folder, then `speleodb_unlink`, then the moved copy is
    // loaded, so the session and Recent projects follow it). Cancel leaves the copy
    // unlinked in the cache, where the download guard refuses to replace it.
    await convertToUnmanagedStalx();
  }, [syncProjectLink, closeDialog, showToast, t]);

  const holder = link?.lockHolder ?? null;
  // Shared predicate, and the force-edit exclusion is load-bearing here: a #213
  // force-editor is editing WITHOUT the lock, so telling them the lock is "held by
  // you" and offering Release named a lock they do not have and an action the
  // backend can only refuse. This copy had drifted to the id-only spelling.
  const iHoldLock = holdsWriteLock(link);

  return (
    <div data-testid="speleodb-admin-tab">
      {/* Lock */}
      <SettingsSection title={t('speleodb.lock.title')}>
        {/* Show the release UI whenever a lock is held - by someone else (`holder`
            set) OR by us. After acquiring a lock, `editState` flips to 'editing' but
            `lockHolder` can stay null (the fast start-editing path does not re-seed
            it), so gating on `holder` alone would strand the very stuck-lock recovery
            this section exists for. `iHoldLock` uses the self copy (no holder email). */}
        {holder || iHoldLock ? (
          <>
            <p className="text-sm mb-2" data-testid="speleodb-lock-holder">
              {iHoldLock
                ? t('speleodb.lock.heldByYou')
                : t('speleodb.lock.heldBy', { email: holder })}
            </p>
            {iHoldLock && (
              <p className="text-warning text-xs mb-2" data-testid="speleodb-lock-self-warning">
                {t('speleodb.lock.selfWarning')}
              </p>
            )}
            <Button
              variant="secondary"
              disabled={releasing}
              onClick={() => void handleRelease()}
              data-testid="speleodb-lock-release"
            >
              {releasing ? t('speleodb.lock.releasing') : t('speleodb.lock.releaseButton')}
            </Button>
          </>
        ) : (
          <p className="text-text-secondary text-sm" data-testid="speleodb-lock-none">
            {t('speleodb.lock.none')}
          </p>
        )}
      </SettingsSection>

      {/* Danger zone: delete */}
      {canManage && (
        <SettingsSection title={t('speleodb.delete.title')}>
          <p className="text-text-secondary text-xs mb-2" data-testid="speleodb-delete-warning">
            {t('speleodb.delete.warning')}
          </p>
          <label className="block text-sm mb-1" htmlFor="speleodb-delete-confirm">
            {t('speleodb.delete.confirmLabel', { name: projectName })}
          </label>
          <input
            id="speleodb-delete-confirm"
            type="text"
            value={confirmName}
            disabled={deleting}
            onChange={(e) => setConfirmName(e.target.value)}
            className={inputClass}
            data-testid="speleodb-delete-confirm-input"
          />
          <div className="mt-2">
            <Button
              variant="danger"
              disabled={deleting || confirmName.trim() !== projectName.trim() || projectName === ''}
              onClick={() => void handleDelete()}
              data-testid="speleodb-delete-button"
            >
              {deleting ? t('speleodb.delete.deleting') : t('speleodb.delete.deleteButton')}
            </Button>
          </div>
        </SettingsSection>
      )}
    </div>
  );
}
