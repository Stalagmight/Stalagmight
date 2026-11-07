import React from 'react';
import { useTranslation } from 'react-i18next';
import { formatNumber } from '../../i18n/format';
import { clamp } from '../../utils/clamp';

export interface LoadingProgressProps {
  title: string;                    // "Loading ProjectName..."
  phase: string;                    // Current phase label
  phaseProgress?: number;           // 0-100 or undefined for indeterminate
  phaseCurrent?: number;            // e.g., 23
  phaseTotal?: number;              // e.g., 50
  overallProgress: number;          // 0-100
  onCancel?: () => void;            // Cancel handler (also used for dismiss on error)
  onRetry?: () => void;             // Retry handler (shown on error)
  error?: {
    message: string;
    details?: string | undefined;
  };
  /**
   * Set by the project loading window, a blocking modal layer: its error state is
   * then a modal alert dialog (`role="alertdialog"`, `aria-modal`, labelled by its
   * heading, described by the message) and its progress state is `aria-modal`.
   * Left unset where the progress shows inline inside another dialog (the export
   * dialog), which must not claim to be modal.
   */
  modal?: boolean;
}

/** Clamp a number to [0, 100], treating NaN/Infinity as 0 */
function clampProgress(value: number): number {
  if (!Number.isFinite(value)) return 0;
  return clamp(value, 0, 100);
}

