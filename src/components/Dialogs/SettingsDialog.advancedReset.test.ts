/**
 * #357 item 15: Settings > Advanced "Reset to Defaults" must PRESERVE the data-table
 * view state. The Advanced page exposes exactly three `panels` fields - the two panel
 * widths and the data-table height - but the reset replaced the whole `panels` block,
 * so it also wiped column order, hidden columns, the two width maps and the angle
 * modes, none of which it displays or lists in its change summary.
 *
 * Worse than merely wide, it was incoherent: the open DataTable never noticed (its
 * seed effect is one-shot) so it kept rendering the old layout, and the next unrelated
 * view change posted the whole in-memory map back and restored exactly what had been
 * cleared. The reset therefore read as a no-op until restart, or was actively undone.
 * Pre-existing for order/hidden columns; #334 enrolled the width maps in it.
 *
 * Same shape as the #317 DPV-rig guard in SettingsDialog.gasReset.test.ts.
 */
import { describe, it, expect } from 'vitest';

import { getCategoryDefaults } from './SettingsDialog';
import { defaultConfig } from '../../types/config';
import type { Config } from '../../types/config';

/** A config whose data-table layout has been customised away from the defaults. */
function customisedConfig(): Config {
  const current = structuredClone(defaultConfig) as Config;
  current.panels.data_table_shot_column_order = ['comment', 'length', 'shot_id'];
  current.panels.data_table_shot_hidden_columns = ['_warnings'];
  current.panels.data_table_shot_column_widths = { length: 240 };
  current.panels.data_table_station_column_widths = { _x: 180 };
  current.panels.data_table_azimuth_mode = 'inout';
  current.panels.data_table_inclination_mode = 'inout';
  // The three the page DOES own, moved off their defaults so the reset has
  // something to actually do.
  current.panels.sections_panel_width = 777;
  current.panels.detail_panel_width = 555;
  current.panels.data_table_height = 333;
  return current;
}

describe('getCategoryDefaults for the advanced category (#357 item 15)', () => {
  it('resets the three panel sizes the page shows', () => {
    const out = getCategoryDefaults('advanced', 'dark', customisedConfig());
    expect(out.panels?.sections_panel_width).toBe(defaultConfig.panels.sections_panel_width);
    expect(out.panels?.detail_panel_width).toBe(defaultConfig.panels.detail_panel_width);
    expect(out.panels?.data_table_height).toBe(defaultConfig.panels.data_table_height);
  });

  it('preserves every data-table view field, which the page does not show', () => {
    const current = customisedConfig();
    const out = getCategoryDefaults('advanced', 'dark', current);
    expect(out.panels?.data_table_shot_column_order).toEqual(['comment', 'length', 'shot_id']);
    expect(out.panels?.data_table_shot_hidden_columns).toEqual(['_warnings']);
    expect(out.panels?.data_table_shot_column_widths).toEqual({ length: 240 });
    expect(out.panels?.data_table_station_column_widths).toEqual({ _x: 180 });
    expect(out.panels?.data_table_azimuth_mode).toBe('inout');
    expect(out.panels?.data_table_inclination_mode).toBe('inout');
  });

  it('still resets the other advanced sections', () => {
    const current = customisedConfig();
    current.closure_adjustment.base_sigma_length_m = 99;
    const out = getCategoryDefaults('advanced', 'dark', current);
    expect(out.closure_adjustment?.base_sigma_length_m).toBe(
      defaultConfig.closure_adjustment.base_sigma_length_m
    );
    expect(out.logging).toEqual(defaultConfig.logging);
  });

  it('falls back to the compiled defaults when no current config is supplied', () => {
    // Nothing to preserve without a current config, so the whole block resets -
    // the same fallback the gas category takes.
    const out = getCategoryDefaults('advanced', 'dark');
    expect(out.panels).toEqual(defaultConfig.panels);
  });
});
