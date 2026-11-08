import { describe, expect, it } from 'vitest';
import type { Map as MaplibreMap } from 'maplibre-gl';
import {
  addCustomLayer,
  isStyleRestoring,
  onStyleRestored,
  reAddCustomLayer,
  removeCustomLayer,
  type RestorableCustomLayer,
} from '../styleRestore';

type Listener = () => void;

/** Just enough of MapLibre's Evented: on / once / off / fire. */
function makeEventedMap(): { map: MaplibreMap; fire: (type: string) => void; count: (type: string) => number } {
  const listeners: Array<{ type: string; fn: Listener; once: boolean }> = [];
  const map = {
    on: (type: string, fn: Listener) => listeners.push({ type, fn, once: false }),
    once: (type: string, fn: Listener) => listeners.push({ type, fn, once: true }),
    off: (type: string, fn: Listener) => {
      const i = listeners.findIndex((l) => l.type === type && l.fn === fn);
      if (i >= 0) listeners.splice(i, 1);
    },
  } as unknown as MaplibreMap;
  const fire = (type: string): void => {
    for (const l of listeners.filter((x) => x.type === type)) {
      if (l.once) listeners.splice(listeners.indexOf(l), 1);
      l.fn();
    }
  };
  const count = (type: string): number => listeners.filter((l) => l.type === type).length;
  return { map, fire, count };
}

describe('onStyleRestored', () => {
  it('reattaches on the style.load that follows a restore, never before or on a plain load', () => {
    const { map, fire } = makeEventedMap();
    let calls = 0;
    onStyleRestored(map, () => calls++);
    fire('style.load');
    expect(calls).toBe(0);
    fire('webglcontextrestored');
    expect(calls).toBe(0);
    fire('style.load');
    expect(calls).toBe(1);
    fire('style.load');
    expect(calls).toBe(1);
  });

  it('two restores before the style loads reattach once', () => {
    const { map, fire } = makeEventedMap();
    let calls = 0;
    onStyleRestored(map, () => calls++);
    fire('webglcontextrestored');
    fire('webglcontextrestored');
    fire('style.load');
    expect(calls).toBe(1);
  });

  it('dispose drops a pending reattach and every later one', () => {
    const { map, fire } = makeEventedMap();
    let calls = 0;
    const dispose = onStyleRestored(map, () => calls++);
    fire('webglcontextrestored');
    dispose();
    fire('style.load');
    fire('webglcontextrestored');
    fire('style.load');
    expect(calls).toBe(0);
  });

  it('subscribers on one map share one set of map listeners', () => {
    const { map, count } = makeEventedMap();
    onStyleRestored(map, () => {});
    onStyleRestored(map, () => {});
    expect(count('webglcontextlost')).toBe(1);
    expect(count('webglcontextrestored')).toBe(1);
  });
});

describe('isStyleRestoring', () => {
  it('covers the loss, the restore and the frame until the rebuilt style loads', () => {
    const { map, fire } = makeEventedMap();
    let restoringAtReattach: boolean | null = null;
    onStyleRestored(map, () => (restoringAtReattach = isStyleRestoring(map)));
    expect(isStyleRestoring(map)).toBe(false);
    fire('webglcontextlost');
    expect(isStyleRestoring(map)).toBe(true);
    fire('webglcontextrestored');
    expect(isStyleRestoring(map)).toBe(true);
    fire('style.load');
    expect(isStyleRestoring(map)).toBe(false);
    expect(restoringAtReattach).toBe(false);
  });

  it('is true for a lost context MapLibre has not reported yet', () => {
    const map = { painter: { context: { gl: { isContextLost: () => true } } } } as unknown as MaplibreMap;
    expect(isStyleRestoring(map)).toBe(true);
  });
});

describe('addCustomLayer / removeCustomLayer', () => {
  function setup(inStyle: boolean) {
    const log: string[] = [];
    const { map: evented, fire } = makeEventedMap();
    const map = Object.assign(evented, {
      painter: { context: { gl: {} } },
      getLayer: (id: string) => (inStyle && id === 'custom' ? { id } : undefined),
      addLayer: () => log.push('addLayer'),
      removeLayer: () => log.push('removeLayer'),
    });
    const layer = { id: 'custom', type: 'custom' as const, render: () => {}, onRemove: () => log.push('onRemove') };
    onStyleRestored(map, () => {});
    return { map, layer, log, fire };
  }

  it('holds a layer back while a restore is under way', () => {
    const { map, layer, log, fire } = setup(false);
    fire('webglcontextlost');
    addCustomLayer(map, layer);
    expect(log).toEqual([]);
    fire('webglcontextrestored');
    fire('style.load');
    addCustomLayer(map, layer);
    expect(log).toEqual(['addLayer']);
  });

  it('removes a layer the style holds', () => {
    const { map, layer, log } = setup(true);
    removeCustomLayer(map, layer);
    expect(log).toEqual(['removeLayer']);
  });

  it('frees the GL resources of a layer a rebuild already dropped', () => {
    const { map, layer, log } = setup(false);
    removeCustomLayer(map, layer);
    expect(log).toEqual(['onRemove']);
  });
});

describe('reAddCustomLayer', () => {
  function setup(present: boolean, disabledAfterAdd = false) {
    const log: string[] = [];
    let disabled = false;
    const layer: RestorableCustomLayer = {
      id: 'custom',
      type: 'custom',
      render: () => {},
      onAdd: () => {
        log.push('onAdd');
        disabled = disabledAfterAdd;
      },
      onRemove: () => log.push('onRemove'),
      isDisabled: () => disabled,
    };
    const map = {
      painter: { context: { gl: {} } },
      getLayer: (id: string) => (present && id === 'custom' ? { id } : undefined),
      addLayer: (l: RestorableCustomLayer, beforeId?: string) => {
        log.push(`addLayer:${beforeId ?? 'top'}`);
        l.onAdd?.(map as unknown as MaplibreMap, {} as WebGL2RenderingContext);
      },
    };
    return { map: map as unknown as MaplibreMap, layer, log };
  }

  it('is a no-op while the style still holds the layer', () => {
    const { map, layer, log } = setup(true);
    expect(reAddCustomLayer(map, layer, 'anchor')).toBe(true);
    expect(log).toEqual([]);
  });

  it('frees the old GL resources before adding the layer back at its anchor', () => {
    const { map, layer, log } = setup(false);
    expect(reAddCustomLayer(map, layer, 'anchor')).toBe(true);
    expect(log).toEqual(['onRemove', 'addLayer:anchor', 'onAdd']);
  });

  it('reports a layer that cannot start again', () => {
    const { map, layer } = setup(false, true);
    expect(reAddCustomLayer(map, layer)).toBe(false);
  });
});
