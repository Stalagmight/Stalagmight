import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore, type SpeleoLinkState } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { permissionAtLeast } from '../../utils/speleoPermission';
import { speleoReadOnlyReason, speleoReadOnlyReasonKey } from '../../utils/speleoReadOnlyReason';
import { Button } from '../common/Button';
import { AlertTriangleIcon, LinkOffIcon } from '../common/icons';

/**
 * Plain-language lock state for the open project, in the same words as the
 * home-screen badges ("You are editing", "Currently being edited by X",
 * "Ready for editing").
 */
export function speleoLockStateLabel(
  link: SpeleoLinkState,
  t: TFunction,
  session?: { loggedIn: boolean } | null,
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
  if (!link.lockHolder && !link.noAccess && permissionAtLeast(link.permission, 'READ_AND_WRITE')) {
    return t('speleodb.lockState.readyForEditing');
  }
  return t('speleodb.lockState.readOnly');
}

/** Project name, lock and connection state, and the recovery actions they call for. */
export function SpeleoWorkspaceHeader({ link }: { link: SpeleoLinkState }) {
  const { t } = useTranslation();
  const projectName = useProjectStore((s) => s.project?.name);
  const session = useSpeleoStore((s) => s.session);
  return (
    <div className="shrink-0 border-b border-border pb-3 space-y-1" data-testid="speleodb-workspace-project-header">
      <h2 className="font-semibold break-words">{projectName}</h2>
      <p data-testid="speleodb-workspace-lock-state">{speleoLockStateLabel(link, t, session)}</p>
      <p className="text-text-secondary">{!session?.loggedIn ? 'Not connected' : link.serverError ? link.serverError.message : link.remoteState === 'unknown' ? 'Connection unavailable' : 'Connected'}</p>
      {link.noAccess && <Button onClick={() => useUIStore.getState().openDialog('speleodb-no-permission')} icon={<LinkOffIcon size={16} />} data-testid="speleodb-workspace-no-access">Keep a local project</Button>}
      {link.remoteState === 'conflict' && <Button onClick={() => useUIStore.getState().openDialog('speleodb-divergence')} icon={<AlertTriangleIcon size={16} />} data-testid="speleodb-workspace-resolve">Resolve synchronization conflict</Button>}
      {link.hasUnpushedEdits && <p className="text-warning-text">Changes on this computer have not been uploaded.</p>}
    </div>
  );
}
