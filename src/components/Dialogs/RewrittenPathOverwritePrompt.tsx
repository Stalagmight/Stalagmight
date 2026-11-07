import { useTranslation } from 'react-i18next';
import { useOverwritePromptStore } from '../../utils/overwriteGuard';
import { Button } from '../common/Button';
import { AlertDialog } from './AlertDialog';

/**
 * The "replace?" question for a save path the app rewrote after the OS dialog
 * returned it (#576 D35) - see `utils/overwriteGuard`. Mounted once in App and
 * layered over whatever dialog is open (an alert, not the `activeDialog` slot),
 * so the Export dialog or SpeleoDB prompt waiting for the answer stays put.
 */
export default function RewrittenPathOverwritePrompt() {
  const { t } = useTranslation();
  const request = useOverwritePromptStore((s) => s.request);

  if (!request) return null;
  const answer = (replace: boolean) => request.resolve(replace);

  return (
    <AlertDialog
      open
      title={t('dialogs.overwriteRewritten.title')}
      onClose={() => answer(false)}
      data-testid="overwrite-rewritten-prompt"
      actions={
        <>
          <Button onClick={() => answer(false)} data-testid="overwrite-rewritten-cancel">
            {t('common.actions.cancel')}
          </Button>
          <Button
            variant="danger"
            onClick={() => answer(true)}
            data-testid="overwrite-rewritten-replace"
          >
            {t('dialogs.overwriteRewritten.replace')}
          </Button>
        </>
      }
    >
      <p className="text-sm text-text-secondary" data-testid="overwrite-rewritten-message">
        {request.existenceUnknown
          ? t('dialogs.overwriteRewritten.messageUnknown', {
              fileName: request.fileName,
              extension: request.extension,
            })
          : t('dialogs.overwriteRewritten.message', {
              fileName: request.fileName,
              extension: request.extension,
            })}
      </p>
    </AlertDialog>
  );
}
