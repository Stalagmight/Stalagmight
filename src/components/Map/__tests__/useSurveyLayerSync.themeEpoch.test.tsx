/**
 * [#576 RV4-FE-SL-4] A WebGL context restore (`styleEpoch` bump) re-applies the map
 * background, which the restored style lost - and nothing else. `applyThemeColors`
 * used to share that effect, so every restore re-ran it: with a live style that is
 * a second full survey rebuild (geometry index, custom geometry, label sync) right
 * after the restore's own replay, for a theme that did not change.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { describe, it, expect, vi } from 'vitest';
import { renderHook } from '@testing-library/react';
import type { MutableRefObject } from 'react';
import type * as maplibregl from 'maplibre-gl';

import { useSurveyLayerSync } from '../useSurveyLayerSync';
import type SurveyLayer from '../SurveyLayer';

type Options = Parameters<typeof useSurveyLayerSync>[0];

/** A SurveyLayer whose every method is a recorded no-op. */
function fakeSurveyLayer(): { layer: SurveyLayer; calls: (name: string) => number } {
  const fns = new Map<string, ReturnType<typeof vi.fn>>();
  const layer = new Proxy({}, {
    get: (_t, name: string) => {
      if (!fns.has(name)) fns.set(name, vi.fn());
      return fns.get(name);
    },
  }) as unknown as SurveyLayer;
  return { layer, calls: (name) => fns.get(name)?.mock.calls.length ?? 0 };
}

function options(over: Partial<Options>): Options {
  return {
    mapLoaded: true,
    styleEpoch: 0,
    project: null,
    activeStations: [],
    gpsOrigin: null,
    effectiveDepthGradient: false,
    depthRange: { min: 0, max: 0 },
    geometryMode: 'measured',
    surveyDataAdjusted: null,
    effectiveIssueSeverity: false,
    severityWarnings: [],
    config: null,
    resolvedTheme: 'dark',
    selection: null,
    snappedStation: null,
    mergeStationA: null,
    mergeStationB: null,
    showShotData: true,
    showShots: true,
    showSectionNames: true,
    showStationLabels: true,
    showStationComments: true,
    showStations: true,
    showShotComments: true,
    showReferenceShots: true,
    showJumps: true,
    moveCommentSource: null,
    anchoredSectionIds: new Set<number>(),
    immovableFeatureIds: { stationIds: new Set<number>(), shotIds: new Set<number>() },
    ...over,
  } as unknown as Options;
}

describe('useSurveyLayerSync: theme vs a context restore (#576 RV4-FE-SL-4)', () => {
  it('a style epoch bump re-applies the background only; a theme change re-applies both', () => {
    const { layer, calls } = fakeSurveyLayer();
    const setPaintProperty = vi.fn();
    const map = { current: { style: {}, setPaintProperty } as unknown as maplibregl.Map };
    const surveyLayerRef = { current: layer } as MutableRefObject<SurveyLayer | null>;
    const toolCallbacks = {} as Options['toolCallbacks'];
    const precision = {} as Options['precision'];
    const base = { map, surveyLayerRef, toolCallbacks, precision };
    const backgroundWrites = () =>
      setPaintProperty.mock.calls.filter((c) => c[0] === 'background' && c[1] === 'background-color').length;

    const { rerender } = renderHook((props: Options) => useSurveyLayerSync(props), {
      initialProps: options({ ...base, styleEpoch: 0 }),
    });
    expect(calls('applyThemeColors')).toBe(1);
    expect(backgroundWrites()).toBe(1);

    // The context came back: the restored style needs its background again.
    rerender(options({ ...base, styleEpoch: 1 }));
    expect(backgroundWrites()).toBe(2);
    expect(calls('applyThemeColors')).toBe(1);

    // A real theme change still re-colours the survey and the background.
    rerender(options({ ...base, styleEpoch: 1, resolvedTheme: 'light' }));
    expect(calls('applyThemeColors')).toBe(2);
    expect(backgroundWrites()).toBe(3);
  });

  // #576 RV18-MAP-1: in the frame before a rebuilt style loads, MapLibre 6's
  // setPaintProperty reaches a style that refuses the write and throws out of the
  // effect. The write waits for the epoch the loaded style brings.
  it('a theme change in the frame before a rebuilt style loads waits for it', () => {
    const { layer } = fakeSurveyLayer();
    const style = { _loaded: true };
    const setPaintProperty = vi.fn(() => {
      if (!style._loaded) throw new Error('Style is not done loading.');
    });
    const map = { current: { style, setPaintProperty } as unknown as maplibregl.Map };
    const surveyLayerRef = { current: layer } as MutableRefObject<SurveyLayer | null>;
    const base = { map, surveyLayerRef, toolCallbacks: {} as Options['toolCallbacks'], precision: {} as Options['precision'] };
    const backgroundWrites = () =>
      setPaintProperty.mock.calls.filter((c: unknown[]) => c[0] === 'background').length;
    const { rerender } = renderHook((props: Options) => useSurveyLayerSync(props), {
      initialProps: options({ ...base, styleEpoch: 0 }),
    });
    expect(backgroundWrites()).toBe(1);

    style._loaded = false; // restored, loading next frame
    let thrown: string | null = null;
    try {
      rerender(options({ ...base, styleEpoch: 0, resolvedTheme: 'light' }));
    } catch (e) {
      thrown = (e as Error).message;
    }
    console.log('[RV18-MAP-1] background effect in the loading frame threw:', thrown);
    expect(thrown).toBeNull();
    expect(backgroundWrites()).toBe(1);

    style._loaded = true; // style.load: the epoch moves
    rerender(options({ ...base, styleEpoch: 1, resolvedTheme: 'light' }));
    expect(backgroundWrites()).toBe(2);
  });
});
