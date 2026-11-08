import { describe, it, expect } from 'vitest';
import {
  DEFAULT_DPV_RANGE_RESERVE_FRACTION,
  MAX_DPV_RANGE_RESERVE_FRACTION,
  dpvRigInvalid,
  dpvUnitInvalid,
  dpvUsableRangeM,
  normalizedReserveFraction,
} from '../dpv';
import type { DpvRig } from '../../types/ipc.generated';

/**
 * #317 (M5): these are the cross-language twins of the Rust functions in
 * `src-tauri/src/config/gas.rs` - `dpvUnitInvalid` mirrors `dpv_unit_is_valid`,
 * `dpvUsableRangeM` mirrors `DpvRig::usable_range_m`, and `normalizedReserveFraction`
 * mirrors the branch-asymmetric clamp inside `GasConfig::normalize`. They moved here,
 * unchanged, when the DPV rig editor moved out of Settings into Plan mode's bottom
 * panel (#317) - GasSettings.test.tsx used to hold them rendered through the Settings
 * panel; they are exercised directly against the pure functions now.
 */

describe('dpvUnitInvalid (twin of Rust dpv_unit_is_valid)', () => {
  it('flags a unit with a non-positive speed', () => {
    expect(dpvUnitInvalid({ speed_m_per_min: 0, burn_minutes: 60 })).toBe(true);
    expect(dpvUnitInvalid({ speed_m_per_min: -1, burn_minutes: 60 })).toBe(true);
  });

  it('flags a unit with a non-positive burn time', () => {
    expect(dpvUnitInvalid({ speed_m_per_min: 24, burn_minutes: 0 })).toBe(true);
    expect(dpvUnitInvalid({ speed_m_per_min: 24, burn_minutes: -1 })).toBe(true);
  });

  it('flags a non-finite speed or burn time', () => {
    expect(dpvUnitInvalid({ speed_m_per_min: NaN, burn_minutes: 60 })).toBe(true);
    expect(dpvUnitInvalid({ speed_m_per_min: 24, burn_minutes: Infinity })).toBe(true);
  });

  it('accepts a valid unit', () => {
    expect(dpvUnitInvalid({ speed_m_per_min: 24, burn_minutes: 60 })).toBe(false);
  });
});

describe('dpvUsableRangeM (twin of Rust DpvRig::usable_range_m)', () => {
  it('matches the Rust formula for a hand-picked pair', () => {
    // Primary: 10 m/min x 100 min = 1000 m range but a slow burn. Backup: 40 m/min x 20 min =
    // 800 m range on a much shorter burn - picking the "weaker" unit by burn time (20 < 100)
    // would (wrongly) still land on the backup, so this pair does not distinguish the two
    // methods on its own; what matters is the number itself only comes out right - 800 m, not
    // 1000 m - when the engine compares RANGE (speed x burn), never burn alone.
    const rig: DpvRig = {
      primary: { speed_m_per_min: 10, burn_minutes: 100 },
      backup: { speed_m_per_min: 40, burn_minutes: 20 },
      range_reserve_fraction: 0.25,
    };
    // weaker = min(1000, 800) = 800; usable = 800 * (1 - 0.25) = 600
    expect(dpvUsableRangeM(rig)).toBeCloseTo(600, 6);
  });
});

/**
 * #317 (M5): the usable-range preview must accept the exact same domain the backend does for
 * `range_reserve_fraction` - `[0, MAX_DPV_RANGE_RESERVE_FRACTION]` (0.9,
 * `src-tauri/src/config/gas.rs`). Before this, the preview computed with the raw value while
 * Save only blocked a NEGATIVE one - a diver typing 0.95 previewed 5% of range while the backend
 * would clamp to 0.9 and give 10%, a silent drift once saved.
 */
