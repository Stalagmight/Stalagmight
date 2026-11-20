import type { TFunction } from 'i18next';
import type { LoadingPhase } from '../types/ipc/LoadingPhase';

/**
 * Map a backend `LoadingPhase` (snake_case, src-tauri/src/core/loading.rs) to a
 * human-readable label.
 *
 * Shared by the two surfaces that render loading progress: the GlobalView
 * overlay (project open/create) and the TML import dialog (#411 C3) - by import
 * time the view is 'single', so the overlay is not mounted and the dialog has to
 * render the import_* phases itself.
 */
export function formatPhaseLabel(
  t: TFunction,
  phase: string | undefined,
  phaseIndex?: number,
  phaseCount?: number
): string {
  if (!phase) return t('globalView.loading.phases.initializing');

  // Keyed by the wire's phase names, so a renamed phase is a type error here
  // rather than a raw phase name on screen.
  const labels: Partial<Record<LoadingPhase | 'loading_taking_long', string>> = {
    // Project opening phases
    opening_file: t('globalView.loading.phases.openingFile'),
    loading_project_data: t('globalView.loading.phases.loadingProjectData'),
    loading_lines: t('globalView.loading.phases.loadingSections'),
    computing_stations: t('globalView.loading.phases.computingStations'),
    computing_statistics: t('globalView.loading.phases.computingStatistics'),
    validating_data: t('globalView.loading.phases.validatingData'),
    loading_settings: t('globalView.loading.phases.loadingSettings'),
    finalizing: t('globalView.loading.phases.finalizing'),
    // Timeout message
    loading_taking_long: t('globalView.loading.phases.takingLong'),
    // Create project phases
    creating_file: t('globalView.loading.phases.creatingFile'),
    initializing_project: t('globalView.loading.phases.initializingProject'),
    saving_settings: t('globalView.loading.phases.savingSettings'),
    // Survey-file import phases
    import_parsing: t('globalView.loading.phases.importParsing'),
    import_converting: t('globalView.loading.phases.importConverting'),
    import_applying_changes: t('globalView.loading.phases.importApplyingChanges'),
    import_finalizing: t('globalView.loading.phases.importFinalizing'),
  };

  const label = labels[phase as keyof typeof labels] ?? phase;

  if (phaseIndex !== undefined && phaseCount !== undefined && phaseCount > 0) {
    return t('globalView.loading.step', { current: phaseIndex + 1, total: phaseCount, label });
  }

  return label;
}
