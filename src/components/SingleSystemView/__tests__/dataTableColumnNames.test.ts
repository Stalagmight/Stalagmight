/**
 * Every column-name key the data table's header menu and eye menu look up
 * exists in the English strings: a key renamed on one side only shows the raw
 * key ("Hide dataTable.columnsMenu.lineId") instead of the column's name.
 */
import { describe, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn().mockResolvedValue(undefined) }));

import i18n from '../../../i18n';
import { HEADER_HIDE_GROUPS, STATION_MENU_COLS } from '../dataTableColumnGroups';

describe('data table column names', () => {
  it('names every header-hide group of the shots view', () => {
    for (const { nameKey } of Object.values(HEADER_HIDE_GROUPS)) {
      expect(i18n.exists(`dataTable.columnsMenu.${nameKey}`), nameKey).toBe(true);
    }
    expect(i18n.t(`dataTable.columnsMenu.${HEADER_HIDE_GROUPS.line_id!.nameKey}`)).toBe('Line name');
    expect(i18n.t(`dataTable.columnsMenu.${HEADER_HIDE_GROUPS.shot_id!.nameKey}`)).toBe('Shot (from → to)');
  });

  it('names every column group of the stations view', () => {
    for (const { nameKey } of STATION_MENU_COLS) {
      expect(i18n.exists(`dataTable.stationColumnsMenu.${nameKey}`), nameKey).toBe(true);
    }
  });
});
