/**
 * #429 - what the module believes the webview's zoom is.
 *
 * `appliedScale` is module-private, but it is fully observable through the next
 * apply: `windowCssSize` recovers the window as `innerSize * appliedScale`, so
 * the scale the following `setZoom` is called with reports exactly what the
 * module thinks is on screen. Both directions matter - claiming a zoom that never
 * landed overstates the window and lets the cap go slack, while forgetting one
 * that did land understates it and starts a setZoom/resize ping-pong.
 *
 * `.tsx` is load-bearing - `vite.config.ts` routes only `*.test.tsx` to jsdom,
 * and these need `window` and `localStorage`.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { nextTimer } from '../../test/factories/async';

const setZoom = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/webview', () => ({
  getCurrentWebview: () => ({ setZoom }),
}));

/** A fresh module per test - `appliedScale` lives for the module's lifetime. */
async function loadUiScale() {
  vi.resetModules();
  return import('../uiScale');
}

function setWindowSize(width: number, height: number): void {
  Object.defineProperty(window, 'innerWidth', { value: width, configurable: true });
  Object.defineProperty(window, 'innerHeight', { value: height, configurable: true });
}

// 1280x720 caps a 3.0 preference to 1.44 - height binds (720/500 vs 1280/800).
const ZOOMED_WIDTH = Math.floor(1280 / 1.44);
const ZOOMED_HEIGHT = Math.floor(720 / 1.44);

describe('webview zoom bookkeeping', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    setWindowSize(1280, 720);
  });

  it('records a zoom that landed, so the re-measure through it settles', async () => {
    setZoom.mockResolvedValue(undefined);
    const { setUiScale, refreshUiScale } = await loadUiScale();

    setUiScale(3.0);
    expect(setZoom).toHaveBeenCalledTimes(1);
    expect(setZoom).toHaveBeenLastCalledWith(1.44);
    await nextTimer();

    // The zoom shrank the CSS viewport, which fires `resize`.
    setWindowSize(ZOOMED_WIDTH, ZOOMED_HEIGHT);
    refreshUiScale();

    // Read back through the applied scale the window is 1280x720 again, so the
    // cap is unchanged and nothing is pushed a second time.
    expect(setZoom).toHaveBeenCalledTimes(1);
  });

  it('does not keep a zoom the webview rejected (#429)', async () => {
    setZoom.mockRejectedValue(new Error('zoom unsupported on this platform'));
    const { setUiScale, refreshUiScale } = await loadUiScale();

    setUiScale(3.0);
    expect(setZoom).toHaveBeenLastCalledWith(1.44);
    await nextTimer();

    // Nothing zoomed, so the viewport is still the full window.
    refreshUiScale();

    // Recovering 1280x720 through a 1.44 that never took would report an
    // 1843x1037 window and hand the webview 2.07 - a cap loosened by a zoom that
    // does not exist.
    expect(setZoom).toHaveBeenCalledTimes(2);
    expect(setZoom).toHaveBeenLastCalledWith(1.44);
  });

  it('has the scale recorded before setZoom settles, for the resize it races', async () => {
    // setZoom is an IPC round trip, but the native side zooms straight away - the
    // `resize` that provokes can reach the page while the promise is still out.
    // That handler must already see the new scale, or it recovers the window from
    // an already-zoomed viewport through the OLD one, caps harder than it should
    // and pushes another zoom: the ping-pong UI_SCALE_APPLY_DEADBAND rules out.
    let landZoom!: () => void;
    setZoom.mockResolvedValue(undefined);
    setZoom.mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          landZoom = () => resolve();
        })
    );
    const { setUiScale, refreshUiScale } = await loadUiScale();

    setUiScale(3.0);
    expect(setZoom).toHaveBeenCalledTimes(1);

    setWindowSize(ZOOMED_WIDTH, ZOOMED_HEIGHT);
    refreshUiScale();
    expect(setZoom).toHaveBeenCalledTimes(1);

    landZoom();
    await nextTimer();
  });
});
