import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore, type SpeleoLinkState } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { emailsMatch } from '../../utils/email';
import { canResumeSpeleoEditing, permissionAtLeast } from '../../utils/speleoPermission';
import { speleoReadOnlyReason, speleoReadOnlyReasonKey } from '../../utils/speleoReadOnlyReason';
import { Button } from '../common/Button';
import { AlertTriangleIcon, LinkOffIcon } from '../common/icons';

/**
 * Plain-language lock state for the open project, in the same words as the
 * home-screen badges ("You are editing", "Currently being edited by X",
 * "Ready for editing").
 *
 * `lastKnown` is the lock holder the last sign-in or lock scan saw for this
 * project (`speleoStore.remoteLocks`): `null` free, `undefined` never seen.
 */
export function speleoLockStateLabel(
  link: SpeleoLinkState,
  t: TFunction,
  session?: { loggedIn: boolean; userEmail?: string | null } | null,
  lastKnown?: string | null,
): string {
  if (link.editState === 'editing') return t('speleodb.lockState.youAreEditing');
  // A lasting reason the project cannot be edited (signed out, view-only access)
  // is said in words, the same words as the map's read-only pill. Only consulted
  // when the caller knows the session.
  const reason = session === undefined ? null : speleoReadOnlyReason(link, session);
  if (reason) return t(speleoReadOnlyReasonKey(reason));
  if (link.lockHolder && link.lockedByOther) {
    return t('speleodb.lockState.beingEditedBy', { email: link.lockHolder });
  }
  // This account holds the lock and this session is not editing under it: it
  // may be another computer's edit session (sdbdemo2 issue 3), and "Read-only"
  // alone left the user to guess why Resume editing was offered.
  if (link.lockHolder && emailsMatch(link.lockHolder, session?.userEmail ?? null)) {
    return t('speleodb.lockState.heldByYourAccount');
  }
  // Demo item 6. While the connection is unavailable (the header's next line
  // says so) the status could not read the lock: no holder is "not known", not
  // "free", and "Ready for editing" beside the read-only badge read as if this
  // copy could be edited. Say it is offline, and what was last known.
  if (session?.loggedIn && offline(link)) {
    if (lastKnown === null) return t('speleodb.lockState.offlineFree');
    if (lastKnown === undefined) return t('speleodb.lockState.offlineUnknown');
    return emailsMatch(lastKnown, session.userEmail ?? null)
      ? t('speleodb.lockState.offlineHeldByYou')
      : t('speleodb.lockState.offlineHeldBy', { email: lastKnown });
  }
  if (!link.lockHolder && !link.noAccess && permissionAtLeast(link.permission, 'READ_AND_WRITE')) {
    return t('speleodb.lockState.readyForEditing');
  }
  return t('speleodb.lockState.readOnly');
}

/** The status could not reach SpeleoDB: the header says "Connection unavailable". */
function offline(link: SpeleoLinkState): boolean {
  return !link.serverError && link.remoteState === 'unknown';
}

/** Project name, lock and connection state, and the recovery actions they call for. */
export function SpeleoWorkspaceHeader({ link }: { link: SpeleoLinkState }) {
  const { t } = useTranslation();
  const projectName = useProjectStore((s) => s.project?.name);
  const session = useSpeleoStore((s) => s.session);
  const lastKnown = useSpeleoStore((s) => (s.remoteProjectId ? s.remoteLocks[s.remoteProjectId] : undefined));
  // Demo item 5: the copy is behind SpeleoDB (an open whose download was lost, or
  // another session's upload since) - said for as long as the status knows it,
  // not only in the open's toast. Resume editing downloads the newer revision
  // before it starts editing (`resume_editing_on_a_copy_behind_downloads_the_
  // newer_revision_first`), so the line says so where that button is offered.
  const behind = link.remoteState === 'remote_ahead';
  const resumes = link.editState !== 'editing' && canResumeSpeleoEditing(link, session);
  return (
    <div className="shrink-0 border-b border-border pb-3 space-y-1" data-testid="speleodb-workspace-project-header">
      <h2 className="font-semibold break-words">{projectName}</h2>
      <p data-testid="speleodb-workspace-lock-state">{speleoLockStateLabel(link, t, session, lastKnown)}</p>
      <p className="text-text-secondary">{!session?.loggedIn ? 'Not connected' : link.serverError ? link.serverError.message : offline(link) ? 'Connection unavailable' : 'Connected'}</p>
      {behind && (
        <p className="text-warning-text" data-testid="speleodb-workspace-newer-on-server">
          {t(resumes ? 'speleodb.workspace.newerOnServerResume' : 'speleodb.workspace.newerOnServer')}
        </p>
      )}
      {link.noAccess && <Button onClick={() => useUIStore.getState().openDialog('speleodb-no-permission')} icon={<LinkOffIcon size={16} />} data-testid="speleodb-workspace-no-access">Keep a local project</Button>}
      {link.remoteState === 'conflict' && <Button onClick={() => useUIStore.getState().openDialog('speleodb-divergence')} icon={<AlertTriangleIcon size={16} />} data-testid="speleodb-workspace-resolve">Resolve synchronization conflict</Button>}
      {link.hasUnpushedEdits && <p className="text-warning-text">Changes on this computer have not been uploaded.</p>}
    </div>
  );
}
