/**
 * Unit tests for keyboard shortcut utilities.
 * Tests verify that TypeScript canonicalization matches Rust behavior.
 */

import { describe, it, expect, afterEach } from 'vitest';
import {
  isMac,
  RESERVED_SHORTCUTS,
  normalizeShortcut,
  parseKeyEvent,
  isReservedShortcut,
  checkReservedConflict,
  findConflict,
  formatShortcut,
  formatActionName,
} from '../shortcuts';

describe('isMac', () => {
  const originalPlatform = navigator.platform;

  afterEach(() => {
    Object.defineProperty(navigator, 'platform', {
      value: originalPlatform,
      writable: true,
    });
  });

  it('returns true for Mac platform', () => {
    Object.defineProperty(navigator, 'platform', {
      value: 'MacIntel',
      writable: true,
    });
    expect(isMac()).toBe(true);
  });

  it('returns true for iPhone platform', () => {
    Object.defineProperty(navigator, 'platform', {
      value: 'iPhone',
      writable: true,
    });
    expect(isMac()).toBe(true);
  });

  it('returns true for iPad platform', () => {
    Object.defineProperty(navigator, 'platform', {
      value: 'iPad',
      writable: true,
    });
    expect(isMac()).toBe(true);
  });

  it('returns false for Windows platform', () => {
    Object.defineProperty(navigator, 'platform', {
      value: 'Win32',
      writable: true,
    });
    expect(isMac()).toBe(false);
  });

  it('returns false for Linux platform', () => {
    Object.defineProperty(navigator, 'platform', {
      value: 'Linux x86_64',
      writable: true,
    });
    expect(isMac()).toBe(false);
  });
});

describe('RESERVED_SHORTCUTS', () => {
  it('includes Escape', () => {
    expect(RESERVED_SHORTCUTS.has('Escape')).toBe(true);
  });

  it('includes Delete', () => {
    expect(RESERVED_SHORTCUTS.has('Delete')).toBe(true);
  });

  it('includes Backspace', () => {
    expect(RESERVED_SHORTCUTS.has('Backspace')).toBe(true);
  });

  it('includes Tab', () => {
    expect(RESERVED_SHORTCUTS.has('Tab')).toBe(true);
  });

  it('includes Enter', () => {
    expect(RESERVED_SHORTCUTS.has('Enter')).toBe(true);
  });

  it('includes Ctrl+A (Select All)', () => {
    expect(RESERVED_SHORTCUTS.has('Ctrl+A')).toBe(true);
  });

  it('does NOT include Ctrl+I (now configurable)', () => {
    expect(RESERVED_SHORTCUTS.has('Ctrl+I')).toBe(false);
  });

  it('does NOT include Ctrl+E (now configurable)', () => {
    expect(RESERVED_SHORTCUTS.has('Ctrl+E')).toBe(false);
  });

  it('includes Ctrl+, (Open Settings)', () => {
    expect(RESERVED_SHORTCUTS.has('Ctrl+,')).toBe(true);
  });

  it('has expected number of reserved shortcuts', () => {
    // Escape, Delete, Backspace, Tab, Enter, Ctrl+A, Ctrl+,
    expect(RESERVED_SHORTCUTS.size).toBe(7);
  });
});

