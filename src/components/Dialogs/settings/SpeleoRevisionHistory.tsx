import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Button } from '../../common/Button';
import { DownloadIcon, ExternalLinkIcon, EyeIcon, UndoIcon } from '../../common/icons';
import { MENU_ICON_COLORS } from '../../common/menuColors';
import { useUIStore } from '../../../store/uiStore';
import {
  useSpeleoStore,
} from '../../../store/speleoStore';
import { useProjectStore } from '../../../store/projectStore';
import { useRestoreRevision } from '../../../hooks/useRestoreRevision';
import { useSpeleoWebLink } from '../../../hooks/useSpeleoWebLink';
import { useSpeleoOperationBusy } from '../../../hooks/useSpeleoOperationBusy';
import { invokeIpc } from '../../../utils/ipc';
import { humanizeSpeleoError, unpushedLocalEdits, } from '../../../utils/speleoError';
import { holdsWriteLock } from '../../../utils/quit';
import { permissionAtLeast } from '../../../utils/speleoPermission';
import { formatRelativeTime } from '../../../i18n/format';
import type { SpeleoRevision } from '../../../types/ipc.generated';
import { TOAST_KEYS } from '../../../utils/toastKeys';
import { formatRevisionTime, parseRevisionDate } from '../../../utils/speleoRevisionTime';

/**
 * sdk#7: a revision's time as the viewer's local time with its GMT offset, and the
 * relative time as the tooltip. `date` is the server time as an instant (see
 * `speleoRevisionTime`); a revision whose time cannot be placed shows none.
 */
function RevisionWhen({ date, id }: { date: Date; id: string }) {
  return (
    <time
      dateTime={date.toISOString()}
      title={formatRelativeTime(date)}
      data-testid={`speleodb-revision-date-${id}`}
    >
      {formatRevisionTime(date)}
    </time>
  );
}

