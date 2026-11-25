import { afterEach } from 'vitest';

/**
 * Lets the vitest worker's event loop turn after every test in this file.
 *
 * The worker reports progress to the main process over an RPC whose reply
 * must arrive within 60 s. A file of synchronous tests never yields between
 * them, so the reply waits until the whole file is done; when that takes
 * more than 60 s (the pill colour sweeps on a slow CI runner) the run fails
 * with `Timeout calling "onTaskUpdate"` although every test passed. Awaiting
 * a `setImmediate` lets the loop turn once, and any reply that has arrived is
 * handled then; one still in flight is handled at the next yield. So only a
 * stretch over 60 s between two yields (one test plus its hooks) can still
 * trip it.
 *
 * Call it at the top of any test file whose total synchronous time can
 * approach a minute on CI. Not for files that fake `setImmediate`
 * (`vi.useFakeTimers()`), where the yield would never resolve.
 */
export function yieldBetweenTests(): void {
  afterEach(() => new Promise<void>((resolve) => setImmediate(resolve)));
}
