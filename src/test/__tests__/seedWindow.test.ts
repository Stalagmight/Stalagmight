/**
 * [#576 fuzz farm] The seed-range knobs of the seeded vitest suites: unset, every
 * suite keeps the seeds it always ran; set, the window moves so a long run can
 * roll it over fresh seeds.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { seedWindow } from '../seedWindow';
import { determinismSeeds } from '../determinism';

const KNOBS = [
  'STALAGMIGHT_VITEST_SEED_FIRST',
  'STALAGMIGHT_VITEST_SEED_COUNT',
  'STALAGMIGHT_DETERMINISM_FIRST',
  'STALAGMIGHT_DETERMINISM_RUNS',
];

// Each test starts from the knobs unset, whatever the run itself was given.
beforeEach(() => {
  for (const knob of KNOBS) vi.stubEnv(knob, '');
});

afterEach(() => {
  vi.unstubAllEnvs();
});

describe('seedWindow (STALAGMIGHT_VITEST_SEED_FIRST / _COUNT)', () => {
  it("is the suite's own range when the knobs are unset", () => {
    expect(seedWindow(101, 3)).toEqual([101, 102, 103]);
  });

  it('moves the first seed and the count independently', () => {
    vi.stubEnv('STALAGMIGHT_VITEST_SEED_FIRST', '5000');
    expect(seedWindow(1, 3)).toEqual([5000, 5001, 5002]);
    vi.stubEnv('STALAGMIGHT_VITEST_SEED_COUNT', '2');
    expect(seedWindow(1, 3)).toEqual([5000, 5001]);
  });

  it('refuses a knob that is not a usable integer instead of running no seeds', () => {
    vi.stubEnv('STALAGMIGHT_VITEST_SEED_COUNT', '0');
    expect(() => seedWindow(1, 3)).toThrow(/STALAGMIGHT_VITEST_SEED_COUNT=0/);
    vi.stubEnv('STALAGMIGHT_VITEST_SEED_COUNT', '');
    vi.stubEnv('STALAGMIGHT_VITEST_SEED_FIRST', 'abc');
    expect(() => seedWindow(1, 3)).toThrow(/STALAGMIGHT_VITEST_SEED_FIRST=abc/);
  });
});

describe('determinismSeeds (STALAGMIGHT_DETERMINISM_FIRST / _RUNS)', () => {
  it('is the regression seeds then 1..8 by default', () => {
    expect(determinismSeeds([113])).toEqual([113, 1, 2, 3, 4, 5, 6, 7, 8]);
  });

  it('rolls the generated window from the first seed asked for', () => {
    vi.stubEnv('STALAGMIGHT_DETERMINISM_FIRST', '1000');
    vi.stubEnv('STALAGMIGHT_DETERMINISM_RUNS', '3');
    expect(determinismSeeds([113, 1001])).toEqual([113, 1001, 1000, 1002]);
  });
});
