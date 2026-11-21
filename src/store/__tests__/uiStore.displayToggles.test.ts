/**
 * Unit tests for display toggle state logic in uiStore.
 * Tests toggle defaults, toggle action, the toast surface, and selection clearing.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';

// Mock Tauri invoke to prevent noisy console warnings and verify persistence calls
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockResolvedValue({}),
}));

import { useUIStore, colorModeOf, colorModeFlags, type DisplayToggleKey } from '../uiStore';
import {
  DISPLAY_DEFAULTS,
  getEffectiveShowReferenceShots,
  getEffectiveShowShotData,
} from '../displaySelectors';
import type { DisplaySettings } from '../../types/loading';

// A saved-settings payload for the load path. Spreads the documented single source
// of truth for the defaults instead of hand-listing them, so a changed default
// fails loudly here rather than the test asserting against a stale copy.
const settingsWith = (overrides: Partial<DisplaySettings>): DisplaySettings => ({
  ...DISPLAY_DEFAULTS,
  geometryMode: 'measured',
  ...overrides,
});

// Helper function to get a clean test state for non-default tests
// This is used to reset between tests that modify state, NOT for testing defaults
const getCleanTestState = () => ({
  showStations: true,
  showShots: true,
  showSectionNames: true,
  showStationLabels: false,
  showStationNames: false,
  showShotData: false,
  showShotErrors: false,
  showReferenceShots: true,
  showZeroDatum: true,
  showGrid: false,
  showSatellite: false,
  showDepthGradient: false,
  // The mutually-exclusive coloring overlays must be reset too, or a test that
  // enables one leaks it into later tests (setState merges partials) (#57 #12).
  showDistanceGradient: false,
  distanceGradientOrigin: null,
  distanceField: null,
  showIssueSeverity: false,
  // The Debug submenu's four toggles too - setState merges partials, so a test
  // that flips one would otherwise leak it into every later test in the file.
  showDevStats: false,
  showMemoryStats: false,
  showMapStats: false,
  showZoomStats: false,
  gridSpacing: 10,
  toasts: [],
  selection: null,
  multiSelection: [],
});

describe('Display Toggle State', () => {
  describe('Store Uses DISPLAY_DEFAULTS', () => {
    // These tests verify that the store derives its values from DISPLAY_DEFAULTS.
    // Since uiStore imports and uses DISPLAY_DEFAULTS for initial state, we verify
    // the store values match DISPLAY_DEFAULTS at any point after reset.
    // This catches drift if someone changes one but not the other.

    beforeEach(() => {
      // Reset to verify the store's resetDisplaySettings uses DISPLAY_DEFAULTS
      useUIStore.getState().resetDisplaySettings();
    });

    it('store showStations uses DISPLAY_DEFAULTS value', () => {
      expect(useUIStore.getState().showStations).toBe(DISPLAY_DEFAULTS.showStations);
    });

    it('store showShots uses DISPLAY_DEFAULTS value', () => {
      expect(useUIStore.getState().showShots).toBe(DISPLAY_DEFAULTS.showShots);
    });

    it('store showSectionNames uses DISPLAY_DEFAULTS value', () => {
      expect(useUIStore.getState().showSectionNames).toBe(DISPLAY_DEFAULTS.showSectionNames);
    });

    it('store showStationLabels uses DISPLAY_DEFAULTS value', () => {
      expect(useUIStore.getState().showStationLabels).toBe(DISPLAY_DEFAULTS.showStationLabels);
    });

    it('store showShotData uses DISPLAY_DEFAULTS value', () => {
      expect(useUIStore.getState().showShotData).toBe(DISPLAY_DEFAULTS.showShotData);
    });

    it('store showShotErrors uses DISPLAY_DEFAULTS value (off)', () => {
      expect(useUIStore.getState().showShotErrors).toBe(DISPLAY_DEFAULTS.showShotErrors);
      expect(DISPLAY_DEFAULTS.showShotErrors).toBe(false);
    });

    it('store showReferenceShots uses DISPLAY_DEFAULTS value', () => {
      expect(useUIStore.getState().showReferenceShots).toBe(DISPLAY_DEFAULTS.showReferenceShots);
    });

    it('store showZeroDatum uses DISPLAY_DEFAULTS value', () => {
      expect(useUIStore.getState().showZeroDatum).toBe(DISPLAY_DEFAULTS.showZeroDatum);
    });

    it('store showGrid uses DISPLAY_DEFAULTS value', () => {
      expect(useUIStore.getState().showGrid).toBe(DISPLAY_DEFAULTS.showGrid);
    });

    it('store showSatellite uses DISPLAY_DEFAULTS value', () => {
      expect(useUIStore.getState().showSatellite).toBe(DISPLAY_DEFAULTS.showSatellite);
    });

    it('store showDepthGradient uses DISPLAY_DEFAULTS value', () => {
      expect(useUIStore.getState().showDepthGradient).toBe(DISPLAY_DEFAULTS.showDepthGradient);
    });

    it('store gridSpacing uses DISPLAY_DEFAULTS value', () => {
      expect(useUIStore.getState().gridSpacing).toBe(DISPLAY_DEFAULTS.gridSpacing);
    });

    it('DISPLAY_DEFAULTS contains all expected toggle keys', () => {
      // Verify DISPLAY_DEFAULTS has all the keys we expect
      // This catches if someone adds a new toggle to the store but forgets DISPLAY_DEFAULTS
      const expectedKeys = [
        'showStations',
        'showShots',
        'showSectionNames',
        'showStationLabels',
        'showStationNames',
        'showShotData',
        'showShotErrors',
        'showReferenceShots',
        'showZeroDatum',
        'showGrid',
        'showSatellite',
        'showDepthGradient',
        'gridSpacing',
      ];
      for (const key of expectedKeys) {
        expect(DISPLAY_DEFAULTS).toHaveProperty(key);
      }
    });
  });

  describe('resetDisplaySettings Action', () => {
    it('resets all display settings to DISPLAY_DEFAULTS', () => {
      // Modify ALL settings to non-default values
      useUIStore.setState({
        showStations: !DISPLAY_DEFAULTS.showStations,
        showShots: !DISPLAY_DEFAULTS.showShots,
        showSectionNames: !DISPLAY_DEFAULTS.showSectionNames,
        showStationLabels: !DISPLAY_DEFAULTS.showStationLabels,
        showStationNames: !DISPLAY_DEFAULTS.showStationNames,
        showShotData: !DISPLAY_DEFAULTS.showShotData,
        showShotErrors: !DISPLAY_DEFAULTS.showShotErrors,
        showReferenceShots: !DISPLAY_DEFAULTS.showReferenceShots,
        showZeroDatum: !DISPLAY_DEFAULTS.showZeroDatum,
        showGrid: !DISPLAY_DEFAULTS.showGrid,
        showSatellite: !DISPLAY_DEFAULTS.showSatellite,
        showDepthGradient: !DISPLAY_DEFAULTS.showDepthGradient,
        gridSpacing: 99,
      });

      // Reset
      useUIStore.getState().resetDisplaySettings();

      // Verify ALL defaults are restored matching DISPLAY_DEFAULTS
      const state = useUIStore.getState();
      expect(state.showStations).toBe(DISPLAY_DEFAULTS.showStations);
      expect(state.showShots).toBe(DISPLAY_DEFAULTS.showShots);
      expect(state.showSectionNames).toBe(DISPLAY_DEFAULTS.showSectionNames);
      expect(state.showStationLabels).toBe(DISPLAY_DEFAULTS.showStationLabels);
      expect(state.showStationNames).toBe(DISPLAY_DEFAULTS.showStationNames);
      expect(state.showShotData).toBe(DISPLAY_DEFAULTS.showShotData);
      expect(state.showShotErrors).toBe(DISPLAY_DEFAULTS.showShotErrors);
      expect(state.showReferenceShots).toBe(DISPLAY_DEFAULTS.showReferenceShots);
      expect(state.showZeroDatum).toBe(DISPLAY_DEFAULTS.showZeroDatum);
      expect(state.showGrid).toBe(DISPLAY_DEFAULTS.showGrid);
      expect(state.showSatellite).toBe(DISPLAY_DEFAULTS.showSatellite);
      expect(state.showDepthGradient).toBe(DISPLAY_DEFAULTS.showDepthGradient);
      expect(state.gridSpacing).toBe(DISPLAY_DEFAULTS.gridSpacing);
    });
  });

  describe('toggleDisplay Action', () => {
    beforeEach(() => {
      useUIStore.setState(getCleanTestState());
      vi.clearAllMocks();
    });

    it('toggles showStations from true to false', () => {
      const { toggleDisplay } = useUIStore.getState();
      expect(useUIStore.getState().showStations).toBe(true);
      toggleDisplay('showStations');
      expect(useUIStore.getState().showStations).toBe(false);
    });

    it('toggles showStations from false to true', () => {
      useUIStore.setState({ showStations: false });
      const { toggleDisplay } = useUIStore.getState();
      expect(useUIStore.getState().showStations).toBe(false);
      toggleDisplay('showStations');
      expect(useUIStore.getState().showStations).toBe(true);
    });

    it('toggles showShots correctly', () => {
      const { toggleDisplay } = useUIStore.getState();
      expect(useUIStore.getState().showShots).toBe(true);
      toggleDisplay('showShots');
      expect(useUIStore.getState().showShots).toBe(false);
      toggleDisplay('showShots');
      expect(useUIStore.getState().showShots).toBe(true);
    });

    it('toggles all display keys independently', () => {
      const toggleKeys: DisplayToggleKey[] = [
        'showStations',
        'showShots',
        'showSectionNames',
        'showStationLabels',
        'showShotData',
        'showShotErrors',
        'showReferenceShots',
        'showZeroDatum',
        'showGrid',
        'showSatellite',
        'showDepthGradient',
      ];

      for (const key of toggleKeys) {
        const initialValue = useUIStore.getState()[key];
        useUIStore.getState().toggleDisplay(key);
        expect(useUIStore.getState()[key]).toBe(!initialValue);
      }
    });

    it('persists toggle state via invoke', async () => {
      const { toggleDisplay } = useUIStore.getState();

      toggleDisplay('showStations');

      // Wait for async persistence
      await vi.waitFor(() => {
        expect(invoke).toHaveBeenCalledWith('save_display_setting', {
          key: 'showStations',
          value: 'false',
        });
      });
    });

    it('persists each toggle with correct value', async () => {
      const { toggleDisplay } = useUIStore.getState();

      // Toggle showGrid (default false -> true)
      toggleDisplay('showGrid');

      await vi.waitFor(() => {
        expect(invoke).toHaveBeenCalledWith('save_display_setting', {
          key: 'showGrid',
          value: 'true',
        });
      });
    });

    // The Debug submenu's four toggles are session-only (reset on every project
    // load, never restored from the backend - see applyDisplaySettings), so
    // persisting them would only ever write orphan rows into the project's
    // display-settings table. toggleDisplay must skip the IPC call for them.
    it.each(['showDevStats', 'showMemoryStats', 'showMapStats', 'showZoomStats'] as const)(
      'does not persist the session-only %s toggle',
      async (key) => {
        const { toggleDisplay } = useUIStore.getState();

        toggleDisplay(key);

        expect(useUIStore.getState()[key]).toBe(true);
        // Give any (incorrect) async persistence call a chance to fire before asserting absence.
        await new Promise((resolve) => setTimeout(resolve, 10));
        expect(invoke).not.toHaveBeenCalledWith('save_display_setting', expect.objectContaining({ key }));
      }
    );
  });

  // Depth and distance gradients both paint the shots layer; showing both at once
  // produced mixed coloring (depth-colored shots, distance-colored stations after
  // a Shots toggle). They are mutually exclusive - enabling one disables the other
  // - so the conflicting state is unreachable (#34).
  describe('Distance/Depth Gradient Mutual Exclusion (#34)', () => {
    beforeEach(() => {
      useUIStore.setState(getCleanTestState());
      vi.clearAllMocks();
    });

    it('enabling the distance gradient turns the depth gradient off', () => {
      useUIStore.setState({ showDepthGradient: true, showDistanceGradient: false });
      useUIStore.getState().toggleDistanceGradient();
      const s = useUIStore.getState();
      expect(s.showDistanceGradient).toBe(true);
      expect(s.showDepthGradient).toBe(false);
    });

    it('enabling the depth gradient turns the distance gradient off and clears its origin/field', () => {
      useUIStore.setState({
        showDistanceGradient: true,
        distanceGradientOrigin: { section_id: 1, station_index: 2 },
        distanceField: {} as never,
        showDepthGradient: false,
      });
      useUIStore.getState().toggleDisplay('showDepthGradient');
      const s = useUIStore.getState();
      expect(s.showDepthGradient).toBe(true);
      expect(s.showDistanceGradient).toBe(false);
      expect(s.distanceGradientOrigin).toBeNull();
      expect(s.distanceField).toBeNull();
    });

    it('never leaves both gradients enabled, from either entry point', () => {
      useUIStore.setState({ showDepthGradient: true, showDistanceGradient: false });
      useUIStore.getState().toggleDistanceGradient();
      let s = useUIStore.getState();
      expect(s.showDepthGradient && s.showDistanceGradient).toBe(false);
      useUIStore.getState().toggleDisplay('showDepthGradient');
      s = useUIStore.getState();
      expect(s.showDepthGradient && s.showDistanceGradient).toBe(false);
    });

    it('does not persist depth=off when it is suppressed by enabling the distance overlay', () => {
      useUIStore.setState({ showDepthGradient: true, showDistanceGradient: false });
      useUIStore.getState().toggleDistanceGradient();
      // Distance is runtime-only; the depth gradient's persisted preference must
      // be left untouched so it returns next session.
      expect(invoke).not.toHaveBeenCalledWith(
        'save_display_setting',
        expect.objectContaining({ key: 'showDepthGradient' })
      );
    });
  });

  // Issue-severity coloring also paints the shots/stations layer, so it is
  // mutually exclusive with both gradients and, like the distance overlay, is
  // runtime-only (never persisted) (#22).
  describe('Issue-severity Mutual Exclusion (#22)', () => {
    beforeEach(() => {
      useUIStore.setState(getCleanTestState());
      vi.clearAllMocks();
    });

    it('enabling issue severity turns off both gradients and clears the distance origin/field', () => {
      useUIStore.setState({
        showDepthGradient: true,
        showDistanceGradient: true,
        distanceGradientOrigin: { section_id: 1, station_index: 2 },
        distanceField: {} as never,
        showIssueSeverity: false,
      });
      useUIStore.getState().toggleIssueSeverity();
      const s = useUIStore.getState();
      expect(s.showIssueSeverity).toBe(true);
      expect(s.showDepthGradient).toBe(false);
      expect(s.showDistanceGradient).toBe(false);
      expect(s.distanceGradientOrigin).toBeNull();
      expect(s.distanceField).toBeNull();
    });

    it('enabling the depth gradient turns issue severity off', () => {
      useUIStore.setState({ showIssueSeverity: true, showDepthGradient: false });
      useUIStore.getState().toggleDisplay('showDepthGradient');
      const s = useUIStore.getState();
      expect(s.showDepthGradient).toBe(true);
      expect(s.showIssueSeverity).toBe(false);
    });

    it('enabling the distance gradient turns issue severity off', () => {
      useUIStore.setState({ showIssueSeverity: true, showDistanceGradient: false });
      useUIStore.getState().toggleDistanceGradient();
      const s = useUIStore.getState();
      expect(s.showDistanceGradient).toBe(true);
      expect(s.showIssueSeverity).toBe(false);
    });

    it('setIssueSeverity(true) enables it exclusively; setIssueSeverity(false) clears it', () => {
      useUIStore.setState({ showDepthGradient: true, showIssueSeverity: false });
      useUIStore.getState().setIssueSeverity(true);
      let s = useUIStore.getState();
      expect(s.showIssueSeverity).toBe(true);
      expect(s.showDepthGradient).toBe(false);
      useUIStore.getState().setIssueSeverity(false);
      s = useUIStore.getState();
      expect(s.showIssueSeverity).toBe(false);
    });

    it('is runtime-only - toggling it never persists a display setting', () => {
      useUIStore.setState({ showIssueSeverity: false });
      useUIStore.getState().toggleIssueSeverity();
      expect(invoke).not.toHaveBeenCalledWith(
        'save_display_setting',
        expect.objectContaining({ key: 'showIssueSeverity' })
      );
    });
  });

  // The three overlays are single-select, but only depth is persisted - so the
  // pair "saved depth = on" + "runtime distance = on" was reachable: any re-apply
  // of the saved display settings (project load, revision restore) wrote depth
  // back to true while distance was still up. colorModeOf reads that pair as
  // 'distance', and the old writers were `if (!flag) toggle()` guards that no-op
  // on it - so picking Depth in the Display menu did nothing, for good. These
  // pin that setColorMode is authoritative (lands on the requested mode from ANY
  // starting combination) and that the load path can't build the bad pair.
  describe('setColorMode single-select authority (stuck-overlay regression)', () => {
    const ui = () => useUIStore.getState();

    beforeEach(() => {
      useUIStore.setState(getCleanTestState());
      vi.clearAllMocks();
    });

    it('reaches depth from the invalid depth+distance pair', () => {
      useUIStore.setState({ showDepthGradient: true, showDistanceGradient: true });
      // The pair reads as distance, so the menu shows Distance selected...
      expect(colorModeOf(ui())).toBe('distance');
      ui().setColorMode('depth');
      expect(colorModeOf(ui())).toBe('depth');
      expect(ui().showDistanceGradient).toBe(false);
    });

    it('repro: distance on, a load re-applies the saved depth preference, then pick Depth', () => {
      ui().setColorMode('distance');
      ui().setDistanceGradientOrigin({ section_id: 1, station_index: 2 });
      // Project load / revision restore: the saved depth=on comes back underneath
      // the runtime-only distance overlay. The load must leave ONE valid mode.
      ui().applyDisplaySettings(settingsWith({ showDepthGradient: true }));
      expect(ui().showDistanceGradient).toBe(false);
      expect(ui().distanceGradientOrigin).toBeNull();
      // ...and the Display menu must still be able to select Depth.
      ui().setColorMode('depth');
      expect(colorModeOf(ui())).toBe('depth');
      expect(ui().showDistanceGradient).toBe(false);
    });

    it('round-trips distance -> depth -> distance -> depth with the saved depth live', () => {
      // A bare round trip always worked; what broke it is the persisted depth flag
      // returning underneath the runtime-only distance overlay, so the re-apply is
      // part of the round trip being pinned here.
      ui().setColorMode('distance');
      ui().applyDisplaySettings(settingsWith({ showDepthGradient: true }));
      ui().setColorMode('depth');
      expect(colorModeOf(ui())).toBe('depth');
      ui().setColorMode('distance');
      expect(colorModeOf(ui())).toBe('distance');
      expect(ui().showDepthGradient).toBe(false);
      ui().setColorMode('depth');
      expect(colorModeOf(ui())).toBe('depth');
      expect(ui().showDistanceGradient).toBe(false);
    });

    it("selecting 'section' from the invalid pair clears every overlay and the distance cache", () => {
      useUIStore.setState({
        showDepthGradient: true,
        showDistanceGradient: true,
        distanceGradientOrigin: { section_id: 1, station_index: 2 },
        distanceField: {} as never,
      });
      ui().setColorMode('section');
      const s = ui();
      expect(colorModeOf(s)).toBe('section');
      expect(s.showDepthGradient).toBe(false);
      expect(s.showDistanceGradient).toBe(false);
      expect(s.distanceGradientOrigin).toBeNull();
      expect(s.distanceField).toBeNull();
    });

    it("leaving distance for 'section' still clears the origin and cached field", () => {
      ui().setColorMode('distance');
      ui().setDistanceGradientOrigin({ section_id: 3, station_index: 4 });
      useUIStore.setState({ distanceField: {} as never });
      ui().setColorMode('section');
      const s = ui();
      expect(colorModeOf(s)).toBe('section');
      expect(s.distanceGradientOrigin).toBeNull();
      expect(s.distanceField).toBeNull();
    });

    it('selecting distance suppresses depth at runtime without touching its saved preference', () => {
      // Distance is transient (#34): the depth preference must survive it and come
      // back next session, so no save_display_setting for depth may be emitted.
      useUIStore.setState({ showDepthGradient: true });
      ui().setColorMode('distance');
      expect(ui().showDepthGradient).toBe(false);
      expect(invoke).not.toHaveBeenCalledWith(
        'save_display_setting',
        expect.objectContaining({ key: 'showDepthGradient' })
      );
    });

    it('selecting depth persists it on, and deselecting it persists it off', async () => {
      ui().setColorMode('depth');
      await vi.waitFor(() => {
        expect(invoke).toHaveBeenCalledWith('save_display_setting', {
          key: 'showDepthGradient',
          value: 'true',
        });
      });
      ui().setColorMode('section');
      await vi.waitFor(() => {
        expect(invoke).toHaveBeenCalledWith('save_display_setting', {
          key: 'showDepthGradient',
          value: 'false',
        });
      });
    });

    it.each([
      ['distance', 'distance'],
      ['issue-severity', 'issues'],
    ] as const)(
      "picking None after %s took depth over still persists depth OFF",
      async (_name, overlay) => {
        // depth -> overlay -> None. The overlay suppressed depth's RUNTIME flag
        // (#34), so by the time None arrives showDepthGradient already reads false
        // while the SAVED value is still true - the one state where the two
        // disagree. None means no overlay, suppressed or not, so the deselect has to
        // reach the disk; gating the write on the runtime flag made it a no-op and
        // depth silently came back next session.
        useUIStore.setState({ appMode: 'survey' });
        ui().setColorMode('depth');
        ui().setColorMode(overlay);
        expect(ui().showDepthGradient).toBe(false);
        vi.clearAllMocks();
        ui().setColorMode('section');
        expect(colorModeOf(ui())).toBe('section');
        await vi.waitFor(() => {
          expect(invoke).toHaveBeenCalledWith('save_display_setting', {
            key: 'showDepthGradient',
            value: 'false',
          });
        });
      }
    );

    it('picking None with no overlay on writes nothing (the deselect is not unconditional)', () => {
      // Pins the narrow trigger above: the extra write exists only for the
      // suppressed-depth case. Turning it into a blanket write on every None would
      // emit an IPC round trip on a pure no-op.
      useUIStore.setState({ appMode: 'survey' });
      expect(colorModeOf(ui())).toBe('section');
      vi.clearAllMocks();
      ui().setColorMode('section');
      expect(invoke).not.toHaveBeenCalledWith(
        'save_display_setting',
        expect.objectContaining({ key: 'showDepthGradient' })
      );
    });

    it.each([
      ['distance', () => useUIStore.setState({ showDistanceGradient: true })],
      ['issue-severity', () => useUIStore.setState({ showIssueSeverity: true })],
    ])(
      'applyDisplaySettings cannot restore depth on top of the %s overlay',
      (_name, enableRuntimeOverlay) => {
        enableRuntimeOverlay();
        ui().applyDisplaySettings(settingsWith({ showDepthGradient: true }));
        const s = ui();
        expect(colorModeOf(s)).toBe('depth');
        expect(s.showDistanceGradient).toBe(false);
        expect(s.showIssueSeverity).toBe(false);
      }
    );

    it('colorModeFlags sets exactly one overlay flag per mode', () => {
      // Asserted on the FLAG TRIPLE, not just the round-trip through
      // colorModeOf. The round-trip alone is far weaker than it looks:
      // colorModeOf is a priority ladder (issues > distance > depth), so it
      // ABSORBS a leak into a higher-priority mode. Injecting
      // `showDepthGradient: mode === 'depth' || mode === 'distance'` into
      // colorModeFlags reproduces the exact invalid pair this whole change
      // exists to make unrepresentable, and the round-trip still returns
      // 'distance' and still passes.
      //
      // That matters more than a normal test-strength nit, because no renderer
      // re-checks the invariant: MapCanvas.tsx's `effectiveDepthGradient` and
      // colorResolvers.ts both gate depth against issue-severity ONLY, never
      // against showDistanceGradient. This assertion is the thing standing
      // between a colorModeFlags regression and depth painting over distance.
      for (const mode of ['section', 'depth', 'distance', 'issues'] as const) {
        expect(colorModeFlags(mode)).toMatchObject({
          showDepthGradient: mode === 'depth',
          showDistanceGradient: mode === 'distance',
          showIssueSeverity: mode === 'issues',
        });
      }
    });

    it('colorModeFlags round-trips through colorModeOf for every mode', () => {
      for (const mode of ['section', 'depth', 'distance', 'issues'] as const) {
        expect(colorModeOf({ ...getCleanTestState(), ...colorModeFlags(mode) })).toBe(mode);
      }
    });
  });
});

describe('Effective Value Selectors', () => {
  // These tests verify the effective value gating logic used for rendering.
  // Effective values determine actual visibility based on parent-child toggle relationships.

  describe('getEffectiveShowReferenceShots', () => {
    it('returns true when both showShots and showReferenceShots are true', () => {
      expect(getEffectiveShowReferenceShots(true, true)).toBe(true);
    });

    it('returns false when showShots is false (regardless of showReferenceShots)', () => {
      expect(getEffectiveShowReferenceShots(false, true)).toBe(false);
      expect(getEffectiveShowReferenceShots(false, false)).toBe(false);
    });

    it('returns false when showReferenceShots is false (regardless of showShots)', () => {
      expect(getEffectiveShowReferenceShots(true, false)).toBe(false);
    });
  });

  describe('getEffectiveShowShotData', () => {
    it('returns true when both showShots and showShotData are true', () => {
      expect(getEffectiveShowShotData(true, true)).toBe(true);
    });

    it('returns false when showShots is false (regardless of showShotData)', () => {
      expect(getEffectiveShowShotData(false, true)).toBe(false);
      expect(getEffectiveShowShotData(false, false)).toBe(false);
    });

    it('returns false when showShotData is false (regardless of showShots)', () => {
      expect(getEffectiveShowShotData(true, false)).toBe(false);
    });
  });

  describe('Effective values from store state', () => {
    // These tests verify that effective values computed from store state
    // match what the pure selector functions return.
    // This validates the integration between store state and selectors.

    beforeEach(() => {
      useUIStore.setState(getCleanTestState());
    });

    it('effective reference shots is true when store has showShots=true and showReferenceShots=true', () => {
      const state = useUIStore.getState();
      expect(getEffectiveShowReferenceShots(state.showShots, state.showReferenceShots)).toBe(true);
    });

    it('effective reference shots is false when showShots is toggled off', () => {
      useUIStore.getState().toggleDisplay('showShots');
      const state = useUIStore.getState();
      expect(getEffectiveShowReferenceShots(state.showShots, state.showReferenceShots)).toBe(false);
    });

    it('effective reference shots is false when showReferenceShots is toggled off', () => {
      useUIStore.getState().toggleDisplay('showReferenceShots');
      const state = useUIStore.getState();
      expect(getEffectiveShowReferenceShots(state.showShots, state.showReferenceShots)).toBe(false);
    });

    it('effective shot data is false by default (showShotData defaults to false)', () => {
      const state = useUIStore.getState();
      expect(getEffectiveShowShotData(state.showShots, state.showShotData)).toBe(false);
    });

    it('effective shot data is true when showShotData is toggled on', () => {
      useUIStore.getState().toggleDisplay('showShotData');
      const state = useUIStore.getState();
      expect(getEffectiveShowShotData(state.showShots, state.showShotData)).toBe(true);
    });

    it('effective shot data is false when showShots is off (even with showShotData=true)', () => {
      useUIStore.getState().toggleDisplay('showShotData'); // on
      useUIStore.getState().toggleDisplay('showShots'); // off
      const state = useUIStore.getState();
      expect(getEffectiveShowShotData(state.showShots, state.showShotData)).toBe(false);
    });
  });
});

describe('Toggle State Preservation', () => {
  // These tests verify that the store preserves toggle states independently.
  // The "effective value" gating is computed by selectors; the store just holds state.

  beforeEach(() => {
    useUIStore.setState(getCleanTestState());
  });

  it('showReferenceShots state is preserved when showShots toggles off/on', () => {
    // Initial: showShots=true, showReferenceShots=true
    expect(useUIStore.getState().showShots).toBe(true);
    expect(useUIStore.getState().showReferenceShots).toBe(true);

    // Turn off showReferenceShots
    useUIStore.getState().toggleDisplay('showReferenceShots');
    expect(useUIStore.getState().showReferenceShots).toBe(false);

    // Toggle showShots off
    useUIStore.getState().toggleDisplay('showShots');
    expect(useUIStore.getState().showShots).toBe(false);

    // showReferenceShots state should be preserved (still false)
    expect(useUIStore.getState().showReferenceShots).toBe(false);

    // Toggle showShots back on
    useUIStore.getState().toggleDisplay('showShots');
    expect(useUIStore.getState().showShots).toBe(true);

    // showReferenceShots should still be false (preserved state)
    expect(useUIStore.getState().showReferenceShots).toBe(false);
  });

  it('showShotData state is preserved when showShots toggles off/on', () => {
    // Turn on showShotData
    useUIStore.getState().toggleDisplay('showShotData');
    expect(useUIStore.getState().showShotData).toBe(true);

    // Toggle showShots off
    useUIStore.getState().toggleDisplay('showShots');

    // showShotData state should be preserved
    expect(useUIStore.getState().showShotData).toBe(true);

    // Toggle showShots back on
    useUIStore.getState().toggleDisplay('showShots');

    // showShotData should still be true (preserved state)
    expect(useUIStore.getState().showShotData).toBe(true);
  });

  it('multiple dependent toggles preserve state through parent toggle cycle', () => {
    // Set up: showShots=true, showReferenceShots=false, showShotData=true
    useUIStore.getState().toggleDisplay('showReferenceShots'); // true -> false
    useUIStore.getState().toggleDisplay('showShotData'); // false -> true

    expect(useUIStore.getState().showShots).toBe(true);
    expect(useUIStore.getState().showReferenceShots).toBe(false);
    expect(useUIStore.getState().showShotData).toBe(true);

    // Toggle parent off then on
    useUIStore.getState().toggleDisplay('showShots');
    useUIStore.getState().toggleDisplay('showShots');

    // Both dependent states should be preserved
    expect(useUIStore.getState().showReferenceShots).toBe(false);
    expect(useUIStore.getState().showShotData).toBe(true);
  });
});

/**
 * Toast behaviour moved to the stack model (one array, per-key refresh, capped at
 * MAX_VISIBLE_TOASTS) and is covered in full by uiStore.toast.test.ts. What stays
 * here is only the display-toggle suite's own interest in it: that a toast lands,
 * and that clearToast wipes the stack.
 */
