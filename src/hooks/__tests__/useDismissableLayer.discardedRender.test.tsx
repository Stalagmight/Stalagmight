/**
 * #576 D17: a render React throws away must not leave its values behind.
 *
 * useDismissableLayer mirrored `onDismiss` into a ref by writing it in the render
 * body. When a transition re-rendered the layer with a new callback and then
 * suspended, React kept the old screen, but the ref already held the new
 * callback - so the next outside click ran a handler from a render that never
 * reached the screen. The mirror is now useLatest, which publishes a value only
 * when its render commits.
 */
import { describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { Suspense, startTransition, useRef, useState } from 'react';
import { useDismissableLayer } from '../useDismissableLayer';

const never = new Promise<never>(() => {});

/** Throws a never-settling promise while `on`, so a transition rendering it stays pending. */
function Suspender({ on }: { on: boolean }) {
  if (on) throw never;
  return null;
}

function Layer({ label, onDismiss }: { label: string; onDismiss: () => void }) {
  const inside = useRef<HTMLDivElement>(null);
  useDismissableLayer({ open: true, onDismiss, contains: [inside] });
  return (
    <div ref={inside} data-testid="layer">
      {label}
    </div>
  );
}

describe('useDismissableLayer under a discarded render (#576 D17)', () => {
  it('dismisses through the committed callback, not one from a suspended transition', () => {
    const dismissShown = vi.fn();
    const dismissNeverShown = vi.fn();
    let go: () => void = () => {};
    function App() {
      const [next, setNext] = useState(false);
      go = () => startTransition(() => setNext(true));
      return (
        <Suspense fallback={<span>loading</span>}>
          <Layer label={next ? 'B' : 'A'} onDismiss={next ? dismissNeverShown : dismissShown} />
          <Suspender on={next} />
        </Suspense>
      );
    }

    render(<App />);
    act(() => go());
    // The transition suspended: React kept the committed screen.
    expect(screen.getByTestId('layer').textContent).toBe('A');

    fireEvent.mouseDown(document.body);

    expect(dismissNeverShown).not.toHaveBeenCalled();
    expect(dismissShown).toHaveBeenCalledTimes(1);
  });
});
