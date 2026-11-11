import { useTranslation } from 'react-i18next';
import { COMMIT_MESSAGE_MAX_CHARS, commitMessageLength } from '../../utils/commitMessage';

/**
 * The character count under an upload description, and the reason the upload is
 * refused once it is longer than SpeleoDB can store (sdk#2, see
 * `utils/commitMessage`). The count is the stored length, so a line break shows
 * up as two characters, exactly as the server will count it.
 */
export function CommitMessageLimit({ message, id, testIdPrefix }: { message: string; id: string; testIdPrefix: string }) {
  const { t } = useTranslation();
  const length = commitMessageLength(message);
  const over = length > COMMIT_MESSAGE_MAX_CHARS;
  return (
    <div id={id} className="mt-1 space-y-1">
      <p className={`text-xs text-right ${over ? 'text-error' : 'text-text-secondary'}`}
        data-testid={`${testIdPrefix}-count`} data-over={over ? '' : undefined}>
        {t('speleodb.commitMessage.count', { length, max: COMMIT_MESSAGE_MAX_CHARS })}
      </p>
      {over && (
        <p role="alert" className="text-sm text-error" data-testid={`${testIdPrefix}-too-long`}>
          {t('speleodb.commitMessage.tooLong', { length, max: COMMIT_MESSAGE_MAX_CHARS })}
        </p>
      )}
    </div>
  );
}