describe('normalizeShortcut', () => {
  describe('simple keys', () => {
    it('normalizes single lowercase letter to UPPERCASE', () => {
      expect(normalizeShortcut('s')).toBe('S');
      expect(normalizeShortcut('a')).toBe('A');
      expect(normalizeShortcut('z')).toBe('Z');
    });

    it('preserves single uppercase letter', () => {
      expect(normalizeShortcut('S')).toBe('S');
      expect(normalizeShortcut('A')).toBe('A');
    });

    it('preserves numbers', () => {
      expect(normalizeShortcut('1')).toBe('1');
      expect(normalizeShortcut('0')).toBe('0');
      expect(normalizeShortcut('9')).toBe('9');
    });

    it('trims whitespace', () => {
      expect(normalizeShortcut('  s  ')).toBe('S');
      expect(normalizeShortcut('\tA\n')).toBe('A');
    });
  });

  describe('modifiers', () => {
    it('normalizes Ctrl+key', () => {
      expect(normalizeShortcut('Ctrl+S')).toBe('Ctrl+S');
      expect(normalizeShortcut('ctrl+s')).toBe('Ctrl+S');
      expect(normalizeShortcut('CTRL+S')).toBe('Ctrl+S');
    });

    it('normalizes control alias', () => {
      expect(normalizeShortcut('Control+S')).toBe('Ctrl+S');
    });

    it('normalizes Alt+key', () => {
      expect(normalizeShortcut('Alt+S')).toBe('Alt+S');
      expect(normalizeShortcut('alt+s')).toBe('Alt+S');
    });

    it('normalizes Option alias to Alt', () => {
      expect(normalizeShortcut('Option+S')).toBe('Alt+S');
    });

    it('normalizes Shift+key', () => {
      expect(normalizeShortcut('Shift+S')).toBe('Shift+S');
      expect(normalizeShortcut('shift+s')).toBe('Shift+S');
    });

    it('normalizes Meta/Cmd/Command to Ctrl', () => {
      expect(normalizeShortcut('Meta+S')).toBe('Ctrl+S');
      expect(normalizeShortcut('Cmd+S')).toBe('Ctrl+S');
      expect(normalizeShortcut('Command+S')).toBe('Ctrl+S');
    });
  });

  describe('modifier order', () => {
    it('sorts modifiers to Ctrl+Alt+Shift order', () => {
      expect(normalizeShortcut('Shift+Ctrl+S')).toBe('Ctrl+Shift+S');
      expect(normalizeShortcut('Alt+Ctrl+S')).toBe('Ctrl+Alt+S');
      expect(normalizeShortcut('Shift+Alt+Ctrl+S')).toBe('Ctrl+Alt+Shift+S');
    });

    it('handles all modifier combinations', () => {
      expect(normalizeShortcut('Ctrl+Alt+S')).toBe('Ctrl+Alt+S');
      expect(normalizeShortcut('Alt+Shift+S')).toBe('Alt+Shift+S');
      expect(normalizeShortcut('Ctrl+Shift+S')).toBe('Ctrl+Shift+S');
    });
  });

  describe('special keys', () => {
    it('normalizes space key', () => {
      expect(normalizeShortcut('space')).toBe('Space');
      expect(normalizeShortcut('Space')).toBe('Space');
      expect(normalizeShortcut('spacebar')).toBe('Space');
      expect(normalizeShortcut(' ')).toBe('Space');
    });

    it('normalizes enter key', () => {
      expect(normalizeShortcut('enter')).toBe('Enter');
      expect(normalizeShortcut('Enter')).toBe('Enter');
      expect(normalizeShortcut('return')).toBe('Enter');
    });

    it('normalizes escape key', () => {
      expect(normalizeShortcut('escape')).toBe('Escape');
      expect(normalizeShortcut('Escape')).toBe('Escape');
      expect(normalizeShortcut('esc')).toBe('Escape');
    });

    it('normalizes arrow keys', () => {
      expect(normalizeShortcut('ArrowUp')).toBe('ArrowUp');
      expect(normalizeShortcut('arrowup')).toBe('ArrowUp');
      expect(normalizeShortcut('up')).toBe('ArrowUp');
      expect(normalizeShortcut('ArrowDown')).toBe('ArrowDown');
      expect(normalizeShortcut('down')).toBe('ArrowDown');
      expect(normalizeShortcut('ArrowLeft')).toBe('ArrowLeft');
      expect(normalizeShortcut('left')).toBe('ArrowLeft');
      expect(normalizeShortcut('ArrowRight')).toBe('ArrowRight');
      expect(normalizeShortcut('right')).toBe('ArrowRight');
    });

    it('normalizes plus/minus/equal', () => {
      expect(normalizeShortcut('+')).toBe('Plus');
      expect(normalizeShortcut('plus')).toBe('Plus');
      expect(normalizeShortcut('-')).toBe('Minus');
      expect(normalizeShortcut('minus')).toBe('Minus');
      expect(normalizeShortcut('=')).toBe('Equal');
      expect(normalizeShortcut('equal')).toBe('Equal');
      expect(normalizeShortcut('equals')).toBe('Equal');
    });

    it('normalizes navigation keys', () => {
      expect(normalizeShortcut('backspace')).toBe('Backspace');
      expect(normalizeShortcut('back')).toBe('Backspace');
      expect(normalizeShortcut('delete')).toBe('Delete');
      expect(normalizeShortcut('del')).toBe('Delete');
      expect(normalizeShortcut('tab')).toBe('Tab');
      expect(normalizeShortcut('home')).toBe('Home');
      expect(normalizeShortcut('end')).toBe('End');
      expect(normalizeShortcut('pageup')).toBe('PageUp');
      expect(normalizeShortcut('pgup')).toBe('PageUp');
      expect(normalizeShortcut('pagedown')).toBe('PageDown');
      expect(normalizeShortcut('pgdn')).toBe('PageDown');
      expect(normalizeShortcut('insert')).toBe('Insert');
      expect(normalizeShortcut('ins')).toBe('Insert');
    });

    it('normalizes function keys', () => {
      expect(normalizeShortcut('f1')).toBe('F1');
      expect(normalizeShortcut('F1')).toBe('F1');
      expect(normalizeShortcut('f12')).toBe('F12');
      expect(normalizeShortcut('F12')).toBe('F12');
    });
  });

  describe('special cases for + key', () => {
    it('handles Ctrl++ as Ctrl+Plus', () => {
      expect(normalizeShortcut('Ctrl++')).toBe('Ctrl+Plus');
    });

    it('handles Ctrl+Shift++ as Ctrl+Shift+Plus', () => {
      expect(normalizeShortcut('Ctrl+Shift++')).toBe('Ctrl+Shift+Plus');
    });

    it('handles Ctrl+++ as Ctrl+Plus (multiple trailing +)', () => {
      expect(normalizeShortcut('Ctrl+++')).toBe('Ctrl+Plus');
    });
  });

  describe('invalid shortcuts', () => {
    it('returns null for empty string', () => {
      expect(normalizeShortcut('')).toBeNull();
    });

    it('returns null for whitespace only', () => {
      expect(normalizeShortcut('   ')).toBeNull();
    });

    it('returns null for multiple non-modifier keys', () => {
      expect(normalizeShortcut('A+B')).toBeNull();
      expect(normalizeShortcut('Ctrl+A+B')).toBeNull();
    });

    it('returns null for empty parts in middle', () => {
      expect(normalizeShortcut('Ctrl++S')).toBeNull();
    });

    it('returns null for Ctrl+Plus+Key (already has Plus as key)', () => {
      expect(normalizeShortcut('Ctrl+S++')).toBeNull();
    });
  });

  describe('trailing plus (matching Rust behavior)', () => {
    it('normalizes single trailing + as Plus', () => {
      // "Ctrl+" is valid and means Ctrl+Plus (matching Rust)
      expect(normalizeShortcut('Ctrl+')).toBe('Ctrl+Plus');
      expect(normalizeShortcut('Shift+')).toBe('Shift+Plus');
      expect(normalizeShortcut('Alt+')).toBe('Alt+Plus');
    });

    it('normalizes double trailing ++ as Plus', () => {
      // "Ctrl++" is also valid and means Ctrl+Plus
      expect(normalizeShortcut('Ctrl++')).toBe('Ctrl+Plus');
      expect(normalizeShortcut('Shift++')).toBe('Shift+Plus');
    });

    it('handles multiple modifiers with trailing +', () => {
      expect(normalizeShortcut('Ctrl+Shift+')).toBe('Ctrl+Shift+Plus');
      expect(normalizeShortcut('Ctrl+Shift++')).toBe('Ctrl+Shift+Plus');
      expect(normalizeShortcut('Ctrl+Alt+Shift+')).toBe('Ctrl+Alt+Shift+Plus');
    });
  });
});