describe('normalizedReserveFraction branch asymmetry (twin of Rust GasConfig::normalize)', () => {
  it('resets a negative reserve to the default (0.1), mirroring Rust normalize', () => {
    // ⚠ CROSS-LANGUAGE PIN (#317 FIX 4) - Rust's `GasConfig::normalize` RESETS a negative
    // `range_reserve_fraction` to the default 0.1; it does NOT clamp it to 0 the way a positive
    // value above the cap clamps to 0.9. A naive `Math.max(0, ...)` here would preview 100% of the
    // weaker unit's range for a negative entry instead of the 90% the backend actually gives it.
    expect(normalizedReserveFraction(-0.5)).toBe(DEFAULT_DPV_RANGE_RESERVE_FRACTION);
  });

  it('clamps a reserve above the cap down to 0.9', () => {
    // ⚠ CROSS-LANGUAGE PIN - twin of `test_gas_config_normalize_clamps_dpv_range_reserve_at_the_
    // cap_boundary` in `src-tauri/src/config/tests.rs`; that test names this one back.
    expect(normalizedReserveFraction(0.95)).toBe(MAX_DPV_RANGE_RESERVE_FRACTION);
  });

  it('leaves a value already inside the domain unchanged', () => {
    expect(normalizedReserveFraction(0.2)).toBe(0.2);
  });

  it('resets a non-finite reserve to the default', () => {
    expect(normalizedReserveFraction(NaN)).toBe(DEFAULT_DPV_RANGE_RESERVE_FRACTION);
  });

  it('the usable-range preview resets a negative reserve to the default (0.1), mirroring Rust normalize', () => {
    const rig: DpvRig = {
      primary: { speed_m_per_min: 10, burn_minutes: 100 }, // range 1000 m
      backup: { speed_m_per_min: 100, burn_minutes: 10 }, // range 1000 m - equal on purpose
      range_reserve_fraction: -0.5,
    };
    // weaker = 1000; reset to the default reserve 0.1; usable = 1000 * 0.9 = 900
    expect(dpvUsableRangeM(rig)).toBeCloseTo(900, 6);
  });

  it('the usable-range preview clamps a reserve above the cap down to 0.9', () => {
    // Primary and backup are given EQUAL range (1000 m each) so the "weaker unit" pick can't mask
    // the clamp: an unclamped 0.95 would preview 1000 * 0.05 = 50 m; the clamped 0.9 previews 100 m.
    const rig: DpvRig = {
      primary: { speed_m_per_min: 10, burn_minutes: 100 },
      backup: { speed_m_per_min: 100, burn_minutes: 10 },
      range_reserve_fraction: 0.95,
    };
    expect(dpvUsableRangeM(rig)).toBeCloseTo(100, 6);
  });

  it('does not block Save when the range reserve sits exactly at the cap (0.9)', () => {
    // The upstream Save-blocking guard used the same [0, MAX] domain; pinned here as a plain
    // boundary check on the shared constant now that the guard lives in DpvRigEditor's per-field
    // clamp rather than a Settings validation effect.
    expect(normalizedReserveFraction(0.9)).toBe(0.9);
  });
});

describe('dpvRigInvalid (the engine drop rule: GasConfig::normalize nulls the rig whole)', () => {
  const valid = { speed_m_per_min: 45, burn_minutes: 60 };
  it('either invalid unit condemns the rig; the reserve fraction never does', () => {
    expect(dpvRigInvalid({ primary: valid, backup: valid, range_reserve_fraction: 0.1 })).toBe(false);
    expect(
      dpvRigInvalid({
        primary: { speed_m_per_min: 45, burn_minutes: 0 },
        backup: valid,
        range_reserve_fraction: 0.1,
      })
    ).toBe(true);
    expect(
      dpvRigInvalid({
        primary: valid,
        backup: { speed_m_per_min: -1, burn_minutes: 45 },
        range_reserve_fraction: 0.1,
      })
    ).toBe(true);
    // An out-of-domain reserve keeps the rig - normalize clamps/resets it instead of dropping.
    expect(dpvRigInvalid({ primary: valid, backup: valid, range_reserve_fraction: 99 })).toBe(false);
  });
});
