import { describe, expect, it } from 'vitest';
import { DISPLAY_SETTING_STORAGE_KEYS, displaySettingStorageKey } from '../displaySettingKeys';

// The backend reads the Line names toggle from the display_settings row
// "showSectionNames" (core loading.rs) and writes that row on project create
// (project_create.rs). The store field is showLineNames, so the save must map it.
describe('displaySettingStorageKey', () => {
  it('stores the Line names toggle under its pre-rename key', () => {
    expect(displaySettingStorageKey('showLineNames')).toBe('showSectionNames');
  });

  it('stores every other setting under its own name', () => {
    for (const key of ['showStations', 'showStationNames', 'showShotErrors', 'gridSpacing', 'geometryMode'] as const) {
      expect(displaySettingStorageKey(key)).toBe(key);
    }
  });

  it('renames no other key', () => {
    expect(DISPLAY_SETTING_STORAGE_KEYS).toEqual({ showLineNames: 'showSectionNames' });
  });
});
