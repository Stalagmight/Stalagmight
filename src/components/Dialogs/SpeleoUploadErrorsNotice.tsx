import { useTranslation } from 'react-i18next';
import { Button } from '../common/Button';
import { AlertCircleIcon } from '../common/icons';

interface SpeleoUploadErrorsNoticeProps {
  errorCount: number;
  /** The user ticked "Upload anyway": the upload button may go ahead. */
  accepted: boolean;
  onAcceptedChange: (accepted: boolean) => void;
  /** Leave the upload for the Warnings tab, where the errors can be fixed. */
  onResolve: () => void;
  disabled: boolean;
}

/**
 * The red notice in the SpeleoDB upload dialog when the survey has errors. Data
 * that cannot be right (a negative length, an azimuth outside 0-360) goes to every
 * collaborator once it is uploaded, so the upload waits until the user either
 * fixes it ("Resolve now", which opens the Warnings tab) or says it should go as
 * it is (the checkbox).
 *
 * "Resolve now" sits inside the notice rather than in the dialog's footer row, a
 * deliberate exception to the style guide: the button belongs to the problem it
 * resolves, and the footer stays about the upload.
 */
export function SpeleoUploadErrorsNotice({
  errorCount, accepted, onAcceptedChange, onResolve, disabled,
}: SpeleoUploadErrorsNoticeProps) {
  const { t } = useTranslation();
  return (
    <div
      className="rounded border border-error bg-error-10 text-sm px-3 py-2 space-y-2"
      role="alert"
      data-testid="speleodb-upload-errors"
    >
      <p className="flex items-start gap-2 text-error font-medium">
        <AlertCircleIcon size={16} className="shrink-0 mt-0.5" aria-hidden="true" />
        <span data-testid="speleodb-upload-errors-count">{t('speleodb.done.errors.heading', { count: errorCount })}</span>
      </p>
      <p className="text-text-primary">{t('speleodb.done.errors.body')}</p>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <label className="flex items-center gap-2 text-text-primary cursor-pointer">
          <input
            type="checkbox"
            checked={accepted}
            disabled={disabled}
            onChange={(e) => onAcceptedChange(e.target.checked)}
            data-testid="speleodb-upload-errors-accept"
          />
          {t('speleodb.done.errors.accept')}
        </label>
        <Button size="sm" onClick={onResolve} disabled={disabled} data-testid="speleodb-upload-errors-resolve">
          {t('speleodb.done.errors.resolve')}
        </Button>
      </div>
    </div>
  );
}
