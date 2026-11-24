/**
 * Line pill legibility, measured over a sweep of line colours on every theme.
 *
 * A line colour is user data: any CSS colour. The sweep covers a hue sweep at
 * several OKLCH lightness and chroma levels, black, white, the greys, near
 * white and near black, pure yellow and blue, every CSS name, the app's line
 * palettes, a translucent colour and an unpaintable one. The contrasts are
 * measured with `utils/colors` (`contrastRatio`), not the module's own maths.
 *
 * Since RS-NODOTS a pill has no colour dot: its line shows by its fill (a
 * tint of the line's hue) and its border (the line colour itself, moved
 * toward the text only as far as it must go to stand out from the panel and
 * the fill), so a near-white, a grey and a near-black line, which share a
 * neutral fill, differ by their borders.
 */
import { describe, expect, it } from 'vitest';
import { themeTokens } from '../tokens';
import { COMMENT_PALETTE, LIGHT_LINE_COLORS, LINE_COLORS, contrastRatio } from '../../utils/colors';
import { CSS_COLOR_NAMES, hexOfOklch, oklchOfHex } from '../cssColor';
import {
  BORDER_MIN,
  PILL_DOT_MIN,
  PILL_MUTED_MIN,
  PILL_TEXT_MIN,
  missingPillColors,
  pillColors,
  pillColorsOf,
  pillDot,
  pillThemeOf,
  type PillTheme,
} from '../pillColors';

/** The missing-target pill's grey border has to read as an edge on the panel. */
const MISSING_BORDER_MIN = 1.5;

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
  it.each(THEMES)('%s: main text 7:1, muted 4.5:1, a border that stands out from the panel and the fill, at rest and on hover', (name) => {
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
      }
      check(`${color} border/panel`, contrastRatio(p.border, theme.panel), BORDER_MIN);
      check(`${color} border/fill`, contrastRatio(p.border, p.bg), BORDER_MIN);
      check(`${color} hover border/panel`, contrastRatio(p.hoverBorder, theme.panel), BORDER_MIN);
      check(`${color} hover border/fill`, contrastRatio(p.hoverBorder, p.hoverBg), BORDER_MIN);
    }
    expect(failures).toEqual([]);
  });

  it.each(THEMES)('%s: the border is the line colour itself wherever that already stands out', (name) => {
    const theme = pillThemeOf(name);
    for (const color of COLORS) {
      if (!/^#[0-9a-f]{6}$/.test(color)) continue;
      const p = pillColors(color, theme);
      const alone = Math.min(contrastRatio(color, theme.panel), contrastRatio(color, p.bg));
      if (alone >= BORDER_MIN + 0.05) expect(p.border, color).toBe(color);
      // Moved only as far as it must go: never far past the floor.
      else expect(Math.min(contrastRatio(p.border, theme.panel), contrastRatio(p.border, p.bg)), color).toBeLessThan(BORDER_MIN + 0.6);
    }
  });

  it.each(THEMES)('%s: hover is a visible step from rest', (name) => {
    const theme = pillThemeOf(name);
    for (const color of COLORS) {
      const p = pillColors(color, theme);
      expect(p.hoverBg, color).not.toBe(p.bg);
      expect(contrastRatio(p.hoverBorder, theme.panel), color).toBeGreaterThanOrEqual(contrastRatio(p.border, theme.panel));
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
      expect(bg.C, color).toBeLessThanOrEqual(0.085);
      if (line.C >= 0.08) {
        // Low-chroma tints lose hue precision to 8-bit rounding; the border
        // carries the line's own chroma, so it is held tighter.
        if (border.C >= 0.03) expect(hueGap(border.h, line.h), `${color} border`).toBeLessThan(12);
        if (bg.C >= 0.02) expect(hueGap(bg.h, line.h), `${color} fill`).toBeLessThan(25);
      }
      if (line.C < 0.02) {
        // A grey's fill is neutral; its border is its own colour, no more chroma than it has.
        expect(bg.C, `${color} grey`).toBeLessThan(0.012);
        expect(border.C, `${color} grey`).toBeLessThanOrEqual(line.C + 0.005);
      }
    }
  });

  it('the border is the line colour itself, translucency flattened on the panel', () => {
    const dark = pillThemeOf('dark');
    expect(pillColors('#4ECDC4', dark).border).toBe('#4ecdc4');
    expect(pillColors('gold', dark).border).toBe('#ffd700');
    expect(pillColors('rgb(255 0 0)', dark).border).toBe('#ff0000');
    // 30% blue on #16213e is #0f1778, too close to the panel: moved toward the text.
    const veiled = oklchOfHex(pillColors('rgba(0, 0, 255, 0.3)', dark).border);
    expect(veiled.L).toBeGreaterThan(oklchOfHex('#0f1778').L);
    expect(hueGap(veiled.h, oklchOfHex('#0f1778').h)).toBeLessThan(12);
  });

  it('a border moves toward the text side: lighter on a dark theme, darker on a light one, in its hue', () => {
    const dark = pillThemeOf('dark');
    const light = pillThemeOf('light');
    const onDark = pillColors('#050505', dark);
    expect(oklchOfHex(onDark.border).L).toBeGreaterThan(oklchOfHex(onDark.bg).L);
    const onLight = pillColors('#fafafa', light);
    expect(oklchOfHex(onLight.border).L).toBeLessThan(oklchOfHex(onLight.bg).L);
    // Yellow on the light theme: an olive border in yellow's hue.
    const yellow = oklchOfHex(pillColors('#ffff00', light).border);
    expect(yellow.L).toBeLessThan(oklchOfHex('#ffff00').L);
    expect(hueGap(yellow.h, oklchOfHex('#ffff00').h)).toBeLessThan(12);
    // A saturated blue stands out on the light fill by itself.
    expect(pillColors('#0000ff', light).border).toBe('#0000ff');
  });

  it.each(THEMES)('%s: near-white, grey and near-black, which share a neutral fill, differ by their borders', (name) => {
    const theme = pillThemeOf(name);
    const [white, grey, black] = ['#f6f6f2', '#808080', '#101010'].map((c) => pillColors(c, theme));
    expect(contrastRatio(white!.border, black!.border)).toBeGreaterThanOrEqual(2);
    // A mid grey can land near a near-black line's lifted border (nord: 1.29),
    // but never on it.
    for (const [a, b] of [[white!, grey!], [grey!, black!]] as const) {
      expect(contrastRatio(a.border, b.border)).toBeGreaterThanOrEqual(1.25);
    }
  });

  it('an unpaintable colour is a neutral pill with a neutral border', () => {
    const dark = pillThemeOf('dark');
    const p = pillColors('not-a-colour', dark);
    expect(oklchOfHex(p.bg).C).toBeLessThan(0.012);
    expect(oklchOfHex(p.border).C).toBeLessThan(0.012);
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
    expect(contrastRatio(m.border, theme.panel)).toBeGreaterThanOrEqual(MISSING_BORDER_MIN);
  });
});

describe('pillDot (the Sections panel row dot)', () => {
  it('leaves a colour that already reads alone', () => {
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