describe('parseKeyEvent', () => {
  function createKeyEvent(
    key: string,
    options: { ctrlKey?: boolean; altKey?: boolean; shiftKey?: boolean; metaKey?: boolean } = {}
  ): KeyboardEvent {
    return {
      key,
      ctrlKey: options.ctrlKey ?? false,
      altKey: options.altKey ?? false,
      shiftKey: options.shiftKey ?? false,
      metaKey: options.metaKey ?? false,
    } as KeyboardEvent;
  }

  it('parses simple key', () => {
    expect(parseKeyEvent(createKeyEvent('s'))).toBe('S');
    expect(parseKeyEvent(createKeyEvent('S'))).toBe('S');
    expect(parseKeyEvent(createKeyEvent('1'))).toBe('1');
  });

  it('parses Ctrl+key', () => {
    expect(parseKeyEvent(createKeyEvent('s', { ctrlKey: true }))).toBe('Ctrl+S');
  });

  it('parses Meta+key as Ctrl', () => {
    expect(parseKeyEvent(createKeyEvent('s', { metaKey: true }))).toBe('Ctrl+S');
  });

  it('parses Alt+key', () => {
    expect(parseKeyEvent(createKeyEvent('s', { altKey: true }))).toBe('Alt+S');
  });

  it('parses Shift+key', () => {
    expect(parseKeyEvent(createKeyEvent('s', { shiftKey: true }))).toBe('Shift+S');
  });

  it('parses multiple modifiers in canonical order', () => {
    expect(parseKeyEvent(createKeyEvent('s', { ctrlKey: true, shiftKey: true }))).toBe('Ctrl+Shift+S');
    expect(parseKeyEvent(createKeyEvent('s', { ctrlKey: true, altKey: true }))).toBe('Ctrl+Alt+S');
    expect(parseKeyEvent(createKeyEvent('s', { altKey: true, shiftKey: true }))).toBe('Alt+Shift+S');
    expect(parseKeyEvent(createKeyEvent('s', { ctrlKey: true, altKey: true, shiftKey: true }))).toBe(
      'Ctrl+Alt+Shift+S'
    );
  });

  it('parses special keys', () => {
    expect(parseKeyEvent(createKeyEvent(' '))).toBe('Space');
    expect(parseKeyEvent(createKeyEvent('Enter'))).toBe('Enter');
    expect(parseKeyEvent(createKeyEvent('Escape'))).toBe('Escape');
    expect(parseKeyEvent(createKeyEvent('ArrowUp'))).toBe('ArrowUp');
    expect(parseKeyEvent(createKeyEvent('ArrowDown'))).toBe('ArrowDown');
    expect(parseKeyEvent(createKeyEvent('+'))).toBe('Plus');
    expect(parseKeyEvent(createKeyEvent('-'))).toBe('Minus');
    expect(parseKeyEvent(createKeyEvent('='))).toBe('Equal');
  });

  it('ignores modifier-only key presses', () => {
    expect(parseKeyEvent(createKeyEvent('Control', { ctrlKey: true }))).toBe('Ctrl');
    expect(parseKeyEvent(createKeyEvent('Shift', { shiftKey: true }))).toBe('Shift');
    expect(parseKeyEvent(createKeyEvent('Alt', { altKey: true }))).toBe('Alt');
    expect(parseKeyEvent(createKeyEvent('Meta', { metaKey: true }))).toBe('Ctrl');
  });

  it('parses Ctrl+space', () => {
    expect(parseKeyEvent(createKeyEvent(' ', { ctrlKey: true }))).toBe('Ctrl+Space');
  });
});

