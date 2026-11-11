import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';

// uiStore → telemetry → @tauri-apps/api/core; stub the IPC so importing the store
// in jsdom doesn't hit Tauri. `.tsx` so this runs in the jsdom tier (renderHook).
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockResolvedValue({}),
}));

import { usePanelReveal } from '../usePanelReveal';
import { useUIStore } from '../../../store/uiStore';

describe('usePanelReveal - close-grace + pin clearing', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    // Detail panel in hover mode, closed at rest.
    useUIStore.setState({ detailPanelMode: 'hover', detailPanelOpen: false });
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('does not let a shorter scheduleClose clobber a longer pending grace', () => {
    const { result } = renderHook(() => usePanelReveal('detail'));

    act(() => result.current.onRegionEnter()); // hovering → floating open
    expect(result.current.isFloatingOpen).toBe(true);

    act(() => result.current.scheduleClose(700)); // generous post-resize grace
    act(() => result.current.scheduleClose(120)); // shorter - must NOT clobber the 700

    act(() => vi.advanceTimersByTime(120));
    // The 120ms grace would have closed it; the 700ms one is still pending.
    expect(result.current.isFloatingOpen).toBe(true);

    act(() => vi.advanceTimersByTime(600)); // total 720 > 700
    expect(result.current.isFloatingOpen).toBe(false);
  });

  it('a later LONGER scheduleClose does extend a shorter pending grace', () => {
    const { result } = renderHook(() => usePanelReveal('detail'));

    act(() => result.current.onRegionEnter());
    act(() => result.current.scheduleClose(120));
    act(() => result.current.scheduleClose(700)); // longer - replaces the 120

    act(() => vi.advanceTimersByTime(120));
    expect(result.current.isFloatingOpen).toBe(true); // 120 timer was cleared
    act(() => vi.advanceTimersByTime(600));
    expect(result.current.isFloatingOpen).toBe(false);
  });

  it('clears transient hover + cancels the pending grace when the panel is pinned', () => {
    const { result } = renderHook(() => usePanelReveal('detail'));

    act(() => result.current.onRegionEnter());
    act(() => result.current.scheduleClose(700));
    expect(result.current.isFloatingOpen).toBe(true);

    // Pin (section-focus / keyboard / menu all force mode='pin').
    act(() => {
      useUIStore.setState({ detailPanelMode: 'pin' });
    });
    expect(result.current.isFloatingOpen).toBe(false);

    // Returning to hover must NOT re-float: hovering was cleared on pin (no pointer
    // is over the region), and the pending grace was cancelled.
    act(() => {
      useUIStore.setState({ detailPanelMode: 'hover' });
    });
    act(() => vi.advanceTimersByTime(1000));
    expect(result.current.isFloatingOpen).toBe(false);
  });
});

/**
 * #260: when the viewport can't afford a pinned side panel AND the map's minimum
 * width, the panel is force-closed for display only. It must behave exactly like a
 * hover panel - un-reflowed, still floatable over the map - WITHOUT touching the
 * persisted mode/open, so the user's real layout comes straight back.
 */
describe('usePanelReveal - viewport-forced close (#260)', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    // The normal case being squeezed: pinned open in the user's saved layout.
    useUIStore.setState({ detailPanelMode: 'pin', detailPanelOpen: true });
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('un-pins the panel for layout without changing the persisted mode/open', () => {
    const { result, rerender } = renderHook(
      ({ forcedClosed }: { forcedClosed: boolean }) =>
        usePanelReveal('detail', { forcedClosed }),
      { initialProps: { forcedClosed: false } },
    );

    expect(result.current.isPinnedOpen).toBe(true);

    rerender({ forcedClosed: true });

    expect(result.current.isPinnedOpen).toBe(false);
    expect(result.current.isOpen).toBe(false);
    // The user's saved layout is untouched - this is a display clamp, not a change.
    expect(useUIStore.getState().detailPanelMode).toBe('pin');
    expect(useUIStore.getState().detailPanelOpen).toBe(true);
  });

  it('still lets the opener tab float it over the map (no layout width, so no conflict)', () => {
    const { result } = renderHook(() => usePanelReveal('detail', { forcedClosed: true }));

    act(() => result.current.openClick());

    expect(result.current.isFloatingOpen).toBe(true);
    expect(result.current.isPinnedOpen).toBe(false);
    // openClick took the hover branch, so it wrote nothing to the store.
    expect(useUIStore.getState().detailPanelOpen).toBe(true);
  });

  it('does not snap a floated panel shut on the still-pinned persisted mode', () => {
    // The pin-clears-hover effect keys on mode === 'pin', which is still true while
    // force-closed - it must not fire, or the panel would close the instant it opened.
    const { result } = renderHook(() => usePanelReveal('detail', { forcedClosed: true }));

    act(() => result.current.onRegionEnter());
    act(() => vi.advanceTimersByTime(50));

    expect(result.current.isFloatingOpen).toBe(true);
  });

  it('restores the pinned layout as soon as the viewport can afford it again', () => {
    const { result, rerender } = renderHook(
      ({ forcedClosed }: { forcedClosed: boolean }) =>
        usePanelReveal('detail', { forcedClosed }),
      { initialProps: { forcedClosed: true } },
    );

    expect(result.current.isPinnedOpen).toBe(false);

    rerender({ forcedClosed: false });

    expect(result.current.isPinnedOpen).toBe(true);
  });
});

it('programmatic reveal preserves preferences and focus keeps a floating dock accessible', () => {
  vi.useFakeTimers();
  useUIStore.setState({ sectionsPanelMode: 'pin', sectionsPanelOpen: false });
  const { result } = renderHook(() => usePanelReveal('sections'));
  act(() => result.current.revealTemporarily());
  expect(result.current.isFloatingOpen).toBe(true);
  expect(useUIStore.getState().sectionsPanelOpen).toBe(false);
  expect(useUIStore.getState().sectionsPanelMode).toBe('pin');
  act(() => { result.current.onRegionFocus(); });
  act(() => { result.current.scheduleClose(100); vi.advanceTimersByTime(200); });
  expect(result.current.isOpen).toBe(true);
  act(() => result.current.close());
  expect(result.current.isOpen).toBe(false);
  vi.useRealTimers();
});
