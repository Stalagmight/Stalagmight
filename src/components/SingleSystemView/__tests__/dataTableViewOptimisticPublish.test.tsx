/**
 * #357 item 4, the collateral half: the optimistic in-memory publish that makes a
 * remount seed from the just-saved values must NOT invalidate the cached
 * closure-adjusted geometry.
 *
 * The publish originally went through `useProjectStore.setConfig`, which is the
 * whole-config REPLACEMENT path (Settings > Advanced "Reload from disk") and so
 * also does `set({ stationsAdjusted: null, surveyDataAdjusted: null })` - correct
 * there, because a replaced config can carry different closure sigmas. Reached
 * from the debounced view save it fires on every column resize, column hide and
 * angle-mode change, 500ms after the last edit and again on the unmount flush,
 * and nothing under `[panels]` feeds the least-squares solve. Two things broke
 * downstream, both in MapCanvas.tsx: the effect keyed on
 * `[stationsAdjusted, surveyDataAdjusted, geometryMode, clearMeasure]` wipes an
 * active ruler measurement in `closureAdjusted` mode, and `loadAdjustedGeometry`
 * has `surveyDataAdjusted` in its deps, so the nulling forces a refetch and
 * re-solve while the selectors fall back to measured geometry - a visible jump.
 *
 * Rendered at the full-component tier (real DataTable, real projectStore, mocked
 * `invokeIpc`) because what is under test is the store write the debounced save
 * performs, and DataTable owns the view state that triggers it. `sections={[]}`
 * keeps SpreadsheetGrid out of the render; the column-visibility checkbox stands
 * in for the resize/angle-mode edits that share the same debounced effect.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';

vi.mock('../../../utils/ipc');

import DataTable from '../DataTable';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { defaultConfig } from '../../../types/config';
import type { Config } from '../../../types/config';
import type { Station } from '../../../types/project';
import type { SurveyData } from '../../../types/survey-data';
import { invokeIpcMock } from '../../../test/factories/ipc';
import { NoopResizeObserver } from '../../../test/factories/dom';
import { microtasks } from '../../../test/factories/async';

function freshConfig(): Config {
  return structuredClone(defaultConfig) as Config;
}

/** Only identity matters here - these stand for a solved adjusted set that is
 *  expensive to recompute, and the assertion is that it is the SAME object. */
const adjustedStations = [{ tag: 'adjusted' } as unknown as Station];
const adjustedSurvey = { tag: 'adjusted' } as unknown as SurveyData;

/** Resolver for the in-flight `update_data_table_view`, so a test can inspect the
 *  store while the IPC is still open - before the `.then` re-syncs `config` from
 *  disk and hides whether the optimistic publish happened at all. */
let releaseSave: (() => void) | null = null;

beforeEach(() => {
  // jsdom has no ResizeObserver; DataTable's chrome-offset measurement effect
  // needs the constructor to exist, but this suite never depends on it firing.
  vi.stubGlobal('ResizeObserver', NoopResizeObserver);
  vi.useFakeTimers();
  releaseSave = null;
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation((command: string) => {
    if (command === 'get_config') return Promise.resolve(freshConfig());
    if (command === 'update_data_table_view') {
      return new Promise<void>((resolve) => {
        releaseSave = resolve;
      });
    }
    return Promise.resolve(undefined);
  });
  useProjectStore.setState({
    config: freshConfig(),
    stationsAdjusted: adjustedStations,
    surveyDataAdjusted: adjustedSurvey,
  });
  // The mode that makes the invalidation user-visible: MapCanvas clears the ruler
  // on an adjusted-geometry change only while this mode is active.
  useUIStore.setState({ geometryMode: 'closureAdjusted', toasts: [] });
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  useProjectStore.setState({ stationsAdjusted: null, surveyDataAdjusted: null });
  useUIStore.setState({ geometryMode: 'measured' });
});

function toggleColumn(key: string) {
  fireEvent.click(screen.getByTestId('data-table-columns-menu'));
  fireEvent.click(screen.getByTestId(`col-vis-${key}`));
}

describe('data-table view save vs cached adjusted geometry (#357 item 4)', () => {
  it('publishes the panels optimistically without discarding the adjusted geometry', async () => {
    render(<DataTable sections={[]} />);

    toggleColumn('comment');
    await act(async () => {
      await vi.advanceTimersByTimeAsync(500);
    });

    // The write went out and is still in flight - the window in which the
    // in-memory config is the only thing a remount could seed from.
    expect(
      invokeIpcMock.mock.calls.filter(([command]) => command === 'update_data_table_view')
    ).toHaveLength(1);
    // The panels did reach the in-memory config: the publish still does its job.
    expect(useProjectStore.getState().config?.panels.data_table_shot_hidden_columns).toEqual([
      'comment',
      'section_id',
      'shot_id',
    ]);
    // But the solved geometry is untouched. Identity, not just non-null: a
    // re-solve would hand back a different object and still re-run the MapCanvas
    // effects that clear the ruler and refetch.
    expect(useProjectStore.getState().stationsAdjusted).toBe(adjustedStations);
    expect(useProjectStore.getState().surveyDataAdjusted).toBe(adjustedSurvey);

    // ...and the success path's `loadConfig` re-sync must not drop it either.
    releaseSave?.();
    await act(async () => {
      await microtasks(2);
    });
    expect(useProjectStore.getState().stationsAdjusted).toBe(adjustedStations);
    expect(useProjectStore.getState().surveyDataAdjusted).toBe(adjustedSurvey);
  });

  it('leaves the adjusted geometry alone on the unmount flush too', async () => {
    const { unmount } = render(<DataTable sections={[]} />);

    // Close the table inside the 500ms window so the unmount flush - not the
    // timer - is what fires the save and the publish alongside it.
    toggleColumn('comment');
    unmount();
    await microtasks(2);

    expect(
      invokeIpcMock.mock.calls.filter(([command]) => command === 'update_data_table_view')
    ).toHaveLength(1);
    expect(useProjectStore.getState().stationsAdjusted).toBe(adjustedStations);
    expect(useProjectStore.getState().surveyDataAdjusted).toBe(adjustedSurvey);
  });
});
