/**
 * The coordinate grid in the frame before a rebuilt style loads (#576
 * RV18-MAP-1). A WebGL context restore or a worker restart puts a new, empty
 * style in place that loads one frame later; a grid effect that ran then (the
 * grid toggled, or new stations moved its lines) found no grid source, tried to
 * add one, and MapLibre threw "Style is not done loading." out of the effect.
 * It now skips, and `styleEpoch` re-runs it once the style has loaded.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from '@testing-library/react';
import type * as maplibregl from 'maplibre-gl';

vi.mock('maplibre-gl', () => ({ default: {} }));

import GridOverlay from '../GridOverlay';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import { makeStation as stationOf } from '../../../test/factories/survey';

/** A map whose style is in place but has not loaded: empty, and every write throws. */
function loadingMap() {
  const style = { _loaded: false };
  const layers = new Set<string>();
  const sources = new Set<string>();
  const checkLoaded = (): void => {
    if (!style._loaded) throw new Error('Style is not done loading.');
  };
  const map = {
    style,
    getLayer: (id: string) => (layers.has(id) ? { id } : undefined),
    getSource: (id: string) => (sources.has(id) ? { setData: vi.fn() } : undefined),
    addSource: vi.fn((id: string) => {
      checkLoaded();
      sources.add(id);
    }),
    addLayer: vi.fn((layer: { id: string }) => {
      checkLoaded();
      layers.add(layer.id);
    }),
    removeLayer: vi.fn((id: string) => {
      checkLoaded();
      layers.delete(id);
    }),
    removeSource: vi.fn((id: string) => {
      checkLoaded();
      sources.delete(id);
    }),
    setPaintProperty: vi.fn(() => checkLoaded()),
  };
  return { map: map as unknown as maplibregl.Map, raw: map, load: () => (style._loaded = true), layers };
}

let priorShowGrid: boolean;

beforeEach(() => {
  priorShowGrid = useUIStore.getState().showGrid;
  useProjectStore.setState({
    stations: [
      stationOf({ station_ref: { section_id: 1, station_index: 0 }, coordinates: { x: 0, y: 0, z: 0 }, closures_with: [] }),
      stationOf({ station_ref: { section_id: 1, station_index: 1 }, coordinates: { x: 40, y: 25, z: 0 }, closures_with: [] }),
    ],
  });
  useUIStore.setState({ showGrid: true, gridSpacing: 10 });
});

afterEach(() => {
  useProjectStore.setState({ stations: [] });
  useUIStore.setState({ showGrid: priorShowGrid });
});

describe('GridOverlay in the frame before a rebuilt style loads (#576 RV18-MAP-1)', () => {
  it('does not throw, and adds the grid once the style has loaded', () => {
    const { map, raw, load, layers } = loadingMap();
    let thrown: string | null = null;
    let view: ReturnType<typeof render> | null = null;
    try {
      view = render(<GridOverlay map={map} gpsOrigin={null} styleEpoch={0} />);
    } catch (e) {
      thrown = (e as Error).message;
    }
    console.log('[RV18-MAP-1] grid effect in the loading frame threw:', thrown);
    expect(thrown).toBeNull();
    expect(raw.addSource).not.toHaveBeenCalled();

    // style.load: useStyleEpoch moves and the effect runs again on the loaded style.
    load();
    view!.rerender(<GridOverlay map={map} gpsOrigin={null} styleEpoch={1} />);
    expect(layers.has('grid-lines')).toBe(true);
  });
});
