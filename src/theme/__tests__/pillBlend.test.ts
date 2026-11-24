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
import { linkSegmentStyle, mixHex, proseSegmentStyle, proseSegmentTint, SEAM_EM } from '../pillBlend';
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
