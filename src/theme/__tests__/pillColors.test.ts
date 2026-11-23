/**
 * Line pill legibility, measured over a sweep of line colours on every theme.
 *
 * A line colour is user data: any CSS colour. The sweep covers a hue sweep at
 * several OKLCH lightness and chroma levels, black, white, the greys, near
 * white and near black, pure yellow and blue, every CSS name, the app's line
 * palettes, a translucent colour and an unpaintable one. The contrasts are
 * measured with `utils/colors` (`contrastRatio`), not the module's own maths.
 */
import { describe, expect, it } from 'vitest';
import { themeTokens } from '../tokens';
import { COMMENT_PALETTE, LIGHT_LINE_COLORS, LINE_COLORS, contrastRatio } from '../../utils/colors';
import { CSS_COLOR_NAMES, hexOfOklch, oklchOfHex } from '../cssColor';
import {
  PILL_DOT_MIN,
  PILL_MUTED_MIN,
  PILL_TEXT_MIN,
  missingPillColors,
  pillColors,
  pillColorsOf,
  pillDot,
  pillMarkOf,
  pillThemeOf,
  type PillColors,
  type PillTheme,
} from '../pillColors';

/** A border has to read as an edge on the panel and around the fill. */
const BORDER_MIN = 1.5;

const THEMES = Object.keys(themeTokens);

function sweepColors(): string[] {
  const out: string[] = [];
  for (const L of [0.12, 0.25, 0.4, 0.55, 0.7, 0.85, 0.95, 0.99]) {
    for (const C of [0, 0.03, 0.08, 0.15, 0.25, 0.37]) {
      for (let h = 0; h < 360; h += 10) out.push(hexOfOklch({ L, C, h }));
    }
  }
  for (let v = 0; v <= 255; v += 15) {
    const x = v.toString(16).padStart(2, '0');
    out.push(`#${x}${x}${x}`);
  }
  out.push('#000', '#fff', '#050505', '#0a0a0a', '#fafafa', '#fdfdfd', '#ffff00', '#0000ff', '#1e40ff', '#00ff00', '#ff0000');
  out.push(...CSS_COLOR_NAMES, ...LINE_COLORS, ...LIGHT_LINE_COLORS, ...COMMENT_PALETTE);
  out.push('rgba(0, 0, 255, 0.3)', 'hsl(60 100% 50%)', 'oklch(0.5 0.2 250)', 'not-a-colour', '');
  return out;
}

const COLORS = sweepColors();

function hueGap(a: number, b: number): number {
  const d = Math.abs(a - b) % 360;
  return d > 180 ? 360 - d : d;
}

