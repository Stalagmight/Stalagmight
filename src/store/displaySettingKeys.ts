/**
 * Storage keys for the per-project display settings (`save_display_setting`).
 *
 * A display setting is stored under its UIStore field name, with one exception:
 * the Line names toggle was renamed from `showSectionNames` to `showLineNames`
 * in the store, but the project file keeps the old row key. The backend reads
 * "showSectionNames" when it loads the settings (core `loading.rs`) and writes
 * it when it creates a project (`project_create.rs`), so writing the new name
 * would leave a row the load never reads and the toggle would not survive a
 * reopen. Every `save_display_setting` call goes through
 * {@link displaySettingStorageKey}.
 */

import type { DisplayToggleKey } from './uiStore';

/** Every key the frontend persists through `save_display_setting`. */
export type DisplaySettingKey = DisplayToggleKey | 'gridSpacing' | 'geometryMode';

/** Store key -> stored row key, for the keys whose stored name differs. */
export const DISPLAY_SETTING_STORAGE_KEYS: Readonly<Partial<Record<DisplaySettingKey, string>>> = {
  showLineNames: 'showSectionNames',
};

/** The display_settings row key that `key` is saved under. */
export function displaySettingStorageKey(key: DisplaySettingKey): string {
  return DISPLAY_SETTING_STORAGE_KEYS[key] ?? key;
}
