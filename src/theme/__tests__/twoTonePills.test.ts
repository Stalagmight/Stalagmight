/**
 * RS-OWNERSHIP: a shot pill with an end of another line is two-tone, the
 * half holding that end in that line's colours and the rest in the shot's.
 * Each half must meet the floors on its own background: main text 7:1, the
 * muted context 4.5:1, the leading dot (the shot's line) 3:1 on the shot's
 * half, and the foreign end's dot (its own line's colour) 3:1 on its half,
 * at rest and on hover for a link pill, on the tint and on the bare panel
 * for a prose pill. Swept over ordered pairs of hard line colours (near-white
 * next to near-black, yellow next to blue, greys, pure primaries, a hue
 * sweep) on every theme, measured with `utils/colors`.
 * Injection: drawing the foreign end's dot in its bare line colour, without
 * the ring `pillMarkOf` adds, fails both sweeps for a near-black end on the
 * dark themes (about 1:1 on its own half).
 */
import { describe, expect, it } from 'vitest';
import { themeTokens } from '../tokens';
import { hexOfOklch } from '../cssColor';
import { contrastRatio } from '../../utils/colors';
import {
  PILL_DOT_MIN,
  PILL_MUTED_MIN,
  PILL_TEXT_MIN,
  pillColors,
  pillMarkOf,
  pillThemeOf,
  prosePillColors,
  type PillColors,
} from '../pillColors';

const THEMES = Object.keys(themeTokens);

const HARD = [
  '#f6f6f2', // near-white
  '#101010', // near-black
  '#ffe600', // yellow
  '#1f4fff', // blue
  '#8a8a8a', // grey
  '#000000',
  '#ffffff',
  '#ff0000',
  '#00ff00',
  'navy',
  '#c2185b',
  ...[0, 60, 120, 180, 240, 300].flatMap((h) => [hexOfOklch({ L: 0.5, C: 0.15, h }), hexOfOklch({ L: 0.88, C: 0.12, h })]),
];

const PAIRS: Array<[string, string]> = HARD.flatMap((a) => HARD.filter((b) => b !== a).map((b) => [a, b] as [string, string]));

/** The floors of one half: its text, muted part and the dots drawn on it, on each of its backgrounds. */
function checkHalf(
  label: string,
  half: PillColors,
  backgrounds: readonly string[],
  dots: ReadonlyArray<{ dot: string; ring: string | null }>,
  failures: string[],
) {
  const check = (what: string, value: number, min: number) => {
    if (!(value >= min)) failures.push(`${label} ${what} ${value.toFixed(2)} < ${min}`);
  };
  for (const bg of backgrounds) {
    check(`text on ${bg}`, contrastRatio(half.text, bg), PILL_TEXT_MIN);
    check(`muted on ${bg}`, contrastRatio(half.muted, bg), PILL_MUTED_MIN);
    for (const d of dots) check(`dot ${d.dot} on ${bg}`, contrastRatio(d.ring ?? d.dot, bg), PILL_DOT_MIN);
  }
}

describe('two-tone shot pills (RS-OWNERSHIP)', () => {
  it.each(THEMES)('%s: each half of a link pill meets the floors at rest and on hover', (name) => {
    const theme = pillThemeOf(name);
    const failures: string[] = [];
    for (const [shotLine, owner] of PAIRS) {
      const shotHalf = pillColors(shotLine, theme);
      const ownerHalf = pillColors(owner, theme);
      // The leading dot is the shot's line on the shot's half; the foreign
      // end's dot is the owner's line on the owner's half (`markOf` with the
      // owner's resolved pill).
      checkHalf(`${shotLine}|${owner} shot half`, shotHalf, [shotHalf.bg, shotHalf.hoverBg], [shotHalf], failures);
      const endDot = pillMarkOf(owner, ownerHalf, name);
      checkHalf(`${shotLine}|${owner} owner half`, ownerHalf, [ownerHalf.bg, ownerHalf.hoverBg], [endDot], failures);
    }
    expect(failures).toEqual([]);
  });

  it.each(THEMES)('%s: each half of a prose pill meets the floors on its tint and on the panel', (name) => {
    const theme = pillThemeOf(name);
    const failures: string[] = [];
    for (const [shotLine, owner] of PAIRS) {
      const shotHalf = prosePillColors(shotLine, theme);
      const ownerHalf = prosePillColors(owner, theme);
      checkHalf(`${shotLine}|${owner} shot half`, shotHalf, [shotHalf.bg, theme.panel], [shotHalf], failures);
      const endDot = pillMarkOf(owner, ownerHalf, name);
      checkHalf(`${shotLine}|${owner} owner half`, ownerHalf, [ownerHalf.bg, theme.panel], [endDot], failures);
    }
    expect(failures).toEqual([]);
  });

  it.each(THEMES)('%s: near-white next to near-black gives two different halves, both legible', (name) => {
    const theme = pillThemeOf(name);
    const white = pillColors('#f6f6f2', theme);
    const black = pillColors('#101010', theme);
    // The halves are told apart by their fills and their dots.
    expect(white.bg).not.toBe(black.bg);
    expect(white.dot).not.toBe(black.dot);
    for (const half of [white, black]) {
      expect(contrastRatio(half.text, half.bg)).toBeGreaterThanOrEqual(PILL_TEXT_MIN);
      expect(contrastRatio(half.ring ?? half.dot, half.bg)).toBeGreaterThanOrEqual(PILL_DOT_MIN);
    }
  });
});
