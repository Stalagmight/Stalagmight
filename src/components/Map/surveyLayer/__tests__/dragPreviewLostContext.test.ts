import { describe, expect, it, vi } from 'vitest';
import type { Map as MaplibreMap } from 'maplibre-gl';
import { dropStyle } from '../customRender/__tests__/styleGone';

/**
 * A segment pick-up whose drag preview begins while the WebGL context is lost
 * (#576 NF-W4MAP-1). The segment carry is async - the click fetches the
 * connected sections, and `beginDragPreview` runs when that reply lands - so a
 * context loss inside that window reaches the preview's begin with no style.
 *
 * The fake map loses its style the way MapLibre 5.16 does: `getLayer` /
 * `addLayer` / `removeLayer` / `getSource` / `addSource` / `setPaintProperty`
 * read `this.style` and throw, and `isSourceLoaded` - `this.style &&
 * this.style.tileManagers[id]`, then `.loaded()` on the null it got - throws
 * too. (The real `addLayer` instead lazily builds an EMPTY style; the guard keeps
 * it from being called at all, which the test pins.)
 */
vi.mock('../customRender/surveyGhostLayer', () => ({
  SurveyGhostLayer: class {
    readonly type = 'custom' as const;
    constructor(readonly id: string) {}
    isDisabled(): boolean {
      return false;
    }
    getDisabledReason(): string | null {
      return null;
    }
    setOffset(): void {}
  },
}));

import { DragGhostController } from '../dragGhostController';
import { DragPreviewController, type DragPreviewHost } from '../dragPreviewController';
import type { AnalyticalOverlays } from '../analyticalOverlays';
import type { GhostRenderData } from '../customRender/surveyGhostLayer';
import type { SurveyGeometryIndex } from '../geometryIndex';
import type { SurveyData } from '../../../../types/survey-data';
import type { Section } from '../../../../types/project';

function lostContextMap() {
  const calls: string[] = [];
  const map = {
    style: {},
    getFilter: () => undefined,
    setFilter: () => calls.push('setFilter'),
    triggerRepaint: () => {},
    on: () => {},
    off: () => {},
  } as unknown as MaplibreMap;
  dropStyle(map);
  const m = map as unknown as Record<string, unknown>;
  for (const name of ['addLayer', 'getLayer', 'removeLayer', 'getSource', 'addSource', 'removeSource', 'setPaintProperty']) {
    m[name] = () => {
      calls.push(name);
      throw new TypeError(`Cannot read properties of null (reading '${name}')`);
    };
  }
  m.isSourceLoaded = (id: string) => {
    calls.push('isSourceLoaded');
    const style = m.style as { tileManagers: Record<string, { loaded(): boolean }> } | null;
    const tileManager = style && style.tileManagers[id];
    if (tileManager === undefined) return undefined;
    return (tileManager as { loaded(): boolean }).loaded();
  };
  return { map, calls };
}

function host(): DragPreviewHost {
  return {
    geoIndex: () =>
      ({
        sectionGeometry: () => ({ stations: [], shots: [] }),
        stationCoords: () => null,
      }) as unknown as SurveyGeometryIndex,
    surveyData: () => ({}) as SurveyData,
    sections: () => [{ id: 1, color: '#ffffff' } as unknown as Section],
    loopClosures: () => [],
    gpsOrigin: () => null,
    displayColorContext: () => ({}) as never,
    themeColors: () => ({ stationDefault: '#ffffff' }) as never,
    parseFeatureId: () => null,
    customRender: () => null,
    customLabels: () => null,
    refreshInteractOverlay: vi.fn(),
    invalidateInteractOverlay: vi.fn(),
  };
}

describe('a segment drag preview begun while the WebGL context is lost (#576 NF-W4MAP-1)', () => {
  it('begins, follows and ends without reading the missing style', () => {
    const { map, calls } = lostContextMap();
    const ghost = new DragGhostController(map, {
      ghostData: () => ({}) as GhostRenderData,
      layerCtx: () => ({ colorExpr: ['get', 'color'], shotWidth: 3, stationRadius: 5, selectHighlight: '#fff' }),
      projector: () => ({ project: ([x, y]) => ({ x, y }), unproject: ([x, y]) => ({ lng: x, lat: y }) }),
      dpr: () => 1,
      scheduleRestore: (fn) => fn(),
    });
    const preview = new DragPreviewController(map, ghost, { distanceMode: false, gasReachMode: false } as unknown as AnalyticalOverlays, host());

    expect(() => preview.beginDragPreview(new Set([1]))).not.toThrow();
    expect(preview.dragPreviewActive).toBe(true);
    expect(ghost.isCustom()).toBe(false);
    expect(() => preview.endDragPreview()).not.toThrow();
    // Not one style-backed call, the lazy-init addLayer included.
    expect(calls).toEqual([]);
  });
});
