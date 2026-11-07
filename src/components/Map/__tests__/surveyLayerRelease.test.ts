/**
 * SurveyLayer.release() (#576 D16). Leaving the map runs MapCanvas's lightweight
 * unmount: `release()`, then `map.remove()`, and deliberately NOT `destroy()`
 * (see MapCanvas). So every JS resource that can still fire - a timer, an
 * animation frame, a listener - has to be dropped by `release()`. Before, five of
 * them were dropped only in `destroy()`: they kept the whole layer (survey data,
 * hit index, glyph atlas) alive for up to 2 s, and the interact-overlay fallback
 * restore then called `map.getLayer` on the removed map and threw.
 *
 * Drives a REAL SurveyLayer over the fake map (see surveyLayerFakeMap.ts), arming
 * each resource through the same entry points the app uses.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// maplibre-gl sets up its worker at module init, which node lacks; SurveyLayer
// only needs the type surface here.
vi.mock('maplibre-gl', () => ({ default: {} }));
// No WebGL under node: a ghost layer that fails to initialize sends the drag
// ghost down its stock path - the one with a bake timer and a map listener.
vi.mock('../surveyLayer/customRender/surveyGhostLayer', () => ({
  SurveyGhostLayer: class {
    readonly type = 'custom';
    constructor(readonly id: string) {}
    isDisabled(): boolean {
      return true;
    }
    getDisabledReason(): string {
      return 'no-webgl-in-node';
    }
  },
}));

import SurveyLayer from '../SurveyLayer';
import { INTERACT_HIDE_FALLBACK_MS } from '../surveyLayer/interactOverlayHide';
import type { DragPreviewSet } from '../surveyLayer/dragPreviewSource';
import { makeFakeSurveyMap, type FakeSurveyMap } from './surveyLayerFakeMap';

/** Private members the test arms directly (no public entry point reaches them
 *  without real geometry). */
interface LayerInternals {
  interactHide: { hideUntilRetile(): void };
  dragGhost: { begin(set: DragPreviewSet): string };
}

const SET: DragPreviewSet = {
  features: [{ emit: 'point', coords: [[0, 0]], movingEnd: 'uniform', properties: { kind: 'station', color: '#00ff00' } }],
};

function touch(type: string, x = 10, y = 10): Event {
  return Object.assign(new Event(type), { touches: [{ clientX: x, clientY: y }] });
}

let fake: FakeSurveyMap;
let layer: SurveyLayer;

beforeEach(() => {
  vi.useFakeTimers();
  // Frames ride the fake clock so a frame that outlives release() is observable.
  vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => setTimeout(() => cb(0), 16));
  vi.stubGlobal('cancelAnimationFrame', (id: ReturnType<typeof setTimeout>) => clearTimeout(id));
  vi.spyOn(console, 'error').mockImplementation(() => {});
  vi.spyOn(console, 'warn').mockImplementation(() => {});
  fake = makeFakeSurveyMap();
  layer = new SurveyLayer(fake.map);
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

/** Arm every deferred resource the way the app does while the map is in use. */
function armEverything(): void {
  const internals = layer as unknown as LayerInternals;
  // Hover: a mousemove queues one animation frame.
  fake.fire('mousemove', { point: { x: 5, y: 5 } });
  // A removal flush that touched a highlight hides the interact overlay until its
  // re-tile (map listener + 2 s fallback timer).
  internals.interactHide.hideUntilRetile();
  // A move drag: the stock ghost arms its bake timer + land listener, and the
  // overlay follow queues a frame.
  internals.dragGhost.begin(SET);
  layer.updateDragPreview(4, 2);
  // A finger down on the canvas arms the 500 ms long-press timer.
  fake.canvas.dispatchEvent(touch('touchstart'));
}

describe('SurveyLayer.release (the lightweight-unmount teardown)', () => {
  it('leaves no timer or animation frame pending', () => {
    armEverything();
    expect(vi.getTimerCount()).toBeGreaterThan(0);

    layer.release();

    expect(vi.getTimerCount()).toBe(0);
  });

  it('drops the canvas long-press listeners and the map sourcedata listeners', () => {
    armEverything();
    expect(fake.canvas.listenerCount('touchstart')).toBe(1);
    expect(fake.listenerCount('sourcedata')).toBeGreaterThan(0);

    layer.release();

    expect(fake.canvas.listenerCount('touchstart')).toBe(0);
    expect(fake.canvas.listenerCount('touchend')).toBe(0);
    expect(fake.listenerCount('sourcedata')).toBe(0);
  });

  it('nothing fires against the removed map afterwards (the post-unmount TypeError)', () => {
    armEverything();

    // MapCanvas's unmount order: release(), then map.remove().
    layer.release();
    fake.remove();

    expect(() => vi.advanceTimersByTime(INTERACT_HIDE_FALLBACK_MS + 1000)).not.toThrow();
  });
});