describe('Toasts (display-toggle surface)', () => {
  beforeEach(() => {
    useUIStore.setState(getCleanTestState());
  });

  it('first toast shows immediately', () => {
    useUIStore.getState().showToast('Test message', 'test-key', 'info');
    expect(useUIStore.getState().toasts.map((t) => t.message)).toEqual(['Test message']);
  });

  it('toasts with different keys coexist', () => {
    const { showToast } = useUIStore.getState();
    showToast('Stations message', 'stations', 'info');
    showToast('Shots message', 'shots', 'info');
    expect(useUIStore.getState().toasts.map((t) => t.message)).toEqual([
      'Stations message',
      'Shots message',
    ]);
  });

  it('clearToast removes every toast', () => {
    const { showToast, clearToast } = useUIStore.getState();

    showToast('Test', 'key', 'info');
    expect(useUIStore.getState().toasts).toHaveLength(1);

    clearToast();
    expect(useUIStore.getState().toasts).toEqual([]);
  });
});

describe('Selection Clearing', () => {
  beforeEach(() => {
    useUIStore.setState(getCleanTestState());
  });

  it('clearSelectionByType clears only stations when type is station', () => {
    const stationSelection = {
      type: 'station' as const,
      stationRef: { section_id: 1, station_index: 0 },
    };
    const shotSelection = {
      type: 'shot' as const,
      sectionId: 1,
      shotId: 0,
    };

    // Add both selections
    useUIStore.getState().addToSelection(stationSelection);
    useUIStore.getState().addToSelection(shotSelection);

    expect(useUIStore.getState().multiSelection).toHaveLength(2);

    // Clear only stations
    useUIStore.getState().clearSelectionByType('station');

    // Only shot should remain
    const remaining = useUIStore.getState().multiSelection;
    expect(remaining).toHaveLength(1);
    expect(remaining[0]!.type).toBe('shot');
    expect(remaining[0]!.sectionId).toBe(1);
    expect(remaining[0]!.shotId).toBe(0);
  });

  it('clearSelectionByType clears only shots when type is shot', () => {
    const stationSelection = {
      type: 'station' as const,
      stationRef: { section_id: 1, station_index: 0 },
    };
    const shotSelection = {
      type: 'shot' as const,
      sectionId: 1,
      shotId: 0,
    };

    // Add both selections
    useUIStore.getState().addToSelection(stationSelection);
    useUIStore.getState().addToSelection(shotSelection);

    expect(useUIStore.getState().multiSelection).toHaveLength(2);

    // Clear only shots
    useUIStore.getState().clearSelectionByType('shot');

    // Only station should remain
    const remaining = useUIStore.getState().multiSelection;
    expect(remaining).toHaveLength(1);
    expect(remaining[0]!.type).toBe('station');
    expect(remaining[0]!.stationRef?.section_id).toBe(1);
    expect(remaining[0]!.stationRef?.station_index).toBe(0);
  });

  it('clearSelectionByType updates selection to last remaining item', () => {
    const stationSelection1 = {
      type: 'station' as const,
      stationRef: { section_id: 1, station_index: 0 },
    };
    const stationSelection2 = {
      type: 'station' as const,
      stationRef: { section_id: 1, station_index: 1 },
    };
    const shotSelection = {
      type: 'shot' as const,
      sectionId: 1,
      shotId: 0,
    };

    // Add selections in order
    useUIStore.getState().addToSelection(stationSelection1);
    useUIStore.getState().addToSelection(stationSelection2);
    useUIStore.getState().addToSelection(shotSelection);

    // Current selection should be the last added (shot)
    expect(useUIStore.getState().selection?.type).toBe('shot');

    // Clear shots - selection should update to last remaining station (station2)
    useUIStore.getState().clearSelectionByType('shot');

    const selection = useUIStore.getState().selection;
    expect(selection?.type).toBe('station');
    // Verify it's specifically station2 (station_index: 1), not station1
    expect(selection?.stationRef?.station_index).toBe(1);
    expect(useUIStore.getState().multiSelection).toHaveLength(2);
  });

  it('filterSelection removes items that fail predicate', () => {
    const stationSelection = {
      type: 'station' as const,
      stationRef: { section_id: 1, station_index: 0 },
    };
    const shotSelection = {
      type: 'shot' as const,
      sectionId: 1,
      shotId: 0,
    };

    // Add both selections
    useUIStore.getState().addToSelection(stationSelection);
    useUIStore.getState().addToSelection(shotSelection);

    // Filter to keep only stations
    useUIStore.getState().filterSelection((s) => s.type === 'station');

    const remaining = useUIStore.getState().multiSelection;
    expect(remaining).toHaveLength(1);
    expect(remaining[0]!.type).toBe('station');
    expect(remaining[0]!.stationRef?.section_id).toBe(1);
    expect(remaining[0]!.stationRef?.station_index).toBe(0);
  });

  it('clearSelectionByType with empty selection does nothing', () => {
    expect(useUIStore.getState().multiSelection).toHaveLength(0);
    expect(useUIStore.getState().selection).toBeNull();

    useUIStore.getState().clearSelectionByType('station');

    expect(useUIStore.getState().multiSelection).toHaveLength(0);
    expect(useUIStore.getState().selection).toBeNull();
  });
});