describe('isReservedShortcut', () => {
  it('returns true for Escape', () => {
    expect(isReservedShortcut('Escape')).toBe(true);
    expect(isReservedShortcut('escape')).toBe(true);
    expect(isReservedShortcut('esc')).toBe(true);
  });

  it('returns true for Delete', () => {
    expect(isReservedShortcut('Delete')).toBe(true);
    expect(isReservedShortcut('delete')).toBe(true);
    expect(isReservedShortcut('del')).toBe(true);
  });

  it('returns true for Backspace', () => {
    expect(isReservedShortcut('Backspace')).toBe(true);
    expect(isReservedShortcut('backspace')).toBe(true);
    expect(isReservedShortcut('back')).toBe(true);
  });

  it('returns true for Tab', () => {
    expect(isReservedShortcut('Tab')).toBe(true);
    expect(isReservedShortcut('tab')).toBe(true);
  });

  it('returns true for Enter', () => {
    expect(isReservedShortcut('Enter')).toBe(true);
    expect(isReservedShortcut('enter')).toBe(true);
    expect(isReservedShortcut('return')).toBe(true);
  });

  it('returns true for Ctrl+A', () => {
    expect(isReservedShortcut('Ctrl+A')).toBe(true);
    expect(isReservedShortcut('ctrl+a')).toBe(true);
  });

  it('returns false for Ctrl+I (now configurable)', () => {
    expect(isReservedShortcut('Ctrl+I')).toBe(false);
    expect(isReservedShortcut('ctrl+i')).toBe(false);
    expect(isReservedShortcut('Meta+I')).toBe(false);
  });

  it('returns false for Ctrl+E (now configurable)', () => {
    expect(isReservedShortcut('Ctrl+E')).toBe(false);
    expect(isReservedShortcut('ctrl+e')).toBe(false);
    expect(isReservedShortcut('Meta+E')).toBe(false);
  });

  it('returns false for non-reserved shortcuts', () => {
    expect(isReservedShortcut('S')).toBe(false);
    expect(isReservedShortcut('Ctrl+S')).toBe(false);
    expect(isReservedShortcut('Ctrl+Z')).toBe(false);
    expect(isReservedShortcut('F1')).toBe(false);
  });

  it('returns false for invalid shortcuts', () => {
    expect(isReservedShortcut('')).toBe(false);
    expect(isReservedShortcut('A+B')).toBe(false);
  });
});

