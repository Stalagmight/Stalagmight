/**
 * The Warnings-tab visit: while the Warnings tab is on screen the map paints
 * Survey Issues and draws the Errors labels by default; what the user changes in
 * that time lasts until they leave the tab and is never saved.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockResolvedValue({}),
}));

import { useUIStore } from '../uiStore';
import {
  DISPLAY_DEFAULTS,
  isWarningsVisitActive,
  selectDistanceOverlayActive,
  selectEffectiveColorMode,
  selectEffectiveShowShotErrors,
  selectIssueDisplayAuto,
} from '../displaySelectors';
import type { DisplaySettings } from '../../types/loading';

const saved = () =>
  vi.mocked(invoke).mock.calls.filter(([cmd]) => cmd === 'save_display_setting');

/** Survey workspace, single-survey view, detail panel open on Details, the saved
 *  colour mode Section and the Errors labels off. */
function reset(): void {
  useUIStore.setState({
    view: 'single',
    appMode: 'survey',
    detailPanelOpen: true,
    detailPanelTab: 'details',
    dataTableFullScreen: false,
    warningsAutoDisplay: true,
    warningsVisitOverride: null,
    showDepthGradient: false,
    showDistanceGradient: false,
    showIssueSeverity: false,
    distanceGradientOrigin: null,
    distanceField: null,
    showShotErrors: false,
  });
  vi.mocked(invoke).mockClear();
}

const openWarnings = () => useUIStore.setState({ detailPanelTab: 'warnings' });
const leaveWarnings = () => useUIStore.setState({ detailPanelTab: 'details' });
const effective = () => {
  const s = useUIStore.getState();
  return { color: selectEffectiveColorMode(s), errors: selectEffectiveShowShotErrors(s) };
};

describe('isWarningsVisitActive', () => {
  beforeEach(reset);

  it('is on only while the Warnings tab is the visible tab in Survey', () => {
    const active = () => isWarningsVisitActive(useUIStore.getState());
    expect(active()).toBe(false);
    openWarnings();
    expect(active()).toBe(true);
    for (const off of [
      { detailPanelOpen: false },
      { appMode: 'plan' as const },
      { appMode: 'export' as const },
      { view: 'global' as const },
      { dataTableFullScreen: true },
      { warningsAutoDisplay: false },
    ]) {
      reset();
      openWarnings();
      useUIStore.setState(off);
      expect(active(), JSON.stringify(off)).toBe(false);
    }
  });
});