describe('Grid Spacing', () => {
  beforeEach(() => {
    useUIStore.setState(getCleanTestState());
    vi.clearAllMocks();
  });

  it('setGridSpacing updates gridSpacing value', () => {
    expect(useUIStore.getState().gridSpacing).toBe(10);

    useUIStore.getState().setGridSpacing(25);
    expect(useUIStore.getState().gridSpacing).toBe(25);

    useUIStore.getState().setGridSpacing(5);
    expect(useUIStore.getState().gridSpacing).toBe(5);
  });

  it('setGridSpacing persists value via invoke', async () => {
    useUIStore.getState().setGridSpacing(25);

    await vi.waitFor(() => {
      expect(invoke).toHaveBeenCalledWith('save_display_setting', {
        key: 'gridSpacing',
        value: '25',
      });
    });
  });
});

describe('Geometry Mode', () => {
  beforeEach(() => {
    useUIStore.setState({ geometryMode: 'measured' });
    vi.clearAllMocks();
  });

  it('defaults to measured after reset', () => {
    useUIStore.setState({ geometryMode: 'closureAdjusted' });
    useUIStore.getState().resetDisplaySettings();
    expect(useUIStore.getState().geometryMode).toBe('measured');
  });

  it('setGeometryMode updates state', () => {
    useUIStore.getState().setGeometryMode('closureAdjusted');
    expect(useUIStore.getState().geometryMode).toBe('closureAdjusted');
  });

  it('setGeometryMode persists the mode via invoke', async () => {
    useUIStore.getState().setGeometryMode('closureAdjusted');
    await vi.waitFor(() => {
      expect(invoke).toHaveBeenCalledWith('save_display_setting', {
        key: 'geometryMode',
        value: 'closureAdjusted',
      });
    });
  });

  it('setGeometryMode is a no-op when already in that mode', () => {
    useUIStore.setState({ geometryMode: 'measured' });
    vi.clearAllMocks();
    useUIStore.getState().setGeometryMode('measured');
    expect(invoke).not.toHaveBeenCalled();
  });

  it('applyDisplaySettings restores the saved geometryMode and never inherits the previous one', () => {
    // No saved preference (backend sends "measured") → measured, not carried over.
    useUIStore.setState({ geometryMode: 'closureAdjusted' });
    useUIStore.getState().applyDisplaySettings({ geometryMode: 'measured' } as never);
    expect(useUIStore.getState().geometryMode).toBe('measured');

    // A saved closureAdjusted preference is restored ("remember last used").
    useUIStore.setState({ geometryMode: 'measured' });
    useUIStore.getState().applyDisplaySettings({ geometryMode: 'closureAdjusted' } as never);
    expect(useUIStore.getState().geometryMode).toBe('closureAdjusted');
  });

  it('geometryMode is NOT part of DISPLAY_DEFAULTS (telemetry registry contract)', () => {
    expect('geometryMode' in DISPLAY_DEFAULTS).toBe(false);
  });

  it('applyDisplaySettings resets the Debug submenu toggles so they never leak across project loads', () => {
    // The Debug submenu toggles (Dev/Memory/Map/Zoom Stats) are session-only and
    // absent from the backend payload; a project load must reset them, not inherit them.
    useUIStore.setState({ showDevStats: true, showMemoryStats: true, showMapStats: true, showZoomStats: true });
    useUIStore.getState().applyDisplaySettings({ geometryMode: 'measured' } as never);
    expect(useUIStore.getState().showDevStats).toBe(false);
    expect(useUIStore.getState().showMemoryStats).toBe(false);
    expect(useUIStore.getState().showMapStats).toBe(false);
    expect(useUIStore.getState().showZoomStats).toBe(false);
  });

  it('setView(global) resets the Debug submenu toggles so the overlays cannot strand over GlobalView', () => {
    // The Debug submenu lives only in SingleSystemView, so leaving for GlobalView
    // must clear its toggles - otherwise the app-mounted probe lingers with no way to hide.
    useUIStore.setState({
      view: 'single',
      showDevStats: true,
      showMemoryStats: true,
      showMapStats: true,
      showZoomStats: true,
    });
    useUIStore.getState().setView('global');
    expect(useUIStore.getState().showDevStats).toBe(false);
    expect(useUIStore.getState().showMemoryStats).toBe(false);
    expect(useUIStore.getState().showMapStats).toBe(false);
    expect(useUIStore.getState().showZoomStats).toBe(false);
    expect(useUIStore.getState().view).toBe('global');
  });
});

