import i18n from '../i18n';

/**
 * Keyboard shortcut utilities for platform detection, normalization, conflict detection,
 * and display formatting.
 *
 * IMPORTANT: Canonicalization rules MUST match Rust (src-tauri/src/config/mod.rs)
 * for consistent conflict detection between frontend and backend.
 */

/**
 * Detect if running on macOS/iOS platform.
 * Used for platform-specific shortcut display (symbols vs text).
 */
export function isMac(): boolean {
  return /Mac|iPhone|iPad/.test(navigator.platform);
}

/**
 * Reserved shortcuts that cannot be reassigned.
 * These are system-level or critical app shortcuts.
 * Stored in canonical form.
 */
export const RESERVED_SHORTCUTS: ReadonlySet<string> = new Set([
  'Escape',
  'Delete',
  'Backspace',
  'Tab',
  'Enter',
  'Ctrl+A', // Select all
  'Ctrl+,', // Open Settings
]);

/**
 * Special key canonical names mapping.
 * Maps various input forms to consistent canonical names.
 * Keys are lowercase for lookup, values are PascalCase canonical names.
 *
 * Must match Rust normalize_key_name() function.
 */
const SPECIAL_KEYS: Record<string, string> = {
  // Space
  ' ': 'Space',
  space: 'Space',
  spacebar: 'Space',

  // Enter
  enter: 'Enter',
  return: 'Enter',

  // Escape
  esc: 'Escape',
  escape: 'Escape',

  // Arrow keys
  up: 'ArrowUp',
  arrowup: 'ArrowUp',
  down: 'ArrowDown',
  arrowdown: 'ArrowDown',
  left: 'ArrowLeft',
  arrowleft: 'ArrowLeft',
  right: 'ArrowRight',
  arrowright: 'ArrowRight',

  // Math symbols
  '+': 'Plus',
  plus: 'Plus',
  '-': 'Minus',
  minus: 'Minus',
  '=': 'Equal',
  equal: 'Equal',
  equals: 'Equal',

  // Navigation
  backspace: 'Backspace',
  back: 'Backspace',
  delete: 'Delete',
  del: 'Delete',
  tab: 'Tab',
  home: 'Home',
  end: 'End',
  pageup: 'PageUp',
  pgup: 'PageUp',
  pagedown: 'PageDown',
  pgdn: 'PageDown',
  insert: 'Insert',
  ins: 'Insert',
};

/**
 * Normalize a key name to canonical form.
 * - Special keys: PascalCase (Space, ArrowUp, etc.)
 * - Single letters: UPPERCASE (matches KeyboardEvent.key)
 * - Numbers and other keys: preserved as-is
 *
 * Must match Rust normalize_key_name() function.
 */
function normalizeKeyName(key: string): string {
  const lower = key.toLowerCase();

  // Check special keys first
  if (SPECIAL_KEYS[lower]) {
    return SPECIAL_KEYS[lower];
  }

  // Handle space character directly (KeyboardEvent.key returns ' ' for space)
  if (key === ' ') {
    return 'Space';
  }

  // Normalize digit and numpad keys (e.g., "Digit1", "Numpad1") to "1"
  const digitMatch = lower.match(/^digit([0-9])$/);
  if (digitMatch) {
    const digit = digitMatch[1];
    if (digit !== undefined) {
      return digit;
    }
  }

  const numpadMatch = lower.match(/^numpad([0-9])$/);
  if (numpadMatch) {
    const digit = numpadMatch[1];
    if (digit !== undefined) {
      return digit;
    }
  }

  // Function keys (F1, F2, ... - match Rust: accepts any numeric suffix that parses)
  if (lower.startsWith('f') && lower.length > 1) {
    const numStr = lower.slice(1);
    const num = parseInt(numStr, 10);
    // Rust accepts any F<n> where n parses as a number (no upper bound)
    if (!isNaN(num) && numStr === String(num)) {
      return `F${num}`;
    }
  }

  // Single letter keys - UPPERCASE (matches KeyboardEvent.key)
  if (key.length === 1 && /^[a-zA-Z]$/.test(key)) {
    return key.toUpperCase();
  }

  // Numbers and other keys - preserve as-is
  return key;
}