describe('checkReservedConflict', () => {
  it('returns conflict for reserved keys (Escape, Delete, Backspace)', () => {
    expect(checkReservedConflict('Escape')).toEqual({ isReserved: true, reason: 'Cancel / Close dialogs' });
    expect(checkReservedConflict('Delete')).toEqual({ isReserved: true, reason: 'Delete selection' });
    expect(checkReservedConflict('Backspace')).toEqual({ isReserved: true, reason: 'Delete selection' });
  });

  it('returns conflict for Tab and Enter (UI Navigation)', () => {
    expect(checkReservedConflict('Tab')).toEqual({ isReserved: true, reason: 'UI Navigation' });
    expect(checkReservedConflict('Enter')).toEqual({ isReserved: true, reason: 'UI Navigation' });
  });

  it('blocks reserved keys even with modifiers', () => {
    expect(checkReservedConflict('Ctrl+Escape')).toEqual({ isReserved: true, reason: 'Cancel / Close dialogs' });
    expect(checkReservedConflict('Ctrl+Delete')).toEqual({ isReserved: true, reason: 'Delete selection' });
    expect(checkReservedConflict('Shift+Backspace')).toEqual({ isReserved: true, reason: 'Delete selection' });
    expect(checkReservedConflict('Ctrl+Tab')).toEqual({ isReserved: true, reason: 'UI Navigation' });
    expect(checkReservedConflict('Shift+Enter')).toEqual({ isReserved: true, reason: 'UI Navigation' });
  });

  it('returns conflict for Ctrl+A (Select all)', () => {
    expect(checkReservedConflict('Ctrl+A')).toEqual({ isReserved: true, reason: 'Select all' });
  });

  it('returns conflict for Ctrl+, (Open Settings)', () => {
    expect(checkReservedConflict('Ctrl+,')).toEqual({ isReserved: true, reason: 'Open Settings' });
  });

  it('blocks Ctrl+A even with extra modifiers', () => {
    expect(checkReservedConflict('Ctrl+Shift+A')).toEqual({ isReserved: true, reason: 'Select all' });
    expect(checkReservedConflict('Ctrl+Alt+,')).toEqual({ isReserved: true, reason: 'Open Settings' });
  });

  it('returns conflict for the UI-zoom keys (#319)', () => {
    // useUiScale listens for these unconditionally, so binding them to an
    // action would fire both handlers.
    expect(checkReservedConflict('Ctrl+Equal')).toEqual({ isReserved: true, reason: 'Zoom interface in' });
    expect(checkReservedConflict('Ctrl+Plus')).toEqual({ isReserved: true, reason: 'Zoom interface in' });
    expect(checkReservedConflict('Ctrl+Minus')).toEqual({ isReserved: true, reason: 'Zoom interface out' });
    expect(checkReservedConflict('Ctrl+0')).toEqual({ isReserved: true, reason: 'Reset interface zoom' });
  });

  it("blocks Shift+'-', which normalizes to Ctrl+Shift+_ (#319)", () => {
    expect(checkReservedConflict('Ctrl+Shift+_')).toEqual({
      isReserved: true,
      reason: 'Zoom interface out',
    });
  });

  it('blocks the UI-zoom keys even with extra modifiers (#319)', () => {
    expect(checkReservedConflict('Ctrl+Alt+Equal')).toEqual({ isReserved: true, reason: 'Zoom interface in' });
    expect(checkReservedConflict('Ctrl+Shift+0')).toEqual({
      isReserved: true,
      reason: 'Reset interface zoom',
    });
  });

  it('leaves the bare zoom keys bindable - the map-zoom defaults use them (#319)', () => {
    expect(checkReservedConflict('Equal')).toBeNull();
    expect(checkReservedConflict('Plus')).toBeNull();
    expect(checkReservedConflict('Minus')).toBeNull();
    // The raw glyphs normalize to the same keys and must stay bindable too.
    expect(checkReservedConflict('=')).toBeNull();
    expect(checkReservedConflict('+')).toBeNull();
    expect(checkReservedConflict('-')).toBeNull();
    // Only the Ctrl branch is reserved, so Shift/Alt alone are fine.
    expect(checkReservedConflict('Shift+Equal')).toBeNull();
    expect(checkReservedConflict('Alt+Minus')).toBeNull();
    expect(checkReservedConflict('0')).toBeNull();
  });

  it('returns null for Ctrl+I (now configurable)', () => {
    expect(checkReservedConflict('Ctrl+I')).toBeNull();
    expect(checkReservedConflict('ctrl+i')).toBeNull();
    expect(checkReservedConflict('Meta+I')).toBeNull();
  });

  it('returns null for Ctrl+E (now configurable)', () => {
    expect(checkReservedConflict('Ctrl+E')).toBeNull();
    expect(checkReservedConflict('ctrl+e')).toBeNull();
    expect(checkReservedConflict('Meta+E')).toBeNull();
  });

  it('returns null for non-reserved shortcuts', () => {
    expect(checkReservedConflict('S')).toBeNull();
    expect(checkReservedConflict('Ctrl+S')).toBeNull();
    expect(checkReservedConflict('Ctrl+Z')).toBeNull();
    expect(checkReservedConflict('F1')).toBeNull();
  });

  it('returns null for invalid shortcuts', () => {
    expect(checkReservedConflict('')).toBeNull();
    expect(checkReservedConflict('A+B')).toBeNull();
  });
});

