/**
 * The DMP wizard's status steps (split out of DmpNewImportDialog with #576 D4):
 * the centred body of the phases that have no panel of their own - `init` (the
 * anchor import, the suggestion call, or why either failed), `placing` (the
 * late import, or why it failed) and `done` - and the footer actions that go
 * with a failure there.
 */
import { useTranslation } from 'react-i18next';
import { Button } from '../../common/Button';
import { CheckCircleIcon } from '../../common/icons';
import type { DmpRunFailure } from './useDmpImportRun';
import type { DonePhase, InitPhase, PlacingPhase } from './wizardState';
import { WIZARD_SECONDARY_BUTTON_CLASS as importSecondaryButtonClass } from './WizardFooter';

export interface StatusStepProps {
  phase: InitPhase | PlacingPhase | DonePhase;
  /** The run is importing (the anchor, or a batch). */
  isImporting: boolean;
  /** The run's failure, typed by the step that failed. */
  failure: DmpRunFailure | null;
  /** How many sections the import in progress places. */
  placingCount: number;
  /** Every selected section is in the project. */
  allPlaced: boolean;
}

export function StatusStep({ phase, isImporting, failure, placingCount, allPlaced }: StatusStepProps) {
  const { t } = useTranslation();
  const error = failure?.message ?? null;
  const isCalculating = phase.kind === 'init' && phase.calculating;
  return (
    <div className="flex-1 flex flex-col items-center justify-center p-6 min-h-0 overflow-auto">
      {/* Init/Importing zero datum */}
      {phase.kind === 'init' && isImporting && (
        <div className="text-center" data-testid="wizard-importing-zero-datum">
          <div className="text-lg mb-2">{t('dialogs.dmpNewImport.importingZeroDatum')}</div>
          <div className="animate-pulse text-text-secondary">...</div>
        </div>
      )}

      {/* A failed step with no panel of its own - the anchor import in
          `init` above all, which used to leave this body blank (#576
          NF-AN4-N1). `placing` shows its failure below, with its Retry. */}
      {phase.kind !== 'placing' && error && !isImporting && !isCalculating && (
        <div
          className="text-center"
          data-testid={failure?.kind === 'suggestions' ? 'wizard-suggestions-failed' : 'wizard-status-failed'}
        >
          <div className="text-lg mb-2 text-error" data-testid="wizard-error">{error}</div>
        </div>
      )}

      {/* Calculating suggestions */}
      {isCalculating && (
        <div className="text-center" data-testid="wizard-calculating">
          <div className="text-lg mb-2">{t('dialogs.dmpNewImport.calculating')}</div>
          <div className="animate-pulse text-text-secondary">...</div>
        </div>
      )}

      {/* Placing - batch import in progress or failed */}
      {phase.kind === 'placing' && (
        <div className="text-center" data-testid="wizard-placing">
          {error && !isImporting ? (
            <>
              <div className="text-lg mb-2 text-error">{error}</div>
            </>
          ) : (
            <>
              <div className="text-lg mb-2">{t('dialogs.dmpNewImport.placing', { count: placingCount })}</div>
              <div className="animate-pulse text-text-secondary">...</div>
            </>
          )}
        </div>
      )}

      {/* Done - all placed */}
      {phase.kind === 'done' && allPlaced && (
        <div className="text-center" data-testid="wizard-all-placed">
          <CheckCircleIcon size={32} className="text-success mx-auto mb-2" />
          <div className="text-lg">{t('dialogs.dmpNewImport.allPlaced')}</div>
        </div>
      )}
    </div>
  );
}

/** The suggestion call failed (#576 D24): ask again, or place by hand. */
export function SuggestionsFailedActions({ onPlaceManually, onRetry }: { onPlaceManually: () => void; onRetry: () => void }) {
  const { t } = useTranslation();
  return (
    <>
      <Button
        onClick={onPlaceManually}
        className={importSecondaryButtonClass}
        data-testid="suggestions-place-manually-button"
      >
        {t('dialogs.dmpNewImport.placeManually')}
      </Button>
      <Button
        variant="primary"
        onClick={onRetry}
        data-testid="suggestions-retry-button"
      >
        {t('common.actions.retry')}
      </Button>
    </>
  );
}

/** The late import failed: Retry re-runs it - the only failure Retry is for. */
export function LateImportRetryAction({ onRetry }: { onRetry: () => void }) {
  const { t } = useTranslation();
  return (
    <Button
      variant="primary"
      onClick={onRetry}
      data-testid="retry-button"
    >
      {t('common.actions.retry')}
    </Button>
  );
}
