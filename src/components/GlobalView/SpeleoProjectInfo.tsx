import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import type { SpeleoCachedProject, SpeleoProjectSummary } from '../../types/ipc.generated';
import { SpeleoDbLogo } from '../common/SpeleoDbLogo';
import { permissionAtLeast } from '../../utils/speleoPermission';
import type { SpeleoLockInfo } from './SpeleoLockBadge';

/**
 * The one line that says what the user can do with the project: edit it, wait for
 * someone, or why they cannot edit it at all. Signed out (the backend lists every
 * cached copy, and none can be edited), the answer is the read-only prose.
 */
export function speleoProjectEditLine(
  t: TFunction,
  summary: SpeleoProjectSummary,
  lock: SpeleoLockInfo,
  cached: SpeleoCachedProject | undefined,
): string {
  if (cached?.ownership === 'signed_out') return t('speleodb.cachedCopies.readOnlySignedOut');
  if (lock.lockHolder && lock.lockedByMe) return t('speleodb.projectListing.lockedByYou');
  // A known permission below read-and-write can never edit, whoever holds the lock.
  if (summary.permission && !permissionAtLeast(summary.permission, 'READ_AND_WRITE')) {
    return t('speleodb.cachedCopies.viewOnly');
  }
  if (lock.lockHolder) return t('speleodb.projectListing.lockedBy', { email: lock.lockHolder });
  return t(lock.lockHolder === null ? 'speleodb.projectListing.unlocked' : 'speleodb.projectListing.lockUnknown');
}

/** Server identity, access and lock state, independent of local cache availability. */
export function SpeleoProjectInfo({ summary, lock, cached, testIdPrefix }: {
  summary: SpeleoProjectSummary;
  lock: SpeleoLockInfo;
  /** This computer's copy, when there is one the session may see. */
  cached?: SpeleoCachedProject | undefined;
  testIdPrefix: string;
}) {
  const { t } = useTranslation();
  const signedOut = cached?.ownership === 'signed_out';
  return (
    <div className="space-y-2 text-xs text-text-secondary" data-testid={`${testIdPrefix}-info`}>
      <div className="flex flex-wrap items-center gap-2">
        <span className="inline-flex items-center gap-1.5"><SpeleoDbLogo size={16} />SpeleoDB</span>
        {/* Signed out, the server was not asked: there is no permission to show. */}
        {!signedOut && (
          <span className="rounded bg-background px-2 py-1" data-testid={`${testIdPrefix}-permission`}>
            {t('speleodb.projectListing.permission', {
              permission: t(`speleodb.projectListing.permissions.${summary.permission ?? 'unknown'}`),
            })}
          </span>
        )}
      </div>
      <div className="break-words" data-testid={`${testIdPrefix}-lock`}>
        {speleoProjectEditLine(t, summary, lock, cached)}
      </div>
    </div>
  );
}
