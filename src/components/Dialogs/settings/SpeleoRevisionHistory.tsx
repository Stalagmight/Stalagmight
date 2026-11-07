import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { SettingsSection } from './SettingsSection';
import { Button } from '../../common/Button';
import { useUIStore } from '../../../store/uiStore';
import {
  speleoProjectSessionIssuedFor,
  speleoProjectSessionStillCurrent,
  useSpeleoStore,
} from '../../../store/speleoStore';
import { useProjectStore } from '../../../store/projectStore';
import { useRestoreRevision } from '../../../hooks/useRestoreRevision';
import { useSpeleoWebLink } from '../../../hooks/useSpeleoWebLink';
import { invokeIpc } from '../../../utils/ipc';
import { humanizeSpeleoError, unpushedLocalEdits, dumpCandidates } from '../../../utils/speleoError';
import { toastProjectChanged } from '../../../utils/projectChangedNotice';
import { holdsWriteLock } from '../../../utils/quit';
import { permissionAtLeast } from '../../../utils/speleoPermission';
import { formatDate } from '../../../i18n/format';
import type { SpeleoRevision } from '../../../types/ipc.generated';
import { TOAST_KEYS } from '../../../utils/toastKeys';

/** RFC-3339 -> localized date/time; falls back to the raw string if unparseable. */
function formatWhen(iso: string | null): string {
  if (!iso) return '';
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return formatDate(date, undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

/**
 * [#218] Read-only revision history for the open remote project - the History tab
 * of the SpeleoDB project dialog. Fetches
 * `speleodb_get_revisions` on mount and lists the server-ordered (newest-first)
 * commit chain: short SHA, author, date, message, a merge marker for >1 parent.
 *
 * Each PAST revision offers actions keyed on the edit state:
 * - **Restore** (genuine lock held): a git-forward restore that pulls the revision
 *   into the working copy as an unsaved, pushable edit (via a confirm dialog carrying
 *   the disclaimer + unsaved-changes safeguard).
 * - **View** (read-only): a non-destructive peek - it checks the revision out
 *   read-only, and the SpeleoDB Update action fast-forwards back to the latest.
 * - **Revert** (read-only, upload rights): the whole round trip - take the lock,
 *   check the revision out, upload it, release the lock - so a user who never held
 *   the lock can undo a bad revision and end read-only exactly as they started.
 * - **Neither** (force-editing, i.e. editing WITHOUT the lock): a revision swap would
 *   clobber the unsaved lock-less edits, so the backend refuses both and a hint asks
 *   the user to finish or discard the force-edit session first.
 *
 * The newest commit is the current server state and is marked, not actionable.
 */
export function SpeleoRevisionHistory() {
  const { t } = useTranslation();
  const openDialog = useUIStore((s) => s.openDialog);
  const showToast = useUIStore((s) => s.showToast);
  const projectId = useSpeleoStore((s) => s.remoteProjectId);
  const isForceEditing = useSpeleoStore((s) => s.link?.forceEditing ?? false);
  // A genuine lock-held session (NOT force-editing, which holds no lock) may Restore;
  // a plain read-only session may View. A force-editor gets neither: swapping the tree
  // would clobber their unsaved, lock-less edits, so the backend refuses both and the
  // UI offers no action. Shared predicate - see `holdsWriteLock`.
  const iHoldLock = useSpeleoStore((s) => holdsWriteLock(s.link));
  const openDumpPicker = useSpeleoStore((s) => s.openDumpPicker);
  // A View is read-only in INTENT, but not in effect: the backend swaps the working
  // copy to the chosen revision, so it runs the same un-pushed-edits guard a Restore
  // does and refuses whenever this copy holds work the server has never seen. That
  // state is ordinary here - a push that failed, or a lock released without one,
  // leaves exactly it - so offering a button that can only be refused made the whole
  // section a dead end: the raw "local edits that were never pushed" line for what
  // the user asked to be a peek, with nothing said about how to get out of it.
  // Named up front instead, with the way through (see the hint below).
  const hasUnpushedEdits = useSpeleoStore((s) => s.link?.hasUnpushedEdits ?? false);
  const viewBlocked = !iHoldLock && !isForceEditing && hasUnpushedEdits;
  // Revert takes the lock, uploads the chosen revision and releases again, so it needs
  // upload rights. A `null` permission is "not known" (offline), not "none" - treat it
  // as potentially editable and let the backend gate decide, the same way the status
  // button does, rather than hiding the only route a read-only user has.
  const permission = useSpeleoStore((s) => s.link?.permission ?? null);
  const canRevert = permission === null || permissionAtLeast(permission, 'READ_AND_WRITE');
  // [#576 RV4-FE-SP4] Restore (and the Revert built on it) replace the survey, which the
  // read-only Plan and Export workspaces refuse (RV2-R-15). The hamburger's SpeleoDB menu
  // reaches this tab there, so neither is offered; View, a read-only peek, stays.
  const workspaceReadOnly = useUIStore((s) => s.appMode !== 'survey');
  const projectName = useProjectStore((s) => s.project?.name ?? '');
  const restoreRevision = useRestoreRevision();
  const web = useSpeleoWebLink('revisions');

  const [revisions, setRevisions] = useState<SpeleoRevision[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  // The revision whose read-only View is currently loading (disables that button).
  const [pendingId, setPendingId] = useState<string | null>(null);

  useEffect(() => {
    if (!projectId) return undefined;
    let cancelled = false;
    setRevisions(null);
    setError(null);
    void (async () => {
      try {
        const list = await invokeIpc('speleodb_get_revisions', { projectId });
        if (!cancelled) setRevisions(list);
      } catch (e) {
        if (!cancelled) setError(humanizeSpeleoError(e, t('speleodb.revisions.failed')));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [projectId, t]);

  // Read-only View of a past revision (no lock): check it out for inspection; the
  // SpeleoDB Update then fast-forwards back to the latest. Errors toast (there is no
  // confirm dialog for the non-destructive path).
  const handleView = async (rev: SpeleoRevision, entryPath?: string) => {
    if (!projectId) return;
    setPendingId(rev.id);
    // Captured BEFORE the request; the ambiguity catch checks both.
    const issuedFor = speleoProjectSessionIssuedFor(projectId);
    try {
      const applied = await restoreRevision({
        projectId,
        projectName,
        hexsha: rev.id,
        shortSha: rev.id.slice(0, 8),
        restore: false,
        ...(entryPath ? { entryPath } : {}),
      });
      if (!applied) return;
    } catch (e) {
      // [#258] More than one openable file: a question, not a failure. No lock is
      // ever held on the View path, so the retry is simply this same call again -
      // and re-entering HERE rather than the hook means the retry's own failure
      // lands in the toast below, exactly as a first attempt's would.
      const candidates = dumpCandidates(e);
      // [#258 review] Same issuing-session guard the open verbs use, PLUS the project
      // one they must not use. A View binds to whatever is open, so dismissing this
      // dialog and opening something else while the download is out would raise project
      // A's picker over project B - and its retry cannot succeed, because the backend
      // binds a revision swap to the open project. The owning component is unmounted by
      // then too, so the picker could only sit there showing the mismatch.
      if (candidates && speleoProjectSessionStillCurrent(issuedFor)) {
        openDumpPicker(candidates, 'stalx', (chosen) => handleView(rev, chosen));
        return;
      }
      // The un-pushed-edits refusal is normally pre-empted by `viewBlocked`, but the
      // dirty bit can flip between render and click (an auto-save, a poll), so the
      // race still has to say the same thing the hint does rather than fall through
      // to the backend's raw line.
      // [#576 W55 NF-W53-1] Unless the copy could not be read (`state_unknown`): then
      // nothing is known about un-pushed changes, and the toast says that instead.
      // [#576 Q-W44-7-D1] Refused because the project changed: the short line instead.
      if (toastProjectChanged(e)) return;
      const refusal = unpushedLocalEdits(e);
      showToast(
        refusal
          ? refusal.state_unknown === true
            ? t('speleodb.revisions.viewBlockedByUnreadable')
            : t('speleodb.revisions.viewBlockedByUnpushed')
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
    <SettingsSection title={t('speleodb.revisions.title')} data-testid="speleodb-revisions">
      {/* Sits under the section heading rather than in it: SettingsSection takes a
          plain string title. */}
      <div className="flex justify-end">
        <button
          type="button"
          disabled={web.url === null}
          title={web.url === null ? t('speleodb.web.unavailable') : undefined}
          onClick={web.open}
          className="text-xs text-accent hover:underline disabled:opacity-50 disabled:no-underline"
          data-testid="speleodb-web-revisions"
        >
          {t('speleodb.web.openRevisions')}
        </button>
      </div>
      <div className="mb-2 space-y-1" data-testid="speleodb-revisions-hint">
        <p className="text-xs text-text-secondary">{t('speleodb.revisions.restoreHint')}</p>
        {isForceEditing ? (
          <p className="text-xs text-warning" data-testid="speleodb-revisions-forceedit-hint">
            {t('speleodb.revisions.forceEditActive')}
          </p>
        ) : workspaceReadOnly ? (
          // Ahead of the lock and un-pushed hints: neither remedy they name helps here.
          <p className="text-xs text-warning" data-testid="speleodb-revisions-workspace-hint">
            {t('editGate.workspaceReadOnlyNotice')}
          </p>
        ) : viewBlocked ? (
          // Says WHY View is gone and WHAT clears it, because the block is not
          // permanent: locking the project for editing and uploading the changes
          // resolves it, and Revert stays on offer here precisely because it carries
          // its own explicit discard route for the same changes.
          <p className="text-xs text-warning" data-testid="speleodb-revisions-unpushed-hint">
            {t('speleodb.revisions.viewBlockedByUnpushed')}
          </p>
        ) : (
          !iHoldLock && (
            <p className="text-xs text-warning" data-testid="speleodb-revisions-lock-hint">
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
                        className="px-1.5 py-0.5 rounded text-xs bg-warning-20 text-warning"
                        data-testid={`speleodb-revision-merge-${rev.id}`}
                      >
                        {t('speleodb.revisions.merge')}
                      </span>
                    )}
                    <span className="text-xs text-text-secondary truncate">
                      {rev.author_email || t('speleodb.revisions.unknownAuthor')}
                    </span>
                    {rev.authored_date && (
                      <span className="text-xs text-text-tertiary">
                        · {formatWhen(rev.authored_date)}
                      </span>
                    )}
                  </div>
                  {rev.message && (
                    <p className="mt-0.5 text-sm text-text-primary whitespace-pre-wrap break-words">
                      {rev.message}
                    </p>
                  )}
                </div>
                {!isCurrent && iHoldLock && !workspaceReadOnly && (
                  <Button
                    variant="secondary"
                    size="sm"
                    onClick={() =>
                      openDialog(TOAST_KEYS.SPELEODB_RESTORE, {
                        projectId,
                        projectName,
                        hexsha: rev.id,
                        shortSha: rev.id.slice(0, 8),
                        message: rev.message ?? '',
                      })
                    }
                    data-testid={`speleodb-revision-restore-${rev.id}`}
                  >
                    {t('speleodb.revisions.restore')}
                  </Button>
                )}
                {!isCurrent && !iHoldLock && !isForceEditing && !viewBlocked && (
                  <Button
                    variant="secondary"
                    size="sm"
                    disabled={pendingId !== null}
                    onClick={() => void handleView(rev)}
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
                {!isCurrent && !iHoldLock && !isForceEditing && canRevert && !workspaceReadOnly && (
                  <Button
                    variant="secondary"
                    size="sm"
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
    </SettingsSection>
  );
}
