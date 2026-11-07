/**
 * `canWriteStyle` (#576 RV18-MAP-1): a stock-style write lands only on a style
 * that is alive, loaded, and not mid-rebuild. In the frame between a restore and
 * the rebuilt style's `style.load`, MapLibre 6 has a style that refuses every
 * write ("Style is not done loading."), which `hasLiveStyle` alone let through.
 */
import { describe, expect, it } from 'vitest';
import type { Map as MaplibreMap } from 'maplibre-gl';
import { canWriteStyle, hasLiveStyle } from '../mapUtils';
import { onStyleRestored } from '../surveyLayer/customRender/styleRestore';

type Listener = () => void;

function evented(style: unknown): { map: MaplibreMap; fire: (type: string) => void } {
  const listeners: Array<{ type: string; fn: Listener; once: boolean }> = [];
  const map = {
    style,
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
  return { map, fire };
}

describe('canWriteStyle', () => {
  it('is false without a map or a style', () => {
    expect(canWriteStyle(null)).toBe(false);
    expect(canWriteStyle(evented(null).map)).toBe(false);
    expect(canWriteStyle(evented(undefined).map)).toBe(false);
  });

  it('is false for a style that has not loaded, where hasLiveStyle is true', () => {
    const { map } = evented({ _loaded: false });
    expect(hasLiveStyle(map)).toBe(true);
    expect(canWriteStyle(map)).toBe(false);
  });

  it('is true for a loaded style, and for a stand-in without the flag', () => {
    expect(canWriteStyle(evented({ _loaded: true }).map)).toBe(true);
    expect(canWriteStyle(evented({}).map)).toBe(true);
  });

  it('is false from the loss until the rebuilt style has loaded', () => {
    const style = { _loaded: true };
    const { map, fire } = evented(style);
    onStyleRestored(map, () => {});
    // A worker restart: the synthetic loss fires while the old style is still loaded.
    fire('webglcontextlost');
    expect(canWriteStyle(map)).toBe(false);
    fire('webglcontextrestored');
    expect(canWriteStyle(map)).toBe(false);
    fire('style.load');
    expect(canWriteStyle(map)).toBe(true);
  });
});
