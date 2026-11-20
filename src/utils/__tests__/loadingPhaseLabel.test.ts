import { describe, expect, it } from 'vitest';
import type { TFunction } from 'i18next';
import type { LoadingPhase } from '../../types/ipc/LoadingPhase';
import { formatPhaseLabel } from '../loadingPhaseLabel';

/** A `t` that answers the key it was asked for, so a label names its key. */
const t = ((key: string) => key) as unknown as TFunction;

describe('formatPhaseLabel', () => {
  it('words every open and create phase the backend sends, by its wire name', () => {
    const phases: Array<[LoadingPhase, string]> = [
      ['opening_file', 'globalView.loading.phases.openingFile'],
      ['loading_project_data', 'globalView.loading.phases.loadingProjectData'],
      ['loading_lines', 'globalView.loading.phases.loadingSections'],
      ['computing_stations', 'globalView.loading.phases.computingStations'],
      ['computing_statistics', 'globalView.loading.phases.computingStatistics'],
      ['validating_data', 'globalView.loading.phases.validatingData'],
      ['loading_settings', 'globalView.loading.phases.loadingSettings'],
      ['finalizing', 'globalView.loading.phases.finalizing'],
      ['creating_file', 'globalView.loading.phases.creatingFile'],
      ['initializing_project', 'globalView.loading.phases.initializingProject'],
      ['saving_settings', 'globalView.loading.phases.savingSettings'],
    ];
    for (const [phase, key] of phases) {
      expect(formatPhaseLabel(t, phase), phase).toBe(key);
    }
  });

  it('shows an unknown phase as it came, and no phase as initializing', () => {
    expect(formatPhaseLabel(t, 'something_new')).toBe('something_new');
    expect(formatPhaseLabel(t, undefined)).toBe('globalView.loading.phases.initializing');
  });
});
