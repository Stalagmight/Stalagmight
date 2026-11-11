import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { selectPushFailure, useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { useScreenOccupant } from '../../hooks/useScreenOccupant';
import {
  closeAfterUpload,
  closeDiscardingLocal,
  closeKeepingLock,
  useSpeleoExitPrompt,
  type ExitCloseOutcome,
  type SpeleoExitRequest,
} from '../../utils/speleoExit';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { commitMessageTooLong } from '../../utils/commitMessage';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { SpeleoExitUploadFields, SpeleoExitUploadProgress, useSpeleoExitUpload } from './SpeleoExitUpload';

type Step = 'choose' | 'upload' | 'discard';

/**
 * The SpeleoDB exit prompt (see `utils/speleoExit` for when and why it is raised).
 * Three answers, each carried out here before the exit continues:
 *
 * - **Upload** - the normal Finish editing upload (`done`: save, upload,
 *   release) with its description field and bounded progress, then close. A
 *   failure keeps the user in the project with the error on screen.
 * - **Keep the lock** - save locally, close without releasing. The next open
 *   resumes editing under the same lock with no prompt.
 * - **Discard** - after a second "lost forever" confirmation, the local copy is
 *   removed and the lock released, so the next open gets SpeleoDB's copy.
 *
 * Laid out like every other dialog in the app (`UnsavedChangesDialog`): the body
 * says what each answer does, and every button sits in the one footer row, Cancel
 * first and the primary last. The upload and discard steps keep the same row
 * (Back, then the step's action).
 *
 * Cancel / Escape / X / backdrop abort the exit. None of them is available while
 * an answer is being carried out (except walking away from a slow upload).
 */
export default function SpeleoExitDialog() {
  const request = useSpeleoExitPrompt((s) => s.request);
  if (!request) return null;
  // Keyed on the request so a replacement prompt starts from a clean slate.
  return <SpeleoExitPrompt key={request.id} request={request} />;
}

function SpeleoExitPrompt({ request }: { request: SpeleoExitRequest }) {
  const { t } = useTranslation();
  useScreenOccupant(true);
  const projectName = useProjectStore((s) => s.project?.name ?? '');
  const pushFailure = useSpeleoStore(selectPushFailure);
  const { upload, uploading, slow, unwound } = useSpeleoExitUpload();
  const [step, setStep] = useState<Step>('choose');
  const [message, setMessage] = useState('');
  const [working, setWorking] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const isCurrent = () =>
    useSpeleoExitPrompt.getState().request?.id === request.id &&
    (useProjectStore.getState().project?.file_path ?? null) === request.projectPath;

  /** Carry out one answer; a failure stays on screen with the project still open. */
  const run = async (work: () => Promise<ExitCloseOutcome | 'stay'>) => {
    if (working) return;
    if (!isCurrent()) {
      request.settle(false);
      return;
    }
    setWorking(true);
    setError(null);
    useSpeleoExitPrompt.setState({ working: true });
    try {
      const outcome = await work();
      if (useSpeleoExitPrompt.getState().request?.id !== request.id || outcome === 'stay') return;
      if (!outcome.ok) {
        setError(outcome.error);
        return;
      }
      if (outcome.warning) {
        useUIStore.getState().showToast(outcome.warning, TOAST_KEYS.SPELEODB_LOCK, 'warning');
      }
      request.settle(true);
    } finally {
      setWorking(false);
      if (useSpeleoExitPrompt.getState().request?.id === request.id) {
        useSpeleoExitPrompt.setState({ working: false });
      }
    }
  };

  const uploadAndClose = () => run(async () => {
    const text = message.trim();
    if (!text || commitMessageTooLong(text)) return 'stay';
    // The upload's result is applied while the same project is open, even if the
    // user has walked away from this prompt meanwhile (frontend#2); only the
    // close that follows needs the prompt.
    const projectCurrent = () => (useProjectStore.getState().project?.file_path ?? null) === request.projectPath;
    const shown = () => useSpeleoExitPrompt.getState().request?.id === request.id;
    const result = await upload(text, projectCurrent, shown);
    if (!isCurrent()) return 'stay';
    if (result === 'diverged') {
      // SpeleoDB moved under us: its own resolution dialog is the error surface,
      // and this prompt gets out of its way. The exit does not happen.
      request.settle(false);
      return 'stay';
    }
    // A failed upload already raised the failure notice shown in this step.
    if (result === 'failed') return 'stay';
    if (result === 'newer') return { ok: false, error: t('speleoExitPrompt.errors.newerChanges') };
    return closeAfterUpload();
  });

  // A slow upload may be walked away from (it can still finish, which releases the
  // lock and leaves the project open, read-only); nothing else in flight may.
  const canDismiss = !working || (uploading && slow);
  const cancel = () => {
    if (canDismiss) request.settle(false);
  };

  const action = t(`speleoExitPrompt.action.${request.reason}`);

  return (
    <>
      <DialogWrapper
        title={step === 'discard' ? t('speleoExitPrompt.discardTitle') : t('speleoExitPrompt.title')}
        onClose={canDismiss ? cancel : () => {}}
        size="md"
        data-testid="speleodb-exit-dialog"
      >
        <div className="space-y-4" data-step={step}>
          {step === 'choose' && (
            <>
              <p className="text-sm text-text-secondary" data-testid="speleodb-exit-body">
                {t('speleoExitPrompt.body', { name: projectName, action })}
              </p>
              <p className="text-sm text-text-secondary" data-testid="speleodb-exit-choices">
                {t('speleoExitPrompt.choices')}
              </p>
            </>
          )}

          {step === 'upload' && (
            <SpeleoExitUploadFields message={message} onMessage={setMessage} disabled={working}
              uploading={uploading} slow={slow} unwound={unwound} testIdPrefix="speleodb-exit" />
          )}

          {step === 'discard' && (
            <>
              <p className="text-sm font-medium text-text-primary" data-testid="speleodb-exit-discard-body">
                {t('speleoExitPrompt.discardBody')}
              </p>
              <p className="text-sm text-text-secondary">{t('speleoExitPrompt.discardDetail')}</p>
            </>
          )}

          {error && !pushFailure && (
            <p role="alert" className="text-sm text-error" data-testid="speleodb-exit-error">{error}</p>
          )}

          <div className="flex flex-wrap justify-end gap-2 pt-4 border-t border-border" data-testid="speleodb-exit-footer">
            {step === 'choose' ? (
              <>
                <Button onClick={cancel} disabled={working} data-testid="speleodb-exit-cancel">
                  {t('common.actions.cancel')}
                </Button>
                <Button variant="danger" onClick={() => setStep('discard')} disabled={working}
                  data-testid="speleodb-exit-discard">
                  {t('speleoExitPrompt.discard')}
                </Button>
                <Button onClick={() => void run(closeKeepingLock)} disabled={working}
                  data-testid="speleodb-exit-keep">
                  {working ? t('speleoExitPrompt.working') : t('speleoExitPrompt.keep')}
                </Button>
                <Button variant="primary" onClick={() => setStep('upload')} disabled={working}
                  data-testid="speleodb-exit-upload">
                  {t('speleoExitPrompt.upload')}
                </Button>
              </>
            ) : (
              <Button onClick={() => { setError(null); setStep('choose'); }} disabled={working}
                data-testid="speleodb-exit-back">
                {t('speleoExitPrompt.back')}
              </Button>
            )}
            {step === 'upload' && (
              <Button variant="primary" onClick={() => void uploadAndClose()} disabled={working || !message.trim() || commitMessageTooLong(message)}
                data-testid="speleodb-exit-upload-confirm">
                {working ? t('speleoExitPrompt.uploading') : t('speleoExitPrompt.uploadConfirm')}
              </Button>
            )}
            {step === 'discard' && (
              <Button variant="danger" onClick={() => void run(closeDiscardingLocal)} disabled={working}
                data-testid="speleodb-exit-discard-confirm">
                {working ? t('speleoExitPrompt.working') : t('speleoExitPrompt.discardConfirm')}
              </Button>
            )}
          </div>
        </div>
      </DialogWrapper>
      <SpeleoExitUploadProgress uploading={uploading} slow={slow} testId="speleodb-exit-upload-progress" />
    </>
  );
}
