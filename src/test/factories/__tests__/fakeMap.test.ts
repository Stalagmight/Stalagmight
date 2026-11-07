/**
 * Pins the recording behaviour of the shared fake map (#576 E15) that the
 * controller tests assert through.
 */
import { describe, expect, it, vi } from 'vitest';
import { makeFakeMap } from '../fakeMap';

describe('makeFakeMap', () => {
  it('records the layer lifecycle and keeps the live layer set', () => {
    const f = makeFakeMap({ layers: ['stock'] });
    f.map.addLayer({ id: 'a', type: 'line', source: 's', paint: { 'line-width': 2 } }, 'stock');
    f.map.removeLayer('stock');
    expect(f.added).toEqual(['a']);
    expect(f.addCalls).toEqual([{ id: 'a', beforeId: 'stock' }]);
    expect(f.removed).toEqual(['stock']);
    expect([...f.layers]).toEqual(['a']);
    expect(f.map.getLayer('a')).toEqual({ id: 'a' });
    expect(f.map.getLayer('stock')).toBeUndefined();
    f.map.setPaintProperty('a', 'line-width', 5);
    expect(f.paint.get('a')).toEqual({ 'line-width': 5 });
    expect(f.paintCalls).toEqual([{ layer: 'a', prop: 'line-width', value: 5 }]);
  });

  it('seeds visibility (a seeded layer is on the map) and records visibility writes only', () => {
    const f = makeFakeMap({ visibility: { 'survey-shots': 'visible' } });
    expect(f.map.getLayer('survey-shots')).toBeDefined();
    f.map.setLayoutProperty('survey-shots', 'visibility', 'none');
    f.map.setLayoutProperty('survey-shots', 'line-cap', 'round');
    expect(f.visibility).toEqual({ 'survey-shots': 'none' });
    expect(f.map.getLayoutProperty('survey-shots', 'visibility')).toBe('none');
  });

  it("runs a custom layer's onAdd as asked", () => {
    const onAdd = vi.fn();
    const layer = { id: 'c', type: 'custom' as const, render: () => {}, onAdd };
    const withMap = makeFakeMap();
    withMap.map.addLayer(layer);
    expect(onAdd).toHaveBeenLastCalledWith(withMap.map, {});
    makeFakeMap({ onAdd: 'bare' }).map.addLayer(layer);
    expect(onAdd).toHaveBeenLastCalledWith(undefined, undefined);
    makeFakeMap({ onAdd: 'skip' }).map.addLayer(layer);
    expect(onAdd).toHaveBeenCalledTimes(2);
  });

  it('counts setData and repaints, and keeps one entry per registered listener', () => {
    const f = makeFakeMap();
    f.map.addSource('src', { type: 'geojson', data: { type: 'FeatureCollection', features: [] } });
    (f.map.getSource('src') as unknown as { setData: () => void }).setData();
    f.map.triggerRepaint();
    expect(f.setDataCount).toBe(1);
    expect(f.repaintCount).toBe(1);
    const fn = vi.fn();
    f.map.on('sourcedata', fn);
    f.map.on('sourcedata', fn);
    expect(f.listenerCount('sourcedata')).toBe(1);
    f.emit('sourcedata', { sourceId: 'src' });
    expect(fn).toHaveBeenCalledWith({ sourceId: 'src' });
    f.map.off('sourcedata', fn);
    expect(f.listeners('sourcedata')).toEqual([]);
  });

  it('leaves out the members it is told to omit, and has a live style otherwise', () => {
    expect((makeFakeMap().map as unknown as { style: unknown }).style).toEqual({});
    const bare = makeFakeMap({ omit: ['style', 'on', 'off'] }).map as unknown as Record<string, unknown>;
    expect('style' in bare).toBe(false);
    expect(bare.on).toBeUndefined();
    expect(bare.off).toBeUndefined();
  });
});
