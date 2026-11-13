// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import { hexColorFor } from '../SectionsPanel';

/**
 * The section color dot seeds a native color input, which can only hold
 * `#rrggbb`. A section may carry any CSS color (Edit Section's free-text field
 * accepts one), so the seed is read back from the painted dot.
 */
describe('hexColorFor', () => {
  it('keeps a #rrggbb color, lowercased like the input reports it', () => {
    expect(hexColorFor('#45B7D1', null)).toBe('#45b7d1');
  });

  it('reads any other color back from what the element paints', () => {
    const dot = document.createElement('span');
    dot.style.backgroundColor = 'rgb(255, 0, 0)';
    document.body.appendChild(dot);
    expect(hexColorFor('red', dot)).toBe('#ff0000');
    dot.style.backgroundColor = 'rgba(0, 128, 255, 0.5)';
    expect(hexColorFor('#08f8', dot)).toBe('#0080ff');
    dot.remove();
  });

  it('falls back to black, as the input itself would, when nothing is painted', () => {
    expect(hexColorFor('#abc', null)).toBe('#000000');
    const dot = document.createElement('span');
    document.body.appendChild(dot);
    expect(hexColorFor('not-a-color', dot)).toBe('#000000');
    dot.remove();
  });
});
