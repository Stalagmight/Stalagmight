import { useTranslation } from 'react-i18next';
import { useScreenOccupant } from '../../hooks/useScreenOccupant';
import { cancelPendingQuit, forceQuitNow, setQuitWaitStep, useQuitWait } from '../../utils/quitAfterWork';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';

/**
 * The quit that is waiting for a SpeleoDB transfer to finish (see
 * `utils/quitAfterWork`). An independent overlay, mounted by App after the
 * upload progress overlay, so it sits on top of it: the progress is what the
 * user waits on, this is where they change their mind.
 *
 * - Keep Stalagmight open (also Escape / X / backdrop): the quit is called off,
 *   the transfer carries on.
 * - Quit now: only after a warning that the transfer will be cut off. Never the
 *   default action.
 */
export default function QuitWaitDialog() {
  const waiting = useQuitWait((s) => s.waiting);
  if (!waiting) return null;
  return <QuitWait />;
}

function QuitWait() {
  const { t } = useTranslation();
  useScreenOccupant(true);
  const step = useQuitWait((s) => s.step);
  const error = useQuitWait((s) => s.error);
  const forcing = step === 'forcing';
  const warning = step === 'force' || forcing;
  return (
    <DialogWrapper
      title={warning ? t('quitWait.forceTitle') : t('quitWait.title')}
      onClose={forcing ? () => {} : cancelPendingQuit}
      size="sm"
      data-testid="quit-wait-dialog"
    >
      <div className="space-y-4" data-step={step}>
        {warning ? (
          <>
            <p className="text-sm font-medium text-text-primary" data-testid="quit-wait-force-warning">
              {t('quitWait.forceBody')}
            </p>
            <p className="text-sm text-text-secondary">{t('quitWait.forceDetail')}</p>
          </>
        ) : (
          <div className="flex items-start gap-3" role="status" aria-live="polite" aria-busy="true">
            <svg aria-hidden="true" className="mt-0.5 h-6 w-6 shrink-0 animate-spin text-accent" viewBox="0 0 24 24" fill="none">
              <circle className="opacity-20" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="2.5" />
              <path d="M12 2a10 10 0 0 1 10 10" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" />
            </svg>
            <p className="text-sm text-text-secondary" data-testid="quit-wait-body">{t('quitWait.body')}</p>
          </div>
        )}
        {error && <p role="alert" className="text-sm text-error" data-testid="quit-wait-error">{error}</p>}
        <div className="flex flex-wrap justify-end gap-2 pt-2 border-t border-border">
          {warning ? (
            <>
              <Button onClick={() => setQuitWaitStep('wait')} disabled={forcing} data-primary-action="" data-testid="quit-wait-back">
                {t('quitWait.back')}
              </Button>
              <Button variant="danger" onClick={() => void forceQuitNow()} disabled={forcing} data-testid="quit-wait-force-confirm">
                {forcing ? t('quitWait.forcing') : t('quitWait.forceConfirm')}
              </Button>
            </>
          ) : (
            <>
              <Button variant="danger" onClick={() => setQuitWaitStep('force')} data-testid="quit-wait-force">
                {t('quitWait.force')}
              </Button>
              <Button variant="primary" onClick={cancelPendingQuit} data-primary-action="" data-testid="quit-wait-cancel">
                {t('quitWait.cancel')}
              </Button>
            </>
          )}
        </div>
      </div>
    </DialogWrapper>
  );
}
