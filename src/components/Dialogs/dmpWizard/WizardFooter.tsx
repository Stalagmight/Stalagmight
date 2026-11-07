/**
 * The DMP wizard's footer (split out of DmpNewImportDialog with #576 D4): the
 * two exits on the left, the step's own actions on the right.
 */
import { useEffect, useRef, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { Button } from '../../common/Button';
import { useUIStore } from '../../../store/uiStore';

/** The wizard's secondary buttons: outlined, so the primary action stands out. */
export const WIZARD_SECONDARY_BUTTON_CLASS = 'ring-1 ring-inset ring-border hover:bg-background-50';
const importSecondaryButtonClass = WIZARD_SECONDARY_BUTTON_CLASS;

export function WizardFooter({
  exitBlocked,
  onCancel,
  onBack,
  children,
}: {
  /** Neither exit may open while an import is in flight (see the dialog). */
  exitBlocked: boolean;
  onCancel: () => void;
  onBack: () => void;
  /** The step's actions. */
  children: ReactNode;
}) {
  const { t } = useTranslation();
  // F8: registers this dialog's footer height with the toast stack for as long
  // as the footer is mounted, so a lingering toast (e.g. the manual-placement
  // hint, which fires just before this dialog closes and can still be on
  // screen when it reopens) is raised above the footer instead of covering
  // the Cancel/Back row.
  const footerRef = useRef<HTMLDivElement | null>(null);
  const registerToastBottomOffset = useUIStore((s) => s.registerToastBottomOffset);
  const unregisterToastBottomOffset = useUIStore((s) => s.unregisterToastBottomOffset);
  useEffect(() => {
    const el = footerRef.current;
    if (!el || typeof ResizeObserver === 'undefined') return undefined;
    const owner = 'dmp-import-new-footer';
    const report = () => registerToastBottomOffset(owner, el.getBoundingClientRect().height);
    const ro = new ResizeObserver(report);
    ro.observe(el);
    report();
    return () => {
      ro.disconnect();
      unregisterToastBottomOffset(owner);
    };
  }, [registerToastBottomOffset, unregisterToastBottomOffset]);
  return (
    <div
      ref={footerRef}
      data-testid="dmp-import-dialog-footer"
      className="px-4 py-3 border-t border-border flex justify-between items-center"
    >
      <div className="flex gap-2">
        <Button
          onClick={onCancel}
          disabled={exitBlocked}
          className={importSecondaryButtonClass}
          data-testid="cancel-button"
        >
          {t('common.actions.cancel')}
        </Button>
        <Button
          onClick={onBack}
          disabled={exitBlocked}
          className={importSecondaryButtonClass}
          data-testid="back-button"
        >
          {t('common.actions.back')}
        </Button>
      </div>
      <div className="flex gap-2" data-testid="wizard-suggestion-actions">
        {children}
      </div>
    </div>
  );
}
