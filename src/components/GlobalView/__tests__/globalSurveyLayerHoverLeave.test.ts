// @vitest-environment jsdom
/**
 * The Global View map takes back its hover when the pointer leaves the canvas,
 * on the canvas's `mouseout` (#576 W41-NF2), as the survey map does (W39-NF4).
 *
 * The canvas has no children, so `mouseout` and `mouseleave` fire on the same
 * moves; `mouseout` comes first. React handles a panel row's onMouseEnter from
 * that same native `mouseout` as it bubbles to the root, so a leave handled on
 * `mouseleave` would clear a hover a row had just set. No Global View panel sets
 * a system hover today; this pins the order the survey map relies on, so one
 * that does cannot lose it.
 *
 * Drives a real GlobalSurveyLayer over a minimal map; the custom-render
 * controller is a stub whose hit-test says "system A is under any point".
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Map as MaplibreMap } from 'maplibre-gl';

const render = vi.hoisted(() => ({ setHover: vi.fn() }));

vi.mock('maplibre-gl', () => ({ default: {} }));
vi.mock('../customRender/globalController', () => ({
  GlobalCustomController: class {
    setHover = render.setHover;
    pick(): { systemId: string } {
      return { systemId: 'system-a' };
    }
    getSystemMetadata(): { name: string } {
      return { name: 'CaveA' };
    }
    destroy(): void {}
    resync(): void {}
  },
}));
vi.mock('../../../utils/dprWatcher', () => ({ watchDevicePixelRatio: () => () => {} }));

import GlobalSurveyLayer from '../GlobalSurveyLayer';

type Handler = (e: unknown) => void;

/** The map members GlobalSurveyLayer touches; anything else is a no-op. */
function makeMap(canvas: HTMLCanvasElement, container: HTMLElement): { map: MaplibreMap; fire: (type: string, e: unknown) => void } {
  const handlers = new Map<string, Set<Handler>>();
  const known: Record<string, unknown> = {
    on: (type: string, fn: Handler) => {
      const set = handlers.get(type) ?? new Set<Handler>();
      set.add(fn);
      handlers.set(type, set);
    },
    off: (type: string, fn: Handler) => handlers.get(type)?.delete(fn),
    getCanvas: () => canvas,
    getContainer: () => container,
    getCanvasContainer: () => container,
    getCenter: () => ({ lng: 0, lat: 0 }),
    getZoom: () => 10,
    getBearing: () => 0,
    getPitch: () => 0,
    project: () => ({ x: 0, y: 0 }),
  };
  const map = new Proxy(known, {
    get: (target, prop) => (prop in target ? target[prop as string] : () => undefined),
  }) as unknown as MaplibreMap;
  return { map, fire: (type, e) => handlers.get(type)?.forEach((fn) => fn(e)) };
}

let canvas: HTMLCanvasElement;
let container: HTMLElement;
let fake: ReturnType<typeof makeMap>;
let layer: GlobalSurveyLayer;

/** A pointer move over the map, which hovers system A. */
function hoverSystemA(): void {
  fake.fire('mousemove', { point: { x: 5, y: 5 }, originalEvent: new MouseEvent('mousemove', { clientX: 5, clientY: 5 }) });
}

beforeEach(() => {
  render.setHover.mockReset();
  container = document.createElement('div');
  canvas = document.createElement('canvas');
  container.appendChild(canvas);
  document.body.appendChild(container);
  fake = makeMap(canvas, container);
  layer = new GlobalSurveyLayer(fake.map, {});
});

afterEach(() => {
  layer.destroy();
  container.remove();
  vi.restoreAllMocks();
});

describe('GlobalSurveyLayer hover vs pointer leave (#576 W41-NF2)', () => {
  it('hovers the system under the pointer (control)', () => {
    hoverSystemA();
    expect(render.setHover).toHaveBeenLastCalledWith('system-a');
    expect(canvas.style.cursor).toBe('pointer');
  });

  it("clears the map's hover on the canvas's mouseout, before React's root listener sees it", () => {
    hoverSystemA();
    const tooltip = container.querySelector<HTMLElement>('.globalview-survey-tooltip');
    expect(tooltip?.style.display).toBe('block');

    canvas.dispatchEvent(new MouseEvent('mouseout', { bubbles: true }));

    expect(render.setHover).toHaveBeenLastCalledWith(null);
    expect(canvas.style.cursor).toBe('');
    expect(tooltip?.style.display).toBe('none');
  });

  it('leaves a hover set after that mouseout alone when the mouseleave follows', () => {
    hoverSystemA();
    canvas.dispatchEvent(new MouseEvent('mouseout', { bubbles: true }));
    // What a panel row's onMouseEnter would do from the same native mouseout.
    render.setHover('system-b');
    canvas.dispatchEvent(new MouseEvent('mouseleave'));

    expect(render.setHover).toHaveBeenLastCalledWith('system-b');
  });

  it('removes the leave listener it added', () => {
    const added = vi.spyOn(canvas, 'addEventListener');
    const removed = vi.spyOn(canvas, 'removeEventListener');
    const second = new GlobalSurveyLayer(fake.map, {});
    const leaveTypes = added.mock.calls.map(([type]) => type).filter((type) => type === 'mouseout' || type === 'mouseleave');
    second.destroy();

    expect(leaveTypes).toEqual(['mouseout']);
    expect(removed.mock.calls.map(([type]) => type)).toContain('mouseout');
  });
});
