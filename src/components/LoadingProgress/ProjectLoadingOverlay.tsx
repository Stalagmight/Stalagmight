import { useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';
import { useProjectStore } from '../../store/projectStore';
import { useScreenOccupant } from '../../hooks/useScreenOccupant';
import { formatPhaseLabel } from '../../utils/loadingPhaseLabel';
import { setProjectActionSource, type SourceName } from '../../utils/telemetry';
import { LoadingProgress } from './LoadingProgress';

export interface ProjectLoadingOverlayProps {
  /** The entry point a Retry is attributed to (telemetry). */
  retrySource: SourceName;
  /**
   * Register the overlay as a screen occupant while it is up, so the window-level
   * shortcuts stand down as they do for a dialog (the project view: #576 LOAD-VIEW-D1;
   * the project list: #576 Q-W60-1-D1).
   */
  occupiesScreen?: boolean;
}

/**
 * The loading modal of a project load or create, and its error state (Retry,
 * Dismiss): shown while the store's loading state says one is in progress.
 *
 * [#576 LOAD-VIEW-D1] It used to live in the project list only. A load started
 * from the project view (Ctrl+O or a drop, then Discard in the unsaved-changes
 * prompt) ran with no overlay: the Display menu and the shortcuts still reached
 * the outgoing project, and a load that failed left its error in the loading
 * state with nothing on screen to show or dismiss it (and every banner gated on
 * "no load in progress" hidden until the next load). The project view now shows
 * it too, over everything, and it blocks the menus and the shortcuts.
 *
 * [#576 Q-W60-1-D1] The project list's overlay holds the screen the same way, so
 * one rule covers both views: an OS-launched file waits for the load as well.
 */
export function ProjectLoadingOverlay({ retrySource, occupiesScreen = false }: ProjectLoadingOverlayProps) {
  const { t } = useTranslation();
  const { loading, loadProject, resetLoading, cancelLoading } = useProjectStore(
    useShallow((s) => ({
      loading: s.loading,
      loadProject: s.loadProject,
      resetLoading: s.resetLoading,
      cancelLoading: s.cancelLoading,
    })),
  );
  useScreenOccupant(occupiesScreen && loading.isLoading);

  // [#576 Q-W44-2-D1] The user's Cancel: the load's answer then shows no error.
  const handleCancel = useCallback(async () => {
    await cancelLoading({ byUser: true });
  }, [cancelLoading]);

  // Retry re-drives `loadProject` directly on the SAME path a previous call
  // already routed and validated; re-routing would gate nothing new.
  const handleRetry = useCallback(async () => {
    if (loading.projectPath) {
      setProjectActionSource(retrySource);
      await loadProject(loading.projectPath);
    }
  }, [loading.projectPath, loadProject, retrySource]);

  const handleDismiss = useCallback(() => {
    resetLoading();
  }, [resetLoading]);

  if (!loading.isLoading) return null;
  return (
    <div className="fixed inset-0 bg-overlay flex items-center justify-center z-50" data-testid="project-loading-overlay">
      <LoadingProgress
        title={
          loading.error
            ? t('globalView.loading.failedTitle')
            : t('globalView.loading.loadingTitle', {
              name: loading.projectName ?? t('globalView.loading.projectFallback'),
            })
        }
        phase={formatPhaseLabel(t, loading.phase, loading.phaseIndex, loading.phaseCount)}
        overallProgress={loading.overallProgress}
        onCancel={loading.error ? handleDismiss : handleCancel}
        {...(loading.phaseProgress !== undefined && { phaseProgress: loading.phaseProgress })}
        {...(loading.phaseCurrent !== undefined && { phaseCurrent: loading.phaseCurrent })}
        {...(loading.phaseTotal !== undefined && { phaseTotal: loading.phaseTotal })}
        {...(loading.error ? { error: loading.error } : {})}
        {...(loading.error && loading.projectPath ? { onRetry: handleRetry } : {})}
      />
    </div>
  );
}
