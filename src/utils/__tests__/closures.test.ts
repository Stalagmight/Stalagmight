import { describe, it, expect } from 'vitest';
import { hasAdjustmentInputs, hasEligibleClosures, CLOSURE_ELIGIBLE_STRATEGIES } from '../closures';
import type { ClosureStrategy } from '../../types/ipc/ClosureStrategy';

const c = (strategy: ClosureStrategy) => ({ strategy });

describe('hasEligibleClosures', () => {
  it('is false for an empty list', () => {
    expect(hasEligibleClosures([])).toBe(false);
  });

  it('is true when a Batch or LeastSquaresWeighted closure is present', () => {
    expect(hasEligibleClosures([c('Batch')])).toBe(true);
    expect(hasEligibleClosures([c('LeastSquaresWeighted')])).toBe(true);
    expect(hasEligibleClosures([c('Move'), c('Batch')])).toBe(true);
  });

  it('is false when only ineligible (Move / deferred) closures are present', () => {
    expect(hasEligibleClosures([c('Move')])).toBe(false);
    expect(hasEligibleClosures([c('ProportionalByLength'), c('EqualDistribution')])).toBe(false);
  });

  it('eligible set mirrors the backend (Batch + LeastSquaresWeighted only)', () => {
    expect([...CLOSURE_ELIGIBLE_STRATEGIES].sort()).toEqual(['Batch', 'LeastSquaresWeighted']);
  });
});

describe('hasAdjustmentInputs (#741)', () => {
  const placement = { role: 'Placement' as const };
  const fix = { role: 'GpsFix' as const };

  it('is false with no eligible closure and only placement datums', () => {
    expect(hasAdjustmentInputs({ loop_closures: [c('Move')], zero_datums: [placement, placement] })).toBe(false);
  });

  it('is true for a GPS fix alone, with no closure at all', () => {
    expect(hasAdjustmentInputs({ loop_closures: [], zero_datums: [placement, fix] })).toBe(true);
  });

  it('is still true for an eligible closure with no datum', () => {
    expect(hasAdjustmentInputs({ loop_closures: [c('Batch')], zero_datums: [] })).toBe(true);
  });
});
