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

  // [#576 RVF-c-M2] The first-seed knob replaced every loop's own first, so two loops
  // of one file (customRender's 1.. and 101..) ran the same window under a farm run.
  it("offsets the first-seed knob by each loop's own first, so two loops stay disjoint", () => {
    vi.stubEnv('STALAGMIGHT_VITEST_SEED_FIRST', '5000');
    const low = seedWindow(1, 3);
    const high = seedWindow(101, 3);
    expect(low).toEqual([5000, 5001, 5002]);
    expect(high).toEqual([5100, 5101, 5102]);
    expect(low.filter((seed) => high.includes(seed)), 'seeds both loops ran').toEqual([]);
  });

  it('leaves each loop on its own range while the first-seed knob is unset', () => {
    vi.stubEnv('STALAGMIGHT_VITEST_SEED_COUNT', '2');
    expect(seedWindow(1, 40)).toEqual([1, 2]);
    expect(seedWindow(101, 40)).toEqual([101, 102]);
    vi.stubEnv('STALAGMIGHT_VITEST_SEED_COUNT', '');
    expect(seedWindow(1, 40)).toEqual(Array.from({ length: 40 }, (_unused, i) => 1 + i));
    expect(seedWindow(101, 40)).toEqual(Array.from({ length: 40 }, (_unused, i) => 101 + i));
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