/**
 * Normalize a shortcut string to canonical form.
 * Returns null if the shortcut is invalid.
 *
 * Canonical form rules (MUST match Rust):
 * 1. Modifiers sorted: Ctrl -> Alt -> Shift -> Key
 * 2. Single letter keys: UPPERCASE
 * 3. Special keys: PascalCase canonical names
 * 4. Platform aliases: Meta/Cmd/Command -> Ctrl
 *
 * @param shortcut - Raw shortcut string (e.g., "ctrl+s", "Shift+Ctrl+A")
 * @returns Normalized shortcut string or null if invalid
 */
export function normalizeShortcut(shortcut: string): string | null {
  // Handle single space character before trimming
  if (shortcut === ' ') {
    return 'Space';
  }

  const trimmed = shortcut.trim();
  if (!trimmed) {
    return null;
  }

  // Special case: single "+" key
  if (trimmed === '+') {
    return 'Plus';
  }

  // Special case: single "-" key
  if (trimmed === '-') {
    return 'Minus';
  }

  // Special case: single "=" key
  if (trimmed === '=') {
    return 'Equal';
  }

  // Split by + to get modifiers and key
  const parts = trimmed.split('+').map((s) => s.trim());
  if (parts.length === 0) {
    return null;
  }

  // Count trailing empty parts (any trailing + means Plus key, matching Rust)
  // "Ctrl+" -> ["Ctrl", ""] -> 1 trailing empty -> Ctrl+Plus
  // "Ctrl++" -> ["Ctrl", "", ""] -> 2 trailing empties -> Ctrl+Plus
  // "Shift+" -> ["Shift", ""] -> 1 trailing empty -> Shift+Plus
  let trailingEmptyCount = 0;
  for (let i = parts.length - 1; i >= 0; i--) {
    if (parts[i] === '') {
      trailingEmptyCount++;
    } else {
      break;
    }
  }

  // Check for empty parts in the middle (not trailing)
  // e.g., "Ctrl++S" has empty in middle -> invalid
  const nonTrailingParts = parts.slice(0, parts.length - trailingEmptyCount);
  if (nonTrailingParts.some((p) => p === '')) {
    return null;
  }

  let ctrl = false;
  let alt = false;
  let shift = false;
  let key: string | null = null;

  // Process non-empty parts
  for (const part of nonTrailingParts) {
    const lower = part.toLowerCase();
    switch (lower) {
      case 'ctrl':
      case 'control':
      case 'meta':
      case 'cmd':
      case 'command':
        ctrl = true;
        break;
      case 'alt':
      case 'option':
        alt = true;
        break;
      case 'shift':
        shift = true;
        break;
      default:
        // This is the key (should be only one non-modifier)
        if (key !== null) {
          // Multiple non-modifier keys is invalid
          return null;
        }
        key = normalizeKeyName(part);
    }
  }

  // Handle trailing empty parts as Plus key (matching Rust behavior)
  // Any trailing + is treated as Plus: "Ctrl+" -> "Ctrl+Plus", "Ctrl++" -> "Ctrl+Plus"
  if (trailingEmptyCount > 0) {
    if (key !== null) {
      // Already have a key, can't have Plus too
      return null;
    }
    key = 'Plus';
  }

  // Must have exactly one key
  if (key === null) {
    return null;
  }

  // Build canonical form: Ctrl+Alt+Shift+Key
  let result = '';
  if (ctrl) {
    result += 'Ctrl+';
  }
  if (alt) {
    result += 'Alt+';
  }
  if (shift) {
    result += 'Shift+';
  }
  result += key;

  return result;
}

/**
 * Parse a KeyboardEvent into a normalized shortcut string.
 * Useful for capturing user key presses in shortcut recorder.
 *
 * Note: This function does not filter `event.repeat`. Callers should check
 * `event.repeat` themselves if they need to prevent repeated key events
 * from triggering multiple actions.
 *
 * @param event - The keyboard event
 * @returns Normalized shortcut string (e.g., "Ctrl+Shift+S")
 */
