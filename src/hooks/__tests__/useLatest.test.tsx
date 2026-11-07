/**
 * useLatest (#576 D17): the ref holds the value of the latest COMMITTED render.
 */
import { describe, expect, it } from 'vitest';
import { act, render } from '@testing-library/react';
import { Suspense, startTransition, useEffect, useState, type MutableRefObject } from 'react';
import { useLatest } from '../useLatest';

const never = new Promise<never>(() => {});
function Suspender({ on }: { on: boolean }) {
  if (on) throw never;
  return null;
}

function Probe({ value, out }: { value: string; out: { ref?: MutableRefObject<string> } }) {
  out.ref = useLatest(value);
  return <span>{value}</span>;
}

describe('useLatest', () => {
  it('holds the initial value on mount and the new value after a re-render commits', () => {
    const out: { ref?: MutableRefObject<string> } = {};
    const { rerender } = render(<Probe value="a" out={out} />);
    const first = out.ref;
    expect(first?.current).toBe('a');
    rerender(<Probe value="b" out={out} />);
    expect(out.ref).toBe(first); // one stable ref object
    expect(first?.current).toBe('b');
  });

  it('is already current in the passive effects of the commit that rendered the value', () => {
    const seen: string[] = [];
    function Reader({ value }: { value: string }) {
      const ref = useLatest(value);
      useEffect(() => {
        seen.push(ref.current);
      });
      return null;
    }
    const { rerender } = render(<Reader value="a" />);
    rerender(<Reader value="b" />);
    expect(seen).toEqual(['a', 'b']);
  });

  it('is not updated by a render React discards (a suspended transition)', () => {
    const out: { ref?: MutableRefObject<string> } = {};
    let go: () => void = () => {};
    function App() {
      const [next, setNext] = useState(false);
      go = () => startTransition(() => setNext(true));
      return (
        <Suspense fallback={null}>
          <Probe value={next ? 'never shown' : 'shown'} out={out} />
          <Suspender on={next} />
        </Suspense>
      );
    }
    const { container } = render(<App />);
    act(() => go());
    expect(container.textContent).toBe('shown');
    expect(out.ref?.current).toBe('shown');
  });
});
