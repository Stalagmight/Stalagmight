/**
 * The DMP wizard's two exit confirmations (split out of DmpNewImportDialog
 * with #576 D4). Because the wizard imports eagerly, both decide what happens
 * to sections already in the project (#394): Cancel offers remove and close,
 * keep and close, or continue editing; Back only removes - the step it returns
 * to re-imports on the way forward. A removal that fails is reported inside
 * the confirmation that asked for it (#576 NF-AN4-N2).
 */
import { useTranslation } from 'react-i18next';
import { Button } from '../../common/Button';
import { AlertDialog } from '../AlertDialog';
import type { DmpWithdrawalFailure } from './useDmpImportRun';
import type { ExitConfirmation } from './wizardState';
import { WIZARD_SECONDARY_BUTTON_CLASS as importSecondaryButtonClass } from './WizardFooter';

export function ExitConfirmations({
  exit,
  withdrawInFlight,
  withdrawalFailure,
  onContinue,
  onKeepAndClose,
  onRemoveAndClose,
  onRemoveAndBack,
}: {
  /** The confirmation open, with the count it quotes. */
  exit: ExitConfirmation | null;
  /** A removal is on the wire: every way out of the confirmation is shut. */
  withdrawInFlight: boolean;
  withdrawalFailure: DmpWithdrawalFailure | null;
  /** Close the confirmation and stay in the wizard. */
  onContinue: () => void;
  /** Cancel: close the wizard, keeping what it imported. */
  onKeepAndClose: () => void;
  onRemoveAndClose: () => void;
  onRemoveAndBack: () => void;
}) {
  const { t } = useTranslation();
  /** How many sections the open confirmation is talking about, taken when it
   *  opened: the live count lives in the run's refs, which do not re-render. */
  const pendingWithdrawCount = exit?.count ?? 0;
  return (
    <>
      {/* Cancel confirmation. Three actions once anything has been imported -
          the exit is no longer just "close a dialog", it decides what happens to
          sections that are already in the project. `confirm-cancel-button` keeps
          its id and its meaning (close, change nothing on disk); the removal is
          the new, additional action. */}
      <AlertDialog
        open={exit?.kind === 'cancel'}
        title={t('dialogs.dmpNewImport.cancelWizard')}
        // A delete is on the wire; every route out of this confirmation is shut
        // until it lands, including Escape and the backdrop. "Keep and close"
        // above all: it would close the wizard over a removal that still
        // completes, leaving the user looking at a project they chose to keep
        // sections in while those sections disappear behind them.
        onClose={() => { if (!withdrawInFlight) onContinue(); }}
        data-testid="cancel-confirm-dialog"
        actions={
          <>
            <Button
              onClick={onContinue}
              disabled={withdrawInFlight}
              className={importSecondaryButtonClass}
              data-testid="cancel-confirm-continue"
            >
              {pendingWithdrawCount > 0
                ? t('dialogs.dmpNewImport.cancelContinueEditing')
                : t('common.actions.continue')}
            </Button>
            <Button
              onClick={onKeepAndClose}
              disabled={withdrawInFlight}
              className={importSecondaryButtonClass}
              data-testid="confirm-cancel-button"
            >
              {pendingWithdrawCount > 0
                ? t('dialogs.dmpNewImport.cancelKeepAndClose')
                : t('common.actions.cancel')}
            </Button>
            {pendingWithdrawCount > 0 && (
              <Button
                variant="danger"
                disabled={withdrawInFlight}
                onClick={onRemoveAndClose}
                data-testid="cancel-remove-button"
              >
                {t('dialogs.dmpNewImport.cancelRemoveAndClose')}
              </Button>
            )}
          </>
        }
      >
        <p data-testid="cancel-confirm-message">
          {pendingWithdrawCount > 0
            ? t('dialogs.dmpNewImport.cancelWizardMessage', { count: pendingWithdrawCount })
            : t('dialogs.dmpNewImport.cancelWizardNoImports')}
        </p>
        {/* Why the removal did not happen, where the user asked for it (#576
            NF-AN4-N2) - not in the panel behind this dialog. */}
        {withdrawalFailure && (
          <p className="mt-3 text-sm text-error" role="alert" data-testid="cancel-confirm-error">
            {withdrawalFailure.message}
          </p>
        )}
      </AlertDialog>

      {/* Back confirmation. No "keep" here: the step Back returns to re-imports
          from scratch on the way forward, so keeping would duplicate. */}
      <AlertDialog
        open={exit?.kind === 'back'}
        title={t('dialogs.dmpNewImport.backWizard')}
        onClose={() => { if (!withdrawInFlight) onContinue(); }}
        data-testid="back-confirm-dialog"
        actions={
          <>
            <Button
              onClick={onContinue}
              disabled={withdrawInFlight}
              className={importSecondaryButtonClass}
              data-testid="back-confirm-continue"
            >
              {t('common.actions.continue')}
            </Button>
            <Button
              variant="danger"
              disabled={withdrawInFlight}
              onClick={onRemoveAndBack}
              data-testid="confirm-back-button"
            >
              {pendingWithdrawCount > 0
                ? t('dialogs.dmpNewImport.backRemoveAndGo')
                : t('common.actions.back')}
            </Button>
          </>
        }
      >
        <p data-testid="back-confirm-message">
          {pendingWithdrawCount > 0
            ? t('dialogs.dmpNewImport.backWizardMessage', { count: pendingWithdrawCount })
            : t('dialogs.dmpNewImport.backWizardNoImports')}
        </p>
        {withdrawalFailure && (
          <p className="mt-3 text-sm text-error" role="alert" data-testid="back-confirm-error">
            {withdrawalFailure.message}
          </p>
        )}
      </AlertDialog>
    </>
  );
}
