/**
 * Station IDs / Station names: one choice between two labels.
 *
 * The map's station label is the station ID or its name, never both. Turning
 * one Display-menu toggle on turns the other off; turning one off leaves the
 * other alone, so "no station labels" is reachable. Every toggle of either
 * key saves the pair as it now stands (partner first), so a stored pair that
 * reads both-on (a failed write, a file edited elsewhere) is repaired by the
 * next toggle; until then the load lets the IDs win, as the map and the image
 * export do.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockResolvedValue({}),
}));

const trackActionMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/telemetry', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/telemetry')>()),
  trackAction: trackActionMock,
}));

import { useUIStore } from '../uiStore';
import { DISPLAY_DEFAULTS, getEffectiveStationLabelMode } from '../displaySelectors';
import { emptySurveyData, hasStationNames } from '../../types/survey-data';
import type { DisplaySettings } from '../../types/loading';

const settingsWith = (overrides: Partial<DisplaySettings>): DisplaySettings => ({
  ...DISPLAY_DEFAULTS,
  geometryMode: 'measured',
  ...overrides,
});

/** The save_display_setting calls for the two label keys, in call order. */
function labelSaves(): Array<[string, string]> {
  return vi
    .mocked(invoke)
    .mock.calls.filter(([cmd]) => cmd === 'save_display_setting')
    .map(([, args]) => args as { key: string; value: string })
    .filter(({ key }) => key === 'showStationLabels' || key === 'showStationNames')
    .map(({ key, value }) => [key, value]);
}

const pair = () => {
  const s = useUIStore.getState();
  return { ids: s.showStationLabels, names: s.showStationNames };
};

beforeEach(() => {
  useUIStore.setState({ appMode: 'survey', showStationLabels: false, showStationNames: false });
  vi.clearAllMocks();
});

describe('toggleDisplay: Station IDs and Station names are exclusive', () => {
  it('turning names on turns IDs off, and saves the IDs false first', () => {
    useUIStore.setState({ showStationLabels: true });
    useUIStore.getState().toggleDisplay('showStationNames');
    expect(pair()).toEqual({ ids: false, names: true });
    expect(labelSaves()).toEqual([
      ['showStationLabels', 'false'],
      ['showStationNames', 'true'],
    ]);
  });

  it('turning IDs on turns names off, and saves the names false first', () => {
    useUIStore.setState({ showStationNames: true });
    useUIStore.getState().toggleDisplay('showStationLabels');
    expect(pair()).toEqual({ ids: true, names: false });
    expect(labelSaves()).toEqual([
      ['showStationNames', 'false'],
      ['showStationLabels', 'true'],
    ]);
  });

  it('turning one off leaves the other alone (both off is allowed) and saves the pair', () => {
    useUIStore.setState({ showStationNames: true });
    useUIStore.getState().toggleDisplay('showStationNames');
    expect(pair()).toEqual({ ids: false, names: false });
    expect(labelSaves()).toEqual([
      ['showStationLabels', 'false'],
      ['showStationNames', 'false'],
    ]);
  });

  it('turning one on while the other is already off saves the pair as it stands', () => {
    useUIStore.getState().toggleDisplay('showStationNames');
    expect(pair()).toEqual({ ids: false, names: true });
    expect(labelSaves()).toEqual([
      ['showStationLabels', 'false'],
      ['showStationNames', 'true'],
    ]);
  });

  it('a stored both-on pair is repaired by the next toggle, so it cannot reopen as names', () => {
    // Loads as IDs only (the IDs win)...
    useUIStore
      .getState()
      .applyDisplaySettings(settingsWith({ showStationLabels: true, showStationNames: true }));
    vi.clearAllMocks();
    // ...and turning the IDs off must not leave the stale names row behind.
    useUIStore.getState().toggleDisplay('showStationLabels');
    expect(pair()).toEqual({ ids: false, names: false });
    expect(labelSaves()).toEqual([
      ['showStationNames', 'false'],
      ['showStationLabels', 'false'],
    ]);
  });

  it('tracks only the toggle the user made, not the partner it turned off', () => {
    useUIStore.setState({ showStationLabels: true });
    useUIStore.getState().toggleDisplay('showStationNames');
    expect(trackActionMock.mock.calls).toEqual([['toggle.showStationNames']]);
  });

  it('never leaves both on, whatever the toggle sequence', () => {
    // A fixed pseudo-random walk (LCG) over the two keys.
    let seed = 7;
    const next = () => (seed = (seed * 1103515245 + 12345) % 2147483648);
    for (let i = 0; i < 200; i++) {
      useUIStore.getState().toggleDisplay(next() % 2 === 0 ? 'showStationLabels' : 'showStationNames');
      const { ids, names } = pair();
      expect(ids && names).toBe(false);
    }
  });

  it('leaves the other toggles alone', () => {
    useUIStore.setState({ showStationLabels: true, showStationComments: true, showSectionNames: true });
    useUIStore.getState().toggleDisplay('showStationNames');
    const s = useUIStore.getState();
    expect(s.showStationComments).toBe(true);
    expect(s.showSectionNames).toBe(true);
  });
});

