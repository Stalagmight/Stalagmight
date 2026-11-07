import { useTranslation } from 'react-i18next';
import { useProjectStore } from '../../store/projectStore';
import { formatPhaseLabel } from '../../utils/loadingPhaseLabel';
import { clamp } from '../../utils/clamp';
import { formatNumber } from '../../i18n/format';

/**
 * In-dialog import progress: the phase the backend reports (Reading -> Converting ->
 * Applying -> Finalizing) and the overall bar, from the store's `loading` state.
 *
 * The survey-file import dialogs render it themselves because the only overlay that
 * draws these phases is GlobalView's, and an import into an open project runs in
 * Single System View, where it is not mounted (#411 C3 for TML; #576 NF-W3P-1 for
 * Compass, Walls and GPX). Render it while the dialog's import runs; it shows nothing
 * unless a load is actually in progress.
 */
export function ImportProgress({ testIdPrefix }: { testIdPrefix: string }) {
  const { t } = useTranslation();
  const loading = useProjectStore((s) => s.loading);
  if (!loading.isLoading) return null;
  // `clamp` passes NaN through, and an unseeded/garbled progress event would then
  // render as "NaN%" and an invalid bar width.
  const overallProgress = Number.isFinite(loading.overallProgress)
    ? Math.round(clamp(loading.overallProgress, 0, 100))
    : 0;
  return (
    <div className="space-y-1" data-testid={`${testIdPrefix}-progress`}>
      <div className="flex justify-between text-xs text-text-secondary">
        <span data-testid={`${testIdPrefix}-progress-phase`}>
          {formatPhaseLabel(t, loading.phase, loading.phaseIndex, loading.phaseCount)}
        </span>
        <span>{formatNumber(overallProgress)}%</span>
      </div>
      <div className="h-1.5 bg-border rounded-full overflow-hidden">
        <div
          className="h-full bg-accent transition-all duration-300"
          style={{ width: `${overallProgress}%` }}
          role="progressbar"
          aria-label={t('loading.overall.aria')}
          aria-valuenow={overallProgress}
          aria-valuemin={0}
          aria-valuemax={100}
        />
      </div>
    </div>
  );
}
