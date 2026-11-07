/**
 * Promise and timer helpers for tests that drive async code by hand (#576
 * E15-NF1). The act()-wrapped variants for component tests are in `./act.ts`.
 *
 * The waits are exact: `microtasks(n)` is n `await Promise.resolve()` turns
 * inside one async function, `timerTurns(n)` is n zero-delay timers in a row,
 * `nextTimer()` is one zero-delay timer as a plain promise. A test that pins an
 * ordering ("the save has not landed after three turns") depends on the count,
 * so pick the one that matches what the test means and do not round it up.
 */

/** A promise the test settles by hand. */
export interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (reason?: unknown) => void;
}

/** A promise and the functions that settle it: `const d = deferred<Reply>(); ...; d.resolve(reply)`. */
export function deferred<T = void>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** Let `turns` rounds of queued promise callbacks run (no timers). */
export async function microtasks(turns: number): Promise<void> {
  for (let i = 0; i < turns; i += 1) await Promise.resolve();
}

/** Wait for one zero-delay timer: everything already queued, timers included, has run. */
export function nextTimer(): Promise<void> {
  return new Promise<void>((resolve) => setTimeout(resolve, 0));
}

/** Wait for `turns` zero-delay timers, one after the other. */
export async function timerTurns(turns: number): Promise<void> {
  for (let i = 0; i < turns; i += 1) await new Promise((resolve) => setTimeout(resolve, 0));
}
