/**
 * The Data workspace's persisted view has its defaults on BOTH sides: the Rust
 * `PanelsConfig::default()` (serialized into the generated defaultConfig.ts)
 * and the store's `DEFAULT_VIEW` / columnsMeta's `DATA_DEFAULT_HIDDEN`. A
 * one-sided change would make a fresh install and a reset store disagree about
 * which columns start hidden, so they are pinned against each other here.
 */
import { describe, expect, it } from 'vitest';
import { defaultConfig } from '../ipc/defaultConfig';
import { DATA_COLUMN_BY_KEY, DATA_DEFAULT_HIDDEN } from '../../components/DataWorkspace/columnsMeta';
import { DEFAULT_VIEW, MAP_PANE_DEFAULT_WIDTH } from '../../components/DataWorkspace/store';
import { TINT_MODES } from '../../components/DataWorkspace/types';

describe('panels.data_workspace_* defaults (generated) match the Data workspace store', () => {
  const panels = defaultConfig.panels;

  it('hidden columns equal DATA_DEFAULT_HIDDEN, in order', () => {
    expect(panels.data_workspace_hidden_columns).toEqual([...DATA_DEFAULT_HIDDEN]);
    expect(DEFAULT_VIEW.hiddenColumns).toEqual([...DATA_DEFAULT_HIDDEN]);
    for (const key of panels.data_workspace_hidden_columns) {
      expect(DATA_COLUMN_BY_KEY.has(key as never), `unknown column key ${key}`).toBe(true);
    }
  });

  it('every other field equals DEFAULT_VIEW', () => {
    expect(panels.data_workspace_column_order).toEqual(DEFAULT_VIEW.columnOrder);
    expect(panels.data_workspace_column_widths).toEqual(DEFAULT_VIEW.columnWidths);
    expect(panels.data_workspace_tint_mode).toBe(DEFAULT_VIEW.tintMode);
    expect(TINT_MODES).toContain(panels.data_workspace_tint_mode);
    expect(panels.data_workspace_map_pane_open).toBe(DEFAULT_VIEW.mapPaneOpen);
    expect(panels.data_workspace_map_pane_width).toBe(MAP_PANE_DEFAULT_WIDTH);
    expect(panels.data_workspace_map_pane_width).toBe(DEFAULT_VIEW.mapPaneWidth);
    expect(panels.data_workspace_section_headers).toBe(DEFAULT_VIEW.showSectionHeaders);
  });
});
