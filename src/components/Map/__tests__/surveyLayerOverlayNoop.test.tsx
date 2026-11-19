/**
 * MapCanvas re-applies the distance-gradient and gas-reach overlays on every
 * survey refresh (its effects depend on the survey data), and each apply
 * rebuilds every label. Switching an overlay off while it is already off must
 * do nothing, or every undo, redo and edit on a large survey pays for two
 * full label rebuilds of overlays that are not shown. `Object.create` skips
 * the heavy constructor, as in surveyLayerRaise.test.tsx.
 */
import { describe, expect, it, vi } from 'vitest';

// maplibre-gl runs `window.URL.createObjectURL` at module-init, which jsdom
// lacks; nothing here touches maplibre.
vi.mock('maplibre-gl', () => ({ default: {} }));

import SurveyLayer from '../SurveyLayer';

interface Probe {
  setDistanceGradient: (enabled: boolean, state: unknown) => void;
  setGasReach: (enabled: boolean, state: unknown) => void;
}

function makeLayer() {
  const resync = vi.fn();
  const paint = vi.fn();
  const layer = Object.create(SurveyLayer.prototype) as Probe;
  Object.assign(layer, {
    map: { style: {} },
    distanceMode: false,
    distanceState: null,
    gasReachMode: false,
    gasReachState: null,
    applyOverlayPaint: paint,
    resyncCustomRenderFromStore: resync,
    syncCustomDimming: vi.fn(),
  });
  return { layer, resync, paint };
}

describe('SurveyLayer overlays switched off while off', () => {
  it('the distance gradient rebuilds nothing, but every real change still applies', () => {
    const { layer, resync, paint } = makeLayer();
    layer.setDistanceGradient(false, null);
    expect(resync).not.toHaveBeenCalled();
    expect(paint).not.toHaveBeenCalled();
    const state = { dist: new Map() };
    layer.setDistanceGradient(true, state);
    expect(resync).toHaveBeenCalledTimes(1);
    layer.setDistanceGradient(true, { dist: new Map() }); // re-rooted: applies again
    expect(resync).toHaveBeenCalledTimes(2);
    layer.setDistanceGradient(false, null); // on -> off: applies
    expect(resync).toHaveBeenCalledTimes(3);
    layer.setDistanceGradient(false, null); // off -> off: nothing
    expect(resync).toHaveBeenCalledTimes(3);
    layer.setDistanceGradient(false, { dist: new Map() }); // off, but a new state: applies
    expect(resync).toHaveBeenCalledTimes(4);
  });

  it('the gas reach rebuilds nothing, but every real change still applies', () => {
    const { layer, resync } = makeLayer();
    layer.setGasReach(false, null);
    expect(resync).not.toHaveBeenCalled();
    layer.setGasReach(true, { reach: new Map() });
    expect(resync).toHaveBeenCalledTimes(1);
    layer.setGasReach(false, null);
    expect(resync).toHaveBeenCalledTimes(2);
    layer.setGasReach(false, null);
    expect(resync).toHaveBeenCalledTimes(2);
  });

  it('a style restore re-applies the interact paint of an overlay switched off while the style was gone', () => {
    const { layer } = makeLayer();
    const interactPaint = vi.fn();
    const map: { style?: object | undefined } = { style: undefined };
    Object.assign(layer, {
      map,
      applyInteractPaint: interactPaint,
      refreshInteractOverlay: vi.fn(),
      syncCustomMeasurePath: vi.fn(),
      deferredUpdate: null,
      customRender: null,
      measurePath: null,
      customLabels: null,
      dragGhost: { reattach: vi.fn() },
    });
    // On, then off, both while the style is gone: neither reaches the paint.
    layer.setDistanceGradient(true, { dist: new Map() });
    layer.setDistanceGradient(false, null);
    expect(interactPaint).not.toHaveBeenCalled();
    // The next survey refresh switches it off again, which is now a no-op, so
    // the restore is what puts the plain colours back on hover and selection.
    map.style = {};
    (layer as unknown as { reattachCustomLayers: () => void }).reattachCustomLayers();
    expect(interactPaint).toHaveBeenCalledTimes(1);
  });
});
