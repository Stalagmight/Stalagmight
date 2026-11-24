/**
 * RS-BLEND: two-tone pill segments blend at their seams. Each segment's fill
 * and border fade from its own colour to the OKLab midpoint with its
 * neighbour over the last 0.4em at a seam, so neighbours meet in the same
 * colour (no seam line), at rest and on hover. A prose segment's glow is
 * painted per line (`useProseGlow`, `proseGlow.test.ts`); its style carries
 * the words' colours, which must read on the blend toward a stronger
 * neighbour. Injections: a segment ending in its own colour at a seam fails
 * "neighbours meet in one colour"; a prose segment keeping its own muted
 * colour fails "words that read on the blend".
 */
import { describe, expect, it } from 'vitest';
import { linkBridgeStyle, linkSegmentStyle, mixHex, proseSegmentStyle, SEAM_EM } from '../pillBlend';
import { contrastOfHex } from '../cssColor';
import { pillColorsOf, pillThemeOf, prosePillColorsOf, proseTintOf } from '../pillColors';

const stops = (gradient: string) => {
  const inner = /linear-gradient\(to right, (.*)\)$/.exec(gradient)?.[1] ?? '';
  return inner.split(/,\s(?=#)/).map((s) => s.trim().split(' ')[0]!);
};
const lastOf = (xs: string[]) => xs[xs.length - 1];

describe('link segments (RS-BLEND)', () => {
  const A = pillColorsOf('#f6f6f2', 'dark');
  const B = pillColorsOf('#101010', 'dark');
  const C = pillColorsOf('#ffe600', 'dark');
  const head = linkSegmentStyle(A, { left: null, right: B }) as Record<string, string>;
  const from = linkSegmentStyle(B, { left: A, right: C }) as Record<string, string>;
  const to = linkSegmentStyle(C, { left: B, right: null }) as Record<string, string>;

  it('neighbours meet in one colour, fill and border, at rest and on hover', () => {
    for (const key of ['--seg-fill', '--seg-fill-hover', '--seg-edge', '--seg-edge-hover']) {
      expect(lastOf(stops(head[key]!))).toBe(stops(from[key]!)[0]);
      expect(lastOf(stops(from[key]!))).toBe(stops(to[key]!)[0]);
    }
    expect(lastOf(stops(head['--seg-fill']!))).toBe(mixHex(A.bg, B.bg, 0.5));
  });

  it('each half keeps its own colour away from the seam, over a 2 x SEAM_EM cross-fade', () => {
    expect(stops(head['--seg-fill']!)[0]).toBe(A.bg);
    expect(to['--seg-fill']).toContain(`${C.bg} ${SEAM_EM}em`);
    expect(head['--seg-fill']).toContain(`${A.bg} calc(100% - ${SEAM_EM}em)`);
    expect(head['--pill-bg']).toBe(A.bg);
  });
});

describe('prose segments (RS-BLEND, RS-NODOTS)', () => {
  it('a segment style carries its words and its glow\'s wash, and no dot', () => {
    const A = prosePillColorsOf('#ffe600', 'light');
    const B = prosePillColorsOf('#1f4fff', 'light');
    const style = proseSegmentStyle(A, { left: null, right: B }, 'light') as Record<string, string>;
    expect(style['--pill-tint']).toBe(proseTintOf(A.bg, pillThemeOf('light').panel).tint);
    expect(style['--pill-text']).toBe(A.text);
    expect(style['--pill-dot']).toBeUndefined();
    expect(style['--seg-tint']).toBeUndefined();
  });

  it('beside a stronger glow, its words read on the blend toward it', () => {
    // A near-black line's glow beside a near-white line's on the dark theme:
    // the blend is lighter than the near-black glow its words were set for.
    const black = prosePillColorsOf('#101010', 'dark');
    const white = prosePillColorsOf('#f6f6f2', 'dark');
    const style = proseSegmentStyle(black, { left: null, right: white }, 'dark') as Record<string, string>;
    const seam = mixHex(black.bg, white.bg, 0.5);
    expect(contrastOfHex(black.muted, seam)).toBeLessThan(4.5);
    expect(contrastOfHex(style['--pill-muted']!, seam)).toBeGreaterThanOrEqual(4.5);
    expect(contrastOfHex(style['--pill-text']!, seam)).toBeGreaterThanOrEqual(7);
  });
});

/**
 * RS-ARROW: the blend between the from and the to half is centred on the
 * arrow: the arrow is a bridge segment whose colour runs from the from
 * half's colour (its left edge) to the to half's (its right edge), the
 * midpoint at its middle; the halves keep their own colour up to it.
 * Injection: a bridge with its stops reversed fails "runs from the from
 * half's colour to the to half's".
 */
describe('the arrow bridge (RS-ARROW)', () => {
  const L = pillColorsOf('#ffe600', 'light');
  const R = pillColorsOf('#1f4fff', 'light');
  const arrow = linkBridgeStyle(L, R, 'light') as Record<string, string>;
  const from = linkSegmentStyle(L, { left: null, right: L }) as Record<string, string>;
  const to = linkSegmentStyle(R, { left: R, right: null }) as Record<string, string>;

  it("runs from the from half's colour to the to half's, the midpoint on the arrow's middle", () => {
    for (const [key, pick] of [
      ['--seg-fill', (c: typeof L) => c.bg],
      ['--seg-fill-hover', (c: typeof L) => c.hoverBg],
      ['--seg-edge', (c: typeof L) => c.border],
      ['--seg-edge-hover', (c: typeof L) => c.hoverBorder],
    ] as const) {
      const s = stops(arrow[key]!);
      expect(s[0]).toBe(pick(L));
      expect(s[2]).toBe(mixHex(pick(L), pick(R), 0.5));
      expect(lastOf(s)).toBe(pick(R));
      // The halves meet it in their own colour.
      expect(lastOf(stops(from[key]!))).toBe(pick(L));
      expect(stops(to[key]!)[0]).toBe(pick(R));
    }
    expect(arrow['--seg-fill']).toContain(`${mixHex(L.bg, R.bg, 0.5)} 50.00%`);
  });

});