describe('pillColors sweep', () => {
  it.each(THEMES)('%s: main text 7:1, muted 4.5:1, dot or ring 3:1, a visible border, at rest and on hover', (name) => {
    const theme = pillThemeOf(name);
    const failures: string[] = [];
    const check = (label: string, value: number, min: number) => {
      if (!(value >= min)) failures.push(`${label} ${value.toFixed(2)} < ${min}`);
    };
    for (const color of COLORS) {
      const p = pillColors(color, theme);
      for (const [state, bg] of [['rest', p.bg], ['hover', p.hoverBg]] as const) {
        check(`${color} ${state} text`, contrastRatio(p.text, bg), PILL_TEXT_MIN);
        check(`${color} ${state} muted`, contrastRatio(p.muted, bg), PILL_MUTED_MIN);
        check(`${color} ${state} dot`, contrastRatio(p.ring ?? p.dot, bg), PILL_DOT_MIN);
      }
      check(`${color} border/panel`, contrastRatio(p.border, theme.panel), BORDER_MIN);
      check(`${color} border/fill`, contrastRatio(p.border, p.bg), BORDER_MIN);
      check(`${color} hover border/panel`, contrastRatio(p.hoverBorder, theme.panel), BORDER_MIN);
    }
    expect(failures).toEqual([]);
  });

  it.each(THEMES)('%s: a ring only where the colour alone does not reach 3:1', (name) => {
    const theme = pillThemeOf(name);
    for (const color of COLORS) {
      const p = pillColors(color, theme);
      const alone = Math.min(contrastRatio(p.dot, p.bg), contrastRatio(p.dot, p.hoverBg));
      if (p.ring === null) expect(alone, color).toBeGreaterThanOrEqual(PILL_DOT_MIN);
      else expect(alone, color).toBeLessThan(PILL_DOT_MIN + 0.3);
    }
  });

  it.each(THEMES)('%s: hover is a visible step from rest', (name) => {
    const theme = pillThemeOf(name);
    for (const color of COLORS) {
      const p = pillColors(color, theme);
      expect(p.hoverBg, color).not.toBe(p.bg);
      expect(contrastRatio(p.hoverBorder, theme.panel), color).toBeGreaterThan(contrastRatio(p.border, theme.panel));
    }
  });

  it.each(THEMES)('%s: every fill sits in one narrow lightness band (one family)', (name) => {
    const theme = pillThemeOf(name);
    const panelL = oklchOfHex(theme.panel).L;
    const offsets = COLORS.map((c) => oklchOfHex(pillColors(c, theme).bg).L - panelL);
    const spread = Math.max(...offsets) - Math.min(...offsets);
    expect(spread).toBeLessThan(0.06);
    for (const o of offsets) {
      expect(Math.abs(o)).toBeGreaterThan(0.015);
      expect(Math.abs(o)).toBeLessThan(0.1);
    }
  });

  it.each(THEMES)("%s: tints keep the line's hue, with chroma capped", (name) => {
    const theme = pillThemeOf(name);
    for (const color of COLORS) {
      if (!/^#[0-9a-f]{6}$/.test(color)) continue;
      const line = oklchOfHex(color);
      const p = pillColors(color, theme);
      const bg = oklchOfHex(p.bg);
      const border = oklchOfHex(p.border);
      expect(bg.C, color).toBeLessThanOrEqual(0.055);
      expect(border.C, color).toBeLessThanOrEqual(0.105);
      if (line.C >= 0.08) {
        // Low-chroma tints lose hue precision to 8-bit rounding; the border
        // carries more chroma, so it is held tighter.
        expect(hueGap(border.h, line.h), `${color} border`).toBeLessThan(12);
        if (bg.C >= 0.02) expect(hueGap(bg.h, line.h), `${color} fill`).toBeLessThan(25);
      }
      if (line.C < 0.02) {
        expect(bg.C, `${color} grey`).toBeLessThan(0.012);
        expect(border.C, `${color} grey`).toBeLessThan(0.012);
      }
    }
  });

  it('the dot is the line colour itself, translucency flattened on the panel', () => {
    const dark = pillThemeOf('dark');
    expect(pillColors('#4ECDC4', dark).dot).toBe('#4ecdc4');
    expect(pillColors('gold', dark).dot).toBe('#ffd700');
    expect(pillColors('rgb(255 0 0)', dark).dot).toBe('#ff0000');
    // 30% blue on #16213e.
    expect(pillColors('rgba(0, 0, 255, 0.3)', dark).dot).toBe('#0f1778');
  });

  it('rings sit on the text side: light on a dark theme, dark on a light one', () => {
    const dark = pillThemeOf('dark');
    const light = pillThemeOf('light');
    const onDark = pillColors('#050505', dark);
    expect(onDark.ring).not.toBeNull();
    expect(oklchOfHex(onDark.ring!).L).toBeGreaterThan(oklchOfHex(onDark.bg).L);
    const onLight = pillColors('#fafafa', light);
    expect(onLight.ring).not.toBeNull();
    expect(oklchOfHex(onLight.ring!).L).toBeLessThan(oklchOfHex(onLight.bg).L);
    // Yellow on the light theme: an olive ring in yellow's hue.
    const yellow = pillColors('#ffff00', light);
    expect(yellow.ring).not.toBeNull();
    expect(hueGap(oklchOfHex(yellow.ring!).h, oklchOfHex('#ffff00').h)).toBeLessThan(12);
    // A saturated blue reads on the light fill by itself.
    expect(pillColors('#0000ff', light).ring).toBeNull();
  });

  it('an unpaintable colour is a neutral pill with a muted dot', () => {
    const dark = pillThemeOf('dark');
    const p = pillColors('not-a-colour', dark);
    expect(oklchOfHex(p.bg).C).toBeLessThan(0.012);
    expect(p.dot).toBe(dark.muted);
    expect(pillColors(null, dark)).toEqual(p);
  });
});

describe('pill text comes from the theme tokens', () => {
  it.each(['dark', 'light', 'cave', 'dracula', 'one_dark', 'catppuccin_latte', 'nord', 'catppuccin_mocha', 'kaleido_eink'])(
    '%s keeps --cc-text-primary and --cc-text-secondary as they are',
    (name) => {
      const tokens = (themeTokens as Record<string, Record<string, string>>)[name]!;
      const theme = pillThemeOf(name);
      expect(theme.text).toBe(tokens['--cc-text-primary']!.toLowerCase());
      expect(theme.muted).toBe(tokens['--cc-text-secondary']!.toLowerCase());
    },
  );

  it('dark lifts its pills above the panel and light sinks them below it', () => {
    expect(pillThemeOf('dark').lift).toBe(1);
    expect(pillThemeOf('light').lift).toBe(-1);
  });

  it("solarized_light's own text cannot reach 7:1 on its panel, so its pills darken it, same hue", () => {
    const tokens = themeTokens.solarized_light;
    expect(contrastRatio(tokens['--cc-text-primary'], tokens['--cc-panel'])).toBeLessThan(PILL_TEXT_MIN);
    const theme = pillThemeOf('solarized_light');
    expect(theme.text).not.toBe(tokens['--cc-text-primary'].toLowerCase());
    expect(oklchOfHex(theme.text).L).toBeLessThan(oklchOfHex(tokens['--cc-text-primary']).L);
    expect(hueGap(oklchOfHex(theme.text).h, oklchOfHex(tokens['--cc-text-primary']).h)).toBeLessThan(15);
  });
});

describe('missingPillColors', () => {
  it.each(THEMES)('%s: neutral, text 7:1, muted 4.5:1, warning glyph 3:1, border visible', (name) => {
    const theme = pillThemeOf(name);
    const m = missingPillColors(theme);
    expect(oklchOfHex(m.bg).C).toBeLessThan(0.012);
    expect(contrastRatio(m.text, m.bg)).toBeGreaterThanOrEqual(PILL_TEXT_MIN);
    expect(contrastRatio(m.muted, m.bg)).toBeGreaterThanOrEqual(PILL_MUTED_MIN);
    expect(contrastRatio(m.warning, m.bg)).toBeGreaterThanOrEqual(PILL_DOT_MIN);
    expect(contrastRatio(m.border, theme.panel)).toBeGreaterThanOrEqual(BORDER_MIN);
  });
});

describe('the home-line mark (second dot)', () => {
  it.each(THEMES)('%s: the mark or its ring reaches 3:1 on the pill it sits in, at rest and on hover', (name) => {
    const host = ['#4ecdc4', '#ffff00', '#0a0a0a', '#fafafa', '#0000ff'];
    const marks = ['#ff6b6b', '#000000', '#ffffff', '#ffff00', 'navy', '#808080'];
    for (const h of host) {
      const on: PillColors = pillColorsOf(h, name);
      for (const m of marks) {
        const mark = pillMarkOf(m, on, name);
        const shown = mark.ring ?? mark.dot;
        expect(contrastRatio(shown, on.bg), `${m} on ${h}`).toBeGreaterThanOrEqual(PILL_DOT_MIN);
        expect(contrastRatio(shown, on.hoverBg), `${m} on ${h} hover`).toBeGreaterThanOrEqual(PILL_DOT_MIN);
      }
    }
  });

  it('pillDot leaves a colour that already reads alone', () => {
    const theme: PillTheme = pillThemeOf('dark');
    expect(pillDot('#ffffff', ['#16213e'], theme)).toEqual({ dot: '#ffffff', ring: null });
  });
});

describe('pillColorsOf', () => {
  it('is pillColors on the named theme, cached', () => {
    const a = pillColorsOf('#ff6b6b', 'light');
    expect(a).toEqual(pillColors('#ff6b6b', pillThemeOf('light')));
    expect(pillColorsOf('#ff6b6b', 'light')).toBe(a);
  });

  it('an unknown theme name is the dark theme', () => {
    expect(pillThemeOf('no-such-theme')).toEqual(pillThemeOf('dark'));
  });
});