export function parseKeyEvent(event: KeyboardEvent): string {
  const parts: string[] = [];

  // Add modifiers in canonical order: Ctrl -> Alt -> Shift
  // Note: On Mac, metaKey is Cmd, on Windows/Linux it's the Windows/Super key
  // We normalize both ctrlKey and metaKey to Ctrl for cross-platform consistency
  if (event.ctrlKey || event.metaKey) {
    parts.push('Ctrl');
  }
  if (event.altKey) {
    parts.push('Alt');
  }
  if (event.shiftKey) {
    parts.push('Shift');
  }

  // Get the key and normalize it
  // Skip if the key is a modifier itself
  const key = event.key;
  if (!['Control', 'Alt', 'Shift', 'Meta'].includes(key)) {
    parts.push(normalizeKeyName(key));
  }

  return parts.join('+');
}

/**
 * Check if a shortcut is reserved and cannot be reassigned.
 *
 * @param shortcut - Shortcut string (will be normalized before checking)
 * @returns true if the shortcut is reserved
 */
export function isReservedShortcut(shortcut: string): boolean {
  const normalized = normalizeShortcut(shortcut);
  if (normalized === null) {
    return false;
  }
  return RESERVED_SHORTCUTS.has(normalized);
}

/**
 * Reserved keys that cannot be used in any shortcut combination.
 * These keys trigger reserved handlers regardless of modifiers.
 * For example, Delete key triggers delete regardless of Ctrl/Alt/Shift.
 */
export const RESERVED_KEYS: ReadonlySet<string> = new Set([
  'Escape',
  'Delete',
  'Backspace',
  'Tab',
  'Enter',
  '?',
]);

/**
 * Reserved key+modifier combinations.
 * The modifier (Ctrl) plus these keys are reserved.
 *
 * The zoom keys are here because useUiScale installs an unconditional window
 * keydown listener for Ctrl/Cmd + '=' / '+' / '-' / '_' / '0'. Binding any of
 * them to an action would fire both handlers, so the editor must refuse them -
 * which also makes enforcement agree with the Settings dialog, where they have
 * always been listed as reserved. Only the Ctrl-modified forms are blocked:
 * the bare Equal/Plus/Minus map-zoom defaults stay bindable.
 */
export const RESERVED_CTRL_KEYS: ReadonlySet<string> = new Set([
  'A', // Select all
  ',', // Open Settings
  'Equal', // Zoom interface in
  'Plus', // Zoom interface in
  'Minus', // Zoom interface out
  '_', // Zoom interface out (Shift+Minus)
  '0', // Reset interface zoom
]);

/**
 * Human-readable descriptions for reserved Ctrl+key shortcuts.
 * Used in checkReservedConflict() to explain why a shortcut is reserved.
 */
export const RESERVED_CTRL_DESCRIPTIONS: Readonly<Record<string, string>> = {
  A: 'Select all',
  ',': 'Open Settings',
  Equal: 'Zoom interface in',
  Plus: 'Zoom interface in',
  Minus: 'Zoom interface out',
  _: 'Zoom interface out',
  '0': 'Reset interface zoom',
};

/**
 * Check if a shortcut conflicts with reserved shortcuts.
 * This is broader than isReservedShortcut - it blocks:
 * 1. Any shortcut using Escape, Delete, or Backspace (regardless of modifiers)
 * 2. Any shortcut using Ctrl plus a key in RESERVED_CTRL_KEYS - A, ',' and the
 *    UI-zoom keys Equal/Plus/Minus/_/0 (regardless of additional modifiers)
 *
 * The "regardless of additional modifiers" part is DELIBERATELY broader than the
 * runtime handlers, not a mirror of them. The UI-zoom listener in
 * src/hooks/useUiScale.ts bails on Alt (`if (!(event.ctrlKey || event.metaKey) ||
 * event.altKey) return;`), so Ctrl+Alt+Equal never actually zooms - yet the editor
 * still refuses it. That over-block is intentional and conservative: it keeps the
 * refusal rule stated in terms of the key rather than a modifier matrix that would
 * have to track each handler's guards, and it matches how Ctrl+Alt+A is already
 * refused for Select all.
 *
 * @param shortcut - Shortcut string (will be normalized before checking)
 * @returns Object with isReserved flag and reason string, or null if not reserved
 */