describe('Plan/Export display isolation', () => {
  beforeEach(() => {
    useUIStore.setState({ ...getCleanTestState(), appMode: 'survey' });
    vi.clearAllMocks();
  });

  it('Display changes in Plan update the (plan) view but are restored on return to Survey', () => {
    expect(useUIStore.getState().showGrid).toBe(false);
    useUIStore.getState().setAppMode('plan');
    // Toggling in Plan takes effect immediately (the plan view re-renders).
    useUIStore.getState().toggleDisplay('showGrid');
    expect(useUIStore.getState().showGrid).toBe(true);
    // Returning to Survey restores the pre-Plan survey value - Plan didn't touch it.
    useUIStore.getState().setAppMode('survey');
    expect(useUIStore.getState().showGrid).toBe(false);
  });

  it('Display changes in Plan do NOT persist to the saved (survey) config', () => {
    useUIStore.getState().setAppMode('plan');
    vi.clearAllMocks();
    useUIStore.getState().toggleDisplay('showGrid');
    useUIStore.getState().setGridSpacing(25);
    expect(invoke).not.toHaveBeenCalledWith(
      'save_display_setting',
      expect.objectContaining({ key: 'showGrid' }),
    );
    expect(invoke).not.toHaveBeenCalledWith(
      'save_display_setting',
      expect.objectContaining({ key: 'gridSpacing' }),
    );
  });

  it('entering Plan force-clears the color overlays, but Survey gets them back on exit', () => {
    // A survey depth gradient must not be silently lost by a round-trip through Plan.
    useUIStore.setState({ showDepthGradient: true });
    useUIStore.getState().setAppMode('plan');
    expect(useUIStore.getState().showDepthGradient).toBe(false); // gas gradient owns coloring in Plan
    useUIStore.getState().setAppMode('survey');
    expect(useUIStore.getState().showDepthGradient).toBe(true); // restored
  });

  it.each(['plan', 'export'] as const)(
    'entering %s clears every overlay (plus the distance cache) and forces the closure map',
    (mode) => {
      // The entry write goes through colorModeFlags('section') rather than
      // hand-listing the flags, so this pins the whole set it is responsible for -
      // including the distance origin/field cache and the ruler reset that share
      // the same set().
      useUIStore.setState({
        appMode: 'survey',
        geometryMode: 'measured',
        showDepthGradient: true,
        showDistanceGradient: true,
        showIssueSeverity: true,
        distanceGradientOrigin: { section_id: 1, station_index: 2 },
        distanceField: {} as never,
        measureStart: { kind: 'point', x: 1, y: 2, z: 3 },
      });
      useUIStore.getState().setAppMode(mode);
      const s = useUIStore.getState();
      expect(s.appMode).toBe(mode);
      expect(s.geometryMode).toBe('closureAdjusted');
      expect(colorModeOf(s)).toBe('section');
      expect(s.showDepthGradient).toBe(false);
      expect(s.showDistanceGradient).toBe(false);
      expect(s.showIssueSeverity).toBe(false);
      expect(s.distanceGradientOrigin).toBeNull();
      expect(s.distanceField).toBeNull();
      expect(s.measureStart).toBeNull();
    }
  );

  it('a Survey display change still persists (isolation is Plan/Export-only)', async () => {
    useUIStore.getState().toggleDisplay('showGrid');
    await vi.waitFor(() => {
      expect(invoke).toHaveBeenCalledWith('save_display_setting', {
        key: 'showGrid',
        value: 'true',
      });
    });
  });
});