describe('the Warnings-tab visit', () => {
  beforeEach(reset);

  it('turns Survey Issues and the Errors labels on, without touching the saved flags', () => {
    openWarnings();
    expect(effective()).toEqual({ color: 'issues', errors: true });
    expect(selectIssueDisplayAuto(useUIStore.getState())).toEqual({ visit: true, color: true, errors: true });
    const s = useUIStore.getState();
    expect([s.showIssueSeverity, s.showShotErrors]).toEqual([false, false]);
  });

  it('leaving the tab brings the saved settings back', () => {
    useUIStore.setState({ showDepthGradient: true, showShotErrors: false });
    openWarnings();
    expect(effective()).toEqual({ color: 'issues', errors: true });
    leaveWarnings();
    expect(effective()).toEqual({ color: 'depth', errors: false });
  });

  it('a colour picked during the visit holds for the visit and is never saved', () => {
    openWarnings();
    useUIStore.getState().setColorMode('depth');
    expect(effective().color).toBe('depth');
    expect(selectIssueDisplayAuto(useUIStore.getState()).color).toBe(false);
    expect(useUIStore.getState().showDepthGradient).toBe(false);
    expect(saved()).toHaveLength(0);
    // Unrelated store changes keep the pick.
    useUIStore.setState({ selection: null });
    expect(effective().color).toBe('depth');
    leaveWarnings();
    expect(effective().color).toBe('section');
    expect(useUIStore.getState().warningsVisitOverride).toBeNull();
  });

  it('turning the Errors labels off during the visit holds for the visit and is never saved', () => {
    openWarnings();
    useUIStore.getState().toggleDisplay('showShotErrors');
    expect(effective().errors).toBe(false);
    useUIStore.getState().toggleDisplay('showShotErrors');
    expect(effective().errors).toBe(true);
    useUIStore.getState().toggleDisplay('showShotErrors');
    leaveWarnings();
    expect(effective().errors).toBe(false);
    expect(useUIStore.getState().showShotErrors).toBe(false);
    expect(saved()).toHaveLength(0);
  });

  it('a new visit starts from the defaults again', () => {
    openWarnings();
    useUIStore.getState().setColorMode('section');
    useUIStore.getState().toggleDisplay('showShotErrors');
    leaveWarnings();
    openWarnings();
    expect(effective()).toEqual({ color: 'issues', errors: true });
  });

  it('Shift+D (toggleDisplay depth) flips depth for the visit only', () => {
    openWarnings();
    useUIStore.getState().toggleDisplay('showDepthGradient');
    expect(effective().color).toBe('depth');
    useUIStore.getState().toggleDisplay('showDepthGradient');
    expect(effective().color).toBe('section');
    expect(useUIStore.getState().showDepthGradient).toBe(false);
    expect(saved()).toHaveLength(0);
  });

  it('Distance picked during the visit drives the distance overlay, and its cache goes with the visit', () => {
    openWarnings();
    useUIStore.getState().setColorMode('distance');
    expect(selectDistanceOverlayActive(useUIStore.getState())).toBe(true);
    useUIStore.setState({ distanceGradientOrigin: { section_id: 1, station_index: 2 } });
    leaveWarnings();
    const s = useUIStore.getState();
    expect(selectDistanceOverlayActive(s)).toBe(false);
    expect(s.distanceGradientOrigin).toBeNull();
    expect(s.distanceField).toBeNull();
  });

  it('a saved Distance keeps its origin across a visit', () => {
    const origin = { section_id: 1, station_index: 2 };
    useUIStore.setState({ showDistanceGradient: true, distanceGradientOrigin: origin });
    openWarnings();
    expect(selectDistanceOverlayActive(useUIStore.getState())).toBe(false);
    useUIStore.getState().setColorMode('depth');
    leaveWarnings();
    expect(useUIStore.getState().distanceGradientOrigin).toEqual(origin);
    expect(selectDistanceOverlayActive(useUIStore.getState())).toBe(true);
  });

  it('every way out ends the visit: closing the panel, Plan, the Global view, the setting', () => {
    for (const out of [
      { detailPanelOpen: false },
      { appMode: 'plan' as const },
      { view: 'global' as const },
      { warningsAutoDisplay: false },
    ]) {
      reset();
      openWarnings();
      useUIStore.getState().setColorMode('depth');
      useUIStore.setState(out);
      expect(useUIStore.getState().warningsVisitOverride, JSON.stringify(out)).toBeNull();
    }
  });

  it('with the setting off the Warnings tab changes nothing and toggles save as usual', async () => {
    useUIStore.setState({ warningsAutoDisplay: false });
    openWarnings();
    expect(effective()).toEqual({ color: 'section', errors: false });
    useUIStore.getState().toggleDisplay('showShotErrors');
    expect(useUIStore.getState().showShotErrors).toBe(true);
    await vi.waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('save_display_setting', { key: 'showShotErrors', value: 'true' }),
    );
  });

  it('outside a visit the Errors toggle is an ordinary saved toggle', async () => {
    useUIStore.getState().toggleDisplay('showShotErrors');
    expect(effective().errors).toBe(true);
    await vi.waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('save_display_setting', { key: 'showShotErrors', value: 'true' }),
    );
  });

  it('loading a project drops the visit changes', () => {
    openWarnings();
    useUIStore.getState().setColorMode('depth');
    const settings: DisplaySettings = { ...DISPLAY_DEFAULTS, geometryMode: 'measured' };
    useUIStore.getState().applyDisplaySettings(settings);
    expect(useUIStore.getState().warningsVisitOverride).toBeNull();
  });
});