export function checkReservedConflict(shortcut: string): { isReserved: boolean; reason: string } | null {
  const normalized = normalizeShortcut(shortcut);
  if (normalized === null) {
    return null;
  }

  // Extract key from shortcut (last part after splitting by +)
  const parts = normalized.split('+');
  const key = parts[parts.length - 1] ?? '';
  const hasCtrl = parts.includes('Ctrl');

  // Check if the key itself is reserved (Escape, Delete, Backspace, Tab, Enter)
  if (key && RESERVED_KEYS.has(key)) {
    const descriptions: Record<string, string> = {
      Escape: 'Cancel / Close dialogs',
      Delete: 'Delete selection',
      Backspace: 'Delete selection',
      Tab: 'UI Navigation',
      Enter: 'UI Navigation',
      '?': 'Keyboard shortcuts',
    };
    return {
      isReserved: true,
      reason: descriptions[key] ?? `Reserved for ${key}`,
    };
  }

  // Check if Ctrl + reserved key (A, ',', and the UI-zoom keys).
  // Bare Equal/Plus/Minus stay bindable - the map-zoom defaults use them.
  if (key && hasCtrl && RESERVED_CTRL_KEYS.has(key)) {
    return {
      isReserved: true,
      reason: RESERVED_CTRL_DESCRIPTIONS[key] ?? `Reserved for Ctrl+${key}`,
    };
  }

  return null;
}

/**
 * Find a conflict for a shortcut in the registry.
 * Returns the action name if the shortcut is already assigned, or null if no conflict.
 *
 * @param shortcut - Shortcut string to check (will be normalized)
 * @param registry - Map of action names to their assigned shortcuts (already normalized)
 * @param excludeAction - Optional action to exclude from conflict check (for editing existing binding)
 * @returns The conflicting action name, or null if no conflict
 */
export function findConflict(
  shortcut: string,
  registry: Map<string, string[]>,
  excludeAction?: string
): string | null {
  const normalized = normalizeShortcut(shortcut);
  if (normalized === null) {
    return null;
  }

  for (const [action, shortcuts] of registry) {
    if (excludeAction && action === excludeAction) {
      continue;
    }
    if (shortcuts.some((s) => normalizeShortcut(s) === normalized)) {
      return action;
    }
  }

  return null;
}

/**
 * Platform-specific display symbols for Mac.
 */
const MAC_SYMBOLS: Record<string, string> = {
  Ctrl: '\u2318', // Command (Cmd maps to Ctrl in canonical form)
  Alt: '\u2325', // Option
  Shift: '\u21E7', // Shift
  Enter: '\u21A9', // Return
  Backspace: '\u232B', // Delete backward
  Delete: '\u2326', // Delete forward
  Escape: '\u238B', // Escape
  Tab: '\u21E5', // Tab
  ArrowUp: '\u2191', // Up arrow
  ArrowDown: '\u2193', // Down arrow
  ArrowLeft: '\u2190', // Left arrow
  ArrowRight: '\u2192', // Right arrow
  Space: 'Space', // Keep as text for clarity
};

/**
 * Symbol keys shown as glyphs on all platforms (e.g. "Ctrl+Equal" -> "Ctrl+=").
 * Applied after normalization so the canonical form stays "Equal"/"Minus"/"Plus".
 */
const DISPLAY_SYMBOLS: Record<string, string> = {
  Equal: '=',
  Plus: '+',
  Minus: '-',
};

