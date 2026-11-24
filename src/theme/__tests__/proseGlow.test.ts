/**
 * RS-NODOTS: the prose glow's shape and its two-colour blend. The CSS holds
 * the same geometry as `GLOW` (a one-colour pill is CSS alone, a two-colour
 * one an SVG built from `GLOW`, and the two must read alike). Across the
 * arrow the colour runs straight from one line's glow to the other's in
 * OKLab: its lightness stays between the two, never dipping toward the
 * panel (the user: "they should blend into each other, not blend to white
 * and white to second color"). Injections: an sRGB-to-panel fade at the
 * bridge's middle fails "no dip"; a CSS value out of step with GLOW fails
 * "the CSS holds GLOW".
 */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { themeTokens } from '../tokens';
import { oklchOfHex } from '../cssColor';
import { mixHex, SEAM_EM } from '../pillBlend';
import { PROSE_ALPHA, pillThemeOf, prosePillColorsOf } from '../pillColors';
import { GLOW, GLOW_REACH, colourAlong, glowSvg, samplePoints, type PieceSegment } from '../proseGlow';

const HARD = ['#f6f6f2', '#101010', '#ffe600', '#1f4fff', '#808080', '#c2185b', '#00ff00', 'navy'];

describe('the glow geometry', () => {
  it('the CSS holds GLOW', () => {
    const css = readFileSync(resolve(__dirname, '../../index.css'), 'utf8');
    const rule = /\n\.sh-ppill \{([^}]*)\}/.exec(css)?.[1] ?? '';
    const value = (name: string) => Number(new RegExp(`--glow-${name}: ([-0-9.]+);`).exec(rule)?.[1]);
    expect(value('s')).toBe(GLOW.scale);
    expect(value('pt')).toBe(GLOW.padTop);
    expect(value('pb')).toBe(GLOW.padBottom);
    expect(value('px')).toBe(GLOW.padX);
    expect(value('mx')).toBe(GLOW.marginX);
    expect(value('spread')).toBe(GLOW.spread);
    expect(value('blur')).toBe(GLOW.blur);
    expect(rule).toContain('--glow-reach: calc(var(--glow-spread) + 1.5 * var(--glow-blur));');
    expect(GLOW_REACH).toBeCloseTo(GLOW.spread + 1.5 * GLOW.blur, 10);
  });

  it('a pill takes a little more room than its words, and its halo reaches beyond its core', () => {
    expect(GLOW.padX + GLOW.marginX).toBeGreaterThan(0);
    expect(GLOW.padX + GLOW.marginX).toBeLessThan(0.2);
    expect(GLOW.marginX).toBeLessThan(0);
    expect(GLOW_REACH).toBeGreaterThan(0.4);
  });
});

