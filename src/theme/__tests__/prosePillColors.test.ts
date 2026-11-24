/**
 * RS-PROSE: a prose pill paints a soft radial tint that fades from the line's
 * tint at its centre to the bare panel at its edges, so its text, muted
 * parts, dot and home-line dot must read on both: main text 7:1, the context
 * and Station ID 4.5:1, the dot (or its ring) and a home-line dot 3:1. Swept
 * over the same line colours as the link pill (`pillColors.test.ts`) on every
 * theme, measured with `utils/colors`.
 * Injection: computing the prose dot against the tint alone (not the panel)
 * fails "the dot reads on the panel" for a near-black line on dark themes.
 */
import { describe, expect, it } from 'vitest';
import { themeTokens } from '../tokens';
import { CSS_COLOR_NAMES, hexOfOklch } from '../cssColor';
import { COMMENT_PALETTE, LIGHT_LINE_COLORS, LINE_COLORS, contrastRatio } from '../../utils/colors';
import {
  PILL_DOT_MIN,
  PILL_MUTED_MIN,
  PILL_TEXT_MIN,
  pillColors,
  pillMarkOf,
  pillThemeOf,
  prosePillColors,
  prosePillColorsOf,
} from '../pillColors';

const THEMES = Object.keys(themeTokens);

function sweepColors(): string[] {
  const out: string[] = [];
  for (const L of [0.12, 0.25, 0.4, 0.55, 0.7, 0.85, 0.95, 0.99]) {
    for (const C of [0, 0.03, 0.08, 0.15, 0.25, 0.37]) {
      for (let h = 0; h < 360; h += 20) out.push(hexOfOklch({ L, C, h }));
    }
  }
  out.push('#000', '#fff', '#050505', '#101010', '#f6f6f2', '#fdfdfd', '#ffff00', '#ffe600', '#0000ff', '#1f4fff', '#8a8a8a');
  out.push(...CSS_COLOR_NAMES, ...LINE_COLORS, ...LIGHT_LINE_COLORS, ...COMMENT_PALETTE);
  out.push('rgba(0, 0, 255, 0.3)', 'not-a-colour', '');
  return out;
}

const COLORS = sweepColors();
/** Home-line dots: a few hard cases on any pill. */
const MARKS = ['#000', '#fff', '#101010', '#f6f6f2', '#ffe600', '#1f4fff', '#8a8a8a'];

describe('prosePillColors sweep', () => {
  it.each(THEMES)('%s: text 7:1, muted 4.5:1, dot or ring 3:1 on the tint and on the panel', (name) => {
    const theme = pillThemeOf(name);
    const failures: string[] = [];
    const check = (label: string, value: number, min: number) => {
      if (!(value >= min)) failures.push(`${label} ${value.toFixed(2)} < ${min}`);
    };
    for (const color of COLORS) {
      const p = prosePillColors(color, theme);
      for (const [where, bg] of [['tint', p.bg], ['panel', theme.panel]] as const) {
        check(`${color} ${where} text`, contrastRatio(p.text, bg), PILL_TEXT_MIN);
        check(`${color} ${where} muted`, contrastRatio(p.muted, bg), PILL_MUTED_MIN);
        check(`${color} ${where} dot`, contrastRatio(p.ring ?? p.dot, bg), PILL_DOT_MIN);
        for (const markColor of MARKS) {
          const m = pillMarkOf(markColor, p, name);
          check(`${color} ${where} mark ${markColor}`, contrastRatio(m.ring ?? m.dot, bg), PILL_DOT_MIN);
        }
      }
    }
    expect(failures).toEqual([]);
  });

  it.each(THEMES)("%s: the tint's centre is the link pill's hover tint, and it fades to the panel", (name) => {
    const theme = pillThemeOf(name);
    for (const color of ['#ffe600', '#1f4fff', '#101010', '#f6f6f2']) {
      const p = prosePillColors(color, theme);
      expect(p.bg).toBe(pillColors(color, theme).hoverBg);
      expect(p.hoverBg).toBe(theme.panel);
    }
  });

  it('the dot is the line colour itself, ringed when it alone does not read on the panel', () => {
    const dark = prosePillColors('#101010', pillThemeOf('dark'));
    expect(dark.dot).toBe('#101010');
    expect(dark.ring).not.toBeNull();
    const yellow = prosePillColors('#ffe600', pillThemeOf('dark'));
    expect(yellow.ring).toBeNull();
    const white = prosePillColors('#f6f6f2', pillThemeOf('light'));
    expect(white.ring).not.toBeNull();
  });

  it('prosePillColorsOf is prosePillColors on the named theme, cached', () => {
    const a = prosePillColorsOf('#1f4fff', 'light');
    expect(a).toEqual(prosePillColors('#1f4fff', pillThemeOf('light')));
    expect(prosePillColorsOf('#1f4fff', 'light')).toBe(a);
  });
});
