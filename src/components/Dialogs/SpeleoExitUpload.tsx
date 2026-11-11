import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { isUnwoundNotEdited, useSpeleoStore } from '../../store/speleoStore';
import { useSpeleoActions } from '../../hooks/useSpeleoActions';
import { SpeleoProgressOverlay } from './SpeleoProgressOverlay';
import { SpeleoPushFailureNotice } from './SpeleoPushFailureNotice';
import { CommitMessageLimit } from './CommitMessageLimit';
import { toastLateUploadOutcome } from '../../utils/lateUploadOutcome';

/** Same bound the Save / Finish editing upload uses before it lets the user walk away. */
export const UPLOAD_PROGRESS_LIMIT_MS = 120_000;

/**
 * How an exit upload ended. `released`: uploaded, lock handed back, nothing newer
 * left behind. `diverged`: SpeleoDB moved on, and its own resolution dialog takes
 * over. `failed`: not uploaded; the failure notice is on screen. `newer`: the
 * upload landed but edits made meanwhile are still only local.
 */
export type ExitUploadResult = 'released' | 'diverged' | 'failed' | 'newer';

/**
 * The upload the exit prompt and the sign-out run: the ordinary Finish editing
 * (`done` - save, upload, release), plus the bounded-progress bookkeeping the
 * Save dialog uses. Shared so the two prompts cannot drift apart.
 */
export function useSpeleoExitUpload() {
  const { done } = useSpeleoActions();
  const unwound = useSpeleoStore((s) => isUnwoundNotEdited(s.link));
  const [uploading, setUploading] = useState(false);
  const [slow, setSlow] = useState(false);

  useEffect(() => {
    if (!uploading) return;
    const timer = window.setTimeout(() => setSlow(true), UPLOAD_PROGRESS_LIMIT_MS);
    return () => window.clearTimeout(timer);
  }, [uploading]);

  /**
   * `isCurrent` is the PROJECT check (still the project the upload was started
   * for); it gates applying the result. `isShown` is the prompt's own identity:
   * once the user walks away from a slow upload it is false, and the result that
   * lands later is still applied and toasted (frontend#2), not dropped.
   */
  const upload = async (message: string, isCurrent: () => boolean, isShown?: () => boolean): Promise<ExitUploadResult> => {
    setSlow(false);
    setUploading(true);
    try {
      const resolved = await done(message, {
        isCurrent,
        // The older-version warning is on screen beside the description field, so
        // pressing Upload is the informed choice `done` would otherwise stop to ask
        // for in a dialog of its own - which would land behind this prompt.
        ...(unwound ? { acceptOlderVersion: true } : {}),
      });
      if (isShown && !isShown() && isCurrent()) toastLateUploadOutcome(resolved);
      if (resolved?.kind === 'divergence_needed') return 'diverged';
      if (resolved?.kind !== 'released') return 'failed';
      if (useProjectStore.getState().isDirty || useSpeleoStore.getState().link?.hasUnpushedEdits) return 'newer';
      return 'released';
    } finally {
      setUploading(false);
    }
  };

  return { upload, uploading, slow, unwound };
}

/** The description field and the notices that sit around it. */
export function SpeleoExitUploadFields({
  message, onMessage, disabled, uploading, slow, unwound, testIdPrefix,
}: {
  message: string;
  onMessage: (value: string) => void;
  disabled: boolean;
  uploading: boolean;
  slow: boolean;
  unwound: boolean;
  testIdPrefix: string;
}) {
  const { t } = useTranslation();
  const id = `${testIdPrefix}-message`;
  return (
    <>
      <p className="text-sm text-text-secondary">{t('speleoExitPrompt.uploadBody')}</p>
      {unwound && (
        <p role="note" className="text-sm text-warning" data-testid={`${testIdPrefix}-older-version`}>
          {t('speleoExitPrompt.olderVersion')}
        </p>
      )}
      <div>
        <label htmlFor={id} className="block text-sm text-text-secondary mb-1">
          {t('speleoExitPrompt.messageLabel')}
        </label>
        <textarea
          id={id}
          disabled={disabled}
          value={message}
          onChange={(e) => onMessage(e.target.value)}
          rows={3}
          className="w-full px-2 py-1 bg-background border border-border rounded text-sm"
          placeholder={t('speleoExitPrompt.messagePlaceholder')}
          data-testid={id}
          aria-describedby={`${id}-limit`}
        />
        <CommitMessageLimit message={message} id={`${id}-limit`} testIdPrefix={id} />
      </div>
      {uploading && slow && (
        <p role="alert" className="text-sm text-warning" data-testid={`${testIdPrefix}-upload-slow`}>
          {t('speleoExitPrompt.uploadSlow')}
        </p>
      )}
      <SpeleoPushFailureNotice />
    </>
  );
}

/** The non-dismissable progress surface, until the bound runs out. */
export function SpeleoExitUploadProgress({ uploading, slow, testId }: { uploading: boolean; slow: boolean; testId: string }) {
  const { t } = useTranslation();
  if (!uploading || slow) return null;
  return (
    <SpeleoProgressOverlay
      title={t('speleoExitPrompt.uploadProgressTitle')}
      description={t('speleoExitPrompt.uploadProgressBody')}
      testId={testId}
    />
  );
}