/**
 * Format a shortcut for display using platform-appropriate symbols.
 * - Mac: Uses symbols (e.g., "Cmd+Shift+S")
 * - Windows/Linux: Uses text (e.g., "Ctrl+Shift+S")
 *
 * @param shortcut - Shortcut string (will be normalized first)
 * @param forceMac - Force Mac-style display (for testing or cross-platform preview)
 * @returns Formatted display string
 */
export function formatShortcut(shortcut: string, forceMac?: boolean): string {
  const normalized = normalizeShortcut(shortcut);
  if (normalized === null) {
    return shortcut; // Return original if can't normalize
  }

  const useMacSymbols = forceMac ?? isMac();

  if (!useMacSymbols) {
    // Windows/Linux: normalized form, with symbol keys shown as glyphs (=, -, +)
    return normalized
      .split('+')
      .map((part) => DISPLAY_SYMBOLS[part] ?? part)
      .join('+');
  }

  // Mac: replace modifiers and special keys with symbols
  const parts = normalized.split('+');
  const formatted = parts.map((part) => {
    return MAC_SYMBOLS[part] ?? DISPLAY_SYMBOLS[part] ?? part;
  });

  // Mac style: symbols without separators for modifiers, then key
  // e.g., Ctrl+Shift+S -> Cmd+Shift+S
  return formatted.join('');
}

/**
 * Get the action name to display in human-readable format.
 * Converts snake_case action names to Title Case.
 *
 * @param action - Action name (e.g., "toggle_sections_panel")
 * @returns Formatted action name (e.g., "Toggle Sections Panel")
 */
const ACTION_NAME_KEYS: Record<string, string> = {
  new_file: 'settings.shortcuts.actions.newFile',
  open_file: 'settings.shortcuts.actions.openFile',
  save: 'settings.shortcuts.actions.save',
  save_as: 'settings.shortcuts.actions.saveAs',
  import_dmp: 'settings.shortcuts.actions.importDmp',
  export: 'settings.shortcuts.actions.export',
  select: 'settings.shortcuts.actions.select',
  pan: 'settings.shortcuts.actions.pan',
  zoom: 'settings.shortcuts.actions.zoom',
  merge: 'settings.shortcuts.actions.merge',
  measure: 'settings.shortcuts.actions.measure',
  move_station: 'settings.shortcuts.actions.moveStation',
  move_segment: 'settings.shortcuts.actions.moveSegment',
  move_shot: 'settings.shortcuts.actions.moveShot',
  undo: 'settings.shortcuts.actions.undo',
  redo: 'settings.shortcuts.actions.redo',
  toggle_depth_gradient: 'settings.shortcuts.actions.toggleDepthGradient',
  toggle_grid: 'settings.shortcuts.actions.toggleGrid',
  toggle_satellite: 'settings.shortcuts.actions.toggleSatellite',
  toggle_station_labels: 'settings.shortcuts.actions.toggleStationLabels',
  toggle_shot_labels: 'settings.shortcuts.actions.toggleShotLabels',
  toggle_line_labels: 'settings.shortcuts.actions.toggleSectionLabels',
  fit_to_bounds: 'settings.shortcuts.actions.fitToBounds',
  zoom_in: 'settings.shortcuts.actions.zoomIn',
  zoom_out: 'settings.shortcuts.actions.zoomOut',
  toggle_sections_panel: 'settings.shortcuts.actions.toggleSectionsPanel',
  toggle_detail_panel: 'settings.shortcuts.actions.toggleDetailPanel',
  toggle_data_table: 'settings.shortcuts.actions.toggleDataTable',
  comment_tool: 'settings.shortcuts.actions.commentTool',
  move_comment_tool: 'settings.shortcuts.actions.moveCommentTool',
  quit: 'settings.shortcuts.actions.quit',
};

export function formatActionName(action: string): string {
  const key = ACTION_NAME_KEYS[action];
  if (key) {
    const translated = i18n.t(key);
    if (translated && translated !== key && translated !== `[${key}]`) {
      return translated;
    }
  }
  return action
    .split('_')
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1))
    .join(' ');
}
