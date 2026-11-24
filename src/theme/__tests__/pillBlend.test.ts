/**
 * RS-BLEND: two-tone pill segments blend at their seams. Each segment's fill
 * and border fade from its own colour to the OKLab midpoint with its
 * neighbour over the last 0.4em at a seam, so neighbours meet in the same
 * colour (no seam line), at rest and on hover; a prose segment's tint
 * shifts to the same midpoint and fades around the seam, never to nothing
 * at it (no dip). Injections: a segment ending in its own colour at a seam
 * fails "neighbours meet in one colour"; a radial fade centred on the
 * segment's middle fails "the prose fade is centred on the seam".
 */
import { describe, expect, it } from 'vitest';
import { linkBridgeStyle, linkSegmentStyle, mixHex, proseBridgeTint, proseSegmentStyle, proseSegmentTint, SEAM_EM } from '../pillBlend';
import { pillColorsOf, pillThemeOf, prosePillColorsOf } from '../pillColors';

const stops = (gradient: string) => {
  const inner = /linear-gradient\(to right, (.*)\)$/.exec(gradient)?.[1] ?? '';
  return inner.split(/,\s(?=#)/).map((s) => s.trim().split(' ')[0]!);
};
const lastOf = (xs: string[]) => xs[xs.length - 1];

describe('link segments (RS-BLEND)', () => {
  const A = pillColorsOf('#f6f6f2', 'dark');
  const B = pillColorsOf('#101010', 'dark');
  const C = pillColorsOf('#ffe600', 'dark');
  const head = linkSegmentStyle(A, { left: null, right: B }, 'dark') as Record<string, string>;
  const from = linkSegmentStyle(B, { left: A, right: C }, 'dark') as Record<string, string>;
  const to = linkSegmentStyle(C, { left: B, right: null }, 'dark') as Record<string, string>;

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

describe('prose segments (RS-BLEND)', () => {
  const panel = pillThemeOf('light').panel;
  const A = prosePillColorsOf('#ffe600', 'light');
  const B = prosePillColorsOf('#1f4fff', 'light');
  const svg = (url: string) => decodeURIComponent(/data:image\/svg\+xml,(.*)"\)$/.exec(url)?.[1] ?? '');

  it('the tint shifts to the midpoint at the seam, and the fade is centred on the seam', () => {
    const left = svg(proseSegmentTint(A, { left: null, right: B }, panel, 0.3));
    const right = svg(proseSegmentTint(B, { left: A, right: null }, panel, 0.3));
    const middle = svg(proseSegmentTint(B, { left: A, right: A }, panel, 0.3));
    // The radial fade's centre is the seam: the right edge, the left edge.
    expect(left).toMatch(/radialGradient id='f' cx='1'/);
    expect(right).toMatch(/radialGradient id='f' cx='0'/);
    // Between two seams, only the top and bottom fade.
    expect(middle).toMatch(/linearGradient id='f' x1='0' y1='0' x2='0' y2='1'/);
    // Both sides of the seam stop in the same overlay.
    const last = /<stop offset='1\.0000' stop-color='([^']+)' stop-opacity='([^']+)'\/><\/linearGradient>/.exec(left);
    const first = /<linearGradient id='c'[^>]*><stop offset='0\.0000' stop-color='([^']+)' stop-opacity='([^']+)'/.exec(right);
    expect(last?.slice(1)).toEqual(first?.slice(1));
  });

  it('a segment style carries its tint, text and dot', () => {
    const style = proseSegmentStyle(A, { left: null, right: B }, 'light', 3) as Record<string, string>;
    expect(style['--seg-tint']).toMatch(/^url\("data:image\/svg\+xml,/);
    expect(style['--pill-text']).toBe(A.text);
    expect(style['--pill-dot']).toBe(A.dot);
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
  const from = linkSegmentStyle(L, { left: null, right: L }, 'light') as Record<string, string>;
  const to = linkSegmentStyle(R, { left: R, right: null }, 'light') as Record<string, string>;

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

  it('a prose bridge fades only top and bottom between its halves', () => {
    const panel = pillThemeOf('light').panel;
    const svg = decodeURIComponent(
      /data:image\/svg\+xml,(.*)"\)$/.exec(proseBridgeTint(L, R, panel, { left: true, right: true }))?.[1] ?? '',
    );
    expect(svg).toMatch(/linearGradient id='f' x1='0' y1='0' x2='0' y2='1'/);
    const half = svg.match(/stop offset='0\.5000'/g);
    expect(half).toHaveLength(1);
  });
});
