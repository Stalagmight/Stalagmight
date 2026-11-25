/**
 * Line pill legibility, measured over a sweep of line colours on every theme.
 *
 * A line colour is user data: any CSS colour. The sweep covers a hue sweep at
 * several OKLCH lightness and chroma levels, black, white, the greys, near
 * white and near black, pure yellow and blue, every CSS name, the app's line
 * palettes, a translucent colour and an unpaintable one. The contrasts are
 * measured with `utils/colors` (`contrastRatio`), not the module's own maths.
 *
 * A pill's line shows by its fill (a tint of the line's hue), its border
 * (the line colour itself, moved toward the text only as far as it must go
 * to stand out from the panel and the fill, RS-NODOTS), so a near-white, a
 * grey and a near-black line, which share a neutral fill, differ by their
 * borders, and its leading dot (the line colour, ringed only where it alone
 * does not reach 3:1 on the fill, RS-LEADDOT). A foreign station's
 * superscript home dot is its line's colour at a lower floor, 2:1
 * (`HOME_DOT_MIN`: it repeats what its words say, and the user asked for it
 * "more subtle"), ringed only below it, on its owner's pill (`PillColors.home`)
 * and on a slot's pill of another line (`homeDotOn`), swept over every
 * colour and over pairs of line colours; its ring is never stronger than the
 * leading dot's.
 * Injection: dropping the ring (`ring: null` from `pillDot`) fails "dot or
 * ring 3:1" for a near-black line on the dark themes and a near-white or a
 * yellow one on the light themes.
 */
