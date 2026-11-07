/**
 * act()-wrapped waits for component and hook tests (#576 E15-NF1): the same
 * exact counts as `./async.ts`, run inside React's `act` so the state updates
 * they release are flushed before the test reads the DOM.
 */
import { act } from '@testing-library/react';

/** `turns` rounds of queued promise callbacks, inside `act`. */
export async function actMicrotasks(turns: number): Promise<void> {
  await act(async () => {
    for (let i = 0; i < turns; i += 1) await Promise.resolve();
  });
}

/** One zero-delay timer, inside `act`. */
export async function actNextTimer(): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
}
