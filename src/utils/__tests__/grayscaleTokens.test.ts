/**
 * #576 NF-W12DMP-1: the placement-mode dim's gray without a CSS filter. `grayscale(1)` maps
 * each sRGB pixel to its luminance with fixed weights, so a colour token desaturated the same
 * way paints what the filter painted (the screenshot comparison is in the #576 report).
 */
import { describe, expect, it } from 'vitest';
import { colorPropertyNames, grayscaleColor, grayscaleTokenStyle } from '../grayscaleTokens';
import { themeTokens } from '../../theme/tokens';
import { SYNTACTIC_ROLES, syntacticVarName } from '../../theme/syntacticPalette';

/** The Filter Effects grayscale matrix at amount 1, applied to one channel triple. */
const filterLuma = (r: number, g: number, b: number) => 0.2126 * r + 0.7152 * g + 0.0722 * b;
const lumaOf = (gray: string | null) => Number(/rgb\(([\d.]+) /.exec(gray ?? '')?.[1]);

describe('grayscaleColor', () => {
  it.each([
    ['#4ecdc4', [78, 205, 196]],
    ['#FFF', [255, 255, 255]],
    ['rgb(15, 52, 96)', [15, 52, 96]],
    ['rgb(15 52 96)', [15, 52, 96]],
  ] as const)('%s takes the luminance grayscale(1) gives it', (value, [r, g, b]) => {
    const gray = grayscaleColor(value);
    expect(gray).toMatch(/^rgb\([\d.]+ [\d.]+ [\d.]+\)$/);
    expect(lumaOf(gray)).toBeCloseTo(filterLuma(r, g, b), 2);
  });

  it('keeps the alpha of a translucent colour', () => {
    expect(grayscaleColor('rgba(78, 205, 196, 0.3)')).toBe(`rgb(${Math.round(filterLuma(78, 205, 196) * 1000) / 1000} ${Math.round(filterLuma(78, 205, 196) * 1000) / 1000} ${Math.round(filterLuma(78, 205, 196) * 1000) / 1000} / 0.3)`);
    expect(grayscaleColor('#00000080')).toBe('rgb(0 0 0 / 0.502)');
    expect(grayscaleColor('transparent')).toBe('rgb(0 0 0 / 0)');
  });

  it('leaves out what is not a plain colour', () => {
    for (const value of ['', 'none', 'var(--cc-accent)', 'hsl(10 20% 30%)', 'rgb(10%, 20%, 30%)', '#12345']) {
      expect(grayscaleColor(value), value).toBeNull();
    }
  });
});

describe('grayscaleTokenStyle', () => {
  it('covers every theme token and every syntactic role', () => {
    const names = colorPropertyNames();
    expect(names).toEqual(expect.arrayContaining(Object.keys(themeTokens.dark)));
    expect(names).toEqual(expect.arrayContaining(SYNTACTIC_ROLES.map(({ id }) => syntacticVarName(id))));
  });

  it('desaturates each colour the root resolves, and only those', () => {
    const root: Record<string, string> = { '--cc-accent': ' #4ecdc4', '--sh-section': '#a335ee', '--cc-panel': '' };
    const style = grayscaleTokenStyle((name) => root[name] ?? '');
    expect(Object.keys(style).sort()).toEqual(['--cc-accent', '--sh-section']);
    expect(lumaOf(style['--sh-section'] ?? null)).toBeCloseTo(filterLuma(0xa3, 0x35, 0xee), 2);
  });

  it('turns every colour token of every theme gray', () => {
    for (const [theme, tokens] of Object.entries(themeTokens)) {
      const values = tokens as Record<string, string>;
      const style = grayscaleTokenStyle((name) => values[name] ?? '');
      // Every token of the theme is a plain colour, so none is left in colour.
      expect(Object.keys(style).sort(), theme).toEqual(Object.keys(values).sort());
    }
  });
});