export function LoadingProgress({
  title,
  phase,
  phaseProgress,
  phaseCurrent,
  phaseTotal,
  overallProgress,
  onCancel,
  onRetry,
  error,
  modal = false,
}: LoadingProgressProps) {
  const { t } = useTranslation();
  const [detailsExpanded, setDetailsExpanded] = React.useState(false);
  const [copyFeedback, setCopyFeedback] = React.useState<'idle' | 'copied' | 'failed'>('idle');
  const isIndeterminate = phaseProgress === undefined;
  const titleId = React.useId();
  const phaseId = React.useId();
  const detailsId = React.useId();
  const messageId = React.useId();

  // Ref for cleanup of feedback timeout
  const feedbackTimeoutRef = React.useRef<ReturnType<typeof setTimeout> | null>(null);

  // Cleanup timeout on unmount
  React.useEffect(() => {
    return () => {
      if (feedbackTimeoutRef.current) {
        clearTimeout(feedbackTimeoutRef.current);
      }
    };
  }, []);

  /** Copy error message and details to clipboard with error handling */
  const handleCopyError = React.useCallback(async () => {
    if (!error) return;

    const errorText = error.details
      ? `${error.message}\n\n${error.details}`
      : error.message;

    // Clear any pending timeout
    if (feedbackTimeoutRef.current) {
      clearTimeout(feedbackTimeoutRef.current);
    }

    try {
      // Explicitly check clipboard API availability
      if (!navigator.clipboard?.writeText) {
        throw new Error('Clipboard API not available');
      }
      await navigator.clipboard.writeText(errorText);
      setCopyFeedback('copied');
    } catch {
      setCopyFeedback('failed');
    }
    feedbackTimeoutRef.current = setTimeout(() => setCopyFeedback('idle'), 2000);
  }, [error]);

  // Clamp progress values
  const clampedOverall = clampProgress(overallProgress);
  const clampedPhase = phaseProgress !== undefined ? clampProgress(phaseProgress) : undefined;

  // Error state renders a different UI
  if (error) {
    return (
      <div
        className="w-80 bg-panel rounded-lg shadow-lg p-6"
        // [NF4] In the loading window the error holds the app until Retry or
        // Dismiss: a modal alert dialog, named by its heading, that the window
        // moves focus into - so no live region on top, which would announce it a
        // second time. Anywhere else it stays an inline alert.
        role={modal ? 'alertdialog' : 'alert'}
        aria-modal={modal || undefined}
        aria-labelledby={modal ? titleId : undefined}
        aria-describedby={modal ? messageId : undefined}
        aria-live={modal ? undefined : 'assertive'}
        data-testid="loading-progress"
      >
        {/* Error Title */}
        <h2 id={titleId} className="text-lg font-medium text-error mb-2">
          {t('loading.error.title')}
        </h2>

        {/* Error Message */}
        <p id={messageId} className="text-text-secondary mb-3">
          {error.message}
        </p>

        {/* Show/Hide Details Toggle */}
        {error.details && (
          <div className="mb-4">
            <button
              type="button"
              onClick={() => setDetailsExpanded(v => !v)}
              className="text-sm text-text-tertiary hover:text-text-secondary"
              aria-expanded={detailsExpanded}
              aria-controls={detailsId}
              data-testid="loading-error-details-toggle"
            >
              {detailsExpanded ? t('loading.error.hideDetails') : t('loading.error.showDetails')}
            </button>
            {detailsExpanded && (
              <pre
                id={detailsId}
                className="mt-2 p-2 bg-surface-alt rounded text-xs text-text-tertiary overflow-auto max-h-32"
              >
                {error.details}
              </pre>
            )}
          </div>
        )}

        {/* Action Buttons */}
        <div className="flex gap-2">
          {onRetry && (
            <button
              type="button"
              onClick={onRetry}
              data-testid="loading-error-retry"
              className="px-3 py-1.5 bg-accent text-accent-on rounded hover:bg-accent-90 transition-colors"
            >
              {t('common.actions.retry')}
            </button>
          )}
          <button
            type="button"
            onClick={onCancel}
            disabled={!onCancel}
            data-testid="loading-error-dismiss"
            className="px-3 py-1.5 bg-surface-alt text-text-primary rounded hover:bg-border transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {t('common.actions.dismiss')}
          </button>
          <button
            type="button"
            onClick={handleCopyError}
            className="px-3 py-1.5 text-text-secondary hover:text-text-primary transition-colors"
            title={t('loading.error.copyTitle')}
            data-testid="loading-error-copy"
          >
            {copyFeedback === 'copied'
              ? t('loading.error.copied')
              : copyFeedback === 'failed'
                ? t('loading.error.copyFailed')
                : t('loading.error.copy')}
          </button>
        </div>
      </div>
    );
  }

  // Normal progress UI
  return (
    <div
      className="w-80 bg-panel rounded-lg shadow-lg p-6"
      role="dialog"
      aria-modal={modal || undefined}
      aria-labelledby={titleId}
      aria-describedby={phaseId}
      data-testid="loading-progress"
    >
      {/* Title */}
      <h2 id={titleId} className="text-lg font-medium text-text-primary mb-4">
        {title}
      </h2>

      {/* Overall Progress */}
      <div className="mb-4">
        <div className="flex justify-between text-sm text-text-secondary mb-1">
          <span>{t('loading.overall.label')}</span>
          <span>{formatNumber(Math.round(clampedOverall))}%</span>
        </div>
        <div className="h-1.5 bg-border rounded-full overflow-hidden">
          <div
            className="h-full bg-accent transition-all duration-300"
            style={{ width: `${clampedOverall}%` }}
            role="progressbar"
            aria-label={t('loading.overall.aria')}
            aria-valuenow={Math.round(clampedOverall)}
            aria-valuemin={0}
            aria-valuemax={100}
          />
        </div>
      </div>

      {/* Current Phase */}
      <div
        id={phaseId}
        className="mb-4"
        role="status"
        aria-live="polite"
      >
        <div className="flex justify-between text-sm text-text-secondary mb-1">
          <span>{phase}</span>
          {phaseCurrent !== undefined && phaseTotal !== undefined && (
            <span>{formatNumber(phaseCurrent)}/{formatNumber(phaseTotal)}</span>
          )}
        </div>
        <div className="h-1 bg-border rounded-full overflow-hidden">
          {isIndeterminate ? (
            <div
              className="h-full bg-accent-70 animate-pulse-shimmer"
              role="progressbar"
              aria-label={t('loading.phase.aria')}
              aria-valuetext={t('common.states.loading')}
            />
          ) : (
            <div
              className="h-full bg-accent-70 transition-all duration-150"
              style={{ width: `${clampedPhase}%` }}
              role="progressbar"
              aria-label={t('loading.phase.aria')}
              aria-valuenow={Math.round(clampedPhase!)}
              aria-valuemin={0}
              aria-valuemax={100}
            />
          )}
        </div>
      </div>

      {/* Cancel Button */}
      {onCancel && (
        <button
          type="button"
          onClick={onCancel}
          data-testid="loading-progress-cancel"
          className="text-sm text-text-secondary hover:text-text-primary transition-colors"
        >
          {t('common.actions.cancel')}
        </button>
      )}
    </div>
  );
}