describe('Data mode shares the Survey display', () => {
  beforeEach(() => {
    useUIStore.setState({ ...getCleanTestState(), appMode: 'survey' });
    vi.clearAllMocks();
  });

  it('Survey <-> Data keeps every display flag, the geometry and the colour mode', () => {
    useUIStore.setState({ showGrid: true, showDepthGradient: true, geometryMode: 'closureAdjusted' });
    useUIStore.getState().setAppMode('data');
    let s = useUIStore.getState();
    expect(s.appMode).toBe('data');
    expect(s.showGrid).toBe(true);
    expect(s.showDepthGradient).toBe(true);
    // Not forced, not cleared: Data is not a read-only mode.
    expect(s.geometryMode).toBe('closureAdjusted');
    useUIStore.getState().toggleDisplay('showStations');
    useUIStore.getState().setAppMode('survey');
    s = useUIStore.getState();
    // A Data change is a Survey change (one shared view), not snapshotted away.
    expect(s.showStations).toBe(!getCleanTestState().showStations);
    expect(s.showGrid).toBe(true);
    expect(s.geometryMode).toBe('closureAdjusted');
  });

  it('Display changes in Data persist like Survey ones', async () => {
    useUIStore.getState().setAppMode('data');
    useUIStore.getState().toggleDisplay('showGrid');
    useUIStore.getState().setGridSpacing(25);
    await vi.waitFor(() => {
      expect(invoke).toHaveBeenCalledWith('save_display_setting', { key: 'showGrid', value: 'true' });
      expect(invoke).toHaveBeenCalledWith('save_display_setting', { key: 'gridSpacing', value: '25' });
    });
  });

  it('Data -> Plan -> Data restores the Data view exactly as Survey would', () => {
    useUIStore.setState({ showDepthGradient: true, showGrid: false, geometryMode: 'measured' });
    useUIStore.getState().setAppMode('data');
    useUIStore.getState().setAppMode('plan');
    useUIStore.getState().toggleDisplay('showGrid');
    expect(useUIStore.getState().showDepthGradient).toBe(false);
    useUIStore.getState().setAppMode('data');
    const s = useUIStore.getState();
    expect(s.appMode).toBe('data');
    expect(s.geometryMode).toBe('measured');
    expect(s.showDepthGradient).toBe(true);
    expect(s.showGrid).toBe(false);
  });
});

