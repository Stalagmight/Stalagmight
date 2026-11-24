/**
 * RS-PROSE, RS-NODOTS: a prose pill paints a glow, a capsule solid behind the
 * name whose halo fades to the bare panel, so its text and muted parts must
 * read on both: main text 7:1, the context and Station ID 4.5:1; its leading
 * dot (RS-LEADDOT) sits inside the core at its start, so the dot or its ring
 * reaches 3:1 there (on the glow over the panel and over each tinted box,
 * and on the bare ones for good measure). The glow is
 * a wash at one alpha, lighter than a dark panel and darker than a light
 * one, clearly off the panel, in the line's hue, and further off it for a
 * line nearer the text pole, so a near-white, a grey and a near-black line
 * differ. Swept over the same line colours as the link pill
 * (`pillColors.test.ts`) on every theme, measured with `utils/colors`.
 * Injections: the glow on the link pills' side (`theme.lift`) fails "lighter
 * than a dark panel" on cave; one lightness for every line fails "the
 * greys differ"; the old radial tint's centre fails "clearly off the panel";
 * the link pill's dot (ringed against its fill, not the glow) fails "dot or
 * ring 3:1 on the glow" for a near-black line on cave.
 */
import { describe, expect, it } from 'vitest';
import { themeTokens } from '../tokens';
import { CSS_COLOR_NAMES, hexOfOklch, oklchOfHex } from '../cssColor';
import { COMMENT_PALETTE, LIGHT_LINE_COLORS, LINE_COLORS, contrastRatio } from '../../utils/colors';
import {
  PILL_DOT_MIN,
  PILL_MUTED_MIN,
  PILL_TEXT_MIN,
  PROSE_ALPHA,
  pillThemeOf,
  proseBackdrops,
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
  it.each(THEMES)('%s: text 7:1, muted 4.5:1 on the glow, the panel and the tinted boxes, bare and under the glow', (name) => {
    const theme = pillThemeOf(name);
    expect(theme.surfaces.length).toBe(3);
    const failures: string[] = [];
    const check = (label: string, value: number, min: number) => {
      if (!(value >= min)) failures.push(`${label} ${value.toFixed(2)} < ${min}`);
    };
    for (const color of COLORS) {
      const p = prosePillColors(color, theme);
      for (const [where, bg] of proseBackdrops(p.bg, theme).map((b, i) => [String(i), b] as const)) {
        check(`${color} ${where} text`, contrastRatio(p.text, bg), PILL_TEXT_MIN);
        check(`${color} ${where} muted`, contrastRatio(p.muted, bg), PILL_MUTED_MIN);
      }
    }
    expect(failures).toEqual([]);
  });

  it.each(THEMES)('%s: the dot or its ring 3:1 on the glow over the panel and the tinted boxes, ringed only where needed (RS-LEADDOT)', (name) => {
    const theme = pillThemeOf(name);
    const failures: string[] = [];
    for (const color of COLORS) {
      const p = prosePillColors(color, theme);
      const under = proseBackdrops(p.bg, theme);
      for (const bg of under) {
        const c = contrastRatio(p.ring ?? p.dot, bg);
        if (!(c >= PILL_DOT_MIN)) failures.push(`${color} dot on ${bg} ${c.toFixed(2)}`);
      }
      const alone = Math.min(...under.map((bg) => contrastRatio(p.dot, bg)));
      if (p.ring === null && alone < PILL_DOT_MIN) failures.push(`${color} needs a ring`);
      if (p.ring !== null && alone >= PILL_DOT_MIN + 0.3) failures.push(`${color} ringed for nothing`);
    }
    expect(failures).toEqual([]);
  });

  it('the dot is the line colour itself, ringed where it alone does not read on the glow', () => {
    const dark = prosePillColors('#101010', pillThemeOf('cave'));
    expect(dark.dot).toBe('#101010');
    expect(dark.ring).not.toBeNull();
    const yellow = prosePillColors('#ffe600', pillThemeOf('cave'));
    expect(yellow.ring).toBeNull();
    const white = prosePillColors('#f6f6f2', pillThemeOf('light'));
    expect(white.ring).not.toBeNull();
    const blue = prosePillColors('#1f4fff', pillThemeOf('light'));
    expect(blue.ring).toBeNull();
  });

  it.each(THEMES)('%s: the muted words stay apart from the main text', (name) => {
    const theme = pillThemeOf(name);
    const apart = Math.min(1.4, contrastRatio(theme.text, theme.muted));
    for (const color of COLORS) {
      const p = prosePillColors(color, theme);
      expect(contrastRatio(p.text, p.muted), color).toBeGreaterThanOrEqual(apart - 0.01);
    }
  });

  it.each(THEMES)('%s: the glow lightens a dark panel and darkens a light one, clearly off it', (name) => {
    const theme = pillThemeOf(name);
    const panelL = oklchOfHex(theme.panel).L;
    for (const color of COLORS) {
      const p = prosePillColors(color, theme);
      const off = oklchOfHex(p.bg).L - panelL;
      expect(theme.dark ? off : -off, color).toBeGreaterThan(0.025);
      expect(contrastRatio(p.bg, theme.panel), color).toBeGreaterThanOrEqual(1.1);
      expect(p.hoverBg).toBe(theme.panel);
    }
  });

  // nord's text leaves the glow one lightness (its panel is light for a dark
  // theme): its greys differ on link pills by their borders only.
  it.each(THEMES.filter((t) => t !== 'nord'))('%s: a near-white, a grey and a near-black line glow apart, the pole-most furthest', (name) => {
    const theme = pillThemeOf(name);
    const panelL = oklchOfHex(theme.panel).L;
    const off = (c: string) => Math.abs(oklchOfHex(prosePillColors(c, theme).bg).L - panelL);
    const [white, grey, black] = [off('#f6f6f2'), off('#808080'), off('#101010')];
    const [near, mid, far] = theme.dark ? [black, grey, white] : [white, grey, black];
    // Spread over the room the theme's text leaves: 0.04 to 0.05 of OKLCH
    // lightness apart on the light themes, 0.026 to 0.05 on the dark ones.
    expect(mid - near).toBeGreaterThan(0.02);
    expect(far - mid).toBeGreaterThan(0.02);
  });

  it.each(THEMES)("%s: a coloured line's glow keeps its hue", (name) => {
    const theme = pillThemeOf(name);
    for (const color of COLORS) {
      if (!/^#[0-9a-f]{6}$/.test(color)) continue;
      const line = oklchOfHex(color);
      const glow = oklchOfHex(prosePillColors(color, theme).bg);
      if (line.C < 0.08 || glow.C < 0.03) continue;
      const gap = Math.abs(glow.h - line.h) % 360;
      expect(Math.min(gap, 360 - gap), color).toBeLessThan(25);
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

  it.each(THEMES)('%s: over the panel the wash at the glow alpha is the glow itself, within one 8-bit step', (name) => {
    const theme = pillThemeOf(name);
    for (const color of COLORS) {
      const target = prosePillColors(color, theme).bg;
      const { tint, clear } = proseTintOf(target, theme.panel);
      const o = rgba(tint);
      const panel = hex(theme.panel);
      const composed = panel.map((p, i) => p * (1 - o.a) + o.rgb[i]! * o.a);
      hex(target).forEach((t, i) => expect(Math.abs(composed[i]! - t), `${name} ${color}`).toBeLessThanOrEqual(1.01));
      expect(o.a).toBe(PROSE_ALPHA);
      expect(rgba(clear).rgb).toEqual(o.rgb);
      expect(rgba(clear).a).toBe(0);
    }
  });

  it('a tint equal to the panel is no overlay at all', () => {
    expect(proseTintOf('#202020', '#202020')).toEqual({ tint: 'rgba(0, 0, 0, 0)', clear: 'rgba(0, 0, 0, 0)' });
  });
});
