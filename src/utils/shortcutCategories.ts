import type { ShortcutsConfig } from '../types/config';

/**
 * Categories for organizing shortcuts in the settings UI and shortcuts overlay.
 */
export interface ShortcutCategory {
  id: string;
  labelKey: string;
  actions: (keyof ShortcutsConfig)[];
}

export const SHORTCUT_CATEGORIES: ShortcutCategory[] = [
  {
    id: 'file',
    labelKey: 'settings.shortcuts.categories.file',
    actions: ['new_file', 'open_file', 'save', 'save_as', 'import_dmp', 'export', 'quit'],
  },
  {
    id: 'tools',
    labelKey: 'settings.shortcuts.categories.tools',
    actions: ['select', 'pan', 'zoom', 'merge', 'measure', 'move_station', 'move_segment', 'move_shot', 'comment_tool', 'move_comment_tool'],
  },
  {
    id: 'edit',
    labelKey: 'settings.shortcuts.categories.edit',
    actions: ['undo', 'redo'],
  },
  {
    id: 'view',
    labelKey: 'settings.shortcuts.categories.view',
    actions: [
      'toggle_depth_gradient',
      'toggle_grid',
      'toggle_satellite',
      'toggle_station_labels',
      'toggle_shot_labels',
      'toggle_line_labels',
      'fit_to_bounds',
      'zoom_in',
      'zoom_out',
    ],
  },
  {
    id: 'panels',
    labelKey: 'settings.shortcuts.categories.panels',
    actions: ['toggle_sections_panel', 'toggle_detail_panel', 'toggle_data_table'],
  },
];

/**
 * Reserved shortcut display entries.
 * Groups related shortcuts into single display rows.
 */
export interface ReservedShortcutEntry {
  shortcuts: string[];
  descriptionKey: string;
}

export const RESERVED_DISPLAY_ENTRIES: ReservedShortcutEntry[] = [
  { shortcuts: ['Escape'], descriptionKey: 'settings.shortcuts.reserved.cancelClose' },
  { shortcuts: ['Delete', 'Backspace'], descriptionKey: 'settings.shortcuts.reserved.deleteSelection' },
  { shortcuts: ['Tab', 'Enter'], descriptionKey: 'settings.shortcuts.reserved.uiNavigation' },
  { shortcuts: ['Ctrl+A'], descriptionKey: 'settings.shortcuts.reserved.selectAll' },
  { shortcuts: ['Ctrl+,'], descriptionKey: 'settings.shortcuts.reserved.openSettings' },
  { shortcuts: ['?'], descriptionKey: 'settings.shortcuts.reserved.keyboardShortcuts' },
  { shortcuts: ['Ctrl+=', 'Ctrl+-', 'Ctrl+0'], descriptionKey: 'settings.shortcuts.reserved.uiZoom' },
];