export function SpeleoRevisionHistory() {
  const { t } = useTranslation();
  const openDialog = useUIStore((s) => s.openDialog);
  const showToast = useUIStore((s) => s.showToast);
  const projectId = useSpeleoStore((s) => s.remoteProjectId);
  const iHoldLock = useSpeleoStore((s) => holdsWriteLock(s.link));
  // A View is read-only in INTENT, but not in effect: the backend swaps the working
  // copy to the chosen revision, so it runs the same un-pushed-edits guard a Restore
  // does and refuses whenever this copy holds work the server has never seen. That
  // state is ordinary here - a push that failed, or a lock released without one,
  // leaves exactly it - so offering a button that can only be refused made the whole
  // section a dead end: the raw "local edits that were never pushed" line for what
  // the user asked to be a peek, with nothing said about how to get out of it.
  // Named up front instead, with the way through (see the hint below).
  const hasUnpushedEdits = useSpeleoStore((s) => s.link?.hasUnpushedEdits ?? false);
  const viewBlocked = !iHoldLock && hasUnpushedEdits;
  // Revert takes the lock, uploads the chosen revision and releases again, so it needs
  // upload rights. A `null` permission is "not known" (offline), not "none" - treat it
  // as potentially editable and let the backend gate decide, the same way the status
  // button does, rather than hiding the only route a read-only user has.
  const permission = useSpeleoStore((s) => s.link?.permission ?? null);
  const survey = useUIStore((s) => s.appMode === 'survey');
  const generation = useSpeleoStore((s) => s.sessionGeneration);
  const canRevert = survey && permissionAtLeast(permission, 'READ_AND_WRITE');
  const projectName = useProjectStore((s) => s.project?.name ?? '');
  const restoreRevision = useRestoreRevision();
  const web = useSpeleoWebLink('revisions');

  const [revisions, setRevisions] = useState<SpeleoRevision[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  // The revision whose read-only View is currently loading (disables that button).
  const [pendingId, setPendingId] = useState<string | null>(null);
  // View, Restore and Revert each download a revision (Restore and Revert also
  // lock), so none may start while another lock, upload or download is running.
  const operationBusy = useSpeleoOperationBusy();

  useEffect(() => {
    if (!projectId) return undefined;
    let cancelled = false;
    setRevisions(null);
    setError(null);
    void (async () => {
      try {
        const list = await invokeIpc<SpeleoRevision[]>('speleodb_get_revisions', { projectId });
        if (!cancelled) setRevisions(list);
      } catch (e) {
        if (!cancelled) setError(humanizeSpeleoError(e, t('speleodb.revisions.failed')));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [projectId, generation, t]);

  // Read-only View of a past revision (no lock): check it out for inspection; the
  // SpeleoDB Update then fast-forwards back to the latest. Errors toast (there is no
  // confirm dialog for the non-destructive path).
  const handleView = async (rev: SpeleoRevision) => {
    if (!projectId) return;
    setPendingId(rev.id);
    // Captured BEFORE the request; the ambiguity catch checks both.
    try {
      const applied = await restoreRevision({
        projectId,
        projectName,
        hexsha: rev.id,
        shortSha: rev.id.slice(0, 8),
        restore: false,
      });
      if (!applied) return;
    } catch (e) {
      // The un-pushed-edits refusal is normally pre-empted by `viewBlocked`, but the
      // dirty bit can flip between render and click (an auto-save, a refresh), so the
      // race still has to say the same thing the hint does rather than fall through
      // to the backend's raw line.
      showToast(
        unpushedLocalEdits(e)
          ? t('speleodb.revisions.viewBlockedByUnpushed')
          : humanizeSpeleoError(e, t('speleodb.restore.failed')),
        TOAST_KEYS.SPELEODB_RESTORE,
        'error',
      );
    } finally {
      setPendingId(null);
    }
  };

  // A local project (no open remote) has no history to show.
  if (!projectId) return null;

  return (
    <div data-testid="speleodb-revisions">
      <Button
        icon={<ExternalLinkIcon size={16} />}
        iconColor={MENU_ICON_COLORS.projectDetails}
        disabled={web.url === null}
        title={web.url === null ? t('speleodb.web.unavailable') : undefined}
        onClick={web.open}
        className="w-full"
        data-testid="speleodb-web-revisions"
      >
        {t('speleodb.web.openRevisions')}
      </Button>
      <div className="mt-2 mb-2 space-y-1" data-testid="speleodb-revisions-hint">
        <p className="text-xs text-text-secondary">{t('speleodb.revisions.restoreHint')}</p>
        {viewBlocked ? (
          // Says WHY View is gone and WHAT clears it, because the block is not
          // permanent: locking the project for editing and uploading the changes
          // resolves it, and Revert stays on offer here precisely because it carries
          // its own explicit discard route for the same changes.
          <p className="text-xs text-warning-text" data-testid="speleodb-revisions-unpushed-hint">
            {t('speleodb.revisions.viewBlockedByUnpushed')}
          </p>
        ) : (
          !iHoldLock && (
            <p className="text-xs text-warning-text" data-testid="speleodb-revisions-lock-hint">
              {t('speleodb.revisions.lockToRestore')}
            </p>
          )
        )}
      </div>
      {error ? (
        <p className="text-error text-sm" data-testid="speleodb-revisions-error">
          {error}
        </p>
      ) : revisions === null ? (
        <p className="text-text-secondary text-sm" data-testid="speleodb-revisions-loading">
          {t('speleodb.revisions.loading')}
        </p>
      ) : revisions.length === 0 ? (
        <p className="text-text-secondary text-sm" data-testid="speleodb-revisions-empty">
          {t('speleodb.revisions.empty')}
        </p>
      ) : (
        <ul
          className="divide-y divide-border border border-border rounded max-h-80 overflow-auto"
          data-testid="speleodb-revisions-list"
        >
          {revisions.map((rev, idx) => {
            // Server order is newest-first, so index 0 is the current server HEAD:
            // restoring to it would be a no-op, so it is marked, not restorable.
            const isCurrent = idx === 0;
            const when = rev.authored_date ? parseRevisionDate(rev.authored_date) : null;
            return (
              <li
                key={rev.id}
                className="p-2.5 flex items-start gap-2"
                data-testid={`speleodb-revision-${rev.id}`}
              >
                <div className="flex-1 min-w-0">
                  <div className="flex items-center gap-2 flex-wrap">
                    <code className="px-1.5 py-0.5 rounded bg-panel border border-border text-xs text-text-secondary">
                      {rev.id.slice(0, 8)}
                    </code>
                    {isCurrent && (
                      <span
                        className="px-1.5 py-0.5 rounded text-xs bg-accent-20 text-accent"
                        data-testid={`speleodb-revision-current-${rev.id}`}
                      >
                        {t('speleodb.revisions.current')}
                      </span>
                    )}
                    {rev.parent_ids.length > 1 && (
                      <span
                        className="px-1.5 py-0.5 rounded text-xs bg-warning-20 text-warning-text"
                        data-testid={`speleodb-revision-merge-${rev.id}`}
                      >
                        {t('speleodb.revisions.merge')}
                      </span>
                    )}
                    <span className="text-xs text-text-secondary truncate">
                      {rev.author_email || t('speleodb.revisions.unknownAuthor')}
                    </span>
                    {when && (
                      <span className="text-xs text-text-secondary">
                        · <RevisionWhen date={when} id={rev.id} />
                      </span>
                    )}
                  </div>
                  {rev.message && (
                    <p className="mt-0.5 text-sm text-text-primary whitespace-pre-wrap break-words">
                      {rev.message}
                    </p>
                  )}
                </div>
                {!isCurrent && iHoldLock && survey && (
                  <Button
                    variant="secondary"
                    size="sm"
                    disabled={operationBusy}
                    onClick={() =>
                      openDialog(TOAST_KEYS.SPELEODB_RESTORE, {
                        projectId,
                        projectName,
                        hexsha: rev.id,
                        shortSha: rev.id.slice(0, 8),
                        message: rev.message ?? '',
                      })
                    }
                    icon={<DownloadIcon size={14} />}
                    data-testid={`speleodb-revision-restore-${rev.id}`}
                  >
                    {t('speleodb.revisions.restore')}
                  </Button>
                )}
                {!isCurrent && !iHoldLock && !viewBlocked && (
                  <Button
                    variant="secondary"
                    size="sm"
                    disabled={pendingId !== null || operationBusy}
                    onClick={() => void handleView(rev)}
                    icon={<EyeIcon size={14} />}
                    data-testid={`speleodb-revision-view-${rev.id}`}
                  >
                    {pendingId === rev.id
                      ? t('speleodb.revisions.viewing')
                      : t('speleodb.revisions.view')}
                  </Button>
                )}
                {/* After View, so the non-destructive read is what a scanning eye hits
                    first. Offered even when `viewBlocked`, unlike View: that state used
                    to show no buttons at all, and Revert is the one route out of it -
                    it takes the lock for the user, with the armed discard as the
                    explicit consent for replacing their un-pushed work. */}
                {!isCurrent && !iHoldLock && canRevert && (
                  <Button
                    variant="secondary"
                    size="sm"
                    disabled={operationBusy}
                    onClick={() =>
                      openDialog('speleodb-revert', {
                        projectId,
                        projectName,
                        hexsha: rev.id,
                        shortSha: rev.id.slice(0, 8),
                        returnTo: 'speleodb-project',
                        // The dialog data to reopen WITH. `openDialog(returnTo)`
                        // alone resets `dialogData` to `{}`, so the project dialog
                        // read `tab` as undefined and defaulted to General - every
                        // exit from the revert flow (cancel, success, or a dismissed
                        // error) dropped the user on the metadata form instead of
                        // the revision list they pressed Revert from.
                        returnData: { tab: 'history' },
                      })
                    }
                    icon={<UndoIcon size={14} />}
                    data-testid={`speleodb-revision-revert-${rev.id}`}
                  >
                    {t('speleodb.revert.action')}
                  </Button>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}
