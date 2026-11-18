/**
 * Unit tests for color utilities.
 */

import { describe, it, expect } from 'vitest';
import {
  getErrorColor,
  hexToRgb,
  rgbToHex,
  interpolateColor,
  getDepthColor,
  hexToRgba,
  lightenColor,
  darkenColor,
  isLightColor,
  getContrastColor,
  getNextLineColor,
  LINE_COLORS,
  LIGHT_LINE_COLORS,
  relativeLuminance,
  contrastRatio,
  isLightCanvasTheme,
  lineColorPalette,
  hexToHSL,
  hslToHex,
  interpolateHue,
} from '../colors';
import { mapCanvasFor } from '../../types/config';
import { themeTokens } from '../../theme/tokens';

describe('getDepthColor', () => {
  const shallowColor = '#00bfff';
  const deepColor = '#ff4444';

  describe('basic interpolation', () => {
    it('returns shallow color at minimum depth', () => {
      const color = getDepthColor(0, shallowColor, deepColor, 0, 50);
      expect(color).toBe(shallowColor);
    });

    it('returns deep color at maximum depth', () => {
      const color = getDepthColor(50, shallowColor, deepColor, 0, 50);
      expect(color).toBe(deepColor);
    });

    it('returns intermediate color at midpoint depth', () => {
      const color = getDepthColor(25, shallowColor, deepColor, 0, 50);
      // Should be a valid hex color
      expect(color).toMatch(/^#[0-9a-f]{6}$/i);
      // Should be different from both endpoints
      expect(color).not.toBe(shallowColor);
      expect(color).not.toBe(deepColor);
    });

    it('handles non-zero minimum depth', () => {
      const color = getDepthColor(10, shallowColor, deepColor, 10, 60);
      expect(color).toBe(shallowColor);
    });

    it('produces smooth gradient at quarter points', () => {
      const at25 = getDepthColor(12.5, shallowColor, deepColor, 0, 50);
      const at50 = getDepthColor(25, shallowColor, deepColor, 0, 50);
      const at75 = getDepthColor(37.5, shallowColor, deepColor, 0, 50);

      // All should be valid hex colors
      expect(at25).toMatch(/^#[0-9a-f]{6}$/i);
      expect(at50).toMatch(/^#[0-9a-f]{6}$/i);
      expect(at75).toMatch(/^#[0-9a-f]{6}$/i);

      // All should be distinct
      expect(at25).not.toBe(at50);
      expect(at50).not.toBe(at75);
    });
  });

  describe('depth clamping', () => {
    it('clamps depth below minimum to shallow color', () => {
      const color = getDepthColor(-10, shallowColor, deepColor, 0, 50);
      expect(color).toBe(shallowColor);
    });

    it('clamps depth above maximum to deep color', () => {
      const color = getDepthColor(100, shallowColor, deepColor, 0, 50);
      expect(color).toBe(deepColor);
    });

    it('clamps depth far below minimum', () => {
      const color = getDepthColor(-1000, shallowColor, deepColor, 0, 50);
      expect(color).toBe(shallowColor);
    });

    it('clamps depth far above maximum', () => {
      const color = getDepthColor(1000, shallowColor, deepColor, 0, 50);
      expect(color).toBe(deepColor);
    });
  });

  describe('edge cases', () => {
    it('returns shallow color when min equals max', () => {
      const color = getDepthColor(25, shallowColor, deepColor, 25, 25);
      expect(color).toBe(shallowColor);
    });

    it('returns shallow color when min equals max with clamped depth', () => {
      // Depth outside the single-value range
      const colorBelow = getDepthColor(0, shallowColor, deepColor, 25, 25);
      const colorAbove = getDepthColor(50, shallowColor, deepColor, 25, 25);
      expect(colorBelow).toBe(shallowColor);
      expect(colorAbove).toBe(shallowColor);
    });

    it('returns null for NaN depth', () => {
      const color = getDepthColor(NaN, shallowColor, deepColor, 0, 50);
      expect(color).toBeNull();
    });

    it('returns null for Infinity depth', () => {
      const color = getDepthColor(Infinity, shallowColor, deepColor, 0, 50);
      expect(color).toBeNull();
    });

    it('returns null for -Infinity depth', () => {
      const color = getDepthColor(-Infinity, shallowColor, deepColor, 0, 50);
      expect(color).toBeNull();
    });

    it('returns null for NaN minDepth', () => {
      const color = getDepthColor(25, shallowColor, deepColor, NaN, 50);
      expect(color).toBeNull();
    });

    it('returns null for NaN maxDepth', () => {
      const color = getDepthColor(25, shallowColor, deepColor, 0, NaN);
      expect(color).toBeNull();
    });
  });

  describe('invalid color format', () => {
    it('returns null for invalid shallow color format', () => {
      const color = getDepthColor(25, 'invalid', deepColor, 0, 50);
      expect(color).toBeNull();
    });

    it('returns null for invalid deep color format', () => {
      const color = getDepthColor(25, shallowColor, 'invalid', 0, 50);
      expect(color).toBeNull();
    });

    it('returns null for both invalid colors', () => {
      const color = getDepthColor(25, 'invalid', 'also-invalid', 0, 50);
      expect(color).toBeNull();
    });

    it('returns null for rgb() format (not supported)', () => {
      const color = getDepthColor(25, 'rgb(0, 191, 255)', deepColor, 0, 50);
      expect(color).toBeNull();
    });

    it('returns null for short hex format without # (not supported)', () => {
      // The regex expects 6 hex digits
      const color = getDepthColor(25, 'fff', deepColor, 0, 50);
      expect(color).toBeNull();
    });

    it('accepts hex without # prefix', () => {
      const color = getDepthColor(0, '00bfff', 'ff4444', 0, 50);
      expect(color).toBe('#00bfff');
    });
  });
});

describe('hexToHSL', () => {
  it('converts red correctly', () => {
    const result = hexToHSL('#ff0000');
    expect(result).not.toBeNull();
    expect(result!.h).toBeCloseTo(0, 1);
    expect(result!.s).toBeCloseTo(1, 2);
    expect(result!.l).toBeCloseTo(0.5, 2);
  });

  it('converts green correctly', () => {
    const result = hexToHSL('#00ff00');
    expect(result).not.toBeNull();
    expect(result!.h).toBeCloseTo(120, 1);
    expect(result!.s).toBeCloseTo(1, 2);
    expect(result!.l).toBeCloseTo(0.5, 2);
  });

  it('converts blue correctly', () => {
    const result = hexToHSL('#0000ff');
    expect(result).not.toBeNull();
    expect(result!.h).toBeCloseTo(240, 1);
    expect(result!.s).toBeCloseTo(1, 2);
    expect(result!.l).toBeCloseTo(0.5, 2);
  });

  it('converts white correctly (achromatic)', () => {
    const result = hexToHSL('#ffffff');
    expect(result).not.toBeNull();
    expect(result!.h).toBe(0); // Hue is arbitrary for achromatic
    expect(result!.s).toBe(0);
    expect(result!.l).toBe(1);
  });

  it('converts black correctly (achromatic)', () => {
    const result = hexToHSL('#000000');
    expect(result).not.toBeNull();
    expect(result!.h).toBe(0);
    expect(result!.s).toBe(0);
    expect(result!.l).toBe(0);
  });

  it('converts gray correctly (achromatic)', () => {
    const result = hexToHSL('#808080');
    expect(result).not.toBeNull();
    expect(result!.h).toBe(0);
    expect(result!.s).toBe(0);
    expect(result!.l).toBeCloseTo(0.5, 1);
  });

  it('converts cyan (#00bfff) correctly', () => {
    const result = hexToHSL('#00bfff');
    expect(result).not.toBeNull();
    // Cyan-ish hue around 195
    expect(result!.h).toBeGreaterThan(190);
    expect(result!.h).toBeLessThan(200);
  });

  it('returns null for invalid hex format', () => {
    expect(hexToHSL('invalid')).toBeNull();
    expect(hexToHSL('rgb(255,0,0)')).toBeNull();
    expect(hexToHSL('#fff')).toBeNull(); // Short form not supported
    expect(hexToHSL('')).toBeNull();
  });

  it('accepts hex without # prefix', () => {
    const result = hexToHSL('ff0000');
    expect(result).not.toBeNull();
    expect(result!.h).toBeCloseTo(0, 1);
  });
});

describe('hslToHex', () => {
  it('converts red correctly', () => {
    const hex = hslToHex(0, 1, 0.5);
    expect(hex.toLowerCase()).toBe('#ff0000');
  });

  it('converts green correctly', () => {
    const hex = hslToHex(120, 1, 0.5);
    expect(hex.toLowerCase()).toBe('#00ff00');
  });

  it('converts blue correctly', () => {
    const hex = hslToHex(240, 1, 0.5);
    expect(hex.toLowerCase()).toBe('#0000ff');
  });

  it('converts white correctly', () => {
    const hex = hslToHex(0, 0, 1);
    expect(hex.toLowerCase()).toBe('#ffffff');
  });

  it('converts black correctly', () => {
    const hex = hslToHex(0, 0, 0);
    expect(hex.toLowerCase()).toBe('#000000');
  });

  it('handles hue normalization for values > 360', () => {
    const hex360 = hslToHex(360, 1, 0.5);
    const hex0 = hslToHex(0, 1, 0.5);
    expect(hex360.toLowerCase()).toBe(hex0.toLowerCase());
  });

  it('handles hue normalization for negative values', () => {
    const hexNeg60 = hslToHex(-60, 1, 0.5);
    const hex300 = hslToHex(300, 1, 0.5);
    expect(hexNeg60.toLowerCase()).toBe(hex300.toLowerCase());
  });

  it('is inverse of hexToHSL for primary colors', () => {
    const colors = ['#ff0000', '#00ff00', '#0000ff'];
    for (const color of colors) {
      const hsl = hexToHSL(color);
      expect(hsl).not.toBeNull();
      const roundTrip = hslToHex(hsl!.h, hsl!.s, hsl!.l);
      expect(roundTrip.toLowerCase()).toBe(color.toLowerCase());
    }
  });
});

describe('interpolateHue', () => {
  it('returns start hue at t=0', () => {
    const result = interpolateHue(100, 200, 0);
    expect(result).toBeCloseTo(100, 5);
  });

  it('returns end hue at t=1', () => {
    const result = interpolateHue(100, 200, 1);
    expect(result).toBeCloseTo(200, 5);
  });

  it('returns midpoint at t=0.5 for simple case', () => {
    const result = interpolateHue(100, 200, 0.5);
    expect(result).toBeCloseTo(150, 5);
  });

  it('takes shortest path across 0/360 boundary (350 to 10)', () => {
    // From 350 to 10 should go through 0, not through 180
    // Midpoint of 350->360->0->10 is 0 (or 360)
    const result = interpolateHue(350, 10, 0.5);
    expect(result).toBeCloseTo(0, 1);
  });

  it('takes shortest path across 0/360 boundary (10 to 350)', () => {
    // From 10 to 350 should go backwards through 0
    // Midpoint should be 0 (or 360)
    const result = interpolateHue(10, 350, 0.5);
    expect(result).toBeCloseTo(0, 1);
  });

  it('takes shortest path for 180-degree difference (goes forward)', () => {
    // 0 to 180 - either direction is equally short
    const result = interpolateHue(0, 180, 0.5);
    expect(result).toBeCloseTo(90, 5);
  });

  it('takes shortest path for > 180 difference', () => {
    // 0 to 270 should go backwards through 360/0 to get to 270
    // Shortest path: 0 -> 360 -> 270 (90 degree backwards = -90)
    // Midpoint: 0 + (-90)*0.5 = -45 = 315
    const result = interpolateHue(0, 270, 0.5);
    expect(result).toBeCloseTo(315, 1);
  });

  it('normalizes result to 0-360 range', () => {
    // Various inputs that might produce out-of-range results
    const result1 = interpolateHue(350, 10, 0.25);
    expect(result1).toBeGreaterThanOrEqual(0);
    expect(result1).toBeLessThan(360);

    const result2 = interpolateHue(10, 350, 0.75);
    expect(result2).toBeGreaterThanOrEqual(0);
    expect(result2).toBeLessThan(360);
  });

  it('handles same hue (no interpolation needed)', () => {
    const result = interpolateHue(180, 180, 0.5);
    expect(result).toBeCloseTo(180, 5);
  });

  it('handles t values at quarter points', () => {
    // 0 to 100, quarter points
    expect(interpolateHue(0, 100, 0.25)).toBeCloseTo(25, 5);
    expect(interpolateHue(0, 100, 0.75)).toBeCloseTo(75, 5);
  });
});

// ---------------------------------------------------------------------------
// getErrorColor
// ---------------------------------------------------------------------------

describe('getErrorColor', () => {
  it('returns green for < 1% error', () => {
    expect(getErrorColor(0)).toBe('#22c55e');
    expect(getErrorColor(0.5)).toBe('#22c55e');
    expect(getErrorColor(0.99)).toBe('#22c55e');
  });

  it('returns yellow for 1-3% error', () => {
    expect(getErrorColor(1)).toBe('#eab308');
    expect(getErrorColor(2.5)).toBe('#eab308');
  });

  it('returns orange for 3-5% error', () => {
    expect(getErrorColor(3)).toBe('#f97316');
    expect(getErrorColor(4.99)).toBe('#f97316');
  });

  it('returns red for >= 5% error', () => {
    expect(getErrorColor(5)).toBe('#ef4444');
    expect(getErrorColor(100)).toBe('#ef4444');
  });
});

// ---------------------------------------------------------------------------
// hexToRgb
// ---------------------------------------------------------------------------

describe('hexToRgb', () => {
  it('parses primary colors', () => {
    expect(hexToRgb('#ff0000')).toEqual([255, 0, 0]);
    expect(hexToRgb('#00ff00')).toEqual([0, 255, 0]);
    expect(hexToRgb('#0000ff')).toEqual([0, 0, 255]);
  });

  it('parses without # prefix', () => {
    expect(hexToRgb('ff8800')).toEqual([255, 136, 0]);
  });

  it('parses black and white', () => {
    expect(hexToRgb('#000000')).toEqual([0, 0, 0]);
    expect(hexToRgb('#ffffff')).toEqual([255, 255, 255]);
  });

  it('returns [0,0,0] for invalid input', () => {
    expect(hexToRgb('invalid')).toEqual([0, 0, 0]);
    expect(hexToRgb('#fff')).toEqual([0, 0, 0]);
  });
});

// ---------------------------------------------------------------------------
// rgbToHex
// ---------------------------------------------------------------------------

describe('rgbToHex', () => {
  it('converts primary colors', () => {
    expect(rgbToHex(255, 0, 0)).toBe('#ff0000');
    expect(rgbToHex(0, 255, 0)).toBe('#00ff00');
    expect(rgbToHex(0, 0, 255)).toBe('#0000ff');
  });

  it('pads single-digit hex values', () => {
    expect(rgbToHex(0, 0, 0)).toBe('#000000');
    expect(rgbToHex(1, 2, 3)).toBe('#010203');
  });

  it('rounds fractional values', () => {
    expect(rgbToHex(127.6, 0, 0)).toBe('#800000');
  });
});

// ---------------------------------------------------------------------------
// interpolateColor
// ---------------------------------------------------------------------------

describe('interpolateColor', () => {
  it('returns first color at t=0', () => {
    expect(interpolateColor('#ff0000', '#0000ff', 0)).toBe('#ff0000');
  });

  it('returns second color at t=1', () => {
    expect(interpolateColor('#ff0000', '#0000ff', 1)).toBe('#0000ff');
  });

  it('returns midpoint at t=0.5', () => {
    const mid = interpolateColor('#000000', '#ffffff', 0.5);
    // Expect ~128 for each channel: #808080
    expect(mid).toBe(rgbToHex(128, 128, 128));
  });
});

// ---------------------------------------------------------------------------
// hexToRgba
// ---------------------------------------------------------------------------

describe('hexToRgba', () => {
  it('converts with alpha', () => {
    expect(hexToRgba('#ff0000', 0.5)).toBe('rgba(255, 0, 0, 0.5)');
  });

  it('handles alpha 0 and 1', () => {
    expect(hexToRgba('#000000', 0)).toBe('rgba(0, 0, 0, 0)');
    expect(hexToRgba('#ffffff', 1)).toBe('rgba(255, 255, 255, 1)');
  });
});

// ---------------------------------------------------------------------------
// lightenColor / darkenColor
// ---------------------------------------------------------------------------

describe('lightenColor', () => {
  it('returns white at 100%', () => {
    expect(lightenColor('#000000', 100)).toBe('#ffffff');
  });

  it('returns same color at 0%', () => {
    expect(lightenColor('#ff0000', 0)).toBe('#ff0000');
  });

  it('lightens by 50%', () => {
    const result = lightenColor('#000000', 50);
    // 0 + (255-0)*0.5 = 127.5 -> #808080
    expect(result).toBe(rgbToHex(128, 128, 128));
  });
});

describe('darkenColor', () => {
  it('returns black at 100%', () => {
    expect(darkenColor('#ffffff', 100)).toBe('#000000');
  });

  it('returns same color at 0%', () => {
    expect(darkenColor('#ff0000', 0)).toBe('#ff0000');
  });

  it('darkens by 50%', () => {
    const result = darkenColor('#ffffff', 50);
    // 255 * 0.5 = 127.5 -> #808080
    expect(result).toBe(rgbToHex(128, 128, 128));
  });
});

// ---------------------------------------------------------------------------
// isLightColor / getContrastColor
// ---------------------------------------------------------------------------

describe('isLightColor', () => {
  it('white is light', () => {
    expect(isLightColor('#ffffff')).toBe(true);
  });

  it('black is not light', () => {
    expect(isLightColor('#000000')).toBe(false);
  });

  it('yellow is light (high perceived luminance)', () => {
    expect(isLightColor('#ffff00')).toBe(true);
  });

  it('dark blue is not light', () => {
    expect(isLightColor('#000080')).toBe(false);
  });
});

describe('getContrastColor', () => {
  it('returns black for light backgrounds', () => {
    expect(getContrastColor('#ffffff')).toBe('#000000');
    expect(getContrastColor('#ffff00')).toBe('#000000');
  });

  it('returns white for dark backgrounds', () => {
    expect(getContrastColor('#000000')).toBe('#ffffff');
    expect(getContrastColor('#000080')).toBe('#ffffff');
  });
});

// ---------------------------------------------------------------------------
// getNextLineColor
// ---------------------------------------------------------------------------

describe('getNextLineColor', () => {
  it('returns first palette color when none used', () => {
    expect(getNextLineColor([], 'dark')).toBe(LINE_COLORS[0]);
  });

  it('returns second palette color when first is used', () => {
    expect(getNextLineColor([LINE_COLORS[0]!], 'dark')).toBe(LINE_COLORS[1]);
  });

  it('skips used colors', () => {
    const used = [LINE_COLORS[0]!, LINE_COLORS[1]!];
    expect(getNextLineColor(used, 'dark')).toBe(LINE_COLORS[2]);
  });

  it('generates random color when all palette colors exhausted', () => {
    const allUsed = [...LINE_COLORS];
    const result = getNextLineColor(allUsed, 'dark');
    expect(result).toMatch(/^#[0-9a-f]{6}$/i);
    expect(LINE_COLORS).not.toContain(result);
  });

  // #357 item 9. `LINE_COLORS` is dark-tuned pastel and draws at ~1.5:1 or
  // worse on the light map canvas (`#faf7f2`), so a survey seeded under a
  // light theme got barely-visible line lines. `LIGHT_LINE_COLORS` is
  // the fix; these guards measure it rather than trust it.
  describe('theme-aware seeding (#357 item 9)', () => {
    const ALL_THEMES = Object.keys(themeTokens);
    /** WCAG's floor for a non-text graphical object, which is what a line line is. */
    const FLOOR = 3;

    it('every LINE_COLORS entry clears 3:1 on the dark canvas', () => {
      const canvas = themeTokens.dark['--cc-map-background'];
      for (const color of LINE_COLORS) {
        const ratio = contrastRatio(color, canvas);
        expect(ratio, `${color} on ${canvas} is ${ratio.toFixed(2)}:1`).toBeGreaterThanOrEqual(FLOOR);
      }
    });

    it('every LIGHT_LINE_COLORS entry clears 3:1 on the light canvas', () => {
      const canvas = themeTokens.light['--cc-map-background'];
      for (const color of LIGHT_LINE_COLORS) {
        const ratio = contrastRatio(color, canvas);
        expect(ratio, `${color} on ${canvas} is ${ratio.toFixed(2)}:1`).toBeGreaterThanOrEqual(FLOOR);
      }
    });

    /**
     * Distinguishability: no entry may be a near-duplicate of another in the
     * same palette (they exist to tell lines apart). Floor is below both
     * palettes' measured minimum pairwise RGB distance (LINE_COLORS: 31.2,
     * teal/sky-blue; LIGHT_LINE_COLORS: 28.2, the two teal-family entries)
     * so the legitimate same-hue-family pairs (distinguished by saturation,
     * not hue) still pass.
     */
    function minPairwiseRgbDistance(palette: string[]): number {
      let min = Infinity;
      for (let i = 0; i < palette.length; i++) {
        for (let j = i + 1; j < palette.length; j++) {
          const [r1, g1, b1] = hexToRgb(palette[i]!);
          const [r2, g2, b2] = hexToRgb(palette[j]!);
          const d = Math.sqrt((r1 - r2) ** 2 + (g1 - g2) ** 2 + (b1 - b2) ** 2);
          min = Math.min(min, d);
        }
      }
      return min;
    }

    it('LINE_COLORS entries are pairwise distinguishable', () => {
      expect(minPairwiseRgbDistance(LINE_COLORS)).toBeGreaterThan(25);
    });

    it('LIGHT_LINE_COLORS entries are pairwise distinguishable', () => {
      expect(minPairwiseRgbDistance(LIGHT_LINE_COLORS)).toBeGreaterThan(25);
    });

    it.each(ALL_THEMES)(
      '%s: getNextLineColor draws from the palette matching its MAP CANVAS, not its chrome',
      (theme) => {
        const canvasIsLight = relativeLuminance(mapCanvasFor(theme)) > 0.5;
        expect(isLightCanvasTheme(theme)).toBe(canvasIsLight);

        const result = getNextLineColor([], theme);
        if (canvasIsLight) {
          expect(LIGHT_LINE_COLORS, `${theme}: canvas is light`).toContain(result);
          expect(LINE_COLORS).not.toContain(result);
        } else {
          expect(LINE_COLORS, `${theme}: canvas is dark`).toContain(result);
          expect(LIGHT_LINE_COLORS).not.toContain(result);
        }
      }
    );

    it('chrome-only light themes (solarized_light, catppuccin_latte) still get the DARK palette', () => {
      // These themes have light chrome but leave --cc-map-background unset, so
      // they inherit :root's dark canvas - the bug the first half of #357 item
      // 9 fixed for Config defaults. Pin it here too, at the real seeding path.
      expect(lineColorPalette('solarized_light')).toEqual(LINE_COLORS);
      expect(lineColorPalette('catppuccin_latte')).toEqual(LINE_COLORS);
    });

    it('light and kaleido_eink get the LIGHT palette', () => {
      expect(lineColorPalette('light')).toEqual(LIGHT_LINE_COLORS);
      expect(lineColorPalette('kaleido_eink')).toEqual(LIGHT_LINE_COLORS);
    });
  });
});
