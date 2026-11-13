/**
 * The Survey Issues palette: the Default entry is the original ramp, a chosen
 * palette maps the warn level to its middle, and loop-closure lines follow the
 * Survey Issues scale only while it paints the map.
 */
import { describe, expect, it } from 'vitest';
import { resolveChoice } from '../../../../utils/palettes';
import { getErrorColor, hexToRgb, SEVERITY_GREEN, SEVERITY_RED, SEVERITY_YELLOW } from '../../../../utils/colors';
import { getMapThemeColors } from '../../../../utils/themeColors';
import { closureLineColor, issueScoreColor, severityTextColor } from '../issueColor';
import { issueScaleFrom, scoreToPosition } from '../issueScale';

const theme = getMapThemeColors();
const T = { azimuthDiff: 5, pitchDiff: 10, inclinationDepthDiff: 15 };

describe('the Survey Issues palette', () => {
  it('Default resolves to the original green -> yellow -> red', () => {
    const def = resolveChoice({ id: 'theme', reversed: false }, 'issues', theme);
    expect(def.hex(0)).toBe(SEVERITY_GREEN);
    expect(def.hex(0.5)).toBe(SEVERITY_YELLOW);
    expect(def.hex(1)).toBe(SEVERITY_RED);
    // Within one step per channel (float rounding at a few positions); the map
    // never takes this path for the untouched Default anyway (palette null).
    let worst = 0;
    for (let q = 0; q <= 300; q++) {
      const a = hexToRgb(def.hex(scoreToPosition(q / 100))!);
      const b = hexToRgb(issueScoreColor(theme, q / 100));
      for (let k = 0; k < 3; k++) worst = Math.max(worst, Math.abs(a[k]! - b[k]!));
    }
    expect(worst).toBeLessThanOrEqual(1);
  });

  it('Default reversed runs red -> yellow -> green', () => {
    const rev = resolveChoice({ id: 'theme', reversed: true }, 'issues', theme);
    expect(issueScoreColor(theme, 0, rev)).toBe(SEVERITY_RED);
    expect(issueScoreColor(theme, 1, rev)).toBe(SEVERITY_YELLOW);
    expect(issueScoreColor(theme, 3, rev)).toBe(SEVERITY_GREEN);
  });

  it('a chosen palette puts the warn level at its middle and Max at its end', () => {
    const viridis = resolveChoice({ id: 'mpl.viridis', reversed: false }, 'issues', theme);
    expect(issueScoreColor(theme, 0, viridis)).toBe(viridis.hex(0));
    expect(issueScoreColor(theme, 1, viridis)).toBe(viridis.hex(0.5));
    expect(issueScoreColor(theme, 3, viridis)).toBe(viridis.hex(1));
    expect(issueScoreColor(theme, 7, viridis)).toBe(viridis.hex(1));
  });
});

describe('closureLineColor', () => {
  const scale = issueScaleFrom(T, {}, [], 'worst');
  it('keeps the colour bands while Survey Issues is off', () => {
    for (const pct of [0.5, 2, 4, 9]) expect(closureLineColor(pct, null, theme)).toBe(getErrorColor(pct));
    expect(closureLineColor(2, { color: false, scale, palette: null }, theme)).toBe(getErrorColor(2));
  });
  it('is graded on the closure levels while Survey Issues paints the map', () => {
    const on = { color: true, scale, palette: null };
    expect(closureLineColor(0, on, theme)).toBe(SEVERITY_GREEN);
    expect(closureLineColor(1, on, theme)).toBe(SEVERITY_YELLOW);
    expect(closureLineColor(5, on, theme)).toBe(SEVERITY_RED);
    expect(closureLineColor(3, on, theme)).toBe(issueScoreColor(theme, 2));
  });
  it('falls back to the bands when the closure error is switched off', () => {
    const off = { color: true, scale: issueScaleFrom(T, {}, ['closure'], 'worst'), palette: null };
    expect(closureLineColor(3, off, theme)).toBe(getErrorColor(3));
  });
});

describe('severityTextColor with user palettes', () => {
  const dark = { ...theme, background: '#101418' };
  const light = { ...theme, background: '#f4efe6' };
  const mean = (hex: string) => {
    const [r, g, b] = hexToRgb(hex);
    return (0.2126 * r! + 0.7152 * g! + 0.0722 * b!) / 255;
  };
  it('keeps the default green, yellow and red as they were on a dark map', () => {
    for (const c of [SEVERITY_GREEN, SEVERITY_YELLOW, SEVERITY_RED]) expect(severityTextColor(c, dark)).toBe(c);
  });
  it('lifts a near-black palette colour on a dark map', () => {
    const lifted = severityTextColor('#440154', dark);
    expect(mean(lifted)).toBeGreaterThanOrEqual(0.39);
  });
  it('pushes a near-white palette colour down on a light map', () => {
    expect(mean(severityTextColor('#ffffff', light))).toBeLessThanOrEqual(0.46);
    expect(severityTextColor(SEVERITY_YELLOW, light)).toBe('#816204');
  });
});