describe('findConflict', () => {
  it('returns null when no conflict exists', () => {
    const registry = new Map([
      ['select', ['S']],
      ['pan', ['P']],
    ]);
    expect(findConflict('D', registry)).toBeNull();
    expect(findConflict('Ctrl+S', registry)).toBeNull();
  });

  it('finds conflict with existing shortcut', () => {
    const registry = new Map([
      ['select', ['S']],
      ['pan', ['P', 'Shift+P']],
    ]);
    expect(findConflict('S', registry)).toBe('select');
    expect(findConflict('s', registry)).toBe('select');
    expect(findConflict('P', registry)).toBe('pan');
    expect(findConflict('Shift+P', registry)).toBe('pan');
  });

  it('normalizes shortcut before checking', () => {
    const registry = new Map([['undo', ['Ctrl+Z']]]);
    expect(findConflict('ctrl+z', registry)).toBe('undo');
    expect(findConflict('Meta+Z', registry)).toBe('undo');
  });

  it('excludes specified action from conflict check', () => {
    const registry = new Map([
      ['select', ['S']],
      ['pan', ['P']],
    ]);
    // Editing "select" action - S should not conflict with itself
    expect(findConflict('S', registry, 'select')).toBeNull();
    // But P still conflicts with "pan"
    expect(findConflict('P', registry, 'select')).toBe('pan');
  });

  it('returns null for invalid shortcut', () => {
    const registry = new Map([['select', ['S']]]);
    expect(findConflict('', registry)).toBeNull();
    expect(findConflict('A+B', registry)).toBeNull();
  });

  it('handles empty registry', () => {
    const registry = new Map<string, string[]>();
    expect(findConflict('S', registry)).toBeNull();
  });

  it('handles multiple shortcuts per action', () => {
    const registry = new Map([['redo', ['Ctrl+Y', 'Ctrl+Shift+Z']]]);
    expect(findConflict('Ctrl+Y', registry)).toBe('redo');
    expect(findConflict('Ctrl+Shift+Z', registry)).toBe('redo');
    expect(findConflict('Ctrl+Z', registry)).toBeNull();
  });
});

