/**
 * #576 D19: the Memory Stats panel costs nothing while it is closed, and its
 * renderer rows show real numbers.
 *
 * D19-D1: the freeze detector (a `longtask` PerformanceObserver plus a
 * requestAnimationFrame frame-gap chain that re-arms itself every frame) was
 * started when the always-mounted panel MOUNTED, hidden or not, and had no way
 * to stop - a 60 Hz wakeup for the whole session, so the WebView never went idle.
 * It now runs only while the panel is visible (Debug > Memory Stats, or
 * `debug.memory_probe`), and hiding the panel cancels the chain.
 *
 * D19-D2: the "MapLibre source counts" rows read counters nothing has written
 * since the GeoJSON sources were replaced by the custom renderer, so they were
 * always empty. The panel now shows the custom renderer's per-build buffer bytes,
 * which its controllers publish (see the controller tests for that wiring).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen } from '@testing-library/react';

vi.mock('../../../utils/ipc');

import { MemoryProbe } from '../MemoryProbe';
import * as probe from '../../../utils/memoryProbe';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import { invokeIpcMock } from '../../../test/factories/ipc';

const STATS = {
  backend: { pid: 1, name: 'stalagmight', role: 'backend', rss_kb: 1024, vm_size_kb: 0, vm_peak_kb: 0, vm_rss_anon_kb: 0 },
  webkit_processes: [],
  total_rss_kb: 1024,
  webkit_enumeration_supported: true,
};

let rafCalls: number;
let cancelled: number[];
let nextHandle: number;

beforeEach(() => {
  rafCalls = 0;
  cancelled = [];
  nextHandle = 0;
  // Never fire the callback: what matters is whether a frame is REQUESTED.
  vi.stubGlobal('requestAnimationFrame', () => {
    rafCalls += 1;
    nextHandle += 1;
    return nextHandle;
  });
  vi.stubGlobal('cancelAnimationFrame', (h: number) => {
    cancelled.push(h);
  });
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue(STATS);
  useUIStore.setState({ showMemoryStats: false });
  useProjectStore.setState({ config: null });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('MemoryProbe freeze detector runs only while the panel is visible (D19-D1)', () => {
  it('requests no animation frame while the panel is hidden', () => {
    render(<MemoryProbe />);
    expect(rafCalls, 'frames requested by a hidden panel').toBe(0);
  });

  it('starts one frame-gap chain when shown and cancels it when hidden', () => {
    render(<MemoryProbe />);
    act(() => useUIStore.setState({ showMemoryStats: true }));
    expect(rafCalls).toBe(1);
    const handle = nextHandle;

    act(() => useUIStore.setState({ showMemoryStats: false }));
    expect(cancelled, 'frame-gap chain cancelled on hide').toEqual([handle]);

    act(() => useUIStore.setState({ showMemoryStats: true }));
    expect(rafCalls, 'one new chain on re-show').toBe(2);
  });
});

describe('MemoryProbe shows the custom renderer buffers (D19-D2)', () => {
  it('lists the bytes of the last survey geometry build', () => {
    const record = (probe as Record<string, unknown>).recordRendererBuild as
      | ((kind: string, bytes: number) => void)
      | undefined;
    record?.('survey-geometry', 3 * 1024 * 1024);
    useUIStore.setState({ showMemoryStats: true });
    render(<MemoryProbe />);
    const row = screen.queryByTestId('memory-probe-renderer-survey-geometry');
    expect(row, 'renderer buffer row for the survey geometry').not.toBeNull();
    expect(row?.textContent).toContain('3.0 MB');
  });
});