describe('Station label toggles in Plan/Export', () => {
  it.each(['plan', 'export'] as const)(
    'in %s the switch applies but saves nothing, and Survey gets its pair back on return',
    (mode) => {
      useUIStore.setState({ showStationLabels: true });
      useUIStore.getState().setAppMode(mode);
      vi.clearAllMocks();
      useUIStore.getState().toggleDisplay('showStationNames');
      expect(pair()).toEqual({ ids: false, names: true });
      expect(labelSaves()).toEqual([]);
      useUIStore.getState().setAppMode('survey');
      expect(pair()).toEqual({ ids: true, names: false });
    },
  );
});

describe('Station label toggles in Data', () => {
  it('apply AND save in Data, which shares the Survey view', () => {
    useUIStore.setState({ showStationLabels: true });
    useUIStore.getState().setAppMode('data');
    vi.clearAllMocks();
    useUIStore.getState().toggleDisplay('showStationNames');
    expect(pair()).toEqual({ ids: false, names: true });
    expect(labelSaves().length).toBeGreaterThan(0);
    useUIStore.getState().setAppMode('survey');
    // No snapshot between the editable modes: Survey keeps the Data choice.
    expect(pair()).toEqual({ ids: false, names: true });
  });
});

describe('applyDisplaySettings: the station label pair', () => {
  it('restores a saved names choice', () => {
    useUIStore.getState().applyDisplaySettings(settingsWith({ showStationNames: true }));
    expect(pair()).toEqual({ ids: false, names: true });
  });

  it('a stored pair that reads both-on loads as IDs only, and writes nothing back', () => {
    useUIStore
      .getState()
      .applyDisplaySettings(settingsWith({ showStationLabels: true, showStationNames: true }));
    expect(pair()).toEqual({ ids: true, names: false });
    expect(labelSaves()).toEqual([]);
  });

  it("does not carry one project's names choice into the next", () => {
    useUIStore.getState().applyDisplaySettings(settingsWith({ showStationNames: true }));
    useUIStore.getState().applyDisplaySettings(settingsWith({}));
    expect(pair()).toEqual({ ids: false, names: false });
  });

  it('resetDisplaySettings turns both off', () => {
    useUIStore.setState({ showStationNames: true });
    useUIStore.getState().resetDisplaySettings();
    expect(pair()).toEqual({ ids: DISPLAY_DEFAULTS.showStationLabels, names: DISPLAY_DEFAULTS.showStationNames });
    expect(pair()).toEqual({ ids: false, names: false });
  });
});

describe('getEffectiveStationLabelMode', () => {
  it('maps the pair to off / id / name, with IDs winning a both-on pair', () => {
    expect(getEffectiveStationLabelMode(false, false)).toBe('off');
    expect(getEffectiveStationLabelMode(true, false)).toBe('id');
    expect(getEffectiveStationLabelMode(false, true)).toBe('name');
    expect(getEffectiveStationLabelMode(true, true)).toBe('id');
  });
});

describe('hasStationNames', () => {
  it('is true only when some station has a name', () => {
    const d = emptySurveyData();
    expect(hasStationNames(d)).toBe(false);
    expect(hasStationNames(null)).toBe(false);
    expect(hasStationNames(undefined)).toBe(false);
    d.stationNames = new Map([[3, '12']]);
    expect(hasStationNames(d)).toBe(true);
  });

  it('ignores names that would draw nothing (whitespace, control and bidi characters)', () => {
    const d = emptySurveyData();
    d.stationNames = new Map([[0, '   '], [1, '\u202e\u2066'], [2, '\n\ufeff']]);
    expect(hasStationNames(d)).toBe(false);
    // A new names map (a refreshed project) is evaluated afresh.
    d.stationNames = new Map([[0, '   '], [5, '7']]);
    expect(hasStationNames(d)).toBe(true);
  });
});
