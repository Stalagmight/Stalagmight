/**
 * [#576 NF-W8E3-1] map-context-loss.spec.ts "throws nothing, and the style
 * catches up after the restore" failed 8 of 12 runs on a loaded machine with the
 * page error "Cannot read properties of null (reading 'requestAnimationFrame')".
 * The context loss had nothing to do with it: the test selects a shot and then
 * switches to Plan mode, the selection makes the Sections panel reveal its row
 * (`scrollToIndex`), and Plan mode unmounts the panel before the reveal's frame
 * runs. The real TanStack Virtual runs here, in jsdom, with the frames held so
 * the order is the losing one every time.
 */
import { describe, it, expect, vi, afterEach } from 'vitest';
import { act, render } from '@testing-library/react';
import { useEffect, useRef } from 'react';
import { useVirtualizer, type Virtualizer } from '@tanstack/react-virtual';
import { useVirtualizerUnmountGuard } from '../useVirtualizerUnmountGuard';

type Rows = Virtualizer<HTMLDivElement, Element>;

/** A measured (dynamic-size) list, like the Sections panel's. */
function List({ guarded, onReady }: { guarded: boolean; onReady: (v: Rows) => void }) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const rows = useVirtualizer({
    count: 200,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 24,
    overscan: 4,
    // jsdom lays nothing out: give the viewport a size so rows render (and measure).
    initialRect: { width: 300, height: 120 },
  });
  // The hook is called either way, so both variants run the same hooks; only the
  // guard's effect differs.
  useVirtualizerUnmountGuard(guarded ? rows : DUMMY);
  useEffect(() => {
    onReady(rows);
  }, [rows, onReady]);
  return (
    <div ref={scrollRef} style={{ height: 120, overflow: 'auto' }}>
      <div style={{ height: rows.getTotalSize(), position: 'relative' }}>
        {rows.getVirtualItems().map((item) => (
          <div key={item.key} data-index={item.index} ref={rows.measureElement}>
            Row {item.index}
          </div>
        ))}
      </div>
    </div>
  );
}

/** A stand-in the unguarded variant hands the hook, so its real list is untouched. */
const DUMMY = { elementsCache: new Map() } as unknown as Rows;

afterEach(() => vi.restoreAllMocks());

function revealThenUnmount(guarded: boolean): { runFrames: () => void } {
  const frames: FrameRequestCallback[] = [];
  vi.spyOn(window, 'requestAnimationFrame').mockImplementation((cb) => frames.push(cb));
  let rows: Rows | null = null;
  const view = render(<List guarded={guarded} onReady={(v) => (rows = v)} />);
  expect(rows!.elementsCache.size, 'rows are measured: the list is in dynamic-size mode').toBeGreaterThan(0);
  act(() => rows!.scrollToIndex(150, { align: 'auto' }));
  expect(frames.length, 'scrollToIndex scheduled its verify frame').toBeGreaterThan(0);
  // Plan mode: the panel unmounts before the frame runs.
  view.unmount();
  return {
    runFrames: () => {
      while (frames.length > 0) frames.shift()!(performance.now());
    },
  };
}

describe('useVirtualizerUnmountGuard (#576 NF-W8E3-1)', () => {
  it('a reveal pending when its measured list unmounts throws nothing from its frame', () => {
    const { runFrames } = revealThenUnmount(true);
    expect(runFrames).not.toThrow();
  });

  it('without the guard the same order throws (the library bug the guard covers)', () => {
    const { runFrames } = revealThenUnmount(false);
    expect(runFrames).toThrow(/reading 'requestAnimationFrame'/);
  });
});