describe('Placement mode forces stations visible (#518)', () => {
  const sectionData = { name: 'S', surveyor: null, date: null, color: '#fff', direction: 'in', quality: 1, shots: [] };

  beforeEach(() => {
    vi.clearAllMocks();
    useUIStore.setState({ showStations: false, placementMode: null });
  });

  it('turns stations on when a placement starts', () => {
    expect(useUIStore.getState().showStations).toBe(false);
    useUIStore.getState().startPlacementMode(sectionData as never, 0);
    // MapCanvas cancels an active placement whenever showStations is false, so a
    // placement that started with the toggle off died before the first click.
    expect(useUIStore.getState().showStations).toBe(true);
    expect(useUIStore.getState().placementMode).not.toBeNull();
  });

  it('does NOT persist the forcing to the saved display settings', async () => {
    useUIStore.getState().startPlacementMode(sectionData as never, 0);
    await new Promise((r) => setTimeout(r, 20));
    // A prerequisite of a transient mode is not a preference: `toggleDisplay`
    // would have written it through `saveDisplaySetting`.
    //
    // The 20 ms is a bound on the ABSENCE of a call, so it is best-effort by
    // nature: it catches a synchronous or microtask-scheduled write, which is
    // what `saveDisplaySetting` does. What makes the assertion trustworthy is
    // not the window but the code - `startPlacementMode` calls `set` directly
    // and has no IPC path at all.
    const wrote = (invoke as unknown as { mock: { calls: unknown[][] } }).mock.calls.some(
      ([cmd, args]) => cmd === 'save_display_setting' && (args as { key?: string })?.key === 'showStations',
    );
    expect(wrote).toBe(false);
  });

  it('moves a placement started in Data to Survey, where the map is', () => {
    useUIStore.setState({ appMode: 'data' });
    useUIStore.getState().startPlacementMode(sectionData as never, 0);
    expect(useUIStore.getState().appMode).toBe('survey');
    expect(useUIStore.getState().placementMode).not.toBeNull();
    useUIStore.getState().cancelPlacementMode();
  });

  it('leaves stations on when the placement ends, like every other mode exit', () => {
    useUIStore.getState().startPlacementMode(sectionData as never, 0);
    useUIStore.getState().cancelPlacementMode();
    // One-way, exactly as the gas exit and the measure clear are: re-hiding them
    // would take away the thing the user just placed against.
    expect(useUIStore.getState().showStations).toBe(true);
    expect(useUIStore.getState().placementMode).toBeNull();
  });
});
