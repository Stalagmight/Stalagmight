/**
 * Unit tests for `isShotKindVisible` (#27).
 *
 * Jumps and References are independent display toggles. The mechanical
 * `is_reference` -> `kind !== 'Normal'` rewrite left several effects testing
 * BOTH virtual kinds against the reference toggle alone, so hiding References
 * silently threw away state attached to still-visible Jumps (a selection, a
 * measure snap, a line's fit-bounds). This predicate is the single place
 * that decides "is this leg currently drawn", so those effects agree.
 */

import { describe, it, expect } from 'vitest';

import { isShotKindVisible } from '../displaySelectors';

describe('isShotKindVisible', () => {
  it('hides everything when the master shots toggle is off', () => {
    for (const kind of ['Normal', 'Jump', 'Reference'] as const) {
      expect(isShotKindVisible(kind, false, true, true)).toBe(false);
    }
  });

  it('keeps jumps visible when only references are hidden', () => {
    // The regression: `kind !== 'Normal'` treated this jump as hidden.
    expect(isShotKindVisible('Jump', true, true, false)).toBe(true);
    expect(isShotKindVisible('Reference', true, true, false)).toBe(false);
    expect(isShotKindVisible('Normal', true, true, false)).toBe(true);
  });

  it('keeps references visible when only jumps are hidden', () => {
    // The mirror-image case, which the reference-only test could not see at all.
    expect(isShotKindVisible('Reference', true, false, true)).toBe(true);
    expect(isShotKindVisible('Jump', true, false, true)).toBe(false);
    expect(isShotKindVisible('Normal', true, false, true)).toBe(true);
  });

  it('hides both virtual kinds when both toggles are off', () => {
    expect(isShotKindVisible('Jump', true, false, false)).toBe(false);
    expect(isShotKindVisible('Reference', true, false, false)).toBe(false);
    expect(isShotKindVisible('Normal', true, false, false)).toBe(true);
  });

  it('reads an unresolvable shot as visible, so a lookup miss discards nothing', () => {
    // `undefined !== 'Normal'` was true, so a shot the effect could not find
    // during a project refresh counted as hidden and its selection / live
    // measurement was thrown away. Undefined now degrades to Normal, matching
    // the backend's ShotKind::from_u8 fallback.
    expect(isShotKindVisible(undefined, true, true, false)).toBe(true);
    expect(isShotKindVisible(undefined, true, false, false)).toBe(true);
    // The master toggle still wins.
    expect(isShotKindVisible(undefined, false, true, true)).toBe(false);
  });
});
