/**
 * Why the Data workspace can't be edited right now, with the way back when
 * there is one - the same words and actions as the map's read-only pills:
 *   - the Closures Computed View: "Resume editing" switches to the Raw Data View;
 *   - a remote SpeleoDB project without the write lock: the lock reason, and
 *     "Resume editing" / "Sign in" when they apply;
 *   - anything else (a mode that is read-only by design): a plain notice.
 * The gate itself is `useEditingBlocked()` (the caller renders this only when
 * it is true); the geometry mode is read here for the message only.
 */
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { LockIcon } from '../common/icons';
import {
  PILL_LABEL_CLS,
  RawDataResumeButton,
  SpeleoLogInButton,
  SpeleoReadOnlyLabel,
  SpeleoResumeEditingButton,
} from '../Map/MapReadOnlyResume';

export function DataReadOnlyBanner() {
  const { t } = useTranslation();
  // Message selection only: anything but the raw (measured) view is the
  // closure-adjusted one. Not a gate - see the file comment.
  const adjustedView = useUIStore((s) => s.geometryMode !== 'measured');
  const remote = useSpeleoStore((s) => s.link !== null || s.projectLinkKind !== 'local');
  const reason = adjustedView ? 'closure' : remote ? 'speleo' : 'other';
  return (
    <div
      className="flex shrink-0 items-start gap-1.5 border-b border-accent bg-panel px-3 py-1.5 text-xs"
      role="status"
      data-testid="data-readonly-banner"
      data-reason={reason}
    >
      <span className="flex h-5 shrink-0 items-center text-accent">
        <LockIcon size={13} />
      </span>
      <div className="flex min-w-0 flex-1 flex-wrap items-center gap-x-2 gap-y-1">
        {reason === 'closure' && (
          <>
            <span className={PILL_LABEL_CLS}>{t('dataWorkspace.readOnly.closure')}</span>
            <RawDataResumeButton />
          </>
        )}
        {reason === 'speleo' && (
          <>
            <SpeleoReadOnlyLabel />
            <SpeleoResumeEditingButton />
            <SpeleoLogInButton />
          </>
        )}
        {reason === 'other' && <span className={PILL_LABEL_CLS}>{t('dataWorkspace.readOnly.generic')}</span>}
      </div>
    </div>
  );
}