import { describe, expect, it } from 'vitest';
import { themeTokens } from '../tokens';
import { COMMENT_PALETTE, LIGHT_LINE_COLORS, LINE_COLORS, contrastRatio } from '../../utils/colors';
import { ringHues } from './sweepDensity';
import { CSS_COLOR_NAMES, hexOfOklch, oklchOfHex } from '../cssColor';
import {
  BORDER_MIN,
  HOME_DOT_MIN,
  PILL_DOT_MIN,
  PILL_MUTED_MIN,
  PILL_TEXT_MIN,
  homeDotOn,
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
      for (const h of ringHues(10)) out.push(hexOfOklch({ L, C, h }));
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
  it.each(THEMES)('%s: main text 7:1, muted 4.5:1, dot or ring 3:1, a border that stands out from the panel and the fill, at rest and on hover', (name) => {
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

  it.each(THEMES)('%s: a ring only where the dot alone does not reach 3:1 on the fill (RS-LEADDOT)', (name) => {
    const theme = pillThemeOf(name);
    for (const color of COLORS) {
      const p = pillColors(color, theme);
      const alone = Math.min(contrastRatio(p.dot, p.bg), contrastRatio(p.dot, p.hoverBg));
      if (p.ring === null) expect(alone, color).toBeGreaterThanOrEqual(PILL_DOT_MIN);
      else expect(alone, color).toBeLessThan(PILL_DOT_MIN + 0.3);
    }
  });

  it.each(THEMES)('%s: a home dot on another line\'s pill (bySlot) or its ring 2:1 on the fill, at rest and on hover, ringed only where needed (RS-LEADDOT)', (name) => {
    // Every pill colour of a coarser sweep against every station line colour of it.
    const some = COLORS.filter((_, i) => i % 9 === 0).concat(['#f6f6f2', '#101010', '#ffe600', '#1f4fff', '#c2185b', '#808080', '#0000ff', '#7b2fff', '#ff9900']);
    const failures: string[] = [];
    for (const slot of some) {
      const on = pillColorsOf(slot, name);
      for (const owner of some) {
        const d = homeDotOn(owner, on, name);
        for (const bg of [on.bg, on.hoverBg]) {
          const c = contrastRatio(d.ring ?? d.dot, bg);
          if (!(c >= HOME_DOT_MIN)) failures.push(`${owner} on ${slot} (${bg}) ${c.toFixed(2)}`);
        }
        const alone = Math.min(contrastRatio(d.dot, on.bg), contrastRatio(d.dot, on.hoverBg));
        if (d.ring !== null && alone >= HOME_DOT_MIN + 0.3) failures.push(`${owner} on ${slot} ringed for nothing`);
      }
    }
    expect(failures.slice(0, 20)).toEqual([]);
  });

  it.each(THEMES)('%s: on its owner\'s pill the home dot is the line colour at 2:1 on the fill, ringed only where needed, never more than the leading dot (RS-LEADDOT)', (name) => {
    const theme = pillThemeOf(name);
    const failures: string[] = [];
    for (const color of COLORS) {
      const p = pillColors(color, theme);
      const h = p.home;
      if (h.dot !== p.dot) failures.push(`${color} home dot ${h.dot} is not the line colour ${p.dot}`);
      for (const bg of [p.bg, p.hoverBg]) {
        const c = contrastRatio(h.ring ?? h.dot, bg);
        if (!(c >= HOME_DOT_MIN)) failures.push(`${color} home on ${bg} ${c.toFixed(2)}`);
      }
      const alone = Math.min(contrastRatio(h.dot, p.bg), contrastRatio(h.dot, p.hoverBg));
      if (h.ring !== null && alone >= HOME_DOT_MIN + 0.3) failures.push(`${color} home ringed for nothing`);
      // Quieter, never louder: a ring only where the leading dot has one, and no stronger.
      if (h.ring !== null && p.ring === null) failures.push(`${color} home ringed, leading dot not`);
      if (h.ring !== null && p.ring !== null && contrastRatio(h.ring, p.bg) > contrastRatio(p.ring, p.bg) + 0.05) {
        failures.push(`${color} home ring ${h.ring} stronger than ${p.ring}`);
      }
    }
    expect(failures.slice(0, 20)).toEqual([]);
    // On its owner's pill it is what `homeDotOn` gives for the owner itself.
    for (const color of ['#f6f6f2', '#101010', '#ffe600', '#1f4fff', '#c2185b']) {
      const p = pillColorsOf(color, name);
      expect(homeDotOn(color, p, name)).toEqual(p.home);
    }
  });

  it('a near-white home dot on the light theme keeps a ring, a lighter one than the leading dot\'s (RS-LEADDOT)', () => {
    const p = pillColorsOf('#f6f6f2', 'light');
    expect(p.ring).not.toBeNull();
    expect(p.home.ring).not.toBeNull();
    expect(oklchOfHex(p.home.ring!).L).toBeGreaterThan(oklchOfHex(p.ring!).L);
    // Yellow on light: an olive ring at 2:1, lighter than the 3:1 one.
    const y = pillColorsOf('#ffe600', 'light');
    expect(y.home.ring).not.toBeNull();
    expect(oklchOfHex(y.home.ring!).L).toBeGreaterThan(oklchOfHex(y.ring!).L);
    // Blue and magenta read alone on light, at either floor.
    expect(pillColorsOf('#1f4fff', 'light').home.ring).toBeNull();
    expect(pillColorsOf('#c2185b', 'light').home.ring).toBeNull();
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

  it('the dot is the line colour itself, translucency flattened on the panel (RS-LEADDOT)', () => {
    const dark = pillThemeOf('dark');
    expect(pillColors('#4ECDC4', dark).dot).toBe('#4ecdc4');
    expect(pillColors('gold', dark).dot).toBe('#ffd700');
    expect(pillColors('rgb(255 0 0)', dark).dot).toBe('#ff0000');
    // 30% blue on #16213e.
    expect(pillColors('rgba(0, 0, 255, 0.3)', dark).dot).toBe('#0f1778');
  });

  it('rings sit on the text side, in the line\'s hue: light on a dark theme, dark on a light one', () => {
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

  it('an unpaintable colour is a neutral pill with a neutral border and a muted dot', () => {
    const dark = pillThemeOf('dark');
    const p = pillColors('not-a-colour', dark);
    expect(oklchOfHex(p.bg).C).toBeLessThan(0.012);
    expect(oklchOfHex(p.border).C).toBeLessThan(0.012);
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
    expect(contrastRatio(m.border, theme.panel)).toBeGreaterThanOrEqual(MISSING_BORDER_MIN);
  });
});

describe('pillDot (the pill dots and the Sections panel row dot)', () => {
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