describe('formatShortcut', () => {
  describe('Windows/Linux format', () => {
    it('returns normalized shortcut as-is', () => {
      expect(formatShortcut('Ctrl+S', false)).toBe('Ctrl+S');
      expect(formatShortcut('ctrl+s', false)).toBe('Ctrl+S');
      expect(formatShortcut('Ctrl+Shift+S', false)).toBe('Ctrl+Shift+S');
    });

    it('handles special keys', () => {
      expect(formatShortcut('Escape', false)).toBe('Escape');
      expect(formatShortcut('ArrowUp', false)).toBe('ArrowUp');
      expect(formatShortcut('Space', false)).toBe('Space');
    });
  });

  describe('Mac format', () => {
    it('uses symbols for modifiers', () => {
      expect(formatShortcut('Ctrl+S', true)).toBe('\u2318S');
      expect(formatShortcut('Alt+S', true)).toBe('\u2325S');
      expect(formatShortcut('Shift+S', true)).toBe('\u21E7S');
    });

    it('uses symbols for multiple modifiers', () => {
      expect(formatShortcut('Ctrl+Shift+S', true)).toBe('\u2318\u21E7S');
      expect(formatShortcut('Ctrl+Alt+S', true)).toBe('\u2318\u2325S');
      expect(formatShortcut('Ctrl+Alt+Shift+S', true)).toBe('\u2318\u2325\u21E7S');
    });

    it('uses symbols for special keys', () => {
      expect(formatShortcut('Escape', true)).toBe('\u238B');
      expect(formatShortcut('Enter', true)).toBe('\u21A9');
      expect(formatShortcut('Backspace', true)).toBe('\u232B');
      expect(formatShortcut('Delete', true)).toBe('\u2326');
      expect(formatShortcut('Tab', true)).toBe('\u21E5');
      expect(formatShortcut('ArrowUp', true)).toBe('\u2191');
      expect(formatShortcut('ArrowDown', true)).toBe('\u2193');
      expect(formatShortcut('ArrowLeft', true)).toBe('\u2190');
      expect(formatShortcut('ArrowRight', true)).toBe('\u2192');
    });

    it('preserves Space as text for clarity', () => {
      expect(formatShortcut('Space', true)).toBe('Space');
      expect(formatShortcut('Ctrl+Space', true)).toBe('\u2318Space');
    });
  });

  it('returns original string for invalid shortcuts', () => {
    expect(formatShortcut('')).toBe('');
    expect(formatShortcut('A+B')).toBe('A+B');
  });
});

describe('formatActionName', () => {
  it('converts snake_case to Title Case', () => {
    expect(formatActionName('select')).toBe('Select');
    expect(formatActionName('toggle_sections_panel')).toBe('Toggle Sections Panel');
    expect(formatActionName('move_station')).toBe('Move Station');
    expect(formatActionName('zoom_in')).toBe('Zoom In');
  });

  it('handles single word', () => {
    expect(formatActionName('pan')).toBe('Pan');
    expect(formatActionName('zoom')).toBe('Zoom');
    expect(formatActionName('merge')).toBe('Merge');
  });

  it('handles multiple underscores', () => {
    expect(formatActionName('toggle_depth_gradient')).toBe('Toggle Depth Gradient');
    // A translated action uses its i18n name (the L key toggles the station IDs).
    expect(formatActionName('toggle_station_labels')).toBe('Toggle Station IDs');
    // An untranslated one falls back to title-casing its words.
    expect(formatActionName('some_unknown_action')).toBe('Some Unknown Action');
  });
});

