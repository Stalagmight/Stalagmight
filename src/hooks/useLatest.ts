import { useLayoutEffect, useRef, type MutableRefObject } from 'react';

/**
 * A ref that holds `value` as of the latest COMMITTED render (#576 D17).
 *
 * It replaces the mirror `const r = useRef(v); r.current = v;` written in the
 * render body. React forbids that write: a render it throws away (an error
 * boundary retry, a suspended or interrupted concurrent render) still leaves
 * its value in the ref, so an event handler, timer or effect could act on
 * props the screen never showed. Here the value is published in a layout
 * effect, so only a render that commits reaches the ref, and it does so before
 * any passive effect, timer or event handler of that commit runs.
 *
 * One ordering caveat: React runs a child's layout effects before its
 * parent's, so a child layout effect that calls back into this component in
 * the same commit still sees the previous value - read the prop directly there.
 *
 * The ref stays writable for the few callers that also update it eagerly
 * between renders; the next commit overwrites it with the rendered value, as
 * the render-body write used to on the next render.
 *
 * The `stalagmight/no-render-ref-write` ESLint rule flags a new render-body
 * write and points here.
 */
export function useLatest<T>(value: T): MutableRefObject<T> {
  const ref = useRef(value);
  useLayoutEffect(() => {
    ref.current = value;
  });
  return ref;
}
