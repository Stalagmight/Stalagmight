/**
 * #317 review round: Settings ▸ Gas "Reset to Defaults" must PRESERVE the DPV rig. The rig
 * lives on `gas` in the config data model, but it is planned in Plan mode's DPV tab and
 * the Gas settings page no longer displays it - so a category reset that wholesale-replaced
 * `gas` silently destroyed equipment with zero on-screen indication, while the working plan
 * store kept its copy and the engine went on planning a scootered dive over a config that had
 * lost the rig.
 */
import { describe, it, expect } from 'vitest';

import { getCategoryDefaults } from './SettingsDialog';
import { defaultConfig } from '../../types/config';
import type { Config } from '../../types/config';
import type { DpvRig } from '../../types/ipc.generated';

const RIG: DpvRig = {
  primary: { speed_m_per_min: 45, burn_minutes: 60 },
  backup: { speed_m_per_min: 45, burn_minutes: 45 },
  range_reserve_fraction: 0.1,
};

describe('getCategoryDefaults for the gas category (#317)', () => {
  it('preserves the current DPV rig while resetting every field the page actually shows', () => {
    const current = structuredClone(defaultConfig) as Config;
    current.gas.swim_speed_m_per_min = 99;
    current.gas.jump_rmv_multiplier = 2.5;
    current.gas.dpv = RIG;

    const out = getCategoryDefaults('gas', 'dark', current);
    expect(out.gas?.dpv).toEqual(RIG);
    expect(out.gas?.swim_speed_m_per_min).toBe(defaultConfig.gas.swim_speed_m_per_min);
    expect(out.gas?.jump_rmv_multiplier).toBe(defaultConfig.gas.jump_rmv_multiplier);
  });

  it('falls back to the compiled default (no rig) when no current config is supplied', () => {
    expect(getCategoryDefaults('gas', 'dark').gas?.dpv).toBe(defaultConfig.gas.dpv);
  });
});
