/**
 * Stacking-order guard for the coordinate grid (#461).
 *
 * The grid shipped anchored on `survey-shots`, a stock layer the custom WebGL
 * renderer replaced. `getLayer` never resolved it, so `beforeId` was always
 * undefined and MapLibre appended the grid as the topmost layer, drawing it
 * over the survey. What matters is not that the layer is added but WHERE, so
 * every case below asserts the second argument handed to `addLayer`.
 *
 * The fake map is enough: GridOverlay only ever asks it whether a layer or
 * source exists, and it exercises the real component (effect, store reads and
 * all) rather than a re-implementation of the anchor choice.
 *
 * The satellite raster shares that stack and is pinned here too, at the list
 * level: MapCanvas needs a real map to reach its effect, but the ordering that
 * decides where the raster lands is the candidate list itself.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from '@testing-library/react';
import type * as maplibregl from 'maplibre-gl';

// maplibre-gl touches `window.URL.createObjectURL` at module init (worker
// setup), which jsdom lacks. GridOverlay imports it for types only.
vi.mock('maplibre-gl', () => ({ default: {} }));

import GridOverlay from '../GridOverlay';
import { BOTTOM_SURVEY_LAYERS, SATELLITE_BOTTOM_CANDIDATES } from '../mapUtils';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import type { Station } from '../../../types/project';

function makeStation(station_index: number, x: number, y: number): Station {
  return {
    station_ref: { section_id: 1, station_index },
    coordinates: { x, y, z: 0 },
    is_zero_datum: false,
    gps_coordinates: null,
    merged_with: [],
    closures_with: [],
  };
}

interface FakeMap {
  map: maplibregl.Map;
  addLayer: ReturnType<typeof vi.fn>;
}

function makeMap(existingLayers: string[]): FakeMap {
  const layers = new Set(existingLayers);
  const sources = new Set<string>();
  const addLayer = vi.fn((layer: { id: string }, _beforeId?: string) => {
    layers.add(layer.id);
  });
  const map = {
    // hasLiveStyle() reads this on cleanup
    style: {},
    getLayer: (id: string) => (layers.has(id) ? { id } : undefined),
    addLayer,
    removeLayer: vi.fn((id: string) => layers.delete(id)),
    getSource: (id: string) => (sources.has(id) ? { setData: vi.fn() } : undefined),
    addSource: vi.fn((id: string) => sources.add(id)),
    removeSource: vi.fn((id: string) => sources.delete(id)),
    setPaintProperty: vi.fn(),
  };
  return { map: map as unknown as maplibregl.Map, addLayer };
}

/** The anchor `addLayer` was called with for the grid. */
function gridAnchor(addLayer: ReturnType<typeof vi.fn>): string | undefined {
  const call = addLayer.mock.calls.find(([layer]) => (layer as { id: string }).id === 'grid-lines');
  expect(call).toBeDefined();
  return (call as unknown[])[1] as string | undefined;
}

let priorDisplay: { showGrid: boolean; gridSpacing: number };

beforeEach(() => {
  const { showGrid, gridSpacing } = useUIStore.getState();
  priorDisplay = { showGrid, gridSpacing };
  // Stations only decide whether the grid has lines to draw; the layer is added
  // either way. Seed them so the exercised path is the one users see.
  useProjectStore.setState({ stations: [makeStation(0, 0, 0), makeStation(1, 40, 25)] });
  useUIStore.setState({ showGrid: true, gridSpacing: 10 });
});

afterEach(() => {
  useProjectStore.setState({ stations: [] });
  useUIStore.setState(priorDisplay);
});

describe('GridOverlay layer anchoring', () => {
  it('goes below the bottom-most survey layer, not on top of the survey', () => {
    // initializeLayers adds the ghost shot hitbox first, so anchoring there puts
    // the grid below the ghosts, the loop closures and the WebGL geometry alike.
    const { map, addLayer } = makeMap([
      'survey-ghost-shots-hitbox',
      'survey-ghost-stations-hitbox',
      'survey-ghost-shots',
      'survey-loop-closures',
      'survey-custom-render',
      'survey-interact-shots',
    ]);
    render(<GridOverlay map={map} gpsOrigin={null} />);
    expect(gridAnchor(addLayer)).toBe('survey-ghost-shots-hitbox');
  });

  it('falls back through the candidates when the ghost hitbox is absent', () => {
    const { map, addLayer } = makeMap(['survey-ghost-shots', 'survey-custom-render']);
    render(<GridOverlay map={map} gpsOrigin={null} />);
    expect(gridAnchor(addLayer)).toBe('survey-ghost-shots');
  });

  it('still lands under the geometry when only the custom render layer is there', () => {
    const { map, addLayer } = makeMap(['survey-custom-render', 'survey-interact-shots']);
    render(<GridOverlay map={map} gpsOrigin={null} />);
    expect(gridAnchor(addLayer)).toBe('survey-custom-render');
  });

  it('takes no anchor when no survey layer exists yet', () => {
    // Survey layers added afterwards are appended above the grid, so an absent
    // anchor is self-correcting - naming a layer that is not there would throw.
    const { map, addLayer } = makeMap([]);
    render(<GridOverlay map={map} gpsOrigin={null} />);
    expect(gridAnchor(addLayer)).toBeUndefined();
  });

  it('ignores a survey-shots layer even when one exists', () => {
    // The stale anchor is not merely dead, it is wrong: a style that happens to
    // carry a `survey-shots` id (a user style, a future re-add) must not pull
    // the grid off the bottom of the stack.
    const { map, addLayer } = makeMap([
      'survey-shots',
      'survey-ghost-shots-hitbox',
      'survey-ghost-shots',
      'survey-custom-render',
    ]);
    render(<GridOverlay map={map} gpsOrigin={null} />);
    expect(gridAnchor(addLayer)).toBe('survey-ghost-shots-hitbox');
  });
});

describe('satellite anchor candidates', () => {
  it('names the grid first, so the raster cannot cover it', () => {
    // Anchoring satellite on the same survey layer the grid uses would insert it
    // between grid and survey, hiding the grid under 0.7-opacity imagery.
    expect(SATELLITE_BOTTOM_CANDIDATES[0]).toBe('grid-lines');
  });

  it('falls through the same survey layers as the grid', () => {
    // One ordering, two overlays: the grid-off case must land satellite exactly
    // where the grid would have gone.
    expect(SATELLITE_BOTTOM_CANDIDATES.slice(1)).toEqual([...BOTTOM_SURVEY_LAYERS]);
  });

  it('names no layer production code stopped creating', () => {
    expect([...SATELLITE_BOTTOM_CANDIDATES]).not.toContain('survey-shots');
  });
});
