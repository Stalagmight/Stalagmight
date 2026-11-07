import { useTranslation } from 'react-i18next';
import { useInternalErrorStore } from '../../store/internalErrorStore';

/**
 * [#576 A11] The backend hit an internal error (a crash left one of its locks
 * poisoned), recovered the data and carried on. Saving still works; a few actions
 * that would replay or publish what the crash may have half-written (undo, redo,
 * pushing to SpeleoDB) refuse until a restart.
 *
 * An ongoing condition, so a banner (docs/notifications.md), and the most severe one
 * the app shows: `role="alert"`, error colours, first in the stack. No Dismiss and no
 * action button - it lasts until the restart it asks for, and the remedy (save, then
 * restart) is the user's to take.
 */
export function InternalErrorBanner() {
  const { t } = useTranslation();
  const raised = useInternalErrorStore((s) => s.raised);
  if (!raised) return null;
  return (
    <div
      className="bg-error-10 border-b border-error-20 px-4 py-2 flex items-center gap-3 text-sm"
      role="alert"
      data-testid="internal-error-banner"
    >
      <span className="text-error font-medium flex-1 min-w-0">
        {t('banners.internalError.lead')}{' '}
        <span className="text-text-secondary font-normal">{t('banners.internalError.detail')}</span>
      </span>
    </div>
  );
}
