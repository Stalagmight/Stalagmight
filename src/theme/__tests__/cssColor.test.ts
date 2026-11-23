import { describe, expect, it } from 'vitest';
import {
  CSS_COLOR_NAMES,
  contrastOfHex,
  flattenOver,
  hexOfOklch,
  hexOfRgb,
  oklchOfHex,
  parseCssColor,
} from '../cssColor';
import { contrastRatio } from '../../utils/colors';

const hexOf = (s: string) => {
  const c = parseCssColor(s);
  return c ? hexOfRgb(c.r, c.g, c.b) : null;
};

describe('parseCssColor', () => {
  it('reads hex in every length', () => {
    expect(hexOf('#abc')).toBe('#aabbcc');
    expect(hexOf('#ABCD')).toBe('#aabbcc');
    expect(parseCssColor('#abcd')!.a).toBeCloseTo(0xdd / 255);
    expect(hexOf('#4ECDC4')).toBe('#4ecdc4');
    expect(parseCssColor('#4ecdc480')!.a).toBeCloseTo(0x80 / 255);
    expect(parseCssColor('#12')).toBeNull();
    expect(parseCssColor('#ggg')).toBeNull();
  });

  it('reads rgb() and rgba(), comma and space syntax, numbers and percentages', () => {
    expect(hexOf('rgb(255, 0, 128)')).toBe('#ff0080');
    expect(hexOf('rgb(255 0 128)')).toBe('#ff0080');
    expect(hexOf('rgb(100% 0% 50%)')).toBe('#ff0080');
    expect(parseCssColor('rgba(0, 0, 255, 0.3)')!.a).toBeCloseTo(0.3);
    expect(parseCssColor('rgb(0 0 255 / 30%)')!.a).toBeCloseTo(0.3);
  });

  it('reads hsl(), hwb(), oklab() and oklch()', () => {
    expect(hexOf('hsl(60, 100%, 50%)')).toBe('#ffff00');
    expect(hexOf('hsl(0.5turn 100% 50%)')).toBe('#00ffff');
    expect(hexOf('hwb(0 0% 0%)')).toBe('#ff0000');
    expect(hexOf('hwb(0 50% 50%)')).toBe('#808080');
    expect(hexOf('oklab(1 0 0)')).toBe('#ffffff');
    expect(hexOf('oklch(0 0 0)')).toBe('#000000');
    expect(hexOf('oklch(62.8% 0.2577 29.23)')).toBe('#ff0000');
  });

  it('reads every CSS name and transparent', () => {
    expect(CSS_COLOR_NAMES).toHaveLength(148);
    for (const name of CSS_COLOR_NAMES) expect(parseCssColor(name), name).not.toBeNull();
    expect(hexOf('RebeccaPurple')).toBe('#663399');
    expect(parseCssColor('transparent')!.a).toBe(0);
  });

  it('is null for what it cannot paint', () => {
    for (const bad of [null, undefined, '', 'not-a-colour', 'lab(50 20 20)', 'rgb(1, 2)', 'rgb(a b c)']) {
      expect(parseCssColor(bad as string), String(bad)).toBeNull();
    }
  });
});

describe('OKLab maths', () => {
  it('round-trips sRGB through OKLCH', () => {
    for (const hex of ['#000000', '#ffffff', '#4ecdc4', '#ff0000', '#0000ff', '#ffff00', '#808080', '#16213e']) {
      expect(hexOfOklch(oklchOfHex(hex))).toBe(hex);
    }
  });

  it('known values: white L 1, black L 0, a grey has no chroma', () => {
    expect(oklchOfHex('#ffffff').L).toBeCloseTo(1, 4);
    expect(oklchOfHex('#000000').L).toBeCloseTo(0, 4);
    expect(oklchOfHex('#808080').C).toBeLessThan(1e-4);
    // sRGB blue: OKLCH(0.452, 0.313, 264.05).
    const blue = oklchOfHex('#0000ff');
    expect(blue.L).toBeCloseTo(0.452, 2);
    expect(blue.C).toBeCloseTo(0.313, 2);
    expect(blue.h).toBeCloseTo(264.05, 0);
  });

  it('maps an out-of-gamut colour into sRGB by lowering chroma, keeping lightness', () => {
    const hex = hexOfOklch({ L: 0.95, C: 0.4, h: 264 });
    expect(hex).toMatch(/^#[0-9a-f]{6}$/);
    expect(oklchOfHex(hex).L).toBeCloseTo(0.95, 2);
  });

  it('contrast agrees with utils/colors', () => {
    for (const [a, b] of [['#000000', '#ffffff'], ['#4ecdc4', '#16213e'], ['#6b5b4a', '#ffffff']] as const) {
      expect(contrastOfHex(a, b)).toBeCloseTo(contrastRatio(a, b), 2);
    }
  });

  it('flattens a translucent colour over the panel', () => {
    expect(flattenOver({ r: 1, g: 1, b: 1, a: 0.5 }, '#000000')).toBe('#808080');
    expect(flattenOver({ r: 1, g: 0, b: 0, a: 1 }, '#ffffff')).toBe('#ff0000');
  });
});
