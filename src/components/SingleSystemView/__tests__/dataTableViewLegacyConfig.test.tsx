/**
 * The legacy-backend defense on the data-table view seed (#357 item 4).
 *
 * `panelsConfig` comes straight from `get_config` with no `defaultConfig` merge -
 * the only guarantee is the Rust `#[serde(default)]`. A frontend running against a
 * backend that predates these fields sees them as `undefined`, so the seed guards
 * every one of them. The guard was applied to the change-detection SNAPSHOT but
 * not to the setter calls, which left the guard doing nothing: the component state
 * became `undefined`, and the very next run of the save effect called
 * `stableWidths(undefined)` -> `Object.entries(undefined)` -> the exact crash that
 * takes the whole data table down on open.
 *
 * Rendered at the full-component tier (real DataTable, real projectStore, mocked
 * `invokeIpc`) because the crash is in the seed -> save-effect handoff across a
 * real state update, which only exists when the component owns the state.
 * `sections={[]}` keeps SpreadsheetGrid out of the render.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen } from '@testing-library/react';

vi.mock('../../../utils/ipc');

import DataTable from '../DataTable';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { defaultConfig } from '../../../types/config';
import type { Config } from '../../../types/config';
import type { PanelsConfig } from '../../../types/ipc/PanelsConfig';
import { invokeIpcMock } from '../../../test/factories/ipc';
import { NoopResizeObserver } from '../../../test/factories/dom';

/** A config as an older backend would answer `get_config`: the data-table view
 *  fields simply absent, not empty. `Partial` because the generated
 *  `PanelsConfig` types them as always-present - which is precisely the
 *  assumption this suite exists to say the frontend must not make. */
function legacyConfig(): Config {
  const config = structuredClone(defaultConfig) as Config;
  const panels = config.panels as Partial<PanelsConfig>;
  delete panels.data_table_shot_column_order;
  delete panels.data_table_shot_hidden_columns;
  delete panels.data_table_shot_column_widths;
  delete panels.data_table_station_column_order;
  delete panels.data_table_station_hidden_columns;
  delete panels.data_table_station_column_widths;
  return config;
}

beforeEach(() => {
  // jsdom has no ResizeObserver; DataTable's chrome-offset measurement effect
  // needs the constructor to exist, but this suite never depends on it firing.
  vi.stubGlobal('ResizeObserver', NoopResizeObserver);
  vi.useFakeTimers();
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation((command: string) => {
    if (command === 'get_config') return Promise.resolve(legacyConfig());
    return Promise.resolve(undefined);
  });
  useProjectStore.setState({ config: legacyConfig() });
  useUIStore.setState({ toasts: [] });
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe('data-table view seed against a backend predating the view fields', () => {
  it('mounts and survives the debounce instead of crashing on the absent fields', async () => {
    expect(() => render(<DataTable sections={[]} />)).not.toThrow();
    expect(screen.getByTestId('data-table-columns-menu')).toBeTruthy();

    // The crash is not at mount: the seed effect and the save effect run in the
    // same commit, and the save effect still sees the pre-seed state. It lands on
    // the NEXT run, once the seeded `undefined` reaches `stableWidths`.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(500);
    });
    expect(screen.getByTestId('data-table-columns-menu')).toBeTruthy();
  });

  it('starts with no overrides rather than scheduling a save of state the user never touched', async () => {
    render(<DataTable sections={[]} />);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(500);
    });

    // The seeded fallbacks have to match what the seed recorded as "last saved",
    // or merely OPENING the data table writes back a view nobody edited - the
    // same class of spurious save the sorted hidden lists exist to prevent.
    expect(
      invokeIpcMock.mock.calls.filter(([command]) => command === 'update_data_table_view')
    ).toHaveLength(0);
  });
});
