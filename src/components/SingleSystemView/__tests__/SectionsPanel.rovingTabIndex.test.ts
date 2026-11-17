import { describe, expect, it } from 'vitest';
import { rovingDefaultTabIndex } from '../SectionsPanel';

/**
 * Roving-tabindex owner for the virtualized lines list. The default keyboard
 * Tab stop must sit on a MOUNTED row, or a list scrolled past index 0 (its row
 * unmounted) leaves every mounted row `tabIndex={-1}` and unreachable by Tab.
 */
describe('rovingDefaultTabIndex', () => {
  it('gives the Tab stop to the first mounted row when nothing is focused/selected', () => {
    // Window scrolled past the top: rows 0-4 are unmounted, 5-9 mounted.
    const mounted = [5, 6, 7, 8, 9];
    expect(rovingDefaultTabIndex(-1, mounted, -1)).toBe(5);
  });

  it('regression: a window past index 0 still owns a mounted tab stop (not the unmounted 0)', () => {
    const mounted = [12, 13, 14, 15];
    const owner = rovingDefaultTabIndex(-1, mounted, -1);
    expect(owner).not.toBe(0);
    expect(mounted).toContain(owner);
  });

  it('prefers the selected row when it is within the mounted window', () => {
    const mounted = [5, 6, 7, 8, 9];
    expect(rovingDefaultTabIndex(-1, mounted, 7)).toBe(7);
  });

  it('falls back to the first mounted row when the selected row is not mounted', () => {
    const mounted = [5, 6, 7, 8, 9];
    expect(rovingDefaultTabIndex(-1, mounted, 2)).toBe(5);
  });

  it('yields no default owner while a MOUNTED row is focused (the focused row keeps the stop)', () => {
    const mounted = [5, 6, 7, 8, 9];
    expect(rovingDefaultTabIndex(6, mounted, 7)).toBe(-1);
  });

  it('regression: a focused row scrolled out of the window still exposes a default stop', () => {
    // focusedIndex 3 but the window is 5-9 (row 3 unmounted, blur not fired):
    // must NOT return -1, or every mounted row is tabIndex=-1 and Tab is trapped.
    const mounted = [5, 6, 7, 8, 9];
    expect(rovingDefaultTabIndex(3, mounted, -1)).toBe(5);
    expect(rovingDefaultTabIndex(3, mounted, 7)).toBe(7);
  });

  it('yields no owner when nothing is mounted', () => {
    expect(rovingDefaultTabIndex(-1, [], -1)).toBe(-1);
  });

  it('picks the lowest mounted index regardless of ordering', () => {
    expect(rovingDefaultTabIndex(-1, [9, 5, 7, 6, 8], -1)).toBe(5);
  });

  it('keeps index 0 as the owner in the common top-of-list case', () => {
    // Top of the list mounted, nothing selected -> matches the old behavior.
    expect(rovingDefaultTabIndex(-1, [0, 1, 2, 3], -1)).toBe(0);
  });
});