describe('colourAlong (the two-colour blend)', () => {
  const piece = (a: string, b: string): PieceSegment[] => [
    { left: 0, right: 40, colour: a },
    { left: 40, right: 60, colour: a, bridgeTo: b },
    { left: 60, right: 100, colour: b },
  ];

  it('runs from one colour to the other across the arrow, the midpoint at its middle, in OKLab', () => {
    const [a, b] = ['#f2e798', '#c2d5ff'];
    expect(colourAlong(piece(a, b), 20, 5)).toBe(a);
    expect(colourAlong(piece(a, b), 40, 5)).toBe(a);
    expect(colourAlong(piece(a, b), 50, 5)).toBe(mixHex(a, b, 0.5));
    expect(colourAlong(piece(a, b), 60, 5)).toBe(b);
    expect(colourAlong(piece(a, b), 80, 5)).toBe(b);
    // The rounded ends keep the colour of their end.
    expect(colourAlong(piece(a, b), -10, 5)).toBe(a);
    expect(colourAlong(piece(a, b), 110, 5)).toBe(b);
  });

  it.each(Object.keys(themeTokens))('%s: no dip toward the panel between two glows, for every pair of hard colours', (name) => {
    const panelL = oklchOfHex(pillThemeOf(name).panel).L;
    for (const x of HARD) {
      for (const y of HARD) {
        if (x === y) continue;
        const [a, b] = [prosePillColorsOf(x, name).bg, prosePillColorsOf(y, name).bg];
        const off = (c: string) => Math.abs(oklchOfHex(c).L - panelL);
        const least = Math.min(off(a), off(b));
        for (let px = 40; px <= 60; px += 1) {
          // Within 8-bit rounding of the nearer end's distance from the panel.
          expect(off(colourAlong(piece(a, b), px, 5)), `${name} ${x}->${y} at ${px}`).toBeGreaterThanOrEqual(least - 0.006);
        }
      }
    }
  });

  it('a seam between two regular segments blends half way at the boundary, its own colour a seam in', () => {
    const [h, a] = ['#c7c7c7', '#f2e798'];
    const segs: PieceSegment[] = [
      { left: 0, right: 50, colour: h },
      { left: 50, right: 60, colour: a },
    ];
    expect(colourAlong(segs, 49.999, 6)).toBe(mixHex(h, a, 0.5));
    expect(colourAlong(segs, 50, 6)).toBe(mixHex(a, h, 0.5));
    expect(colourAlong(segs, 44, 6)).toBe(h);
    expect(colourAlong(segs, 56, 6)).toBe(a);
  });

  it('samples a bridge at its eighths and a seam at eighths of its blend, inside the box only', () => {
    const segs: PieceSegment[] = [
      { left: 0, right: 50, colour: '#c7c7c7' },
      { left: 50, right: 60, colour: '#f2e798' },
      { left: 60, right: 76, colour: '#f2e798', bridgeTo: '#c2d5ff' },
      { left: 76, right: 90, colour: '#c2d5ff' },
    ];
    const xs = samplePoints(segs, -10, 100, 4);
    for (const x of [46, 47, 48, 49, 50, 51, 52, 53, 54, 60, 62, 64, 66, 68, 70, 72, 74, 76]) expect(xs).toContain(x);
    expect(xs[0]).toBe(-10);
    expect(xs[xs.length - 1]).toBe(100);
    expect(samplePoints(segs, 63, 65, 4)).toEqual([63, 64, 65]);
    expect(SEAM_EM).toBeGreaterThan(0);
  });
});

describe('glowSvg', () => {
  const decode = (url: string) => decodeURIComponent(/data:image\/svg\+xml,(.*)"\)$/.exec(url)?.[1] ?? '');
  const base = { width: 120, height: 30, reach: 8, spread: 2, sigma: 2, panel: '#ffffff' };

  it('is the core with rounded ends and the halo, a blurred grown core, over washes at the glow alpha', () => {
    const svg = decode(glowSvg({ ...base, capLeft: true, capRight: true, stops: [[0, '#f2e798'], [120, '#c2d5ff']] }));
    // The core: from the reach in, 14 px tall, its ends half its height round.
    expect(svg).toContain(`<rect x='8' y='8' width='104' height='14' rx='7' fill='#fff'/>`);
    // The halo: the core grown by the spread, blurred.
    expect(svg).toContain(`<rect x='6' y='6' width='108' height='18' rx='9' fill='#fff' filter='url(#b)'/>`);
    expect(svg).toContain(`<feGaussianBlur stdDeviation='2'/>`);
    const opacities = [...svg.matchAll(/stop-opacity='([^']+)'/g)].map((m) => Number(m[1]));
    expect(opacities).toEqual([PROSE_ALPHA, PROSE_ALPHA]);
  });

  it('runs on past a side with no rounded end', () => {
    const svg = decode(glowSvg({ ...base, capLeft: false, capRight: true, stops: [[0, '#f2e798']] }));
    const core = /<rect x='([-0-9.]+)' y='8' width='([0-9.]+)'/.exec(svg)!;
    expect(Number(core[1])).toBeLessThan(-100);
    expect(Number(core[1]) + Number(core[2])).toBe(112);
  });
});
