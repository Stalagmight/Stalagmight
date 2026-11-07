import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { trackDialogComplete } from '../../utils/telemetry';
import { Button } from '../common/Button';
import { DialogWrapper } from './DialogWrapper';
import { useShallow } from 'zustand/react/shallow';

export default function ErrorDialog() {
  const { t } = useTranslation();
  const { closeDialog, dialogData } = useUIStore(
    useShallow((s) => ({ closeDialog: s.closeDialog, dialogData: s.dialogData })),
  );

  const handleClose = useCallback(() => {
    trackDialogComplete('error');
    closeDialog();
  }, [closeDialog]);

  // Normalize dialogData values to strings to guard against non-string callers
  const rawMessage = dialogData.message;
  const message = typeof rawMessage === 'string' ? rawMessage : rawMessage ? String(rawMessage) : undefined;
  const rawTitle = dialogData.title;
  const title = (typeof rawTitle === 'string' ? rawTitle : rawTitle ? String(rawTitle) : undefined) ?? t('dialogs.error.title');
  const rawStack = dialogData.stackTrace;
  const stackTrace = typeof rawStack === 'string' ? rawStack : rawStack ? String(rawStack) : undefined;
  // Callers surfacing an actionable, non-bug condition (e.g. an import's
  // deferred-save failure - see finalizeDeferredSaveWarnings) set this to drop the
  // "report this as a bug" hint, which would otherwise contradict the message.
  const hideReportHint = Boolean(dialogData.hideReportHint);

  const [copyFeedback, setCopyFeedback] = useState<'idle' | 'copied' | 'failed'>('idle');
  const feedbackTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Cleanup timeout on unmount
  useEffect(() => {
    return () => {
      if (feedbackTimeoutRef.current) {
        clearTimeout(feedbackTimeoutRef.current);
      }
    };
  }, []);

  /** Copy error message and stack trace to clipboard */
  const handleCopyError = useCallback(async () => {
    const errorText = stackTrace
      ? `${message ?? t('dialogs.error.fallbackMessage')}\n\n${stackTrace}`
      : (message ?? t('dialogs.error.fallbackMessage'));

    // Clear any pending timeout
    if (feedbackTimeoutRef.current) {
      clearTimeout(feedbackTimeoutRef.current);
    }

    try {
      if (!navigator.clipboard?.writeText) {
        throw new Error('Clipboard API not available');
      }
      await navigator.clipboard.writeText(errorText);
      setCopyFeedback('copied');
    } catch {
      setCopyFeedback('failed');
    }
    feedbackTimeoutRef.current = setTimeout(() => setCopyFeedback('idle'), 2000);
  }, [message, stackTrace, t]);

  return (
    <DialogWrapper title={title} onClose={handleClose} size="md" data-testid="error-dialog">
      <div className="space-y-4">
        <p className="text-text-secondary whitespace-pre-wrap">{message ?? t('dialogs.error.fallbackMessage')}</p>

        {stackTrace && (
          <pre
            className="p-2 bg-surface-alt rounded text-xs text-text-tertiary overflow-auto max-h-48"
            data-testid="error-dialog-stack-trace"
          >
            {stackTrace}
          </pre>
        )}

        {!hideReportHint && (
          <p className="text-xs text-text-tertiary">
            {stackTrace
              ? t('dialogs.error.reportHintWithStack')
              : t('dialogs.error.reportHint')}
          </p>
        )}

        <div className="flex justify-between items-center pt-4 border-t border-border">
          <button
            type="button"
            onClick={handleCopyError}
            className="px-3 py-1.5 text-sm text-text-secondary hover:text-text-primary transition-colors"
            data-testid="error-dialog-copy-btn"
          >
            {copyFeedback === 'copied' ? t('dialogs.error.copied') : copyFeedback === 'failed' ? t('dialogs.error.copyFailed') : t('dialogs.error.copy')}
          </button>
          <Button onClick={handleClose} data-testid="error-dialog-close">{t('dialogs.error.ok')}</Button>
        </div>
      </div>
    </DialogWrapper>
  );
}
