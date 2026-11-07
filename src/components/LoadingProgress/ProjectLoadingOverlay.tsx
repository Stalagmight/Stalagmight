import { useCallback, useEffect, useLayoutEffect, useRef, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';
import { useProjectStore } from '../../store/projectStore';
import { useScreenOccupant } from '../../hooks/useScreenOccupant';
import { getFocusableElements, useFocusTrap } from '../../hooks/useFocusTrap';
import { isTopmost, popDialog, pushDialogBeneath } from '../../utils/dialogStack';
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
 *
 * [W71-NF1] It holds the keyboard too: see `LoadingWindow`.
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
    <LoadingWindow hasError={Boolean(loading.error)}>
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
    </LoadingWindow>
  );
}

/**
 * The loading window's modal layer, mounted only while a load (or its error) is up.
 *
 * [W71-NF1] It used to be a full-screen div inside `#root`: it covered the app for
 * the pointer, but Tab and Shift+Tab walked on to the controls under it (the
 * project list's cards, search field and sort / density controls; the project
 * view's toolbar and panels), where Enter and Space acted. It now uses the
 * dialogs' mechanism: portaled into `#dialog-root` and registered with the dialog
 * stack, so everything in `#root` is inert while it is up (an in-`#root` overlay
 * would be made inert with it), `useFocusTrap` wraps Tab inside it, focus moves in
 * when it opens, and goes back to the element that had it when it closes - when
 * that element is still there (a load that succeeds from the project list has
 * replaced the list with the project view by then).
 *
 * It goes in BENEATH any open dialog (`pushDialogBeneath`): a dialog was always
 * painted above the loading window, and a survey-file import raises the window
 * under its own dialog, which stays the interactive surface. While a dialog is
 * above it, the window is inert and leaves focus alone; when that dialog closes,
 * the dialog hands focus back to its trigger here.
 */
function LoadingWindow({ hasError, children }: { hasError: boolean; children: ReactNode }) {
  const windowRef = useRef<HTMLDivElement>(null);
  // The element that had focus when the window opened. Read at the first render,
  // as DialogWrapper does, so a Strict Mode effect replay cannot re-read it from
  // inside the window.
  const triggerRef = useRef<Element | null>(document.activeElement);

  useFocusTrap(windowRef);

  useLayoutEffect(() => {
    const el = windowRef.current;
    if (!el) return;
    pushDialogBeneath(el);
    return () => {
      popDialog(el);
    };
  }, []);

  // Focus in when the window opens, and again when its error state replaces the
  // progress (the focused Cancel goes with it) - unless a dialog is above it.
  useLayoutEffect(() => {
    const el = windowRef.current;
    if (!el || !isTopmost(el) || el.contains(document.activeElement)) return;
    getFocusableElements(el)[0]?.focus();
  }, [hasError]);

  // Give focus back on close, after popDialog has lifted `inert` from `#root` and
  // the window is gone. Only when focus went with the window (it is on the body
  // now): a dialog above it that kept focus throughout keeps it.
  useEffect(() => {
    const trigger = triggerRef.current;
    return () => {
      const active = document.activeElement;
      if (active !== null && active !== document.body) return;
      if (trigger instanceof HTMLElement && trigger.isConnected) trigger.focus();
    };
  }, []);

  return createPortal(
    <div
      ref={windowRef}
      className="fixed inset-0 bg-overlay flex items-center justify-center z-50"
      data-testid="project-loading-overlay"
    >
      {children}
    </div>,
    document.getElementById('dialog-root') ?? document.body,
  );
}
