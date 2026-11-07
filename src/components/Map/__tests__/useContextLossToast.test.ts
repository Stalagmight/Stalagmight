/**
 * The lost-WebGL-context watch (#576 D38-LOST): a loss the browser restores is
 * nothing to report (the layers come back by themselves); one that is not
 * restored within the grace period is reported once, and the report is taken
 * back when the map comes back or the watch ends. "Restored" means the style
 * MapLibre rebuilds on the restore has loaded (`style.load`), not the restore
 * event itself (#576 RV18-MAP-2).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Map as MaplibreMap } from 'maplibre-gl';
import { CONTEXT_RESTORE_GRACE_MS, watchContextRestore } from '../useContextLossToast';

function fakeMap() {
  const handlers = new Map<string, Set<() => void>>();
  const on = (ev: string, fn: () => void) => {
    if (!handlers.has(ev)) handlers.set(ev, new Set());
    handlers.get(ev)!.add(fn);
  };
  const off = (ev: string, fn: () => void) => handlers.get(ev)?.delete(fn);
  // Like MapLibre, off() also removes a listener registered with once().
  const onceWrappers = new Map<() => void, () => void>();
  const map = {
    on,
    once: (ev: string, fn: () => void) => {
      const wrapped = () => {
        handlers.get(ev)?.delete(wrapped);
        onceWrappers.delete(fn);
        fn();
      };
      onceWrappers.set(fn, wrapped);
      on(ev, wrapped);
    },
    off: (ev: string, fn: () => void) => {
      off(ev, fn);
      const wrapped = onceWrappers.get(fn);
      if (wrapped) off(ev, wrapped);
      onceWrappers.delete(fn);
    },
  } as unknown as MaplibreMap;
  const fire = (ev: string) => [...(handlers.get(ev) ?? [])].forEach((fn) => fn());
  const count = () => [...handlers.values()].reduce((n, s) => n + s.size, 0);
  return { map, fire, count };
}

let fake: ReturnType<typeof fakeMap>;
let unrecovered: ReturnType<typeof vi.fn>;
let recovered: ReturnType<typeof vi.fn>;
let detach: () => void;

beforeEach(() => {
  vi.useFakeTimers();
  fake = fakeMap();
  unrecovered = vi.fn();
  recovered = vi.fn();
  detach = watchContextRestore(fake.map, {
    graceMs: CONTEXT_RESTORE_GRACE_MS,
    onUnrecovered: unrecovered,
    onRecovered: recovered,
  });
});

afterEach(() => {
  vi.useRealTimers();
});

describe('watchContextRestore', () => {
  it('waits about five seconds before reporting a loss', () => {
    expect(CONTEXT_RESTORE_GRACE_MS).toBe(5000);
    fake.fire('webglcontextlost');
    vi.advanceTimersByTime(4999);
    expect(unrecovered).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(unrecovered).toHaveBeenCalledTimes(1);
  });

  /** MapLibre's restore: the event, then the rebuilt style loads next frame. */
  function restore(): void {
    fake.fire('webglcontextrestored');
    fake.fire('style.load');
  }

  it('reports nothing when the context comes back within the grace period', () => {
    fake.fire('webglcontextlost');
    vi.advanceTimersByTime(2000);
    restore();
    vi.advanceTimersByTime(10000);
    expect(unrecovered).not.toHaveBeenCalled();
    expect(recovered).not.toHaveBeenCalled();
  });

  it('takes the report back when the context returns late, once the rebuilt style has loaded', () => {
    fake.fire('webglcontextlost');
    vi.advanceTimersByTime(5000);
    fake.fire('webglcontextrestored');
    expect(recovered).not.toHaveBeenCalled();
    fake.fire('style.load');
    expect(recovered).toHaveBeenCalledTimes(1);
    // And a second loss is reported again.
    fake.fire('webglcontextlost');
    vi.advanceTimersByTime(5000);
    expect(unrecovered).toHaveBeenCalledTimes(2);
  });

  // #576 RV18-MAP-2: a second loss in the frame before the rebuilt style loads
  // leaves MapLibre nothing to rebuild from, so its restore brings back no style
  // and no style.load ever follows. The map is as blank as after a restore that
  // never came, and the user is told the same way.
  it('reports a restore that brings no loaded style back within the grace period', () => {
    fake.fire('webglcontextlost');
    fake.fire('webglcontextrestored');
    fake.fire('webglcontextlost');
    fake.fire('webglcontextrestored');
    vi.advanceTimersByTime(4999);
    expect(unrecovered).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    console.log('[RV18-MAP-2] reports after a second loss in the loading frame + 5 s:', unrecovered.mock.calls.length);
    expect(unrecovered).toHaveBeenCalledTimes(1);
    // A style that does load after all takes the report back.
    fake.fire('style.load');
    expect(recovered).toHaveBeenCalledTimes(1);
  });

  it('a style load without a restore settles nothing', () => {
    fake.fire('webglcontextlost');
    fake.fire('style.load');
    vi.advanceTimersByTime(5000);
    expect(unrecovered).toHaveBeenCalledTimes(1);
  });

  it('a repeated loss event does not restart or double the report', () => {
    fake.fire('webglcontextlost');
    vi.advanceTimersByTime(3000);
    fake.fire('webglcontextlost');
    vi.advanceTimersByTime(2000);
    fake.fire('webglcontextlost');
    vi.advanceTimersByTime(10000);
    expect(unrecovered).toHaveBeenCalledTimes(1);
  });

  it('detach clears the timer, the listeners and a standing report (map rebuilt or left)', () => {
    fake.fire('webglcontextlost');
    vi.advanceTimersByTime(5000);
    fake.fire('webglcontextrestored');
    detach();
    expect(recovered).toHaveBeenCalledTimes(1);
    expect(fake.count()).toBe(0);
    fake.fire('webglcontextlost');
    vi.advanceTimersByTime(10000);
    expect(unrecovered).toHaveBeenCalledTimes(1);
  });
});
