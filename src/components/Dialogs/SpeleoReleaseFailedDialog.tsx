import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { useScreenOccupant } from '../../hooks/useScreenOccupant';
import { useReleaseFailurePrompt, type ReleaseFailureRequest } from '../../utils/speleoReleaseFailure';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';

/**
 * "Couldn't hand back the lock" (see `utils/speleoReleaseFailure` for when it is
 * raised). Retry runs the holder-checked release again and stays up, with the new
 * reason, while it keeps failing; Close and keep the lock leaves on purpose;
 * Cancel / Escape / X / backdrop stay in the project, read-only. Nothing but the
 * retry itself can be pressed while a retry is in flight.
 *
 * Laid out like every other dialog in the app (`UnsavedChangesDialog`): the body
 * says what went wrong and what each answer does, and every button sits in the
 * one footer row, Cancel first and Retry (the primary) last.
 */
export default function SpeleoReleaseFailedDialog() {
  const request = useReleaseFailurePrompt((s) => s.request);
  if (!request) return null;
  // Keyed on the request so a replacement starts from a clean slate.
  return <ReleaseFailedPrompt key={request.id} request={request} />;
}

function ReleaseFailedPrompt({ request }: { request: ReleaseFailureRequest }) {
  const { t } = useTranslation();
  useScreenOccupant(true);
  const projectName = useProjectStore((s) => s.project?.name ?? '');
  const [working, setWorking] = useState(false);
  const [detail, setDetail] = useState(request.detail);

  const shown = () => useReleaseFailurePrompt.getState().request?.id === request.id;

  const retry = async () => {
    if (working || !shown()) return;
    setWorking(true);
    useReleaseFailurePrompt.setState({ working: true });
    try {
      const outcome = await request.retry();
      if (!shown()) return;
      if (outcome.ok) request.settle('settled');
      else setDetail(outcome.detail);
    } finally {
      setWorking(false);
      if (shown()) useReleaseFailurePrompt.setState({ working: false });
    }
  };

  const cancel = () => {
    if (!working) request.settle('cancel');
  };

  return (
    <DialogWrapper
      title={t('speleoReleaseFailed.title')}
      onClose={cancel}
      size="md"
      data-testid="speleodb-release-failed-dialog"
    >
      <div className="space-y-4" data-reason={request.reason}>
        <p className="text-sm text-text-secondary" data-testid="speleodb-release-failed-body">
          {t('speleoReleaseFailed.body', { name: projectName, action: t(`speleoExitPrompt.action.${request.reason}`) })}
        </p>
        {detail && (
          <p role="alert" className="text-xs text-text-secondary break-words" data-testid="speleodb-release-failed-reason">
            {t('speleoReleaseFailed.reason', { reason: detail })}
          </p>
        )}
        <p className="text-sm text-text-secondary" data-testid="speleodb-release-failed-choices">
          {t('speleoReleaseFailed.choices')}
        </p>
        <div className="flex flex-wrap justify-end gap-2 pt-4 border-t border-border" data-testid="speleodb-release-failed-footer">
          <Button onClick={cancel} disabled={working} data-testid="speleodb-release-failed-cancel">
            {t('common.actions.cancel')}
          </Button>
          <Button onClick={() => request.settle('keep')} disabled={working}
            data-testid="speleodb-release-failed-keep">
            {t('speleoReleaseFailed.keep')}
          </Button>
          <Button variant="primary" onClick={() => void retry()} disabled={working}
            data-testid="speleodb-release-failed-retry">
            {working ? t('speleoReleaseFailed.retrying') : t('speleoReleaseFailed.retry')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
