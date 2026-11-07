/**
 * The map factory all three surfaces build through (#576 D18): survey map,
 * Global View and section preview.
 *
 * - Satellite tiles that failed offline reload when the connection returns. The
 *   three per-surface effects this replaced called a `clearTiles` that is not on
 *   the source `getSource` returns, so they never ran (NF-MAP-2).
 * - The Android tile protocol is registered once and never removed: it used to
 *   be registered by the survey map only and removed on its unmount, under any
 *   other live map.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const h = vi.hoisted(() => {
  type Handler = () => void;
  const state = {
    maps: [] as FakeMap[],
    addProtocol: vi.fn(),
    removeProtocol: vi.fn(),
  };
  class FakeRasterSource {
    tiles = ['stalagmight-tiles://esri/{z}/{y}/{x}'];
    setTiles = vi.fn((tiles: string[]) => {
      this.tiles = tiles;
      return this;
    });
  }
  class FakeMap {
    options: Record<string, unknown>;
    style: object | null = {};
    satellite: FakeRasterSource | null = new FakeRasterSource();
    dragRotate = { disable: vi.fn() };
    touchPitch = { disable: vi.fn() };
    touchZoomRotate = { disableRotation: vi.fn() };
    private onceHandlers = new Map<string, Handler[]>();
    constructor(options: Record<string, unknown>) {
      this.options = options;
      state.maps.push(this);
    }
    getSource(id: string) {
      if (!this.style) throw new TypeError("Cannot read properties of null (reading 'getSource')");
      return id === 'satellite-tiles' ? (this.satellite ?? undefined) : undefined;
    }
    once(ev: string, fn: Handler) {
      this.onceHandlers.set(ev, [...(this.onceHandlers.get(ev) ?? []), fn]);
    }
    remove() {
      for (const fn of this.onceHandlers.get('remove') ?? []) fn();
      this.onceHandlers.delete('remove');
      this.style = null;
    }
  }
  return { state, FakeMap };
});

vi.mock('maplibre-gl', () => ({
  Map: h.FakeMap,
  addProtocol: h.state.addProtocol,
  removeProtocol: h.state.removeProtocol,
}));
vi.mock('@tauri-apps/api/core', async () => (await import('../../../test/factories/tauri')).tauriCore(vi.fn()));

import { createSurveyMap, reloadSatelliteTiles, type SurveyMapOptions } from '../createSurveyMap';
import { useNetworkStore } from '../../../store/networkStore';

type FakeMapT = InstanceType<typeof h.FakeMap>;

const container = {} as HTMLElement;
const style = { version: 8, sources: {}, layers: [] } as SurveyMapOptions['style'];
const SURFACES: Record<string, SurveyMapOptions> = {
  survey: { container, style, center: [0, 0], zoom: 14, keyboard: false, boxZoom: false, performanceTuned: true },
  global: { container, style, center: [10, 20], zoom: 10, minZoom: 0, maxZoom: 19, keyboard: false, boxZoom: true },
  preview: { container, style, center: [0, 0], zoom: 14, minZoom: 12, maxZoom: 22, keyboard: true, boxZoom: true },
};

function build(name: keyof typeof SURFACES): FakeMapT {
  createSurveyMap(SURFACES[name]!);
  return h.state.maps[h.state.maps.length - 1]!;
}

const reconnect = () => useNetworkStore.getState().triggerTileReload();

beforeEach(() => {
  h.state.maps = [];
});

afterEach(() => {
  for (const m of h.state.maps) m.remove();
});

describe('createSurveyMap', () => {
  it.each(Object.keys(SURFACES))('%s: shared options and interaction disabling', (name) => {
    const map = build(name);
    const o = SURFACES[name]!;
    expect(map.options).toMatchObject({
      container,
      style,
      center: o.center,
      zoom: o.zoom,
      attributionControl: false,
      keyboard: o.keyboard,
      boxZoom: o.boxZoom,
    });
    expect(map.options.fadeDuration).toBe(o.performanceTuned ? 0 : undefined);
    expect(map.options.renderWorldCopies).toBe(o.performanceTuned ? false : undefined);
    expect(map.dragRotate.disable).toHaveBeenCalledTimes(1);
    expect(map.touchPitch.disable).toHaveBeenCalledTimes(1);
    expect(map.touchZoomRotate.disableRotation).toHaveBeenCalledTimes(1);
  });

  it('reloads every surface\'s satellite tiles, failed ones included, when the connection returns', () => {
    const maps = (['survey', 'global', 'preview'] as const).map(build);
    for (const m of maps) expect(m.satellite!.setTiles).not.toHaveBeenCalled();
    reconnect();
    for (const m of maps) {
      expect(m.satellite!.setTiles).toHaveBeenCalledTimes(1);
      expect(m.satellite!.setTiles).toHaveBeenCalledWith(['stalagmight-tiles://esri/{z}/{y}/{x}']);
    }
    reconnect();
    for (const m of maps) expect(m.satellite!.setTiles).toHaveBeenCalledTimes(2);
  });

  it('stops reloading once the map is removed', () => {
    const map = build('global');
    const source = map.satellite!;
    map.remove();
    expect(() => reconnect()).not.toThrow();
    expect(source.setTiles).not.toHaveBeenCalled();
  });

  it('a map without satellite imagery, or without a style, is left alone', () => {
    const plain = build('preview');
    plain.satellite = null;
    const lost = build('survey');
    const lostSource = lost.satellite!;
    lost.style = null; // a lost WebGL context
    expect(() => reconnect()).not.toThrow();
    expect(lostSource.setTiles).not.toHaveBeenCalled();
    expect(reloadSatelliteTiles(plain as unknown as Parameters<typeof reloadSatelliteTiles>[0])).toBe(false);
  });
});

describe('Android tile protocol', () => {
  const ua = Object.getOwnPropertyDescriptor(Navigator.prototype, 'userAgent');
  beforeEach(() => {
    vi.resetModules();
    h.state.addProtocol.mockClear();
    h.state.removeProtocol.mockClear();
    Object.defineProperty(navigator, 'userAgent', { value: 'Mozilla/5.0 (Linux; Android 14) AppleWebKit', configurable: true });
  });
  afterEach(() => {
    delete (navigator as unknown as Record<string, unknown>).userAgent;
    if (ua) Object.defineProperty(Navigator.prototype, 'userAgent', ua);
  });

  it('is registered once for every map and never removed', async () => {
    const { registerTileProtocol } = await import('../../../utils/tileProtocol');
    const factory = await import('../createSurveyMap');
    registerTileProtocol(); // app start (main.tsx)
    const a = factory.createSurveyMap(SURFACES.survey!);
    factory.createSurveyMap(SURFACES.global!);
    a!.remove(); // leaving a project used to unregister it
    factory.createSurveyMap(SURFACES.preview!);
    expect(h.state.addProtocol).toHaveBeenCalledTimes(1);
    expect(h.state.addProtocol.mock.calls[0]![0]).toBe('stalagmight-tiles');
    expect(h.state.removeProtocol).not.toHaveBeenCalled();
  });
});
