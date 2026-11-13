import { describe, it, expect, beforeEach } from 'vitest';
import { useUIStore } from '../uiStore';
import { useGasPlanStore } from '../gasPlanStore';

/**
 * The Plan workspace mode (#38) paints the same shots/stations layer as the three
 * color overlays (depth / distance / issue-severity), so they must be mutually
 * exclusive. Entering Plan clears the color overlays; enabling a color overlay -
 * reachable via the Shift+D keyboard shortcut, which calls toggleDisplay directly,
 * bypassing setColorMode - returns to Survey (exitGasPlan). These guards pin BOTH
 * directions.
 */
describe('Plan mode ↔ color-overlay mutual exclusion (#38)', () => {
  beforeEach(() => {
    useUIStore.setState({
      appMode: 'survey',
      geometryMode: 'measured',
      showDepthGradient: false,
      showDistanceGradient: false,
      showIssueSeverity: false,
      distanceGradientOrigin: null,
      distanceField: null,
    });
    useGasPlanStore.getState().reset();
  });

  it('entering Plan clears all three color overlays and forces the read-only closure map', () => {
    useUIStore.setState({ showDepthGradient: true });
    useUIStore.getState().setAppMode('plan');
    const s = useUIStore.getState();
    expect(s.appMode).toBe('plan');
    expect(s.geometryMode).toBe('closureAdjusted');
    expect(s.showDepthGradient).toBe(false);
    expect(s.showDistanceGradient).toBe(false);
    expect(s.showIssueSeverity).toBe(false);
  });

  it.each([
    ['depth via toggleDisplay (the Shift+D path)', () => useUIStore.getState().toggleDisplay('showDepthGradient')],
    ['distance via toggleDistanceGradient', () => useUIStore.getState().toggleDistanceGradient()],
    ['issue-severity via toggleIssueSeverity', () => useUIStore.getState().toggleIssueSeverity()],
    ['issue-severity via setIssueSeverity(true)', () => useUIStore.getState().setIssueSeverity(true)],
    ['distance via setColorMode (the DisplayMenu path)', () => useUIStore.getState().setColorMode('distance')],
    ['depth via setColorMode', () => useUIStore.getState().setColorMode('depth')],
  ])('enabling a color overlay (%s) returns to Survey', (_name, enable) => {
    useUIStore.getState().setAppMode('plan');
    expect(useUIStore.getState().appMode).toBe('plan');
    enable();
    expect(useUIStore.getState().appMode).toBe('survey');
  });

  it('setColorMode("section") while in Plan returns to Survey (no overlay selected)', () => {
    // 'section' = no color overlay; Plan still owns the layer paint, so picking it must
    // leave Plan. Guards the setColorMode early-return path (colorModeOf is already
    // 'section' in Plan, so the exit must run BEFORE the no-op check).
    useUIStore.getState().setAppMode('plan');
    expect(useUIStore.getState().appMode).toBe('plan');
    useUIStore.getState().setColorMode('section');
    expect(useUIStore.getState().appMode).toBe('survey');
  });

  it('returning to Survey restores the prior geometry mode and KEEPS the gas plan (#244)', () => {
    // Enter Plan from a closure-adjusted survey view: the forced closureAdjusted is the
    // same value, but on exit the saved survey value must come back (here: closureAdjusted).
    useUIStore.setState({ geometryMode: 'closureAdjusted' });
    useUIStore.getState().setAppMode('plan');
    useGasPlanStore.getState().setOrigin({ section_id: 2, station_index: 1 });
    useUIStore.getState().setAppMode('survey');
    expect(useUIStore.getState().appMode).toBe('survey');
    expect(useUIStore.getState().geometryMode).toBe('closureAdjusted');
    // The gas plan PERSISTS across mode switches now (#244); it is emptied only on a project
    // load (applyDisplaySettings), not on leaving Plan. (Was previously reset here.)
    expect(useGasPlanStore.getState().origin).toEqual({ section_id: 2, station_index: 1 });
  });

  it('exitGasPlan returns to Survey from Plan and from Export', () => {
    useUIStore.getState().setAppMode('plan');
    useUIStore.getState().exitGasPlan();
    expect(useUIStore.getState().appMode).toBe('survey');

    useUIStore.getState().setAppMode('export');
    expect(useUIStore.getState().appMode).toBe('export');
    useUIStore.getState().exitGasPlan();
    expect(useUIStore.getState().appMode).toBe('survey');
  });

  it.each([
    ['depth via toggleDisplay (the Shift+D path)', () => useUIStore.getState().toggleDisplay('showDepthGradient')],
    ['distance via toggleDistanceGradient', () => useUIStore.getState().toggleDistanceGradient()],
    ['issue-severity via toggleIssueSeverity', () => useUIStore.getState().toggleIssueSeverity()],
    ['issue-severity via setIssueSeverity(true)', () => useUIStore.getState().setIssueSeverity(true)],
    ['section via setColorMode', () => useUIStore.getState().setColorMode('section')],
    ['depth via setColorMode', () => useUIStore.getState().setColorMode('depth')],
  ])('enabling a color overlay (%s) in Data stays in Data (exitGasPlan is a no-op there)', (_name, enable) => {
    useUIStore.getState().setAppMode('data');
    enable();
    expect(useUIStore.getState().appMode).toBe('data');
  });

  it('exitGasPlan is a no-op in Data and returns a Plan entered from Data to Data', () => {
    useUIStore.getState().setAppMode('data');
    useUIStore.getState().exitGasPlan();
    expect(useUIStore.getState().appMode).toBe('data');
    // A read-only mode entered from Data goes back where the user came from.
    useUIStore.getState().setAppMode('plan');
    useUIStore.getState().toggleDisplay('showDepthGradient');
    expect(useUIStore.getState().appMode).toBe('data');
    expect(useUIStore.getState().showDepthGradient).toBe(true);
  });

  it('restores measured geometry after Plan on the default (measured) survey view', () => {
    // The realistic default path: forced closureAdjusted ≠ saved measured, so this
    // distinguishes "restored the saved value" from "left the forced value" (the
    // closureAdjusted-entry case can't tell those apart - they're equal).
    expect(useUIStore.getState().geometryMode).toBe('measured');
    useUIStore.getState().setAppMode('plan');
    expect(useUIStore.getState().geometryMode).toBe('closureAdjusted');
    useUIStore.getState().setAppMode('survey');
    expect(useUIStore.getState().geometryMode).toBe('measured');
  });

  it('a project load is authoritative: drops Plan mode without restoring stale geometry', () => {
    // Project A in Plan (measured survey view) captures prePlanGeometryMode='measured'.
    // Loading project B whose saved geometry is closureAdjusted: applyDisplaySettings must
    // reset appMode to survey and clear the stale capture, so the later exitGasPlan funnel
    // can't restore A's 'measured' over B's 'closureAdjusted'.
    useUIStore.getState().setAppMode('plan');
    useUIStore.getState().applyDisplaySettings({ geometryMode: 'closureAdjusted' } as never);
    expect(useUIStore.getState().appMode).toBe('survey');
    expect(useUIStore.getState().geometryMode).toBe('closureAdjusted');
    // The leftover funnel is now a no-op (already survey) - geometry stays B's.
    useUIStore.getState().exitGasPlan();
    expect(useUIStore.getState().geometryMode).toBe('closureAdjusted');
  });

  it('a project load resets the gas store (no pushyAck / plan-input leak across projects)', () => {
    // Project A: in Plan, the user accepted the advanced staged-reserve presets and picked an
    // origin. A DIRECT switch to project B sets appMode='survey' in applyDisplaySettings, which
    // would short-circuit the later exitGasPlan funnel - so applyDisplaySettings must reset the
    // gas store itself, or the safety ack (and inputs) leak into B's Plan session.
    useUIStore.getState().setAppMode('plan');
    useGasPlanStore.getState().setPushyAck(true);
    useGasPlanStore.getState().setOrigin({ section_id: 9, station_index: 3 });
    useUIStore.getState().applyDisplaySettings({ geometryMode: 'measured' } as never);
    expect(useGasPlanStore.getState().pushyAck).toBe(false);
    expect(useGasPlanStore.getState().origin).toBeNull();
  });
});
