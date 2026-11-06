/**
 * [#576 fuzz farm] The harness suites' seed-range knobs: unset, each suite keeps
 * the seeds it always ran; set, the window moves so a long run can roll it.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { seedWindow } from './seedWindow';

// Each test starts from the knobs unset, whatever the run itself was given.
beforeEach(() => {
  vi.stubEnv('STALAGMIGHT_VITEST_SEED_FIRST', '');
  vi.stubEnv('STALAGMIGHT_VITEST_SEED_COUNT', '');
});

afterEach(() => {
  vi.unstubAllEnvs();
});

describe('seedWindow (STALAGMIGHT_VITEST_SEED_FIRST / _COUNT)', () => {
  it("is the suite's own range when the knobs are unset", () => {
    expect(seedWindow(1, 3)).toEqual([1, 2, 3]);
  });

  it('moves the first seed and the count independently', () => {
    vi.stubEnv('STALAGMIGHT_VITEST_SEED_FIRST', '7000');
    expect(seedWindow(1, 2)).toEqual([7000, 7001]);
    vi.stubEnv('STALAGMIGHT_VITEST_SEED_COUNT', '3');
    expect(seedWindow(1, 2)).toEqual([7000, 7001, 7002]);
  });

  it('refuses a knob that is not a usable integer', () => {
    vi.stubEnv('STALAGMIGHT_VITEST_SEED_FIRST', '-1');
    expect(() => seedWindow(1, 2)).toThrow(/STALAGMIGHT_VITEST_SEED_FIRST=-1/);
  });
});