describe('Rust canonicalization compatibility', () => {
  // These tests verify TypeScript matches Rust normalize_shortcut() behavior
  // Tests are named to match the Rust test functions

  it('test_normalize_shortcut_simple_key', () => {
    expect(normalizeShortcut('S')).toBe('S');
    expect(normalizeShortcut('s')).toBe('S');
    expect(normalizeShortcut('1')).toBe('1');
  });

  it('test_normalize_shortcut_with_modifiers', () => {
    expect(normalizeShortcut('Ctrl+S')).toBe('Ctrl+S');
    expect(normalizeShortcut('ctrl+s')).toBe('Ctrl+S');
    expect(normalizeShortcut('CTRL+S')).toBe('Ctrl+S');
  });

  it('test_normalize_shortcut_modifier_order', () => {
    expect(normalizeShortcut('Shift+Ctrl+S')).toBe('Ctrl+Shift+S');
    expect(normalizeShortcut('Alt+Ctrl+S')).toBe('Ctrl+Alt+S');
    expect(normalizeShortcut('Shift+Alt+Ctrl+S')).toBe('Ctrl+Alt+Shift+S');
  });

  it('test_normalize_shortcut_platform_aliases', () => {
    expect(normalizeShortcut('Meta+S')).toBe('Ctrl+S');
    expect(normalizeShortcut('Cmd+S')).toBe('Ctrl+S');
    expect(normalizeShortcut('Command+S')).toBe('Ctrl+S');
    expect(normalizeShortcut('Option+S')).toBe('Alt+S');
  });

  it('test_normalize_shortcut_special_keys', () => {
    expect(normalizeShortcut('space')).toBe('Space');
    expect(normalizeShortcut('enter')).toBe('Enter');
    expect(normalizeShortcut('return')).toBe('Enter');
    expect(normalizeShortcut('escape')).toBe('Escape');
    expect(normalizeShortcut('esc')).toBe('Escape');
    expect(normalizeShortcut('arrowup')).toBe('ArrowUp');
    expect(normalizeShortcut('up')).toBe('ArrowUp');
  });

  it('test_normalize_shortcut_plus_minus_equal', () => {
    expect(normalizeShortcut('+')).toBe('Plus');
    expect(normalizeShortcut('-')).toBe('Minus');
    expect(normalizeShortcut('=')).toBe('Equal');
    expect(normalizeShortcut('plus')).toBe('Plus');
    expect(normalizeShortcut('minus')).toBe('Minus');
    expect(normalizeShortcut('equal')).toBe('Equal');
  });

  it('test_normalize_shortcut_ctrl_plus', () => {
    expect(normalizeShortcut('Ctrl++')).toBe('Ctrl+Plus');
    expect(normalizeShortcut('Ctrl+Shift++')).toBe('Ctrl+Shift+Plus');
  });

  it('test_normalize_shortcut_invalid', () => {
    expect(normalizeShortcut('')).toBeNull();
    expect(normalizeShortcut('   ')).toBeNull();
    expect(normalizeShortcut('Ctrl++S')).toBeNull(); // Empty in middle
    expect(normalizeShortcut('A+B')).toBeNull(); // Multiple keys
  });

  it('test_normalize_shortcut_function_keys', () => {
    expect(normalizeShortcut('f1')).toBe('F1');
    expect(normalizeShortcut('F1')).toBe('F1');
    expect(normalizeShortcut('f12')).toBe('F12');
    expect(normalizeShortcut('Ctrl+F5')).toBe('Ctrl+F5');
    // Rust accepts any F<n> that parses (no upper bound)
    expect(normalizeShortcut('F13')).toBe('F13');
    expect(normalizeShortcut('f24')).toBe('F24');
  });

  it('test_normalize_shortcut_trailing_plus', () => {
    // Matching Rust's test_normalize_shortcut_trailing_plus
    // "Ctrl+" is valid and means Ctrl+Plus
    expect(normalizeShortcut('Ctrl+')).toBe('Ctrl+Plus');
    expect(normalizeShortcut('Shift+')).toBe('Shift+Plus');
    // "Ctrl++" is also valid and means Ctrl+Plus (common representation)
    expect(normalizeShortcut('Ctrl++')).toBe('Ctrl+Plus');
    expect(normalizeShortcut('Shift++')).toBe('Shift+Plus');
    // Multiple modifiers with trailing +
    expect(normalizeShortcut('Ctrl+Shift+')).toBe('Ctrl+Shift+Plus');
    expect(normalizeShortcut('Ctrl+Shift++')).toBe('Ctrl+Shift+Plus');
  });
});
