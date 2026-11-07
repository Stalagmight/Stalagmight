/**
 * [#576 RV6b-F2] `failOnRefreshErrors` counts a refresh that fails during a file's
 * own teardown.
 *
 * vitest runs afterEach hooks in reverse registration order (`sequence.hooks`
 * defaults to 'stack'), so the guard's afterEach - registered by a call after the
 * file's own top-level afterEach, as DmpNewImportDialog.runFailures does - read
 * console.error BEFORE that afterEach's final flush ran. A refresh that failed in
 * the flush was printed and the test passed. The guard now checks when the test
 * has finished, after every afterEach.
 *
 * `it.fails`: the test body is fine; the guard is what must fail it.
 */
import { afterEach, describe, it, vi } from 'vitest';

import { failOnRefreshErrors } from '../refreshGuard';

describe('failOnRefreshErrors', () => {
  // Registered BEFORE the guard, so on the base it ran AFTER the guard's check.
  afterEach(async () => {
    await Promise.resolve();
    // The line refreshProjectState logs when its decode fails.
    console.error('Failed to refresh project state:', new Error('late refresh failure'));
  });
  failOnRefreshErrors();

  it.fails('fails a test whose refresh fails in an afterEach registered before it', () => {
    vi.fn()();
  });
});
