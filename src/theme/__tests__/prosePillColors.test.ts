/**
 * RS-PROSE: a prose pill paints a soft radial tint that fades from the line's
 * tint at its centre to the bare panel at its edges, so its text and muted
 * parts must read on both: main text 7:1, the context and Station ID 4.5:1
 * (no dots since RS-NODOTS). Swept over the same line colours as the link
 * pill (`pillColors.test.ts`) on every theme, measured with `utils/colors`.
 */
import { describe, expect, it } from 'vitest';
import { themeTokens } from '../tokens';
import { CSS_COLOR_NAMES, hexOfOklch } from '../cssColor';
import { COMMENT_PALETTE, LIGHT_LINE_COLORS, LINE_COLORS, contrastRatio } from '../../utils/colors';
import {
  PILL_MUTED_MIN,
  PILL_TEXT_MIN,
  pillColors,
  pillThemeOf,
  prosePillColors,
  prosePillColorsOf,
  proseTintOf,
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

describe('prosePillColors sweep', () => {
  it.each(THEMES)('%s: text 7:1, muted 4.5:1 on the tint and on the panel', (name) => {
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

  it('prosePillColorsOf is prosePillColors on the named theme, cached', () => {
    const a = prosePillColorsOf('#1f4fff', 'light');
    expect(a).toEqual(prosePillColors('#1f4fff', pillThemeOf('light')));
    expect(prosePillColorsOf('#1f4fff', 'light')).toBe(a);
  });
});

describe('proseTintOf', () => {
  const rgba = (css: string) => {
    const m = /rgba\((\d+), (\d+), (\d+), ([\d.]+)\)/.exec(css);
    if (!m) throw new Error(css);
    return { rgb: [Number(m[1]), Number(m[2]), Number(m[3])], a: Number(m[4]) };
  };
  const hex = (h: string) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16));

  it.each(THEMES)('%s: over the panel the overlay is the tint itself, within one 8-bit step', (name) => {
    const theme = pillThemeOf(name);
    for (const color of COLORS) {
      const target = prosePillColors(color, theme).bg;
      const { tint, clear } = proseTintOf(target, theme.panel);
      const o = rgba(tint);
      const panel = hex(theme.panel);
      const composed = panel.map((p, i) => p * (1 - o.a) + o.rgb[i]! * o.a);
      hex(target).forEach((t, i) => expect(Math.abs(composed[i]! - t), `${name} ${color}`).toBeLessThanOrEqual(1.01));
      expect(o.a).toBeGreaterThan(0);
      expect(o.a).toBeLessThanOrEqual(1);
      expect(rgba(clear).rgb).toEqual(o.rgb);
      expect(rgba(clear).a).toBe(0);
    }
  });

  it('a tint equal to the panel is no overlay at all', () => {
    expect(proseTintOf('#202020', '#202020')).toEqual({ tint: 'rgba(0, 0, 0, 0)', clear: 'rgba(0, 0, 0, 0)' });
  });
});
